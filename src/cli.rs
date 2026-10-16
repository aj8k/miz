//! Command-line parsing (a handful of flags; no parser crate needed).

use std::path::PathBuf;

use anyhow::{Context, bail};

use crate::audio::SourceKind;

pub const USAGE: &str = "\
miz — a calm music visualizer for PipeWire

USAGE:
    miz [OPTIONS]

OPTIONS:
    --source <pipewire|test>   Audio input (default: pipewire = monitor of the default output)
    --debug-audio              Print live analysis in the terminal instead of opening a window
    --scene <NAME>             Start scene: feedback (default), lissajous, radial, nebula, bass-pulse
    --hud                      Start with the debug HUD visible
    --capture <FILE.ppm>       Save one frame after --capture-after seconds, then quit (dev tool)
    --capture-after <SECS>     Delay before --capture (default 4)
    -h, --help                 Show this help
    -V, --version              Show version
";

#[derive(Debug)]
pub struct Options {
    pub source: SourceKind,
    pub debug_audio: bool,
    pub scene: String,
    pub hud: bool,
    pub capture: Option<PathBuf>,
    pub capture_after: f32,
}

pub enum Command {
    Run(Options),
    Help,
    Version,
}

pub fn parse(args: impl IntoIterator<Item = String>) -> anyhow::Result<Command> {
    let mut opts = Options {
        source: SourceKind::PipeWire,
        debug_audio: false,
        scene: "feedback".into(),
        hud: false,
        capture: None,
        capture_after: 4.0,
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
            "--scene" => opts.scene = value("--scene")?,
            "--hud" => opts.hud = true,
            "--capture" => opts.capture = Some(PathBuf::from(value("--capture")?)),
            "--capture-after" => {
                opts.capture_after = value("--capture-after")?
                    .parse()
                    .context("--capture-after expects seconds")?
            }
            "--source" => {
                opts.source = match value("--source")?.as_str() {
                    "pipewire" | "pw" => SourceKind::PipeWire,
                    "test" => SourceKind::Test,
                    other => bail!("unknown source '{other}' (expected 'pipewire' or 'test')"),
                }
            }
            other => bail!("unknown argument '{other}'\n\n{USAGE}"),
        }
    }
    Ok(Command::Run(opts))
}
