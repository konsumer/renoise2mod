//! XM note/keymap/envelope logic and the Renoise-effect-to-XM-effect dispatch tables (mirrors
//! `XMUtils.cs`). XM's pitch effects use simple linear division (no Amiga-period/channel-state
//! tracking like MOD's `mod_format::encoder::ModEncoder`) -- implemented here as a genuinely
//! separate code path via `xm_format::commands`, not inheritance from the MOD encoder.

use crate::commands as shared_commands;
use crate::error::{Error, Result};
use crate::model::LoopMode;
use crate::xm_format::commands as xm_commands;

const NOTES_ARRAY: [&str; 12] = [
    "C-", "C#", "D-", "D#", "E-", "F-", "F#", "G-", "G#", "A-", "A#", "B-",
];

/// Parses a Renoise note string into an XM note number: `"OFF"` -> 97 (key-off), else
/// `12*octave + (letter_index+1)` (1-96 for octaves 0-7). Mirrors `GetXMNote`.
pub fn get_xm_note(note: &str) -> Result<u8> {
    if note == "OFF" {
        return Ok(97);
    }
    if note.len() != 3 {
        return Ok(0);
    }
    let tune = &note[0..2];
    let Some(index) = NOTES_ARRAY.iter().position(|n| *n == tune) else {
        return Ok(0);
    };
    let octave: i32 = note[2..3]
        .parse()
        .map_err(|_| Error::Conversion(format!("note {note} is out of range")))?;
    if octave >= 8 {
        return Err(Error::Conversion(format!("note {note} is out of range")));
    }
    Ok((12 * octave + (index as i32 + 1)) as u8)
}

/// Panning byte for the 40-byte sample header. Fixed from the original's `(byte)Math.Abs(255 *
/// panning + 1)`, which wraps hard-right pan (`panning == 1.0` -> 256) to hard-left (0) when cast
/// to a byte -- one of the two agreed accidental-bug fixes. Clamped to 255 instead.
pub fn get_panning(panning: f32) -> u8 {
    ((255.0 * panning + 1.0).abs().round() as i32).clamp(0, 255) as u8
}

pub fn get_sample_loop_mode(loop_mode: LoopMode) -> u8 {
    match loop_mode {
        LoopMode::Off => 0,
        LoopMode::Forward => 1,
        LoopMode::PingPong => 2,
    }
}

/// Converts a loop point from frames to bytes at the sample's actual encoded format.
pub fn get_sample_loop_value(value: u32, bits_per_sample: u32, is_stereo: bool) -> u32 {
    value * (bits_per_sample / 8) * if is_stereo { 2 } else { 1 }
}

pub fn get_volume_panning_type(enabled: bool, sustain_enabled: bool, loop_enabled: bool) -> u8 {
    (enabled as u8) | ((sustain_enabled as u8) << 1) | ((loop_enabled as u8) << 2)
}

/// One (x, y) envelope point, `y` already in XM's 0-63-ish header units.
pub type EnvelopePoint = (i32, i32);

/// Builds the final envelope point list, inserting an interpolated point at the sustain/loop
/// position if Renoise didn't already define a point there (XM references sustain/loop by point
/// *index*, so a point must exist at that exact x). Mirrors `GetEnvelopePointsValue`.
pub fn build_envelope_points(
    raw_points: &[(f32, f32)],
    sustain_x: f32,
    loop_start_x: f32,
    loop_end_x: f32,
    sustain_enabled: bool,
    loop_enabled: bool,
) -> Vec<EnvelopePoint> {
    // Divide first, then round the final result -- rounding `127.0 * y` and *then*
    // integer-dividing by 2 systematically undershoots by 1 for values like y=1.0 (127.0.round()
    // = 127, 127/2 truncates to 63 instead of the correct 64). Verified against real envelope
    // data: y=1.0 must map to 64, not 63.
    let mut points: Vec<EnvelopePoint> = raw_points
        .iter()
        .map(|&(x, y)| (x.round() as i32, (127.0 * y / 2.0).abs().round() as i32))
        .collect();
    points.sort_by_key(|p| p.0);

    if sustain_enabled {
        let target = sustain_x.round() as i32;
        if !points.iter().any(|p| p.0 == target) {
            insert_interpolated_point(&mut points, target);
        }
    }
    if loop_enabled {
        let start = loop_start_x.round() as i32;
        if !points.iter().any(|p| p.0 == start) {
            insert_interpolated_point(&mut points, start);
        }
        let end = loop_end_x.round() as i32;
        if !points.iter().any(|p| p.0 == end) {
            insert_interpolated_point(&mut points, end);
        }
    }

    points
}

