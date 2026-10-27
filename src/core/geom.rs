//! Window geometry in physical pixels: bubble sizing, default placement,
//! snapping, clamping, panel placement and fullscreen checks.
//!
//! Everything here is pure. The window layer collects monitors, work areas,
//! the taskbar and peer bubbles, and passes them in as plain [`Rect`]s.

use crate::usage::ProviderId;

/// Screen rectangle with exclusive right/bottom edges, like Win32 `RECT`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub l: i32,
    pub t: i32,
    pub r: i32,
    pub b: i32,
}

impl Rect {
    pub const fn new(l: i32, t: i32, r: i32, b: i32) -> Self {
        Self { l, t, r, b }
    }

    /// Rect from a top-left corner and a size.
    pub const fn from_xywh(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self {
            l: x,
            t: y,
            r: x + w,
            b: y + h,
        }
    }

    pub const fn w(&self) -> i32 {
        self.r - self.l
    }

    pub const fn h(&self) -> i32 {
        self.b - self.t
    }

    pub const fn is_empty(&self) -> bool {
        self.r <= self.l || self.b <= self.t
    }

    /// True when the two rects share a non-empty area.
    pub const fn intersects(&self, other: &Rect) -> bool {
        self.l < other.r && other.l < self.r && self.t < other.b && other.t < self.b
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

impl Point {
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }
}

/// The DPI the OS reports for 100% scaling.
pub const BASE_DPI: u32 = 96;

/// Scale a logical (96-DPI) pixel measurement to the given DPI.
pub fn scale(logical_px: i32, dpi: u32) -> i32 {
    ((logical_px as i64) * (dpi as i64) / (BASE_DPI as i64)) as i32
}

// ---------- Bubble size ----------

// Width clamps in logical pixels. Height is derived per width (see
// `bubble_height_logical`) — aspect tapers from 3:1 at the small end toward
// 2.6:1 at the large end so the bars look proportionally chunkier as the
// bubble grows.
pub const MIN_BUBBLE_SIZE: i32 = 140;
pub const MAX_BUBBLE_SIZE: i32 = 360;
pub const DEFAULT_BUBBLE_SIZE: i32 = 200;
pub const RESIZE_STEP_LOGICAL: i32 = 20;

const SNAP_ZONE_LOGICAL: i32 = 12;
const CORNER_SNAP_ZONE_LOGICAL: i32 = 32;
const CORNER_INSET_LOGICAL: i32 = 12;
const TASKBAR_GAP_LOGICAL: i32 = 4;
const PEER_ALIGN_TOLERANCE_LOGICAL: i32 = 8;

/// Raw (unscaled) gap used by the default position and the Codex stagger.
const STAGGER_GAP: i32 = 24;

/// Side of the square probed when validating a saved position. 140 px
/// matches `MIN_BUBBLE_SIZE`: a saved top-left a few px past the work-area
/// edge still passes the validator, but a position fully on a disconnected
/// monitor fails.
const POSITION_PROBE_PX: i32 = 140;

/// Gap between a bubble and its details panel, in physical px.
const PANEL_GAP_PX: i32 = 8;

const FULLSCREEN_EDGE_TOLERANCE_PX: i32 = 2;

/// (num, den) such that bubble_height = (width * den) / num. 3:1 below 200,
/// 2.8:1 below 280, 2.6:1 above — the bubble gets a touch taller as it
/// grows so the wider bars don't look anaemic.
pub fn aspect_at_width(w_logical: i32) -> (i32, i32) {
    if w_logical <= 200 {
        (3, 1)
    } else if w_logical <= 280 {
        (14, 5) // 2.8 : 1
    } else {
        (13, 5) // 2.6 : 1
    }
}

pub fn bubble_height_logical(width_logical: i32) -> i32 {
    let (num, den) = aspect_at_width(width_logical);
    ((width_logical * den) / num).max(20)
}

/// Physical `(width, height)` of a bubble at `size_logical` and `dpi`.
pub fn bubble_size_px(size_logical: i32, dpi: u32) -> (i32, i32) {
    (
        scale(size_logical, dpi),
        scale(bubble_height_logical(size_logical), dpi),
    )
}

pub fn corner_radius_px(height_px: i32) -> i32 {
    height_px / 2
}

/// Whether the local point `(x, y)` lies inside a `w`×`h` rounded rect with
/// corner radius `r`.
pub fn point_in_rounded_rect(x: i32, y: i32, w: i32, h: i32, r: i32) -> bool {
    if x < 0 || y < 0 || x >= w || y >= h {
        return false;
    }
    // The straight horizontal and vertical strips are always inside; only the
    // four corner squares need the circular falloff check.
    let in_x_strip = x >= r && x < w - r;
    let in_y_strip = y >= r && y < h - r;
    if in_x_strip || in_y_strip {
        return true;
    }
    let cx = if x < r { r } else { w - 1 - r };
    let cy = if y < r { r } else { h - 1 - r };
    let dx = x - cx;
    let dy = y - cy;
    dx * dx + dy * dy <= r * r
}

// ---------- Placement ----------

/// Bottom-right of the primary work area, with a 24-pixel gap from the
/// edges. The Codex bubble is staggered above the Claude one so both fit
/// when enabled.
pub fn default_position(
    work_area: Rect,
    width_px: i32,
    height_px: i32,
    model: ProviderId,
) -> Point {
    let stagger = match model {
        ProviderId::Claude => 0,
        ProviderId::ChatGpt => height_px + STAGGER_GAP,
    };
    let x = work_area.r - width_px - STAGGER_GAP;
    let y = work_area.b - height_px - STAGGER_GAP - stagger;
    Point::new(x, y)
}

/// Keep `rect` inside `work_area` and return the clamped top-left.
///
/// When both bubbles get clamped to the same bottom-right corner (e.g.
/// saved positions were on a disconnected monitor and the validator missed
/// them), the Codex bubble keeps the stagger `default_position` uses so they
/// don't visually stack.
pub fn clamp_into_work_area(rect: Rect, work_area: Rect, model: ProviderId) -> Point {
    let (w, h) = (rect.w(), rect.h());
    let nx = rect
        .l
        .clamp(work_area.l, (work_area.r - w).max(work_area.l));
    let mut ny = rect
        .t
        .clamp(work_area.t, (work_area.b - h).max(work_area.t));
    if model == ProviderId::ChatGpt && nx == work_area.r - w && ny == work_area.b - h {
        ny = (ny - h - STAGGER_GAP).max(work_area.t);
    }
    Point::new(nx, ny)
}

/// Monitor edge the taskbar is docked to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    Left,
    Top,
    Right,
    Bottom,
}

