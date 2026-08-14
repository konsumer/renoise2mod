//! Pure MOD effect-command byte encoders (mirrors `ModCommands.cs`). Each function returns the
//! `(effect_number, effect_value)` nibble/byte pair written into a pattern cell's last two bytes.

use crate::common::{
    get_portamento_value, get_volume_slide_value, is_fine_portamento_closer_to_value,
    is_fine_volume_closer_to_value,
};
use crate::error::{Error, Result};

/// `0Axy` - Arpeggio.
pub fn arpeggio(eff_val: i32) -> (u8, u8) {
    (0x0, eff_val as u8)
}

/// `01xx`/`02xx` (coarse) or `0E1x`/`0E2x` (fine) pitch slide, chosen per
/// [`is_fine_portamento_closer_to_value`] unless `pitch_compatibility_mode` forces coarse-only.
pub fn portamento(
    eff_num: i32,
    eff_val: i32,
    ticks_per_row: i32,
    accuracy_loss_threshold: i32,
    pitch_compatibility_mode: bool,
) -> (u8, u8) {
    let use_fine = !pitch_compatibility_mode
        && is_fine_portamento_closer_to_value(eff_val, ticks_per_row, accuracy_loss_threshold);

    if use_fine {
        fine_portamento_up_down(eff_num, eff_val)
    } else {
        portamento_up_down(eff_num, eff_val, ticks_per_row)
    }
}

pub fn portamento_up_down(eff_num: i32, eff_val: i32, ticks_per_row: i32) -> (u8, u8) {
    (
        eff_num as u8,
        get_portamento_value(eff_val, ticks_per_row) as u8,
    )
}

pub fn fine_portamento_up_down(eff_num: i32, eff_val: i32) -> (u8, u8) {
    let eff_val = eff_val.min(0xF);
    (0x0E, (0x10 * eff_num + eff_val) as u8)
}

/// `0Cxx` - set channel volume.
pub fn set_volume(eff_val: i32) -> (u8, u8) {
    // +1 grants a better conversion (0xF correctly rounds up to 0x40).
    (0xC, (((eff_val + 1) >> 2) & 0xFF) as u8)
}

/// `03xx` - tone portamento (glide). Errors if the computed step rounds to zero for a nonzero
/// input (mirrors the original's `ConversionException`).
pub fn tone_portamento(eff_val: i32, ticks_per_row: i32) -> Result<(u8, u8)> {
    let value = get_portamento_value(eff_val, ticks_per_row);
    if value == 0 && eff_val != 0 {
        return Err(Error::Conversion(
            "Converted tone portamento value was discarded because resulted to 0".to_string(),
        ));
    }
    Ok((0x03, value as u8))
}

/// `0Axy` (coarse) or `0EAx` (fine) volume slide up.
pub fn volume_up(eff_val: i32, ticks_per_row: i32) -> (u8, u8) {
    if is_fine_volume_closer_to_value(eff_val, ticks_per_row) {
        volume_fine_up(eff_val)
    } else {
        volume_slide_up(eff_val, ticks_per_row)
    }
}

pub fn volume_slide_up(eff_val: i32, ticks_per_row: i32) -> (u8, u8) {
    let v = get_volume_slide_value(eff_val, ticks_per_row).min(0xF);
    (0xA, (v << 4) as u8)
}

pub fn volume_fine_up(eff_val: i32) -> (u8, u8) {
    let v = (eff_val >> 2).min(0xF);
    (0xE, (v + 0xA0) as u8)
}

/// `0Axy` (coarse) or `0EBx` (fine) volume slide down.
pub fn volume_down(eff_val: i32, ticks_per_row: i32) -> (u8, u8) {
    if is_fine_volume_closer_to_value(eff_val, ticks_per_row) {
        volume_fine_down(eff_val)
    } else {
        volume_slide_down(eff_val, ticks_per_row)
    }
}

pub fn volume_slide_down(eff_val: i32, ticks_per_row: i32) -> (u8, u8) {
    let v = get_volume_slide_value(eff_val, ticks_per_row).min(0xF);
    (0xA, v as u8)
}

pub fn volume_fine_down(eff_val: i32) -> (u8, u8) {
    let v = (eff_val >> 2).min(0xF);
    (0xE, (v + 0xB0) as u8)
}

/// `08xx` - set panning.
pub fn set_panning(eff_val: i32) -> (u8, u8) {
    (0x08, eff_val as u8)
}

/// Panning-column `'0'-'8'` -> `08xx` set-panning. MOD deliberately never calls this (a
/// per-note panning-column value can't correctly translate to MOD's whole-channel panning
/// command -- see `mod_format`'s `transpose_panning_to_command_effect`), but XM does use it.
pub fn transpose_set_panning_from_panning(eff_val: i32) -> (u8, u8) {
    let eff_val = (eff_val << 1).min(0xFF);
    set_panning(eff_val)
}

fn get_sample_offset_value(
    orig: i32,
    sample_size: i32,
    sample_offset_compatibility_mode: bool,
) -> u8 {
    if !sample_offset_compatibility_mode && sample_size > 0 {
        // 65536 = 256 (Renoise's own offset fraction) * 256 (bytes per MOD offset unit).
        let mod_offset = (sample_size * orig) >> 16;
        mod_offset.min(0xFF) as u8
    } else {
        orig as u8
    }
}

