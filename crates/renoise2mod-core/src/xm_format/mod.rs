//! XM (FastTracker II) output writer. Mirrors `XMConverter.cs`/`XMUtils.cs`/`XMCommands.cs`/
//! `XMExtras.cs`.

mod commands;
mod encoder;
mod extras;

use crate::common::{self, VolumeScalingMode};
use crate::error::Result;
use crate::model::{InstrumentData, PatternData, SampleData, SongData};

pub use crate::mod_format::{ConversionLog, LogLevel};

const MAX_ENV_POINTS: usize = 12;
const INSTRUMENT_HEADER_SIZE: usize = 0x107;
const SAMPLE_HEADER_SIZE: usize = 40;

/// Master-track column parsing is capped at 1 for XM (the original already had this right; MOD
/// was fixed to match it -- see `mod_format`).
const MAX_MASTER_TRACK_COLUMNS_TO_PARSE: usize = 1;

#[derive(Debug, Clone)]
pub struct XmSettings {
    pub ticks_row: i32,
    pub tempo: i32,
    pub volume_scaling_mode: VolumeScalingMode,
}

/// Decoded+encoded PCM for one sample, ready to drop into an XM sample slot. Unlike MOD, XM keeps
/// the source's native sample rate and channel count -- only bit depth is clamped (to 8 or 16).
pub struct EncodedXmSample {
    pub encoded_pcm: Vec<u8>,
    pub sample_rate: u32,
    pub channels: u32,
    pub bits_per_sample: u32,
}

pub trait XmSampleSource {
    /// `Ok(None)` means the sample genuinely has no usable audio (silence, not an error).
    fn encode_sample(
        &self,
        instrument_index: usize,
        sample_index: usize,
        sample: &SampleData,
        settings: &XmSettings,
    ) -> Result<Option<EncodedXmSample>>;
}

pub fn convert(
    song: &SongData,
    settings: &XmSettings,
    samples: &dyn XmSampleSource,
    log: &mut dyn ConversionLog,
) -> Result<Vec<u8>> {
    let key_maps: Vec<[Option<u8>; 120]> = song.instruments.iter().map(|i| i.key_map).collect();
    let sample_counts: Vec<usize> = song.instruments.iter().map(|i| i.samples.len()).collect();
    let mut encoder = encoder::XmEncoder::new(
        &key_maps,
        &sample_counts,
        settings.ticks_row,
        song.playback_engine_version,
        song.pitch_compatibility_mode,
        song.sample_offset_compatibility_mode,
    );

    log.log(LogLevel::Info, "Processing XM Header".to_string());
    let header = get_xm_header_data(song, settings);

    let instruments_data =
        get_all_instruments_data(&song.instruments, settings, samples, &mut encoder, log)?;

    let patterns_data = get_all_patterns_data(
        &song.patterns,
        &song.instruments,
        song.num_channels,
        song.num_master_track_columns,
        song.playback_engine_version,
        settings,
        &mut encoder,
        log,
    )?;

    let mut out = Vec::with_capacity(header.len() + patterns_data.len() + instruments_data.len());
    out.extend_from_slice(&header);
    out.extend_from_slice(&patterns_data);
    out.extend_from_slice(&instruments_data);
    Ok(out)
}

fn write_name_bytes(s: &str, len: usize) -> Vec<u8> {
    let mut bytes: Vec<u8> = s.bytes().take(len).collect();
    bytes.resize(len, 0);
    bytes
}

