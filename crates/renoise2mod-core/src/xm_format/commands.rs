//! XM-specific effect-command byte encoders (mirrors `XMCommands.cs`). Functions the original
//! inherited unchanged from `ModCommands` are reused directly from [`crate::commands`] at call
//! sites in `encoder.rs` rather than duplicated here.
//!
//! XM's own pitch-effect math is deliberately simpler than MOD's: no Amiga-period/channel-state
//! tracking, just linear division by ticks-per-row (consistent with XM's Linear Frequency Table
//! flag). It is a genuinely different formula from [`crate::commands`]'s MOD-side
//! `get_portamento_value` (no "never round to zero" clamp, and it throws instead of clamping) --
//! kept as its own function rather than unified, matching the original's own separate
//! implementation.

use crate::commands::fine_portamento_up_down;
use crate::common::{get_volume_slide_value, is_fine_portamento_closer_to_value};
use crate::error::{Error, Result};

fn tick_divider(ticks_per_row: i32) -> i32 {
    if ticks_per_row > 1 {
        ticks_per_row - 1
    } else {
        ticks_per_row
    }
}

/// Mirrors `XMCommands.GetPortamentoValue`. Divides by ticks-per-row when
/// `ignore_pitch_compatibility_mode` is true OR `pitch_compatibility_mode` is false; otherwise
/// passes the raw value through unchanged.
fn xm_portamento_value(
    original_value: i32,
    ticks_per_row: i32,
    ignore_pitch_compatibility_mode: bool,
    pitch_compatibility_mode: bool,
) -> Result<i32> {
    if ignore_pitch_compatibility_mode || !pitch_compatibility_mode {
        let divider = tick_divider(ticks_per_row);
        let value = if divider == 0 {
            0
        } else {
            (original_value as f64 / divider as f64).round() as i32
        };
        if value == 0 && original_value != 0 {
            return Err(Error::Conversion(
                "Converted portamento value was discarded because resulted to 0".to_string(),
            ));
        }
        Ok(value)
    } else {
        Ok(original_value)
    }
}

/// `21 9F` - a fixed FT2 extended command that plays the sample backwards.
pub fn play_sample_backward() -> (u8, u8) {
    (0x21, 0x9F)
}

/// Volume/panning-column `'B'` -> backward/forward playback toggle.
pub fn transpose_play_sample_direction_vol_pan_column(eff_val: i32) -> (u8, u8) {
    (0x21, (0x9F - eff_val) as u8)
}

/// `1Bxy` - multi-retrigger, remapping Renoise's volume-change nibble to XM's own table.
pub fn multi_retrig(eff_val: i32) -> (u8, u8) {
    let volume_value = (eff_val & 0xF0) >> 4;
    let ticks_value = eff_val & 0x0F;

    let converted_volume_value = match volume_value {
        0 | 1 | 6 | 7 | 9 | 0x0E | 0x0F => volume_value,
        2 | 3 | 4 | 0x0A | 0x0B | 0x0C => volume_value + 1,
        5 | 0x0D => volume_value + 2,
        8 => 0,
        _ => unreachable!("volume_value is always a 4-bit nibble"),
    };

    (0x1B, ((converted_volume_value << 4) + ticks_value) as u8)
}

/// `10xx` - set global (master) volume.
pub fn set_global_volume(eff_val: i32) -> (u8, u8) {
    let eff_val = eff_val.min(0xC0);
    (0x10, (eff_val / 3) as u8)
}

/// `11xx` (high nibble) - global volume slide up.
pub fn global_volume_slide_up(value: i32, ticks_per_row: i32) -> (u8, u8) {
    let v = get_volume_slide_value(value, ticks_per_row).min(0xF);
    (0x11, (v << 4) as u8)
}

/// `11xx` (low nibble) - global volume slide down.
pub fn global_volume_slide_down(value: i32, ticks_per_row: i32) -> (u8, u8) {
    let v = get_volume_slide_value(value, ticks_per_row).min(0xF);
    (0x11, v as u8)
}

/// `0Fxx` - set tempo (BPM). XM's own `Fxx` command is dual-purpose: values below 0x20 set the
/// tick speed instead (handled by `set_speed`, not here) -- this is a genuine format distinction
/// from MOD's own single-purpose tempo command, not an inconsistency to reconcile.
pub fn set_tempo(eff_val: i32) -> (u8, u8) {
    if eff_val < 0x20 {
        (0, 0)
    } else {
        (0x0F, eff_val as u8)
    }
}

