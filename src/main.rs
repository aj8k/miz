// Runtime paths must handle errors; tests may unwrap freely.
#![cfg_attr(not(test), warn(clippy::unwrap_used, clippy::expect_used))]

mod analysis;
mod audio;
mod cli;
mod debug_term;
mod snapshot;

use analysis::AnalyzerConfig;
use audio::AudioEngine;
use cli::Command;

fn main() -> anyhow::Result<()> {
    let opts = match cli::parse(std::env::args().skip(1))? {
        Command::Help => {
            print!("{}", cli::USAGE);
            return Ok(());
        }
        Command::Version => {
            println!("miz {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        Command::Run(opts) => opts,
    };

    let engine = AudioEngine::start(opts.source, AnalyzerConfig::default())?;
    if opts.debug_audio {
        return debug_term::run(&engine);
    }
    anyhow::bail!("the window comes in phase 2 — for now run with --debug-audio")
}