fn get_xm_header_data(song: &SongData, settings: &XmSettings) -> Vec<u8> {
    const ID_TEXT: &str = "Extended Module: ";
    const PROG_NAME: &str = "Xrns2Mod";
    const HEADER_SIZE_FIELD: u32 = 80 - 60 + 256;

    let mut h = vec![0u8; 80 + 256];

    h[0..17].copy_from_slice(&write_name_bytes(ID_TEXT, 17));
    h[17..37].copy_from_slice(&write_name_bytes(&song.name, 20));
    h[37] = 0x1A;
    h[38..58].copy_from_slice(&write_name_bytes(PROG_NAME, 20));
    h[58] = 4;
    h[59] = 1;
    h[60..64].copy_from_slice(&HEADER_SIZE_FIELD.to_le_bytes());
    h[64..66].copy_from_slice(&(song.pattern_order_table.len() as u16).to_le_bytes());
    h[66..68].copy_from_slice(&song.restart_position.to_le_bytes());
    h[68..70].copy_from_slice(&(song.num_channels as u16).to_le_bytes());
    h[70..72].copy_from_slice(&(song.patterns.len() as u16).to_le_bytes());
    h[72..74].copy_from_slice(&(song.instruments.len() as u16).to_le_bytes());
    h[74] = 1; // flags: bit0 = Linear Frequency Table
    h[76..78].copy_from_slice(&(settings.ticks_row as u16).to_le_bytes());
    h[78..80].copy_from_slice(&(settings.tempo as u16).to_le_bytes());

    let n = song.pattern_order_table.len().min(256);
    h[80..80 + n].copy_from_slice(&song.pattern_order_table[..n]);

    h
}

fn get_all_instruments_data(
    instruments: &[InstrumentData],
    settings: &XmSettings,
    samples: &dyn XmSampleSource,
    encoder: &mut encoder::XmEncoder,
    log: &mut dyn ConversionLog,
) -> Result<Vec<u8>> {
    let mut out = Vec::new();

    for (ci, instrument) in instruments.iter().enumerate() {
        out.extend_from_slice(&get_instrument_header_data(instrument));

        let mut encoded_samples: Vec<Vec<u8>> = Vec::with_capacity(instrument.samples.len());

        for (si, sample) in instrument.samples.iter().enumerate() {
            log.log(
                LogLevel::Info,
                format!(
                    "Processing instrument {}/{}, sample {}/{} ",
                    ci + 1,
                    instruments.len(),
                    si + 1,
                    instrument.samples.len()
                ),
            );

            let (encoded_pcm, base_note, fine_tune, bits_per_sample, channels, sample_rate) =
                match samples.encode_sample(ci, si, sample, settings) {
                    Ok(Some(encoded)) => {
                        encoder.store_sample_info(
                            ci,
                            si,
                            encoded.encoded_pcm.len() as i64,
                            encoded.sample_rate,
                            encoded.channels,
                            encoded.bits_per_sample,
                            sample.rel_note_number as i32,
                            sample.fine_tune as i32,
                            sample.transpose as i32,
                        );
                        (
                            encoded.encoded_pcm,
                            encoder.sample_base_note(ci, si),
                            encoder.sample_fine_tune(ci, si),
                            encoded.bits_per_sample as u8,
                            encoded.channels,
                            encoded.sample_rate,
                        )
                    }
                    Ok(None) => (Vec::new(), 0, 0, 8, 1, 0),
                    Err(e) => {
                        log.log(LogLevel::Error, e.to_string());
                        (Vec::new(), 0, 0, 8, 1, 0)
                    }
                };

            let header = get_sample_header_data(
                sample,
                base_note,
                fine_tune,
                encoded_pcm.len() as i32,
                bits_per_sample,
                channels,
                sample_rate,
            );
            out.extend_from_slice(&header);
            encoded_samples.push(encoded_pcm);
        }

        for pcm in encoded_samples {
            out.extend_from_slice(&pcm);
        }
    }

    Ok(out)
}