/// `03xx` - tone portamento (glide), using XM's own linear pitch math.
pub fn tone_portamento(
    eff_val: i32,
    ticks_per_row: i32,
    ignore_pitch_compatibility_mode: bool,
    pitch_compatibility_mode: bool,
) -> Result<(u8, u8)> {
    let value = xm_portamento_value(
        eff_val,
        ticks_per_row,
        ignore_pitch_compatibility_mode,
        pitch_compatibility_mode,
    )?;
    Ok((0x03, value as u8))
}

fn portamento_up_down(
    eff_num: i32,
    eff_val: i32,
    ticks_per_row: i32,
    ignore_pitch_compatibility_mode: bool,
    pitch_compatibility_mode: bool,
) -> Result<(u8, u8)> {
    let value = xm_portamento_value(
        eff_val,
        ticks_per_row,
        ignore_pitch_compatibility_mode,
        pitch_compatibility_mode,
    )?;
    Ok((eff_num as u8, value as u8))
}

/// `01xx`/`02xx` (coarse) or `0E1x`/`0E2x` (fine) pitch slide, using XM's own linear pitch math
/// and a hardcoded zero accuracy-loss threshold (unlike MOD's configurable
/// `Settings.PortamentoLossThreshold`).
pub fn portamento(
    eff_num: i32,
    eff_val: i32,
    ticks_per_row: i32,
    ignore_pitch_compatibility_mode: bool,
    pitch_compatibility_mode: bool,
) -> Result<(u8, u8)> {
    const ACCURACY_LOSS_THRESHOLD: i32 = 0;

    let use_fine = !pitch_compatibility_mode
        && is_fine_portamento_closer_to_value(eff_val, ticks_per_row, ACCURACY_LOSS_THRESHOLD);

    if use_fine {
        Ok(fine_portamento_up_down(eff_num, eff_val))
    } else {
        portamento_up_down(
            eff_num,
            eff_val,
            ticks_per_row,
            ignore_pitch_compatibility_mode,
            pitch_compatibility_mode,
        )
    }
}

/// Volume/panning-column `'G'` -> glide, always dividing (ignore_pitch_compatibility_mode=true).
pub fn transpose_glide_vol_pan_column(
    eff_val: i32,
    ticks_per_row: i32,
    pitch_compatibility_mode: bool,
) -> Result<(u8, u8)> {
    const MAX_VALUE: u8 = 0xFF;
    if eff_val < 0x0F {
        let value =
            xm_portamento_value(eff_val << 4, ticks_per_row, true, pitch_compatibility_mode)?;
        Ok((0x03, value as u8))
    } else {
        Ok((0x03, MAX_VALUE))
    }
}

/// Volume/panning-column `'U'` -> pitch slide up, always dividing.
pub fn transpose_portamento_up_vol_pan_column(
    eff_val: i32,
    ticks_per_row: i32,
    pitch_compatibility_mode: bool,
) -> Result<(u8, u8)> {
    let value = xm_portamento_value(eff_val << 4, ticks_per_row, true, pitch_compatibility_mode)?;
    Ok((1, value as u8))
}

/// Volume/panning-column `'D'` -> pitch slide down, always dividing.
pub fn transpose_portamento_down_vol_pan_column(
    eff_val: i32,
    ticks_per_row: i32,
    pitch_compatibility_mode: bool,
) -> Result<(u8, u8)> {
    let value = xm_portamento_value(eff_val << 4, ticks_per_row, true, pitch_compatibility_mode)?;
    Ok((2, value as u8))
}

/// Plain truncating division (not rounded, unlike [`crate::common::get_portamento_value`]/
/// `get_volume_slide_value`) used only by the panning-slide volume-column commands. Mirrors
/// `XMCommands.GetPanningSlideValue`.
fn panning_slide_value(original_value: i32, ticks_per_row: i32) -> i32 {
    let divider = tick_divider(ticks_per_row);
    if divider == 0 {
        0
    } else {
        original_value / divider
    }
}

/// Volume-column set-volume byte (`0x10-0x50` range).
pub fn set_volume_volume_column(eff_val: i32) -> u8 {
    ((eff_val >> 1) + 0x10) as u8
}

/// Volume-column set-panning byte (`0xC0-0xCF` range).
pub fn set_panning_volume_column(eff_val: i32) -> u8 {
    let v = (eff_val >> 3).min(0xF);
    (v + 0xC0) as u8
}

pub fn fine_volume_up_volume_column(eff_val: i32) -> u8 {
    let v = ((eff_val & 0xF) << 2).min(0xF);
    (v + 0x90) as u8
}

pub fn fine_volume_down_volume_column(eff_val: i32) -> u8 {
    let v = ((eff_val & 0xF) << 2).min(0xF);
    (v + 0x80) as u8
}

