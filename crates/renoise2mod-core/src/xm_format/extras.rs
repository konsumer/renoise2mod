//! `VolumeScalingMode::Column` support -- rewrites already-encoded volume-column/effect-column
//! bytes per note-trigger instead of touching PCM. XM-only (mirrors `XMExtras.cs`).

use crate::error::{Error, Result};

const MAX_SAMPLE_VOLUME: i32 = 0x40;
const VOLUME_COLUMN_SET_VOLUME_DELTA: u8 = 0x10;

/// Is `volume` a "set volume" value in the volume-column's `0x10-0x50` range? Uses a wrapping
/// subtraction (matching the original's implicit byte-underflow trick): values below `0x10`
/// wrap around to a large number and correctly fail the `<= 0x40` check.
pub fn is_volume_set_on_volume_column(volume: u8) -> bool {
    volume.wrapping_sub(VOLUME_COLUMN_SET_VOLUME_DELTA) <= MAX_SAMPLE_VOLUME as u8
}

/// Is `command` the "set channel volume" effect (`0x0C`)?
pub fn is_volume_set_on_effect_column(command: u8) -> bool {
    command == 0x0C
}

/// Scales an existing volume-column set-volume byte by `volume_factor`. Errors (rather than
/// clamping) if the result would exceed nominal volume -- `Column` mode can only attenuate.
pub fn scale_volume_from_volume_command(value: u8, volume_factor: f32) -> Result<u8> {
    let original = value as i32 - VOLUME_COLUMN_SET_VOLUME_DELTA as i32;
    let scaled = (original as f32 * volume_factor) as i32;
    if scaled > MAX_SAMPLE_VOLUME {
        return Err(Error::Conversion(format!(
            "Volume scaling failed, result value: {scaled}"
        )));
    }
    Ok((scaled + VOLUME_COLUMN_SET_VOLUME_DELTA as i32) as u8)
}

/// Writes a *new* volume-column set-volume command scaled from the sample's nominal max volume.
pub fn scale_volume_from_volume_command_new(volume_factor: f32) -> Result<u8> {
    scale_volume_from_volume_command(
        (MAX_SAMPLE_VOLUME + VOLUME_COLUMN_SET_VOLUME_DELTA as i32) as u8,
        volume_factor,
    )
}

/// Scales an existing `0x0C` (set channel volume) effect value by `volume_factor`.
pub fn scale_volume_from_effect_command(value: u8, volume_factor: f32) -> Result<u8> {
    let result = (value as f32 * volume_factor) as i32;
    if result > MAX_SAMPLE_VOLUME {
        return Err(Error::Conversion(format!(
            "Volume scaling failed, result value: {result}"
        )));
    }
    Ok(result as u8)
}

/// Writes a *new* `0x0C` effect command scaled from the sample's nominal max volume.
pub fn scale_volume_from_effect_command_new(volume_factor: f32) -> Result<(u8, u8)> {
    let value = scale_volume_from_effect_command(MAX_SAMPLE_VOLUME as u8, volume_factor)?;
    Ok((0x0C, value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_set_range_detection_matches_wraparound_behavior() {
        assert!(is_volume_set_on_volume_column(0x10));
        assert!(is_volume_set_on_volume_column(0x50));
        assert!(!is_volume_set_on_volume_column(0x51));
        assert!(!is_volume_set_on_volume_column(0x05));
    }

    #[test]
    fn scaling_above_nominal_volume_errors() {
        assert!(scale_volume_from_volume_command(0x50, 2.0).is_err());
    }

    #[test]
    fn scaling_within_range_succeeds() {
        // original = 0x50-0x10 = 0x40 (64); *0.5 = 32; +0x10 = 0x30
        assert_eq!(scale_volume_from_volume_command(0x50, 0.5).unwrap(), 0x30);
    }
}