#[allow(clippy::too_many_arguments)]
fn get_sample_header_data(
    sample: &SampleData,
    base_note: i32,
    fine_tune: i32,
    sample_len: i32,
    bits_per_sample: u8,
    channels: u32,
    _sample_rate: u32,
) -> [u8; SAMPLE_HEADER_SIZE] {
    let mut h = [0u8; SAMPLE_HEADER_SIZE];
    let is_stereo = channels > 1;

    h[0..4].copy_from_slice(&sample_len.to_le_bytes());
    h[4..8].copy_from_slice(
        &encoder::get_sample_loop_value(sample.loop_start, bits_per_sample as u32, is_stereo)
            .to_le_bytes(),
    );
    h[8..12].copy_from_slice(
        &encoder::get_sample_loop_value(
            sample.loop_end - sample.loop_start,
            bits_per_sample as u32,
            is_stereo,
        )
        .to_le_bytes(),
    );
    h[12] = sample.default_volume;
    h[13] = fine_tune as i8 as u8;

    let loop_mode = encoder::get_sample_loop_mode(sample.loop_mode);
    // Bugfix (agreed): a proper 0x10 flag for 16-bit instead of adding the raw bit-depth value
    // (which for 8-bit samples set a stray, undefined bit, and only "worked" for 16-bit samples
    // because 16 decimal happens to equal 0x10 hex).
    let bit_depth_flag = if bits_per_sample == 16 { 0x10 } else { 0 };
    h[14] = loop_mode | bit_depth_flag | if is_stereo { 0x20 } else { 0 };

    h[15] = encoder::get_panning(sample.panning);
    h[16] = base_note as i8 as u8;
    h[17] = 0; // reserved
    h[18..40].copy_from_slice(&write_name_bytes(&sample.name, 22));

    h
}

fn get_instrument_header_data(instrument: &InstrumentData) -> [u8; INSTRUMENT_HEADER_SIZE] {
    let mut h = [0u8; INSTRUMENT_HEADER_SIZE];

    h[0..4].copy_from_slice(&(INSTRUMENT_HEADER_SIZE as u32).to_le_bytes());
    h[4..26].copy_from_slice(&write_name_bytes(&instrument.name, 22));
    h[26] = 0; // instrument type

    let num_samples = instrument.samples.len();
    h[27..29].copy_from_slice(&(num_samples as u16).to_le_bytes());

    if num_samples > 0 {
        h[29..33].copy_from_slice(&0x28u32.to_le_bytes());
    }

    // Renoise notes 96-119 are silently dropped -- XM's real keymap table is genuinely fixed at
    // 96 bytes, this is a format limit, not a bug.
    for i in 0..96 {
        h[33 + i] = instrument.key_map[i].unwrap_or(0);
    }

    let vol_points = encoder::build_envelope_points(
        &instrument.volume_envelope.points,
        instrument.volume_envelope.sustain_point_x,
        instrument.volume_envelope.loop_start_x,
        instrument.volume_envelope.loop_end_x,
        instrument.volume_envelope.sustain_enabled,
        instrument.volume_envelope.loop_enabled,
    );
    let pan_points = encoder::build_envelope_points(
        &instrument.panning_envelope.points,
        instrument.panning_envelope.sustain_point_x,
        instrument.panning_envelope.loop_start_x,
        instrument.panning_envelope.loop_end_x,
        instrument.panning_envelope.sustain_enabled,
        instrument.panning_envelope.loop_enabled,
    );

    let total_vol_points = vol_points.len().min(MAX_ENV_POINTS);
    let total_pan_points = pan_points.len().min(MAX_ENV_POINTS);

    write_envelope_points(&mut h[129..177], &vol_points[..total_vol_points]);
    write_envelope_points(&mut h[177..225], &pan_points[..total_pan_points]);

    h[225] = total_vol_points as u8;
    h[226] = total_pan_points as u8;

    h[227] = encoder::get_point_number(
        &vol_points,
        instrument.volume_envelope.sustain_point_x.round() as i32,
    );
    h[228] = encoder::get_point_number(
        &vol_points,
        instrument.volume_envelope.loop_start_x.round() as i32,
    );
    h[229] = encoder::get_point_number(
        &vol_points,
        instrument.volume_envelope.loop_end_x.round() as i32,
    );
    h[230] = encoder::get_point_number(
        &pan_points,
        instrument.panning_envelope.sustain_point_x.round() as i32,
    );
    h[231] = encoder::get_point_number(
        &pan_points,
        instrument.panning_envelope.loop_start_x.round() as i32,
    );
    h[232] = encoder::get_point_number(
        &pan_points,
        instrument.panning_envelope.loop_end_x.round() as i32,
    );

    h[233] = encoder::get_volume_panning_type(
        instrument.volume_envelope.enabled,
        instrument.volume_envelope.sustain_enabled,
        instrument.volume_envelope.loop_enabled,
    );
    h[234] = encoder::get_volume_panning_type(
        instrument.panning_envelope.enabled,
        instrument.panning_envelope.sustain_enabled,
        instrument.panning_envelope.loop_enabled,
    );

    h[239..241].copy_from_slice(&instrument.volume_envelope.fade_out.to_le_bytes());

    h
}

