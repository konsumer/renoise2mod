//! MOD (ProTracker) output writer. Mirrors `ModConverter.cs`/`ModUtils.cs`/`ModCommands.cs`/
//! `ModCommonBase.cs`.

mod encoder;
mod period;

use crate::common;
use crate::error::{Error, Result};
use crate::model::{InstrumentData, PatternData, SampleData, SampleFreqMode, SongData};

pub use encoder::ModSettings;
pub use period::ProTrackerCompatibility;

const MIN_CHANNELS: usize = 4;
const MAX_INSTRUMENTS: usize = 31;
const MAX_SAMPLE_LENGTH_MOD: usize = 65536;
const NUM_ROWS_PER_PATTERN: usize = 64;
const PATTERN_SEQUENCE_SIZE: usize = 128;
const SAMPLE_INFO_BLOCK_SIZE: usize = 930;
const SAMPLE_HEADER_SIZE: usize = 30;

/// Bugfix (agreed): the original hardcodes this to 0 for MOD (master-track commands are parsed
/// for XM but never for MOD, seemingly an oversight given XM's equivalent constant is 1). MOD now
/// gets the same 1-column handling XM already had.
const MAX_MASTER_TRACK_COLUMNS_TO_PARSE: usize = 1;

const PAL_FREQ: f64 = 7093789.2;
const NTSC_FREQ: f64 = 7159090.0;

#[derive(Debug, Clone, Copy)]
pub enum LogLevel {
    Info,
    Warning,
    Error,
}

pub trait ConversionLog {
    fn log(&mut self, level: LogLevel, message: String);
}

pub struct NullLog;
impl ConversionLog for NullLog {
    fn log(&mut self, _level: LogLevel, _message: String) {}
}

/// Decoded, resampled, MOD-encoded (mono, 8-bit, sign-flipped, ProTracker-compatibility-adjusted)
/// sample data plus the metadata needed to compute period/loop header fields.
pub struct EncodedSample {
    pub encoded_pcm: Vec<u8>,
    /// The sample rate actually resampled to (see [`resolve_sample_rate`]).
    pub sample_rate: u32,
    /// Decoded (pre-resample) source length, in bytes at the source bit depth/channel count.
    pub original_length_bytes: i64,
    pub original_channels: u32,
    pub original_bits_per_sample: u32,
}

/// Supplies decoded+encoded PCM for one instrument's first sample. Implemented for real by
/// `audio.rs`; a stub implementation is used in this crate's own tests to validate the
/// binary-layout/pattern-encoding logic independent of real audio decoding.
pub trait ModSampleSource {
    /// `Ok(None)` means the instrument genuinely has no usable sample (matches the original's
    /// `FORMAT.NONE` case: silence, not an error).
    fn encode_sample(
        &self,
        instrument_index: usize,
        sample: &SampleData,
        settings: &ModSettings,
    ) -> Result<Option<EncodedSample>>;
}

/// Resolves a [`SampleFreqMode`] + the sample's original (decoded) rate into the actual rate to
/// resample to. Mirrors the rate-selection block in `ModConverter.GetAllSamplesData`.
pub fn resolve_sample_rate(
    mode: &SampleFreqMode,
    original_rate: u32,
    ntsc_mode: bool,
    pt_compat: ProTrackerCompatibility,
) -> Result<u32> {
    let sys_freq = if ntsc_mode { NTSC_FREQ } else { PAL_FREQ };
    let note_index_max = if pt_compat == ProTrackerCompatibility::A3Max {
        period::NOTE_VALUE_A3
    } else {
        period::NOTE_VALUE_B3
    };

    let mut freq_from_setting: i64 = 0;
    let mut note_index: i32 = 0;
    let mut note_period: i32 = 0;

    match mode {
        SampleFreqMode::Low => note_index = period::NOTE_VALUE_C2,
        SampleFreqMode::High => note_index = period::NOTE_VALUE_C3,
        SampleFreqMode::Maximum => note_index = note_index_max,
        SampleFreqMode::Original => freq_from_setting = original_rate as i64,
        SampleFreqMode::NoteName(name) => {
            note_period = encoder::ModEncoder::mod_note_period_from_name(name)?;
        }
        SampleFreqMode::Hz(hz) => freq_from_setting = *hz as i64,
    }

    let sample_rate = if freq_from_setting > 0 {
        freq_from_setting as u32
    } else if note_index > 0 {
        (sys_freq / (period::PERIODS_RANGE[note_index as usize] as f64 * 2.0)).round() as u32
    } else if note_period > 0 {
        (sys_freq / (note_period as f64 * 2.0)).round() as u32
    } else {
        (sys_freq / (period::PERIODS_RANGE[period::NOTE_VALUE_C2 as usize] as f64 * 2.0)).round()
            as u32
    };

    Ok(sample_rate)
}

