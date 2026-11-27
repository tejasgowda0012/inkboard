//! Drawing helpers for the app's own controls: rounded panels, soft shadows,
//! text, icons, pictures and the brand mark.

use super::font::{self, Face};
use super::icons;
use crate::geom::IRect;
use tiny_skia::{
    FillRule, LineCap, LineJoin, Mask, Paint, Path, PathBuilder, Pixmap, PixmapMut, PixmapPaint,
    PixmapRef, Rect, Stroke, Transform,
};

pub use super::icons::{Ico, ALL};

// (Outward growth, downward offset, alpha), in design units.
const ELEVATIONS: [[(f32, f32, f32); 4]; 3] = [
    [
        (0.0, 2.0, 0.10),
        (2.0, 4.0, 0.05),
        (5.0, 7.0, 0.03),
        (9.0, 10.0, 0.015),
    ],
    [
        (0.0, 3.0, 0.12),
        (3.0, 6.0, 0.06),
        (7.0, 10.0, 0.04),
        (12.0, 16.0, 0.02),
    ],
    [
        (0.0, 4.0, 0.14),
        (5.0, 8.0, 0.07),
        (10.0, 14.0, 0.04),
        (17.0, 22.0, 0.02),
    ],
];

fn elevation_layers(level: u8) -> &'static [(f32, f32, f32)] {
    match level {
        1..=3 => &ELEVATIONS[usize::from(level - 1)],
        _ => &[],
    }
}

/// Uniform padding in screen pixels sufficient to cover the elevation shadow.
pub fn elev_pad(u: f32, level: u8) -> f32 {
    elevation_layers(level)
        .iter()
        .map(|&(grow, dy, _)| grow + dy.abs())
        .fold(0.0, f32::max)
        * u
}

/// A rectangle in screen pixels.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Rf {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rf {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Rf {
        Rf { x, y, w, h }
    }
    pub fn right(&self) -> f32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
    pub fn cx(&self) -> f32 {
        self.x + self.w / 2.0
    }
    pub fn cy(&self) -> f32 {
        self.y + self.h / 2.0
    }
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }
    pub fn inset(&self, d: f32) -> Rf {
        Rf::new(
            self.x + d,
            self.y + d,
            (self.w - 2.0 * d).max(0.0),
            (self.h - 2.0 * d).max(0.0),
        )
    }
    /// Whole pixels covering the rectangle grown by `pad`.
    pub fn covering(&self, pad: f32) -> IRect {
        IRect::new(
            (self.x - pad).floor() as i32,
            (self.y - pad).floor() as i32,
            (self.right() + pad).ceil() as i32,
            (self.bottom() + pad).ceil() as i32,
        )
    }
}

/// Paints into a pixel buffer that covers the screen area starting at
/// (`ox`, `oy`). Callers use screen coordinates.
pub struct Painter<'a> {
    pub pm: PixmapMut<'a>,
    pub ox: f32,
    pub oy: f32,
}

fn paint(color: u32) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color_rgba8(
        (color >> 16) as u8,
        (color >> 8) as u8,
        color as u8,
        (color >> 24) as u8,
    );
    p.anti_alias = true;
    p
}

pub(super) fn rrect_path(r: Rf, radius: f32) -> Option<Path> {
    if r.w <= 0.0 || r.h <= 0.0 {
        return None;
    }
    let rad = radius.min(r.w / 2.0).min(r.h / 2.0).max(0.0);
    if rad <= 0.0 {
        return Some(PathBuilder::from_rect(Rect::from_xywh(r.x, r.y, r.w, r.h)?));
    }
    // Circular corners from cubic curves.
    let k = rad * 0.447_715;
    let (l, t, rt, b) = (r.x, r.y, r.right(), r.bottom());
    let mut pb = PathBuilder::new();
    pb.move_to(l + rad, t);
    pb.line_to(rt - rad, t);
    pb.cubic_to(rt - k, t, rt, t + k, rt, t + rad);
    pb.line_to(rt, b - rad);
    pb.cubic_to(rt, b - k, rt - k, b, rt - rad, b);
    pb.line_to(l + rad, b);
    pb.cubic_to(l + k, b, l, b - k, l, b - rad);
    pb.line_to(l, t + rad);
    pb.cubic_to(l, t + k, l + k, t, l + rad, t);
    pb.close();
    pb.finish()
}

