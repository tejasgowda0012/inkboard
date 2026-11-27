//! Software rendering with tiny-skia into plain RGBA buffers.
//!
//! Everything draws through a [`Target`]: a rectangular window onto the screen
//! with its own small pixel buffer, so repainting a region costs time in
//! proportion to that region and never to the size of the screen.

use crate::geom::{IRect, Pt, Rect};
use crate::import::ImageAsset;
use crate::mindmap;
use crate::shape::{Shape, Tool};
use crate::text;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, OnceLock};
use tiny_skia::{
    FillRule, FilterQuality, LineCap, LineJoin, Paint, Path, PathBuilder, Pixmap, PixmapMut,
    PixmapPaint, Stroke, Transform,
};

mod board_objects;
pub(crate) use board_objects::draw_connection;

#[path = "math/draw.rs"]
mod math_draw;

pub const BACKGROUND: u32 = 0xfff9fafb;
pub const GRID_DOT: u32 = 0xffd9dce1;
pub const SELECTION_BLUE: u32 = 0xff2f80ed;

/// Camera: `screen = world * zoom + cam` in logical pixels, times `dpr` for
/// buffer pixels. The pixel offset is rounded so panning moves whole pixels.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct View {
    pub cam: Pt,
    pub zoom: f64,
    pub dpr: f64,
}

impl View {
    pub fn scale(&self) -> f64 {
        self.zoom * self.dpr
    }
    pub fn offset_px(&self) -> (f64, f64) {
        ((self.cam.x * self.dpr).round(), (self.cam.y * self.dpr).round())
    }
    pub fn to_px(&self, p: Pt) -> Pt {
        let (ox, oy) = self.offset_px();
        Pt::new(p.x * self.scale() + ox, p.y * self.scale() + oy)
    }
    pub fn rect_px(&self, r: &Rect) -> Rect {
        Rect::from_points(self.to_px(r.top_left()), self.to_px(r.bottom_right()))
    }
}

/// A screen-sized RGBA8888 (premultiplied) pixel buffer.
#[derive(Clone)]
pub struct Layer {
    pub w: u32,
    pub h: u32,
    pub data: Vec<u8>,
}

impl Layer {
    pub fn new(w: u32, h: u32) -> Layer {
        Layer { w, h, data: vec![0; w as usize * h as usize * 4] }
    }
    pub fn bounds(&self) -> IRect {
        IRect::new(0, 0, self.w as i32, self.h as i32)
    }
    fn row(&self, x0: i32, y: i32, width: i32) -> std::ops::Range<usize> {
        let start = (y as usize * self.w as usize + x0 as usize) * 4;
        start..start + width as usize * 4
    }
    /// Copies the pixels of `r` (which must lie inside the layer) out.
    pub fn read(&self, r: IRect) -> Vec<u8> {
        let mut out = Vec::with_capacity(r.width() as usize * r.height() as usize * 4);
        for y in r.y0..r.y1 {
            out.extend_from_slice(&self.data[self.row(r.x0, y, r.width())]);
        }
        out
    }
    pub fn write(&mut self, r: IRect, buf: &[u8]) {
        let stride = r.width() as usize * 4;
        for (i, y) in (r.y0..r.y1).enumerate() {
            let range = self.row(r.x0, y, r.width());
            self.data[range].copy_from_slice(&buf[i * stride..(i + 1) * stride]);
        }
    }
    pub fn copy_from(&mut self, src: &Layer, r: IRect) {
        if r == self.bounds() && self.w == src.w && self.h == src.h {
            self.data.copy_from_slice(&src.data);
            return;
        }
        for y in r.y0..r.y1 {
            let range = self.row(r.x0, y, r.width());
            self.data[range.clone()].copy_from_slice(&src.data[range]);
        }
    }
    /// Moves the picture by whole pixels. Pixels that scroll in are garbage
    /// and must be repainted by the caller.
    pub fn scroll(&mut self, dx: i32, dy: i32) {
        let offset = (dy as isize * self.w as isize + dx as isize) * 4;
        let len = self.data.len();
        if offset > 0 {
            let o = offset as usize;
            self.data.copy_within(0..len - o, o);
        } else if offset < 0 {
            let o = (-offset) as usize;
            self.data.copy_within(o..len, 0);
        }
    }
}

/// A window of pixels covering screen rectangle `region`.
pub struct Target<'a> {
    pub pm: PixmapMut<'a>,
    pub region: IRect,
    pub view: View,
}

