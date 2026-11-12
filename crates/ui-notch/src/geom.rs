//! Pure notch geometry (visual-spec §0–§1, §5.2, §11).
//!
//! Everything is laid out once in a **canonical frame** — the right-edge layout seen as
//! `u` = distance along the edge (top → bottom on upright edges, left → right on flat ones) and
//! `v` = depth from the screen edge into the screen — and mapped to window coordinates by
//! [`Frame`]. Right maps `(u, v) → (W − v, u)`, left `(v, u)`, top `(u, v)`, bottom `(u, H − v)`,
//! which is the spec's "left = mirror, top/bottom = transposed" rule. All values are logical px of
//! the notch window, already multiplied by the notch size scale.

use crate::dims::*;
use codenotch_core::config::Edge;

pub type Pt = (f32, f32);

/// Axis-aligned rectangle (logical px).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }
    pub fn ltrb(l: f32, t: f32, r: f32, b: f32) -> Self {
        Self {
            x: l,
            y: t,
            w: (r - l).max(0.0),
            h: (b - t).max(0.0),
        }
    }
    pub fn right(&self) -> f32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
    #[cfg(test)]
    pub fn centre(&self) -> Pt {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }
    pub fn contains(&self, p: Pt) -> bool {
        p.0 >= self.x && p.0 <= self.right() && p.1 >= self.y && p.1 <= self.bottom()
    }
    pub fn inflate(&self, d: f32) -> Rect {
        Rect::ltrb(self.x - d, self.y - d, self.right() + d, self.bottom() + d)
    }
    pub fn union(&self, o: &Rect) -> Rect {
        Rect::ltrb(
            self.x.min(o.x),
            self.y.min(o.y),
            self.right().max(o.right()),
            self.bottom().max(o.bottom()),
        )
    }
    pub fn intersect(&self, o: &Rect) -> Option<Rect> {
        let (l, t) = (self.x.max(o.x), self.y.max(o.y));
        let (r, b) = (self.right().min(o.right()), self.bottom().min(o.bottom()));
        (r > l && b > t).then(|| Rect::ltrb(l, t, r, b))
    }
    pub fn lerp(&self, o: &Rect, t: f32) -> Rect {
        let f = |a: f32, b: f32| a + (b - a) * t;
        Rect::new(
            f(self.x, o.x),
            f(self.y, o.y),
            f(self.w, o.w),
            f(self.h, o.h),
        )
    }
}

/// Maps canonical `(u, v)` to window coordinates for one edge and window size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    pub edge: Edge,
    pub w: f32,
    pub h: f32,
}

impl Frame {
    /// Window extent along the edge.
    pub fn along(&self) -> f32 {
        if self.edge.is_upright() {
            self.h
        } else {
            self.w
        }
    }
    /// Window extent into the screen.
    pub fn depth(&self) -> f32 {
        if self.edge.is_upright() {
            self.w
        } else {
            self.h
        }
    }
    pub fn pt(&self, u: f32, v: f32) -> Pt {
        match self.edge {
            Edge::Right => (self.w - v, u),
            Edge::Left => (v, u),
            Edge::Top => (u, v),
            Edge::Bottom => (u, self.h - v),
        }
    }
    /// Window point → canonical `(u, v)`.
    pub fn inv(&self, p: Pt) -> Pt {
        let (x, y) = p;
        match self.edge {
            Edge::Right => (y, self.w - x),
            Edge::Left => (y, x),
            Edge::Top => (x, y),
            Edge::Bottom => (x, self.h - y),
        }
    }
    /// Canonical rect (`x` = u, `y` = v) → window rect.
    pub fn rect(&self, c: Rect) -> Rect {
        let a = self.pt(c.x, c.y);
        let b = self.pt(c.right(), c.bottom());
        Rect::ltrb(a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1))
    }
    /// Window delta for moving `d` toward the screen edge.
    pub fn toward_edge(&self, d: f32) -> Pt {
        match self.edge {
            Edge::Right => (d, 0.0),
            Edge::Left => (-d, 0.0),
            Edge::Top => (0.0, -d),
            Edge::Bottom => (0.0, d),
        }
    }
}

/// Along-edge insets of the notch window outside the work area (taskbar), logical px.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Insets {
    /// At the start of the along axis (top on upright edges, left on flat ones).
    pub lo: f32,
    pub hi: f32,
}

/// Where the hover card goes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CardPlace {
    /// Card box, window coordinates.
    pub card: Rect,
    /// Tail centre along the edge (canonical u).
    pub tail_u: f32,
}