fn insert_interpolated_point(points: &mut Vec<EnvelopePoint>, x: i32) {
    let mut insert_at = points.len();
    let mut y = 0;

    for (i, &(px, py)) in points.iter().enumerate() {
        if x < px {
            y = if i > 0 {
                let (x1, y1) = points[i - 1];
                let (x2, y2) = (px, py);
                let a = (y2 - y1) as f64 / (x2 - x1) as f64;
                let b = y1 as f64 - a * x1 as f64;
                (a * x as f64 + b) as i32
            } else {
                py
            };
            insert_at = i;
            break;
        } else if i == points.len() - 1 {
            y = py;
            insert_at = i + 1;
        }
    }

    points.insert(insert_at, (x, y));
}

/// Point *index* (not byte offset) of the point whose x equals `value`, or 0 if not found.
/// Mirrors `GetPointNumber` (which searched a packed byte buffer; here it searches the point list
/// directly -- same result, no unnecessary pack/unpack round trip).
pub fn get_point_number(points: &[EnvelopePoint], value: i32) -> u8 {
    points.iter().position(|p| p.0 == value).unwrap_or(0) as u8
}

#[derive(Debug, Clone, Copy, Default)]
struct XmSampleInfo {
    related_note: i32,
    fine_tune: i32,
    /// Encoded sample length in frames.
    length: i32,
}

struct XmInstrumentInfo {
    key_map: [Option<u8>; 120],
    samples: Vec<XmSampleInfo>,
}

pub struct XmEncoder {
    ticks_per_row: i32,
    playback_engine_version: i32,
    pitch_compatibility_mode: bool,
    sample_offset_compatibility_mode: bool,
    instruments: Vec<XmInstrumentInfo>,
}

impl XmEncoder {
    pub fn new(
        key_maps: &[[Option<u8>; 120]],
        sample_counts: &[usize],
        initial_ticks_per_row: i32,
        playback_engine_version: i32,
        pitch_compatibility_mode: bool,
        sample_offset_compatibility_mode: bool,
    ) -> Self {
        let instruments = key_maps
            .iter()
            .zip(sample_counts.iter())
            .map(|(&key_map, &sample_count)| XmInstrumentInfo {
                key_map,
                samples: vec![XmSampleInfo::default(); sample_count],
            })
            .collect();

        Self {
            ticks_per_row: initial_ticks_per_row,
            playback_engine_version,
            pitch_compatibility_mode,
            sample_offset_compatibility_mode,
            instruments,
        }
    }

    pub fn set_ticks_per_row(&mut self, ticks: i32) {
        self.ticks_per_row = ticks;
    }

    pub fn ticks_per_row(&self) -> i32 {
        self.ticks_per_row
    }

    #[allow(clippy::too_many_arguments)]
    pub fn store_sample_info(
        &mut self,
        instrument_index: usize,
        sample_index: usize,
        encoded_length_bytes: i64,
        sample_rate: u32,
        channels: u32,
        bits_per_sample: u32,
        renoise_base_note: i32,
        renoise_fine_tuning: i32,
        transpose: i32,
    ) {
        // NTSC is hardcoded true for XM's base-note/finetune math -- confirmed against the
        // original's constructor chain (`ModUtils(songData, ticksPerRow) : this(..., ntscMode:
        // true)`, commented "NTSC shall be default for this constructor so XM mode doesn't get
        // changed"). PAL/NTSC is an Amiga hardware concept XM doesn't otherwise expose to the
        // user; unlike MOD, this is never configurable for XM output.
        const XM_NTSC_MODE: bool = true;

        let real_length = crate::common::calculate_sample_length(
            encoded_length_bytes as u64,
            bits_per_sample,
            channels,
        );
        let props = crate::common::get_sample_properties(
            sample_rate as f64,
            renoise_base_note,
            transpose,
            renoise_fine_tuning,
            XM_NTSC_MODE,
        );
        self.instruments[instrument_index].samples[sample_index] = XmSampleInfo {
            related_note: props.relative_tone,
            fine_tune: props.fine_tune,
            length: real_length as i32,
        };
    }

