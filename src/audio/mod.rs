//! Audio input: sources, plus the engine thread that runs the analysis.

pub mod pw_record;
pub mod ring;
pub mod test_signal;

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::analysis::{Analyzer, AnalyzerConfig};
use crate::snapshot::SnapshotCell;

/// Every source delivers interleaved stereo f32 at this rate.
pub const SAMPLE_RATE: u32 = 48_000;

/// A stream of stereo audio. Implemented for `pw-record` (Linux) and the
/// synthetic test signal; a macOS source would be another implementation.
pub trait AudioSource: Send {
    fn describe(&self) -> String;

    /// Waits up to `timeout` for the next block of interleaved `L R L R ...`
    /// samples at `SAMPLE_RATE`. `Ok(None)` means nothing arrived in time
    /// (e.g. the output device is suspended) — not an error.
    fn next_block(&mut self, timeout: Duration) -> Result<Option<&[f32]>, AudioError>;
}

#[derive(Debug, Clone)]
pub enum AudioError {
    /// The capture tool isn't installed; retrying is unlikely to help soon.
    Missing(String),
    Failed(String),
}

impl fmt::Display for AudioError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AudioError::Missing(m) | AudioError::Failed(m) => f.write_str(m),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    PipeWire,
    Test,
}

fn open_source(kind: SourceKind) -> Result<Box<dyn AudioSource>, AudioError> {
    Ok(match kind {
        SourceKind::PipeWire => Box::new(pw_record::PwRecordSource::spawn()?),
        SourceKind::Test => Box::new(test_signal::TestSignal::new()),
    })
}

#[derive(Debug, Clone)]
pub enum AudioStatus {
    Starting,
    Running(String),
    Retrying { reason: String, retry_in: Duration },
}

impl fmt::Display for AudioStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AudioStatus::Starting => f.write_str("starting"),
            AudioStatus::Running(d) => write!(f, "running: {d}"),
            AudioStatus::Retrying { reason, retry_in } => {
                write!(
                    f,
                    "ERROR: {reason} — retrying in {:.1}s",
                    retry_in.as_secs_f32()
                )
            }
        }
    }
}

/// Owns the audio thread. Dropping it stops the thread (and pw-record).
pub struct AudioEngine {
    snapshot: Arc<SnapshotCell>,
    status: Arc<Mutex<AudioStatus>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    settings: Sender<(SourceKind, AnalyzerConfig)>,
    current: (SourceKind, AnalyzerConfig),
}

impl AudioEngine {
    pub fn start(kind: SourceKind, cfg: AnalyzerConfig) -> anyhow::Result<Self> {
        let snapshot = Arc::new(SnapshotCell::new());
        let status = Arc::new(Mutex::new(AudioStatus::Starting));
        let stop = Arc::new(AtomicBool::new(false));
        let (settings, settings_rx) = channel();
        let current = (kind, cfg.clone());
        let worker = Worker {
            kind,
            analyzer: Analyzer::new(cfg),
            settings: settings_rx,
            snapshot: Arc::clone(&snapshot),
            status: Arc::clone(&status),
            stop: Arc::clone(&stop),
        };
        let thread = thread::Builder::new()
            .name("audio".into())
            .spawn(move || worker.run())?;
        Ok(Self {
            snapshot,
            status,
            stop,
            thread: Some(thread),
            settings,
            current,
        })
    }

    /// Applies new settings (config reload, menu). Only actual changes are
    /// sent; a new source restarts capture.
    pub fn reconfigure(&mut self, kind: SourceKind, cfg: AnalyzerConfig) {
        if self.current == (kind, cfg.clone()) {
            return;
        }
        self.current = (kind, cfg.clone());
        let _ = self.settings.send((kind, cfg));
    }

    pub fn snapshot(&self) -> &SnapshotCell {
        &self.snapshot
    }

    pub fn status(&self) -> AudioStatus {
        self.status
            .lock()
            .map(|s| s.clone())
            .unwrap_or(AudioStatus::Starting)
    }
}

