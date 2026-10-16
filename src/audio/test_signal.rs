//! Built-in synthetic music: develop visuals without PipeWire or Spotify.
//!
//! A 40 s loop: a slow logarithmic sine sweep that wanders around the stereo
//! field, a kick drum at 120 BPM, soft off-beat hi-hats, and 4 s of silence
//! at the end of each loop to exercise the idle animation.

use std::f32::consts::TAU;
use std::time::{Duration, Instant};

use super::{AudioError, AudioSource, SAMPLE_RATE};

const BLOCK_FRAMES: usize = 512;
const LOOP_S: f32 = 40.0;
const SILENCE_S: f32 = 4.0;
const BEAT_S: f32 = 0.5; // 120 BPM

/// A kick drum hit, `t` seconds after it starts: a sine whose pitch drops
/// quickly from ~150 Hz to 50 Hz under a short exponential envelope.
pub fn kick(t: f32) -> f32 {
    // Integral of f(t) = 50 + 100 e^(-t/0.03), so the pitch glide is smooth.
    let phase = TAU * (50.0 * t + 100.0 * 0.03 * (1.0 - (-t / 0.03).exp()));
    phase.sin() * (-t / 0.12).exp()
}

pub struct TestSignal {
    frame: u64,
    sweep_phase: f32,
    noise_state: u32,
    prev_noise: f32,
    next_due: Instant,
    block: Vec<f32>,
}

impl TestSignal {
    pub fn new() -> Self {
        Self {
            frame: 0,
            sweep_phase: 0.0,
            noise_state: 0x1234_5678,
            prev_noise: 0.0,
            next_due: Instant::now(),
            block: vec![0.0; BLOCK_FRAMES * 2],
        }
    }

    fn noise(&mut self) -> f32 {
        // Tiny LCG; plenty for a hi-hat.
        self.noise_state = self
            .noise_state
            .wrapping_mul(1_664_525)
            .wrapping_add(1_013_904_223);
        (self.noise_state >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0
    }

    fn next_frame(&mut self) -> [f32; 2] {
        // Loop time computed in f64: an f32 seconds counter loses sub-sample
        // precision after a few hours.
        let tl = ((self.frame as f64 / SAMPLE_RATE as f64) % LOOP_S as f64) as f32;
        self.frame += 1;
        if tl > LOOP_S - SILENCE_S {
            return [0.0, 0.0];
        }

        // Sweep 50 Hz -> 6 kHz -> 50 Hz on a log scale, 24 s per round trip.
        let tri = 1.0 - ((tl / 12.0) % 2.0 - 1.0).abs();
        let freq = 50.0 * (6000.0f32 / 50.0).powf(tri);
        self.sweep_phase = (self.sweep_phase + TAU * freq / SAMPLE_RATE as f32) % TAU;
        let sweep = self.sweep_phase.sin() * 0.15;

        let kick = kick(tl % BEAT_S) * 0.7;

        // Hi-hat on the off-beat: differentiated noise (= high-pass) with a
        // very short envelope.
        let n = self.noise();
        let hp = n - self.prev_noise;
        self.prev_noise = n;
        let hat = hp * 0.05 * (-((tl + BEAT_S / 2.0) % BEAT_S) / 0.02).exp();

        // Equal-power pan for the sweep, slowly drifting left/right.
        let pan = 0.5 + 0.45 * (TAU * tl / 7.0).sin();
        let (gl, gr) = ((pan * TAU / 4.0).cos(), (pan * TAU / 4.0).sin());
        [kick + hat + sweep * gl, kick + hat * 0.7 + sweep * gr]
    }
}

impl AudioSource for TestSignal {
    fn describe(&self) -> String {
        "synthetic test signal".into()
    }

    fn next_block(&mut self, _timeout: Duration) -> Result<Option<&[f32]>, AudioError> {
        // Pace generation in real time, like a sound card would.
        let now = Instant::now();
        if self.next_due > now {
            std::thread::sleep(self.next_due - now);
        } else if now - self.next_due > Duration::from_millis(500) {
            self.next_due = now; // we fell far behind (e.g. suspend); don't burst
        }
        self.next_due += Duration::from_secs_f64(BLOCK_FRAMES as f64 / SAMPLE_RATE as f64);

        for i in 0..BLOCK_FRAMES {
            let [l, r] = self.next_frame();
            self.block[2 * i] = l;
            self.block[2 * i + 1] = r;
        }
        Ok(Some(&self.block))
    }
}