pub(super) fn push_polyline(path: &mut PathBuilder, pts: &[(f32, f32)]) {
    if let Some((&(x, y), rest)) = pts.split_first() {
        path.move_to(x, y);
        for &(x, y) in rest {
            path.line_to(x, y);
        }
    }
}

fn round_stroke(width: f32) -> Stroke {
    Stroke {
        width,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Default::default()
    }
}

fn draw_icon_paths(
    pm: &mut PixmapMut<'_>,
    paths: &(Option<Path>, Option<Path>),
    color: &Paint<'_>,
    stroke: &Stroke,
    tf: Transform,
    mask: Option<&Mask>,
) {
    if let Some(path) = &paths.0 {
        pm.stroke_path(path, color, stroke, tf, mask);
    }
    if let Some(path) = &paths.1 {
        pm.fill_path(path, color, FillRule::Winding, tf, mask);
    }
}

impl<'a> Painter<'a> {
    fn tf(&self) -> Transform {
        Transform::from_translate(-self.ox, -self.oy)
    }

    fn clip(&self) -> IRect {
        IRect::new(0, 0, self.pm.width() as i32, self.pm.height() as i32)
    }

    pub fn fill(&mut self, color: u32) {
        let (r, g, b, a) = (
            (color >> 16) as u8,
            (color >> 8) as u8,
            color as u8,
            (color >> 24) as u8,
        );
        self.pm.fill(tiny_skia::Color::from_rgba8(r, g, b, a));
    }

    pub fn rrect(&mut self, r: Rf, radius: f32, color: u32) {
        if color >> 24 == 0 {
            return;
        }
        if let Some(path) = rrect_path(r, radius) {
            let tf = self.tf();
            self.pm
                .fill_path(&path, &paint(color), FillRule::Winding, tf, None);
        }
    }

    /// A rounded outline drawn just inside `r`.
    pub fn rrect_line(&mut self, r: Rf, radius: f32, color: u32, width: f32) {
        let h = width / 2.0;
        if let Some(path) = rrect_path(r.inset(h), (radius - h).max(0.0)) {
            let stroke = tiny_skia::Stroke {
                width,
                ..Default::default()
            };
            let tf = self.tf();
            self.pm.stroke_path(&path, &paint(color), &stroke, tf, None);
        }
    }

    /// A soft shadow under a raised surface, like a Material elevation.
    pub fn shadow(&mut self, r: Rf, radius: f32, d: f32, level: f32) {
        for (grow, dy, alpha) in [
            (0.0, 1.0, 0x16u32),
            (1.0, 1.5, 0x0d),
            (2.2, 2.2, 0x07),
            (3.6, 3.0, 0x04),
        ] {
            let g = grow * d * level;
            let a = ((alpha as f32) * level.min(2.0)).min(255.0) as u32;
            let s = Rf::new(
                r.x - g,
                r.y - g + dy * d * level,
                r.w + 2.0 * g,
                r.h + 2.0 * g,
            );
            self.rrect(s, radius + g, a << 24);
        }
    }

    /// Draws the stacked shadow for design elevation 1, 2 or 3.
    pub fn elev(&mut self, r: Rf, radius: f32, u: f32, level: u8) {
        for &(grow, dy, alpha) in elevation_layers(level) {
            let g = grow * u;
            let shadow = Rf::new(r.x - g, r.y - g + dy * u, r.w + 2.0 * g, r.h + 2.0 * g);
            let a = (alpha * 255.0).round() as u32;
            self.rrect(shadow, radius + g, (a << 24) | 0x0014_1628);
        }
    }