    pub fn sample_base_note(&self, instrument_index: usize, sample_index: usize) -> i32 {
        self.instruments[instrument_index].samples[sample_index].related_note
    }

    pub fn sample_fine_tune(&self, instrument_index: usize, sample_index: usize) -> i32 {
        self.instruments[instrument_index].samples[sample_index].fine_tune
    }

    /// Which sample slot plays for `note` (1-96) on `instrument` (1-based). Renoise notes with no
    /// explicit key-mapping default to sample slot 0 -- matching the original's C# `int[]`
    /// default-zero-initialization behavior for `KeyMap` entries that were never written by
    /// `SongDataFactory` (no explicit "unmapped" sentinel exists there).
    pub fn played_sample_from_keymap(&self, note: u8, instrument: u8) -> u8 {
        self.instruments[(instrument - 1) as usize]
            .key_map
            .get((note - 1) as usize)
            .copied()
            .flatten()
            .unwrap_or(0)
    }

    fn sample_length_frames(&self, xm_note: u8, xm_instrument: u8) -> i32 {
        if xm_note == 0 || xm_instrument == 0 {
            return 0;
        }
        let sample_used = self.played_sample_from_keymap(xm_note, xm_instrument);
        self.instruments[(xm_instrument - 1) as usize]
            .samples
            .get(sample_used as usize)
            .map(|s| s.length)
            .unwrap_or(0)
    }

    /// Mirrors `GetVolumeColumnEffectFromVolume(string)`.
    pub fn volume_column_effect_from_volume(&self, xrns_col_vol_eff: &str) -> u8 {
        let mut chars = xrns_col_vol_eff.chars();
        let Some(command) = chars.next() else {
            return 0;
        };
        let Some(hex_char) = chars.next() else {
            return 0;
        };
        let value = hex_char.to_digit(16).unwrap_or(0) as i32;

        match command {
            '0'..='8' => {
                let Ok(hex_val) = i32::from_str_radix(xrns_col_vol_eff, 16) else {
                    return 0;
                };
                xm_commands::set_volume_volume_column(hex_val)
            }
            'I' => xm_commands::volume_up_volume_column(value, self.ticks_per_row),
            'O' => xm_commands::volume_down_volume_column(value, self.ticks_per_row),
            _ => 0,
        }
    }

    /// Mirrors `GetVolumeColumnEffectFromPanning`.
    pub fn volume_column_effect_from_panning(&self, xrns_col_pan_eff: &str) -> u8 {
        let mut chars = xrns_col_pan_eff.chars();
        let Some(command) = chars.next() else {
            return 0;
        };
        let Some(hex_char) = chars.next() else {
            return 0;
        };
        let value = hex_char.to_digit(16).unwrap_or(0) as i32;

        match command {
            '0'..='8' => {
                let Ok(hex_val) = i32::from_str_radix(xrns_col_pan_eff, 16) else {
                    return 0;
                };
                xm_commands::set_panning_volume_column(hex_val)
            }
            'J' => xm_commands::pan_slide_left_volume_column(value, self.ticks_per_row),
            'K' => xm_commands::pan_slide_right_volume_column(value, self.ticks_per_row),
            _ => 0,
        }
    }

    fn get_sample_commands(
        &self,
        command: char,
        value: i32,
        xm_note: u8,
        xm_instrument: u8,
    ) -> (u8, u8) {
        let ticks = self.ticks_per_row;
        let pitch_compat = self.pitch_compatibility_mode;

        match command {
            'A' => shared_commands::arpeggio(value),
            'U' => xm_commands::portamento(1, value, ticks, false, pitch_compat).unwrap_or((0, 0)),
            'D' => xm_commands::portamento(2, value, ticks, false, pitch_compat).unwrap_or((0, 0)),
            'M' => shared_commands::set_volume(value),
            'G' => {
                xm_commands::tone_portamento(value, ticks, false, pitch_compat).unwrap_or((0, 0))
            }
            'I' => shared_commands::volume_up(value, ticks),
            'O' => shared_commands::volume_down(value, ticks),
            'P' => shared_commands::set_panning(value),
            'S' => {
                let sample_size = self.sample_length_frames(xm_note, xm_instrument);
                shared_commands::set_sample_offset(
                    value,
                    sample_size,
                    self.sample_offset_compatibility_mode,
                )
            }
            'B' => xm_commands::play_sample_backward(),
            'Q' => shared_commands::note_delay(value),
            'R' => xm_commands::multi_retrig(value),
            'V' => shared_commands::vibrato(value),
            'T' => shared_commands::tremolo(value),
            // C (volume slicer), W (surround width), L (pre-mixer track volume), N/E/J/X: not
            // implemented, matches the original.
            _ => (0, 0),
        }
    }

