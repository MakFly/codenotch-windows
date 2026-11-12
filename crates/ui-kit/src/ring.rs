//! Circular progress rings painted with gpui paths.
//!
//! Angles are in **degrees, clockwise from 12 o'clock** (visual-spec §2.1). A fraction ≤ 0 draws
//! nothing (no zero-length dot); a tiny positive fraction with round caps draws a dot; a
//! fraction ≥ 1 draws a full annulus.
//!
//! Arcs are tessellated as filled polygons (non-zero rule) rather than lyon strokes so that
//! round caps work without depending on lyon directly. Chords are ≤ 1 px.
//!
//! ```ignore
//! // A 44 px ring, 61 % used, starting at 12 o'clock, with the spec's hole and an activity arc:
//! ring(RingSpec::notch(1.0).hole(theme.hole).main(0.61, theme.ample, theme.track)
//!      .working(spin_deg, theme.ink))
//! ```

use crate::metrics::{ring_units as u, RING_BOX, RING_K};
use codenotch_core::config::WeeklyRing;
use gpui::{
    canvas, point, px, Canvas, FillOptions, Hsla, PathBuilder, PathStyle, Pixels, Point, Styled,
    Window,
};
use std::f32::consts::PI;

/// Line cap of an arc.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Cap {
    #[default]
    Round,
    Butt,
}

/// One arc (optionally over a full-circle track), centred in the ring box.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ArcLayer {
    /// Centre-line radius in px.
    pub radius: f32,
    /// Stroke width in px.
    pub width: f32,
    /// Share of the circle, 0..=1.
    pub fraction: f32,
    pub color: Hsla,
    /// Start angle, degrees clockwise from 12 o'clock.
    pub start_deg: f32,
    pub cap: Cap,
    /// Dashes along the centre line, px (on, off). Dashes use butt caps.
    pub dash: Option<(f32, f32)>,
    /// Full-circle track painted under the arc.
    pub track: Option<Hsla>,
    /// Dash the track as well.
    pub track_dashed: bool,
}

impl ArcLayer {
    pub fn new(radius: f32, width: f32, fraction: f32, color: Hsla) -> Self {
        Self {
            radius,
            width,
            fraction,
            color,
            start_deg: 0.0,
            cap: Cap::Round,
            dash: None,
            track: None,
            track_dashed: false,
        }
    }
}

/// Everything [`ring()`] paints.
#[derive(Debug, Clone, PartialEq)]
pub struct RingSpec {
    /// Side of the square ring box, px. The element has this size; outer rings may overflow it.
    pub size: f32,
    /// Filled inner disc: (radius px, colour).
    pub hole: Option<(f32, Hsla)>,
    /// Painted in order (first = bottom).
    pub layers: Vec<ArcLayer>,
}

impl RingSpec {
    /// Simple ring: `diameter` box, `stroke` width, `fraction` of `color` over a `track`.
    pub fn new(diameter: f32, stroke: f32, fraction: f32, track: Hsla, color: Hsla) -> Self {
        let mut layer = ArcLayer::new((diameter - stroke) / 2.0, stroke, fraction, color);
        layer.track = Some(track);
        Self {
            size: diameter,
            hole: None,
            layers: vec![layer],
        }
    }

    /// Empty ring box using the notch geometry (56-unit box → 44 px × `scale`).
    pub fn notch(scale: f32) -> Self {
        Self {
            size: RING_BOX * scale,
            hole: None,
            layers: Vec::new(),
        }
    }

    /// px per unit for the notch geometry at this box size.
    fn k(&self) -> f32 {
        self.size / RING_BOX * RING_K
    }

    /// Start angle of the first layer (degrees clockwise from 12 o'clock).
    pub fn start_angle(mut self, deg: f32) -> Self {
        if let Some(l) = self.layers.first_mut() {
            l.start_deg = deg;
        }
        self
    }

    /// Inner disc of the notch geometry (r 22 units, `--hole`).
    pub fn hole(mut self, color: Hsla) -> Self {
        self.hole = Some((u::HOLE_R * self.k(), color));
        self
    }

    /// Inner disc filling up to the inner edge of the first layer (for [`RingSpec::new`] rings).
    pub fn hole_inside(mut self, color: Hsla) -> Self {
        let r = self
            .layers
            .first()
            .map(|l| l.radius - l.width / 2.0)
            .unwrap_or(self.size / 2.0);
        self.hole = Some((r, color));
        self
    }

    pub fn layer(mut self, layer: ArcLayer) -> Self {
        self.layers.push(layer);
        self
    }