/// Everything the drag-end snap needs, in physical px.
#[derive(Clone, Debug)]
pub struct SnapInput {
    /// The dragged bubble's current outer rect.
    pub rect: Rect,
    /// Work area of the monitor nearest the bubble.
    pub work_area: Rect,
    /// DPI of the bubble's monitor.
    pub dpi: u32,
    /// Docked taskbar edge and rect, when the platform reports one.
    pub taskbar: Option<(Edge, Rect)>,
    /// Outer rects of the other bubbles, in `ProviderId::ALL` order.
    pub peers: Vec<Rect>,
}

/// Snap a dropped bubble: corner first; otherwise edge, taskbar and peer
/// alignment; then always clamp into the work area. Returns the new
/// top-left, which equals `input.rect`'s when nothing moves.
pub fn snap(input: &SnapInput) -> Point {
    let dpi = input.dpi;
    let edge_zone = scale(SNAP_ZONE_LOGICAL, dpi);
    let corner_zone = scale(CORNER_SNAP_ZONE_LOGICAL, dpi);
    let corner_inset = scale(CORNER_INSET_LOGICAL, dpi);
    let taskbar_gap = scale(TASKBAR_GAP_LOGICAL, dpi);
    let peer_tolerance = scale(PEER_ALIGN_TOLERANCE_LOGICAL, dpi);

    let wa = &input.work_area;
    let w = input.rect.w();
    let h = input.rect.h();
    let mut nx = input.rect.l;
    let mut ny = input.rect.t;

    // 1. Corner snap — if the bubble's nearest-corner distance is under the
    //    32-px corner zone, slam it into the corner with the 12-px inset.
    let snapped_to_corner = try_corner_snap(&mut nx, &mut ny, wa, w, h, corner_zone, corner_inset);

    if !snapped_to_corner {
        // 2. Edge snap — also handles taskbar adjacency when the taskbar
        //    steals from the work area on the same edge.
        snap_to_work_area_edges(&mut nx, &mut ny, wa, w, h, edge_zone);
        if let Some((edge, tb)) = &input.taskbar {
            let fit = Fit {
                wa,
                w,
                h,
                zone: edge_zone,
            };
            snap_alongside_taskbar(&mut nx, &mut ny, *edge, tb, &fit, taskbar_gap);
        }

        // 3. Peer vertical alignment — when another bubble is within ±8 px
        //    on Y, snap the dragged bubble to share its baseline.
        align_with_peer(&input.peers, &mut ny, peer_tolerance);
    }

    // Clamp into the work area in any case (so the bubble can't be lost off-screen).
    nx = nx.clamp(wa.l, (wa.r - w).max(wa.l));
    ny = ny.clamp(wa.t, (wa.b - h).max(wa.t));
    Point::new(nx, ny)
}