/// Geometry of one notch window: pill, cells, fillets, orb, grip, rest pill, card.
#[derive(Debug, Clone, PartialEq)]
pub struct NotchLayout {
    pub frame: Frame,
    pub scale: f32,
    pub n: usize,
    /// Pill body, canonical.
    pub pill: Rect,
    /// Cell boxes, canonical (upright: 70 along × 44 deep; flat: 44 along × 70 deep).
    pub cells: Vec<Rect>,
}

impl NotchLayout {
    /// Pill depth into the screen: 70 upright, `24 + cellH + 1` flat.
    pub fn pill_depth(edge: Edge, s: f32) -> f32 {
        if edge.is_upright() {
            PILL_DEPTH * s
        } else {
            (2.0 * FLAT_PAD + CELL_H + BORDER) * s
        }
    }

    /// Pill length along the edge: `36 + n·cell + (n−1)·14 + 2` (cell = 70 upright, 44 flat).
    pub fn pill_len(edge: Edge, n: usize, s: f32) -> f32 {
        let n = n.max(1) as f32;
        let cell = if edge.is_upright() { CELL_H } else { RING_BOX };
        (2.0 * PILL_PAD_END + n * cell + (n - 1.0) * PILL_CELL_GAP + 2.0 * BORDER) * s
    }

    /// Notch window size (w, h), logical.
    pub fn window_size(edge: Edge, n: usize, s: f32) -> (f32, f32) {
        let along = (Self::pill_len(edge, n, 1.0) + 2.0 * WIN_END_RESERVE).max(WIN_MIN_ALONG) * s;
        let depth = if edge.is_upright() {
            WIN_UPRIGHT_DEPTH
        } else {
            WIN_FLAT_DEPTH
        } * s;
        if edge.is_upright() {
            (depth, along)
        } else {
            (along, depth)
        }
    }

    pub fn new(edge: Edge, n: usize, s: f32) -> Self {
        let (w, h) = Self::window_size(edge, n, s);
        let frame = Frame { edge, w, h };
        let len = Self::pill_len(edge, n, s);
        let depth = Self::pill_depth(edge, s);
        let p0 = ((frame.along() - len) / 2.0).round();
        let pill = Rect::new(p0, 0.0, len, depth);
        let upright = edge.is_upright();
        let cell_along = if upright { CELL_H } else { RING_BOX } * s;
        let cells = (0..n)
            .map(|i| {
                let u =
                    p0 + (BORDER + PILL_PAD_END) * s + i as f32 * (cell_along + PILL_CELL_GAP * s);
                if upright {
                    // centred in the 69 px content column (no border on the screen side)
                    let vc = (PILL_DEPTH - BORDER) / 2.0 * s;
                    Rect::new(u, vc - RING_BOX * s / 2.0, cell_along, RING_BOX * s)
                } else {
                    Rect::new(u, FLAT_PAD * s, cell_along, CELL_H * s)
                }
            })
            .collect();
        Self {
            frame,
            scale: s,
            n,
            pill,
            cells,
        }
    }

    pub fn upright(&self) -> bool {
        self.frame.edge.is_upright()
    }
    fn f(&self) -> f32 {
        FILLET_R * self.scale
    }

    /// Pill body in window coordinates.
    pub fn pill_rect(&self) -> Rect {
        self.frame.rect(self.pill)
    }
    /// Cell box in window coordinates (always 44 wide × 70 tall: ring on top, figure below).
    pub fn cell_rect(&self, i: usize) -> Option<Rect> {
        self.cells.get(i).map(|c| self.frame.rect(*c))
    }
    /// Ring centre of cell `i`, window coordinates.
    pub fn ring_centre(&self, i: usize) -> Option<Pt> {
        self.cell_rect(i)
            .map(|r| (r.x + r.w / 2.0, r.y + RING_BOX * self.scale / 2.0))
    }

    /// Fillet circle centres, canonical. The fillet square overlaps the pill's end border by
    /// 1 px so the 1 px rim at radius F−0.5 joins the pill outline without a seam.
    pub fn fillet_centre(&self, high: bool) -> Pt {
        let (f, b) = (self.f(), BORDER * self.scale);
        if high {
            (self.pill.right() - b + f, f)
        } else {
            (self.pill.x + b - f, f)
        }
    }

    /// Pill plus both fillet squares, window coordinates.
    pub fn shoulders_rect(&self) -> Rect {
        let lo = self.fillet_centre(false).0;
        let hi = self.fillet_centre(true).0;
        self.frame.rect(Rect::ltrb(lo, 0.0, hi, self.pill.h))
    }