pub fn volume_slide_up_volume_column(eff_val: i32, ticks_per_row: i32) -> u8 {
    let v = get_volume_slide_value((eff_val & 0xF) << 4, ticks_per_row).min(0xF);
    (v + 0x70) as u8
}

pub fn volume_slide_down_volume_column(eff_val: i32, ticks_per_row: i32) -> u8 {
    let v = get_volume_slide_value((eff_val & 0xF) << 4, ticks_per_row).min(0xF);
    (v + 0x60) as u8
}

/// Volume-column `'I'` -> volume fade in, fine below 0x05, otherwise a tick-based slide.
pub fn volume_up_volume_column(eff_val: i32, ticks_per_row: i32) -> u8 {
    if eff_val < 0x05 {
        fine_volume_up_volume_column(eff_val)
    } else {
        volume_slide_up_volume_column(eff_val, ticks_per_row)
    }
}

/// Volume-column `'O'` -> volume fade out, fine below 0x05, otherwise a tick-based slide.
pub fn volume_down_volume_column(eff_val: i32, ticks_per_row: i32) -> u8 {
    if eff_val < 0x05 {
        fine_volume_down_volume_column(eff_val)
    } else {
        volume_slide_down_volume_column(eff_val, ticks_per_row)
    }
}

/// Panning-column `'J'` -> panning slide left volume-column byte (`0xD0-0xDF`). Used by
/// `GetVolumeColumnEffectFromPanning` -- distinct from [`transpose_panning_slide_left`] below,
/// which produces an effect-column command pair instead.
pub fn pan_slide_left_volume_column(eff_val: i32, ticks_per_row: i32) -> u8 {
    let clamped = eff_val.min(0x08) << 4;
    let v = panning_slide_value(clamped, ticks_per_row).min(0x0F);
    (v + 0xD0) as u8
}

/// Panning-column `'K'` -> panning slide right volume-column byte (`0xE0-0xEF`). See
/// [`pan_slide_left_volume_column`].
pub fn pan_slide_right_volume_column(eff_val: i32, ticks_per_row: i32) -> u8 {
    let clamped = eff_val.min(0x08) << 4;
    let v = panning_slide_value(clamped, ticks_per_row).min(0x0F);
    (v + 0xE0) as u8
}

/// `19xx` (high nibble) - panning slide left, as an effect-column command pair. Mirrors
/// `TransposePanningSlideLeft` (used by `TransposePanningToCommandEffect`, not the volume-column
/// path above -- no `0x08` pre-clamp here, unlike the volume-column variant).
pub fn transpose_panning_slide_left(eff_val: i32, ticks_per_row: i32) -> (u8, u8) {
    let v = panning_slide_value(eff_val << 4, ticks_per_row).min(0x0F);
    (0x19, (v << 4) as u8)
}

/// `19xx` (low nibble) - panning slide right, as an effect-column command pair. Mirrors
/// `TransposePanningSlideRight`.
pub fn transpose_panning_slide_right(eff_val: i32, ticks_per_row: i32) -> (u8, u8) {
    let v = panning_slide_value(eff_val << 4, ticks_per_row).min(0x0F);
    (0x19, v as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_tempo_threshold_is_strictly_less_than_020() {
        // XM's own dual-purpose Fxx command: differs from MOD's `<=` on purpose (see doc comment).
        assert_eq!(set_tempo(0x1F), (0, 0));
        assert_eq!(set_tempo(0x20), (0x0F, 0x20));
    }

    #[test]
    fn xm_portamento_value_passes_through_raw_when_pitch_compat_and_not_ignored() {
        let (num, val) = portamento_up_down(1, 42, 6, false, true).unwrap();
        assert_eq!((num, val), (1, 42));
    }

    #[test]
    fn xm_portamento_value_divides_when_ignored() {
        let (num, val) = portamento_up_down(1, 40, 6, true, true).unwrap();
        assert_eq!(num, 1);
        assert_eq!(val, 8); // 40 / (6-1) = 8
    }

    #[test]
    fn multi_retrig_remaps_renoise_volume_table() {
        // Renoise 2 -> XM 3 (per conversion table in the doc comment above MultiRetrig).
        let (cmd, val) = multi_retrig(0x25); // volume=2, ticks=5
        assert_eq!(cmd, 0x1B);
        assert_eq!(val, (3 << 4) + 5);
    }

    #[test]
    fn set_global_volume_clamps_and_divides_by_three() {
        assert_eq!(set_global_volume(0xC0), (0x10, 0x40));
        assert_eq!(set_global_volume(0xFF), (0x10, 0x40)); // clamped to 0xC0 first
    }
}