pub fn convert(
    song: &SongData,
    settings: &ModSettings,
    samples: &dyn ModSampleSource,
    log: &mut dyn ConversionLog,
) -> Result<Vec<u8>> {
    settings.validate()?;
    check_requirements(song)?;

    let mut encoder = encoder::ModEncoder::new(
        song.instruments.len(),
        6,
        settings.clone(),
        song.playback_engine_version,
        song.pitch_compatibility_mode,
        song.sample_offset_compatibility_mode,
    );

    if song.pattern_order_table.len() > PATTERN_SEQUENCE_SIZE {
        log.log(
            LogLevel::Warning,
            format!(
                "Pattern order table has {} entries; MOD only supports {}, extra entries will be dropped",
                song.pattern_order_table.len(),
                PATTERN_SEQUENCE_SIZE
            ),
        );
    }

    let name_data = write_name_bytes(&song.name, 20);
    let (sample_info, sample_pcm) =
        get_all_samples_data(&song.instruments, settings, samples, &mut encoder, log)?;
    let song_len_data =
        get_song_length_data(song.pattern_order_table.len().min(PATTERN_SEQUENCE_SIZE));
    let pattern_sequence = get_pattern_sequence_data(&song.pattern_order_table);
    let channels_data = write_name_bytes(&channels_tag(song.num_channels), 4);
    let patterns_data = get_all_patterns_data(
        &song.patterns,
        song.num_channels,
        song.num_master_track_columns,
        &pattern_sequence,
        song.playback_engine_version,
        &mut encoder,
        log,
    )?;

    let mut out = Vec::new();
    out.extend_from_slice(&name_data);
    out.extend_from_slice(&sample_info[..SAMPLE_INFO_BLOCK_SIZE]);
    out.extend_from_slice(&song_len_data);
    out.extend_from_slice(&pattern_sequence);
    out.extend_from_slice(&channels_data);
    out.extend_from_slice(&patterns_data);
    out.extend_from_slice(&sample_pcm);

    Ok(out)
}

fn check_requirements(song: &SongData) -> Result<()> {
    if song.num_channels < MIN_CHANNELS {
        return Err(Error::Conversion(
            "MOD format requires a minimum of 4 channels".to_string(),
        ));
    }
    Ok(())
}

/// Truncates (or zero-pads) `s` to exactly `len` bytes. Mirrors `Utility.GetBytesFromString`.
/// Deliberately used as-is for the channel-count tag too: `"16CHN"` truncating to `"16CH"` is
/// what makes multi-digit channel counts match the real MOD spec's two-digit tag convention --
/// that's the original's approach and it happens to work, not a coincidence worth "fixing".
fn write_name_bytes(s: &str, len: usize) -> Vec<u8> {
    let mut bytes: Vec<u8> = s.bytes().take(len).collect();
    bytes.resize(len, 0);
    bytes
}

fn channels_tag(num_channels: usize) -> String {
    if num_channels == 4 {
        "M.K.".to_string()
    } else {
        format!("{num_channels}CHN")
    }
}

fn get_song_length_data(num_patterns: usize) -> [u8; 2] {
    // The restart-position byte is hardcoded to 0x7F regardless of the song's actual restart
    // position -- a deliberate quirk of the original (kept, not one of the agreed fixes).
    [num_patterns as u8, 0x7F]
}

fn get_pattern_sequence_data(pattern_order_table: &[u8]) -> [u8; PATTERN_SEQUENCE_SIZE] {
    let mut data = [0u8; PATTERN_SEQUENCE_SIZE];
    for (i, &v) in pattern_order_table
        .iter()
        .take(PATTERN_SEQUENCE_SIZE)
        .enumerate()
    {
        data[i] = v;
    }
    data
}