    pub fn toggle(&mut self, r: Rf, on: bool, track_on: u32, track_off: u32, knob: u32) {
        let radius = r.h / 2.0;
        let diameter = r.h * (44.0 / 52.0);
        let cx = if on { r.right() - radius } else { r.x + radius };
        self.rrect(r, radius, if on { track_on } else { track_off });
        self.circle(cx, r.cy(), diameter / 2.0, knob);
    }

    /// Fills a closed polygon whose vertices are in screen pixels.
    pub fn polygon(&mut self, pts: &[(f32, f32)], color: u32) {
        if pts.len() < 3 {
            return;
        }
        let mut pb = PathBuilder::new();
        push_polyline(&mut pb, pts);
        pb.close();
        if let Some(path) = pb.finish() {
            let tf = self.tf();
            self.pm
                .fill_path(&path, &paint(color), FillRule::Winding, tf, None);
        }
    }

    pub fn circle(&mut self, cx: f32, cy: f32, radius: f32, color: u32) {
        if let Some(path) = PathBuilder::from_circle(cx, cy, radius) {
            let tf = self.tf();
            self.pm
                .fill_path(&path, &paint(color), FillRule::Winding, tf, None);
        }
    }

    pub fn circle_line(&mut self, cx: f32, cy: f32, radius: f32, color: u32, width: f32) {
        if let Some(path) = PathBuilder::from_circle(cx, cy, radius - width / 2.0) {
            let stroke = tiny_skia::Stroke {
                width,
                ..Default::default()
            };
            let tf = self.tf();
            self.pm.stroke_path(&path, &paint(color), &stroke, tf, None);
        }
    }

    /// Draws one line of text; returns its advance.
    pub fn text(
        &mut self,
        face: Face,
        size: f32,
        text: &str,
        x: f32,
        baseline: f32,
        color: u32,
    ) -> f32 {
        let clip = self.clip();
        font::draw(
            &mut self.pm,
            face,
            size,
            text,
            x - self.ox,
            baseline - self.oy,
            color,
            clip,
        )
    }

    /// Draws one line of text cut with "…" to fit `max_width`.
    pub fn text_fit(
        &mut self,
        face: Face,
        size: f32,
        text: &str,
        x: f32,
        baseline: f32,
        max_width: f32,
        color: u32,
    ) -> f32 {
        let clip = self.clip();
        font::draw_ellipsized(
            &mut self.pm,
            face,
            size,
            text,
            x - self.ox,
            baseline - self.oy,
            max_width,
            color,
            clip,
        )
    }

    pub fn icon(&mut self, code: u32, size: f32, cx: f32, cy: f32, color: u32) {
        let clip = self.clip();
        font::draw_icon(
            &mut self.pm,
            code,
            size,
            cx - self.ox,
            cy - self.oy,
            color,
            clip,
        );
    }

    /// Draws a vector icon centred on (cx, cy), at the given size in screen pixels.
    pub fn ico(&mut self, ico: Ico, size: f32, cx: f32, cy: f32, color: u32) {
        if !size.is_finite() || size <= 0.0 || color >> 24 == 0 {
            return;
        }
        let scale = size / 24.0;
        let left = cx - size / 2.0 - self.ox;
        let top = cy - size / 2.0 - self.oy;
        let color = paint(color);
        let paths = icons::paths(ico);
        let stroke = round_stroke(2.0_f32.max(1.2 / scale));

        // The stroked paths have at least a 2-unit inset. For tiny icons the
        // minimum pixel stroke can cross the box, so clip it in a local pixmap.
        if 2.0 * scale < 0.6 {
            let (x, y) = (left.floor(), top.floor());
            let (local_x, local_y) = (left - x, top - y);
            let w = (local_x + size).ceil() as u32;
            let h = (local_y + size).ceil() as u32;
            if let (Some(mut tile), Some(mut mask), Some(clip)) = (
                Pixmap::new(w, h),
                Mask::new(w, h),
                Rect::from_xywh(local_x, local_y, size, size),
            ) {
                mask.fill_path(
                    &PathBuilder::from_rect(clip),
                    FillRule::Winding,
                    true,
                    Transform::identity(),
                );
                let tf = Transform::from_row(scale, 0.0, 0.0, scale, local_x, local_y);
                draw_icon_paths(&mut tile.as_mut(), &paths, &color, &stroke, tf, Some(&mask));
                self.pm.draw_pixmap(
                    x as i32,
                    y as i32,
                    tile.as_ref(),
                    &PixmapPaint::default(),
                    Transform::identity(),
                    None,
                );
            }
        } else {
            let tf = Transform::from_row(scale, 0.0, 0.0, scale, left, top);
            draw_icon_paths(&mut self.pm, &paths, &color, &stroke, tf, None);
        }
    }