fn try_corner_snap(
    nx: &mut i32,
    ny: &mut i32,
    wa: &Rect,
    w: i32,
    h: i32,
    zone: i32,
    inset: i32,
) -> bool {
    // Distance from each work-area corner to the bubble's nearest corner.
    let tl = (*nx - wa.l).abs() + (*ny - wa.t).abs();
    let tr = (wa.r - (*nx + w)).abs() + (*ny - wa.t).abs();
    let bl = (*nx - wa.l).abs() + (wa.b - (*ny + h)).abs();
    let br = (wa.r - (*nx + w)).abs() + (wa.b - (*ny + h)).abs();
    let min = tl.min(tr).min(bl).min(br);
    if min > zone * 2 {
        return false;
    }
    if min == tl {
        *nx = wa.l + inset;
        *ny = wa.t + inset;
    } else if min == tr {
        *nx = wa.r - inset - w;
        *ny = wa.t + inset;
    } else if min == bl {
        *nx = wa.l + inset;
        *ny = wa.b - inset - h;
    } else {
        *nx = wa.r - inset - w;
        *ny = wa.b - inset - h;
    }
    true
}

fn snap_to_work_area_edges(nx: &mut i32, ny: &mut i32, wa: &Rect, w: i32, h: i32, zone: i32) {
    if (*nx - wa.l).abs() < zone {
        *nx = wa.l;
    } else if (wa.r - (*nx + w)).abs() < zone {
        *nx = wa.r - w;
    }
    if (*ny - wa.t).abs() < zone {
        *ny = wa.t;
    } else if (wa.b - (*ny + h)).abs() < zone {
        *ny = wa.b - h;
    }
}

/// Work area, bubble size and snap zone shared by the taskbar snap.
struct Fit<'a> {
    wa: &'a Rect,
    w: i32,
    h: i32,
    zone: i32,
}

fn snap_alongside_taskbar(
    nx: &mut i32,
    ny: &mut i32,
    edge: Edge,
    tb: &Rect,
    fit: &Fit<'_>,
    gap: i32,
) {
    let Fit { wa, w, h, zone } = *fit;
    if !taskbar_borders_work_area(edge, tb, wa) {
        return;
    }
    // Only snap on the taskbar's docked edge. The bubble docks against the
    // inner face of the taskbar with a 4-px gap so it visually leans on it.
    match edge {
        Edge::Bottom => {
            let target = tb.t - gap - h;
            if (*ny - target).abs() < zone {
                *ny = target.max(wa.t);
            }
        }
        Edge::Top => {
            let target = tb.b + gap;
            if (*ny - target).abs() < zone {
                *ny = target.min(wa.b - h);
            }
        }
        Edge::Left => {
            let target = tb.r + gap;
            if (*nx - target).abs() < zone {
                *nx = target.min(wa.r - w);
            }
        }
        Edge::Right => {
            let target = tb.l - gap - w;
            if (*nx - target).abs() < zone {
                *nx = target.max(wa.l);
            }
        }
    }
}

