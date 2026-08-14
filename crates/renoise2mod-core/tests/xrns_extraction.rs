//! Builds a minimal, hand-written `.xrns` in memory (rather than checking in a binary fixture)
//! and verifies `extract_song_data` reads it correctly. Covers song-level fields, channel-width
//! flattening across multiple tracks, pattern order table, note/effect cell extraction, and
//! instrument/sample/keymap extraction.

use zip::write::SimpleFileOptions;

const SONG_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<RenoiseSong doc_version="1">
  <GlobalSongData>
    <SongName>Unit Test Song</SongName>
    <BeatsPerMin>140</BeatsPerMin>
    <LinesPerBeat>4</LinesPerBeat>
    <TicksPerLine>6</TicksPerLine>
    <PlaybackEngineVersion>1</PlaybackEngineVersion>
    <SampleOffsetCompatibilityMode>true</SampleOffsetCompatibilityMode>
    <PitchEffectsCompatibilityMode>false</PitchEffectsCompatibilityMode>
  </GlobalSongData>
  <PatternSequence>
    <LoopSelection>
      <CursorPos>0</CursorPos>
    </LoopSelection>
    <SequenceEntries>
      <SequenceEntry>
        <Pattern>0</Pattern>
      </SequenceEntry>
      <SequenceEntry>
        <Pattern>0</Pattern>
      </SequenceEntry>
    </SequenceEntries>
  </PatternSequence>
  <Tracks>
    <SequencerTrack>
      <NumberOfVisibleNoteColumns>2</NumberOfVisibleNoteColumns>
    </SequencerTrack>
    <SequencerTrack>
      <NumberOfVisibleNoteColumns>1</NumberOfVisibleNoteColumns>
    </SequencerTrack>
    <SequencerMasterTrack>
      <NumberOfVisibleEffectColumns>1</NumberOfVisibleEffectColumns>
    </SequencerMasterTrack>
  </Tracks>
  <Instruments>
    <Instrument>
      <Name>Test Instrument</Name>
      <SampleGenerator>
        <ModulationSets>
          <ModulationSet>
            <Devices>
            </Devices>
          </ModulationSet>
        </ModulationSets>
        <Samples>
          <Sample>
            <Name>Test Sample</Name>
            <Volume>0.8</Volume>
            <Panning>0.5</Panning>
            <FineTune>0</FineTune>
            <Transpose>0</Transpose>
            <LoopStart>0</LoopStart>
            <LoopEnd>0</LoopEnd>
            <LoopMode>Off</LoopMode>
            <Mapping>
              <BaseNote>48</BaseNote>
              <NoteStart>0</NoteStart>
              <NoteEnd>119</NoteEnd>
            </Mapping>
          </Sample>
        </Samples>
      </SampleGenerator>
    </Instrument>
  </Instruments>
  <PatternPool>
    <Patterns>
      <Pattern>
        <NumberOfLines>4</NumberOfLines>
        <Tracks>
          <PatternTrack>
            <Lines>
              <Line index="0">
                <NoteColumns>
                  <NoteColumn>
                    <Note>C-4</Note>
                    <Instrument>00</Instrument>
                  </NoteColumn>
                  <NoteColumn>
                  </NoteColumn>
                </NoteColumns>
                <EffectColumns>
                  <EffectColumn>
                    <Number>0G</Number>
                    <Value>10</Value>
                  </EffectColumn>
                </EffectColumns>
              </Line>
            </Lines>
          </PatternTrack>
          <PatternTrack>
            <Lines>
              <Line index="1">
                <NoteColumns>
                  <NoteColumn>
                    <Note>OFF</Note>
                  </NoteColumn>
                </NoteColumns>
              </Line>
            </Lines>
          </PatternTrack>
          <PatternMasterTrack>
            <Lines>
              <Line index="2">
                <EffectColumns>
                  <EffectColumn>
                    <Number>ZT</Number>
                    <Value>78</Value>
                  </EffectColumn>
                </EffectColumns>
              </Line>
            </Lines>
          </PatternMasterTrack>
        </Tracks>
      </Pattern>
    </Patterns>
  </PatternPool>
</RenoiseSong>
"#;

fn write_fixture_xrns() -> tempfile_xrns::TempXrns {
    tempfile_xrns::TempXrns::new(SONG_XML)
}