#[allow(clippy::too_many_arguments)]
fn get_all_samples_data(
    instruments: &[InstrumentData],
    settings: &ModSettings,
    samples: &dyn ModSampleSource,
    encoder: &mut encoder::ModEncoder,
    log: &mut dyn ConversionLog,
) -> Result<(Vec<u8>, Vec<u8>)> {
    let total_instruments = instruments.len().min(MAX_INSTRUMENTS);
    if instruments.len() > MAX_INSTRUMENTS {
        log.log(
            LogLevel::Warning,
            format!(
                "Song has {} instruments; MOD only supports {}, extra instruments will be dropped",
                instruments.len(),
                MAX_INSTRUMENTS
            ),
        );
    }

    let mut sample_info = vec![0u8; SAMPLE_INFO_BLOCK_SIZE];
    // Pre-initialize every slot's loop-length low byte to 1 ("avoid crashes in Protracker" for
    // blank/unused slots), matching the original.
    for i in (0..sample_info.len()).step_by(SAMPLE_HEADER_SIZE) {
        sample_info[i + 29] = 1;
    }

    let mut pcm = Vec::new();
    let mut offset = 0usize;

    #[allow(clippy::needless_range_loop)] // `ci` is used well beyond just indexing `instruments`.
    for ci in 0..total_instruments {
        let instrument = &instruments[ci];
        log.log(
            LogLevel::Info,
            format!(
                "Processing Sample {}/{} - {}",
                ci + 1,
                total_instruments,
                instrument.name
            ),
        );

        if instrument.samples.len() > 1 {
            log.log(
                LogLevel::Error,
                format!("More samples detected on instrument {}", ci + 1),
            );
        }

        if instrument.samples.is_empty() {
            offset += SAMPLE_HEADER_SIZE;
            continue;
        }

        let sample = &instrument.samples[0];
        let sample_data = match process_one_sample(ci, sample, settings, samples, encoder) {
            Ok(pcm_bytes) => pcm_bytes,
            Err(e) => {
                log.log(LogLevel::Error, e.to_string());
                Vec::new()
            }
        };

        sample_info[offset..offset + 22].copy_from_slice(&write_name_bytes(&instrument.name, 22));
        offset += 22;

        write_u16_be(&mut sample_info, offset, (sample_data.len() / 2) as u16);
        offset += 2;

        sample_info[offset] = ((encoder.sample_fine_tune(ci) >> 4) & 0x0F) as u8;
        offset += 1;

        sample_info[offset] = sample.default_volume;
        offset += 1;

        if !sample_data.is_empty() && sample.loop_mode != crate::model::LoopMode::Off {
            let loop_start = encoder.get_loop_value(sample.loop_start, ci) as u16;
            let loop_len = encoder.get_loop_value(sample.loop_end - sample.loop_start, ci) as u16;
            write_u16_be(&mut sample_info, offset, loop_start);
            offset += 2;
            write_u16_be(&mut sample_info, offset, loop_len);
            offset += 2;
        } else {
            offset += 4;
        }

        pcm.extend_from_slice(&sample_data);
    }

    Ok((sample_info, pcm))
}

fn write_u16_be(buf: &mut [u8], offset: usize, value: u16) {
    buf[offset] = (value >> 8) as u8;
    buf[offset + 1] = (value & 0xFF) as u8;
}

fn process_one_sample(
    instrument_index: usize,
    sample: &SampleData,
    settings: &ModSettings,
    samples: &dyn ModSampleSource,
    encoder: &mut encoder::ModEncoder,
) -> Result<Vec<u8>> {
    let Some(encoded) = samples.encode_sample(instrument_index, sample, settings)? else {
        return Ok(Vec::new());
    };

    encoder.store_sample_info(
        instrument_index,
        encoded.original_length_bytes,
        encoded.encoded_pcm.len() as i64,
        encoded.sample_rate,
        encoded.original_channels,
        encoded.original_bits_per_sample,
        sample.rel_note_number as i32,
        sample.fine_tune as i32,
        sample.transpose as i32,
    );

    if encoded.encoded_pcm.len() > MAX_SAMPLE_LENGTH_MOD {
        return Err(Error::Conversion(format!(
            "Sample number {} is too large: max size for mod is {}. Current length is {}",
            instrument_index + 1,
            MAX_SAMPLE_LENGTH_MOD,
            encoded.encoded_pcm.len()
        )));
    }

    Ok(encoded.encoded_pcm)
}