/// Whether the taskbar sits on this work area's `edge`: its inner face is
/// that work-area edge and it overlaps the work area along that edge. A
/// taskbar on another monitor (the platform reports only the primary's)
/// fails this, so it never pulls a bubble dropped on a secondary monitor.
fn taskbar_borders_work_area(edge: Edge, tb: &Rect, wa: &Rect) -> bool {
    let overlaps_x = tb.l < wa.r && tb.r > wa.l;
    let overlaps_y = tb.t < wa.b && tb.b > wa.t;
    match edge {
        Edge::Bottom => tb.t == wa.b && overlaps_x,
        Edge::Top => tb.b == wa.t && overlaps_x,
        Edge::Left => tb.r == wa.l && overlaps_y,
        Edge::Right => tb.l == wa.r && overlaps_y,
    }
}

fn align_with_peer(peers: &[Rect], ny: &mut i32, tolerance: i32) {
    if let Some(peer) = peers.iter().find(|p| (*ny - p.t).abs() <= tolerance) {
        *ny = peer.t;
    }
}

/// Place a `panel_w`×`panel_h` panel below `anchor` (the bubble's outer
/// rect), flipping above when it would clip, and keep it inside `bounds`
/// (the work area of the bubble's monitor, whose origin can be negative).
pub fn place_near(anchor: Rect, panel_size: (i32, i32), bounds: Rect) -> Point {
    let (panel_w, panel_h) = panel_size;
    let mut x = anchor.l;
    let mut y = anchor.b + PANEL_GAP_PX;
    if y + panel_h > bounds.b {
        y = anchor.t - panel_h - PANEL_GAP_PX;
    }
    if y < bounds.t {
        y = anchor.t.max(bounds.t);
    }
    if x + panel_w > bounds.r {
        x = bounds.r - panel_w - PANEL_GAP_PX;
    }
    if x < bounds.l {
        x = bounds.l + PANEL_GAP_PX;
    }
    Point::new(x, y)
}

/// Whether a saved top-left `(x, y)` still lands on a connected monitor:
/// a `140×140` probe there must overlap at least one of `monitors`.
pub fn position_on_any_monitor(x: i32, y: i32, monitors: &[Rect]) -> bool {
    let probe = Rect::from_xywh(x, y, POSITION_PROBE_PX, POSITION_PROBE_PX);
    monitors.iter().any(|m| probe.intersects(m))
}

// ---------- Fullscreen ----------

/// Whether `rect` covers the monitor `bounds`, allowing 2 px of slack per edge.
pub fn rect_covers_monitor(rect: &Rect, bounds: &Rect) -> bool {
    rect.l <= bounds.l + FULLSCREEN_EDGE_TOLERANCE_PX
        && rect.t <= bounds.t + FULLSCREEN_EDGE_TOLERANCE_PX
        && rect.r >= bounds.r - FULLSCREEN_EDGE_TOLERANCE_PX
        && rect.b >= bounds.b - FULLSCREEN_EDGE_TOLERANCE_PX
}

// Win32 window style bits (`WS_*`), as the plain values `GWL_STYLE` returns.
pub const WS_CHILD: u32 = 0x4000_0000;
pub const WS_POPUP: u32 = 0x8000_0000;
pub const WS_CAPTION: u32 = 0x00C0_0000;
pub const WS_THICKFRAME: u32 = 0x0004_0000;

/// A fullscreen candidate is a top-level popup, or a top-level window with
/// neither a caption nor a resize frame.
pub fn window_style_bits_allow_fullscreen(style: u32) -> bool {
    let has_child = style & WS_CHILD != 0;
    let has_popup = style & WS_POPUP != 0;
    let has_caption = style & WS_CAPTION != 0;
    let has_resize_frame = style & WS_THICKFRAME != 0;

    !has_child && (has_popup || (!has_caption && !has_resize_frame))
}

#[cfg(test)]
mod tests {
    use super::*;

    const WA: Rect = Rect::new(0, 0, 1920, 1040);

