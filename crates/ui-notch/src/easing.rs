//! Timing curves. Pure functions of elapsed milliseconds.

/// CSS `cubic-bezier(x1, y1, x2, y2)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bezier(pub f32, pub f32, pub f32, pub f32);

impl Bezier {
    /// Fold / unfold, notch window glide.
    pub const FOLD: Bezier = Bezier(0.32, 0.72, 0.24, 1.0);
    /// Card glide between cells.
    pub const GLIDE: Bezier = Bezier(0.33, 1.0, 0.68, 1.0);
    /// Ring press (overshoots).
    pub const PRESS: Bezier = Bezier(0.34, 1.4, 0.64, 1.0);
    /// Refresh spin of the reading layer.
    pub const SPIN: Bezier = Bezier(0.32, 0.0, 0.14, 1.0);
    /// CSS `ease`.
    pub const EASE: Bezier = Bezier(0.25, 0.1, 0.25, 1.0);
    /// CSS `ease-out`.
    pub const EASE_OUT: Bezier = Bezier(0.0, 0.0, 0.58, 1.0);
    /// Orb hover transform.
    pub const ORB: Bezier = Bezier(0.34, 1.3, 0.64, 1.0);
    /// Orb gear entrance.
    pub const GEAR: Bezier = Bezier(0.34, 1.2, 0.64, 1.0);
    /// Orb click release.
    pub const BOUNCE: Bezier = Bezier(0.34, 1.56, 0.64, 1.0);
    /// Grip dots out / in.
    pub const GRIP_OUT: Bezier = Bezier(0.35, 0.0, 0.25, 1.0);
    pub const GRIP_IN: Bezier = Bezier(0.42, 0.0, 1.0, 1.0);

    /// Eased value at linear progress `t` (clamped to 0..=1). Values may overshoot 1 for curves
    /// whose control points do.
    pub fn at(self, t: f32) -> f32 {
        if t.is_nan() || t <= 0.0 {
            return 0.0;
        }
        if t >= 1.0 {
            return 1.0;
        }
        let Bezier(x1, y1, x2, y2) = self;
        let s = solve_param(x1, x2, t);
        cubic(y1, y2, s)
    }
}

/// One coordinate of a unit cubic Bézier (P0 = 0, P3 = 1) at parameter `s`.
fn cubic(p1: f32, p2: f32, s: f32) -> f32 {
    let r = 1.0 - s;
    3.0 * r * r * s * p1 + 3.0 * r * s * s * p2 + s * s * s
}

fn cubic_d(p1: f32, p2: f32, s: f32) -> f32 {
    let r = 1.0 - s;
    3.0 * r * r * p1 + 6.0 * r * s * (p2 - p1) + 3.0 * s * s * (1.0 - p2)
}

/// Parameter `s` with x(s) = x (Newton, then bisection when the slope is flat).
fn solve_param(x1: f32, x2: f32, x: f32) -> f32 {
    let mut s = x;
    for _ in 0..8 {
        let err = cubic(x1, x2, s) - x;
        if err.abs() < 1e-5 {
            return s;
        }
        let d = cubic_d(x1, x2, s);
        if d.abs() < 1e-6 {
            break;
        }
        s = (s - err / d).clamp(0.0, 1.0);
    }
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    s = x;
    for _ in 0..30 {
        let v = cubic(x1, x2, s);
        if (v - x).abs() < 1e-5 {
            break;
        }
        if v < x {
            lo = s;
        } else {
            hi = s;
        }
        s = (lo + hi) / 2.0;
    }
    s
}

/// Linear progress of an animation of `duration` ms that starts `delay` ms after its origin.
pub fn progress(elapsed_ms: f32, delay_ms: f32, duration_ms: f32) -> f32 {
    if duration_ms <= 0.0 {
        return if elapsed_ms >= delay_ms { 1.0 } else { 0.0 };
    }
    ((elapsed_ms - delay_ms) / duration_ms).clamp(0.0, 1.0)
}

pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// CSS `steps(n)` (jump-end): ⌊t·n⌋ / n, with t clamped to 0..1 (t = 1 → 1).
pub fn steps(t: f32, n: u32) -> f32 {
    let n = n.max(1) as f32;
    if t >= 1.0 {
        return 1.0;
    }
    (t.max(0.0) * n).floor() / n
}

/// Working arc rotation (degrees): one turn per 1.2 s in 12 steps.
pub fn working_rotation(elapsed_ms: f64) -> f32 {
    let period = codenotch_ui_kit::metrics::WORKING_SPIN_MS as f64;
    let phase = (elapsed_ms.rem_euclid(period) / period) as f32;
    steps(phase, 12) * 360.0
}

/// Waiting ring opacity: 1 → 0.25 → 1 over 1.1 s, 6 steps per half.
pub fn waiting_opacity(elapsed_ms: f64) -> f32 {
    let period = codenotch_ui_kit::metrics::WAITING_PULSE_MS as f64;
    let phase = (elapsed_ms.rem_euclid(period) / period) as f32;
    if phase < 0.5 {
        1.0 - 0.75 * steps(phase / 0.5, 6)
    } else {
        0.25 + 0.75 * steps((phase - 0.5) / 0.5, 6)
    }
}

/// Milliseconds until the next boundary of a stepped animation with `steps` steps per `period`.
pub fn ms_to_next_step(elapsed_ms: f64, period_ms: f64, steps: u32) -> f64 {
    let step = period_ms / steps.max(1) as f64;
    let into = elapsed_ms.rem_euclid(step);
    (step - into).max(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bezier_end_points_and_monotone_ease() {
        for b in [
            Bezier::FOLD,
            Bezier::GLIDE,
            Bezier::SPIN,
            Bezier::EASE,
            Bezier::EASE_OUT,
        ] {
            assert_eq!(b.at(0.0), 0.0);
            assert_eq!(b.at(1.0), 1.0);
            let mut last = 0.0;
            for i in 1..100 {
                let v = b.at(i as f32 / 100.0);
                assert!(v + 1e-4 >= last, "{b:?} not monotone at {i}");
                last = v;
            }
        }
        // linear curve is the identity
        let lin = Bezier(0.0, 0.0, 1.0, 1.0);
        assert!((lin.at(0.37) - 0.37).abs() < 1e-3);
        // CSS `ease` at 50 % ≈ 0.8024
        assert!((Bezier::EASE.at(0.5) - 0.8024).abs() < 2e-3);
    }

    #[test]
    fn press_curve_overshoots() {
        let peak = (1..100)
            .map(|i| Bezier::PRESS.at(i as f32 / 100.0))
            .fold(0.0f32, f32::max);
        assert!(peak > 1.0);
    }

    #[test]
    fn stepped_spin_and_pulse() {
        assert_eq!(working_rotation(0.0), 0.0);
        assert_eq!(working_rotation(99.0), 0.0);
        assert_eq!(working_rotation(100.0), 30.0);
        assert_eq!(working_rotation(1199.0), 330.0);
        assert_eq!(working_rotation(1200.0), 0.0);
        assert_eq!(waiting_opacity(0.0), 1.0);
        assert!((waiting_opacity(550.0) - 0.25).abs() < 1e-6);
        assert!(waiting_opacity(500.0) < 0.4);
        assert!((ms_to_next_step(30.0, 1200.0, 12) - 70.0).abs() < 1e-9);
    }

    #[test]
    fn progress_with_delay() {
        assert_eq!(progress(100.0, 180.0, 200.0), 0.0);
        assert_eq!(progress(280.0, 180.0, 200.0), 0.5);
        assert_eq!(progress(1000.0, 180.0, 200.0), 1.0);
        assert_eq!(progress(5.0, 0.0, 0.0), 1.0);
        assert_eq!(steps(0.99, 6), 5.0 / 6.0);
    }
}