fn write_envelope_points(buf: &mut [u8], points: &[encoder::EnvelopePoint]) {
    for (i, &(x, y)) in points.iter().enumerate() {
        let offset = i * 4;
        if offset + 4 > buf.len() {
            break;
        }
        buf[offset..offset + 2].copy_from_slice(&(x as u16).to_le_bytes());
        buf[offset + 2..offset + 4].copy_from_slice(&(y as u16).to_le_bytes());
    }
}

#[allow(clippy::too_many_arguments)]
fn get_all_patterns_data(
    patterns: &[PatternData],
    instruments: &[InstrumentData],
    num_channels: usize,
    num_master_track_columns: usize,
    playback_engine_version: i32,
    settings: &XmSettings,
    encoder: &mut encoder::XmEncoder,
    log: &mut dyn ConversionLog,
) -> Result<Vec<u8>> {
    let mut out = Vec::new();

    for (i, pattern) in patterns.iter().enumerate() {
        log.log(
            LogLevel::Info,
            format!("Processing pattern {}/{}", i + 1, patterns.len()),
        );

        let data = get_pattern_data(
            pattern,
            instruments,
            num_channels,
            num_master_track_columns,
            playback_engine_version,
            settings,
            encoder,
            log,
        )?;
        let mut pattern_header = [0u8; 9];
        pattern_header[0] = 9;
        pattern_header[5..7].copy_from_slice(&(pattern.num_rows as u16).to_le_bytes());
        pattern_header[7..9].copy_from_slice(&(data.len() as u16).to_le_bytes());

        out.extend_from_slice(&pattern_header);
        out.extend_from_slice(&data);
    }

    Ok(out)
}

const NOTE_BIT: u8 = 1;
const INSTRUMENT_BIT: u8 = 2;
const VOLUME_COL_BIT: u8 = 4;
const EFFECT_TYPE_BIT: u8 = 8;
const EFFECT_PARAM_BIT: u8 = 16;
const EMPTY_BIT: u8 = 128;
const ALL_VALUES_FILLED_BIT: u8 =
    NOTE_BIT | INSTRUMENT_BIT | VOLUME_COL_BIT | EFFECT_TYPE_BIT | EFFECT_PARAM_BIT | EMPTY_BIT;