    /// Concave fillet wedge (square minus the disc of `radius` around the fillet centre), as a
    /// polygon in window coordinates. Paint `radius = F − 1` in the edge colour, then `radius = F`
    /// in the pill colour: what remains visible of the first is the 1 px rim.
    pub fn fillet_polygon(&self, high: bool, radius: f32, segments: usize) -> Vec<Pt> {
        let f = self.f();
        let (cu, cv) = self.fillet_centre(high);
        let dir = if high { -1.0 } else { 1.0 }; // toward the pill along u
        let r = radius.clamp(0.0, f);
        let mut pts = Vec::with_capacity(segments + 6);
        pts.push((cu, 0.0)); // far screen corner
        pts.push((cu + dir * f, 0.0)); // screen corner at the pill end
        pts.push((cu + dir * f, f));
        pts.push((cu + dir * r, cv));
        let n = segments.max(2);
        for k in 1..=n {
            let phi = std::f32::consts::FRAC_PI_2 * k as f32 / n as f32;
            pts.push((cu + dir * r * phi.cos(), cv - r * phi.sin()));
        }
        pts.into_iter().map(|(u, v)| self.frame.pt(u, v)).collect()
    }

    /// Settings orb centre (the far fillet's circle centre), window coordinates.
    pub fn orb_centre(&self) -> Pt {
        let (u, v) = self.fillet_centre(true);
        self.frame.pt(u, v)
    }
    /// Grip centre: 43.2 px past the orb along the edge, away from the pill.
    pub fn grip_centre(&self) -> Pt {
        let (u, v) = self.fillet_centre(true);
        self.frame.pt(u + GRIP_OFFSET * self.scale, v)
    }
    /// The six grip dot centres (two along the edge × three across), window coordinates.
    /// `spread` (0..1) slides them out from the orb centre.
    pub fn grip_dots(&self, spread: f32) -> [Pt; 6] {
        let s = self.scale;
        let (ou, ov) = self.fillet_centre(true);
        let gu = ou + GRIP_OFFSET * s * spread;
        let half = GRIP_PITCH * s / 2.0;
        let mut out = [(0.0, 0.0); 6];
        let mut k = 0;
        for du in [-half, half] {
            for dv in [-GRIP_PITCH * s, 0.0, GRIP_PITCH * s] {
                out[k] = self.frame.pt(gu + du * spread, ov + dv * spread);
                k += 1;
            }
        }
        out
    }
    pub fn over_orb(&self, p: Pt) -> bool {
        dist(p, self.orb_centre()) <= ORB_HOT_R * self.scale
    }
    pub fn over_grip(&self, p: Pt) -> bool {
        dist(p, self.grip_centre()) <= GRIP_HOT_R * self.scale
    }
    /// Rotation of the orb's resting quarter arc, degrees clockwise from 12 o'clock: the arc
    /// sits in the fillet pocket, facing the corner where pill and screen meet.
    pub fn orb_arc_start(&self) -> f32 {
        // pocket direction in canonical: toward −u (pill) and −v (screen)
        let (cu, cv) = self.fillet_centre(true);
        let a = self.frame.pt(cu - 1.0, cv);
        let b = self.frame.pt(cu, cv - 1.0);
        let c = self.frame.pt(cu, cv);
        let ang = |p: Pt| {
            ((p.0 - c.0).atan2(-(p.1 - c.1)))
                .to_degrees()
                .rem_euclid(360.0)
        };
        let (a1, a2) = (ang(a), ang(b));
        // quarter between a1 and a2, clockwise from the smaller-clockwise start
        if (a2 - a1).rem_euclid(360.0) <= 180.0 {
            a1
        } else {
            a2
        }
    }