    /// Draws a premultiplied RGBA picture with its top left at (x, y).
    pub fn image(&mut self, rgba: &[u8], w: u32, h: u32, x: f32, y: f32) {
        if let Some(src) = PixmapRef::from_bytes(rgba, w, h) {
            let (px, py) = ((x - self.ox).round() as i32, (y - self.oy).round() as i32);
            self.pm.draw_pixmap(
                px,
                py,
                src,
                &PixmapPaint::default(),
                Transform::identity(),
                None,
            );
        }
    }

    /// An eraser tilted on its edge, drawn in a 24-unit box like an icon
    /// of `size` centred on (cx, cy).
    pub fn eraser(&mut self, size: f32, cx: f32, cy: f32, color: u32) {
        let s = size / 24.0;
        let tf = Transform::from_row(s, 0.0, 0.0, s, cx - 12.0 * s - self.ox, cy - 12.0 * s - self.oy);
        let poly = |pts: &[(f32, f32)], close: bool| {
            let mut pb = PathBuilder::new();
            pb.move_to(pts[0].0, pts[0].1);
            for p in &pts[1..] {
                pb.line_to(p.0, p.1);
            }
            if close {
                pb.close();
            }
            pb.finish()
        };
        let stroke = tiny_skia::Stroke {
            width: 2.0,
            line_cap: tiny_skia::LineCap::Round,
            line_join: tiny_skia::LineJoin::Round,
            ..Default::default()
        };
        if let Some(tip) = poly(&[(3.5, 14.5), (7.1, 10.9), (14.1, 17.9), (10.5, 21.5)], true) {
            self.pm.fill_path(&tip, &paint(color), FillRule::Winding, tf, None);
        }
        if let Some(body) = poly(&[(3.5, 14.5), (12.5, 5.5), (19.5, 12.5), (10.5, 21.5)], true) {
            self.pm.stroke_path(&body, &paint(color), &stroke, tf, None);
        }
        if let Some(floor) = poly(&[(13.5, 21.5), (21.0, 21.5)], false) {
            self.pm.stroke_path(&floor, &paint(color), &stroke, tf, None);
        }
    }

    /// Inkboard's canonical mark, filling `r`.
    pub fn logo(&mut self, r: Rf) {
        self.logo_accent(r, 0xff27_48e8);
    }

    /// Draws the accent-coloured Inkboard mark in a square centred within `r`.
    pub fn logo_accent(&mut self, r: Rf, accent: u32) {
        let side = r.w.min(r.h);
        if side <= 0.0 {
            return;
        }
        let s = side / 56.0;
        let square = Rf::new(r.cx() - side / 2.0, r.cy() - side / 2.0, side, side);
        self.rrect(square, 16.0 * s, accent);

        let mut wave = PathBuilder::new();
        push_polyline(
            &mut wave,
            &[
                (12.0, 36.0),
                (17.0, 31.0),
                (23.0, 36.0),
                (29.0, 28.0),
                (36.0, 32.0),
                (44.0, 18.0),
            ],
        );
        if let Some(path) = wave.finish() {
            let tf = Transform::from_scale(s, s)
                .post_translate(square.x - self.ox, square.y - self.oy);
            self.pm
                .stroke_path(&path, &paint(0xffff_ffff), &round_stroke(5.0), tf, None);
        }
    }
}

