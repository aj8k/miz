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
    pub _pad: [f32; 2],
    /// Two most recent "light waves" (strong beats): age in seconds and
    /// strength, as (age0, strength0, age1, strength1).
    pub waves: [f32; 4],
}

/// Audio data arrives in bursts (one PipeWire quantum ≈ 21 ms), so a 60 FPS
/// render would see values jump every second or third frame. A light extra
/// smoothing at frame rate turns those steps into continuous motion.
const FRAME_SMOOTHING_S: f32 = 0.045;
/// The "slow" features: for things that should drift with the music's
/// overall energy (zoom, brightness, detail) rather than follow each hit.
const SLOW: AttackRelease = AttackRelease::new(0.5, 1.5);
/// A beat starts a light wave only if it's this strong ...
const WAVE_MIN_STRENGTH: f32 = 0.45;
/// ... and at least this long after the previous wave.
const WAVE_MIN_INTERVAL_S: f32 = 0.45;

pub struct FrameParams {
    pub globals: Globals,
    pub bands: [f32; MAX_BANDS],
    /// f64: an f32 accumulator would lose precision after hours of running.
    energy_time: f64,
    last_beat_count: u32,
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
            energy_time: 0.0,
            last_beat_count: 0,
        }
    }

    pub fn update(&mut self, snap: &Snapshot, time: f64, dt: f32, resolution: (u32, u32)) {
        let a = one_pole_alpha(dt, FRAME_SMOOTHING_S);
        let g = &mut self.globals;
        let lerp = |cur: &mut f32, target: f32| *cur += (target - *cur) * a;
        lerp(&mut g.bass, snap.bass);
        lerp(&mut g.low_mid, snap.low_mid);
        lerp(&mut g.high_mid, snap.high_mid);
        lerp(&mut g.treble, snap.treble);
        lerp(&mut g.loudness, snap.loudness);
        lerp(&mut g.width, snap.width);
        lerp(&mut g.idle, snap.idle);
        // The beat pulse should rise a little quicker than everything else.
        g.beat += (snap.beat - g.beat) * one_pole_alpha(dt, 0.02);
        for (b, &s) in self.bands.iter_mut().zip(snap.bands()) {
            *b += (s - *b) * a;
        }
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