    fn input(rect: Rect) -> SnapInput {
        SnapInput {
            rect,
            work_area: WA,
            dpi: 96,
            taskbar: None,
            peers: Vec::new(),
        }
    }

    #[test]
    fn scale_follows_dpi() {
        assert_eq!(scale(200, 96), 200);
        assert_eq!(scale(200, 144), 300);
        assert_eq!(scale(12, 120), 15);
        assert_eq!(scale(66, 168), 115);
    }

    #[test]
    fn aspect_and_height_per_width() {
        for (w, aspect, h) in [
            (140, (3, 1), 46),
            (200, (3, 1), 66),
            (220, (14, 5), 78),
            (280, (14, 5), 100),
            (300, (13, 5), 115),
            (360, (13, 5), 138),
        ] {
            assert_eq!(aspect_at_width(w), aspect, "aspect at {w}");
            assert_eq!(bubble_height_logical(w), h, "height at {w}");
        }
        assert_eq!(bubble_size_px(200, 144), (300, 99));
    }

    #[test]
    fn rounded_rect_excludes_corners_only() {
        let (w, h) = (200, 66);
        let r = corner_radius_px(h);
        assert!(point_in_rounded_rect(100, 0, w, h, r));
        assert!(point_in_rounded_rect(33, 33, w, h, r));
        assert!(!point_in_rounded_rect(0, 0, w, h, r));
        assert!(!point_in_rounded_rect(w - 1, h - 1, w, h, r));
        assert!(point_in_rounded_rect(1, 33, w, h, r));
        assert!(!point_in_rounded_rect(0, 33, w, h, r));
        assert!(!point_in_rounded_rect(-1, 33, w, h, r));
        assert!(!point_in_rounded_rect(w, 33, w, h, r));
    }

    #[test]
    fn default_position_staggers_by_provider() {
        let (w, h) = (200, 66);
        assert_eq!(
            default_position(WA, w, h, ProviderId::Claude),
            Point::new(1920 - 200 - 24, 1040 - 66 - 24)
        );
        assert_eq!(
            default_position(WA, w, h, ProviderId::ChatGpt),
            Point::new(1696, 1040 - 66 - 24 - 90)
        );
    }

    #[test]
    fn clamp_pulls_offscreen_rect_into_work_area() {
        let r = Rect::from_xywh(-500, -30, 200, 66);
        assert_eq!(
            clamp_into_work_area(r, WA, ProviderId::Claude),
            Point::new(0, 0)
        );
        let inside = Rect::from_xywh(100, 100, 200, 66);
        assert_eq!(
            clamp_into_work_area(inside, WA, ProviderId::ChatGpt),
            Point::new(100, 100)
        );
    }

    #[test]
    fn clamp_lifts_codex_out_of_bottom_right_corner() {
        let r = Rect::from_xywh(5000, 5000, 200, 66);
        assert_eq!(
            clamp_into_work_area(r, WA, ProviderId::Claude),
            Point::new(1720, 974)
        );
        assert_eq!(
            clamp_into_work_area(r, WA, ProviderId::ChatGpt),
            Point::new(1720, 974 - 66 - 24)
        );
        // The lift never goes above the work area.
        let tiny = Rect::new(0, 0, 400, 100);
        assert_eq!(
            clamp_into_work_area(
                Rect::from_xywh(900, 900, 200, 66),
                tiny,
                ProviderId::ChatGpt
            ),
            Point::new(200, 0)
        );
    }

    #[test]
    fn corner_snap_insets_into_each_corner() {
        // Within 64 px (2 x 32) Manhattan distance of a corner.
        let tl = snap(&input(Rect::from_xywh(20, 30, 200, 66)));
        assert_eq!(tl, Point::new(12, 12));
        let br = snap(&input(Rect::from_xywh(1690, 940, 200, 66)));
        assert_eq!(br, Point::new(1920 - 12 - 200, 1040 - 12 - 66));
        let tr = snap(&input(Rect::from_xywh(1700, 10, 200, 66)));
        assert_eq!(tr, Point::new(1708, 12));
        let bl = snap(&input(Rect::from_xywh(10, 950, 200, 66)));
        assert_eq!(bl, Point::new(12, 962));
    }