    /// Notch main ring: r 25, width 5 units, round caps, over `track`.
    pub fn main(self, fraction: f32, color: Hsla, track: Hsla) -> Self {
        let k = self.k();
        let mut l = ArcLayer::new(u::MAIN_R * k, u::MAIN_W * k, fraction, color);
        l.track = Some(track);
        self.layer(l)
    }

    /// Notch weekly ring (inside r 16 / outside r 31, width 2.4 units). `Off` adds nothing.
    /// Pass colours already faded (track ×0.7, arc ×0.85, see `metrics`).
    pub fn weekly(
        self,
        placement: WeeklyRing,
        fraction: f32,
        color: Hsla,
        track: Hsla,
        dashed: bool,
    ) -> Self {
        let k = self.k();
        let r = match placement {
            WeeklyRing::Off => return self,
            WeeklyRing::Inside => u::WEEKLY_INSIDE_R,
            WeeklyRing::Outside => u::WEEKLY_OUTSIDE_R,
        };
        let mut l = ArcLayer::new(r * k, u::WEEKLY_W * k, fraction, color);
        l.track = Some(track);
        if dashed {
            l.dash = Some((u::WEEKLY_DASH.0 * k, u::WEEKLY_DASH.1 * k));
            l.track_dashed = true;
            l.cap = Cap::Butt;
        }
        self.layer(l)
    }

    /// "Working" activity arc: 28 % of the circle at r 19 / width 2.5 units, starting at
    /// `rotation_deg` (animate it: one turn per 1.2 s in 12 steps).
    pub fn working(self, rotation_deg: f32, color: Hsla) -> Self {
        let k = self.k();
        let mut l = ArcLayer::new(
            u::ACTIVITY_R * k,
            u::ACTIVITY_W * k,
            u::ACTIVITY_FRACTION,
            color,
        );
        l.start_deg = rotation_deg;
        self.layer(l)
    }

    /// "Waiting on you" ring: full circle r 19 / width 2.5 units (animate the colour's opacity).
    pub fn waiting(self, color: Hsla) -> Self {
        let k = self.k();
        self.layer(ArcLayer::new(
            u::ACTIVITY_R * k,
            u::ACTIVITY_W * k,
            1.0,
            color,
        ))
    }
}

/// The ring element: a canvas of `spec.size` × `spec.size`. Style it further (margins, etc.).
pub fn ring(spec: RingSpec) -> Canvas<()> {
    let size = spec.size;
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let center = bounds.center();
            paint_ring(window, center, &spec);
        },
    )
    .w(px(size))
    .h(px(size))
    .flex_none()
}

/// Paint a ring spec centred on `center` (for custom canvases).
pub fn paint_ring(window: &mut Window, center: Point<Pixels>, spec: &RingSpec) {
    if let Some((r, color)) = spec.hole {
        paint_disc(window, center, r, color);
    }
    for l in &spec.layers {
        if let Some(track) = l.track {
            if l.track_dashed {
                if let Some(dash) = l.dash {
                    paint_dashed_arc(window, center, l.radius, l.width, 0.0, 360.0, dash, track);
                }
            } else {
                paint_arc(
                    window,
                    center,
                    l.radius,
                    l.width,
                    0.0,
                    360.0,
                    track,
                    Cap::Butt,
                );
            }
        }
        let fraction = l.fraction.clamp(0.0, 1.0);
        if fraction <= 0.0 || !fraction.is_finite() {
            continue;
        }
        let sweep = fraction * 360.0;
        match l.dash {
            Some(dash) => paint_dashed_arc(
                window,
                center,
                l.radius,
                l.width,
                l.start_deg,
                sweep,
                dash,
                l.color,
            ),
            None => paint_arc(
                window,
                center,
                l.radius,
                l.width,
                l.start_deg,
                sweep,
                l.color,
                l.cap,
            ),
        }
    }
}

/// Paint one arc. `sweep_deg` ≥ 360 paints a full annulus; ≤ 0 paints nothing.
#[allow(clippy::too_many_arguments)]
pub fn paint_arc(
    window: &mut Window,
    center: Point<Pixels>,
    radius: f32,
    width: f32,
    start_deg: f32,
    sweep_deg: f32,
    color: Hsla,
    cap: Cap,
) {
    if sweep_deg <= 0.0 || width <= 0.0 || radius <= 0.0 {
        return;
    }
    let mut builder = PathBuilder::fill().with_style(PathStyle::Fill(FillOptions::non_zero()));
    if sweep_deg >= 359.999 {
        let (outer, inner) = annulus(center, radius, width);
        builder.add_polygon(&outer, true);
        builder.add_polygon(&inner, true);
    } else {
        builder.add_polygon(
            &arc_polygon(center, radius, width, start_deg, sweep_deg, cap),
            true,
        );
    }
    if let Ok(path) = builder.build() {
        window.paint_path(path, color);
    }
}