    fn get_global_commands(&self, command: char, value: i32) -> (u8, u8) {
        match command {
            'T' => xm_commands::set_tempo(value),
            'L' => {
                if self.playback_engine_version == 1 {
                    shared_commands::set_speed(value)
                } else {
                    (0, 0)
                }
            }
            'K' => shared_commands::set_speed(value),
            'B' => shared_commands::pattern_break(value),
            'D' => shared_commands::pattern_delay(value),
            _ => (0, 0),
        }
    }

    /// Mirrors `GetXMEffect`.
    pub fn get_xm_effect(
        &self,
        xrns_eff_num: &str,
        xrns_eff_val: &str,
        xm_note: u8,
        xm_instrument: u8,
    ) -> Result<(u8, u8)> {
        let mut chars = xrns_eff_num.chars();
        let eff_type = chars
            .next()
            .ok_or_else(|| Error::Conversion("empty effect number".to_string()))?;
        let command = chars
            .next()
            .ok_or_else(|| Error::Conversion("truncated effect number".to_string()))?;
        let value = i32::from_str_radix(xrns_eff_val, 16)
            .map_err(|_| Error::Conversion(format!("invalid effect value: {xrns_eff_val}")))?;

        Ok(match eff_type {
            '0' => self.get_sample_commands(command, value, xm_note, xm_instrument),
            'Z' => self.get_global_commands(command, value),
            _ => (0, 0),
        })
    }

    /// Mirrors `GetCommandsFromMasterTrack`. XM-native global-volume sample commands (`0M`/`0I`/
    /// `0O`) are also accepted from the master track, with no MOD equivalent.
    pub fn get_commands_from_master_track(
        &self,
        xrns_eff_num: &str,
        xrns_eff_val: &str,
        parse_only_global_volume: bool,
    ) -> Result<(u8, u8)> {
        let mut chars = xrns_eff_num.chars();
        let command_type = chars
            .next()
            .ok_or_else(|| Error::Conversion("empty effect number".to_string()))?;
        let command = chars
            .next()
            .ok_or_else(|| Error::Conversion("truncated effect number".to_string()))?;
        let value = i32::from_str_radix(xrns_eff_val, 16)
            .map_err(|_| Error::Conversion(format!("invalid effect value: {xrns_eff_val}")))?;

        Ok(match command_type {
            '0' => match command {
                'M' => xm_commands::set_global_volume(value),
                'I' => xm_commands::global_volume_slide_up(value, self.ticks_per_row),
                'O' => xm_commands::global_volume_slide_down(value, self.ticks_per_row),
                _ => (0, 0),
            },
            'Z' => {
                if parse_only_global_volume {
                    (0, 0)
                } else {
                    self.get_global_commands(command, value)
                }
            }
            _ => (0, 0),
        })
    }

    /// Mirrors `TransposeVolPanEffectColumnToEffectColumn`.
    fn transpose_vol_pan_to_effect(&self, command: char, value: i32) -> (u8, u8) {
        let ticks = self.ticks_per_row;
        let pitch_compat = self.pitch_compatibility_mode;

        match command {
            'U' => xm_commands::transpose_portamento_up_vol_pan_column(value, ticks, pitch_compat)
                .unwrap_or((0, 0)),
            'D' => {
                xm_commands::transpose_portamento_down_vol_pan_column(value, ticks, pitch_compat)
                    .unwrap_or((0, 0))
            }
            'G' => xm_commands::transpose_glide_vol_pan_column(value, ticks, pitch_compat)
                .unwrap_or((0, 0)),
            'B' => xm_commands::transpose_play_sample_direction_vol_pan_column(value),
            'Q' => shared_commands::note_delay(value),
            'R' => shared_commands::retrig_note(value),
            'C' => shared_commands::note_cut(value),
            _ => (0, 0),
        }
    }