#[allow(clippy::too_many_arguments)]
fn get_pattern_data(
    pattern: &PatternData,
    instruments: &[InstrumentData],
    num_channels: usize,
    num_master_track_columns: usize,
    playback_engine_version: i32,
    settings: &XmSettings,
    encoder: &mut encoder::XmEncoder,
    log: &mut dyn ConversionLog,
) -> Result<Vec<u8>> {
    let mut out = Vec::new();

    // Tracks, per channel, which (instrument, sample) is currently playing there -- needed for
    // COLUMN volume scaling and default-volume lookups (a Carl Corcoran idea, per the original).
    let mut playing_samples: Vec<Option<(usize, usize)>> = vec![None; num_channels];

    let num_master_track_columns_to_parse =
        MAX_MASTER_TRACK_COLUMNS_TO_PARSE.min(num_master_track_columns);
    let mut master_track_command: (u8, u8) = (0, 0);
    let mut is_master_track_command_used = false;
    let mut current_master_track_index = 0usize;
    let mut master_track_index_limit = 0usize;

    let total = pattern.tracks_line_data.len();
    for i in 0..total {
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
                        false,
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

        if !cell.is_set && !is_master_track_command_used {
            out.push(EMPTY_BIT);
            continue;
        }

        let mut xm_note: u8 = 0;
        let mut xm_instrument: u8 = 0;
        let mut xm_volume: u8 = 0;
        let mut xm_effect_number: u8 = 0;
        let mut xm_effect_value: u8 = 0;
        let mut compression_value = EMPTY_BIT;

        let mut is_effect_command_used = false;
        let mut is_volume_command_used = false;
        let mut is_panning_command_used = false;

        if let Some(note) = &cell.note {
            match encoder::get_xm_note(note) {
                Ok(n) => {
                    xm_note = n;
                    compression_value += NOTE_BIT;
                }
                Err(e) => log.log(
                    LogLevel::Error,
                    format!("row {current_row}, channel {}: {e}", current_channel + 1),
                ),
            }
        }

        if let Some(instrument) = &cell.instrument {
            compression_value += INSTRUMENT_BIT;
            if let Ok(v) = i32::from_str_radix(instrument, 16) {
                xm_instrument = (v + 1) as u8;
            }
            if xm_note != 0 && xm_instrument != 0 {
                let xm_sample = encoder.played_sample_from_keymap(xm_note, xm_instrument);
                let instrument_idx = xm_instrument as usize - 1;
                if let Some(inst) = instruments.get(instrument_idx) {
                    if inst.samples.len() > xm_sample as usize {
                        playing_samples[current_channel] =
                            Some((instrument_idx, xm_sample as usize));
                    }
                }
            }
        }

        let currently_playing = playing_samples[current_channel]
            .and_then(|(i_idx, s_idx)| instruments.get(i_idx).and_then(|i| i.samples.get(s_idx)));

        let sample_default_volume = currently_playing
            .map(|s| s.default_volume as i32)
            .unwrap_or(0x40);
        let sample_volume = currently_playing.map(|s| s.volume).unwrap_or(1.0);

        if !cell.effect_number.is_empty() {
            match encoder.get_xm_effect(
                &cell.effect_number,
                &cell.effect_value,
                xm_note,
                xm_instrument,
            ) {
                Ok((n, v)) if n as u32 + v as u32 > 0 => {
                    is_effect_command_used = true;
                    xm_effect_number = n;
                    xm_effect_value = v;
                }
                Ok(_) => {}
                Err(e) => log.log(
                    LogLevel::Error,
                    format!("row {current_row}, channel {}: {e}", current_channel + 1),
                ),
            }
        }

        // volume column gets priority over panning.
        if let Some(volume) = &cell.volume {
            xm_volume = encoder.volume_column_effect_from_volume(volume);
            is_volume_command_used = xm_volume > 0;

            if !is_volume_command_used && !is_effect_command_used {
                let (n, v) = encoder.transpose_volume_to_command_effect(volume);
                if n as u32 + v as u32 > 0 {
                    is_effect_command_used = true;
                    xm_effect_number = n;
                    xm_effect_value = v;
                }
            }
        }

        if settings.volume_scaling_mode == VolumeScalingMode::Column {
            apply_column_volume_scaling(
                current_row,
                current_channel,
                &cell.note,
                &cell.instrument,
                currently_playing,
                sample_default_volume,
                sample_volume,
                &mut xm_volume,
                &mut xm_effect_number,
                &mut xm_effect_value,
                &mut is_volume_command_used,
                &mut is_effect_command_used,
                log,
            );
        }

        if let Some(delay) = &cell.delay {
            if !is_effect_command_used {
                let (n, v) = encoder.transpose_delay_to_command_effect(delay);
                if n as u32 + v as u32 > 0 {
                    is_effect_command_used = true;
                    xm_effect_number = n;
                    xm_effect_value = v;
                }
            } else {
                log.log(
                    LogLevel::Error,
                    format!("row {current_row}, channel {}: Cannot apply delay for this channel due to missing free slots", current_channel + 1),
                );
            }
        }

        if let Some(panning) = &cell.panning {
            if !is_volume_command_used {
                xm_volume = encoder.volume_column_effect_from_panning(panning);
                is_panning_command_used = xm_volume > 0;
            }
            if !is_panning_command_used && !is_effect_command_used {
                let (n, v) = encoder.transpose_panning_to_command_effect(panning);
                if n as u32 + v as u32 > 0 {
                    is_effect_command_used = true;
                    xm_effect_number = n;
                    xm_effect_value = v;
                }
            }
        }

        if is_master_track_command_used && !is_effect_command_used {
            is_effect_command_used = true;
            xm_effect_number = master_track_command.0;
            xm_effect_value = master_track_command.1;
            is_master_track_command_used = false;
        }
        let _ = is_effect_command_used;

        if is_panning_command_used || is_volume_command_used {
            compression_value += VOLUME_COL_BIT;
        }
        if xm_effect_number > 0 {
            compression_value += EFFECT_TYPE_BIT;
        }
        if xm_effect_value > 0 {
            compression_value += EFFECT_PARAM_BIT;
        }

        if compression_value != ALL_VALUES_FILLED_BIT {
            out.push(compression_value);
        }
        if (compression_value & 0x1) > 0 {
            out.push(xm_note);
        }
        if ((compression_value & 0x3) >> 1) > 0 {
            out.push(xm_instrument);
        }
        if ((compression_value & 0x7) >> 2) > 0 {
            out.push(xm_volume);
        }
        if ((compression_value & 0xF) >> 3) > 0 {
            out.push(xm_effect_number);
        }
        if ((compression_value & 0x1F) >> 4) > 0 {
            out.push(xm_effect_value);
        }
    }

    Ok(out)
}

