//! Turns raw stereo PCM into the features the visuals react to.
//!
//! Per hop (512 new frames ≈ 10.7 ms at 48 kHz):
//!  1. take the latest 2048 frames, mix to mono, apply a Hann window, FFT;
//!  2. power spectrum -> log-spaced bands in dB (with spectral tilt);
//!  3. auto-gain: dB relative to a slowly decaying peak -> 0..1;
//!  4. asymmetric smoothing (fast attack, slow release) per band and for
//!     the aggregate features;
//!  5. spectral-flux onset detection -> smoothly decaying beat pulse;
//!  6. silence detection -> `idle` factor for the idle animation.

mod bands;
mod gain;
mod onset;
pub mod smooth;

use std::sync::Arc;

use rustfft::num_complex::Complex32;
use rustfft::{Fft, FftPlanner};

use crate::audio::SAMPLE_RATE;
use crate::audio::ring::StereoRing;
use crate::snapshot::{MAX_BANDS, Snapshot, WAVE_LEN};
use bands::BandLayout;
use gain::AutoGain;
use onset::OnsetDetector;
use smooth::{AttackRelease, one_pole_alpha};

pub const FFT_SIZE: usize = 2048;
/// New frames between analyses. 2048/512 = 75% window overlap; ~94 analyses
/// per second of audio, comfortably above the display frame rate.
pub const HOP: usize = 512;

#[derive(Clone, Debug, PartialEq)]
pub struct AnalyzerConfig {
    pub bands: usize,
    pub f_min: f32,
    pub f_max: f32,
    /// Spectral tilt in dB/octave (0 dB at 1 kHz); see `BandLayout::new`.
    pub tilt_db_per_octave: f32,
    /// Dynamic range shown, in dB below the auto-gain reference.
    pub range_db: f32,
    pub band_smoothing: AttackRelease,
    pub feature_smoothing: AttackRelease,
    pub gain_decay_db_per_s: f32,
    /// Below this RMS (dBFS) the input counts as silence.
    pub silence_db: f32,
    /// How long silence must last before the idle animation starts.
    pub silence_hold_s: f32,
    /// Decay time constant of the beat pulse.
    pub beat_decay_s: f32,
    pub beat_refractory_s: f32,
}

impl Default for AnalyzerConfig {
    fn default() -> Self {
        Self {
            bands: 64,
            f_min: 30.0,
            f_max: 16000.0,
            tilt_db_per_octave: 4.5,
            range_db: 32.0,
            band_smoothing: AttackRelease::new(0.015, 0.25),
            feature_smoothing: AttackRelease::new(0.03, 0.4),
            gain_decay_db_per_s: 2.0,
            silence_db: -65.0,
            silence_hold_s: 1.5,
            beat_decay_s: 0.3,
            beat_refractory_s: 0.15,
        }
    }
}

/// Frequency ranges (Hz) of the aggregate features.
const BASS: (f32, f32) = (30.0, 150.0);
const LOW_MID: (f32, f32) = (150.0, 600.0);
const HIGH_MID: (f32, f32) = (600.0, 3000.0);
const TREBLE: (f32, f32) = (3000.0, 16000.0);

pub struct Analyzer {
    cfg: AnalyzerConfig,
    ring: StereoRing,
    /// Frames pushed since the last analysis.
    pending: usize,
    /// Peak |sample| of the frames since the last analysis (for wave gain).
    pending_peak: f32,

    fft: Arc<dyn Fft<f32>>,
    window: Vec<f32>,
    frames: Vec<[f32; 2]>,
    spectrum: Vec<Complex32>,
    fft_scratch: Vec<Complex32>,
    power: Vec<f32>,

    layout: BandLayout,
    band_db: Vec<f32>,
    band_norm: Vec<f32>,
    gain: AutoGain,
    wave_gain: AutoGain,
    onset: OnsetDetector,
    ranges: [std::ops::Range<usize>; 4],
    silent_for_s: f32,
    zeros: Vec<f32>,

    snap: Snapshot,
}