    /// Mirrors `TransposeVolumeToCommandEffect`.
    pub fn transpose_volume_to_command_effect(&self, xrns_col_vol_eff: &str) -> (u8, u8) {
        let mut chars = xrns_col_vol_eff.chars();
        let Some(command) = chars.next() else {
            return (0, 0);
        };
        let Some(hex_char) = chars.next() else {
            return (0, 0);
        };
        let value = hex_char.to_digit(16).unwrap_or(0) as i32;
        let ticks = self.ticks_per_row;

        match command {
            '0'..='8' => {
                let Ok(hex_val) = i32::from_str_radix(xrns_col_vol_eff, 16) else {
                    return (0, 0);
                };
                shared_commands::transpose_set_volume_from_volume(hex_val)
            }
            'I' => shared_commands::transpose_volume_slide_up_from_volume(value, ticks),
            'O' => shared_commands::transpose_volume_slide_down_from_volume(value, ticks),
            _ => self.transpose_vol_pan_to_effect(command, value),
        }
    }

    /// Mirrors `TransposePanningToCommandEffect`.
    pub fn transpose_panning_to_command_effect(&self, xrns_col_pan_eff: &str) -> (u8, u8) {
        let mut chars = xrns_col_pan_eff.chars();
        let Some(command) = chars.next() else {
            return (0, 0);
        };
        let Some(hex_char) = chars.next() else {
            return (0, 0);
        };
        let value = hex_char.to_digit(16).unwrap_or(0) as i32;
        let ticks = self.ticks_per_row;

        match command {
            '0'..='8' => {
                let Ok(hex_val) = i32::from_str_radix(xrns_col_pan_eff, 16) else {
                    return (0, 0);
                };
                shared_commands::transpose_set_panning_from_panning(hex_val)
            }
            'J' => xm_commands::transpose_panning_slide_left(value, ticks),
            'K' => xm_commands::transpose_panning_slide_right(value, ticks),
            _ => self.transpose_vol_pan_to_effect(command, value),
        }
    }

    /// Mirrors `TransposeDelayToCommandEffect` -- unlike MOD's version, this one always marks the
    /// effect slot used, even when the computed delay rounds to a no-op `(0, 0)`. That looked
    /// like an oversight (it blocks the panning-column/master-track fallbacks that would
    /// otherwise still have a free slot), so it's changed here to only claim the slot when the
    /// command actually produced something, matching MOD's (and this module's own volume/panning
    /// column handling's) behavior.
    pub fn transpose_delay_to_command_effect(&self, xrns_col_delay: &str) -> (u8, u8) {
        let Ok(value) = i32::from_str_radix(xrns_col_delay, 16) else {
            return (0, 0);
        };
        let mut result = ((value * self.ticks_per_row) as f64 / 255.0).round() as i32;
        if result == self.ticks_per_row {
            result -= 1;
        }
        if result > 0 {
            shared_commands::note_delay(result)
        } else {
            (0, 0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xm_note_off_maps_to_97() {
        assert_eq!(get_xm_note("OFF").unwrap(), 97);
    }

    #[test]
    fn xm_note_c0_maps_to_1() {
        assert_eq!(get_xm_note("C-0").unwrap(), 1);
    }

    #[test]
    fn xm_note_octave_8_or_above_errors() {
        assert!(get_xm_note("C-8").is_err());
    }

    #[test]
    fn panning_clamps_instead_of_wrapping() {
        // Old (buggy) formula: abs(255*1.0+1) = 256 -> wraps to 0 as a byte. Fixed: clamps to 255.
        assert_eq!(get_panning(1.0), 255);
        assert_eq!(get_panning(0.0), 1);
    }

    #[test]
    fn envelope_point_interpolation_inserts_missing_sustain_point() {
        let points = build_envelope_points(&[(0.0, 1.0), (10.0, 0.0)], 5.0, 0.0, 0.0, true, false);
        assert_eq!(points.len(), 3);
        assert_eq!(points[1].0, 5);
        // linear interpolation halfway between y=63 (approx) and y=0 -> ~31.
        assert!(points[1].1 > 0 && points[1].1 < points[0].1);
    }

    #[test]
    fn get_point_number_finds_index_by_x() {
        let points = vec![(0, 10), (5, 20), (10, 0)];
        assert_eq!(get_point_number(&points, 5), 1);
        assert_eq!(get_point_number(&points, 999), 0);
    }
}