    /// Folded rest pill, window coordinates.
    pub fn rest_rect(&self) -> Rect {
        self.frame.rect(self.rest_c(0.0, REST_DEPTH))
    }
    /// Wake zone while folded.
    pub fn wake_rect(&self) -> Rect {
        self.frame.rect(self.rest_c(0.0, REST_DEPTH + REST_WAKE))
    }
    /// Adaptive pill probe strip.
    pub fn probe_rect(&self) -> Rect {
        self.frame
            .rect(self.rest_c(REST_DEPTH + PROBE_GAP, REST_DEPTH + PROBE_GAP + PROBE_DEPTH))
    }
    fn rest_c(&self, v0: f32, v1: f32) -> Rect {
        let s = self.scale;
        let mid = self.pill.x + self.pill.w / 2.0;
        Rect::ltrb(
            mid - REST_LEN * s / 2.0,
            v0 * s,
            mid + REST_LEN * s / 2.0,
            v1 * s,
        )
    }
    /// Clip rect of the folding pill at eased progress `t` (0 open, 1 folded).
    pub fn fold_clip(&self, t: f32) -> Rect {
        let s = self.scale;
        let open = Rect::ltrb(
            self.pill.x - FOLD_CLIP_ALONG * s,
            0.0,
            self.pill.right() + FOLD_CLIP_ALONG * s,
            self.pill.h + BORDER * s,
        );
        let rest = self.rest_c(0.0, REST_DEPTH);
        self.frame.rect(open.lerp(&rest, t.clamp(0.0, 1.0)))
    }

    /// Cell under `p` (cell box padded by 6 px).
    pub fn cell_at(&self, p: Pt) -> Option<usize> {
        let pad = CELL_HIT_PAD * self.scale;
        (0..self.n).find(|&i| {
            self.cell_rect(i)
                .is_some_and(|r| r.inflate(pad).contains(p))
        })
    }
    /// `p` inside the pill (+4 px).
    pub fn in_pill(&self, p: Pt) -> bool {
        self.pill_rect()
            .inflate(PILL_ENTER_PAD * self.scale)
            .contains(p)
    }
    /// Cell whose centre is nearest along the edge.
    pub fn nearest_cell(&self, p: Pt) -> Option<usize> {
        let (u, _) = self.frame.inv(p);
        (0..self.n).min_by(|&a, &b| {
            let da = (self.cells[a].x + self.cells[a].w / 2.0 - u).abs();
            let db = (self.cells[b].x + self.cells[b].w / 2.0 - u).abs();
            da.total_cmp(&db)
        })
    }

    /// Card size limits (width, max height), logical.
    pub fn card_limits(&self) -> (f32, f32) {
        let s = self.scale;
        let w = CARD_W * s;
        let max_h = if self.upright() {
            self.frame.along() - CARD_MAX_UPRIGHT_SLACK * s
        } else {
            self.frame.depth() - CARD_MAX_FLAT_SLACK * s
        };
        (w, max_h.max(40.0 * s))
    }

    /// Place a `w × h` card for cell `i` (§5.2): beside the pill, centred on the cell along the
    /// edge, kept 8 px (+ taskbar insets) inside the window; tail centred on the ring.
    pub fn place_card(&self, i: usize, w: f32, h: f32, insets: Insets) -> Option<CardPlace> {
        let cell = *self.cells.get(i)?;
        let s = self.scale;
        let (along_ext, depth_ext) = if self.upright() { (h, w) } else { (w, h) };
        let v0 = self.pill.h + FILLET_R * s;
        let centre = cell.x + cell.w / 2.0;
        let lo = CARD_MARGIN * s + insets.lo;
        let hi = self.frame.along() - insets.hi - along_ext - CARD_MARGIN * s;
        let u0 = if hi < lo {
            lo
        } else {
            (centre - along_ext / 2.0).clamp(lo, hi)
        };
        let ring_u = self.frame.inv(self.ring_centre(i)?).0;
        let half = TAIL_SPAN * s / 2.0;
        let (t_lo, t_hi) = (
            u0 + TAIL_INSET * s + half,
            u0 + along_ext - TAIL_INSET * s - half,
        );
        let tail_u = if t_hi < t_lo {
            u0 + along_ext / 2.0
        } else {
            ring_u.clamp(t_lo, t_hi)
        };
        Some(CardPlace {
            card: self.frame.rect(Rect::new(u0, v0, along_ext, depth_ext)),
            tail_u,
        })
    }