#[allow(clippy::too_many_arguments)]
fn apply_column_volume_scaling(
    current_row: usize,
    current_channel: usize,
    note: &Option<String>,
    instrument: &Option<String>,
    currently_playing: Option<&SampleData>,
    sample_default_volume: i32,
    mut sample_volume: f32,
    xm_volume: &mut u8,
    xm_effect_number: &mut u8,
    xm_effect_value: &mut u8,
    is_volume_command_used: &mut bool,
    is_effect_command_used: &mut bool,
    log: &mut dyn ConversionLog,
) {
    let mut sample_needs_scaling = currently_playing
        .map(|s| (s.volume - 1.0).abs() > f32::EPSILON)
        .unwrap_or(false);
    let does_trigger_sample = note.is_some() && instrument.is_some();

    if sample_needs_scaling
        && *is_volume_command_used
        && extras::is_volume_set_on_volume_column(*xm_volume)
    {
        match extras::scale_volume_from_volume_command(*xm_volume, sample_volume) {
            Ok(v) => *xm_volume = v,
            Err(e) => log.log(
                LogLevel::Error,
                format!("row {current_row}, channel {}: {e}", current_channel + 1),
            ),
        }
        sample_needs_scaling = false;
    }

    if sample_needs_scaling
        && *is_effect_command_used
        && extras::is_volume_set_on_effect_column(*xm_effect_number)
    {
        match extras::scale_volume_from_effect_command(*xm_effect_value, sample_volume) {
            Ok(v) => *xm_effect_value = v,
            Err(e) => log.log(
                LogLevel::Error,
                format!("row {current_row}, channel {}: {e}", current_channel + 1),
            ),
        }
        sample_needs_scaling = false;
    }

    if sample_needs_scaling && does_trigger_sample {
        sample_volume *= sample_default_volume as f32 / 0x40 as f32;

        if !*is_volume_command_used {
            match extras::scale_volume_from_volume_command_new(sample_volume) {
                Ok(v) => {
                    *xm_volume = v;
                    *is_volume_command_used = true;
                }
                Err(e) => log.log(
                    LogLevel::Error,
                    format!("row {current_row}, channel {}: {e}", current_channel + 1),
                ),
            }
            sample_needs_scaling = false;
        }

        if sample_needs_scaling && !*is_effect_command_used {
            match extras::scale_volume_from_effect_command_new(sample_volume) {
                Ok((n, v)) => {
                    *is_effect_command_used = true;
                    *xm_effect_number = n;
                    *xm_effect_value = v;
                }
                Err(e) => log.log(
                    LogLevel::Error,
                    format!("row {current_row}, channel {}: {e}", current_channel + 1),
                ),
            }
            sample_needs_scaling = false;
        }
    }

    if sample_needs_scaling {
        log.log(
            LogLevel::Error,
            format!("row {current_row}, channel {}: Cannot apply scaled volume for this channel due to missing free slots", current_channel + 1),
        );
    }
}
