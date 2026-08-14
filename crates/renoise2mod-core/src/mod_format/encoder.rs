//! Stateful per-channel MOD encoding (mirrors `ModUtils.cs`): the Amiga-period portamento state
//! machine, the three distinct `GetModNote` variants the original has (see doc comments below --
//! they are NOT interchangeable), and the Renoise-effect-to-MOD-effect dispatch tables.

use crate::commands;
use crate::common::{self, VolumeScalingMode};
use crate::error::{Error, Result};
use crate::mod_format::period::{self, is_note_in_range, ProTrackerCompatibility, PERIODS_RANGE};

#[derive(Debug, Clone)]
pub struct ModSettings {
    pub pro_tracker_compatibility: ProTrackerCompatibility,
    pub ntsc_mode: bool,
    /// `Settings.PortamentoLossThreshold`, valid range 0-4.
    pub portamento_loss_threshold: i32,
    /// MOD only supports `None`/`Sample`; `Column` is rejected by [`ModSettings::validate`].
    pub volume_scaling_mode: VolumeScalingMode,
}

impl ModSettings {
    pub fn validate(&self) -> Result<()> {
        if self.volume_scaling_mode == VolumeScalingMode::Column {
            return Err(Error::Conversion(
                "invalid volume scaling mode for MOD".to_string(),
            ));
        }
        if !(0..=4).contains(&self.portamento_loss_threshold) {
            return Err(Error::Conversion(
                "invalid portamento loss threshold value (valid range: 0-4)".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
struct ChannelInfo {
    reached_note: i32,
    tone_portamento_note: i32,
    tone_portamento_period: i32,
    current_pitch: i32,
    current_sample: i32,
    current_period: i32,
    last_portamento_command: i32,
    last_portamento_value: i32,
    portamento_direction_flag: i32,
}

impl ChannelInfo {
    fn new() -> Self {
        Self {
            reached_note: -1,
            tone_portamento_note: 0,
            tone_portamento_period: 0,
            current_pitch: 0,
            current_sample: -1,
            current_period: 0,
            last_portamento_command: 0,
            last_portamento_value: 0,
            portamento_direction_flag: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ModInstrumentInfo {
    /// Signed relative-tone offset baked into every note this instrument plays (mirrors
    /// `InstrumentDataMOD.SampleDataMOD.RelatedNote`).
    pub related_note: i32,
    pub fine_tune: i32,
    /// Encoded (post-resample, mono, 8-bit) MOD sample length in bytes.
    pub length: i32,
    /// Source sample length in frames, pre-resample.
    pub original_length: i32,
}

/// MOD format only ever has exactly 4 hardware channels' worth of portamento state, regardless of
/// how many note-columns the song actually has -- this mirrors a real limitation in the original
/// (`ChannelInfoData[4]`), not an arbitrary choice. Callers with `num_channels > 4` will panic on
/// out-of-range channel access; [`super::MIN_CHANNELS`]/[`super::check_requirements`] enforce the
/// MOD-format channel-count floor, but there is no enforced ceiling in the original either.
const NUM_HARDWARE_CHANNELS: usize = 4;

pub struct ModEncoder {
    channel_data: [ChannelInfo; NUM_HARDWARE_CHANNELS],
    settings: ModSettings,
    ticks_per_row: i32,
    playback_engine_version: i32,
    pitch_compatibility_mode: bool,
    sample_offset_compatibility_mode: bool,
    instruments: Vec<ModInstrumentInfo>,
}

impl ModEncoder {
    pub fn new(
        num_instruments: usize,
        initial_ticks_per_row: i32,
        settings: ModSettings,
        playback_engine_version: i32,
        pitch_compatibility_mode: bool,
        sample_offset_compatibility_mode: bool,
    ) -> Self {
        Self {
            channel_data: [ChannelInfo::new(); NUM_HARDWARE_CHANNELS],
            settings,
            ticks_per_row: initial_ticks_per_row,
            playback_engine_version,
            pitch_compatibility_mode,
            sample_offset_compatibility_mode,
            instruments: vec![ModInstrumentInfo::default(); num_instruments],
        }
    }

    pub fn set_ticks_per_row(&mut self, ticks: i32) {
        self.ticks_per_row = ticks;
    }

    pub fn ticks_per_row(&self) -> i32 {
        self.ticks_per_row
    }

    /// Mirrors `ModUtils.StoreSampleInfo`: records the base-note/finetune split and both the
    /// encoded and original sample lengths for a 0-based instrument index.
    #[allow(clippy::too_many_arguments)]
    pub fn store_sample_info(
        &mut self,
        instrument_index: usize,
        original_length_bytes: i64,
        encoded_length_bytes: i64,
        sample_rate: u32,
        original_channels: u32,
        original_bits_per_sample: u32,
        renoise_base_note: i32,
        renoise_fine_tuning: i32,
        transpose: i32,
    ) {
        let computed_length = common::calculate_sample_length(encoded_length_bytes as u64, 8, 1);
        let computed_original_length = common::calculate_sample_length(
            original_length_bytes as u64,
            original_bits_per_sample,
            original_channels,
        );
        let props = common::get_sample_properties(
            sample_rate as f64,
            renoise_base_note,
            transpose,
            renoise_fine_tuning,
            self.settings.ntsc_mode,
        );

        self.instruments[instrument_index] = ModInstrumentInfo {
            related_note: props.relative_tone,
            fine_tune: props.fine_tune,
            length: computed_length as i32,
            original_length: computed_original_length as i32,
        };
    }

    pub fn sample_fine_tune(&self, instrument_index: usize) -> i32 {
        self.instruments[instrument_index].fine_tune
    }

    pub fn sample_length(&self, instrument_index: usize) -> i32 {
        self.instruments[instrument_index].length
    }

    /// Rescales a Renoise loop point (frames at the source sample rate) into the resampled MOD
    /// sample's frame space, then halves it (MOD stores loop points in words). `instrument_index`
    /// is 0-based (mirrors `GetLoopValue`).
    pub fn get_loop_value(&self, loop_value: u32, instrument_index: usize) -> i32 {
        let info = &self.instruments[instrument_index];
        if info.length > 0 {
            (loop_value as i64 * info.length as i64 / info.original_length.max(1) as i64 / 2) as i32
        } else {
            0
        }
    }

    fn compat(&self) -> ProTrackerCompatibility {
        self.settings.pro_tracker_compatibility
    }

    /// Stateless period lookup by MOD note-name string (e.g. `"C-2"`), no ProTracker-compatibility
    /// clamping -- mirrors the 1-argument `GetModNote(string note)` overload, used only for the
    /// "explicit note name" sample-rate-selection setting.
    pub fn mod_note_period_from_name(note: &str) -> Result<i32> {
        let idx = period::parse_note_name_to_period_index(note)
            .ok_or_else(|| Error::Conversion(format!("note {note} is out of range")))?;
        Ok(PERIODS_RANGE[idx as usize])
    }

    /// Returns a `PeriodsRange` *index* (not a period value) for a raw absolute Renoise note
    /// (0-119ish) adjusted by the given instrument's base-note offset, applying full
    /// ProTracker-compatibility range clamping. Mirrors the 3-argument
    /// `GetModNote(int note, int sampleNumber, int channel)` overload -- note its `channel`
    /// parameter is read but never used in the original, so it's dropped here.
    fn mod_note_index(&self, note: i32, sample_number: i32) -> Result<i32> {
        let tone_to_add = if sample_number >= 0 {
            self.instruments[sample_number as usize].related_note
        } else {
            0
        };
        if note < 0 {
            return Ok(0);
        }
        let octave = note / 12;
        let note_index = note % 12;
        let final_note = (octave - 2) * 12 + note_index + tone_to_add;
        if is_note_in_range(final_note, self.compat()) {
            Ok(final_note)
        } else {
            Err(Error::Conversion(format!(
                "note {note} is out of range (can be fixed by changing sample frequency)"
            )))
        }
    }

    /// Triggers a note on `channel` (0-based), returning its MOD period. Mutates channel state
    /// (`ReachedNote`/`CurrentPeriod`/etc., or just the tone-portamento target if
    /// `is_tone_portamento_triggered`). Mirrors the 4-argument
    /// `GetModNote(string note, int sampleNumber, int channel, bool isTonePortamentoTriggered)`.
    /// `sample_number` is 0-based, `-1` meaning "no instrument specified this row".
    pub fn trigger_mod_note(
        &mut self,
        note: &str,
        sample_number: i32,
        channel: usize,
        is_tone_portamento_triggered: bool,
    ) -> Result<i32> {
        let tone_to_add = if sample_number >= 0 {
            self.instruments[sample_number as usize].related_note
        } else {
            0
        };

        let Some((octave, note_offset)) = period::parse_note_name_parts(note) else {
            return Ok(0);
        };
        let final_index = (octave - 2) * 12 + note_offset + tone_to_add;

        if !is_note_in_range(final_index, self.compat()) {
            return Err(Error::Conversion(format!(
                "note {note} is out of range (can be fixed by changing sample frequency)"
            )));
        }

        let value = PERIODS_RANGE[final_index as usize];
        let renoise_note = octave * 12 + note_offset;

        let ch = &mut self.channel_data[channel];
        ch.current_sample = sample_number;
        if is_tone_portamento_triggered {
            ch.tone_portamento_note = renoise_note;
            ch.tone_portamento_period = value;
        } else {
            ch.current_pitch = 0;
            ch.current_period = value;
            ch.tone_portamento_period = value;
            ch.tone_portamento_note = renoise_note;
            ch.reached_note = renoise_note;
        }

        Ok(value)
    }

    /// Mirrors `IsTonePortamentoTriggered`. Errors (uncatchable, matching the original's plain
    /// `System.Exception` rather than `ConversionException`) if both the effect column and a
    /// vol/pan column simultaneously specify glide -- confirmed against `ModConverter.cs` that
    /// this specific error *is* caught by the surrounding per-cell `catch (Exception e)` there
    /// (logged and the row's note simply doesn't trigger), unlike what a more literal reading of
    /// "uncaught exception type" might suggest.
    pub fn is_tone_portamento_triggered(
        effect: Option<&str>,
        volume: Option<&str>,
        panning: Option<&str>,
    ) -> Result<bool> {
        let mut output = false;

        if let Some(effect) = effect {
            let mut chars = effect.chars();
            let eff_type = chars.next().unwrap_or('\0');
            let eff_com = chars.next().unwrap_or('\0');
            output = eff_type == '0' && eff_com == 'G';
        }
        if !output {
            if let Some(volume) = volume {
                let eff_type = volume.chars().next().unwrap_or('\0');
                output = eff_type == 'G';
                if output && effect.is_some() {
                    return Err(Error::Conversion(
                        "Critical conversion exception: Found value for fx column and glide value on volume column".to_string(),
                    ));
                }
            }
        }
        if !output {
            if let Some(panning) = panning {
                let eff_type = panning.chars().next().unwrap_or('\0');
                output = eff_type == 'G';
                if output && effect.is_some() {
                    return Err(Error::Conversion(
                        "Critical conversion exception: Found value for fx column and glide value on volume column".to_string(),
                    ));
                }
            }
        }

        Ok(output)
    }

    fn periods_range_pair(&self, index: i32) -> Result<(i32, i32)> {
        let a = *PERIODS_RANGE
            .get(index as usize)
            .ok_or_else(|| Error::Conversion(format!("note index {index} out of table range")))?;
        let b = *PERIODS_RANGE.get(index as usize + 1).ok_or_else(|| {
            Error::Conversion(format!("note index {index} has no upper neighbor in table"))
        })?;
        Ok((a, b))
    }

    /// Mirrors `GetTonePortamentoFromChannelPeriod`.
    fn tone_portamento_delta(
        &mut self,
        mut value: i32,
        channel: usize,
        is_note_triggered: bool,
        ignore_pitch_compatibility_mode: bool,
    ) -> Result<i32> {
        let ch = self.channel_data[channel];
        if value == 0 && ch.last_portamento_value > 0 {
            value = ch.last_portamento_value;
        }
        if value == 0 || ch.current_period == ch.tone_portamento_period {
            return if is_note_triggered {
                Ok(0)
            } else {
                Err(Error::Conversion(
                    "tone portamento value equals 0".to_string(),
                ))
            };
        }

        let tone_portamento_period = ch.tone_portamento_period;
        let real_value = if !ignore_pitch_compatibility_mode && self.pitch_compatibility_mode {
            value
                * if self.ticks_per_row > 1 {
                    self.ticks_per_row - 1
                } else {
                    1
                }
        } else {
            value
        };
        let direction_flag = if ch.current_period < tone_portamento_period {
            -1
        } else {
            1
        };
        let current_pitch = ch.current_pitch + real_value * direction_flag;
        let current_renoise_note =
            (current_pitch as f64 / 16.0 + ch.reached_note as f64).trunc() as i32;
        let current_period = ch.current_period;
        let mod_note = self.mod_note_index(current_renoise_note, ch.current_sample)?;

        let mut relative_pitch = current_pitch % 0x10;
        if relative_pitch < 0 {
            relative_pitch += 0x10;
        }

        if current_period == 0 {
            return Err(Error::Conversion(
                "no previous note triggered on this channel".to_string(),
            ));
        }

        let (first_period, second_period) = self.periods_range_pair(mod_note)?;
        let delta = first_period - second_period;
        let portamento = relative_pitch * delta / 0x10;
        let target_period = first_period - portamento;
        let portamento = (current_period - target_period) * direction_flag;

        let ch = &mut self.channel_data[channel];
        ch.reached_note = current_renoise_note;
        ch.current_pitch = relative_pitch;
        ch.last_portamento_command = 0x03;
        ch.last_portamento_value = value;
        ch.portamento_direction_flag = direction_flag;

        Ok(portamento)
    }

    /// Mirrors `GetPortamentoFromChannelPeriod`.
    fn portamento_delta(
        &mut self,
        command: char,
        mut value: i32,
        channel: usize,
        ignore_pitch_compatibility_mode: bool,
    ) -> Result<i32> {
        let ch = self.channel_data[channel];
        if value == 0 && ch.last_portamento_value > 0 {
            value = ch.last_portamento_value;
        }

        let (direction_flag, portamento_command) = match command {
            'U' => (1, 1),
            'D' => (-1, 2),
            _ => return Err(Error::Conversion("command not valid".to_string())),
        };

        let real_value = if !ignore_pitch_compatibility_mode && self.pitch_compatibility_mode {
            value
                * if self.ticks_per_row > 1 {
                    self.ticks_per_row - 1
                } else {
                    1
                }
        } else {
            value
        };
        let current_pitch = ch.current_pitch + real_value * direction_flag;
        let current_renoise_note =
            (current_pitch as f64 / 16.0 + ch.reached_note as f64).trunc() as i32;
        let mod_note = self.mod_note_index(current_renoise_note, ch.current_sample)?;
        let current_period = ch.current_period;

        let mut relative_pitch = current_pitch % 0x10;
        if relative_pitch < 0 {
            relative_pitch += 0x10;
        }

        if current_period == 0 {
            return Err(Error::Conversion(
                "no previous note triggered on this channel".to_string(),
            ));
        }

        let (first_period, second_period) = self.periods_range_pair(mod_note)?;
        let delta = first_period - second_period;
        let portamento = relative_pitch * delta / 0x10;
        let target_period = first_period - portamento;
        let portamento = (current_period - target_period) * direction_flag;

        let ch = &mut self.channel_data[channel];
        ch.reached_note = current_renoise_note;
        ch.current_pitch = relative_pitch;
        ch.last_portamento_command = portamento_command;
        ch.last_portamento_value = value;
        ch.portamento_direction_flag = direction_flag;

        if portamento == 0 {
            return Err(Error::Conversion(
                "Portamento value resulted to 0 value, no effect was triggered there".to_string(),
            ));
        }

        Ok(portamento)
    }

    fn effective_portamento(command: u8, value: u8, ticks_per_row: i32) -> i32 {
        match command {
            0x01..=0x03 => value as i32 * (ticks_per_row - 1),
            0x0E => value as i32 & 0x0F,
            _ => 0,
        }
    }

    /// Mirrors `StoreChannelPeriod`.
    fn store_channel_period(&mut self, portamento: i32, command: char, channel: usize) {
        let ch = &mut self.channel_data[channel];
        let direction_flag = -ch.portamento_direction_flag;
        let mut current_period = ch.current_period + portamento * direction_flag;

        if command == 'G'
            && ((direction_flag > 0 && current_period > ch.tone_portamento_period)
                || (direction_flag < 0 && current_period < ch.tone_portamento_period))
        {
            current_period = ch.tone_portamento_period;
            ch.reached_note = ch.tone_portamento_note;
            ch.current_pitch = 0;
        }

        ch.current_period = current_period;
    }

    /// Mirrors `GetSampleCommands`. `sample_index` is the 1-based MOD sample number (0 = none).
    fn get_sample_commands(
        &mut self,
        command: char,
        value: i32,
        sample_index: i32,
        channel: usize,
        is_note_triggered: bool,
    ) -> (u8, u8) {
        let threshold = self.settings.portamento_loss_threshold;
        let pitch_compat = self.pitch_compatibility_mode;
        let sample_offset_compat = self.sample_offset_compatibility_mode;
        let ticks = self.ticks_per_row;

        match command {
            'A' => commands::arpeggio(value),
            'U' => match self.portamento_delta('U', value, channel, false) {
                Ok(delta) => {
                    let ret = commands::portamento(1, delta, ticks, threshold, pitch_compat);
                    let effective = Self::effective_portamento(ret.0, ret.1, ticks);
                    self.store_channel_period(effective, 'U', channel);
                    ret
                }
                Err(_) => (0, 0),
            },
            'D' => match self.portamento_delta('D', value, channel, false) {
                Ok(delta) => {
                    let ret = commands::portamento(2, delta, ticks, threshold, pitch_compat);
                    let effective = Self::effective_portamento(ret.0, ret.1, ticks);
                    self.store_channel_period(effective, 'D', channel);
                    ret
                }
                Err(_) => (0, 0),
            },
            'M' => commands::set_volume(value),
            'G' => match self.tone_portamento_delta(value, channel, is_note_triggered, false) {
                Ok(delta) => match commands::tone_portamento(delta, ticks) {
                    Ok(ret) => {
                        let effective = Self::effective_portamento(ret.0, ret.1, ticks);
                        self.store_channel_period(effective, 'G', channel);
                        ret
                    }
                    Err(_) => (0, 0),
                },
                Err(_) => (0, 0),
            },
            'I' => commands::volume_up(value, ticks),
            'O' => commands::volume_down(value, ticks),
            'P' => commands::set_panning(value),
            'S' => {
                let sample_size = if sample_index > 0 {
                    self.sample_length((sample_index - 1) as usize)
                } else {
                    0
                };
                commands::set_sample_offset(value, sample_size, sample_offset_compat)
            }
            'Q' => commands::note_delay(value),
            'R' => commands::retrig_note(value),
            'V' => commands::vibrato(value),
            'T' => commands::tremolo(value),
            // C (volume slicer), W (surround width), B (backwards), L (pre-mixer track volume),
            // N/E/J/X: not implemented for MOD, matches the original.
            _ => (0, 0),
        }
    }

    /// Mirrors `GetGlobalCommands`.
    fn get_global_commands(&self, command: char, value: i32) -> (u8, u8) {
        match command {
            'T' => commands::set_tempo(value),
            'L' => {
                if self.playback_engine_version == 1 {
                    commands::set_speed(value)
                } else {
                    (0, 0)
                }
            }
            'K' => commands::set_speed(value),
            'B' => commands::pattern_break(value),
            'D' => commands::pattern_delay(value),
            // G (groove): not implemented, matches the original.
            _ => (0, 0),
        }
    }

    /// Mirrors `GetModEffect`. `sample_index` is the 1-based MOD sample number (0 = none).
    pub fn get_mod_effect(
        &mut self,
        xrns_eff_num: &str,
        xrns_eff_val: &str,
        sample_index: i32,
        channel: usize,
        is_note_triggered: bool,
    ) -> Result<(u8, u8)> {
        let mut chars = xrns_eff_num.chars();
        let eff_type = chars
            .next()
            .ok_or_else(|| Error::Conversion("empty effect number".to_string()))?;
        let eff_com = chars
            .next()
            .ok_or_else(|| Error::Conversion("truncated effect number".to_string()))?;
        let eff_val = i32::from_str_radix(xrns_eff_val, 16)
            .map_err(|_| Error::Conversion(format!("invalid effect value: {xrns_eff_val}")))?;

        Ok(match eff_type {
            '0' => {
                self.get_sample_commands(eff_com, eff_val, sample_index, channel, is_note_triggered)
            }
            'Z' => self.get_global_commands(eff_com, eff_val),
            _ => (0, 0),
        })
    }

    /// Mirrors `GetCommandsFromMasterTrack`.
    pub fn get_commands_from_master_track(
        &self,
        xrns_eff_num: &str,
        xrns_eff_val: &str,
    ) -> Result<(u8, u8)> {
        let mut chars = xrns_eff_num.chars();
        let eff_type = chars
            .next()
            .ok_or_else(|| Error::Conversion("empty effect number".to_string()))?;
        let eff_com = chars
            .next()
            .ok_or_else(|| Error::Conversion("truncated effect number".to_string()))?;
        let eff_val = i32::from_str_radix(xrns_eff_val, 16)
            .map_err(|_| Error::Conversion(format!("invalid effect value: {xrns_eff_val}")))?;

        Ok(if eff_type == 'Z' {
            self.get_global_commands(eff_com, eff_val)
        } else {
            (0, 0)
        })
    }

    /// Mirrors `TransposeVolPanEffectColumnToEffectColumn`.
    fn transpose_vol_pan_to_effect(
        &mut self,
        command: char,
        value: i32,
        sample_index: i32,
        channel: usize,
        is_note_triggered: bool,
    ) -> (u8, u8) {
        let threshold = self.settings.portamento_loss_threshold;
        let pitch_compat = self.pitch_compatibility_mode;
        let ticks = self.ticks_per_row;
        let _ = sample_index;

        match command {
            'U' => match self.portamento_delta('U', value << 4, channel, true) {
                Ok(delta) => {
                    let ret = commands::portamento(1, delta, ticks, threshold, pitch_compat);
                    let effective = Self::effective_portamento(ret.0, ret.1, ticks);
                    self.store_channel_period(effective, 'U', channel);
                    ret
                }
                Err(_) => (0, 0),
            },
            'D' => match self.portamento_delta('D', value << 4, channel, true) {
                Ok(delta) => {
                    let ret = commands::portamento(2, delta, ticks, threshold, pitch_compat);
                    let effective = Self::effective_portamento(ret.0, ret.1, ticks);
                    self.store_channel_period(effective, 'D', channel);
                    ret
                }
                Err(_) => (0, 0),
            },
            'G' => match self.tone_portamento_delta(value << 4, channel, is_note_triggered, true) {
                Ok(delta) => match commands::tone_portamento(delta, ticks) {
                    Ok(ret) => {
                        let effective = Self::effective_portamento(ret.0, ret.1, ticks);
                        self.store_channel_period(effective, 'G', channel);
                        ret
                    }
                    Err(_) => (0, 0),
                },
                Err(_) => (0, 0),
            },
            'Q' => commands::note_delay(value),
            'R' => commands::retrig_note(value),
            'C' => commands::note_cut(value),
            // B (play backwards): not implemented, matches the original.
            _ => (0, 0),
        }
    }

    /// Mirrors `TransposeVolumeToCommandEffect`.
    pub fn transpose_volume_to_command_effect(
        &mut self,
        xrns_col_vol_eff: &str,
        sample_index: i32,
        channel: usize,
        is_note_triggered: bool,
    ) -> (u8, u8) {
        let mut chars = xrns_col_vol_eff.chars();
        let Some(eff_com) = chars.next() else {
            return (0, 0);
        };
        let Some(eff_val_char) = chars.next() else {
            return (0, 0);
        };
        let Some(value) = eff_val_char.to_digit(16) else {
            return (0, 0);
        };
        let value = value as i32;
        let ticks = self.ticks_per_row;

        match eff_com {
            '0'..='8' => {
                let Ok(hex_val) = i32::from_str_radix(xrns_col_vol_eff, 16) else {
                    return (0, 0);
                };
                commands::transpose_set_volume_from_volume(hex_val)
            }
            'I' => commands::transpose_volume_slide_up_from_volume(value, ticks),
            'O' => commands::transpose_volume_slide_down_from_volume(value, ticks),
            _ => self.transpose_vol_pan_to_effect(
                eff_com,
                value,
                sample_index,
                channel,
                is_note_triggered,
            ),
        }
    }

    /// Mirrors `TransposeDelayToCommandEffect`.
    pub fn transpose_delay_to_command_effect(&self, xrns_col_delay: &str) -> (u8, u8) {
        let Ok(value) = i32::from_str_radix(xrns_col_delay, 16) else {
            return (0, 0);
        };
        let mut result = ((value * self.ticks_per_row) as f64 / 255.0).round() as i32;
        if result == self.ticks_per_row {
            result -= 1;
        }
        if result > 0 {
            commands::note_delay(result)
        } else {
            (0, 0)
        }
    }

    /// Mirrors `TransposePanningToCommandEffect`. Panning-column numeric set-panning is
    /// deliberately not converted, matching the original: MOD panning affects the whole channel,
    /// whereas Renoise's panning column only affects the current note.
    pub fn transpose_panning_to_command_effect(
        &mut self,
        xrns_col_pan_eff: &str,
        sample_index: i32,
        channel: usize,
        is_note_triggered: bool,
    ) -> (u8, u8) {
        let mut chars = xrns_col_pan_eff.chars();
        let Some(eff_com) = chars.next() else {
            return (0, 0);
        };
        let Some(eff_val_char) = chars.next() else {
            return (0, 0);
        };
        let Some(value) = eff_val_char.to_digit(16) else {
            return (0, 0);
        };
        let value = value as i32;

        match eff_com {
            '0'..='8' => (0, 0),
            'J' | 'K' => (0, 0),
            _ => self.transpose_vol_pan_to_effect(
                eff_com,
                value,
                sample_index,
                channel,
                is_note_triggered,
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> ModSettings {
        ModSettings {
            pro_tracker_compatibility: ProTrackerCompatibility::None,
            ntsc_mode: false,
            portamento_loss_threshold: 2,
            volume_scaling_mode: VolumeScalingMode::None,
        }
    }

    #[test]
    fn rejects_column_volume_scaling() {
        let mut s = settings();
        s.volume_scaling_mode = VolumeScalingMode::Column;
        assert!(s.validate().is_err());
    }

    #[test]
    fn triggering_a_note_updates_channel_state_and_returns_period() {
        let mut enc = ModEncoder::new(1, 6, settings(), 1, false, false);
        let period = enc.trigger_mod_note("C-4", -1, 0, false).unwrap();
        // Renoise note "C-4" (letter index 0, octave 4) -> table index (4-2)*12+0 = 24 -> 428.
        assert_eq!(period, PERIODS_RANGE[24]);
        assert_eq!(period, 428);
    }

    #[test]
    fn glide_conflict_between_effect_and_volume_column_errors() {
        // The conflict guard only fires when the volume/panning column *independently* signals
        // glide while some *other*, non-glide effect-column command is also present -- if the
        // effect column is itself "0G", `output` is already true before the volume-column check
        // ever runs (see `is_tone_portamento_triggered`'s `if !output` guard), so that combo
        // does NOT hit the conflict path.
        let result = ModEncoder::is_tone_portamento_triggered(Some("0A"), Some("G0"), None);
        assert!(result.is_err());
    }

    #[test]
    fn glide_alone_on_effect_column_is_detected() {
        let result = ModEncoder::is_tone_portamento_triggered(Some("0G"), None, None).unwrap();
        assert!(result);
    }

    #[test]
    fn mod_note_period_from_name_matches_table() {
        assert_eq!(
            ModEncoder::mod_note_period_from_name("C-1").unwrap(),
            PERIODS_RANGE[12]
        );
    }

    #[test]
    fn note_out_of_period_range_errors() {
        assert!(period::parse_note_name_to_period_index("Z-9").is_none());
    }
}