#[allow(clippy::too_many_arguments)]
fn get_all_patterns_data(
    patterns: &[PatternData],
    num_channels: usize,
    num_master_track_columns: usize,
    pattern_sequence_buffer: &[u8; PATTERN_SEQUENCE_SIZE],
    playback_engine_version: i32,
    encoder: &mut encoder::ModEncoder,
    log: &mut dyn ConversionLog,
) -> Result<Vec<u8>> {
    // The original computes this over the *padded* 128-byte sequence buffer (not the raw pattern
    // order table), so trailing zero padding means pattern 0 is included here unless the song
    // uses all 128 sequence slots. It then walks the distinct pattern index *values* using them
    // both as "which pattern to fetch" and "byte offset to seek to" -- which only produces a
    // valid, in-bounds result when the referenced pattern indices happen to form a complete
    // `0..=max` set. A song whose pattern order skips an index entirely (e.g. uses patterns
    // {0, 1, 5} but never 2/3/4) makes the original crash with an out-of-range array access. This
    // rewrite achieves byte-identical output for every case that doesn't crash (each distinct
    // pattern index's data always lands at `pattern_index * block_size` regardless of visitation
    // order) while not crashing on the sparse case -- in the same spirit as the already-agreed
    // fix for the >128-entries overflow.
    let mut distinct: Vec<u8> = Vec::new();
    for &v in pattern_sequence_buffer.iter() {
        if !distinct.contains(&v) {
            distinct.push(v);
        }
    }
    let max_pattern = *distinct.iter().max().unwrap_or(&0) as usize;

    let mut out: Vec<u8> = Vec::new();
    for &pattern_index in &distinct {
        let pattern_index = pattern_index as usize;
        let pattern = patterns.get(pattern_index).ok_or_else(|| {
            Error::Conversion(format!(
                "pattern order table references nonexistent pattern {pattern_index}"
            ))
        })?;

        log.log(
            LogLevel::Info,
            format!("Processing pattern {pattern_index}/{max_pattern}"),
        );

        let data = get_pattern_data(
            pattern,
            num_channels,
            num_master_track_columns,
            playback_engine_version,
            encoder,
            log,
        )?;

        let byte_offset = data.len() * pattern_index;
        if out.len() < byte_offset + data.len() {
            out.resize(byte_offset + data.len(), 0);
        }
        out[byte_offset..byte_offset + data.len()].copy_from_slice(&data);
    }

    Ok(out)
}

