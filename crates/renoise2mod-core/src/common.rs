//! Algorithms shared by both the MOD and XM writers: the base-note/finetune formula, tick-per-row
//! tracking, and the portamento/volume-slide fine-vs-coarse command selection heuristics.
//!
//! These are ported close to verbatim from the C# original (`ModCommonBase.cs`) rather than
//! re-derived, since several of them (the fine/coarse thresholds in particular) are empirically
//! tuned rather than principled formulas -- matching them exactly matters more than deriving them
//! cleanly from first principles.

use crate::model::PatternData;

/// PAL Amiga C-2 frequency, used when NTSC mode is off.
pub const PAL_C2_FREQUENCY: f64 = 8287.13691588785;
/// NTSC Amiga C-2 frequency, used when NTSC mode is on.
pub const NTSC_C2_FREQUENCY: f64 = 8363.42289719626;

/// Renoise's C-4 in its own 0-119 note numbering, used as the neutral pivot note for the
/// base-note/finetune split below.
const DEFAULT_NOTE: i32 = 48;

/// Shared vocabulary between the MOD and XM writers, though MOD only implements `None`/`Sample`
/// -- `Column` is XM-only (rejected at the MOD settings-validation stage).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolumeScalingMode {
    None,
    Sample,
    Column,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SampleProperties {
    /// Signed semitone offset from the sample's own base note, clamped to `[-127, 127]`.
    pub relative_tone: i32,
    /// Signed finetune remainder, in `1/128`-semitone units.
    pub fine_tune: i32,
}

/// Splits a sample's resample rate + Renoise base note/transpose/finetune into a tracker-format
/// relative-tone/finetune pair (mirrors `ModCommonBase.GetSampleProperties`).
///
/// `1536 == 12 semitones * 128 finetune-units/semitone`.
pub fn get_sample_properties(
    sample_rate: f64,
    ren_base_note: i32,
    transpose: i32,
    ren_fine_tuning: i32,
    ntsc_mode: bool,
) -> SampleProperties {
    let renoise_value_to_add = DEFAULT_NOTE - ren_base_note + transpose;
    let c2_freq = if ntsc_mode {
        NTSC_C2_FREQUENCY
    } else {
        PAL_C2_FREQUENCY
    };

    let f2t = (1536.0 * (sample_rate / c2_freq).log2()).round() as i32;

    let mut transp = f2t >> 7;
    let mut ftune = f2t & 0x7F;
    ftune += ren_fine_tuning;
    if ftune > 80 {
        transp += 1;
        ftune -= 128;
    }
    let transp = transp.clamp(-127, 127);

    SampleProperties {
        relative_tone: transp + renoise_value_to_add,
        fine_tune: ftune,
    }
}

/// Converts a raw byte length into a frame count (mirrors `ModCommonBase.CalculateSampleLength`).
pub fn calculate_sample_length(byte_size: u64, bits_per_sample: u32, channels: u32) -> u64 {
    let mut size = byte_size;
    if bits_per_sample > 8 {
        size /= 2;
    }
    size / channels as u64
}

fn tick_divider(ticks_per_row: i32) -> i32 {
    if ticks_per_row > 1 {
        ticks_per_row - 1
    } else {
        ticks_per_row
    }
}

/// Remainder formula used to decide how much precision a coarse (per-tick) pitch-slide command
/// would lose versus a fine (one-shot) one (mirrors `ModCommands.GetPrecisionLostInPitchSlide`).
///
/// NOTE: this is *not* identical to [`get_precision_lost_in_volume_slide`] below -- the pitch
/// version has an extra branch the volume version lacks. They looked like they might be "the same
/// remainder formula" from a distance, but the original source has two distinct copies; verified
/// against `ModCommands.cs` directly rather than trusting that resemblance.
pub fn get_precision_lost_in_pitch_slide(orig_value: i32, ticks_per_row: i32) -> i32 {
    let divider = tick_divider(ticks_per_row);
    if divider == 0 {
        return 0;
    }
    let mut loss = orig_value % divider;
    if loss > divider / 2 {
        loss -= divider;
    } else if orig_value == loss {
        loss = divider - orig_value;
    }
    loss
}