/// Paint a filled disc.
pub fn paint_disc(window: &mut Window, center: Point<Pixels>, radius: f32, color: Hsla) {
    if radius <= 0.0 {
        return;
    }
    let mut builder = PathBuilder::fill().with_style(PathStyle::Fill(FillOptions::non_zero()));
    builder.add_polygon(&circle(center, radius, true), true);
    if let Ok(path) = builder.build() {
        window.paint_path(path, color);
    }
}

/// Dashed arc: butt-capped dashes of `dash.0` px every `dash.0 + dash.1` px along the centre
/// line, cut at the sweep end.
#[allow(clippy::too_many_arguments)]
pub fn paint_dashed_arc(
    window: &mut Window,
    center: Point<Pixels>,
    radius: f32,
    width: f32,
    start_deg: f32,
    sweep_deg: f32,
    dash: (f32, f32),
    color: Hsla,
) {
    for (a, s) in dash_segments(radius, start_deg, sweep_deg, dash) {
        paint_arc(window, center, radius, width, a, s, color, Cap::Butt);
    }
}

/// (start_deg, sweep_deg) of each dash.
pub fn dash_segments(
    radius: f32,
    start_deg: f32,
    sweep_deg: f32,
    dash: (f32, f32),
) -> Vec<(f32, f32)> {
    let (on, off) = dash;
    if radius <= 0.0 || on <= 0.0 || sweep_deg <= 0.0 {
        return Vec::new();
    }
    let len = radius * sweep_deg.min(360.0).to_radians();
    let to_deg = |px_len: f32| (px_len / radius).to_degrees();
    let mut out = Vec::new();
    let mut s = 0.0;
    while s < len - 1e-3 {
        let e = (s + on).min(len);
        out.push((start_deg + to_deg(s), to_deg(e - s)));
        s += on + off.max(0.0);
        if off <= 0.0 && on <= 0.0 {
            break;
        }
    }
    out
}

/// Point at `radius` and `deg` (clockwise from 12 o'clock).
pub fn polar(center: Point<Pixels>, radius: f32, deg: f32) -> Point<Pixels> {
    let a = deg.to_radians();
    point(
        center.x + px(radius * a.sin()),
        center.y - px(radius * a.cos()),
    )
}

fn segments(radius: f32, sweep_deg: f32) -> usize {
    ((radius * sweep_deg.to_radians()).ceil() as usize).clamp(4, 720)
}

fn circle(center: Point<Pixels>, radius: f32, clockwise: bool) -> Vec<Point<Pixels>> {
    let n = segments(radius, 360.0);
    (0..n)
        .map(|i| {
            let t = i as f32 / n as f32 * 360.0;
            polar(center, radius, if clockwise { t } else { -t })
        })
        .collect()
}

/// Outer (clockwise) and inner (counter-clockwise) outlines of a full annulus.
fn annulus(
    center: Point<Pixels>,
    radius: f32,
    width: f32,
) -> (Vec<Point<Pixels>>, Vec<Point<Pixels>>) {
    let ro = radius + width / 2.0;
    let ri = (radius - width / 2.0).max(0.0);
    (
        circle(center, ro, true),
        circle(center, ri.max(0.01), false),
    )
}

/// Closed outline of an arc stroke (outer edge → end cap → inner edge → start cap).
/// Pure geometry, used by [`paint_arc`] and unit tests.
pub fn arc_polygon(
    center: Point<Pixels>,
    radius: f32,
    width: f32,
    start_deg: f32,
    sweep_deg: f32,
    cap: Cap,
) -> Vec<Point<Pixels>> {
    let sweep = sweep_deg.clamp(0.0, 360.0);
    let hw = width / 2.0;
    let ro = radius + hw;
    let ri = (radius - hw).max(0.0);
    let n = segments(ro, sweep.max(1.0));
    let end = start_deg + sweep;
    let mut pts = Vec::with_capacity(2 * n + 40);
    for i in 0..=n {
        pts.push(polar(center, ro, start_deg + sweep * i as f32 / n as f32));
    }
    if cap == Cap::Round {
        pts.extend(cap_points(center, radius, hw, end, true));
    }
    for i in (0..=n).rev() {
        pts.push(polar(center, ri, start_deg + sweep * i as f32 / n as f32));
    }
    if cap == Cap::Round {
        pts.extend(cap_points(center, radius, hw, start_deg, false));
    }
    pts
}

