//! Command-line parsing (a handful of flags; no parser crate needed).

use anyhow::{Context, bail};

use crate::audio::SourceKind;

pub const USAGE: &str = "\
miz — a calm music visualizer for PipeWire

USAGE:
    miz [OPTIONS]

OPTIONS:
    --source <pipewire|test>   Audio input (default: pipewire = monitor of the default output)
    --debug-audio              Print live analysis in the terminal instead of opening a window
    -h, --help                 Show this help
    -V, --version              Show version
";

#[derive(Debug)]
pub struct Options {
    pub source: SourceKind,
    pub debug_audio: bool,
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