impl<'a> Target<'a> {
    pub fn new(buf: &'a mut [u8], region: IRect, view: View) -> Option<Target<'a>> {
        let pm = PixmapMut::from_bytes(buf, region.width() as u32, region.height() as u32)?;
        Some(Target { pm, region, view })
    }
    /// World coordinates to pixels of this target.
    pub fn world(&self) -> Transform {
        let (ox, oy) = self.view.offset_px();
        let s = self.view.scale() as f32;
        Transform::from_scale(s, s)
            .post_translate((ox - self.region.x0 as f64) as f32, (oy - self.region.y0 as f64) as f32)
    }
}

fn paint_for(color: u32, alpha: f32) -> Paint<'static> {
    let mut paint = Paint::default();
    let a = ((color >> 24) & 0xff) as f32 * alpha;
    paint.set_color_rgba8(
        ((color >> 16) & 0xff) as u8,
        ((color >> 8) & 0xff) as u8,
        (color & 0xff) as u8,
        a.round().clamp(0.0, 255.0) as u8,
    );
    paint.anti_alias = true;
    paint
}

fn round_stroke(width: f64) -> Stroke {
    Stroke { width: width as f32, line_cap: LineCap::Round, line_join: LineJoin::Round, ..Stroke::default() }
}

fn f(p: Pt) -> (f32, f32) {
    (p.x as f32, p.y as f32)
}

fn line_path(a: Pt, b: Pt) -> Option<Path> {
    let mut pb = PathBuilder::new();
    pb.move_to(a.x as f32, a.y as f32);
    pb.line_to(b.x as f32, b.y as f32);
    pb.finish()
}