    /// Tail wedge (§5.2) centred at canonical `tail_u`, window coordinates. The base overlaps the
    /// card by 1 px so the two read as one shape.
    pub fn tail_polygon(&self, tail_u: f32, segments: usize) -> Vec<Pt> {
        let s = self.scale;
        let v0 = self.pill.h + FILLET_R * s;
        let u_top = tail_u - TAIL_SPAN * s / 2.0;
        // right-edge local path: x toward the screen, y along the edge
        let p = |x: f32, y: f32| (x * s, y * s);
        let mut local = Vec::with_capacity(2 * segments + 4);
        local.push((-1.0, 0.0));
        local.push(p(0.0, 0.0));
        local.extend(
            cubic(
                p(0.0, 0.0),
                p(0.0, 8.18),
                p(16.36, 12.43),
                p(TAIL_LEN, 16.36),
                segments,
            )
            .into_iter()
            .skip(1),
        );
        local.extend(
            cubic(
                p(TAIL_LEN, 16.36),
                p(16.36, 20.29),
                p(0.0, 24.54),
                p(0.0, TAIL_SPAN),
                segments,
            )
            .into_iter()
            .skip(1),
        );
        local.push((-1.0, TAIL_SPAN * s));
        local
            .into_iter()
            .map(|(x, y)| self.frame.pt(u_top + y, v0 - x))
            .collect()
    }
    pub fn tail_rect(&self, tail_u: f32) -> Rect {
        let s = self.scale;
        let v0 = self.pill.h + FILLET_R * s;
        self.frame.rect(Rect::new(
            tail_u - TAIL_SPAN * s / 2.0,
            v0 - TAIL_LEN * s,
            TAIL_SPAN * s,
            TAIL_LEN * s + 1.0,
        ))
    }

    /// Hot rects of the open notch (pill with fillets, orb, grip when out) — before padding.
    pub fn open_hot_rects(&self, grip_out: bool) -> Vec<Rect> {
        let s = self.scale;
        let mut v = vec![
            self.shoulders_rect(),
            square(self.orb_centre(), ORB_HOT_R * s),
        ];
        if grip_out {
            v.push(square(self.grip_centre(), GRIP_HOT_R * s));
        }
        v
    }
    pub fn bounds(&self) -> Rect {
        Rect::new(0.0, 0.0, self.frame.w, self.frame.h)
    }
}

/// Square of half-side `r` around `c`.
pub fn square(c: Pt, r: f32) -> Rect {
    Rect::ltrb(c.0 - r, c.1 - r, c.0 + r, c.1 + r)
}

pub fn dist(a: Pt, b: Pt) -> f32 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}

/// The window's hit region (§0): every hot rect padded by `pad`; with two or more rects their
/// bounding box counts too. Clipped to `bounds`.
pub fn hot_region(rects: &[Rect], pad: f32, bounds: Rect) -> Option<Rect> {
    let mut it = rects.iter().map(|r| r.inflate(pad));
    let first = it.next()?;
    it.fold(first, |acc, r| acc.union(&r)).intersect(&bounds)
}

/// A wedge (tail) polygon grown outward from the middle of its base — the segment between its
/// first and last points — so its farthest point moves out by `by` px: the hairline drawn under
/// a tail, flush with the card's own hairline.
pub fn grow_from_base(poly: &[Pt], by: f32) -> Vec<Pt> {
    let (Some(a), Some(b)) = (poly.first(), poly.last()) else {
        return Vec::new();
    };
    let c = ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0);
    let far = poly.iter().map(|p| dist(*p, c)).fold(0.0f32, f32::max);
    if far <= 0.0 {
        return poly.to_vec();
    }
    let k = 1.0 + by / far;
    poly.iter()
        .map(|p| (c.0 + (p.0 - c.0) * k, c.1 + (p.1 - c.1) * k))
        .collect()
}

/// Flatten a cubic Bézier into `segments + 1` points (first and last included).
pub fn cubic(p0: Pt, p1: Pt, p2: Pt, p3: Pt, segments: usize) -> Vec<Pt> {
    let n = segments.max(1);
    (0..=n)
        .map(|k| {
            let t = k as f32 / n as f32;
            let r = 1.0 - t;
            let (a, b, c, d) = (r * r * r, 3.0 * r * r * t, 3.0 * r * t * t, t * t * t);
            (
                a * p0.0 + b * p1.0 + c * p2.0 + d * p3.0,
                a * p0.1 + b * p1.1 + c * p2.1 + d * p3.1,
            )
        })
        .collect()
}

