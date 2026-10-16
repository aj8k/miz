//! Per-frame shader inputs derived from the latest analysis snapshot.

use bytemuck::{Pod, Zeroable};

use crate::analysis::smooth::{AttackRelease, one_pole_alpha};
use crate::snapshot::{MAX_BANDS, Snapshot};

/// Uniform block shared by all shaders. Must match `struct Globals` in
/// shaders/common.wgsl exactly: 4-byte fields, `resolution` first so the
/// vec2 is 8-byte aligned, `waves` (a vec4) on a 16-byte boundary, and the
/// total size a multiple of 16.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct Globals {
    pub resolution: [f32; 2],
    pub time: f32,
    pub energy_time: f32,
    pub dt: f32,
    pub bass: f32,
    pub low_mid: f32,
    pub high_mid: f32,
    pub treble: f32,
    pub loudness: f32,
    pub width: f32,
    pub beat: f32,
    pub idle: f32,
    pub calm: f32,
    pub band_count: u32,
    pub beat_count: u32,
    pub frame: u32,
    pub gain: f32,
    pub transition: f32,
    pub bass_slow: f32,
    pub treble_slow: f32,
    pub loudness_slow: f32,
    pub safety_rate: f32,
    pub _pad: f32,
    /// Two most recent "light waves" (strong beats): age in seconds and
    /// strength, as (age0, strength0, age1, strength1).
    pub waves: [f32; 4],
}

/// Audio data arrives in bursts (one PipeWire quantum ≈ 21 ms), so a 60 FPS
/// render would see values jump every second or third frame. Extra
/// smoothing at frame rate turns those steps into continuous motion; its
/// time constant is also the main "feel" difference between the modes:
/// energetic reacts crisply, calm lazily.
const SMOOTHING_ENERGETIC_S: f32 = 0.03;
const SMOOTHING_CALM_S: f32 = 0.15;
/// The "slow" features: for things that should drift with the music's
/// overall energy (zoom, brightness, detail) rather than follow each hit.
const SLOW: AttackRelease = AttackRelease::new(0.5, 1.5);
/// Calm mode on/off eases over this time constant (seconds).
const CALM_FADE_S: f32 = 0.8;
/// A beat starts a light wave only if it's this strong ...
const WAVE_MIN_STRENGTH: f32 = 0.45;
/// ... and at least this long after the previous wave.
const WAVE_MIN_INTERVAL_S: f32 = 0.45;

pub struct FrameParams {
    pub globals: Globals,
    /// Band levels as uploaded to the GPU (smoothed, then shaped).
    pub bands: [f32; MAX_BANDS],
    /// Frame-smoothed features before mode shaping: bass, low-mid,
    /// high-mid, treble, loudness; and bands.
    features: [f32; 5],
    smooth_bands: [f32; MAX_BANDS],
    /// f64: an f32 accumulator would lose precision after hours of running.
    energy_time: f64,
    last_beat_count: u32,
    /// Beat pulse before calm-mode softening.
    beat: f32,
    /// Where `globals.calm` is heading (it eases there; see `update`).
    calm_target: f32,
    safety_rate: f32,
    calm_safety_rate: f32,
}

impl FrameParams {
    pub fn new() -> Self {
        Self {
            globals: Globals {
                idle: 1.0,
                calm: 1.0,
                // Both wave slots start long expired.
                waves: [100.0, 0.0, 100.0, 0.0],
                ..Globals::default()
            },
            bands: [0.0; MAX_BANDS],
            features: [0.0; 5],
            smooth_bands: [0.0; MAX_BANDS],
            energy_time: 0.0,
            last_beat_count: 0,
            beat: 0.0,
            calm_target: 1.0,
            safety_rate: 0.25,
            calm_safety_rate: 0.12,
        }
    }

    pub fn set_calm(&mut self, calm: bool) {
        self.calm_target = if calm { 1.0 } else { 0.0 };
    }

    pub fn calm(&self) -> bool {
        self.calm_target > 0.5
    }

    pub fn set_safety(&mut self, rate: f32, calm_rate: f32) {
        self.safety_rate = rate;
        self.calm_safety_rate = calm_rate;
    }