fn round_rect_path(r: &Rect, radius: f64) -> Option<Path> {
    let (l, t, rt, b) = (r.l as f32, r.t as f32, r.r as f32, r.b as f32);
    let rad = radius as f32;
    let mut pb = PathBuilder::new();
    if rad <= 0.01 {
        pb.move_to(l, t);
        pb.line_to(rt, t);
        pb.line_to(rt, b);
        pb.line_to(l, b);
        pb.close();
        return pb.finish();
    }
    let k = rad * 0.447_715_25; // distance from corner to the Bezier handle
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

fn ellipse_path(r: &Rect) -> Option<Path> {
    let rect = tiny_skia::Rect::from_ltrb(r.l as f32, r.t as f32, r.r as f32, r.b as f32)?;
    PathBuilder::from_oval(rect)
}

fn polygon_path(s: &Shape) -> Option<Path> {
    let mut vertices = s.polygon_vertices();
    let mut pb = PathBuilder::new();
    let (x, y) = f(vertices.next()?);
    pb.move_to(x, y);
    for p in vertices {
        let (x, y) = f(p);
        pb.line_to(x, y);
    }
    pb.close();
    pb.finish()
}

/// The smoothed freehand outline: quadratic curves through segment midpoints.
pub fn pen_path(points: &[Pt]) -> Option<Path> {
    let mut pb = PathBuilder::new();
    let (x, y) = f(points[0]);
    pb.move_to(x, y);
    for i in 1..points.len().saturating_sub(1) {
        let (p, n) = (points[i], points[i + 1]);
        pb.quad_to(p.x as f32, p.y as f32, ((p.x + n.x) / 2.0) as f32, ((p.y + n.y) / 2.0) as f32);
    }
    let (x, y) = f(*points.last().unwrap());
    pb.line_to(x, y);
    pb.finish()
}

fn arrow_head_length(s: &Shape) -> f64 {
    let (a, b) = (s.points[0], *s.points.last().unwrap());
    f64::min(f64::max(14.0, s.width * 4.0), f64::max(4.0, a.dist(b) * 0.5))
}

/// World-space rectangle that contains every pixel the shape paints.
pub fn paint_bounds(s: &Shape) -> Rect {
    let pad = match s.tool {
        Tool::Arrow if !s.points.is_empty() => s.width / 2.0 + arrow_head_length(s),
        Tool::Text => s.font_size * 0.35,
        Tool::Formula => s.font_size * 0.45,
        Tool::Graph => (s.bounds().shortest_side() * 0.005).max(s.width / 2.0),
        _ => s.width / 2.0,
    };
    s.bounds().inflate(pad)
}

type TextCache = Mutex<HashMap<(String, u64, u32, u8), Option<Arc<Pixmap>>>>;

fn text_cache() -> &'static TextCache {
    static CACHE: OnceLock<TextCache> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn text_bitmap(content: &str, font_px: f64, color: u32, font: u8) -> Option<Arc<Pixmap>> {
    let key = (content.to_string(), font_px.to_bits(), color, font);
    if let Some(hit) = text_cache().lock().unwrap().get(&key) {
        return hit.clone();
    }
    let bitmap = text::rasterize(content, font_px, color, font).and_then(|b| {
        let size = tiny_skia::IntSize::from_wh(b.width, b.height)?;
        Pixmap::from_vec(b.rgba, size).map(Arc::new)
    });
    let mut cache = text_cache().lock().unwrap();
    if cache.len() >= 96 {
        cache.clear();
    }
    cache.insert(key, bitmap.clone());
    bitmap
}

fn draw_text_at(
    t: &mut Target,
    s: &Shape,
    (content, a, size): (&str, Pt, f64),
    alpha: f32,
    ts: Transform,
    plain: bool,
) {
    let color = if s.tool == Tool::Text { s.color } else { 0xff14161c };
    draw_label(t, (content, a, size), (s.font, color), alpha, ts, plain);
}

fn draw_label(
    t: &mut Target,
    (content, a, size): (&str, Pt, f64),
    (font, color): (u8, u32),
    alpha: f32,
    ts: Transform,
    plain: bool,
) {
    if content.is_empty() {
        return;
    }
    let scale = t.view.scale();
    // Very large text is rendered once at a capped size and scaled up.
    let font_px = ((size * scale).clamp(1.0, 384.0) * 4.0).round() / 4.0;
    let Some(bitmap) = text_bitmap(content, font_px, color, font) else {
        return;
    };
    let unit = (size / font_px) as f32; // world units per bitmap pixel
    let mut paint = PixmapPaint { opacity: alpha, ..PixmapPaint::default() };
    let exact = plain && (unit as f64 * scale - 1.0).abs() < 0.01;
    let transform = if exact {
        let (ox, oy) = t.view.offset_px();
        Transform::from_translate(
            (a.x * scale + ox).round() as f32 - t.region.x0 as f32,
            (a.y * scale + oy).round() as f32 - t.region.y0 as f32,
        )
    } else {
        paint.quality = FilterQuality::Bilinear;
        ts.pre_translate(a.x as f32, a.y as f32).pre_scale(unit, unit)
    };
    t.pm.draw_pixmap(0, 0, bitmap.as_ref().as_ref(), &paint, transform, None);
}

type ImageCache = VecDeque<([u8; 32], Option<Arc<Pixmap>>)>;

fn image_bitmap(asset: &ImageAsset) -> Option<Arc<Pixmap>> {
    static CACHE: OnceLock<Mutex<ImageCache>> = OnceLock::new();
    const MAX_PIXELS_BYTES: usize = 48 << 20;
    let cache = CACHE.get_or_init(|| Mutex::new(VecDeque::new()));
    let hash = asset.hash();
    let cached = |cache: &mut ImageCache| {
        let position = cache.iter().position(|(key, _)| *key == hash)?;
        let entry = cache.remove(position)?;
        let value = entry.1.clone();
        cache.push_back(entry);
        Some(value)
    };
    if let Some(bitmap) = cached(&mut cache.lock().unwrap()) {
        return bitmap;
    }
    let bitmap = asset.pixmap().map(Arc::new);
    let mut cache = cache.lock().unwrap();
    if let Some(bitmap) = cached(&mut cache) {
        return bitmap;
    }
    let bytes = |bitmap: &Option<Arc<Pixmap>>| bitmap.as_ref().map_or(0, |p| p.data().len());
    let mut total = bytes(&bitmap) + cache.iter().map(|(_, p)| bytes(p)).sum::<usize>();
    while cache.len() >= 64 || total > MAX_PIXELS_BYTES {
        let Some((_, oldest)) = cache.pop_front() else { break };
        total -= bytes(&oldest);
    }
    cache.push_back((hash, bitmap.clone()));
    bitmap
}

fn draw_image(t: &mut Target, shape: &Shape, alpha: f32, transform: Transform) {
    let Some(bitmap) = shape.image.as_deref().and_then(image_bitmap) else { return };
    let bounds = shape.bounds();
    let transform = transform.pre_translate(bounds.l as f32, bounds.t as f32).pre_scale(
        (bounds.width() / f64::from(bitmap.width())) as f32,
        (bounds.height() / f64::from(bitmap.height())) as f32,
    );
    let paint = PixmapPaint { opacity: alpha, quality: FilterQuality::Bilinear, ..PixmapPaint::default() };
    t.pm.draw_pixmap(0, 0, bitmap.as_ref().as_ref(), &paint, transform, None);
}

/// Draws one shape. `extra` is an additional world-space transform, used
/// while a selection is being moved or resized.
pub fn draw_shape(t: &mut Target, s: &Shape, alpha: f32, extra: Option<Transform>) {
    if s.points.is_empty() {
        return;
    }
    let ts = match extra {
        Some(e) => e.post_concat(t.world()),
        None => t.world(),
    };
    let paint = paint_for(s.color, alpha);
    let stroke = round_stroke(s.width);
    let (a, b) = (s.points[0], *s.points.last().unwrap());
    match s.tool {
        Tool::Rectangle | Tool::Ellipse | Tool::Triangle | Tool::Diamond | Tool::Star | Tool::Hexagon => {
            let rect = if matches!(s.tool, Tool::Rectangle | Tool::Ellipse) {
                Rect::from_points(a, b)
            } else {
                s.bounds()
            };
            let path = if rect.width() < 1e-6 || rect.height() < 1e-6 {
                line_path(rect.top_left(), rect.bottom_right())
            } else {
                match s.tool {
                    Tool::Rectangle => round_rect_path(&rect, f64::min(6.0, rect.shortest_side() / 4.0)),
                    Tool::Ellipse => ellipse_path(&rect),
                    _ => polygon_path(s),
                }
            };
            let Some(path) = path else { return };
            if s.filled {
                let fill = paint_for(s.color, alpha * 0.18);
                t.pm.fill_path(&path, &fill, FillRule::Winding, ts, None);
            }
            t.pm.stroke_path(&path, &paint, &stroke, ts, None);
        }
        Tool::Line => {
            if let Some(path) = line_path(a, b) {
                t.pm.stroke_path(&path, &paint, &stroke, ts, None);
            }
        }
        Tool::Arrow => {
            let angle = (b.y - a.y).atan2(b.x - a.x);
            let length = arrow_head_length(s);
            let wing = |d: f64| b - Pt::new((angle + d).cos(), (angle + d).sin()) * length;
            let mut pb = PathBuilder::new();
            pb.move_to(a.x as f32, a.y as f32);
            pb.line_to(b.x as f32, b.y as f32);
            for w in [wing(-0.5), wing(0.5)] {
                pb.move_to(b.x as f32, b.y as f32);
                pb.line_to(w.x as f32, w.y as f32);
            }
            if let Some(path) = pb.finish() {
                t.pm.stroke_path(&path, &paint, &stroke, ts, None);
            }
        }
        Tool::Text => draw_text_at(t, s, (&s.text, a, s.font_size), alpha, ts, extra.is_none()),
        Tool::Formula => math_draw::draw_formula(t, s, alpha, ts),
        Tool::Graph => math_draw::draw_graph(t, s, alpha, ts),
        Tool::Image => draw_image(t, s, alpha, ts),
        Tool::Table => board_objects::draw_table(t, s, alpha, ts),
        Tool::MindNode => board_objects::draw_mind_node(t, s, alpha, ts),
        Tool::Pen => {
            if s.is_dot() {
                if let Some(path) = PathBuilder::from_circle(a.x as f32, a.y as f32, (s.width / 2.0) as f32) {
                    t.pm.fill_path(&path, &paint, FillRule::Winding, ts, None);
                }
            } else if let Some(path) = pen_path(&s.points) {
                t.pm.stroke_path(&path, &paint, &stroke, ts, None);
            }
        }
        Tool::Select | Tool::Hand | Tool::Eraser => {}
    }
}

/// Draws the parts of an in-progress pen stroke that touch the target. The
/// result is pixel-identical to the finished stroke drawn by [`draw_shape`].
pub fn draw_draft(t: &mut Target, points: &[Pt], color: u32, width: f64) {
    let Some(first) = points.first() else { return };
    let ts = t.world();
    let paint = paint_for(color, 1.0);
    if points.len() == 1 || points.iter().all(|p| (*p - *first).len2() <= 0.01) {
        if let Some(path) = PathBuilder::from_circle(first.x as f32, first.y as f32, (width / 2.0) as f32) {
            t.pm.fill_path(&path, &paint, FillRule::Winding, ts, None);
        }
        return;
    }
    let pad = width / 2.0 * t.view.scale() + 2.0;
    let touches = |pts: &[Pt]| {
        let mut r = Rect::from_points(pts[0], pts[0]);
        for p in pts {
            r = r.union(&Rect::from_points(*p, *p));
        }
        IRect::covering(&t.view.rect_px(&r), pad).intersects(&t.region)
    };
    let mid = |a: Pt, b: Pt| (a + b) / 2.0;
    let n = points.len();
    let mut pb = PathBuilder::new();
    let mut pen_down = false;
    // Piece k (k >= 2) is the curve that adding point k created.
    for k in 2..n {
        let start = if k == 2 { points[0] } else { mid(points[k - 2], points[k - 1]) };
        let (ctrl, end) = (points[k - 1], mid(points[k - 1], points[k]));
        if touches(&[start, ctrl, end]) {
            if !pen_down {
                pb.move_to(start.x as f32, start.y as f32);
                pen_down = true;
            }
            pb.quad_to(ctrl.x as f32, ctrl.y as f32, end.x as f32, end.y as f32);
        } else {
            pen_down = false;
        }
    }
    let tail = if n == 2 { points[0] } else { mid(points[n - 2], points[n - 1]) };
    if touches(&[tail, points[n - 1]]) {
        if !pen_down {
            pb.move_to(tail.x as f32, tail.y as f32);
        }
        pb.line_to(points[n - 1].x as f32, points[n - 1].y as f32);
    }
    if let Some(path) = pb.finish() {
        t.pm.stroke_path(&path, &paint, &round_stroke(width), ts, None);
    }
}

/// Outline (and optional fill) of a world rectangle with a stroke width
/// given in logical screen pixels, for selection frames and the marquee.
pub fn draw_frame(t: &mut Target, r: &Rect, stroke_logical: f64, fill: Option<u32>, radius: f64) {
    let ts = t.world();
    let Some(path) = round_rect_path(r, radius) else { return };
    if let Some(color) = fill {
        t.pm.fill_path(&path, &paint_for(color, 1.0), FillRule::Winding, ts, None);
    }
    let stroke = Stroke { width: (stroke_logical / t.view.zoom) as f32, ..Stroke::default() };
    t.pm.stroke_path(&path, &paint_for(SELECTION_BLUE, 1.0), &stroke, ts, None);
}

/// Fills the target with the board background and its dot grid.
pub fn draw_background(t: &mut Target, grid: bool) {
    let bg = BACKGROUND;
    let px = [(bg >> 16) as u8, (bg >> 8) as u8, bg as u8, 0xff];
    for chunk in t.pm.data_mut().chunks_exact_mut(4) {
        chunk.copy_from_slice(&px);
    }
    if !grid {
        return;
    }
    let mut spacing = 24.0 * t.view.zoom;
    while spacing < 14.0 {
        spacing *= 2.0;
    }
    let spacing = spacing * t.view.dpr;
    let radius = (t.view.dpr).max(0.8);
    let (ox, oy) = t.view.offset_px();
    let (start_x, start_y) = (ox.rem_euclid(spacing), oy.rem_euclid(spacing));
    let reach = radius.ceil() as i32 + 1;
    let r = t.region;
    let first = |start: f64, low: i32| (((low - reach) as f64 - start) / spacing).floor() as i64;
    let last = |start: f64, high: i32| (((high + reach) as f64 - start) / spacing).ceil() as i64;
    let (w, h) = (r.width(), r.height());
    let max_r = radius + 0.5;
    let max_r2 = max_r * max_r;
    let dot = [(GRID_DOT >> 16) as u8 as f32, (GRID_DOT >> 8) as u8 as f32, GRID_DOT as u8 as f32];
    let data = t.pm.data_mut();
    for iy in first(start_y, r.y0)..=last(start_y, r.y1) {
        let cy = (start_y + iy as f64 * spacing).round() as i32 - r.y0;
        for ix in first(start_x, r.x0)..=last(start_x, r.x1) {
            let cx = (start_x + ix as f64 * spacing).round() as i32 - r.x0;
            for y in (cy - reach).max(0)..(cy + reach + 1).min(h) {
                let dy = y - cy;
                for x in (cx - reach).max(0)..(cx + reach + 1).min(w) {
                    let dx = x - cx;
                    let d2 = (dx * dx + dy * dy) as f64;
                    if d2 >= max_r2 {
                        continue;
                    }
                    let cover = (max_r - d2.sqrt()).clamp(0.0, 1.0) as f32;
                    let i = (y as usize * w as usize + x as usize) * 4;
                    for c in 0..3 {
                        let old = data[i + c] as f32;
                        data[i + c] = (old + (dot[c] - old) * cover).round() as u8;
                    }
                }
            }
        }
    }
}

/// Scales a cached snapshot into `dst` for fast live feedback during an active
/// pinch-zoom gesture, avoiding a full vector repaint on every frame.
pub fn blit_scaled(dst: &mut Layer, src: &Layer, src_view: View, dst_view: View) {
    // A plain nearest-neighbour copy: tiny-skia's transformed draw_pixmap
    // costs 100-350 ms per 4K frame on the TV, this a few ms.
    let bg = BACKGROUND;
    let px = [(bg >> 16) as u8, (bg >> 8) as u8, bg as u8, 0xff];
    let (sw, sh, dw) = (src.w as usize, src.h as usize, dst.w as usize);
    if dw == 0 || src.data.len() < sw * sh * 4 {
        return;
    }
    let rel = dst_view.scale() / src_view.scale();
    let (sox, soy) = src_view.offset_px();
    let (dox, doy) = dst_view.offset_px();
    // dst = src * rel + t, so src = (dst - t) / rel, sampled at pixel centres.
    let (tx, ty) = (dox - sox * rel, doy - soy * rel);
    let source = |d: usize, t: f64, len: usize| {
        let s = ((d as f64 + 0.5 - t) / rel).floor();
        (s >= 0.0 && s < len as f64).then_some(s as usize)
    };
    let cols: Vec<Option<usize>> = (0..dw).map(|x| source(x, tx, sw)).collect();
    for (y, row) in dst.data.chunks_exact_mut(dw * 4).enumerate() {
        let Some(sy) = source(y, ty, sh) else {
            for out in row.chunks_exact_mut(4) {
                out.copy_from_slice(&px);
            }
            continue;
        };
        let line = &src.data[sy * sw * 4..(sy + 1) * sw * 4];
        for (out, col) in row.chunks_exact_mut(4).zip(&cols) {
            match col {
                Some(sx) => out.copy_from_slice(&line[sx * 4..sx * 4 + 4]),
                None => out.copy_from_slice(&px),
            }
        }
    }
}

/// Renders shapes fitted into a `w` x `h` thumbnail (text is skipped when no
/// text host is installed). Returns RGBA pixels.
pub fn thumbnail(shapes: &[Arc<Shape>], w: u32, h: u32) -> Vec<u8> {
    let mut buf = vec![0u8; w as usize * h as usize * 4];
    let region = IRect::new(0, 0, w as i32, h as i32);
    let mut view = View { cam: Pt::ZERO, zoom: 1.0, dpr: 1.0 };
    if let Some(b) = crate::document::scene_bounds(shapes) {
        let zoom = f64::min(
            1.5,
            f64::min(w as f64 / f64::max(1.0, b.width() + 32.0), h as f64 / f64::max(1.0, b.height() + 32.0)),
        );
        let c = b.center();
        view = View { cam: Pt::new(w as f64 / 2.0 - c.x * zoom, h as f64 / 2.0 - c.y * zoom), zoom, dpr: 1.0 };
    }
    draw_view(shapes, &mut buf, region, view);
    buf
}

/// Renders shapes as seen through `view` into a `w` x `h` RGBA image.
pub fn view_image(shapes: &[Arc<Shape>], w: u32, h: u32, view: View) -> Vec<u8> {
    let mut buf = vec![0u8; w as usize * h as usize * 4];
    draw_view(shapes, &mut buf, IRect::new(0, 0, w as i32, h as i32), view);
    buf
}

fn draw_view(shapes: &[Arc<Shape>], buf: &mut [u8], region: IRect, view: View) {
    if let Some(mut t) = Target::new(buf, region, view) {
        draw_background(&mut t, false);
        for (parent, child) in mindmap::connections(shapes) {
            draw_connection(&mut t, parent.bounds(), child.bounds(), child.color);
        }
        for s in shapes {
            draw_shape(&mut t, s, 1.0, None);
        }
    }
}
