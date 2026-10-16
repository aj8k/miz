//! Fixed-size circular buffer of stereo frames.

/// Keeps the most recent `capacity` stereo frames. Pushing overwrites the
/// oldest data, so memory use is constant and no allocation happens after
/// construction.
pub struct StereoRing {
    buf: Vec<[f32; 2]>,
    /// Index where the next frame will be written.
    write: usize,
}

impl StereoRing {
    pub fn new(capacity: usize) -> Self {
        assert!(capacity > 0, "ring capacity must be positive");
        Self {
            buf: vec![[0.0; 2]; capacity],
            write: 0,
        }
    }

    /// Appends interleaved `L R L R ...` samples. A trailing odd sample is ignored.
    pub fn push_interleaved(&mut self, samples: &[f32]) {
        let cap = self.buf.len();
        let (frames, _odd) = samples.as_chunks::<2>();
        for &frame in frames {
            self.buf[self.write] = frame;
            self.write = (self.write + 1) % cap;
        }
    }

    /// Copies the newest `out.len()` frames into `out`, oldest first.
    /// History that was never written reads as silence.
    pub fn latest(&self, out: &mut [[f32; 2]]) {
        let cap = self.buf.len();
        let n = out.len().min(cap);
        let start = (self.write + cap - n) % cap;
        // The requested window may wrap around the end of the buffer: copy it
        // in (at most) two contiguous pieces.
        let first = (cap - start).min(n);
        out[..first].copy_from_slice(&self.buf[start..start + first]);
        out[first..n].copy_from_slice(&self.buf[..n - first]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latest_returns_newest_frames_in_order_across_wrap() {
        let mut r = StereoRing::new(4);
        r.push_interleaved(&[1.0, -1.0, 2.0, -2.0, 3.0, -3.0]);
        r.push_interleaved(&[4.0, -4.0, 5.0, -5.0]);
        let mut out = [[0.0; 2]; 3];
        r.latest(&mut out);
        assert_eq!(out, [[3.0, -3.0], [4.0, -4.0], [5.0, -5.0]]);
    }

    #[test]
    fn unwritten_history_is_silence() {
        let mut r = StereoRing::new(8);
        r.push_interleaved(&[1.0, 1.0]);
        let mut out = [[9.0; 2]; 3];
        r.latest(&mut out);
        assert_eq!(out, [[0.0, 0.0], [0.0, 0.0], [1.0, 1.0]]);
    }
}