impl Analyzer {
    pub fn new(mut cfg: AnalyzerConfig) -> Self {
        cfg.bands = cfg.bands.clamp(8, MAX_BANDS);
        let fft = FftPlanner::<f32>::new().plan_fft_forward(FFT_SIZE);
        // Hann window: tapers the edges of each block to zero so the FFT
        // doesn't see a hard discontinuity, which would smear energy across
        // all bins ("spectral leakage").
        let window: Vec<f32> = (0..FFT_SIZE)
            .map(|i| {
                let x = i as f32 / FFT_SIZE as f32;
                0.5 - 0.5 * (std::f32::consts::TAU * x).cos()
            })
            .collect();
        let layout = BandLayout::new(
            cfg.bands,
            cfg.f_min,
            cfg.f_max,
            SAMPLE_RATE as f32,
            FFT_SIZE,
            cfg.tilt_db_per_octave,
        );
        let ranges = [BASS, LOW_MID, HIGH_MID, TREBLE].map(|(lo, hi)| layout.index_range(lo, hi));
        let centers: Vec<f32> = (0..layout.count())
            .map(|i| {
                let (lo, hi) = layout.edges(i);
                (lo * hi).sqrt()
            })
            .collect();
        let hops_per_s = SAMPLE_RATE as f32 / HOP as f32;
        let onset =
            OnsetDetector::new(&centers, (0.5 * hops_per_s) as usize, cfg.beat_refractory_s);
        let scratch_len = fft.get_inplace_scratch_len();
        let band_count = cfg.bands;

        let snap = Snapshot {
            band_count,
            ..Snapshot::default()
        };

        Self {
            ring: StereoRing::new(FFT_SIZE * 2),
            pending: 0,
            pending_peak: 0.0,
            fft,
            window,
            frames: vec![[0.0; 2]; FFT_SIZE],
            spectrum: vec![Complex32::default(); FFT_SIZE],
            fft_scratch: vec![Complex32::default(); scratch_len],
            power: vec![0.0; FFT_SIZE / 2 + 1],
            layout,
            band_db: vec![0.0; band_count],
            band_norm: vec![0.0; band_count],
            gain: AutoGain::new(-20.0, 0.05, cfg.gain_decay_db_per_s, -70.0),
            wave_gain: AutoGain::new(-12.0, 0.05, cfg.gain_decay_db_per_s, -60.0),
            onset,
            ranges,
            silent_for_s: f32::INFINITY,
            zeros: vec![0.0; HOP * 2],
            snap,
            cfg,
        }
    }

    /// Applies new settings without starting from scratch: auto-gain,
    /// smoothing state and the idle level carry over, so dragging a
    /// setting doesn't make the visuals drop into their idle animation.
    pub fn reconfigure(&mut self, cfg: AnalyzerConfig) {
        let layout_changed = cfg.bands.clamp(8, MAX_BANDS) != self.cfg.bands
            || cfg.f_min != self.cfg.f_min
            || cfg.f_max != self.cfg.f_max
            || cfg.tilt_db_per_octave != self.cfg.tilt_db_per_octave;
        if layout_changed {
            // Band arrays and the onset detector depend on the layout:
            // rebuild, then carry the slow-moving state over.
            let old = std::mem::replace(self, Analyzer::new(cfg));
            self.gain = old.gain;
            self.wave_gain = old.wave_gain;
            self.ring = old.ring;
            self.silent_for_s = old.silent_for_s;
            let band_count = self.snap.band_count;
            self.snap = old.snap;
            self.snap.band_count = band_count;
        } else {
            self.cfg = cfg;
        }
        self.gain.set_decay(self.cfg.gain_decay_db_per_s);
        self.wave_gain.set_decay(self.cfg.gain_decay_db_per_s);
    }

    pub fn snapshot(&self) -> &Snapshot {
        &self.snap
    }

