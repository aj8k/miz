//! Mapping FFT bins onto logarithmically spaced frequency bands.
//!
//! We hear pitch logarithmically (each octave sounds like the same "step"),
//! so equal-width log bands give every octave the same amount of screen.
//! With a 2048-point FFT at 48 kHz each bin is 23.4 Hz wide, which means the
//! lowest bands (below ~250 Hz with 64 bands) are *narrower than one bin*:
//! they can't be truly resolved, so we linearly interpolate the spectrum at
//! the band centre instead. That gives a smooth curve rather than stair steps.

use std::ops::Range;

pub struct BandLayout {
    /// `count + 1` band edges in Hz, log spaced.
    edges: Vec<f32>,
    /// Geometric centre of each band in Hz.
    centers: Vec<f32>,
    /// Flattened `(bin, weight)` lists; band `i` uses `weights[spans[i]]`.
    weights: Vec<(usize, f32)>,
    spans: Vec<Range<usize>>,
    /// Per-band dB offset that tilts the spectrum (see `new`).
    tilt_db: Vec<f32>,
}

impl BandLayout {
    /// * `tilt_db_per_octave` compensates for music's natural spectral slope:
    ///   per Hz, most music has far more energy in the bass than in the
    ///   treble (roughly -3..-6 dB per octave). Without a tilt the treble
    ///   bands would barely move. The tilt is 0 dB at 1 kHz.
    pub fn new(
        count: usize,
        f_min: f32,
        f_max: f32,
        sample_rate: f32,
        fft_size: usize,
        tilt_db_per_octave: f32,
    ) -> Self {
        assert!(count > 0 && f_min > 0.0 && f_max > f_min);
        let nyquist_bin = fft_size / 2;
        let bin_hz = sample_rate / fft_size as f32;
        let ratio = (f_max / f_min).powf(1.0 / count as f32);

        let edges: Vec<f32> = (0..=count).map(|i| f_min * ratio.powi(i as i32)).collect();
        let mut centers = Vec::with_capacity(count);
        let mut weights = Vec::new();
        let mut spans = Vec::with_capacity(count);
        let mut tilt_db = Vec::with_capacity(count);

        for band in edges.windows(2) {
            let (lo, hi) = (band[0], band[1]);
            let center = (lo * hi).sqrt();
            centers.push(center);
            tilt_db.push(tilt_db_per_octave * (center / 1000.0).log2());

            let start = weights.len();
            if (hi - lo) < bin_hz {
                // Narrower than a bin: interpolate between the two nearest bins.
                let x = center / bin_hz;
                let k0 = (x.floor() as usize).clamp(1, nyquist_bin - 1);
                let frac = (x - k0 as f32).clamp(0.0, 1.0);
                weights.push((k0, 1.0 - frac));
                weights.push((k0 + 1, frac));
            } else {
                // Wider than a bin: average the bins, each weighted by how much
                // of its frequency interval [k-0.5, k+0.5]*bin_hz overlaps the band.
                let k_lo = ((lo / bin_hz - 0.5).floor().max(1.0)) as usize;
                let k_hi = ((hi / bin_hz + 0.5).ceil() as usize).min(nyquist_bin);
                for k in k_lo..=k_hi {
                    let b_lo = (k as f32 - 0.5) * bin_hz;
                    let b_hi = (k as f32 + 0.5) * bin_hz;
                    let overlap = hi.min(b_hi) - lo.max(b_lo);
                    if overlap > 0.0 {
                        weights.push((k, overlap / (hi - lo)));
                    }
                }
            }
            spans.push(start..weights.len());
        }

        Self {
            edges,
            centers,
            weights,
            spans,
            tilt_db,
        }
    }

    pub fn count(&self) -> usize {
        self.centers.len()
    }

    pub fn edges(&self, band: usize) -> (f32, f32) {
        (self.edges[band], self.edges[band + 1])
    }

    /// Bands whose centre frequency lies in `[lo_hz, hi_hz)`.
    pub fn index_range(&self, lo_hz: f32, hi_hz: f32) -> Range<usize> {
        let start = self.centers.partition_point(|&c| c < lo_hz);
        let end = self.centers.partition_point(|&c| c < hi_hz);
        start..end.max(start)
    }

    /// Converts a power spectrum (`|X_k|^2`, already normalized) into
    /// tilted band levels in dB.
    pub fn apply(&self, power: &[f32], out_db: &mut [f32]) {
        for (i, out) in out_db.iter_mut().enumerate().take(self.count()) {
            let p: f32 = self.weights[self.spans[i].clone()]
                .iter()
                .map(|&(k, w)| power[k] * w)
                .sum();
            // +1e-12 keeps log10 finite in digital silence (floor = -120 dB).
            *out = 10.0 * (p + 1e-12).log10() + self.tilt_db[i];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weights_sum_to_one_and_edges_are_increasing() {
        let l = BandLayout::new(64, 30.0, 16000.0, 48000.0, 2048, 0.0);
        assert_eq!(l.count(), 64);
        for i in 0..64 {
            let (lo, hi) = l.edges(i);
            assert!(hi > lo);
            let sum: f32 = l.weights[l.spans[i].clone()].iter().map(|w| w.1).sum();
            assert!((sum - 1.0).abs() < 1e-3, "band {i} weight sum {sum}");
        }
    }

    #[test]
    fn index_range_selects_by_centre() {
        let l = BandLayout::new(64, 30.0, 16000.0, 48000.0, 2048, 0.0);
        let r = l.index_range(30.0, 150.0);
        assert!(!r.is_empty());
        assert!(l.centers[r.end - 1] < 150.0 && l.centers[r.end] >= 150.0);
    }
}
