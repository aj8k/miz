//! Command-line parsing (a handful of flags; no parser crate needed).

use std::path::PathBuf;

use anyhow::{Context, bail};

use crate::audio::SourceKind;

pub const USAGE: &str = "\
miz — a calm music visualizer for PipeWire

USAGE:
    miz [OPTIONS]

OPTIONS:
    --source <pipewire|test>   Audio input (default from config: pipewire = monitor of the default output)
    --scene <NAME>             Start scene: feedback, lissajous, radial, nebula, bass-pulse
    --config <FILE>            Config file (default: ~/.config/miz/config.toml)
    --dev                      Load shaders from the source tree and reload them on save
    --debug-audio              Print live analysis in the terminal instead of opening a window
    --hud                      Start with the debug HUD visible
    --capture <FILE.ppm>       Save the frame shown after --capture-after seconds, then quit (dev tool)
    --capture-after <SECS>     Delay before --capture (default 4)
    --capture-frames <N>       Save N consecutive frames as FILE_000.ppm, FILE_001.ppm, ...
    -h, --help                 Show this help
    -V, --version              Show version

KEYS:
    F          fullscreen          N / P      next / previous scene
    H          debug HUD           1 - 4      jump to scene
    C          calm mode on/off    L          lock the current scene
    Q / Esc    quit
";

#[derive(Debug)]
pub struct Options {
    /// `None` = use the config file's value.
    pub source: Option<SourceKind>,
    pub scene: Option<String>,
    pub config: Option<PathBuf>,
    pub dev: bool,
    pub debug_audio: bool,
    pub hud: bool,
    pub capture: Option<PathBuf>,
    pub capture_after: f32,
    pub capture_frames: u32,
}

pub enum Command {
    Run(Options),
    Help,
    Version,
}

pub fn parse(args: impl IntoIterator<Item = String>) -> anyhow::Result<Command> {
    let mut opts = Options {
        source: None,
        scene: None,
        config: None,
        dev: false,
        debug_audio: false,
        hud: false,
        capture: None,
        capture_after: 4.0,
        capture_frames: 1,
    };
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        // Accept both `--flag value` and `--flag=value`.
        let (flag, inline) = match arg.split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f.to_string(), Some(v.to_string())),
            _ => (arg, None),
        };
        let mut value = |name: &str| {
            inline
                .clone()
                .or_else(|| args.next())
                .with_context(|| format!("{name} needs a value"))
        };
        match flag.as_str() {
            "-h" | "--help" => return Ok(Command::Help),
            "-V" | "--version" => return Ok(Command::Version),
            "--debug-audio" => opts.debug_audio = true,
            "--scene" => opts.scene = Some(value("--scene")?),
            "--config" => opts.config = Some(PathBuf::from(value("--config")?)),
            "--dev" => opts.dev = true,
            "--hud" => opts.hud = true,
            "--capture" => opts.capture = Some(PathBuf::from(value("--capture")?)),
            "--capture-frames" => {
                opts.capture_frames = value("--capture-frames")?
                    .parse()
                    .context("--capture-frames expects a count")?
            }
            "--capture-after" => {
                opts.capture_after = value("--capture-after")?
                    .parse()
                    .context("--capture-after expects seconds")?
            }
            "--source" => {
                opts.source = match value("--source")?.as_str() {
                    "pipewire" | "pw" => Some(SourceKind::PipeWire),
                    "test" => Some(SourceKind::Test),
                    other => bail!("unknown source '{other}' (expected 'pipewire' or 'test')"),
                }
            }
            other => bail!("unknown argument '{other}'\n\n{USAGE}"),
        }
    }
    Ok(Command::Run(opts))
}
