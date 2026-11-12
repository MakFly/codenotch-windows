//! Adaptive rest pill (visual-spec §6.2): while folded, the rest pill flips between the dark and
//! the light pill depending on what is behind it. The screen sampling lives in
//! `codenotch-platform` (`backdrop::luminance`), which this crate may not depend on, so the app
//! installs it with [`set_backdrop_sampler`]. Without a sampler the feature is inert.

use std::sync::{Arc, RwLock};

/// `(x, y, w, h)` in device px → mean relative luminance 0..1, `None` on failure.
pub type Sampler = dyn Fn(i32, i32, i32, i32) -> Option<f32> + Send + Sync;

static SAMPLER: RwLock<Option<Arc<Sampler>>> = RwLock::new(None);

/// Install the backdrop sampler (call once at start-up, e.g. with
/// `codenotch_platform::backdrop::luminance`). It is called off the UI thread.
pub fn set_backdrop_sampler(f: impl Fn(i32, i32, i32, i32) -> Option<f32> + Send + Sync + 'static) {
    if let Ok(mut s) = SAMPLER.write() {
        *s = Some(Arc::new(f));
    }
}

pub(crate) fn sampler() -> Option<Arc<Sampler>> {
    SAMPLER.read().ok().and_then(|s| s.clone())
}

/// Relative luminance of the dark (`#000`) and light (`#f5f5f7`) pills.
pub const DARK_PILL_L: f32 = 0.0;
pub const LIGHT_PILL_L: f32 = 0.914;
/// The other pill must win by this factor before we switch.
pub const HYSTERESIS: f32 = 1.5;
/// First sample after folding, then the period, and the retry delay after a failure.
pub const SETTLE_MS: u64 = 450;
pub const PERIOD_MS: u64 = 500;
pub const RETRY_MS: u64 = 5000;
/// Colour transition.
pub const FADE_MS: f32 = 300.0;

/// WCAG contrast ratio between two relative luminances.
pub fn contrast(a: f32, b: f32) -> f32 {
    let (hi, lo) = if a >= b { (a, b) } else { (b, a) };
    (hi + 0.05) / (lo + 0.05)
}

/// Pill tone over a backdrop of luminance `l`: `true` = light pill. Keeps `current_light`
/// unless the other tone's contrast is more than 1.5× better.
pub fn choose_light(l: f32, current_light: bool) -> bool {
    let l = l.clamp(0.0, 1.0);
    let dark = contrast(l, DARK_PILL_L);
    let light = contrast(l, LIGHT_PILL_L);
    if current_light {
        dark <= light * HYSTERESIS
    } else {
        light > dark * HYSTERESIS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contrast_ratio() {
        assert!((contrast(1.0, 0.0) - 21.0).abs() < 1e-4);
        assert_eq!(contrast(0.3, 0.3), 1.0);
    }

    #[test]
    fn switches_with_hysteresis() {
        // black desktop: light pill wins clearly
        assert!(choose_light(0.0, false));
        // white window: dark pill
        assert!(!choose_light(1.0, true));
        // mid grey: no clear winner, keep what we have
        assert!(choose_light(0.2, true));
        assert!(!choose_light(0.2, false));
    }

    #[test]
    fn sampler_hook() {
        set_backdrop_sampler(|_, _, w, _| Some(w as f32 / 100.0));
        let s = sampler().unwrap();
        assert_eq!(s(0, 0, 50, 1), Some(0.5));
    }
}