    #[test]
    fn corner_snap_tie_order_is_tl_tr_bl_br() {
        // The work area leaves 40 px of slack on both axes, so the four
        // corner distances can be made to tie.
        let wa = Rect::new(0, 0, 240, 106);
        let at = |x, y| SnapInput {
            work_area: wa,
            ..input(Rect::from_xywh(x, y, 200, 66))
        };
        // tl = tr = 20 (bl = br = 60): top-left wins.
        assert_eq!(snap(&at(20, 0)), Point::new(12, 12));
        // tl = 50, tr = br = 30: top-right wins over bottom-right.
        assert_eq!(snap(&at(30, 20)), Point::new(28, 12));
        // tl = tr = 50, bl = br = 30: bottom-left wins over bottom-right.
        assert_eq!(snap(&at(20, 30)), Point::new(12, 28));
        // Only bottom-right is nearest.
        assert_eq!(snap(&at(30, 30)), Point::new(28, 28));
    }

    #[test]
    fn corner_zone_boundary_is_inclusive_at_twice_the_zone() {
        // tl distance exactly 64 snaps; 65 does not.
        assert_eq!(
            snap(&input(Rect::from_xywh(32, 32, 200, 66))),
            Point::new(12, 12)
        );
        let out = snap(&input(Rect::from_xywh(33, 32, 200, 66)));
        assert_ne!(out, Point::new(12, 12));
        // Not a corner: falls through to edge snap on the top edge (32 >= 12
        // so x stays; y=32 is outside the 12 px edge zone too).
        assert_eq!(out, Point::new(33, 32));
    }

    #[test]
    fn edge_zone_is_exclusive_at_twelve_px() {
        // 11 px from the left edge snaps flush; 12 px does not.
        assert_eq!(
            snap(&input(Rect::from_xywh(11, 500, 200, 66))),
            Point::new(0, 500)
        );
        assert_eq!(
            snap(&input(Rect::from_xywh(12, 500, 200, 66))),
            Point::new(12, 500)
        );
        // Right and bottom edges.
        assert_eq!(
            snap(&input(Rect::from_xywh(1709, 500, 200, 66))),
            Point::new(1720, 500)
        );
        assert_eq!(
            snap(&input(Rect::from_xywh(800, 965, 200, 66))),
            Point::new(800, 974)
        );
        assert_eq!(
            snap(&input(Rect::from_xywh(800, 5, 200, 66))),
            Point::new(800, 0)
        );
    }

    #[test]
    fn edge_zone_scales_with_dpi() {
        let mut i = input(Rect::from_xywh(17, 500, 300, 99));
        i.dpi = 144; // zone 18 px
        assert_eq!(snap(&i), Point::new(0, 500));
        i.rect = Rect::from_xywh(18, 500, 300, 99);
        assert_eq!(snap(&i), Point::new(18, 500));
    }

    #[test]
    fn taskbar_snap_on_each_edge() {
        // Monitor 1920x1080; the work area excludes a 40 px taskbar.
        let cases = [
            (
                Edge::Bottom,
                Rect::new(0, 1040, 1920, 1080),
                Rect::new(0, 0, 1920, 1040),
                Rect::from_xywh(800, 960, 200, 66),
                Point::new(800, 1040 - 4 - 66),
            ),
            (
                Edge::Top,
                Rect::new(0, 0, 1920, 40),
                Rect::new(0, 40, 1920, 1080),
                Rect::from_xywh(800, 52, 200, 66),
                Point::new(800, 44),
            ),
            (
                Edge::Left,
                Rect::new(0, 0, 40, 1080),
                Rect::new(40, 0, 1920, 1080),
                Rect::from_xywh(52, 500, 200, 66),
                Point::new(44, 500),
            ),
            (
                Edge::Right,
                Rect::new(1880, 0, 1920, 1080),
                Rect::new(0, 0, 1880, 1080),
                Rect::from_xywh(1666, 500, 200, 66),
                Point::new(1880 - 4 - 200, 500),
            ),
        ];
        for (edge, tb, wa, rect, expected) in cases {
            let i = SnapInput {
                rect,
                work_area: wa,
                dpi: 96,
                taskbar: Some((edge, tb)),
                peers: Vec::new(),
            };
            assert_eq!(snap(&i), expected, "taskbar {edge:?}");
        }
    }

