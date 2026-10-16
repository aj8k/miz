//! Watches the config file and (in dev mode) the shader directory.
//!
//! Editors save in bursts (write temp file, rename, touch...), so events
//! are debounced: a change is reported once nothing has happened to that
//! kind of file for a short while. We watch *directories*, not files,
//! because an atomic save replaces the file and a file watch would be lost.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use notify::{RecursiveMode, Watcher as _};

const DEBOUNCE: Duration = Duration::from_millis(200);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    Config,
    Shaders,
}

pub struct Watcher {
    _inner: notify::RecommendedWatcher,
    events: Receiver<Change>,
    /// Last event time per kind, while waiting for the burst to settle.
    pending: [Option<Instant>; 2],
}

impl Watcher {
    pub fn new(config: Option<&Path>, shader_dir: Option<&Path>) -> anyhow::Result<Self> {
        let (tx, events) = channel();
        let config_file: Option<PathBuf> = config.map(Path::to_path_buf);
        let shader_dir_owned: Option<PathBuf> = shader_dir.map(Path::to_path_buf);
        let mut inner = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            let Ok(event) = res else { return };
            if !(event.kind.is_modify() || event.kind.is_create()) {
                return;
            }
            for path in &event.paths {
                let kind = if config_file.as_ref().is_some_and(|c| {
                    c.file_name() == path.file_name() && c.parent() == path.parent()
                }) {
                    Change::Config
                } else if shader_dir_owned
                    .as_ref()
                    .is_some_and(|d| path.starts_with(d))
                    && path.extension().is_some_and(|e| e == "wgsl")
                {
                    Change::Shaders
                } else {
                    continue;
                };
                let _ = tx.send(kind);
            }
        })?;
        if let Some(dir) = config.and_then(Path::parent) {
            inner.watch(dir, RecursiveMode::NonRecursive)?;
        }
        if let Some(dir) = shader_dir {
            inner.watch(dir, RecursiveMode::NonRecursive)?;
        }
        Ok(Self {
            _inner: inner,
            events,
            pending: [None, None],
        })
    }

    /// Non-blocking: returns the changes whose burst of events has settled.
    pub fn poll(&mut self) -> Vec<Change> {
        let now = Instant::now();
        while let Ok(kind) = self.events.try_recv() {
            self.pending[kind as usize] = Some(now);
        }
        let mut ready = Vec::new();
        for (i, kind) in [Change::Config, Change::Shaders].into_iter().enumerate() {
            if self.pending[i].is_some_and(|t| now - t >= DEBOUNCE) {
                self.pending[i] = None;
                ready.push(kind);
            }
        }
        ready
    }
}