fn get_pattern_data(
    pattern: &PatternData,
    num_channels: usize,
    num_master_track_columns: usize,
    playback_engine_version: i32,
    encoder: &mut encoder::ModEncoder,
    log: &mut dyn ConversionLog,
) -> Result<Vec<u8>> {
    if pattern.num_rows > NUM_ROWS_PER_PATTERN {
        log.log(
            LogLevel::Warning,
            format!(
                "Pattern has {} rows; MOD patterns are fixed at {} rows, extra rows will be dropped",
                pattern.num_rows, NUM_ROWS_PER_PATTERN
            ),
        );
    }

    let max_track_lines = NUM_ROWS_PER_PATTERN * num_channels;
    let mut data = vec![0u8; NUM_ROWS_PER_PATTERN * 4 * num_channels];
    let cycles_to_do = (pattern.num_rows * num_channels).min(max_track_lines);

    let num_master_track_columns_to_parse =
        MAX_MASTER_TRACK_COLUMNS_TO_PARSE.min(num_master_track_columns);

    let mut master_track_command: (u8, u8) = (0, 0);
    let mut is_master_track_command_used = false;
    let mut current_master_track_index = 0usize;
    let mut master_track_index_limit = 0usize;

    for i in 0..cycles_to_do {
        let current_row = i / num_channels;
        let current_channel = i % num_channels;

        if current_channel == 0 {
            let new_ticks = common::compute_ticks_per_row_for_line(
                pattern,
                current_row,
                num_channels,
                num_master_track_columns,
                playback_engine_version,
                encoder.ticks_per_row(),
            );
            encoder.set_ticks_per_row(new_ticks);

            if is_master_track_command_used {
                log.log(
                    LogLevel::Error,
                    format!(
                        "row {current_row}, channel {}: Some MasterTrack command were not used due to missing free command effects slots",
                        current_channel + 1
                    ),
                );
            }
            is_master_track_command_used = false;
            current_master_track_index = current_row * num_master_track_columns;
            master_track_index_limit =
                current_master_track_index + num_master_track_columns_to_parse;
        }

        while current_master_track_index < master_track_index_limit && !is_master_track_command_used
        {
            if let Some(mt_cell) = pattern
                .master_track_line_data
                .get(current_master_track_index)
            {
                if !mt_cell.effect_number.is_empty() {
                    if let Ok(cmd) = encoder.get_commands_from_master_track(
                        &mt_cell.effect_number,
                        &mt_cell.effect_value,
                    ) {
                        if cmd.0 as u32 + cmd.1 as u32 > 0 {
                            master_track_command = cmd;
                            is_master_track_command_used = true;
                        }
                    }
                }
            }
            current_master_track_index += 1;
        }

        let cell = &pattern.tracks_line_data[i];
        let offset = i * 4;

        let mut sample_number: i32 = 0;
        let mut mod_period: i32 = 0;
        let mut effect_num: i32 = 0;
        let mut effect_val: i32 = 0;
        let mut is_effect_command_used = false;

        if cell.is_set || is_master_track_command_used {
            if let Some(instrument) = &cell.instrument {
                if let Ok(v) = i32::from_str_radix(instrument, 16) {
                    sample_number = v + 1;
                }
            }

            if let Some(note) = &cell.note {
                match encoder::ModEncoder::is_tone_portamento_triggered(
                    non_empty(&cell.effect_number),
                    cell.volume.as_deref(),
                    cell.panning.as_deref(),
                ) {
                    Ok(is_tone_portamento_triggered) => {
                        match encoder.trigger_mod_note(
                            note,
                            sample_number - 1,
                            current_channel,
                            is_tone_portamento_triggered,
                        ) {
                            Ok(period) => mod_period = period,
                            Err(e) => log.log(
                                LogLevel::Error,
                                format!(
                                    "row {current_row}, instrument {}, channel {}: {e}",
                                    sample_number - 1,
                                    current_channel + 1
                                ),
                            ),
                        }
                    }
                    Err(e) => log.log(
                        LogLevel::Error,
                        format!(
                            "row {current_row}, instrument {}, channel {}: {e}",
                            sample_number - 1,
                            current_channel + 1
                        ),
                    ),
                }
            }

            if !cell.effect_number.is_empty() {
                match encoder.get_mod_effect(
                    &cell.effect_number,
                    &cell.effect_value,
                    sample_number,
                    current_channel,
                    mod_period != 0,
                ) {
                    Ok((n, v)) if n as u32 + v as u32 > 0 => {
                        is_effect_command_used = true;
                        effect_num = n as i32;
                        effect_val = v as i32;
                    }
                    Ok(_) => {}
                    Err(e) => log.log(
                        LogLevel::Error,
                        format!("row {current_row}, channel {}: {e}", current_channel + 1),
                    ),
                }
            }

            if !is_effect_command_used {
                if let Some(volume) = &cell.volume {
                    let (n, v) = encoder.transpose_volume_to_command_effect(
                        volume,
                        sample_number,
                        current_channel,
                        mod_period != 0,
                    );
                    if n as u32 + v as u32 > 0 {
                        is_effect_command_used = true;
                        effect_num = n as i32;
                        effect_val = v as i32;
                    }
                }
                if !is_effect_command_used {
                    if let Some(delay) = &cell.delay {
                        let (n, v) = encoder.transpose_delay_to_command_effect(delay);
                        if n as u32 + v as u32 > 0 {
                            is_effect_command_used = true;
                            effect_num = n as i32;
                            effect_val = v as i32;
                        }
                    }
                }
                if !is_effect_command_used {
                    if let Some(panning) = &cell.panning {
                        let (n, v) = encoder.transpose_panning_to_command_effect(
                            panning,
                            sample_number,
                            current_channel,
                            mod_period != 0,
                        );
                        if n as u32 + v as u32 > 0 {
                            is_effect_command_used = true;
                            effect_num = n as i32;
                            effect_val = v as i32;
                        }
                    }
                }
                if is_master_track_command_used && !is_effect_command_used {
                    is_effect_command_used = true;
                    effect_num = master_track_command.0 as i32;
                    effect_val = master_track_command.1 as i32;
                    is_master_track_command_used = false;
                }
            }
        }
        let _ = is_effect_command_used;

        data[offset] = ((sample_number & 0xF0) | ((mod_period & 0xF00) >> 8)) as u8;
        data[offset + 1] = (mod_period & 0xFF) as u8;
        data[offset + 2] = (((sample_number & 0xF) << 4) | effect_num) as u8;
        data[offset + 3] = effect_val as u8;
    }

    Ok(data)
}

fn non_empty(s: &str) -> Option<&str> {
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}
