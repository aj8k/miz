//! Auto-gain: make the visuals equally alive at any playback volume.
//!
//! We track a *reference level* (dB) that follows the loudest recent value:
//! it rises quickly when the music gets louder and sinks slowly (a few dB
//! per second) when it gets quieter. Levels are then shown relative to that
//! reference, so a song at 20% volume and the same song at 100% end up
//! looking the same after a few seconds.

use super::smooth::one_pole_alpha;

#[derive(Clone, Copy, Debug)]
pub struct AutoGain {
    level_db: f32,
    /// Time constant for rising (seconds). Not instant, so a single click
    /// doesn't flatten everything for the next ten seconds.
    attack_s: f32,
    /// How fast the reference sinks when the input is quieter (dB/s).
    decay_db_per_s: f32,
    /// The reference never goes below this, so faint noise isn't blown up
    /// into "music".
    floor_db: f32,
}

impl AutoGain {
    pub fn new(initial_db: f32, attack_s: f32, decay_db_per_s: f32, floor_db: f32) -> Self {
        Self {
            level_db: initial_db.max(floor_db),
            attack_s,
            decay_db_per_s,
            floor_db,
        }
    }

    pub fn level_db(&self) -> f32 {
        self.level_db
    }

    /// Feeds the current peak (dB). With `hold` set (e.g. during silence)
    /// the reference does not decay, so music resumes at the old scale.
    pub fn update(&mut self, peak_db: f32, dt: f32, hold: bool) {
        if peak_db > self.level_db {
            self.level_db += (peak_db - self.level_db) * one_pole_alpha(dt, self.attack_s);
        } else if !hold {
            self.level_db = (self.level_db - self.decay_db_per_s * dt).max(peak_db);
        }
        self.level_db = self.level_db.max(self.floor_db);
    }

    /// Maps `db` to 0..1: the reference level lands at ~0.93 (a little
    /// headroom so the loudest band isn't pinned at the top) and anything
    /// `range_db` below the reference is 0.
    pub fn normalize(&self, db: f32, range_db: f32) -> f32 {
        const HEADROOM_DB: f32 = 3.0;
        ((db - (self.level_db - range_db)) / (range_db + HEADROOM_DB)).clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follows_louder_input_quickly_and_quieter_slowly() {
        let mut g = AutoGain::new(-40.0, 0.05, 2.0, -70.0);
        for _ in 0..50 {
            g.update(-10.0, 0.01, false);
        }
        assert!((g.level_db() + 10.0).abs() < 0.5, "{}", g.level_db());
        // One second later the reference has only sunk ~2 dB.
        for _ in 0..100 {
            g.update(-30.0, 0.01, false);
        }
        assert!((g.level_db() + 12.0).abs() < 0.5, "{}", g.level_db());
    }

    #[test]
    fn holds_during_silence_and_respects_floor() {
        let mut g = AutoGain::new(-20.0, 0.05, 2.0, -70.0);
        for _ in 0..1000 {
            g.update(-120.0, 0.01, true);
        }
        assert_eq!(g.level_db(), -20.0);
        for _ in 0..10000 {
            g.update(-120.0, 0.01, false);
        }
        assert_eq!(g.level_db(), -70.0);
    }
}