/// Gear outline (`teeth` trapezoid teeth) as a polygon around `c`, plus its axle hole as a
/// second polygon. Fill with the even-odd rule.
pub fn gear(c: Pt, outer: f32, teeth: usize, rotation_deg: f32) -> (Vec<Pt>, Vec<Pt>) {
    let root = outer * 0.74;
    let hole = outer * 0.34;
    let n = teeth.max(3);
    let step = 360.0 / n as f32;
    let mut pts = Vec::with_capacity(n * 4);
    let at = |r: f32, deg: f32| {
        let a = (deg + rotation_deg).to_radians();
        (c.0 + r * a.sin(), c.1 - r * a.cos())
    };
    for k in 0..n {
        let base = k as f32 * step;
        pts.push(at(root, base - step * 0.30));
        pts.push(at(outer, base - step * 0.17));
        pts.push(at(outer, base + step * 0.17));
        pts.push(at(root, base + step * 0.30));
    }
    let holes = (0..24).map(|k| at(hole, k as f32 * 15.0)).collect();
    (pts, holes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    #[test]
    fn pill_sizes_match_spec() {
        // upright: 36 + n·70 + (n−1)·14 + 2
        assert!(close(NotchLayout::pill_len(Edge::Right, 1, 1.0), 108.0));
        assert!(close(
            NotchLayout::pill_len(Edge::Right, 3, 1.0),
            36.0 + 210.0 + 28.0 + 2.0
        ));
        // flat: 36 + n·44 + (n−1)·14 + 2 and thickness 24 + 70 + 1
        assert!(close(
            NotchLayout::pill_len(Edge::Top, 2, 1.0),
            36.0 + 88.0 + 14.0 + 2.0
        ));
        assert!(close(NotchLayout::pill_depth(Edge::Top, 1.0), 95.0));
        assert!(close(NotchLayout::pill_depth(Edge::Left, 1.25), 87.5));
        // window: 360 × ≥650 upright, 650 deep flat, scaled
        assert_eq!(
            NotchLayout::window_size(Edge::Right, 2, 1.0),
            (360.0, 650.0)
        );
        assert_eq!(
            NotchLayout::window_size(Edge::Bottom, 2, 0.8),
            (520.0, 520.0)
        );
        let (_, h) = NotchLayout::window_size(Edge::Right, 9, 1.0);
        assert!(h >= NotchLayout::pill_len(Edge::Right, 9, 1.0) + 2.0 * 117.0);
    }

    #[test]
    fn frame_maps_each_edge() {
        let f = Frame {
            edge: Edge::Right,
            w: 360.0,
            h: 650.0,
        };
        assert_eq!(f.pt(10.0, 0.0), (360.0, 10.0));
        assert_eq!(f.inv(f.pt(12.0, 34.0)), (12.0, 34.0));
        for edge in [Edge::Left, Edge::Right, Edge::Top, Edge::Bottom] {
            let f = Frame {
                edge,
                w: 400.0,
                h: 700.0,
            };
            let p = f.pt(123.0, 45.0);
            let back = f.inv(p);
            assert!(close(back.0, 123.0) && close(back.1, 45.0), "{edge:?}");
        }
        let top = Frame {
            edge: Edge::Top,
            w: 650.0,
            h: 650.0,
        };
        assert_eq!(
            top.rect(Rect::new(10.0, 0.0, 20.0, 5.0)),
            Rect::new(10.0, 0.0, 20.0, 5.0)
        );
        let bottom = Frame {
            edge: Edge::Bottom,
            w: 650.0,
            h: 650.0,
        };
        assert_eq!(
            bottom.rect(Rect::new(10.0, 0.0, 20.0, 5.0)),
            Rect::new(10.0, 645.0, 20.0, 5.0)
        );
    }

    #[test]
    fn right_edge_pill_and_cells() {
        let l = NotchLayout::new(Edge::Right, 2, 1.0);
        let pill = l.pill_rect();
        assert!(close(pill.right(), 360.0) && close(pill.w, 70.0));
        assert!(close(pill.y + pill.h / 2.0, 325.0));
        // first cell starts 18 px below the inner top border; centre 34.5 px from the screen edge
        let c0 = l.cell_rect(0).unwrap();
        assert!(close(c0.y, pill.y + 19.0));
        assert!(close(360.0 - (c0.x + c0.w / 2.0), 34.5));
        assert!(close(c0.w, 44.0) && close(c0.h, 70.0));
        let c1 = l.cell_rect(1).unwrap();
        assert!(close(c1.y - c0.bottom(), 14.0));
        assert!(close(pill.bottom() - c1.bottom(), 19.0));
        let ring = l.ring_centre(0).unwrap();
        assert!(close(ring.1, c0.y + 22.0));
    }

    #[test]
    fn flat_cells_keep_ring_on_top() {
        for edge in [Edge::Top, Edge::Bottom] {
            let l = NotchLayout::new(edge, 3, 1.0);
            let c = l.cell_rect(1).unwrap();
            assert!(close(c.w, 44.0) && close(c.h, 70.0));
            let pill = l.pill_rect();
            assert!(close(pill.h, 95.0));
            // 12 px pad on the screen side and 12 + 1 px border on the inner side
            if edge == Edge::Top {
                assert!(close(c.y, 12.0));
            } else {
                assert!(close(l.frame.h - c.bottom(), 12.0));
            }
        }
    }

    #[test]
    fn fillet_meets_screen_and_pill_tangentially() {
        let l = NotchLayout::new(Edge::Right, 1, 1.0);
        let (cu, cv) = l.fillet_centre(false);
        assert!(close(cu + FILLET_R, l.pill.x + 1.0) && close(cv, FILLET_R));
        let poly = l.fillet_polygon(false, FILLET_R, 16);
        // all points inside the F × F square above the pill, flush with the screen edge
        for p in &poly {
            assert!(
                p.0 >= 360.0 - FILLET_R - 1e-3 && p.0 <= 360.0 + 1e-3,
                "{p:?}"
            );
            assert!(
                p.1 >= l.pill_rect().y - FILLET_R + 1.0 - 1e-3
                    && p.1 <= l.pill_rect().y + 1.0 + 1e-3
            );
        }
        // the arc ends at the screen edge (v = 0) at the far corner
        let last = *poly.last().unwrap();
        assert!(close(last.0, 360.0) && close(last.1, l.pill_rect().y + 1.0 - FILLET_R));
        // high fillet mirrors the low one
        let hi = l.fillet_polygon(true, FILLET_R, 16);
        assert!(hi
            .iter()
            .all(|p| p.1 >= l.pill_rect().bottom() - 1.0 - 1e-3));
    }

    #[test]
    fn orb_and_grip_positions() {
        let l = NotchLayout::new(Edge::Right, 1, 1.0);
        let orb = l.orb_centre();
        assert!(close(orb.0, 360.0 - FILLET_R));
        assert!(close(orb.1, l.pill_rect().bottom() - 1.0 + FILLET_R));
        let grip = l.grip_centre();
        assert!(close(grip.1 - orb.1, GRIP_OFFSET) && close(grip.0, orb.0));
        assert!(l.over_orb(orb) && !l.over_orb((orb.0, orb.1 + 40.0)));
        let dots = l.grip_dots(1.0);
        let along: Vec<f32> = dots.iter().map(|d| d.1).collect();
        assert!(
            close(along[0], grip.1 - GRIP_PITCH / 2.0)
                && close(along[5], grip.1 + GRIP_PITCH / 2.0)
        );
        // the whole grip fits in the window
        let (_, h) = NotchLayout::window_size(Edge::Right, 1, 1.0);
        assert!(grip.1 + GRIP_HOT_R + HOT_PAD < h);
        // the resting arc faces up-right (toward the pill and the screen) on the right edge
        assert!(close(l.orb_arc_start(), 0.0));
        let left = NotchLayout::new(Edge::Left, 1, 1.0);
        assert!(close(left.orb_arc_start(), 270.0));
    }

    #[test]
    fn rest_pill_and_wake_zone() {
        let l = NotchLayout::new(Edge::Right, 3, 1.0);
        let r = l.rest_rect();
        assert!(close(r.w, 10.0) && close(r.h, 79.0) && close(r.right(), 360.0));
        assert!(close(r.centre().1, l.pill_rect().centre().1));
        let w = l.wake_rect();
        assert!(close(w.w, 44.0));
        let top = NotchLayout::new(Edge::Top, 3, 1.0);
        let r = top.rest_rect();
        assert!(close(r.w, 79.0) && close(r.h, 10.0) && close(r.y, 0.0));
        let probe = top.probe_rect();
        assert!(close(probe.y, 14.0) && close(probe.h, 12.0));
        // fold clip goes from the open clip to the rest rect
        assert!(l.fold_clip(0.0).h > l.pill_rect().h);
        let (a, b) = (l.fold_clip(1.0), l.rest_rect());
        assert!(close(a.x, b.x) && close(a.y, b.y) && close(a.w, b.w) && close(a.h, b.h));
    }

    #[test]
    fn card_sits_108_7_from_the_edge_and_clamps() {
        let l = NotchLayout::new(Edge::Right, 3, 1.0);
        let place = l.place_card(1, 246.0, 200.0, Insets::default()).unwrap();
        assert!(close(360.0 - place.card.right(), 108.7));
        assert!(close(place.card.w, 246.0));
        let cell = l.cell_rect(1).unwrap();
        assert!(close(place.card.centre().1, cell.centre().1));
        // tail on the ring centre
        assert!(close(place.tail_u, l.ring_centre(1).unwrap().1));
        // a tall card is clamped 8 px inside the window plus insets
        let tall = l
            .place_card(0, 246.0, 600.0, Insets { lo: 30.0, hi: 0.0 })
            .unwrap();
        assert!(close(tall.card.y, 38.0));
        // tail stays 16 px inside the card
        let low = l
            .place_card(0, 246.0, 100.0, Insets { lo: 400.0, hi: 0.0 })
            .unwrap();
        assert!(low.tail_u >= low.card.y + 16.0 + TAIL_SPAN / 2.0 - 1e-3);
        // left edge mirrors; flat edges open below / above
        let left = NotchLayout::new(Edge::Left, 1, 1.0);
        assert!(close(
            left.place_card(0, 246.0, 100.0, Insets::default())
                .unwrap()
                .card
                .x,
            108.7
        ));
        let top = NotchLayout::new(Edge::Top, 1, 1.0);
        let c = top
            .place_card(0, 246.0, 100.0, Insets::default())
            .unwrap()
            .card;
        assert!(close(c.y, 95.0 + 38.7) && close(c.w, 246.0));
        let bottom = NotchLayout::new(Edge::Bottom, 1, 1.0);
        let c = bottom
            .place_card(0, 246.0, 100.0, Insets::default())
            .unwrap()
            .card;
        assert!(close(bottom.frame.h - c.bottom(), 95.0 + 38.7));
    }

    #[test]
    fn tail_tip_stops_10_5_short_of_the_pill() {
        let l = NotchLayout::new(Edge::Right, 1, 1.0);
        let tail = l.tail_polygon(300.0, 12);
        let tip = tail.iter().map(|p| p.0).fold(f32::MIN, f32::max);
        assert!(close(tip, 360.0 - 70.0 - 10.5));
        let tip_pt = tail.iter().find(|p| close(p.0, tip)).unwrap();
        assert!(close(tip_pt.1, 300.0 - TAIL_SPAN / 2.0 + 16.36));
        let top = NotchLayout::new(Edge::Top, 1, 1.0);
        let tip_y = top
            .tail_polygon(300.0, 12)
            .iter()
            .map(|p| p.1)
            .fold(f32::MAX, f32::min);
        assert!(close(tip_y, 95.0 + 10.5));
    }

    #[test]
    fn hit_testing() {
        let l = NotchLayout::new(Edge::Right, 2, 1.0);
        let c1 = l.cell_rect(1).unwrap();
        assert_eq!(l.cell_at(c1.centre()), Some(1));
        assert_eq!(l.cell_at((c1.x - 5.0, c1.y)), Some(1)); // within 6 px padding
        assert_eq!(l.cell_at((10.0, 10.0)), None);
        assert_eq!(l.nearest_cell((340.0, c1.bottom() + 3.0)), Some(1));
        assert!(l.in_pill((360.0 - 72.0, l.pill_rect().y + 5.0)));
        let region = hot_region(&l.open_hot_rects(false), HOT_PAD, l.bounds()).unwrap();
        assert!(close(region.right(), 360.0));
        assert!(region.contains(l.orb_centre()));
        assert!(!region.contains((10.0, 10.0)));
        assert!(hot_region(&[], 10.0, l.bounds()).is_none());
    }

    #[test]
    fn gear_and_cubic() {
        let (outline, hole) = gear((0.0, 0.0), 10.0, 8, 0.0);
        assert_eq!(outline.len(), 32);
        assert!(outline.iter().all(|p| dist(*p, (0.0, 0.0)) <= 10.0 + 1e-3));
        assert!(hole.iter().all(|p| close(dist(*p, (0.0, 0.0)), 3.4)));
        let c = cubic((0.0, 0.0), (0.0, 1.0), (1.0, 1.0), (1.0, 0.0), 4);
        assert_eq!(c.len(), 5);
        assert_eq!(c[0], (0.0, 0.0));
        assert_eq!(c[4], (1.0, 0.0));
    }

    #[test]
    fn grown_tail_covers_the_tail() {
        let tail = [(0.0, 0.0), (10.0, 5.0), (0.0, 10.0)];
        let g = grow_from_base(&tail, 1.5);
        assert_eq!(g.len(), 3);
        // base midpoint fixed, tip pushed out by 1.5 px
        assert!((g[1].0 - 11.5).abs() < 1e-4 && (g[1].1 - 5.0).abs() < 1e-4);
        assert!(g[0].1 < 0.0 && g[2].1 > 10.0);
        assert!(grow_from_base(&[], 1.0).is_empty());
    }
}
