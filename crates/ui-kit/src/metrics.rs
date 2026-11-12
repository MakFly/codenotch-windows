//! Geometry from visual-spec §0–§5, in logical px at Medium size (multiply by
//! `NotchSize::scale()`). Ring geometry is defined in a 56-unit box scaled to [`RING_BOX`].

/// Ring box side (px). 56 units map onto this.
pub const RING_BOX: f32 = 44.0;
/// px per ring unit (44/56).
pub const RING_K: f32 = RING_BOX / 56.0;

/// Ring geometry in 56-unit box units (multiply by [`RING_K`] × scale).
pub mod ring_units {
    pub const HOLE_R: f32 = 22.0;
    pub const MAIN_R: f32 = 25.0;
    pub const MAIN_W: f32 = 5.0;
    pub const WEEKLY_INSIDE_R: f32 = 16.0;
    pub const WEEKLY_OUTSIDE_R: f32 = 31.0;
    pub const WEEKLY_W: f32 = 2.4;
    /// Dashed weekly ring: 4 units on, 2 off.
    pub const WEEKLY_DASH: (f32, f32) = (4.0, 2.0);
    pub const ACTIVITY_R: f32 = 19.0;
    pub const ACTIVITY_W: f32 = 2.5;
    /// Working arc length as a fraction of the circle.
    pub const ACTIVITY_FRACTION: f32 = 0.28;
    pub const GLYPH: f32 = 26.0;
}

/// Opacities.
pub const STALE_OPACITY: f32 = 0.55;
pub const WEEKLY_TRACK_OPACITY: f32 = 0.7;
pub const WEEKLY_ARC_OPACITY: f32 = 0.85;

/// Pill (upright edge).
pub const PILL_DEPTH: f32 = 70.0;
pub const PILL_PAD_END: f32 = 18.0;
pub const PILL_CELL_GAP: f32 = 14.0;
pub const PILL_RADIUS: f32 = 20.0;
pub const FILLET_R: f32 = 38.7;
pub const CELL_TEXT_GAP: f32 = 6.0;
/// Percent figure under each ring (bold, in a 20 px line).
pub const PERCENT_FONT: f32 = 16.0;
/// Approximate cell height (ring + gap + one 15 px line).
pub const CELL_H: f32 = 70.0;
/// Folded rest pill (upright): 10 × 79, radius 6.
pub const REST_W: f32 = 10.0;
pub const REST_H: f32 = 79.0;
pub const REST_RADIUS: f32 = 6.0;
pub const REST_WAKE_DEPTH: f32 = 34.0;

/// Upright pill length for `n` cells: `36 + n·cellH + (n−1)·14 + 2`.
pub fn pill_length(n: usize, cell_h: f32) -> f32 {
    let n = n.max(1) as f32;
    2.0 * PILL_PAD_END + n * cell_h + (n - 1.0) * PILL_CELL_GAP + 2.0
}

/// Hover card.
pub const CARD_MAX_W: f32 = 246.0;
pub const CARD_RADIUS: f32 = 16.0;
pub const CARD_PAD: f32 = 16.0;
/// Card progress bar: fully rounded, on the `track` grey.
pub const CARD_BAR_H: f32 = 6.0;
pub const CARD_MAX_SESSIONS: usize = 5;
pub const TAIL_LEN: f32 = 28.2;
pub const TAIL_SPAN: f32 = 32.7;
pub const TAIL_GAP: f32 = 10.5;
/// Card edge distance from the screen edge (70 + 38.7).
pub const CARD_FROM_EDGE: f32 = PILL_DEPTH + TAIL_GAP + TAIL_LEN;

/// Timings (ms).
pub const CARD_CLOSE_GRACE_MS: u64 = 250;
pub const FOLD_DELAY_MS: u64 = 450;
pub const WORKING_SPIN_MS: u64 = 1200;
pub const WAITING_PULSE_MS: u64 = 1100;
pub const REFRESH_SPIN_MS: u64 = 950;

/// Reset card window (logical px) and display time.
pub const RESET_CARD_W: f32 = 280.0;
pub const RESET_CARD_H: f32 = 150.0;
pub const RESET_CARD_SHOW_MS: u64 = 6000;

/// Default settings window size (logical px), shrunk to the work area of smaller screens
/// ([`SETTINGS_MARGIN`] on every side) and centered.
pub const SETTINGS_W: f32 = 1120.0;
pub const SETTINGS_H: f32 = 900.0;
/// Free space kept around the default settings window inside the work area (logical px).
pub const SETTINGS_MARGIN: f32 = 24.0;
/// Minimum size of the (resizable) settings window.
pub const SETTINGS_MIN_W: f32 = 800.0;
pub const SETTINGS_MIN_H: f32 = 560.0;
/// Expanded navigation pane width (shadcn `--sidebar-width`, 16rem).
pub const SETTINGS_SIDEBAR_W: f32 = 256.0;
/// Collapsed navigation rail: icons only (shadcn `--sidebar-width-icon`, 3rem).
pub const SETTINGS_SIDEBAR_RAIL_W: f32 = 48.0;
/// Margin between the window edges / the sidebar and the inset content panel (shadcn `m-2`).
pub const SETTINGS_INSET_MARGIN: f32 = 8.0;
/// Corner radius of the inset content panel (`rounded-xl`).
pub const SETTINGS_INSET_RADIUS: f32 = 12.0;
/// Header bar of the content panel (page title, sidebar toggle, caption buttons; shadcn `h-12`).
pub const SETTINGS_HEADER_H: f32 = 48.0;
/// Window width under which the sidebar collapses to the rail (the saved preference is kept).
pub const SETTINGS_AUTO_COLLAPSE_W: f32 = 900.0;
/// Sidebar collapse / expand animation (ease-out; instant with reduced motion).
pub const SETTINGS_SIDEBAR_ANIM_MS: u64 = 200;
/// Caption button width.
pub const SETTINGS_CAPTION_W: f32 = 46.0;
/// Padding of the page body (shadcn `p-6`), [`SETTINGS_CONTENT_PAD_NARROW`] in a narrow window.
pub const SETTINGS_CONTENT_PAD: f32 = 24.0;
pub const SETTINGS_CONTENT_PAD_NARROW: f32 = 16.0;
/// Content max width (Settings app: 1000).
pub const SETTINGS_CONTENT_MAX_W: f32 = 1000.0;
/// Content max width of the statistics page (a dashboard: room for 5 KPI cards and a full
/// year of heatmap).
pub const SETTINGS_STATS_MAX_W: f32 = 1120.0;

#[cfg(test)]
mod tests {
    #[test]
    fn pill_length_matches_spec() {
        assert_eq!(super::pill_length(1, 70.0), 36.0 + 70.0 + 2.0);
        assert_eq!(super::pill_length(3, 70.0), 36.0 + 210.0 + 28.0 + 2.0);
    }
}