/// Interior points of a semicircular cap at angle `deg` (excluding the two edge points).
/// `forward`: the end cap (outer → tip → inner); otherwise the start cap (inner → tip → outer).
fn cap_points(
    center: Point<Pixels>,
    radius: f32,
    hw: f32,
    deg: f32,
    forward: bool,
) -> Vec<Point<Pixels>> {
    let a = deg.to_radians();
    let (rx, ry) = (a.sin(), -a.cos()); // radial out
    let (tx, ty) = (a.cos(), a.sin()); // clockwise tangent
    let c = polar(center, radius, deg);
    let m = ((PI * hw).ceil() as usize).clamp(4, 64);
    (1..m)
        .map(|k| {
            let psi = PI * k as f32 / m as f32;
            let (dx, dy) = if forward {
                (
                    psi.cos() * rx + psi.sin() * tx,
                    psi.cos() * ry + psi.sin() * ty,
                )
            } else {
                (
                    -psi.cos() * rx - psi.sin() * tx,
                    -psi.cos() * ry - psi.sin() * ty,
                )
            };
            point(c.x + px(hw * dx), c.y + px(hw * dy))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dist(a: Point<Pixels>, b: Point<Pixels>) -> f32 {
        let (dx, dy) = (f32::from(a.x - b.x), f32::from(a.y - b.y));
        (dx * dx + dy * dy).sqrt()
    }

    #[test]
    fn polar_is_clockwise_from_twelve() {
        let c = point(px(50.), px(50.));
        let top = polar(c, 10.0, 0.0);
        assert!((f32::from(top.x) - 50.0).abs() < 1e-4 && (f32::from(top.y) - 40.0).abs() < 1e-4);
        let right = polar(c, 10.0, 90.0);
        assert!(
            (f32::from(right.x) - 60.0).abs() < 1e-4 && (f32::from(right.y) - 50.0).abs() < 1e-4
        );
    }

    #[test]
    fn butt_arc_points_lie_on_the_two_edges() {
        let c = point(px(0.), px(0.));
        let pts = arc_polygon(c, 20.0, 4.0, 0.0, 90.0, Cap::Butt);
        for p in &pts {
            let d = dist(*p, c);
            assert!(
                (d - 22.0).abs() < 1e-3 || (d - 18.0).abs() < 1e-3,
                "point off the stroke edges: {d}"
            );
        }
        // first point at 12 o'clock on the outer edge, last point back at 12 o'clock inner edge
        assert!((f32::from(pts[0].y) + 22.0).abs() < 1e-3);
        assert!((f32::from(pts.last().unwrap().y) + 18.0).abs() < 1e-3);
    }

    #[test]
    fn round_caps_stay_within_half_width_of_the_end_points() {
        let c = point(px(0.), px(0.));
        let pts = arc_polygon(c, 20.0, 4.0, 30.0, 120.0, Cap::Round);
        let end = polar(c, 20.0, 150.0);
        let start = polar(c, 20.0, 30.0);
        let caps: Vec<_> = pts
            .iter()
            .filter(|p| (dist(**p, c) - 22.0).abs() > 1e-3 && (dist(**p, c) - 18.0).abs() > 1e-3)
            .collect();
        assert!(!caps.is_empty());
        for p in caps {
            let d = dist(*p, end).min(dist(*p, start));
            assert!((d - 2.0).abs() < 1e-3, "cap point not on cap circle: {d}");
        }
    }

    #[test]
    fn notch_geometry_scales() {
        let theme = crate::Theme::dark();
        let spec = RingSpec::notch(1.0)
            .hole(theme.hole)
            .main(0.5, theme.ample, theme.track)
            .working(0.0, theme.ink);
        assert_eq!(spec.size, 44.0);
        assert!((spec.layers[0].radius - 25.0 * 44.0 / 56.0).abs() < 1e-4);
        assert!((spec.hole.unwrap().0 - 22.0 * 44.0 / 56.0).abs() < 1e-4);
        assert!((spec.layers[1].fraction - 0.28).abs() < 1e-6);
        let big = RingSpec::notch(1.25).main(0.5, theme.ample, theme.track);
        assert!((big.layers[0].width - 5.0 * 55.0 / 56.0).abs() < 1e-4);
        let off =
            RingSpec::notch(1.0).weekly(WeeklyRing::Off, 0.5, theme.ample, theme.track, false);
        assert!(off.layers.is_empty());
    }

    #[test]
    fn dashes_cover_the_sweep_without_overrun() {
        let segs = dash_segments(10.0, 0.0, 90.0, (4.0, 2.0));
        let len = 10.0 * 90f32.to_radians(); // ~15.7 px → dashes at 0,6,12 (last cut to 3.7)
        assert_eq!(segs.len(), 3);
        let last = segs.last().unwrap();
        assert!((last.0 + last.1 - 90.0).abs() < 1e-3);
        assert!((segs[0].1.to_radians() * 10.0 - 4.0).abs() < 1e-3);
        assert!(len > 15.0);
    }
}