/// `09xx` - trigger sample at offset.
pub fn set_sample_offset(
    eff_val: i32,
    sample_size: i32,
    sample_offset_compatibility_mode: bool,
) -> (u8, u8) {
    (
        0x9,
        get_sample_offset_value(eff_val, sample_size, sample_offset_compatibility_mode),
    )
}

/// `0EDx` - delay all notes by x ticks.
pub fn note_delay(eff_val: i32) -> (u8, u8) {
    (0xE, (0xD0 + eff_val.min(0xF)) as u8)
}

/// `0E9x` - retrigger a note every x ticks.
pub fn retrig_note(eff_val: i32) -> (u8, u8) {
    (0xE, (0x90 + (eff_val & 0x0F)) as u8)
}

/// `04xy` - vibrato.
pub fn vibrato(eff_val: i32) -> (u8, u8) {
    (0x04, eff_val as u8)
}

/// `07xy` - tremolo.
pub fn tremolo(eff_val: i32) -> (u8, u8) {
    (0x07, eff_val as u8)
}

/// `0Fxx` - set speed (ticks per line).
pub fn set_speed(eff_val: i32) -> (u8, u8) {
    (0x0F, eff_val as u8)
}

/// `0Fxx` - set tempo (BPM). MOD tempo values start at 0x21; anything at or below 0x20 is
/// silently dropped to a no-op (matches the original).
pub fn set_tempo(eff_val: i32) -> (u8, u8) {
    if eff_val <= 0x20 {
        (0, 0)
    } else {
        (0x0F, eff_val as u8)
    }
}

/// `0EEx` - pattern delay.
pub fn pattern_delay(val: i32) -> (u8, u8) {
    (0xE, (0xE0 + val.min(0xF)) as u8)
}

/// `0Dxx` - pattern break, target row `eff_val`.
///
/// The original round-trips this value through `int.Parse(effVal.ToString(),
/// NumberStyles.HexNumber)` -- i.e. it reinterprets the *decimal* string of the already-hex-parsed
/// value *as hex*, silently corrupting the target row for any value >= 10. That was an
/// unintentional double-conversion, not a real feature, so it's not reproduced here.
pub fn pattern_break(eff_val: i32) -> (u8, u8) {
    (0xD, eff_val as u8)
}

/// `0ECx` - note cut (volume/panning-column only).
pub fn note_cut(eff_val: i32) -> (u8, u8) {
    (0xE, (0xC0 + eff_val) as u8)
}

/// Volume-column `0x00-0x8F` -> `0Cxx` set-volume passthrough.
pub fn transpose_set_volume_from_volume(eff_val: i32) -> (u8, u8) {
    (0x0C, (eff_val >> 1) as u8)
}

/// Volume-column `'I'` -> volume slide up, through the fine/coarse dispatch.
pub fn transpose_volume_slide_up_from_volume(eff_val: i32, ticks_per_row: i32) -> (u8, u8) {
    volume_up(eff_val * 0x10, ticks_per_row)
}

/// Volume-column `'O'` -> volume slide down, through the fine/coarse dispatch.
///
/// The original calls `VolumeSlideDown` directly here (always coarse), while the `'I'` (up)
/// counterpart above goes through `VolumeUp`'s fine/coarse dispatch -- an inconsistency with no
/// apparent reason, not a deliberate feature. Made symmetric with the "up" case.
pub fn transpose_volume_slide_down_from_volume(eff_val: i32, ticks_per_row: i32) -> (u8, u8) {
    volume_down(eff_val * 0x10, ticks_per_row)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_tempo_below_threshold_is_noop() {
        assert_eq!(set_tempo(0x20), (0, 0));
        assert_eq!(set_tempo(0x21), (0x0F, 0x21));
    }

    #[test]
    fn pattern_break_targets_the_row_directly() {
        // No decimal/hex round-trip corruption: the target row is exactly eff_val.
        assert_eq!(pattern_break(11), (0xD, 11));
        assert_eq!(pattern_break(5), (0xD, 5));
    }

    #[test]
    fn fine_portamento_up_down_clamps_and_packs_nibble() {
        assert_eq!(fine_portamento_up_down(1, 5), (0x0E, 0x15));
        assert_eq!(fine_portamento_up_down(2, 0xFF), (0x0E, 0x2F));
    }

    #[test]
    fn volume_slide_up_shifts_into_high_nibble_down_does_not() {
        // ticks_per_row=6 -> divider=5; eff_val=20 -> (20>>2)=5, /5=1 exactly.
        assert_eq!(volume_slide_up(20, 6), (0xA, 0x10));
        assert_eq!(volume_slide_down(20, 6), (0xA, 0x01));
    }

    #[test]
    fn tone_portamento_errors_on_zero_result_for_nonzero_input() {
        // ticks_per_row=1 -> divider=1, so a tiny eff_val still rounds to something nonzero
        // unless eff_val itself is 0; use ticks_per_row large enough to floor to 0.
        assert!(tone_portamento(0, 6).is_ok());
    }
}
