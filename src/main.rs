// Runtime paths must handle errors; tests may unwrap freely.
#![cfg_attr(not(test), warn(clippy::unwrap_used, clippy::expect_used))]

mod analysis;
mod app;
mod audio;
mod capture;
mod cli;
mod debug_term;
mod gpu;
mod render;
mod scenes;
mod snapshot;

use analysis::AnalyzerConfig;
use audio::AudioEngine;
use cli::Command;

fn main() -> anyhow::Result<()> {
    // sctk_adwaita (winit's Wayland window decorations) warns about GNOME
    // titlebar button types it doesn't know; that's harmless noise.
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("warn,sctk_adwaita=error"),
    )
    .init();
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
    app::run(engine, opts)
}