/// Volume-slide counterpart of [`get_precision_lost_in_pitch_slide`] -- same shape, but without
/// the `orig_value == loss` fallback branch (mirrors `ModCommands.GetPrecisionLostInVolumeSlide`).
pub fn get_precision_lost_in_volume_slide(orig_value: i32, ticks_per_row: i32) -> i32 {
    let divider = tick_divider(ticks_per_row);
    if divider == 0 {
        return 0;
    }
    let mut loss = orig_value % divider;
    if loss > divider / 2 {
        loss -= divider;
    }
    loss
}

/// Should a pitch-slide effect value be encoded as a fine (`0xE1x`/`0xE2x`) command instead of a
/// coarse (`0x1xx`/`0x2xx`) one, given the configured accuracy-loss threshold
/// (`Settings.PortamentoLossThreshold`)?
pub fn is_fine_portamento_closer_to_value(
    effect_value: i32,
    ticks_per_row: i32,
    accuracy_loss_threshold: i32,
) -> bool {
    if effect_value <= 0 {
        return false;
    }
    let loss = get_precision_lost_in_pitch_slide(effect_value, ticks_per_row).abs();
    if effect_value < 19 && loss > accuracy_loss_threshold {
        let fine_loss = effect_value - 15;
        return fine_loss < loss;
    }
    false
}

/// Volume-slide equivalent of [`is_fine_portamento_closer_to_value`], with a flat threshold
/// (no configurable accuracy-loss parameter in the original).
pub fn is_fine_volume_closer_to_value(effect_value: i32, ticks_per_row: i32) -> bool {
    let loss = get_precision_lost_in_volume_slide(effect_value, ticks_per_row).abs();
    if loss > 0 && effect_value < 0x78 {
        let fine_loss = effect_value - 0x3C;
        return fine_loss < loss;
    }
    false
}

/// Coarse per-tick portamento delta (mirrors `GetPortamentoValue`); never rounds a nonzero input
/// down to zero.
pub fn get_portamento_value(orig_value: i32, ticks_per_row: i32) -> i32 {
    let divider = tick_divider(ticks_per_row);
    if divider == 0 {
        return 0;
    }
    let mut v = (orig_value as f64 / divider as f64).round() as i32;
    if v == 0 && orig_value != 0 {
        v = 1;
    }
    v
}

/// Coarse per-tick volume-slide delta (mirrors `GetVolumeSlideValue`). Renoise's volume precision
/// is 4x a tracker's, hence the `>> 2` before dividing.
pub fn get_volume_slide_value(orig_value: i32, ticks_per_row: i32) -> i32 {
    let divider = tick_divider(ticks_per_row);
    if divider == 0 {
        return 0;
    }
    ((orig_value >> 2) as f64 / divider as f64).round() as i32
}

/// Scans one pattern row (both per-channel effect columns and the master track) for a "set LPB"/
/// "set TPL" global command (`"Z" + letter`) and returns the updated ticks-per-row value, or
/// `current` unchanged if none was found. Which letter is checked depends on
/// `playback_engine_version` (`'L'` when compatible with `TIMING MODEL SPEED`, i.e. version == 1;
/// `'K'` otherwise) -- based on `ModCommonBase.ComputeTickPerRowForCurrentLine`.
///
/// Two corrections versus the original: it requires the first character to actually be `'Z'`
/// (the original only checked the second character, so a per-note command like `"0L"` could be
/// misread as a tempo-timing command -- an unused `effType` variable in the source suggests this
/// was a bug, not a feature), and it also scans the master track (the original never did, even
/// though Renoise allows global `"Z"` commands on any track including the master track).
///
/// Must be called in row order: each row's own portamento/volume-slide math depends on the
/// ticks-per-row value as of *that* row.
pub fn compute_ticks_per_row_for_line(
    pattern: &PatternData,
    row: usize,
    num_channels: usize,
    num_master_track_columns: usize,
    playback_engine_version: i32,
    current: i32,
) -> i32 {
    let target_letter = if playback_engine_version == 1 {
        'L'
    } else {
        'K'
    };

    for channel in 0..num_channels {
        let cell = pattern.track_line(row, channel, num_channels);
        if let Some(v) = parse_tick_command(&cell.effect_number, &cell.effect_value, target_letter)
        {
            return v;
        }
    }
    for col in 0..num_master_track_columns {
        let cell = pattern.master_line(row, col, num_master_track_columns);
        if let Some(v) = parse_tick_command(&cell.effect_number, &cell.effect_value, target_letter)
        {
            return v;
        }
    }

    current
}

