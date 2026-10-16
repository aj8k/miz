//! Audio capture through a `pw-record` child process.
//!
//! `pw-record` ships with PipeWire, so we get capture without linking
//! libpipewire (no dev headers, no bindgen). It writes raw little-endian f32
//! stereo frames to stdout; a reader thread turns them into sample blocks.
//!
//! Safety-critical detail: `stream.capture.sink=true` makes WirePlumber link
//! us to the *monitor of the default output*. Without it, pw-record would
//! record the default *source* — which, with Bluetooth earbuds, is their
//! microphone, and opening it switches the earbuds to the low-quality
//! headset profile. This property is therefore hard-coded, not configurable.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, ErrorKind, Read};
use std::process::{Child, ChildStderr, ChildStdout, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use super::{AudioError, AudioSource, SAMPLE_RATE};

/// Node / application name shown in `wpctl status`, pavucontrol, etc.
pub const NODE_NAME: &str = "miz";

/// Ask for buffers of one default graph quantum (1024 frames ≈ 21 ms).
/// Requesting less would make PipeWire shrink the quantum for the *whole*
/// graph, costing every other client extra wakeups.
const LATENCY_FRAMES: u32 = 1024;

pub struct PwRecordSource {
    child: Child,
    blocks: Receiver<Vec<f32>>,
    current: Vec<f32>,
    stderr_tail: Arc<Mutex<VecDeque<String>>>,
}

impl PwRecordSource {
    pub fn spawn() -> Result<Self, AudioError> {
        let props = format!(
            "{{ stream.capture.sink=true \
               node.name={NODE_NAME} \
               node.description=\"miz visualizer\" \
               application.name={NODE_NAME} \
               node.passive=true }}"
        );
        // node.passive: our link doesn't keep the output device awake on its
        // own — when nothing plays, the sink may suspend as usual (we then
        // simply receive no data, which the engine treats as silence).
        let mut child = Command::new("pw-record")
            .args(["-P", &props])
            .args(["--raw", "--format", "f32", "--channels", "2"])
            .args(["--rate", &SAMPLE_RATE.to_string()])
            .args(["--latency", &LATENCY_FRAMES.to_string()])
            .arg("-")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| {
                match e.kind() {
                ErrorKind::NotFound => AudioError::Missing(
                    "pw-record not found in PATH (it is part of PipeWire: `pipewire-bin` on Ubuntu)"
                        .into(),
                ),
                _ => AudioError::Failed(format!("could not start pw-record: {e}")),
            }
            })?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| AudioError::Failed("pw-record stdout missing".into()))?;
        let stderr = child.stderr.take();

        // Bounded queue: ~64 blocks ≈ 1.3 s. If the analysis ever falls that
        // far behind we drop blocks instead of blocking the reader, so
        // pw-record's pipe always drains and PipeWire never waits on us.
        let (tx, rx) = sync_channel(64);
        spawn_named("pw-record-reader", move || read_pcm(stdout, tx))?;

        let stderr_tail = Arc::new(Mutex::new(VecDeque::new()));
        if let Some(stderr) = stderr {
            let tail = Arc::clone(&stderr_tail);
            spawn_named("pw-record-stderr", move || collect_stderr(stderr, tail))?;
        }

        Ok(Self {
            child,
            blocks: rx,
            current: Vec::new(),
            stderr_tail,
        })
    }

    fn exit_error(&mut self) -> AudioError {
        // The reader saw EOF; give the process a moment to finish exiting.
        let mut status = None;
        for _ in 0..20 {
            if let Ok(Some(s)) = self.child.try_wait() {
                status = Some(s);
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        let status = status.map_or_else(|| "still running".to_string(), |s| s.to_string());
        let tail = self
            .stderr_tail
            .lock()
            .map(|t| t.iter().cloned().collect::<Vec<_>>().join(" | "));
        match tail {
            Ok(t) if !t.is_empty() => {
                AudioError::Failed(format!("pw-record exited ({status}): {t}"))
            }
            _ => AudioError::Failed(format!("pw-record exited ({status})")),
        }
    }
}

impl AudioSource for PwRecordSource {
    fn describe(&self) -> String {
        format!(
            "pw-record (pid {}), default output monitor",
            self.child.id()
        )
    }

    fn next_block(&mut self, timeout: Duration) -> Result<Option<&[f32]>, AudioError> {
        match self.blocks.recv_timeout(timeout) {
            Ok(block) => {
                self.current = block;
                Ok(Some(&self.current))
            }
            Err(RecvTimeoutError::Timeout) => Ok(None),
            Err(RecvTimeoutError::Disconnected) => Err(self.exit_error()),
        }
    }
}

impl Drop for PwRecordSource {
    fn drop(&mut self) {
        // Killing the child closes its stdout, which ends the reader thread.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn spawn_named(name: &str, f: impl FnOnce() + Send + 'static) -> Result<(), AudioError> {
    thread::Builder::new()
        .name(name.into())
        .spawn(f)
        .map(|_| ())
        .map_err(|e| AudioError::Failed(format!("could not spawn {name} thread: {e}")))
}

/// Reads raw f32 LE stereo from the pipe and forwards whole frames.
fn read_pcm(mut stdout: ChildStdout, tx: SyncSender<Vec<f32>>) {
    const FRAME_BYTES: usize = 8; // 2 channels * 4 bytes
    let mut buf = vec![0u8; 16 * 1024];
    // Bytes of an incomplete frame left over from the previous read.
    let mut carry: Vec<u8> = Vec::with_capacity(buf.len() + FRAME_BYTES);
    loop {
        let n = match stdout.read(&mut buf) {
            Ok(0) => return, // EOF: pw-record exited
            Ok(n) => n,
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(_) => return,
        };
        carry.extend_from_slice(&buf[..n]);
        let whole = carry.len() / FRAME_BYTES * FRAME_BYTES;
        let block: Vec<f32> = carry[..whole]
            .as_chunks::<4>()
            .0
            .iter()
            .map(|&b| {
                let s = f32::from_le_bytes(b);
                // Never let a NaN/Inf from a broken stream poison the analysis.
                if s.is_finite() { s } else { 0.0 }
            })
            .collect();
        carry.drain(..whole);
        match tx.try_send(block) {
            Ok(()) | Err(TrySendError::Full(_)) => {}
            Err(TrySendError::Disconnected(_)) => return,
        }
    }
}

fn collect_stderr(stderr: ChildStderr, tail: Arc<Mutex<VecDeque<String>>>) {
    for line in BufReader::new(stderr).lines().map_while(Result::ok) {
        if let Ok(mut t) = tail.lock() {
            if t.len() == 4 {
                t.pop_front();
            }
            t.push_back(line);
        }
    }
}
