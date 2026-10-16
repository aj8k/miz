//! Onset ("beat") detection via spectral flux with an adaptive threshold.
//!
//! Spectral flux = how much the spectrum *increased* since the previous hop,
//! summed over bands. Only increases count (half-wave rectification): a drum
//! hit makes many bands jump up at once, while a sustained note doesn't
//! change at all. To decide what counts as "a jump" we compare against the
//! median of the recent flux history (robust against the hits themselves)
//! times a factor, plus a small constant so near-silence never triggers.

use std::collections::VecDeque;

pub struct OnsetDetector {
    prev: Vec<f32>,
    /// Per-band emphasis: bass and low-mids drive the "beat" most.
    weights: Vec<f32>,
    weight_sum: f32,
    history: VecDeque<f32>,
    history_len: usize,
    scratch: Vec<f32>,
    /// Threshold = median * k + delta.
    k: f32,
    delta: f32,
    /// Minimum time between onsets; also caps how often visuals can pulse.
    refractory_s: f32,
    since_onset_s: f32,
    above: bool,
    pub flux: f32,
    pub threshold: f32,
}

impl OnsetDetector {
    /// `band_centers` in Hz; `history_len` hops for the median window.
    pub fn new(band_centers: &[f32], history_len: usize, refractory_s: f32) -> Self {
        let weights: Vec<f32> = band_centers
            .iter()
            .map(|&f| match f {
                f if f < 200.0 => 2.0,
                f if f < 2000.0 => 1.0,
                _ => 0.4,
            })
            .collect();
        let weight_sum = weights.iter().sum();
        Self {
            prev: vec![0.0; band_centers.len()],
            weights,
            weight_sum,
            history: VecDeque::with_capacity(history_len),
            history_len,
            scratch: Vec::with_capacity(history_len),
            k: 1.5,
            delta: 0.02,
            refractory_s,
            since_onset_s: f32::INFINITY,
            above: false,
            flux: 0.0,
            threshold: 0.0,
        }
    }

    /// Feeds normalized (0..1, *unsmoothed*) band levels for one hop.
    /// Returns the onset strength (0..1) if an onset starts at this hop.
    pub fn process(&mut self, bands: &[f32], dt: f32) -> Option<f32> {
        let mut flux = 0.0;
        for ((&b, p), &w) in bands.iter().zip(self.prev.iter_mut()).zip(&self.weights) {
            flux += (b - *p).max(0.0) * w;
            *p = b;
        }
        self.flux = flux / self.weight_sum;
        self.threshold = self.median() * self.k + self.delta;

        if self.history.len() == self.history_len {
            self.history.pop_front();
        }
        self.history.push_back(self.flux);
        self.since_onset_s += dt;

        // Fire on the rising edge of "flux above threshold" (no added latency).
        let above = self.flux > self.threshold;
        let rising = above && !self.above;
        self.above = above;
        if rising && self.since_onset_s >= self.refractory_s {
            self.since_onset_s = 0.0;
            // Relative to the threshold, so it's independent of the overall
            // flux scale: twice the threshold (or more) = full strength.
            let strength = (self.flux / self.threshold - 1.0).clamp(0.2, 1.0);
            return Some(strength);
        }
        None
    }

    fn median(&mut self) -> f32 {
        if self.history.is_empty() {
            return 0.0;
        }
        self.scratch.clear();
        self.scratch.extend(self.history.iter().copied());
        let mid = self.scratch.len() / 2;
        let (_, m, _) = self.scratch.select_nth_unstable_by(mid, f32::total_cmp);
        *m
    }
}
