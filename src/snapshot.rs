//! The analysis result shared between the audio thread and its readers.

use std::sync::{Mutex, TryLockError};
use std::time::Instant;

/// Upper bound for the configurable band count (keeps the snapshot `Copy`
/// and allocation-free).
pub const MAX_BANDS: usize = 128;
/// Number of recent stereo frames exposed as the waveform (~21 ms at 48 kHz).
pub const WAVE_LEN: usize = 1024;

/// Everything the visuals need to know about the music right now.
/// All "level" values are normalized by auto-gain to roughly 0..1.
#[derive(Clone, Copy)]
pub struct Snapshot {
    /// Increments on every analysis hop; lets readers detect fresh data.
    pub seq: u64,
    /// When the audio thread published this snapshot (`None` before the first one).
    pub published_at: Option<Instant>,

    pub band_count: usize,
    /// Smoothed, auto-gained band levels; only `..band_count` are meaningful.
    pub bands: [f32; MAX_BANDS],
    /// Recent stereo samples (oldest first), scaled by the waveform auto-gain.
    pub wave: [[f32; 2]; WAVE_LEN],

    pub bass: f32,
    pub low_mid: f32,
    pub high_mid: f32,
    pub treble: f32,
    pub loudness: f32,
    /// 0 = mono, ~0.5 = wide, 1 = fully out of phase.
    pub width: f32,
    /// Share of the spectrum that is sounding: 0 = one pure tone .. 1 = a
    /// dense mix with every band busy.
    pub complexity: f32,

    /// Beat pulse: jumps up on an onset, then decays smoothly towards 0.
    pub beat: f32,
    /// Total number of detected onsets (handy for "every beat" effects).
    pub beat_count: u32,
    /// 0 while music plays, eases to 1 after a few seconds of silence.
    pub idle: f32,

    /// Auto-gain reference level in dB (the "loudest band" tracker).
    pub gain_db: f32,
    /// Linear gain applied to `wave`.
    pub wave_gain: f32,
    /// Raw signal RMS in dBFS, before any normalization.
    pub rms_db: f32,
    /// Onset detector internals, for the debug views.
    pub flux: f32,
    pub flux_threshold: f32,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            seq: 0,
            published_at: None,
            band_count: 0,
            bands: [0.0; MAX_BANDS],
            wave: [[0.0; 2]; WAVE_LEN],
            bass: 0.0,
            low_mid: 0.0,
            high_mid: 0.0,
            treble: 0.0,
            loudness: 0.0,
            width: 0.0,
            complexity: 0.0,
            beat: 0.0,
            beat_count: 0,
            idle: 1.0,
            gain_db: 0.0,
            wave_gain: 1.0,
            rms_db: -120.0,
            flux: 0.0,
            flux_threshold: 0.0,
        }
    }
}

impl Snapshot {
    pub fn bands(&self) -> &[f32] {
        &self.bands[..self.band_count]
    }
}

/// "Latest value" mailbox between one writer (audio thread) and readers.
///
/// The writer takes the lock just long enough to memcpy ~9 KB. Readers use
/// `try_lock`: if the writer happens to hold the lock, the reader simply keeps
/// the snapshot it already has. So the render loop can never be blocked by
/// audio, and the audio thread only ever waits for a memcpy.
pub struct SnapshotCell {
    inner: Mutex<Snapshot>,
}

impl SnapshotCell {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Snapshot::default()),
        }
    }

    pub fn publish(&self, snap: &Snapshot) {
        // A poisoned lock only means a reader panicked mid-copy; the data is
        // plain old data, so it is still safe to overwrite.
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        *guard = *snap;
        guard.published_at = Some(Instant::now());
    }

    /// Copies the latest snapshot into `out` without blocking.
    /// Returns `false` (leaving `out` untouched) if the lock was busy.
    pub fn read_into(&self, out: &mut Snapshot) -> bool {
        match self.inner.try_lock() {
            Ok(guard) => {
                *out = *guard;
                true
            }
            Err(TryLockError::Poisoned(p)) => {
                *out = *p.into_inner();
                true
            }
            Err(TryLockError::WouldBlock) => false,
        }
    }
}
