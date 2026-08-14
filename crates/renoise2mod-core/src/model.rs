//! Internal song representation shared by both the MOD and XM writers.
//!
//! This mirrors the C# original's `SongStruct.cs` intermediate model on purpose: it decouples
//! "parse Renoise's sparse per-line/per-column XML" from "encode into a specific tracker format,"
//! so both `mod_format` and `xm_format` consume the exact same flattened structure.

/// A single Renoise pattern-line cell for one note sub-column (a "channel" in tracker terms).
///
/// Fields hold the raw Renoise strings unmodified (e.g. `"C-4"`, `"OFF"`, `"0G"`, `"ZT"`) --
/// interpreting them into tracker effect bytes is entirely the job of the per-format writers.
#[derive(Debug, Clone, Default)]
pub struct TrackLineData {
    pub is_set: bool,
    pub note: Option<String>,
    pub instrument: Option<String>,
    pub volume: Option<String>,
    pub panning: Option<String>,
    pub delay: Option<String>,
    /// Only effect-column index 0 of a Renoise track is ever read (matches the original).
    pub effect_number: String,
    pub effect_value: String,
}

#[derive(Debug, Clone, Default)]
pub struct MasterTrackLineData {
    pub effect_number: String,
    pub effect_value: String,
}

#[derive(Debug, Clone, Default)]
pub struct PatternData {
    pub num_rows: usize,
    /// Flat, indexed `row * num_channels + channel`.
    pub tracks_line_data: Vec<TrackLineData>,
    /// Flat, indexed `row * num_master_track_columns + column`.
    pub master_track_line_data: Vec<MasterTrackLineData>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LoopMode {
    #[default]
    Off,
    Forward,
    PingPong,
}

/// Per-sample rate selection. In the C# original this came from a side-channel `.ini` file since
/// Song.xml never carries it; here it's supplied by the CLI (see `xrns2mod-cli`) with the same
/// vocabulary/defaults as the original.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum SampleFreqMode {
    #[default]
    Low,
    High,
    Maximum,
    Original,
    /// A literal MOD note name, e.g. `"C-2"`.
    NoteName(String),
    /// An explicit sample rate in Hz.
    Hz(u32),
}

#[derive(Debug, Clone, Default)]
pub struct SampleData {
    pub name: String,
    pub loop_start: u32,
    pub loop_end: u32,
    pub loop_mode: LoopMode,
    /// `abs()` of Renoise's own sample volume float, matching the original's sign-flip.
    pub volume: f32,
    pub panning: f32,
    pub fine_tune: i8,
    pub transpose: i8,
    /// Base note ("related note"), 0-119 in Renoise's numbering (C-4 == 48).
    pub rel_note_number: i8,
    pub default_volume: u8,
    pub sample_freq: SampleFreqMode,
    pub sinc_interpolation_points: u8,
}

impl SampleData {
    pub fn new() -> Self {
        Self {
            default_volume: 64,
            sinc_interpolation_points: 2,
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct EnvelopeData {
    pub enabled: bool,
    /// (x, y) points, x in envelope time units, y in 0.0..=1.0.
    pub points: Vec<(f32, f32)>,
    pub fade_out: u16,
    pub sustain_enabled: bool,
    pub sustain_point_x: f32,
    pub loop_enabled: bool,
    pub loop_start_x: f32,
    pub loop_end_x: f32,
}

#[derive(Debug, Clone)]
pub struct InstrumentData {
    pub name: String,
    /// Note (0..120) -> sample slot index. `None` where no note-on mapping exists.
    pub key_map: [Option<u8>; 120],
    pub volume_envelope: EnvelopeData,
    pub panning_envelope: EnvelopeData,
    pub samples: Vec<SampleData>,
}

impl Default for InstrumentData {
    fn default() -> Self {
        Self {
            name: String::new(),
            key_map: [None; 120],
            volume_envelope: EnvelopeData::default(),
            panning_envelope: EnvelopeData::default(),
            samples: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct SongData {
    pub name: String,
    pub restart_position: u16,
    pub num_channels: usize,
    pub num_master_track_columns: usize,
    pub initial_bpm: u32,
    pub lines_per_beat: u32,
    pub ticks_per_line: u32,
    pub sample_offset_compatibility_mode: bool,
    pub pitch_compatibility_mode: bool,
    /// Compared against the "TIMING MODEL SPEED" compatible version (1) to decide whether the
    /// global LPB-set command is `'L'` or `'K'`.
    pub playback_engine_version: i32,
    pub pattern_order_table: Vec<u8>,
    pub patterns: Vec<PatternData>,
    pub instruments: Vec<InstrumentData>,
}

impl PatternData {
    pub fn track_line(&self, row: usize, channel: usize, num_channels: usize) -> &TrackLineData {
        &self.tracks_line_data[row * num_channels + channel]
    }

    pub fn master_line(&self, row: usize, col: usize, num_cols: usize) -> &MasterTrackLineData {
        &self.master_track_line_data[row * num_cols + col]
    }
}
