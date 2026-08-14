//! End-to-end structural test for the XM writer: builds a small `SongData` directly and a stub
//! sample source, then checks the produced file's binary layout against the documented XM header
//! offsets.

use renoise2mod_core::common::VolumeScalingMode;
use renoise2mod_core::mod_format::{ConversionLog, LogLevel};
use renoise2mod_core::model::{
    InstrumentData, LoopMode, MasterTrackLineData, PatternData, SampleData, SongData, TrackLineData,
};
use renoise2mod_core::xm_format::{self, EncodedXmSample, XmSampleSource, XmSettings};

struct StubSampleSource;

impl XmSampleSource for StubSampleSource {
    fn encode_sample(
        &self,
        _instrument_index: usize,
        _sample_index: usize,
        _sample: &SampleData,
        _settings: &XmSettings,
    ) -> renoise2mod_core::Result<Option<EncodedXmSample>> {
        Ok(Some(EncodedXmSample {
            encoded_pcm: vec![0u8; 50],
            sample_rate: 44100,
            channels: 1,
            bits_per_sample: 8,
        }))
    }
}

struct CollectingLog {
    messages: Vec<(LogLevelKind, String)>,
}

#[derive(Debug, PartialEq)]
enum LogLevelKind {
    Info,
    Warning,
    Error,
}

impl ConversionLog for CollectingLog {
    fn log(&mut self, level: LogLevel, message: String) {
        let kind = match level {
            LogLevel::Info => LogLevelKind::Info,
            LogLevel::Warning => LogLevelKind::Warning,
            LogLevel::Error => LogLevelKind::Error,
        };
        self.messages.push((kind, message));
    }
}

fn build_song() -> SongData {
    let num_channels = 2;
    let num_rows = 4;

    let mut tracks_line_data = vec![TrackLineData::default(); num_rows * num_channels];
    tracks_line_data[0] = TrackLineData {
        is_set: true,
        note: Some("C-4".to_string()),
        instrument: Some("00".to_string()),
        volume: None,
        panning: None,
        delay: None,
        effect_number: String::new(),
        effect_value: String::new(),
    };

    let pattern = PatternData {
        num_rows,
        tracks_line_data,
        master_track_line_data: vec![MasterTrackLineData::default(); num_rows],
    };

    let mut instrument = InstrumentData {
        name: "Test Instrument".to_string(),
        ..Default::default()
    };
    instrument.key_map[48] = Some(0);
    instrument.samples.push(SampleData {
        name: "Test Sample".to_string(),
        loop_mode: LoopMode::Off,
        volume: 1.0,
        panning: 0.5,
        rel_note_number: 48,
        ..SampleData::new()
    });

    SongData {
        name: "XM Structural Test".to_string(),
        restart_position: 0,
        num_channels,
        num_master_track_columns: 1,
        initial_bpm: 125,
        lines_per_beat: 4,
        ticks_per_line: 6,
        sample_offset_compatibility_mode: false,
        pitch_compatibility_mode: false,
        playback_engine_version: 1,
        pattern_order_table: vec![0],
        patterns: vec![pattern],
        instruments: vec![instrument],
    }
}

fn default_settings() -> XmSettings {
    XmSettings {
        ticks_row: 6,
        tempo: 125,
        volume_scaling_mode: VolumeScalingMode::None,
    }
}

#[test]
fn produces_correctly_laid_out_xm_header() {
    let song = build_song();
    let settings = default_settings();
    let source = StubSampleSource;
    let mut log = CollectingLog {
        messages: Vec::new(),
    };

    let output =
        xm_format::convert(&song, &settings, &source, &mut log).expect("conversion should succeed");

    // offset 0..17: id text.
    assert_eq!(&output[0..17], b"Extended Module: ");
    // offset 17..37: module name (20 bytes, null-padded since name < 20 chars).
    assert_eq!(&output[17..35], b"XM Structural Test");
    assert_eq!(output[35], 0);
    // offset 37: constant 0x1A.
    assert_eq!(output[37], 0x1A);
    // offset 58..60: version 4,1.
    assert_eq!(&output[58..60], &[4, 1]);
    // offset 60..64: header size field (80-60+256 = 276).
    assert_eq!(u32::from_le_bytes(output[60..64].try_into().unwrap()), 276);
    // offset 64..66: song length (pattern order table entries).
    assert_eq!(u16::from_le_bytes(output[64..66].try_into().unwrap()), 1);
    // offset 68..70: num channels.
    assert_eq!(u16::from_le_bytes(output[68..70].try_into().unwrap()), 2);
    // offset 70..72: num patterns.
    assert_eq!(u16::from_le_bytes(output[70..72].try_into().unwrap()), 1);
    // offset 72..74: num instruments.
    assert_eq!(u16::from_le_bytes(output[72..74].try_into().unwrap()), 1);
    // offset 74: flags (bit0 = linear frequency table).
    assert_eq!(output[74], 1);
    // offset 76..78: ticks per row.
    assert_eq!(u16::from_le_bytes(output[76..78].try_into().unwrap()), 6);
    // offset 78..80: tempo.
    assert_eq!(u16::from_le_bytes(output[78..80].try_into().unwrap()), 125);
    // offset 80: pattern order table starts.
    assert_eq!(output[80], 0);

    let errors: Vec<_> = log
        .messages
        .iter()
        .filter(|(k, _)| *k == LogLevelKind::Error)
        .collect();
    assert!(errors.is_empty(), "unexpected errors: {errors:?}");
}

#[test]
fn writes_all_patterns_unconditionally() {
    // Unlike MOD, XM writes every pattern object regardless of whether the order table
    // references it -- add a second, unreferenced pattern and confirm it's still emitted.
    let mut song = build_song();
    song.patterns.push(song.patterns[0].clone());
    let settings = default_settings();
    let source = StubSampleSource;
    let mut log = CollectingLog {
        messages: Vec::new(),
    };

    let output =
        xm_format::convert(&song, &settings, &source, &mut log).expect("conversion should succeed");
    // num patterns field (offset 70..72) should reflect both patterns, not just the 1 referenced
    // by the pattern order table.
    assert_eq!(u16::from_le_bytes(output[70..72].try_into().unwrap()), 2);
}