impl Drop for AudioEngine {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

struct Worker {
    kind: SourceKind,
    analyzer: Analyzer,
    settings: Receiver<(SourceKind, AnalyzerConfig)>,
    snapshot: Arc<SnapshotCell>,
    status: Arc<Mutex<AudioStatus>>,
    stop: Arc<AtomicBool>,
}

enum PumpEnd {
    Stopped,
    Reopen,
}

/// How long to wait for data before treating the gap as silence. Data
/// normally arrives every ~21 ms; this also bounds shutdown latency.
const POLL: Duration = Duration::from_millis(100);

impl Worker {
    fn run(mut self) {
        let mut backoff = Backoff::new(Duration::from_millis(500), Duration::from_secs(10));
        while !self.stopped() {
            let reason = match open_source(self.kind) {
                Ok(mut source) => {
                    self.set_status(AudioStatus::Running(source.describe()));
                    let started = Instant::now();
                    let result = self.pump(source.as_mut());
                    // A source that ran for a while earns a fresh, short backoff.
                    if started.elapsed() > Duration::from_secs(10) {
                        backoff.reset();
                    }
                    match result {
                        Ok(PumpEnd::Stopped) => return,
                        Ok(PumpEnd::Reopen) => continue, // source changed: no backoff
                        Err(e) => e,
                    }
                }
                Err(e) => e,
            };
            let retry_in = backoff.next_delay();
            self.set_status(AudioStatus::Retrying {
                reason: reason.to_string(),
                retry_in,
            });
            self.idle_for(retry_in);
        }
    }

    /// Reads from `source` until it fails, the source kind changes, or we're
    /// asked to stop.
    fn pump(&mut self, source: &mut dyn AudioSource) -> Result<PumpEnd, AudioError> {
        let silence_frames = (POLL.as_secs_f64() * SAMPLE_RATE as f64) as usize;
        while !self.stopped() {
            if self.apply_settings() {
                return Ok(PumpEnd::Reopen);
            }
            let changed = match source.next_block(POLL)? {
                Some(block) => self.analyzer.push(block),
                // A stall reads as silence, so visuals fade to idle instead
                // of freezing on the last frame.
                None => self.analyzer.push_silence(silence_frames),
            };
            if changed {
                self.snapshot.publish(self.analyzer.snapshot());
            }
        }
        Ok(PumpEnd::Stopped)
    }

    /// Takes pending settings; returns true if the source kind changed.
    fn apply_settings(&mut self) -> bool {
        let mut source_changed = false;
        while let Ok((kind, cfg)) = self.settings.try_recv() {
            self.analyzer.reconfigure(cfg);
            source_changed |= kind != self.kind;
            self.kind = kind;
        }
        source_changed
    }

    /// Waits before a retry while feeding silence, so the idle animation runs.
    fn idle_for(&mut self, total: Duration) {
        let deadline = Instant::now() + total;
        let frames = (POLL.as_secs_f64() * SAMPLE_RATE as f64) as usize;
        while !self.stopped() && Instant::now() < deadline {
            if self.apply_settings() {
                return; // a different source might work: try it right away
            }
            thread::sleep(POLL);
            if self.analyzer.push_silence(frames) {
                self.snapshot.publish(self.analyzer.snapshot());
            }
        }
    }

    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    fn set_status(&self, s: AudioStatus) {
        if let Ok(mut g) = self.status.lock() {
            *g = s;
        }
    }
}

/// Exponential backoff: min, 2*min, 4*min, ... capped at max.
struct Backoff {
    min: Duration,
    max: Duration,
    next: Duration,
}

impl Backoff {
    fn new(min: Duration, max: Duration) -> Self {
        Self {
            min,
            max,
            next: min,
        }
    }

    fn next_delay(&mut self) -> Duration {
        let d = self.next;
        self.next = (self.next * 2).min(self.max);
        d
    }

    fn reset(&mut self) {
        self.next = self.min;
    }
}