/// Splits `text` into lines no wider than `width`, breaking at spaces.
pub fn wrap(face: Face, size: f32, text: &str, width: f32) -> Vec<String> {
    let mut lines = Vec::new();
    for paragraph in text.split('\n') {
        let mut line = String::new();
        for word in paragraph.split(' ') {
            let candidate = if line.is_empty() {
                word.to_string()
            } else {
                format!("{line} {word}")
            };
            if !line.is_empty() && font::measure(face, size, &candidate).0 > width {
                lines.push(std::mem::take(&mut line));
                line = word.to_string();
            } else {
                line = candidate;
            }
        }
        lines.push(line);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_draws_inside_its_box() {
        for ico in ALL {
            for size in [32_u32, 12, 6] {
                let mut pm = Pixmap::new(48, 48).unwrap();
                Painter {
                    pm: pm.as_mut(),
                    ox: 100.0,
                    oy: 200.0,
                }
                .ico(ico, size as f32, 124.0, 224.0, 0xff14_161c);

                let bounds = (24 - size / 2)..(24 + size / 2);
                let mut max_alpha = 0;
                for y in 0..48 {
                    for x in 0..48 {
                        let alpha = pm.pixel(x, y).unwrap().alpha();
                        if bounds.contains(&x) && bounds.contains(&y) {
                            max_alpha = max_alpha.max(alpha);
                        } else {
                            assert_eq!(alpha, 0, "{ico:?} at size {size} drew outside its box");
                        }
                    }
                }
                if size == 32 {
                    assert!(max_alpha >= 192, "{ico:?} did not draw any opaque pixels");
                } else {
                    assert!(max_alpha > 0, "{ico:?} at size {size} did not draw");
                }
            }
        }
    }

    #[test]
    fn elevation_two_darkens_below_the_surface() {
        let mut pm = Pixmap::new(48, 48).unwrap();
        let mut painter = Painter {
            pm: pm.as_mut(),
            ox: 0.0,
            oy: 0.0,
        };
        let r = Rf::new(8.0, 8.0, 32.0, 10.0);
        painter.fill(0xffff_ffff);
        painter.elev(r, 4.0, 1.0, 0);
        assert_eq!(painter.pm.as_ref().pixel(24, 18).unwrap().red(), 255);
        painter.elev(r, 4.0, 1.0, 2);
        painter.rrect(r, 4.0, 0xffff_ffff);

        assert_eq!(elev_pad(2.0, 0), 0.0);
        assert_eq!(elev_pad(2.0, 1), 38.0);
        assert_eq!(elev_pad(2.0, 2), 56.0);
        assert_eq!(elev_pad(2.0, 3), 78.0);
        assert_eq!(pm.pixel(24, 17).unwrap().red(), 255);
        let below = pm.pixel(24, 18).unwrap();
        assert!(below.red() < below.blue() && below.blue() < 255);
        assert!(pm.pixel(24, 45).unwrap().red() < 255);
        assert_eq!(pm.pixel(24, 46).unwrap().red(), 255);
    }

    #[test]
    fn toggle_knob_moves_between_sides() {
        for on in [false, true] {
            let mut pm = Pixmap::new(48, 48).unwrap();
            Painter {
                pm: pm.as_mut(),
                ox: 0.0,
                oy: 0.0,
            }
            .toggle(
                Rf::new(2.0, 6.0, 44.0, 26.0),
                on,
                0xff27_48e8,
                0xffc4_cad6,
                0xffff_ffff,
            );

            let left = pm.pixel(15, 19).unwrap();
            let right = pm.pixel(33, 19).unwrap();
            assert_eq!(left.red(), if on { 0x27 } else { 0xff });
            assert_eq!(right.red(), if on { 0xff } else { 0xc4 });
        }
    }
}