    #[test]
    fn taskbar_snap_ignores_far_positions() {
        let i = SnapInput {
            rect: Rect::from_xywh(800, 900, 200, 66),
            work_area: WA,
            dpi: 96,
            taskbar: Some((Edge::Bottom, Rect::new(0, 1040, 1920, 1080))),
            peers: Vec::new(),
        };
        assert_eq!(snap(&i), Point::new(800, 900));
    }

    #[test]
    fn taskbar_of_another_monitor_never_snaps() {
        // The platform reports the primary's bottom taskbar; the bubble is
        // dropped on a secondary monitor right of the primary, 10 px above
        // where that taskbar's snap would put it and far from its own edges.
        let primary_taskbar = Rect::new(0, 1040, 1920, 1080);
        let rect = Rect::from_xywh(2800, 1040 - 4 - 66 - 10, 200, 66);
        for secondary_wa in [
            Rect::new(1920, 0, 3840, 1080),
            // Same bottom as the primary's work area, still no overlap.
            Rect::new(1920, 0, 3840, 1040),
        ] {
            let i = SnapInput {
                rect,
                work_area: secondary_wa,
                dpi: 96,
                taskbar: Some((Edge::Bottom, primary_taskbar)),
                peers: Vec::new(),
            };
            assert_eq!(snap(&i), Point::new(rect.l, rect.t), "{secondary_wa:?}");
        }
    }

    #[test]
    fn peer_alignment_takes_first_peer_within_tolerance() {
        let mut i = input(Rect::from_xywh(800, 500, 200, 66));
        i.peers = vec![
            Rect::from_xywh(100, 509, 200, 66),
            Rect::from_xywh(400, 506, 200, 66),
        ];
        // 9 px is outside the 8 px tolerance; 6 px is inside.
        assert_eq!(snap(&i), Point::new(800, 506));
        i.peers = vec![
            Rect::from_xywh(100, 508, 200, 66),
            Rect::from_xywh(400, 506, 200, 66),
        ];
        assert_eq!(snap(&i), Point::new(800, 508));
        i.peers.clear();
        assert_eq!(snap(&i), Point::new(800, 500));
    }

    #[test]
    fn peer_alignment_is_skipped_after_a_corner_snap() {
        let mut i = input(Rect::from_xywh(20, 20, 200, 66));
        i.peers = vec![Rect::from_xywh(900, 18, 200, 66)];
        assert_eq!(snap(&i), Point::new(12, 12));
    }

    #[test]
    fn final_clamp_keeps_peer_aligned_bubble_on_screen() {
        // The bottom edge snap puts y at 974; aligning to the peer at 980
        // would overflow the 1040 bottom, so the clamp wins.
        let mut i = input(Rect::from_xywh(800, 970, 200, 66));
        i.peers = vec![Rect::from_xywh(100, 980, 200, 30)];
        assert_eq!(snap(&i), Point::new(800, 974));
        // A rect larger than the work area pins to its top-left.
        let mut big = input(Rect::from_xywh(500, 500, 3000, 2000));
        big.work_area = Rect::new(100, 50, 1920, 1040);
        assert_eq!(snap(&big), Point::new(100, 50));
    }

    #[test]
    fn place_near_below_then_above() {
        let bounds = Rect::new(0, 0, 1920, 1080);
        let bubble = Rect::from_xywh(100, 100, 200, 66);
        assert_eq!(place_near(bubble, (280, 120), bounds), Point::new(100, 174));
        let low = Rect::from_xywh(100, 950, 200, 66);
        assert_eq!(place_near(low, (280, 120), bounds), Point::new(100, 822));
    }

    #[test]
    fn place_near_clamps_left_and_right() {
        let bounds = Rect::new(0, 0, 1920, 1080);
        let right = Rect::from_xywh(1800, 100, 200, 66);
        assert_eq!(
            place_near(right, (280, 120), bounds),
            Point::new(1920 - 280 - 8, 174)
        );
        let left = Rect::from_xywh(-50, 100, 200, 66);
        assert_eq!(place_near(left, (280, 120), bounds), Point::new(8, 174));
    }

