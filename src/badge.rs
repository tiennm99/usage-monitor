//! Anti-aliased tray badge rendering.
//!
//! A filled disk plus a percentage-sweep ring, drawn with tiny-skia. The
//! badge is intentionally text-free: the floating bubble already shows the
//! exact percentage, the tray badge is a coarse colour-and-fill indicator.

use tiny_skia::{FillRule, LineCap, Paint, Path, PathBuilder, Pixmap, Stroke, Transform};

use crate::core::usage_color;
use crate::usage::ProviderId;

/// Width and height of the badge in pixels.
pub const BADGE_PX: u32 = 32;

/// Colour of the unused part of the ring.
const TRACK_RGB: [u8; 3] = [0x3a, 0x3a, 0x3a];

/// Render the badge for `kind` with `percent` of the ring filled, or only
/// the track when `percent` is `None`. Premultiplied RGBA.
pub fn render_pixmap(kind: ProviderId, percent: Option<f64>) -> Pixmap {
    let mut pixmap = Pixmap::new(BADGE_PX, BADGE_PX).expect("32x32 pixmap");
    pixmap.fill(tiny_skia::Color::TRANSPARENT);

    let c = BADGE_PX as f32 / 2.0;
    let outer = c - 1.0;
    let inner = outer * 0.62;
    let ring_radius = (inner + outer) / 2.0;
    let ring_width = outer - inner - 1.0;

    // Filled inner disk in the provider's base tint.
    let mut pb = PathBuilder::new();
    pb.push_circle(c, c, inner);
    if let Some(path) = pb.finish() {
        pixmap.fill_path(
            &path,
            &paint(base_color(kind)),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }

    // Track ring (the unused portion of the quota).
    let mut pb = PathBuilder::new();
    pb.push_circle(c, c, ring_radius);
    if let Some(path) = pb.finish() {
        let stroke = Stroke {
            width: ring_width,
            ..Stroke::default()
        };
        pixmap.stroke_path(
            &path,
            &paint(TRACK_RGB),
            &stroke,
            Transform::identity(),
            None,
        );
    }

    // Active sweep: the used share of the ring in the usage colour.
    if let Some(p) = percent {
        let sweep = (p.clamp(0.0, 100.0) / 100.0) as f32;
        if sweep > 0.0 {
            if let Some(path) = build_arc(c, c, ring_radius, sweep) {
                let stroke = Stroke {
                    width: ring_width,
                    line_cap: LineCap::Round,
                    ..Stroke::default()
                };
                pixmap.stroke_path(
                    &path,
                    &paint(usage_rgb(kind, p)),
                    &stroke,
                    Transform::identity(),
                    None,
                );
            }
        }
    }

    pixmap
}

/// The pixmap as straight (non-premultiplied) RGBA bytes, the format
/// `tauri::image::Image::new_owned` expects.
pub fn demultiplied_rgba(pixmap: &Pixmap) -> Vec<u8> {
    pixmap
        .pixels()
        .iter()
        .flat_map(|p| {
            let c = p.demultiply();
            [c.red(), c.green(), c.blue(), c.alpha()]
        })
        .collect()
}

fn paint(rgb: [u8; 3]) -> Paint<'static> {
    let mut paint = Paint::default();
    paint.set_color_rgba8(rgb[0], rgb[1], rgb[2], 255);
    paint.anti_alias = true;
    paint
}

/// An open polyline arc from 12 o'clock, clockwise. tiny-skia 0.11 has no
/// arc primitive, so the circumference is sampled.
fn build_arc(cx: f32, cy: f32, radius: f32, sweep_fraction: f32) -> Option<Path> {
    let segments = ((sweep_fraction * 64.0).ceil() as usize).max(1);
    let start = -std::f32::consts::FRAC_PI_2;
    let total = sweep_fraction * std::f32::consts::TAU;
    let mut pb = PathBuilder::new();
    for i in 0..=segments {
        let a = start + (i as f32 / segments as f32) * total;
        let (x, y) = (cx + a.cos() * radius, cy + a.sin() * radius);
        if i == 0 {
            pb.move_to(x, y);
        } else {
            pb.line_to(x, y);
        }
    }
    pb.finish()
}

fn base_color(kind: ProviderId) -> [u8; 3] {
    match kind {
        // A dark warm tint reads as Claude without copying its exact accent.
        ProviderId::Claude => [0x2a, 0x1f, 0x1c],
        // Cool dark slate for Codex.
        ProviderId::ChatGpt => [0x1a, 0x1f, 0x26],
    }
}