    /// Feeds interleaved stereo samples. Runs one analysis per `HOP` frames;
    /// returns `true` if the snapshot changed.
    pub fn push(&mut self, mut samples: &[f32]) -> bool {
        let mut analyzed = false;
        // Split the input exactly at hop boundaries, so each analysis sees the
        // window ending at the right sample even when a big block arrives.
        while samples.len() >= 2 {
            let take = ((HOP - self.pending) * 2).min(samples.len() & !1);
            let (now, rest) = samples.split_at(take);
            self.ring.push_interleaved(now);
            self.pending_peak = now.iter().fold(self.pending_peak, |m, s| m.max(s.abs()));
            self.pending += take / 2;
            samples = rest;
            if self.pending == HOP {
                self.analyze_hop();
                self.pending = 0;
                self.pending_peak = 0.0;
                analyzed = true;
            }
        }
        analyzed
    }

    /// Feeds `frames` frames of digital silence (used while the source is
    /// stalled or restarting, so the idle animation still kicks in).
    pub fn push_silence(&mut self, mut frames: usize) -> bool {
        let mut analyzed = false;
        let zeros = std::mem::take(&mut self.zeros);
        while frames > 0 {
            let n = frames.min(HOP);
            analyzed |= self.push(&zeros[..n * 2]);
            frames -= n;
        }
        self.zeros = zeros;
        analyzed
    }

