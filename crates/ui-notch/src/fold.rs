//! Fold state machine and fold/unfold choreography (visual-spec §1.3, §7, §11).
//!
//! Time is a monotonic `f64` of milliseconds supplied by the caller, so everything here is pure.

use crate::easing::{progress, Bezier};
use codenotch_core::config::ShowMode;

/// Fold (open clip → rest rect) duration.
pub const FOLD_MS: f64 = 360.0;
/// Show-on-hover: fold this long after the pointer left.
pub const HOVER_FOLD_DELAY_MS: f64 = codenotch_ui_kit::metrics::FOLD_DELAY_MS as f64;
/// Extra wait when the grip was out (the orb turns back first).
pub const GRIP_TURN_BACK_MS: f64 = 500.0;
const CELL_FADE_MS: f32 = 180.0;
const CELL_SLIDE_MS: f32 = 300.0;
const CELL_STAGGER_MS: f32 = 28.0;
pub const CELL_SLIDE_PX: f32 = 10.0;
const REST_IN_DELAY_MS: f32 = 180.0;
const REST_IN_MS: f32 = 200.0;
const REST_OUT_MS: f32 = 160.0;
const HANDLES_OUT_DELAY_MS: f32 = 230.0;
const HANDLES_OUT_MS: f32 = 40.0;
const HANDLES_IN_MS: f32 = 10.0;

/// What decides whether the notch is open.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FoldInputs {
    pub show_mode: ShowMode,
    /// A full-screen app is in front and `foldForFullScreen` is on.
    pub fullscreen_fold: bool,
    /// The pointer is over the notch (open hot region, or the wake zone while folded).
    pub hovered: bool,
    /// Something forbids folding: drag, context menu, a press in progress, an open card.
    pub hold: bool,
    /// The grip dots were out when the pointer left.
    pub grip_was_out: bool,
}

/// Desired state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Want {
    Open,
    /// Fold once the pointer has been away this long.
    Fold {
        delay_ms: f64,
    },
}

pub fn want(i: FoldInputs) -> Want {
    if i.hovered || i.hold {
        return Want::Open;
    }
    if i.fullscreen_fold {
        return Want::Fold { delay_ms: 0.0 };
    }
    match i.show_mode {
        ShowMode::Always => Want::Open,
        ShowMode::Hover => Want::Fold {
            delay_ms: HOVER_FOLD_DELAY_MS
                + if i.grip_was_out {
                    GRIP_TURN_BACK_MS
                } else {
                    0.0
                },
        },
        ShowMode::Hidden => Want::Fold { delay_ms: 0.0 },
    }
}

/// While the notch counts as hovered, re-check the pointer this often (see [`effective_hover`]).
pub const HOVER_RECHECK_MS: u64 = 250;

/// Hover as the fold logic sees it. `os` is gpui's flag (`WM_MOUSEMOVE` / `WM_MOUSELEAVE`),
/// which stays `true` for good when a leave is lost: gpui re-arms `TrackMouseEvent` only after
/// a leave it saw, so a leave cancelled by a popup menu's mouse capture, a window hidden under
/// the pointer, or a hover callback gpui could not deliver ("window not found") leaves the notch
/// open forever. `pointer_in` (is the cursor inside the window's region; `None` = unknown)
/// overrides it. A drag and the `--preview-card` demo always count as hovered.
pub fn effective_hover(os: bool, pointer_in: Option<bool>, drag: bool, preview: bool) -> bool {
    drag || preview || (os && pointer_in.unwrap_or(true))
}

/// Should the notch be folded at `now`, given the wish and when the pointer left?
pub fn folded_target(w: Want, left_at_ms: Option<f64>, now_ms: f64) -> bool {
    match w {
        Want::Open => false,
        Want::Fold { delay_ms } => match left_at_ms {
            Some(t) => now_ms - t >= delay_ms,
            None => true,
        },
    }
}

/// Fold animation. `progress` 0 = open, 1 = folded.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FoldAnim {
    folded: bool,
    /// Virtual time into the current direction's choreography at `start_ms`.
    offset_ms: f64,
    start_ms: f64,
    cells: usize,
}

impl FoldAnim {
    pub fn new(folded: bool, cells: usize) -> Self {
        // A settled state: far past the end of its choreography.
        Self {
            folded,
            offset_ms: 1e9,
            start_ms: 0.0,
            cells,
        }
    }

