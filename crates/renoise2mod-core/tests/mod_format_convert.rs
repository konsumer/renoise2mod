//! End-to-end structural test for the MOD writer: builds a small `SongData` directly (no XML
//! parsing involved) and a stub sample source, then checks the produced file's binary layout
//! against the documented MOD format offsets.

use renoise2mod_core::common::VolumeScalingMode;
use renoise2mod_core::mod_format::{
    self, ConversionLog, EncodedSample, LogLevel, ModSampleSource, ModSettings,
    ProTrackerCompatibility,
};
use renoise2mod_core::model::{
    InstrumentData, LoopMode, MasterTrackLineData, PatternData, SampleData, SongData, TrackLineData,
};

struct StubSampleSource;

impl ModSampleSource for StubSampleSource {
    fn encode_sample(
        &self,
        _instrument_index: usize,
        _sample: &SampleData,
        _settings: &ModSettings,
    ) -> renoise2mod_core::Result<Option<EncodedSample>> {
        // 100 bytes of unsigned-8-bit silence (0x80), pre sign-flip -- the MOD writer itself
        // doesn't sign-flip (that's audio.rs's job when it produces `encoded_pcm`), so hand back
        // already-"encoded" (sign-flipped) silence: 0x00.
        Ok(Some(EncodedSample {
            encoded_pcm: vec![0u8; 100],
            sample_rate: 8287,
            original_length_bytes: 100,
            original_channels: 1,
            original_bits_per_sample: 8,
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
    let num_channels = 4;
    let num_rows = 4;

    let mut tracks_line_data = vec![TrackLineData::default(); num_rows * num_channels];
    // Row 0, channel 0: trigger a C-4 note on instrument 0.
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
        name: "Structural Test Song".to_string(),
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

fn default_settings() -> ModSettings {
    ModSettings {
        pro_tracker_compatibility: ProTrackerCompatibility::None,
        ntsc_mode: false,
        portamento_loss_threshold: 2,
        volume_scaling_mode: VolumeScalingMode::None,
    }
}

#[test]
fn produces_correctly_laid_out_mod_file() {
    let song = build_song();
    let settings = default_settings();
    let source = StubSampleSource;
    let mut log = CollectingLog {
        messages: Vec::new(),
    };

    let output = mod_format::convert(&song, &settings, &source, &mut log)
        .expect("conversion should succeed");

    // offset 0..20: song name (exactly 20 bytes, "Structural Test Song" fills it exactly).
    assert_eq!(&output[0..20], b"Structural Test Song");

    // offset 20..950: sample info block (930 bytes) -- instrument name at offset 20.
    assert_eq!(&output[20..36], b"Test Instrument\0");

    // offset 950..952: song length byte + fixed 0x7F restart byte.
    assert_eq!(output[950], 1); // one entry in the pattern order table
    assert_eq!(output[951], 0x7F);

    // offset 952..1080: 128-byte pattern order table.
    assert_eq!(output[952], 0);
    assert_eq!(&output[953..1080], vec![0u8; 127].as_slice());

    // offset 1080..1084: "M.K." for exactly 4 channels.
    assert_eq!(&output[1080..1084], b"M.K.");

    // offset 1084: start of pattern data (64 rows * 4 bytes * 4 channels = 1024 bytes for 1
    // pattern).
    let pattern_data_len = 64 * 4 * 4;
    assert_eq!(output.len(), 1084 + pattern_data_len + 100 /* PCM */);

    // The triggered note (C-4, instrument 0 -> sampleNumber 1) should show up as the exact
    // expected 4-byte cell at the very start of the pattern data. Renoise note "C-4" (letter
    // index 0, octave 4) with no base-note offset -> PeriodsRange index (4-2)*12+0 = 24 -> period
    // 428 (0x1AC). Packed: byte0 = (sampleNumber&0xF0)|((period&0xF00)>>8) = 0x01,
    // byte1 = period&0xFF = 0xAC, byte2 = ((sampleNumber&0xF)<<4)|effectNum = 0x10, byte3 = 0.
    let cell = &output[1084..1088];
    assert_eq!(cell, &[0x01, 0xAC, 0x10, 0x00]);

    // no errors should have been logged for this well-formed song.
    let errors: Vec<_> = log
        .messages
        .iter()
        .filter(|(k, _)| *k == LogLevelKind::Error)
        .collect();
    assert!(errors.is_empty(), "unexpected errors: {errors:?}");
}

#[test]
fn rejects_songs_with_fewer_than_four_channels() {
    let mut song = build_song();
    song.num_channels = 2;
    let settings = default_settings();
    let source = StubSampleSource;
    let mut log = CollectingLog {
        messages: Vec::new(),
    };

    let result = mod_format::convert(&song, &settings, &source, &mut log);
    assert!(result.is_err());
}

#[test]
fn rejects_column_volume_scaling_mode() {
    let song = build_song();
    let mut settings = default_settings();
    settings.volume_scaling_mode = VolumeScalingMode::Column;
    let source = StubSampleSource;
    let mut log = CollectingLog {
        messages: Vec::new(),
    };

    let result = mod_format::convert(&song, &settings, &source, &mut log);
    assert!(result.is_err());
}