/// Tiny helper module so the test doesn't need a `tempfile` dev-dependency: writes the zip next
/// to the test binary and cleans it up on drop.
mod tempfile_xrns {
    use std::path::{Path, PathBuf};

    pub struct TempXrns {
        pub path: PathBuf,
    }

    impl TempXrns {
        pub fn new(song_xml: &str) -> Self {
            let mut path = std::env::temp_dir();
            path.push(format!(
                "renoise2mod-test-{}-{:?}.xrns",
                std::process::id(),
                std::time::Instant::now()
            ));

            let file = std::fs::File::create(&path).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            zip.start_file("Song.xml", super::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(song_xml.as_bytes()).unwrap();
            zip.finish().unwrap();

            Self { path }
        }

        pub fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempXrns {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }

    use std::io::Write;
}

#[test]
fn extracts_song_level_fields() {
    let fixture = write_fixture_xrns();
    let song = renoise2mod_core::extract_song_data(fixture.path()).unwrap();

    assert_eq!(song.name, "Unit Test Song");
    assert_eq!(song.initial_bpm, 140);
    assert_eq!(song.lines_per_beat, 4);
    assert_eq!(song.ticks_per_line, 6);
    assert_eq!(song.playback_engine_version, 1);
    assert!(song.sample_offset_compatibility_mode);
    assert!(!song.pitch_compatibility_mode);
    assert_eq!(song.restart_position, 0);
    assert_eq!(song.pattern_order_table, vec![0, 0]);
}

#[test]
fn flattens_channel_width_across_tracks() {
    let fixture = write_fixture_xrns();
    let song = renoise2mod_core::extract_song_data(fixture.path()).unwrap();

    // track 0 has 2 note columns, track 1 has 1 -> 3 channels total.
    assert_eq!(song.num_channels, 3);
    // master-track columns are clamped to 1 (bugfix: MOD now parses 1 column like XM did).
    assert_eq!(song.num_master_track_columns, 1);
}

#[test]
fn extracts_pattern_cells_at_correct_flattened_index() {
    let fixture = write_fixture_xrns();
    let song = renoise2mod_core::extract_song_data(fixture.path()).unwrap();

    assert_eq!(song.patterns.len(), 1);
    let pattern = &song.patterns[0];
    assert_eq!(pattern.num_rows, 4);

    // channel 0 (track 0, col 0), row 0: note C-4, instrument 00, effect 0G/10
    let cell = pattern.track_line(0, 0, song.num_channels);
    assert!(cell.is_set);
    assert_eq!(cell.note.as_deref(), Some("C-4"));
    assert_eq!(cell.instrument.as_deref(), Some("00"));
    assert_eq!(cell.effect_number, "0G");
    assert_eq!(cell.effect_value, "10");

    // channel 1 (track 0, col 1) inherits track 0's single effect column (spread across cols).
    let cell1 = pattern.track_line(0, 1, song.num_channels);
    assert_eq!(cell1.effect_number, "0G");
    assert_eq!(cell1.effect_value, "10");
    assert!(cell1.note.is_none());

    // channel 2 (track 1, col 0), row 1: note OFF, no effect column data on this track/row.
    let cell2 = pattern.track_line(1, 2, song.num_channels);
    assert_eq!(cell2.note.as_deref(), Some("OFF"));

    // master track, row 2, col 0: ZT/78
    let master_cell = pattern.master_line(2, 0, song.num_master_track_columns);
    assert_eq!(master_cell.effect_number, "ZT");
    assert_eq!(master_cell.effect_value, "78");
}

#[test]
fn extracts_instrument_and_sample_and_keymap() {
    let fixture = write_fixture_xrns();
    let song = renoise2mod_core::extract_song_data(fixture.path()).unwrap();

    assert_eq!(song.instruments.len(), 1);
    let instrument = &song.instruments[0];
    assert_eq!(instrument.name, "Test Instrument");
    assert_eq!(instrument.samples.len(), 1);

    let sample = &instrument.samples[0];
    assert_eq!(sample.name, "Test Sample");
    assert!((sample.volume - 0.8).abs() < f32::EPSILON);
    assert_eq!(sample.rel_note_number, 48);
    assert_eq!(sample.loop_mode, renoise2mod_core::model::LoopMode::Off);

    // full note range 0..=119 mapped to sample slot 0.
    assert_eq!(instrument.key_map[0], Some(0));
    assert_eq!(instrument.key_map[60], Some(0));
    assert_eq!(instrument.key_map[119], Some(0));
}
