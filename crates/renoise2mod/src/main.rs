use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, ValueEnum};

use renoise2mod_core::audio::{XrnsModSampleSource, XrnsXmSampleSource};
use renoise2mod_core::common::VolumeScalingMode;
use renoise2mod_core::mod_format::{
    self, ConversionLog, LogLevel, ModSettings, ProTrackerCompatibility,
};
use renoise2mod_core::model::SampleFreqMode;
use renoise2mod_core::xm_format::{self, XmSettings};

/// Convert a Renoise .xrns song to a classic .mod (ProTracker) or .xm (FastTracker II) tracker
/// file.
#[derive(Parser)]
#[command(version, about)]
struct Args {
    /// Input .xrns file
    input: PathBuf,

    /// Output format
    #[arg(short = 't', long = "type", value_enum, default_value_t = OutputType::Xm)]
    output_type: OutputType,

    /// Output file (defaults to the input file's name with the format's extension)
    #[arg(long)]
    out: Option<PathBuf>,

    /// ProTracker compatibility (affects only mod)
    #[arg(long, value_enum, default_value_t = PtMode::Hardware)]
    ptmode: PtMode,

    /// NTSC timing (affects only mod; default is PAL)
    #[arg(long)]
    ntsc: bool,

    /// Volume scaling mode
    #[arg(long = "volscal", value_enum, default_value_t = VolScalMode::Sample)]
    volscal: VolScalMode,

    /// Initial tempo/BPM (affects only xm; defaults to the song's own tempo)
    #[arg(long, default_value_t = 0)]
    tempo: i32,

    /// Initial ticks per row (affects only xm; defaults to the song's own value)
    #[arg(long, default_value_t = 0)]
    ticks: i32,

    /// Portamento loss threshold, 0-4 (affects only mod)
    #[arg(long, default_value_t = 2)]
    portresh: i32,

    /// Sample rate selection applied to every instrument: low, high, maximum, original, a MOD
    /// note name like "c-2", or an explicit Hz value (affects only mod's resampling target)
    #[arg(long, default_value = "low", value_parser = parse_sample_freq_mode)]
    sample_rate: SampleFreqMode,

    /// Write progress/error messages to this file instead of stdout/stderr
    #[arg(long)]
    log: Option<PathBuf>,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum OutputType {
    Mod,
    Xm,
}

#[derive(Clone, Copy, ValueEnum)]
enum PtMode {
    #[value(name = "none", alias = "n")]
    None,
    #[value(name = "software", alias = "s")]
    Software,
    #[value(name = "hardware", alias = "h")]
    Hardware,
}

#[derive(Clone, Copy, ValueEnum)]
enum VolScalMode {
    #[value(name = "none", alias = "n")]
    None,
    #[value(name = "sample", alias = "s")]
    Sample,
    #[value(name = "column", alias = "c")]
    Column,
}

fn parse_sample_freq_mode(s: &str) -> Result<SampleFreqMode, String> {
    match s.to_ascii_lowercase().as_str() {
        "low" => Ok(SampleFreqMode::Low),
        "high" => Ok(SampleFreqMode::High),
        "maximum" | "max" => Ok(SampleFreqMode::Maximum),
        "original" => Ok(SampleFreqMode::Original),
        _ => {
            if let Ok(hz) = s.parse::<u32>() {
                Ok(SampleFreqMode::Hz(hz))
            } else if s.len() == 3 {
                Ok(SampleFreqMode::NoteName(s.to_ascii_uppercase()))
            } else {
                Err(format!("invalid --sample-rate value: {s} (expected low|high|maximum|original|<note e.g. C-2>|<Hz>)"))
            }
        }
    }
}

struct ConsoleLog {
    file: Option<std::fs::File>,
    had_errors: bool,
}

impl ConsoleLog {
    fn new(path: Option<&PathBuf>) -> std::io::Result<Self> {
        let file = path.map(std::fs::File::create).transpose()?;
        Ok(Self {
            file,
            had_errors: false,
        })
    }
}

impl ConversionLog for ConsoleLog {
    fn log(&mut self, level: LogLevel, message: String) {
        use std::io::Write;

        let prefix = match level {
            LogLevel::Info => "INFO",
            LogLevel::Warning => "WARN",
            LogLevel::Error => {
                self.had_errors = true;
                "ERROR"
            }
        };
        let line = format!("[{prefix}] {message}");

        if let Some(f) = &mut self.file {
            let _ = writeln!(f, "{line}");
        } else if matches!(level, LogLevel::Error) {
            eprintln!("{line}");
        } else {
            println!("{line}");
        }
    }
}

fn main() -> ExitCode {
    let args = Args::parse();

    if !(0..=4).contains(&args.portresh) {
        eprintln!("error: --portresh must be between 0 and 4");
        return ExitCode::FAILURE;
    }

    let mut log = match ConsoleLog::new(args.log.as_ref()) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("error: could not open log file: {e}");
            return ExitCode::FAILURE;
        }
    };

    let song = match renoise2mod_core::extract_song_data(&args.input) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };

    let volume_scaling_mode = match args.volscal {
        VolScalMode::None => VolumeScalingMode::None,
        VolScalMode::Sample => VolumeScalingMode::Sample,
        VolScalMode::Column => VolumeScalingMode::Column,
    };

    let output = match args.output_type {
        OutputType::Mod => {
            if matches!(args.volscal, VolScalMode::Column) {
                eprintln!("error: -type must be xm when --volscal is column");
                return ExitCode::FAILURE;
            }

            let pro_tracker_compatibility = match args.ptmode {
                PtMode::None => ProTrackerCompatibility::None,
                PtMode::Software => ProTrackerCompatibility::B3Max,
                PtMode::Hardware => ProTrackerCompatibility::A3Max,
            };

            let mut song = song;
            for instrument in &mut song.instruments {
                for sample in &mut instrument.samples {
                    sample.sample_freq = args.sample_rate.clone();
                }
            }

            let settings = ModSettings {
                pro_tracker_compatibility,
                ntsc_mode: args.ntsc,
                portamento_loss_threshold: args.portresh,
                volume_scaling_mode,
            };
            let source = XrnsModSampleSource {
                xrns_path: &args.input,
            };
            mod_format::convert(&song, &settings, &source, &mut log)
        }
        OutputType::Xm => {
            let tempo = if args.tempo == 0 {
                song.initial_bpm as i32
            } else {
                args.tempo
            };
            let ticks_row = if args.ticks == 0 {
                if song.ticks_per_line > 0 {
                    song.ticks_per_line as i32
                } else {
                    6
                }
            } else {
                args.ticks
            };

            let settings = XmSettings {
                ticks_row,
                tempo,
                volume_scaling_mode,
            };
            let source = XrnsXmSampleSource {
                xrns_path: &args.input,
            };
            xm_format::convert(&song, &settings, &source, &mut log)
        }
    };

    let bytes = match output {
        Ok(b) => b,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };

    let extension = match args.output_type {
        OutputType::Mod => "mod",
        OutputType::Xm => "xm",
    };
    let out_path = args
        .out
        .unwrap_or_else(|| args.input.with_extension(extension));

    if let Err(e) = std::fs::write(&out_path, &bytes) {
        eprintln!("error: failed to write {}: {e}", out_path.display());
        return ExitCode::FAILURE;
    }

    println!("wrote {} bytes to {}", bytes.len(), out_path.display());

    if log.had_errors {
        eprintln!("completed with warnings/errors, see log output above");
    }

    ExitCode::SUCCESS
}