    pub fn folded(&self) -> bool {
        self.folded
    }

    pub fn set_cells(&mut self, cells: usize) {
        self.cells = cells;
    }

    /// Head toward `folded`. Reversing mid-way continues from the current clip position.
    pub fn set_target(&mut self, folded: bool, now_ms: f64) {
        if folded == self.folded {
            return;
        }
        let p = self.linear(now_ms) as f64; // 0 open .. 1 folded
        self.folded = folded;
        let remaining = if folded { p } else { 1.0 - p };
        self.offset_ms = remaining * FOLD_MS;
        self.start_ms = now_ms;
    }

    /// Jump to a state with no animation (reduced motion, first frame).
    pub fn snap(&mut self, folded: bool) {
        *self = Self::new(folded, self.cells);
    }

    fn tau(&self, now_ms: f64) -> f32 {
        (self.offset_ms + (now_ms - self.start_ms).max(0.0)) as f32
    }

    /// Linear clip progress (0 open, 1 folded).
    fn linear(&self, now_ms: f64) -> f32 {
        let t = (self.tau(now_ms) / FOLD_MS as f32).clamp(0.0, 1.0);
        if self.folded {
            t
        } else {
            1.0 - t
        }
    }

    /// Eased clip progress for [`crate::geom::NotchLayout::fold_clip`].
    pub fn clip(&self, now_ms: f64) -> f32 {
        let t = (self.tau(now_ms) / FOLD_MS as f32).clamp(0.0, 1.0);
        let e = Bezier::FOLD.at(t);
        if self.folded {
            e
        } else {
            1.0 - e
        }
    }

    /// Total choreography length in the current direction.
    fn total_ms(&self) -> f32 {
        let stagger = CELL_STAGGER_MS * self.cells.saturating_sub(1) as f32;
        if self.folded {
            (FOLD_MS as f32)
                .max(stagger + CELL_SLIDE_MS)
                .max(REST_IN_DELAY_MS + REST_IN_MS)
        } else {
            (FOLD_MS as f32)
                .max(stagger + CELL_SLIDE_MS)
                .max(REST_OUT_MS)
        }
    }

    pub fn animating(&self, now_ms: f64) -> bool {
        self.tau(now_ms) < self.total_ms()
    }

    /// Fully folded and settled: draw only the rest pill.
    pub fn at_rest(&self, now_ms: f64) -> bool {
        self.folded && !self.animating(now_ms)
    }

    /// Fully open and settled.
    #[cfg(test)]
    pub fn fully_open(&self, now_ms: f64) -> bool {
        !self.folded && !self.animating(now_ms)
    }

    /// Cell `i`: (opacity, slide toward the edge in px at scale 1).
    pub fn cell(&self, i: usize, now_ms: f64) -> (f32, f32) {
        let tau = self.tau(now_ms) - CELL_STAGGER_MS * i as f32;
        let fade = progress(tau, 0.0, CELL_FADE_MS);
        let slide = Bezier::FOLD.at(progress(tau, 0.0, CELL_SLIDE_MS));
        if self.folded {
            (1.0 - fade, CELL_SLIDE_PX * slide)
        } else {
            (fade, CELL_SLIDE_PX * (1.0 - slide))
        }
    }

    /// Rest outline opacity.
    pub fn rest_opacity(&self, now_ms: f64) -> f32 {
        let tau = self.tau(now_ms);
        if self.folded {
            Bezier::EASE.at(progress(tau, REST_IN_DELAY_MS, REST_IN_MS))
        } else {
            1.0 - Bezier::EASE.at(progress(tau, 0.0, REST_OUT_MS))
        }
    }

