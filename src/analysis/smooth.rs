//! Asymmetric one-pole smoothing ("fast attack, slow release").

/// Time constants in seconds. A one-pole filter moves `1 - e^(-dt/tau)` of
/// the way to its target each step, so after `tau` seconds it has covered
/// ~63% of a step change. Expressing smoothing as time constants (instead of
/// a per-step factor) keeps behaviour identical at any update rate.
#[derive(Clone, Copy, Debug)]
pub struct AttackRelease {
    pub attack_s: f32,
    pub release_s: f32,
}

impl AttackRelease {
    pub const fn new(attack_s: f32, release_s: f32) -> Self {
        Self {
            attack_s,
            release_s,
        }
    }

    /// Moves `current` towards `target`; uses the attack time when rising,
    /// release time when falling.
    pub fn step(&self, current: f32, target: f32, dt: f32) -> f32 {
        let tau = if target > current {
            self.attack_s
        } else {
            self.release_s
        };
        current + (target - current) * one_pole_alpha(dt, tau)
    }
}

/// Fraction of the remaining distance a one-pole filter covers in `dt`.
pub fn one_pole_alpha(dt: f32, tau: f32) -> f32 {
    if tau <= 0.0 {
        1.0
    } else {
        1.0 - (-dt / tau).exp()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rises_faster_than_it_falls() {
        let s = AttackRelease::new(0.01, 0.5);
        let up = s.step(0.0, 1.0, 0.01);
        let down = 1.0 - s.step(1.0, 0.0, 0.01);
        assert!(up > 0.6 && down < 0.05, "up {up}, down {down}");
    }

    #[test]
    fn independent_of_step_size() {
        let s = AttackRelease::new(0.1, 0.1);
        let one = s.step(0.0, 1.0, 0.1);
        let mut many = 0.0;
        for _ in 0..10 {
            many = s.step(many, 1.0, 0.01);
        }
        assert!((one - many).abs() < 1e-4);
    }
}