    pub fn update(&mut self, snap: &Snapshot, time: f64, dt: f32, resolution: (u32, u32)) {
        let g = &mut self.globals;
        // Calm mode fades in and out over about a second: switching it must
        // not itself cause a sudden change.
        g.calm += (self.calm_target - g.calm) * one_pole_alpha(dt, CALM_FADE_S);
        let calm = g.calm;

        let tau = SMOOTHING_ENERGETIC_S + (SMOOTHING_CALM_S - SMOOTHING_ENERGETIC_S) * calm;
        let a = one_pole_alpha(dt, tau);
        let targets = [
            snap.bass,
            snap.low_mid,
            snap.high_mid,
            snap.treble,
            snap.loudness,
        ];
        for (f, t) in self.features.iter_mut().zip(targets) {
            *f += (t - *f) * a;
        }
        for (b, &s) in self.smooth_bands.iter_mut().zip(snap.bands()) {
            *b += (s - *b) * a;
        }
        // Mode shaping: energetic stretches levels away from the middle
        // (quiet quieter, loud louder: hits stand out), calm squeezes them
        // towards it (everything gentler).
        let contrast = 1.4 + (0.7 - 1.4) * calm;
        let shape = |v: f32| (0.4 + (v - 0.4) * contrast).clamp(0.0, 1.0);
        [g.bass, g.low_mid, g.high_mid, g.treble, g.loudness] = self.features.map(shape);
        for (out, &b) in self.bands.iter_mut().zip(&self.smooth_bands) {
            *out = shape(b);
        }
        let a_fixed = one_pole_alpha(dt, 0.045);
        g.width += (snap.width - g.width) * a_fixed;
        g.idle += (snap.idle - g.idle) * a_fixed;

        // The beat pulse rises a little quicker than everything else; calm
        // mode halves it for every scene.
        self.beat += (snap.beat - self.beat) * one_pole_alpha(dt, 0.02);
        g.beat = self.beat * (1.0 - 0.5 * calm);
        g.bass_slow = SLOW.step(g.bass_slow, snap.bass, dt);
        g.treble_slow = SLOW.step(g.treble_slow, snap.treble, dt);
        g.loudness_slow = SLOW.step(g.loudness_slow, snap.loudness, dt);

        // Light waves: age both, start a new one on a strong, not-too-soon
        // beat. The older wave moves to the second slot so it can finish
        // fading instead of vanishing mid-flight.
        g.waves[0] += dt;
        g.waves[2] += dt;
        if snap.beat_count != self.last_beat_count {
            self.last_beat_count = snap.beat_count;
            if snap.beat >= WAVE_MIN_STRENGTH && g.waves[0] >= WAVE_MIN_INTERVAL_S {
                g.waves = [0.0, snap.beat, g.waves[0], g.waves[1]];
            }
        }

        // "Energy time" runs faster when the music is louder, so motion
        // speeds up gently with the music; during silence it slows to a
        // drift instead of stopping.
        let speed = 0.35 + 1.1 * g.loudness;
        let speed = speed + (0.2 - speed) * g.idle;
        // Mode: energetic runs 1.3x, calm 0.55x.
        let speed = speed * (1.3 + (0.55 - 1.3) * calm);
        self.energy_time += f64::from(dt * speed);

        g.resolution = [resolution.0 as f32, resolution.1 as f32];
        g.time = time as f32;
        g.energy_time = self.energy_time as f32;
        g.dt = dt;
        g.band_count = snap.band_count.max(1) as u32;
        g.beat_count = snap.beat_count;
        g.frame = g.frame.wrapping_add(1);
        // Auto-gain reference from its floor (-70 dB) to 0 dBFS -> 0..1.
        g.gain = ((snap.gain_db + 70.0) / 70.0).clamp(0.0, 1.0);
        g.safety_rate = self.safety_rate + (self.calm_safety_rate - self.safety_rate) * calm;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globals_layout_matches_wgsl() {
        // 28 four-byte fields; `waves` must start on a 16-byte boundary.
        assert_eq!(size_of::<Globals>(), 112);
        assert_eq!(std::mem::offset_of!(Globals, waves), 96);
    }

    #[test]
    fn weak_or_rapid_beats_do_not_start_light_waves() {
        let mut p = FrameParams::new();
        let mut snap = Snapshot::default();
        let dt = 1.0 / 72.0;
        // A strong beat starts a wave.
        snap.beat_count = 1;
        snap.beat = 0.9;
        p.update(&snap, 0.0, dt, (100, 100));
        assert!(p.globals.waves[0] < 0.05 && p.globals.waves[1] > 0.8);
        // Another strong beat 0.1 s later is too soon.
        for _ in 0..7 {
            p.update(&snap, 0.0, dt, (100, 100));
        }
        snap.beat_count = 2;
        p.update(&snap, 0.0, dt, (100, 100));
        assert!(
            p.globals.waves[0] > 0.09,
            "too-soon beat restarted the wave"
        );
        // A weak beat later doesn't either.
        for _ in 0..72 {
            p.update(&snap, 0.0, dt, (100, 100));
        }
        snap.beat_count = 3;
        snap.beat = 0.2;
        p.update(&snap, 0.0, dt, (100, 100));
        assert!(p.globals.waves[0] > 1.0);
    }
}