fn parse_tick_command(effect_number: &str, effect_value: &str, target_letter: char) -> Option<i32> {
    let mut chars = effect_number.chars();
    if chars.next()? != 'Z' {
        return None;
    }
    if chars.next()? != target_letter {
        return None;
    }
    i32::from_str_radix(effect_value, 16).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{MasterTrackLineData, TrackLineData};

    #[test]
    fn sample_properties_unison_rate_gives_zero_offset() {
        let props = get_sample_properties(PAL_C2_FREQUENCY, 48, 0, 0, false);
        assert_eq!(props.relative_tone, 0);
        assert_eq!(props.fine_tune, 0);
    }

    #[test]
    fn sample_properties_octave_up_gives_twelve_semitones() {
        let props = get_sample_properties(PAL_C2_FREQUENCY * 2.0, 48, 0, 0, false);
        assert_eq!(props.relative_tone, 12);
        assert_eq!(props.fine_tune, 0);
    }

    #[test]
    fn sample_properties_base_note_offset_shifts_relative_tone() {
        // A sample whose Renoise base note is one semitone below C-4 (47) should read back as
        // +1 relative tone at the same physical rate.
        let props = get_sample_properties(PAL_C2_FREQUENCY, 47, 0, 0, false);
        assert_eq!(props.relative_tone, 1);
    }

    #[test]
    fn calculate_sample_length_matches_formula() {
        assert_eq!(calculate_sample_length(1000, 8, 1), 1000);
        assert_eq!(calculate_sample_length(1000, 16, 1), 500);
        assert_eq!(calculate_sample_length(1000, 16, 2), 250);
    }

    #[test]
    fn portamento_value_never_rounds_nonzero_to_zero() {
        assert_eq!(get_portamento_value(1, 6), 1);
        assert_eq!(get_portamento_value(0, 6), 0);
    }

    #[test]
    fn fine_portamento_threshold_matches_formula() {
        // effect_value=1, ticks=6 -> divider=5, loss = 1%5=1 (1<=2 so not > divider/2, and
        // orig_value==loss so loss = divider-orig = 4). fine_loss = 1-15 = -14 < 4 -> fine wins.
        assert!(is_fine_portamento_closer_to_value(1, 6, 2));
    }

    #[test]
    fn ticks_per_row_updates_on_matching_global_command() {
        let mut pattern = PatternData {
            num_rows: 1,
            tracks_line_data: vec![TrackLineData::default(); 2],
            master_track_line_data: vec![MasterTrackLineData::default(); 1],
        };
        pattern.tracks_line_data[0].effect_number = "ZL".to_string();
        pattern.tracks_line_data[0].effect_value = "07".to_string();

        // playback_engine_version == 1 -> checks 'L', should pick up the 0x07.
        let updated = compute_ticks_per_row_for_line(&pattern, 0, 2, 1, 1, 6);
        assert_eq!(updated, 7);

        // playback_engine_version != 1 -> checks 'K' instead, 'ZL' shouldn't match.
        let unchanged = compute_ticks_per_row_for_line(&pattern, 0, 2, 1, 2, 6);
        assert_eq!(unchanged, 6);
    }

    #[test]
    fn ticks_per_row_requires_z_prefix_and_ignores_per_note_commands() {
        let mut pattern = PatternData {
            num_rows: 1,
            tracks_line_data: vec![TrackLineData::default(); 2],
            master_track_line_data: vec![MasterTrackLineData::default(); 1],
        };
        // "0L" is a per-note sample command (pre-mixer track volume), not a global command --
        // must NOT be misread as a tick-setter just because its second letter is 'L'.
        pattern.tracks_line_data[0].effect_number = "0L".to_string();
        pattern.tracks_line_data[0].effect_value = "03".to_string();

        let unchanged = compute_ticks_per_row_for_line(&pattern, 0, 2, 1, 1, 6);
        assert_eq!(unchanged, 6);
    }

    #[test]
    fn ticks_per_row_also_checks_the_master_track() {
        let mut pattern = PatternData {
            num_rows: 1,
            tracks_line_data: vec![TrackLineData::default(); 2],
            master_track_line_data: vec![MasterTrackLineData::default(); 1],
        };
        pattern.master_track_line_data[0].effect_number = "ZL".to_string();
        pattern.master_track_line_data[0].effect_value = "05".to_string();

        let updated = compute_ticks_per_row_for_line(&pattern, 0, 2, 1, 1, 6);
        assert_eq!(updated, 5);
    }
}