/// Sweep colour. The badge disk is always dark whatever the system theme,
/// so the dark palette keeps the ring readable (Codex stays white).
fn usage_rgb(kind: ProviderId, percent: f64) -> [u8; 3] {
    let c = usage_color::bar_fill_color(kind, true, percent);
    [c.r, c.g, c.b]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Straight RGB at a pixel, asserting it is fully opaque.
    fn opaque_rgb(pixmap: &Pixmap, x: u32, y: u32) -> [u8; 3] {
        let p = pixmap.pixel(x, y).expect("pixel in range").demultiply();
        assert_eq!(p.alpha(), 255, "pixel ({x},{y}) is not opaque");
        [p.red(), p.green(), p.blue()]
    }

    /// The pixel whose centre lies on the ring radius (12.15) to the right
    /// of the centre: x = 16 + 12.15 rounds into pixel 28.
    const TRACK_SAMPLE: (u32, u32) = (28, 16);
    /// The pixel at 12 o'clock on the ring radius: y = 16 - 12.15 = 3.85.
    const TOP_SAMPLE: (u32, u32) = (16, 3);

    #[test]
    fn centre_is_the_provider_disk() {
        for kind in ProviderId::ALL {
            let pixmap = render_pixmap(kind, Some(50.0));
            assert_eq!(opaque_rgb(&pixmap, 16, 16), base_color(kind), "{kind:?}");
        }
    }

    #[test]
    fn ring_is_track_coloured_without_a_percent() {
        let pixmap = render_pixmap(ProviderId::Claude, None);
        assert_eq!(
            opaque_rgb(&pixmap, TRACK_SAMPLE.0, TRACK_SAMPLE.1),
            TRACK_RGB
        );
        assert_eq!(opaque_rgb(&pixmap, TOP_SAMPLE.0, TOP_SAMPLE.1), TRACK_RGB);
        // Zero percent draws no arc either.
        let zero = render_pixmap(ProviderId::Claude, Some(0.0));
        assert_eq!(opaque_rgb(&zero, TOP_SAMPLE.0, TOP_SAMPLE.1), TRACK_RGB);
    }

    #[test]
    fn arc_starts_at_twelve_o_clock_for_any_percent() {
        for kind in ProviderId::ALL {
            for pct in [0.5, 1.0, 25.0, 59.9, 60.0, 80.0, 95.0, 100.0, 150.0] {
                let pixmap = render_pixmap(kind, Some(pct));
                assert_eq!(
                    opaque_rgb(&pixmap, TOP_SAMPLE.0, TOP_SAMPLE.1),
                    usage_rgb(kind, pct),
                    "{kind:?} at {pct}%"
                );
            }
        }
    }

    #[test]
    fn a_quarter_sweep_leaves_the_left_side_as_track() {
        let pixmap = render_pixmap(ProviderId::Claude, Some(25.0));
        // 3 o'clock is the end of the sweep (round cap), 9 o'clock is track.
        assert_eq!(
            opaque_rgb(&pixmap, TRACK_SAMPLE.0, TRACK_SAMPLE.1),
            usage_rgb(ProviderId::Claude, 25.0)
        );
        assert_eq!(opaque_rgb(&pixmap, 3, 16), TRACK_RGB);
    }

    #[test]
    fn corners_stay_transparent() {
        let pixmap = render_pixmap(ProviderId::ChatGpt, Some(100.0));
        assert_eq!(pixmap.pixel(0, 0).expect("pixel").alpha(), 0);
        assert_eq!(pixmap.pixel(31, 31).expect("pixel").alpha(), 0);
    }

    #[test]
    fn demultiply_restores_straight_alpha() {
        let mut pixmap = Pixmap::new(2, 1).expect("pixmap");
        // 50% alpha red: premultiplied (128, 0, 0, 128).
        pixmap.fill(tiny_skia::Color::from_rgba8(255, 0, 0, 128));
        assert_eq!(pixmap.data()[..4], [128, 0, 0, 128]);
        assert_eq!(demultiplied_rgba(&pixmap), [255, 0, 0, 128, 255, 0, 0, 128]);

        let badge = render_pixmap(ProviderId::Claude, Some(42.0));
        let rgba = demultiplied_rgba(&badge);
        assert_eq!(rgba.len(), (BADGE_PX * BADGE_PX * 4) as usize);
        // Opaque pixels are unchanged; transparent ones stay zero.
        assert_eq!(rgba[..4], [0, 0, 0, 0]);
        let centre = ((16 * BADGE_PX + 16) * 4) as usize;
        assert_eq!(rgba[centre..centre + 3], base_color(ProviderId::Claude));
        assert_eq!(rgba[centre + 3], 255);
    }
}