    /// Orb / grip opacity.
    pub fn handles_opacity(&self, now_ms: f64) -> f32 {
        let tau = self.tau(now_ms);
        if self.folded {
            1.0 - progress(tau, HANDLES_OUT_DELAY_MS, HANDLES_OUT_MS)
        } else {
            progress(tau, 0.0, HANDLES_IN_MS)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs(mode: ShowMode) -> FoldInputs {
        FoldInputs {
            show_mode: mode,
            fullscreen_fold: false,
            hovered: false,
            hold: false,
            grip_was_out: false,
        }
    }

    #[test]
    fn lost_leave_does_not_keep_the_notch_open() {
        // gpui still says hovered, but the pointer is outside: not hovered.
        assert!(!effective_hover(true, Some(false), false, false));
        assert!(effective_hover(true, Some(true), false, false));
        // unknown pointer position: trust gpui
        assert!(effective_hover(true, None, false, false));
        assert!(!effective_hover(false, None, false, false));
        // gpui's flag never invents a hover
        assert!(!effective_hover(false, Some(true), false, false));
        // drag / preview always hold it open
        assert!(effective_hover(false, Some(false), true, false));
        assert!(effective_hover(false, Some(false), false, true));

        // End to end in hover mode: the pointer leaves at t = 1000 and the leave is lost.
        let mode = inputs(ShowMode::Hover);
        let hovered = effective_hover(true, Some(false), false, false);
        let w = want(FoldInputs { hovered, ..mode });
        assert!(!folded_target(w, Some(1000.0), 1449.0));
        assert!(folded_target(w, Some(1000.0), 1450.0));
    }

    #[test]
    fn show_modes() {
        assert_eq!(want(inputs(ShowMode::Always)), Want::Open);
        assert_eq!(
            want(inputs(ShowMode::Hover)),
            Want::Fold { delay_ms: 450.0 }
        );
        let grip = FoldInputs {
            grip_was_out: true,
            ..inputs(ShowMode::Hover)
        };
        assert_eq!(want(grip), Want::Fold { delay_ms: 950.0 });
        let hovered = FoldInputs {
            hovered: true,
            ..inputs(ShowMode::Hover)
        };
        assert_eq!(want(hovered), Want::Open);
        // full screen folds at once, whatever the mode, but hovering still opens
        let fs = FoldInputs {
            fullscreen_fold: true,
            ..inputs(ShowMode::Always)
        };
        assert_eq!(want(fs), Want::Fold { delay_ms: 0.0 });
        assert_eq!(
            want(FoldInputs {
                hovered: true,
                ..fs
            }),
            Want::Open
        );
        // never folds while held (drag, menu)
        assert_eq!(want(FoldInputs { hold: true, ..fs }), Want::Open);
    }

    #[test]
    fn fold_after_delay() {
        let w = Want::Fold { delay_ms: 450.0 };
        assert!(!folded_target(w, Some(1000.0), 1400.0));
        assert!(folded_target(w, Some(1000.0), 1450.0));
        assert!(folded_target(w, None, 0.0));
        assert!(!folded_target(Want::Open, None, 0.0));
    }

    #[test]
    fn choreography() {
        let mut f = FoldAnim::new(false, 3);
        assert!(f.fully_open(0.0));
        assert_eq!(f.clip(0.0), 0.0);
        f.set_target(true, 1000.0);
        assert!(f.animating(1000.0));
        assert_eq!(f.clip(1000.0), 0.0);
        assert_eq!(f.clip(1360.0), 1.0);
        // cell 0 fades over 180 ms, cell 2 starts 56 ms later
        assert_eq!(f.cell(0, 1000.0), (1.0, 0.0));
        assert!((f.cell(0, 1090.0).0 - 0.5).abs() < 1e-4);
        assert!((f.cell(2, 1146.0).0 - 0.5).abs() < 1e-4);
        assert_eq!(f.cell(0, 1400.0).0, 0.0);
        assert!((f.cell(0, 1400.0).1 - 10.0).abs() < 1e-4);
        // rest outline after 180 ms delay over 200 ms
        assert_eq!(f.rest_opacity(1170.0), 0.0);
        assert_eq!(f.rest_opacity(1380.0), 1.0);
        // handles vanish 230..270 ms
        assert_eq!(f.handles_opacity(1229.0), 1.0);
        assert_eq!(f.handles_opacity(1270.0), 0.0);
        assert!(f.at_rest(1400.0));
        assert!(!f.at_rest(1300.0));
    }

    #[test]
    fn reversal_continues_from_current_position() {
        let mut f = FoldAnim::new(false, 1);
        f.set_target(true, 0.0);
        let mid = f.clip(180.0);
        f.set_target(false, 180.0);
        // same linear position (half folded), now heading open
        assert!((f.linear(180.0) - 0.5).abs() < 1e-3);
        assert!(f.clip(180.0) > 0.0 && mid > 0.0);
        assert!(f.linear(270.0) < 0.5);
        assert!(f.fully_open(180.0 + 400.0));
        assert_eq!(f.clip(180.0 + 400.0), 0.0);
        // setting the same target is a no-op
        let before = f;
        f.set_target(false, 999.0);
        assert_eq!(f, before);
    }
}
