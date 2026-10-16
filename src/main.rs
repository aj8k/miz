// Runtime paths must handle errors; tests may unwrap freely.
#![cfg_attr(not(test), warn(clippy::unwrap_used, clippy::expect_used))]

mod analysis;
mod app;
mod audio;
mod capture;
mod cli;
mod config;
mod debug_term;
mod gpu;
mod render;
mod scenes;
mod snapshot;
mod watch;

use audio::{AudioEngine, SourceKind};
use cli::Command;
use config::Config;

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

    // Config: --config, else ~/.config/miz/config.toml (created on first run).
    let config_path = opts.config.clone().or_else(config::default_path);
    let cfg = match &config_path {
        Some(path) => config::load_or_create(path)?,
        None => Config::default(),
    };
    // Command-line flags win over the config file.
    let source = opts
        .source
        .or_else(|| cfg.source_kind())
        .unwrap_or(SourceKind::PipeWire);

    let engine = AudioEngine::start(source, cfg.analyzer())?;
    if opts.debug_audio {
        return debug_term::run(&engine);
    }
    app::run(engine, opts, cfg, config_path)
}
