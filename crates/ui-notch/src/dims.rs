//! Notch geometry constants, logical px at Medium size (scale 1.0).
//! Everything here is multiplied by `NotchSize::scale()` before use. Values that ui-kit already
//! exports are re-used from `codenotch_ui_kit::metrics`.

pub use codenotch_ui_kit::metrics::{
    CARD_PAD, CARD_RADIUS, FILLET_R, PERCENT_FONT, PILL_CELL_GAP, PILL_DEPTH, PILL_PAD_END,
    PILL_RADIUS, RING_BOX, TAIL_LEN,
};

/// Outline width of the pill, the rest pill and the fillet rim.
pub const BORDER: f32 = 1.0;
/// Cell box: ring 44 + gap 6 + one 20 px text line.
pub const CELL_H: f32 = 70.0;
pub const CELL_TEXT_GAP: f32 = 6.0;
pub const PERCENT_LINE: f32 = 20.0;
/// Flat pills: 12 px above and below the cell row.
pub const FLAT_PAD: f32 = 12.0;

/// Notch window: upright 360 deep, flat 650 deep; at least 650 along the edge.
pub const WIN_UPRIGHT_DEPTH: f32 = 360.0;
pub const WIN_FLAT_DEPTH: f32 = 650.0;
pub const WIN_MIN_ALONG: f32 = 650.0;
/// Room kept beyond each pill end for the fillet, the orb and the grip (orb 37.7 + grip 43.2 +
/// hot radius 26.3 + 10 px hot padding ≈ 117).
pub const WIN_END_RESERVE: f32 = 120.0;

/// Folded rest pill, upright orientation: 10 deep × 79 long.
pub const REST_DEPTH: f32 = 10.0;
pub const REST_LEN: f32 = 79.0;
pub const REST_RADIUS: f32 = 6.0;
/// Wake zone: the rest rect extended this far into the screen.
pub const REST_WAKE: f32 = 34.0;
/// Open clip during the fold: pill + 40 px along the edge (so the fillets show) and 1 px across.
pub const FOLD_CLIP_ALONG: f32 = 40.0;
/// Adaptive probe strip: 4 px beyond the rest pill, 12 px deep, as long as the rest pill.
pub const PROBE_GAP: f32 = 4.0;
pub const PROBE_DEPTH: f32 = 12.0;

/// Settings orb: centred on the far fillet's circle centre.
pub const ORB_HOT_R: f32 = 28.5;
pub const ORB_DISC_D: f32 = 46.6;
/// Resting orb handle: a short arc in the pill's material, concentric with the far fillet and
/// centred in its pocket (a grabber hint; hovering turns it into the gear disc). It used to be
/// a full 90° quarter, 6.8 px thick, which read as a detached piece of the pill.
pub const ORB_ARC_R: f32 = 30.0;
pub const ORB_ARC_FILL_W: f32 = 4.0;
pub const ORB_ARC_RIM_W: f32 = 6.0;
pub const ORB_ARC_SWEEP: f32 = 34.0;
pub const ORB_GEAR: f32 = 21.0;
/// Grip: six dots, two along the edge × three across.
pub const GRIP_DOT_D: f32 = 6.1;
pub const GRIP_PITCH: f32 = 9.9;
pub const GRIP_OFFSET: f32 = 43.2;
pub const GRIP_HOT_R: f32 = 26.3;
pub const GRIP_SWELL: f32 = 0.3;

/// Hover card.
pub const CARD_W: f32 = 246.0;
pub const CARD_MARGIN: f32 = 8.0;
pub const TAIL_SPAN: f32 = 32.72;
pub const TAIL_INSET: f32 = 16.0;
/// Card max height: window − 16 (upright) / window − 150 (flat).
pub const CARD_MAX_UPRIGHT_SLACK: f32 = 16.0;
pub const CARD_MAX_FLAT_SLACK: f32 = 150.0;

/// Hot padding around every reported rect and cell hit padding.
pub const HOT_PAD: f32 = 10.0;
pub const CELL_HIT_PAD: f32 = 6.0;
pub const PILL_ENTER_PAD: f32 = 4.0;

/// Drag.
pub const DRAG_THRESHOLD: f32 = 4.0;
pub const DRAG_EDGE_HYSTERESIS: f32 = 40.0;
pub const DRAG_MONITOR_SLACK: f32 = 150.0;

/// Reset card window.
pub const RESET_WIN_W: f32 = 280.0;
pub const RESET_WIN_H: f32 = 150.0;
pub const RESET_CARD_W: f32 = 246.0;
pub const RESET_CARD_H: f32 = 128.0;
pub const RESET_TAIL_LEN: f32 = 34.0;
pub const RESET_TAIL_SPAN: f32 = 32.0;
pub const RESET_TAIL_FLAT_LEN: f32 = 23.0;
pub const RESET_EDGE_GAP_HIDDEN: f32 = 8.0;