    #[test]
    fn place_near_handles_negative_virtual_origin() {
        // A secondary monitor left of and above the primary.
        let bounds = Rect::new(-1920, -200, 1920, 1080);
        let bubble = Rect::from_xywh(-1900, -190, 200, 66);
        assert_eq!(
            place_near(bubble, (280, 120), bounds),
            Point::new(-1900, -116)
        );
        let edge = Rect::from_xywh(-2000, 1000, 200, 66);
        assert_eq!(
            place_near(edge, (280, 120), bounds),
            Point::new(-1912, 1000 - 120 - 8)
        );
        // Flipping above would leave the top too; it pins to the anchor top.
        let tall_bounds = Rect::new(-1920, -200, 1920, 100);
        let b = Rect::from_xywh(0, -190, 200, 66);
        assert_eq!(place_near(b, (280, 400), tall_bounds), Point::new(0, -190));
    }

    #[test]
    fn place_near_flips_above_on_the_shorter_monitor_of_a_mixed_layout() {
        // A 1920x1080 laptop (40 px taskbar) left of a top-aligned
        // 2560x1440 monitor. Near the laptop's bottom the panel flips above
        // within the laptop's work area, though the union of both monitors
        // would leave room below.
        let laptop_work = Rect::new(0, 0, 1920, 1040);
        let bubble = Rect::from_xywh(1500, 940, 200, 66);
        assert_eq!(
            place_near(bubble, (280, 120), laptop_work),
            Point::new(1500, 940 - 120 - 8)
        );
        let union = Rect::new(0, 0, 4480, 1440);
        assert_eq!(
            place_near(bubble, (280, 120), union),
            Point::new(1500, 940 + 66 + 8)
        );
    }

    #[test]
    fn place_near_flips_against_the_work_area_not_the_monitor() {
        // The panel fits below inside the 1080-tall monitor but not inside
        // its work area, which ends at the taskbar's top at 1040.
        let monitor = Rect::new(0, 0, 1920, 1080);
        let work = Rect::new(0, 0, 1920, 1040);
        let bubble = Rect::from_xywh(100, 880, 200, 66);
        assert_eq!(
            place_near(bubble, (280, 120), monitor),
            Point::new(100, 954)
        );
        assert_eq!(
            place_near(bubble, (280, 120), work),
            Point::new(100, 880 - 120 - 8)
        );
    }

    #[test]
    fn position_probe_against_monitors() {
        let monitors = [Rect::new(0, 0, 1920, 1080), Rect::new(1920, 0, 3840, 1080)];
        assert!(position_on_any_monitor(100, 100, &monitors));
        assert!(position_on_any_monitor(2000, 500, &monitors));
        // Partially off the left edge still overlaps.
        assert!(position_on_any_monitor(-100, 100, &monitors));
        // Fully past the left edge does not.
        assert!(!position_on_any_monitor(-140, 100, &monitors));
        assert!(!position_on_any_monitor(4000, 100, &monitors));
        assert!(!position_on_any_monitor(100, 100, &[]));
    }

    #[test]
    fn style_bits_allow_popup_or_borderless_windows_only() {
        assert!(window_style_bits_allow_fullscreen(WS_POPUP));
        assert!(window_style_bits_allow_fullscreen(0));
        assert!(!window_style_bits_allow_fullscreen(
            WS_CAPTION | WS_THICKFRAME
        ));
        assert!(!window_style_bits_allow_fullscreen(WS_CHILD | WS_POPUP));
    }

    #[test]
    fn rect_cover_check_allows_small_edge_differences() {
        let monitor = Rect::new(0, 0, 1920, 1080);
        let almost_exact = Rect::new(1, 2, 1919, 1078);
        let inset = Rect::new(8, 0, 1920, 1080);

        assert!(rect_covers_monitor(&almost_exact, &monitor));
        assert!(!rect_covers_monitor(&inset, &monitor));
    }
}