    fn analyze_hop(&mut self) {
        let dt = HOP as f32 / SAMPLE_RATE as f32;
        self.ring.latest(&mut self.frames);

        // --- Time domain: RMS, stereo width -------------------------------
        let (mut mid_sq, mut side_sq) = (0.0f32, 0.0f32);
        for &[l, r] in &self.frames {
            let (m, s) = ((l + r) * 0.5, (l - r) * 0.5);
            mid_sq += m * m;
            side_sq += s * s;
        }
        let n = FFT_SIZE as f32;
        let total_rms = ((mid_sq + side_sq) / n).sqrt();
        let rms_db = 20.0 * (total_rms + 1e-9).log10();
        let silent_now = rms_db < self.cfg.silence_db;
        self.silent_for_s = if silent_now {
            self.silent_for_s + dt
        } else {
            0.0
        };
        let idle_target = if self.silent_for_s > self.cfg.silence_hold_s {
            1.0
        } else {
            0.0
        };
        let width_now = if silent_now {
            0.0
        } else {
            let (m, s) = ((mid_sq / n).sqrt(), (side_sq / n).sqrt());
            s / (m + s + 1e-9)
        };

        // --- Spectrum ------------------------------------------------------
        for ((c, &[l, r]), &w) in self.spectrum.iter_mut().zip(&self.frames).zip(&self.window) {
            *c = Complex32::new((l + r) * 0.5 * w, 0.0);
        }
        self.fft
            .process_with_scratch(&mut self.spectrum, &mut self.fft_scratch);
        // Normalize so a full-scale sine reads ~0 dB in its bin: the peak
        // magnitude is A * N/2 * 0.5 (0.5 = Hann window's average gain).
        let norm = 4.0 / n;
        for (p, c) in self.power.iter_mut().zip(&self.spectrum) {
            *p = (c.norm() * norm).powi(2);
        }
        self.layout.apply(&self.power, &mut self.band_db);

        // --- Auto-gain -----------------------------------------------------
        let peak_db = self
            .band_db
            .iter()
            .copied()
            .fold(f32::NEG_INFINITY, f32::max);
        self.gain.update(peak_db, dt, silent_now);
        let range = self.cfg.range_db;
        for (norm, &db) in self.band_norm.iter_mut().zip(&self.band_db) {
            *norm = self.gain.normalize(db, range);
        }
        let wave_peak_db = 20.0 * (self.pending_peak + 1e-9).log10();
        self.wave_gain.update(wave_peak_db, dt, silent_now);
        // Scale so the recent peak sits around 0.8 of full scale.
        let wave_gain = 0.8 / 10f32.powf(self.wave_gain.level_db() / 20.0);

        // --- Onsets (on unsmoothed levels: smoothing would hide the jumps) --
        let onset = self.onset.process(&self.band_norm, dt);

        // --- Publish into the snapshot --------------------------------------
        let snap = &mut self.snap;
        snap.seq += 1;
        let bs = self.cfg.band_smoothing;
        for (s, &target) in snap.bands.iter_mut().zip(&self.band_norm) {
            *s = bs.step(*s, target, dt);
        }
        let mean = |r: &std::ops::Range<usize>| {
            let v = &self.band_norm[r.clone()];
            if v.is_empty() {
                0.0
            } else {
                v.iter().sum::<f32>() / v.len() as f32
            }
        };
        let fs = self.cfg.feature_smoothing;
        snap.bass = fs.step(snap.bass, mean(&self.ranges[0]), dt);
        snap.low_mid = fs.step(snap.low_mid, mean(&self.ranges[1]), dt);
        snap.high_mid = fs.step(snap.high_mid, mean(&self.ranges[2]), dt);
        snap.treble = fs.step(snap.treble, mean(&self.ranges[3]), dt);
        snap.loudness = fs.step(snap.loudness, mean(&(0..self.band_norm.len())), dt);
        // Width is a slow, "spatial" quantity: smooth it heavily.
        snap.width += (width_now - snap.width) * one_pole_alpha(dt, 0.5);

        snap.beat *= (-dt / self.cfg.beat_decay_s).exp();
        if let Some(strength) = onset {
            snap.beat = snap.beat.max(strength);
            snap.beat_count = snap.beat_count.wrapping_add(1);
        }
        // Ease into idle slowly, wake up faster.
        let idle_tau = if idle_target > snap.idle { 1.5 } else { 0.3 };
        snap.idle += (idle_target - snap.idle) * one_pole_alpha(dt, idle_tau);

        let start = FFT_SIZE - WAVE_LEN;
        for (w, &[l, r]) in snap.wave.iter_mut().zip(&self.frames[start..]) {
            *w = [l * wave_gain, r * wave_gain];
        }
        snap.gain_db = self.gain.level_db();
        snap.wave_gain = wave_gain;
        snap.rms_db = rms_db;
        snap.flux = self.onset.flux;
        snap.flux_threshold = self.onset.threshold;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::test_signal::kick;

    const SR: f32 = SAMPLE_RATE as f32;

    /// Feeds `secs` of a mono signal produced by `f(t)` in hop-sized blocks.
    fn feed(a: &mut Analyzer, t0: f32, secs: f32, mut f: impl FnMut(f32) -> f32) {
        let frames = (secs * SR) as usize;
        let mut block = Vec::with_capacity(HOP * 2);
        for i in 0..frames {
            let s = f(t0 + i as f32 / SR);
            block.extend_from_slice(&[s, s]);
            if block.len() == HOP * 2 {
                a.push(&block);
                block.clear();
            }
        }
        a.push(&block);
    }

    fn sine(freq: f32, amp: f32) -> impl Fn(f32) -> f32 {
        move |t| amp * (std::f32::consts::TAU * freq * t).sin()
    }

    fn loudest_band(a: &Analyzer) -> usize {
        let b = a.snapshot().bands();
        (0..b.len())
            .max_by(|&i, &j| b[i].total_cmp(&b[j]))
            .unwrap_or(0)
    }

    #[test]
    fn sine_lands_in_the_right_band() {
        for freq in [60.0, 440.0, 1000.0, 5000.0, 12000.0] {
            let mut a = Analyzer::new(AnalyzerConfig::default());
            feed(&mut a, 0.0, 1.0, sine(freq, 0.5));
            let band = loudest_band(&a);
            let (lo, hi) = a.layout.edges(band);
            // Allow the neighbouring band: a pure tone sits near band edges
            // and the Hann main lobe is ~4 bins wide.
            let (lo2, hi2) = (lo / 1.15, hi * 1.15);
            assert!(
                lo2 <= freq && freq < hi2,
                "{freq} Hz -> band {band} [{lo:.0}, {hi:.0})"
            );
        }
    }

    #[test]
    fn silence_reads_as_zero_and_goes_idle() {
        let mut a = Analyzer::new(AnalyzerConfig::default());
        feed(&mut a, 0.0, 1.0, sine(200.0, 0.5));
        feed(&mut a, 1.0, 5.0, |_| 0.0);
        let s = a.snapshot();
        assert!(s.bands().iter().all(|&b| b < 0.01), "{:?}", s.bands());
        assert!(s.loudness < 0.01 && s.bass < 0.01);
        assert!(s.idle > 0.8, "idle {}", s.idle);
    }

    #[test]
    fn faint_noise_floor_does_not_look_like_music() {
        let mut a = Analyzer::new(AnalyzerConfig::default());
        // ~-80 dBFS hiss: well below the auto-gain floor.
        let mut seed = 1u32;
        feed(&mut a, 0.0, 3.0, move |_| {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            (seed as f32 / u32::MAX as f32 - 0.5) * 2e-4
        });
        assert!(a.snapshot().loudness < 0.05, "{}", a.snapshot().loudness);
    }

    #[test]
    fn auto_gain_converges_to_same_picture_at_lower_volume() {
        let mut a = Analyzer::new(AnalyzerConfig::default());
        feed(&mut a, 0.0, 3.0, sine(440.0, 0.5));
        let band = loudest_band(&a);
        let loud = a.snapshot().bands()[band];
        // Same tone, 26 dB quieter ("Spotify at 5%").
        feed(&mut a, 3.0, 20.0, sine(440.0, 0.025));
        let quiet = a.snapshot().bands()[band];
        assert!((loud - quiet).abs() < 0.05, "loud {loud} vs quiet {quiet}");
        assert!(quiet > 0.8);
    }

    #[test]
    fn onsets_fire_on_kicks() {
        let mut a = Analyzer::new(AnalyzerConfig::default());
        // 120 BPM -> a kick every 0.5 s; 8 s = 16 kicks.
        feed(&mut a, 0.0, 8.0, |t| {
            kick(t % 0.5) * 0.8 + sine(1000.0, 0.05)(t)
        });
        let n = a.snapshot().beat_count;
        assert!((15..=17).contains(&n), "detected {n} onsets, expected 16");
    }

    #[test]
    fn no_onsets_on_a_steady_tone() {
        let mut a = Analyzer::new(AnalyzerConfig::default());
        feed(&mut a, 0.0, 1.0, sine(300.0, 0.4));
        let after_attack = a.snapshot().beat_count;
        feed(&mut a, 1.0, 8.0, sine(300.0, 0.4));
        assert_eq!(a.snapshot().beat_count, after_attack);
        assert!(a.snapshot().beat < 0.01);
    }

    #[test]
    fn reconfigure_keeps_state() {
        let mut a = Analyzer::new(AnalyzerConfig::default());
        feed(&mut a, 0.0, 2.0, sine(440.0, 0.5));
        let gain = a.snapshot().gain_db;
        // A smoothing-only change and a layout change: neither resets.
        a.reconfigure(AnalyzerConfig {
            range_db: 40.0,
            ..AnalyzerConfig::default()
        });
        a.reconfigure(AnalyzerConfig {
            bands: 32,
            ..AnalyzerConfig::default()
        });
        assert_eq!(a.snapshot().band_count, 32);
        assert!(a.snapshot().idle < 0.05, "fell into idle");
        feed(&mut a, 2.0, 0.1, sine(440.0, 0.5));
        assert!((a.snapshot().gain_db - gain).abs() < 1.0);
    }

    #[test]
    fn mono_has_zero_width_and_antiphase_is_wide() {
        let mut a = Analyzer::new(AnalyzerConfig::default());
        feed(&mut a, 0.0, 3.0, sine(500.0, 0.5));
        assert!(a.snapshot().width < 0.02);

        let mut a = Analyzer::new(AnalyzerConfig::default());
        let frames = (3.0 * SR) as usize;
        let s: Vec<f32> = (0..frames)
            .flat_map(|i| {
                let v = sine(500.0, 0.5)(i as f32 / SR);
                [v, -v]
            })
            .collect();
        a.push(&s);
        assert!(a.snapshot().width > 0.95);
    }
}
