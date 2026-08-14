//! The Amiga period table and note-range checks (mirrors `ModUtils.cs`'s `PeriodsRange` and the
//! `isNoteInRange`/`isNoteInPeriodRange` checks).
//!
//! Values are taken verbatim from the C# source, not recomputed from a pure semitone formula --
//! they're real historical ProTracker tuning constants and deviate slightly from an idealized
//! `1712 / 2^(n/12)` curve.

/// 72 entries (6 octaves), indexed by a "PeriodsRange index" that is a Renoise absolute note
/// number minus 24 (Renoise note 24 == this table's index 0). Verified directly against
/// `ModUtils.cs` -- an earlier prose description of this table claimed 96 entries/8 octaves,
/// which the literal source array does not match; trust this array, not that count.
pub const PERIODS_RANGE: [i32; 72] = [
    1712, 1616, 1524, 1440, 1356, 1280, 1208, 1140, 1076, 1016, 960, 907, // octave 0
    856, 808, 762, 720, 678, 640, 604, 570, 538, 508, 480, 453, // octave 1
    428, 404, 381, 360, 339, 320, 302, 285, 269, 254, 240, 226, // octave 2
    214, 202, 190, 180, 170, 160, 151, 143, 135, 127, 120, 113, // octave 3
    107, 101, 95, 90, 85, 80, 75, 71, 67, 63, 60, 56, // octave 4
    53, 50, 47, 45, 42, 40, 37, 35, 33, 31, 30, 28, // octave 5
];

pub const NOTE_VALUE_C1: i32 = 12;
pub const NOTE_VALUE_C2: i32 = 24;
pub const NOTE_VALUE_C3: i32 = 36;
pub const NOTE_VALUE_A3: i32 = 45;
pub const NOTE_VALUE_B3: i32 = 47;
#[allow(dead_code)] // Kept for parity with the source constant table; not currently referenced.
pub const NOTE_VALUE_C4: i32 = 48;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProTrackerCompatibility {
    None,
    /// Real ProTracker's own playable range (up to B-3).
    B3Max,
    /// Stricter still: tested against real Amiga hardware DMA limits (up to A-3).
    A3Max,
}

/// Plain bounds check against the table length only (mirrors `isNoteInPeriodRange`).
pub fn is_note_in_period_range(note_index: i32) -> bool {
    (0..PERIODS_RANGE.len() as i32).contains(&note_index)
}

/// Bounds check further restricted by the configured ProTracker compatibility mode (mirrors
/// `isNoteInRange`).
pub fn is_note_in_range(note_index: i32, compat: ProTrackerCompatibility) -> bool {
    let mut in_range = is_note_in_period_range(note_index);
    if compat != ProTrackerCompatibility::None && note_index < NOTE_VALUE_C1 {
        in_range = false;
    }
    if compat == ProTrackerCompatibility::A3Max && note_index > NOTE_VALUE_A3 {
        in_range = false;
    }
    if compat == ProTrackerCompatibility::B3Max && note_index > NOTE_VALUE_B3 {
        in_range = false;
    }
    in_range
}

const NOTES_ARRAY: [&str; 12] = [
    "C-", "C#", "D-", "D#", "E-", "F-", "F#", "G-", "G#", "A-", "A#", "B-",
];

/// Parses a 3-character Renoise/MOD-style note name (e.g. `"C-4"`) into its raw `(octave,
/// note_offset)` parts, with no table-index adjustment or range checking applied. Shared by both
/// `GetModNote` variants that parse note strings -- they each apply a different adjustment to
/// this raw pair (see `mod_format::encoder`).
pub fn parse_note_name_parts(note: &str) -> Option<(i32, i32)> {
    if note.len() != 3 {
        return None;
    }
    let tune = &note[0..2];
    let octave: i32 = note[2..3].parse().ok()?;
    let note_offset = NOTES_ARRAY.iter().position(|n| *n == tune)? as i32;
    Some((octave, note_offset))
}

/// Parses a 3-character MOD-style note name (e.g. `"C-2"`) into its `PeriodsRange` index, without
/// any ProTracker-compatibility clamping (mirrors the stateless `GetModNote(string note)`
/// overload, used only for parsing the sample-rate-selection "explicit note name" ini setting).
pub fn parse_note_name_to_period_index(note: &str) -> Option<i32> {
    let (octave, note_offset) = parse_note_name_parts(note)?;
    let note_index = octave * 12 + note_offset;
    is_note_in_period_range(note_index).then_some(note_index)
}
