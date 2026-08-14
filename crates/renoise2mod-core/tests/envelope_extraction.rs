//! Regression test for a real bug found while comparing output against a real song: envelope
//! extraction was silently returning nothing for every instrument. Two things were wrong at
//! once -- the field names used (`IsActive`/`SustainEnabled`/etc. instead of the
//! `Envelope`-prefixed names Renoise actually uses for `SampleCompatibilityModulationDevice`,
//! which is the device shape modern Renoise actually writes) and the envelope point format
//! (`"x,y,curve"` triples were treated as `"x,y"` pairs, so the two-way split silently failed to
//! parse the y value and every point got dropped).
//!
//! This fixture's structure and values are taken directly from a real `.xrns` file's Song.xml.

use std::io::Write;

use zip::write::SimpleFileOptions;

const SONG_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<RenoiseSong doc_version="1">
  <GlobalSongData>
    <SongName>Envelope Test</SongName>
  </GlobalSongData>
  <Instruments>
    <Instrument>
      <Name>Test Instrument</Name>
      <SampleGenerator>
        <ModulationSets>
          <ModulationSet>
            <Devices>
              <SampleCompatibilityModulationDevice type="SampleCompatibilityModulationDevice">
                <IsActive>
                  <Value>1.0</Value>
                </IsActive>
                <Target>Volume</Target>
                <EnvelopeIsActive>true</EnvelopeIsActive>
                <EnvelopeSustainIsActive>true</EnvelopeSustainIsActive>
                <EnvelopeSustainPos>2</EnvelopeSustainPos>
                <EnvelopeLoopStart>0</EnvelopeLoopStart>
                <EnvelopeLoopEnd>0</EnvelopeLoopEnd>
                <EnvelopeLoopMode>Off</EnvelopeLoopMode>
                <EnvelopeDecay>
                  <Value>1024</Value>
                </EnvelopeDecay>
                <EnvelopeNodes>
                  <PlayMode>Lines</PlayMode>
                  <Points>
                    <Point>0,1.0,0.0</Point>
                    <Point>1,1.0,0.0</Point>
                    <Point>2,1.0,0.0</Point>
                    <Point>6,0.765625,0.0</Point>
                    <Point>182,0.0,0.0</Point>
                  </Points>
                </EnvelopeNodes>
              </SampleCompatibilityModulationDevice>
              <SampleCompatibilityModulationDevice type="SampleCompatibilityModulationDevice">
                <IsActive>
                  <Value>1.0</Value>
                </IsActive>
                <Target>Panning</Target>
                <EnvelopeIsActive>false</EnvelopeIsActive>
                <EnvelopeSustainIsActive>false</EnvelopeSustainIsActive>
                <EnvelopeSustainPos>0</EnvelopeSustainPos>
                <EnvelopeLoopStart>0</EnvelopeLoopStart>
                <EnvelopeLoopEnd>0</EnvelopeLoopEnd>
                <EnvelopeLoopMode>Off</EnvelopeLoopMode>
                <EnvelopeDecay>
                  <Value>128</Value>
                </EnvelopeDecay>
                <EnvelopeNodes>
                  <PlayMode>Lines</PlayMode>
                </EnvelopeNodes>
              </SampleCompatibilityModulationDevice>
            </Devices>
          </ModulationSet>
        </ModulationSets>
        <Samples>
          <Sample>
            <Name>Test Sample</Name>
            <Volume>1.0</Volume>
          </Sample>
        </Samples>
      </SampleGenerator>
    </Instrument>
  </Instruments>
</RenoiseSong>
"#;

fn write_fixture_xrns() -> std::path::PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "renoise2mod-envelope-test-{}-{:?}.xrns",
        std::process::id(),
        std::time::Instant::now()
    ));

    let file = std::fs::File::create(&path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    zip.start_file("Song.xml", SimpleFileOptions::default())
        .unwrap();
    zip.write_all(SONG_XML.as_bytes()).unwrap();
    zip.finish().unwrap();

    path
}

#[test]
fn extracts_volume_envelope_with_real_schema_field_names_and_point_format() {
    let path = write_fixture_xrns();
    let song = renoise2mod_core::extract_song_data(&path).unwrap();
    std::fs::remove_file(&path).ok();

    let instrument = &song.instruments[0];
    let vol_env = &instrument.volume_envelope;

    assert!(
        vol_env.enabled,
        "EnvelopeIsActive should be read, not the device's own IsActive"
    );
    assert!(vol_env.sustain_enabled);
    assert_eq!(vol_env.sustain_point_x, 2.0);
    assert!(!vol_env.loop_enabled);
    assert_eq!(
        vol_env.fade_out, 1024,
        "fade_out should come from EnvelopeDecay/Value"
    );

    // The real regression: point count must be nonzero and x/y values must be parsed correctly
    // out of "x,y,curve" triples (previously every point silently failed to parse).
    assert_eq!(
        vol_env.points.len(),
        5,
        "all 5 points must parse despite the 3-value format"
    );
    assert_eq!(vol_env.points[0], (0.0, 1.0));
    assert_eq!(vol_env.points[3], (6.0, 0.765625));
    assert_eq!(vol_env.points[4], (182.0, 0.0));

    let pan_env = &instrument.panning_envelope;
    assert!(!pan_env.enabled);
    assert_eq!(pan_env.points.len(), 0);
}
