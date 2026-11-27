//! The interactive board: tools, gestures, camera and incremental rendering.
//!
//! Two screen-sized layers are kept:
//!  * `scene`  - background, grid and every committed shape for the current
//!               camera. It is only repainted where something changed, and a
//!               newly finished stroke is drawn straight on top of it, so
//!               adding ink costs the same on a full board as an empty one.
//!  * `frame`  - `scene` plus live feedback (the stroke being drawn, selection
//!               frames, shapes being dragged). This is what goes on screen.
//!
//! Input handlers only record *damage*; [`Editor::render`] repaints exactly
//! the damaged rectangle and returns it so the host can post just that part.

use crate::document::{scene_bounds, Applied, Document};
use crate::geom::{IRect, Pt, Rect};
use crate::math;
use crate::mindmap;
use crate::recognize::recognize_shape;
use crate::render::{self, Layer, Target, View};
use crate::shape::{Shape, Tool};
use crate::table::{self, Cell};
use crate::text;
use std::collections::{HashSet, VecDeque};
use std::sync::Arc;
use tiny_skia::Transform;

#[path = "math/editor.rs"]
mod math_editor;

pub const EV_STATE: u32 = 1; // tool, selection, undo/redo availability, style
pub const EV_DOC: u32 = 2; // shapes changed: schedule a save
pub const EV_TEXT: u32 = 4; // a text request is waiting
pub const EV_HAPTIC: u32 = 8; // a held stroke snapped to a shape
pub const EV_VIEW: u32 = 16; // zoom changed
pub const EV_INK: u32 = 32; // handwriting is waiting to be read

/// What the touch panel says about one contact.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Contact {
    /// Longest side of the touched area in buffer pixels; 0 when unknown.
    pub size: f64,
    pub stylus: bool,
    /// The back (eraser end) of a stylus.
    pub eraser: bool,
    /// Android's normalised size (`getSize`, raw width / panel range) from an
    /// IR panel that reports real widths; 0 when unknown. Measured on the
    /// Aaztec IR frame (raw units of 1/32767): pen tip ~15, pen back nib
    /// ~35, palm 300-650 (it lands smaller and grows as it settles).
    pub norm: f64,
}

/// [`Contact::norm`] bands: below `NIB_NORM` the pen tip (the Aaztec frame
/// reports 15), up to `NIB_MAX` a wide touch: the pen's back nib or a
/// finger, which the frame reports alike (35, with the same minor axis and
/// pressure), from `PALM_NORM` a palm or fist (ignored).
const NIB_NORM: f64 = 26.0 / 32767.0;
const NIB_MAX: f64 = 60.0 / 32767.0;
const PALM_NORM: f64 = 200.0 / 32767.0;

impl Contact {
    /// Back nib or finger, by width.
    fn wide(&self) -> bool {
        self.norm >= NIB_NORM && self.norm <= NIB_MAX
    }
    /// The pen tip, by width.
    fn tip(&self) -> bool {
        self.norm > 0.0 && self.norm < NIB_NORM
    }
    fn big(&self) -> bool {
        self.norm >= PALM_NORM
    }
}

/// Adaptive low-pass filter (One-Euro) for live ink: strong smoothing when
/// the hand moves slowly, where touch-panel jitter shows as wobble, and little
/// lag when it moves fast.
#[derive(Clone, Copy, Default)]
struct Smoother {
    /// Filtered position, filtered velocity (px/s) and time of the last sample.
    last: Option<(Pt, Pt, u64)>,
}

impl Smoother {
    /// Cut-off (Hz) when still.
    const MIN_CUTOFF: f64 = 2.0;
    /// How fast the cut-off rises with speed (per px/s).
    const BETA: f64 = 0.07;
    /// Cut-off (Hz) for the speed estimate.
    const D_CUTOFF: f64 = 5.0;

    fn reset(&mut self, p: Pt, t: u64) {
        self.last = Some((p, Pt::ZERO, t));
    }

    fn filter(&mut self, p: Pt, t: u64) -> Pt {
        let Some((prev, dprev, t0)) = self.last else {
            self.reset(p, t);
            return p;
        };
        let dt = t.saturating_sub(t0).max(1) as f64 / 1000.0;
        let alpha = |cutoff: f64| 1.0 / (1.0 + 1.0 / (2.0 * std::f64::consts::PI * cutoff * dt));
        let v = (p - prev) / dt;
        let dv = dprev + (v - dprev) * alpha(Self::D_CUTOFF);
        let out = prev + (p - prev) * alpha(Self::MIN_CUTOFF + Self::BETA * dv.len());
        self.last = Some((out, dv, t));
        out
    }
}

/// Longest predicted tail, in screen pixels.
const PREDICT_CAP: f64 = 48.0;

/// Where the pen will probably be after half of `horizon` ms and after all of
/// it, as offsets from the newest sample in `recent` (screen pixels, ms).
///
/// The guess follows an arc at the pen's current turn rate, so a curve is
/// predicted as a curve rather than a tangent that overshoots it. It is
/// shortened as the pen slows down or the turn tightens, and dropped at
/// corners and during slow, careful movement, where a wrong guess would show.
fn extrapolate(recent: &VecDeque<(Pt, u64)>, horizon: f64) -> Option<[Pt; 2]> {
    let n = recent.len();
    if n < 3 || horizon <= 0.0 {
        return None;
    }
    let ((p0, t0), (pm, tm), (p1, t1)) = (recent[0], recent[n / 2], recent[n - 1]);
    let (d0, d1) = (tm.saturating_sub(t0) as f64, t1.saturating_sub(tm) as f64);
    if d0 < 2.0 || d1 < 2.0 {
        return None;
    }
    // Velocity (px/ms) over the older and the newer half of the window.
    let (v0, v1) = ((pm - p0) / d0, (p1 - pm) / d1);
    let (s0, s1) = (v0.len(), v1.len());
    if s1 < 0.08 {
        return None;
    }
    // Turn rate (rad/ms) between the two halves.
    let turn = if s0 < 0.02 { 0.0 } else { (v0.x * v1.y - v0.y * v1.x).atan2(v0.x * v1.x + v0.y * v1.y) };
    if turn.abs() > 1.0 {
        return None; // turned more than ~57° within the window: a corner
    }
    let omega = turn / ((d0 + d1) / 2.0);
    // How much of the horizon to trust.
    let mut trust = 1.0;
    if s1 < s0 {
        trust *= s1 / s0; // slowing down, perhaps to stop or turn
    }
    let sweep = (omega * horizon).abs();
    if sweep > std::f64::consts::FRAC_PI_2 {
        return None; // turning too fast to guess where it goes
    }
    trust *= sweep.cos();
    let horizon = horizon * trust;
    if horizon < 2.0 {
        return None;
    }
    // v1 is the velocity half-way through the newer half: turn it to now,
    // then walk the arc.
    let rotate = |v: Pt, a: f64| Pt::new(v.x * a.cos() - v.y * a.sin(), v.x * a.sin() + v.y * a.cos());
    let v = rotate(v1, omega * d1 / 2.0);
    const STEPS: usize = 8;
    let dt = horizon / STEPS as f64;
    let mut at = Pt::ZERO;
    let mut half = Pt::ZERO;
    for i in 0..STEPS {
        at = at + rotate(v, omega * dt * (i as f64 + 0.5)) * dt;
        if i + 1 == STEPS / 2 {
            half = at;
        }
    }
    let len = at.len();
    if len > PREDICT_CAP {
        let k = PREDICT_CAP / len;
        return Some([half * k, at * k]);
    }
    Some([half, at])
}

/// A second stroke being written at the same time as the main one.
struct Extra {
    pointer: i32,
    id: String,
    points: Vec<Pt>,
    bounds: Rect,
    last_screen: Pt,
    /// When it touched down, in milliseconds.
    born: u64,
    smooth: Smoother,
}

/// Handwriting the host should read and answer with [`Editor::commit_ink`].
#[derive(Clone, Debug, PartialEq)]
pub struct InkRequest {
    pub token: u32,
    pub ids: Vec<String>,
    pub strokes: Vec<Vec<Pt>>,
    pub bounds: Rect,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PenMode {
    /// Keeps the stroke as ink.
    Ink,
    /// Replaces a rough outline with a clean shape as soon as the pen lifts.
    Shape,
    /// Reads everything written as typed text after a short pause.
    Write,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    None,
    Draw,
    Shape,
    Move,
    Resize,
    Marquee,
    Erase,
    Pan,
    Pinch,
    Text,
    Blocked,
    /// Two contacts landed together while writing; see [`Pair`].
    Pair,
}

/// Two contacts that came down together while writing: a two-finger pinch,
/// or a pen and the palm that landed with it. Told apart by how they move.
struct Pair {
    since: u64,
    /// Pointer, raw samples (logical pixels, time) and distance travelled.
    tracks: [(i32, Vec<(Pt, u64)>, f64); 2],
}

/// The host should ask the user for text and call [`Editor::commit_text`].
#[derive(Clone, Debug, PartialEq)]
pub struct TextRequest {
    /// Shape being edited, or empty for new text.
    pub id: String,
    pub x: f64,
    pub y: f64,
    pub text: String,
    pub cell: Option<Cell>,
}

/// How long the pen rests before handwriting is read, in milliseconds.
const INK_PAUSE: u64 = 1100;
/// A touch that has stayed put for this long (milliseconds) and then sees a
/// new contact land is a resting hand, not a writer.
const REST_MS: u64 = 220;
/// How far (logical pixels) a resting hand may drift.
const REST_TRAVEL: f64 = 12.0;
/// A mark held still this long (milliseconds) without moving is a hand that
/// rested and lifted, not a dot.
const REST_DOT_MS: u64 = 800;
/// A second contact this close (logical pixels; about 15 cm on a 4K board)
/// to the writer may be the hand holding the pen.
const PALM_REACH: f64 = 160.0;
/// The writer sits above the palm by at least this much (logical pixels).
const PALM_BELOW: f64 = 12.0;
/// A pair that lands together is decided after this long (ms) or travel.
const PAIR_MS: u64 = 120;
const PAIR_TRAVEL: f64 = 24.0;
/// Minimum change in finger spacing (logical pixels) and relative scale before
/// a two-finger gesture unlocks zoom; below this it stays a pure pan so the
/// cached scene slides instead of repainting the whole screen.
const PINCH_ZOOM_PX: f64 = 18.0;
const PINCH_ZOOM_RATIO: f64 = 0.08;

const EMPTY: IRect = IRect { x0: 0, y0: 0, x1: 0, y1: 0 };

pub struct Editor {
    pub doc: Document,
    tool: Tool,
    pen_mode: PenMode,
    color: u32,
    width: f64,
    filled: bool,
    grid: bool,
    snap_shapes: bool,
    auto_convert: bool,
    auto_text: bool,
    font: u8,
    palm_rejection: bool,
    /// Contacts at least this big (logical pixels) are a palm.
    palm_size: f64,
    /// Once a stylus has been used, fingers pan instead of drawing.
    pen_priority: bool,
    /// Several fingers can write at once.
    multi_ink: bool,
    /// A wide touch (pen back nib or finger) erases.
    wide_erases: bool,
    /// How far ahead (milliseconds) the live stroke is drawn.
    predict_ms: f64,
    /// Filter touch jitter out of live ink.
    smoothing: bool,
    smooth: Smoother,
    /// The unfiltered position of the drawing finger (logical pixels).
    raw_screen: Pt,
    stylus_seen: bool,
    tool_override: Option<Tool>,
    extras: Vec<Extra>,
    /// Predicted continuation of the live stroke, in world units.
    prediction: Vec<Pt>,
    /// Recent stroke samples (logical screen pixels, time) for prediction.
    recent: VecDeque<(Pt, u64)>,

    // Camera, in logical pixels (buffer pixels / dpr).
    zoom: f64,
    camera: Pt,
    dpr: f64,

    // Pointer state.
    pointers: Vec<(i32, Pt)>,
    ignored: Vec<i32>,
    mode: Mode,
    primary: Option<i32>,
    down_stamp: u64,
    travel: f64,
    start_world: Pt,
    anchor_world: Pt,
    start_zoom: f64,
    start_camera: Pt,
    pinch_distance: f64,
    /// True once finger spacing changes past the deadzone during a two-finger gesture.
    pinch_zooming: bool,
    /// Where the two pinch contacts first touched.
    pinch_origin: [Pt; 2],
    stylus: Option<i32>,
    stylus_lifted: Option<u64>,
    primary_size: f64,
    /// The primary contact is pen sized on an IR panel.
    primary_pen: bool,
    /// Raw samples of the primary contact since it landed (bounded).
    trail: Vec<(Pt, u64)>,
    pair: Option<Pair>,

    // Handwriting waiting to become text.
    pending: Vec<String>,
    pending_bounds: Rect,
    ink_deadline: Option<u64>,
    ink_requests: VecDeque<InkRequest>,
    ink_token: u32,
    /// The last stroke that was tidied into a shape on its own, kept so it
    /// can go back to ink if it turns out to be the first letter of a word.
    last_auto: Option<(Shape, u64)>,

    // In-progress ink.
    draft: Option<Vec<Pt>>,
    draft_bounds: Rect,
    draft_id: String,
    last_screen: Pt,
    still_anchor: Pt,
    snapped: Option<Shape>,
    snap_deadline: Option<u64>,
    draft_end: Pt,

    // Selection transforms and erasing.
    moving: Vec<Arc<Shape>>,
    dragging: bool,
    delta: Pt,
    resize_origin: Pt,
    resize_bounds: Rect,
    handle: usize,
    scale_x: f64,
    scale_y: f64,
    marquee: Option<Rect>,
    hidden: HashSet<String>,
    last_erase: Pt,
    last_text_tap: Option<(String, Option<Cell>)>,
    last_tap_time: u64,

    // Rendering.
    scene: Layer,
    frame: Layer,
    scene_view: Option<View>,
    /// Cached scene snapshot scaled during an active pinch-zoom gesture.
    pinch_snapshot: Option<(Layer, View)>,
    scene_damage: Vec<Rect>,
    damage: IRect,
    overlay_dirty: bool,
    overlay_prev: IRect,

    events: u32,
    text_request: Option<TextRequest>,
    client_id: String,
    sequence: u64,
    /// Number of full scene repaints, for tests and diagnostics.
    pub full_repaints: u64,
}

fn base36(mut n: u128) -> String {
    let digits = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut out = Vec::new();
    loop {
        out.push(digits[(n % 36) as usize]);
        n /= 36;
        if n == 0 {
            break;
        }
    }
    out.reverse();
    String::from_utf8(out).unwrap()
}

fn micros() -> u128 {
    // Distinct even when called twice in the same millisecond.
    static TICK: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    crate::sys::now_ms() as u128 * 1000 + (TICK.fetch_add(1, std::sync::atomic::Ordering::Relaxed) % 1000) as u128
}

fn corners(r: &Rect) -> [Pt; 4] {
    [r.top_left(), r.top_right(), r.bottom_left(), r.bottom_right()]
}

fn scaled(s: &Shape, origin: Pt, sx: f64, sy: f64) -> Shape {
    let points = s
        .points
        .iter()
        .map(|p| Pt::new(origin.x + (p.x - origin.x) * sx, origin.y + (p.y - origin.y) * sy))
        .collect();
    let mut next = s.with_points(points);
    if matches!(s.tool, Tool::Text | Tool::Formula) {
        let size = (s.font_size * sy).clamp(6.0, 2000.0);
        next = next.with_text(s.text.clone(), size);
    }
    next
}

impl Default for Editor {
    fn default() -> Self {
        Editor::new()
    }
}

impl Editor {
    pub fn new() -> Editor {
        let seed = micros() ^ ((&EMPTY as *const IRect as usize as u128) << 20);
        Editor {
            doc: Document::new(),
            tool: Tool::Pen,
            pen_mode: PenMode::Ink,
            color: 0xff1d1d1d,
            width: 3.5,
            filled: false,
            grid: true,
            snap_shapes: true,
            auto_convert: true,
            auto_text: true,
            font: 0,
            palm_rejection: true,
            palm_size: 52.0,
            pen_priority: false,
            multi_ink: false,
            wide_erases: true,
            predict_ms: 0.0,
            smoothing: true,
            smooth: Smoother::default(),
            raw_screen: Pt::ZERO,
            stylus_seen: false,
            tool_override: None,
            extras: Vec::new(),
            prediction: Vec::new(),
            recent: VecDeque::new(),
            zoom: 1.0,
            camera: Pt::ZERO,
            dpr: 1.0,
            pointers: Vec::new(),
            ignored: Vec::new(),
            mode: Mode::None,
            primary: None,
            down_stamp: 0,
            travel: 0.0,
            start_world: Pt::ZERO,
            anchor_world: Pt::ZERO,
            start_zoom: 1.0,
            start_camera: Pt::ZERO,
            pinch_distance: 1.0,
            pinch_zooming: false,
            pinch_origin: [Pt::ZERO; 2],
            stylus: None,
            stylus_lifted: None,
            primary_size: 0.0,
            primary_pen: false,
            trail: Vec::new(),
            pair: None,
            pending: Vec::new(),
            pending_bounds: Rect::ZERO,
            ink_deadline: None,
            ink_requests: VecDeque::new(),
            ink_token: 0,
            last_auto: None,
            draft: None,
            draft_bounds: Rect::ZERO,
            draft_id: String::new(),
            last_screen: Pt::ZERO,
            still_anchor: Pt::ZERO,
            snapped: None,
            snap_deadline: None,
            draft_end: Pt::ZERO,
            moving: Vec::new(),
            dragging: false,
            delta: Pt::ZERO,
            resize_origin: Pt::ZERO,
            resize_bounds: Rect::ZERO,
            handle: 0,
            scale_x: 1.0,
            scale_y: 1.0,
            marquee: None,
            hidden: HashSet::new(),
            last_erase: Pt::ZERO,
            last_text_tap: None,
            last_tap_time: 0,
            scene: Layer::new(0, 0),
            frame: Layer::new(0, 0),
            scene_view: None,
            pinch_snapshot: None,
            scene_damage: Vec::new(),
            damage: EMPTY,
            overlay_dirty: false,
            overlay_prev: EMPTY,
            events: 0,
            text_request: None,
            client_id: format!("r{}", base36(seed & 0xffff_ffff_ffff)),
            sequence: 0,
            full_repaints: 0,
        }
    }

    // ------------------------------------------------------------ accessors

    pub fn tool(&self) -> Tool {
        self.tool
    }
    pub fn pen_mode(&self) -> PenMode {
        self.pen_mode
    }
    pub fn color(&self) -> u32 {
        self.color
    }
    pub fn stroke_width(&self) -> f64 {
        self.width
    }
    pub fn filled(&self) -> bool {
        self.filled
    }
    pub fn grid(&self) -> bool {
        self.grid
    }
    pub fn snap_shapes(&self) -> bool {
        self.snap_shapes
    }
    pub fn zoom(&self) -> f64 {
        self.zoom
    }
    pub fn auto_convert(&self) -> bool {
        self.auto_convert
    }
    pub fn font(&self) -> u8 {
        self.font
    }
    pub fn palm_rejection(&self) -> bool {
        self.palm_rejection
    }
    pub fn selected_text_count(&self) -> usize {
        self.doc.selected_shapes().iter().filter(|s| matches!(s.tool, Tool::Text | Tool::Formula)).count()
    }
    pub fn take_ink_request(&mut self) -> Option<InkRequest> {
        self.ink_requests.pop_front()
    }
    pub fn camera(&self) -> Pt {
        self.camera
    }
    /// Moves the view by `d` and zooms by `scale` about `focus`, both in
    /// buffer pixels, as a two finger gesture would.
    pub fn pan_zoom(&mut self, d: Pt, scale: f64, focus: Pt) {
        let focus = focus / self.dpr;
        let anchor = self.world(focus);
        self.zoom = (self.zoom * scale).clamp(0.1, 8.0);
        self.camera = focus - anchor * self.zoom + d / self.dpr;
        self.view_changed();
    }

    /// Shows the board from `camera` at `zoom`, as saved with a page.
    pub fn set_view(&mut self, camera: Pt, zoom: f64) {
        self.camera = camera;
        self.zoom = zoom.clamp(0.1, 8.0);
        self.view_changed();
    }
    pub fn pointer_down_now(&self) -> bool {
        !self.pointers.is_empty() || !self.ignored.is_empty() || !self.extras.is_empty()
    }
    pub fn frame(&self) -> &Layer {
        &self.frame
    }
    /// Events raised since the last call.
    pub fn take_events(&mut self) -> u32 {
        std::mem::take(&mut self.events)
    }
    pub fn take_text_request(&mut self) -> Option<TextRequest> {
        self.text_request.take()
    }
    /// When [`Editor::tick`] next needs to run, in the caller's milliseconds.
    pub fn next_deadline(&self) -> Option<u64> {
        match (self.snap_deadline, self.ink_deadline) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// How the board maps to screen pixels.
    pub fn screen_view(&self) -> View {
        self.view()
    }
    fn view(&self) -> View {
        View { cam: self.camera, zoom: self.zoom, dpr: self.dpr }
    }
    fn viewport(&self) -> (f64, f64) {
        (self.scene.w as f64 / self.dpr, self.scene.h as f64 / self.dpr)
    }
    pub fn world(&self, screen: Pt) -> Pt {
        (screen - self.camera) / self.zoom
    }
    pub(crate) fn next_id(&mut self) -> String {
        self.sequence += 1;
        format!("{}-{}-{}", self.client_id, base36(micros()), self.sequence - 1)
    }

    // ------------------------------------------------------- host commands

    /// Sets the buffer size in pixels and how many pixels make one logical
    /// pixel (the unit board coordinates are stored in at zoom 1).
    pub fn resize(&mut self, w: u32, h: u32, dpr: f64) {
        if self.scene.w == w && self.scene.h == h && self.dpr == dpr {
            return;
        }
        self.dpr = if dpr.is_finite() && dpr > 0.0 { dpr } else { 1.0 };
        self.scene = Layer::new(w, h);
        self.frame = Layer::new(w, h);
        self.pinch_snapshot = None;
        self.invalidate_all();
    }

    /// Forgets cached pixels, for example after the surface was recreated.
    pub fn invalidate_all(&mut self) {
        self.scene_view = None;
        self.pinch_snapshot = None;
        self.overlay_dirty = true;
    }

    pub fn load(&mut self, shapes: Vec<Shape>) -> Result<(), String> {
        self.cancel_action();
        self.pointers.clear();
        self.ignored.clear();
        self.mode = Mode::None;
        self.forget_pending();
        self.ink_requests.clear();
        self.doc.load(shapes)?;
        self.invalidate_all();
        self.events |= EV_STATE;
        Ok(())
    }

    /// Brings in shapes changed on another device: `put` adds or replaces,
    /// `remove` deletes. Others' changes are not undone by this person's undo.
    pub fn apply_remote(&mut self, put: Vec<Shape>, remove: Vec<String>) {
        self.merge(put, remove, false);
    }

    /// Adds or replaces `put` and deletes `remove` as one undo step, for
    /// shapes made outside the editor (imports, Claude).
    pub fn apply_shapes(&mut self, put: Vec<Shape>, remove: Vec<String>) {
        self.merge(put, remove, true);
    }

    fn merge(&mut self, put: Vec<Shape>, remove: Vec<String>, undoable: bool) {
        let mut drop: Vec<String> = remove;
        let mut add: Vec<Shape> = Vec::new();
        for s in put {
            if self.doc.get(&s.id).is_some_and(|old| **old == s) {
                continue;
            }
            if self.doc.contains(&s.id) {
                drop.push(s.id.clone());
            }
            add.push(s);
        }
        if drop.is_empty() && add.is_empty() {
            return;
        }
        let applied = if undoable { self.doc.swap(drop, add) } else { self.doc.apply_remote(drop, add) };
        self.on_applied(applied);
    }

    pub fn set_tool(&mut self, tool: Tool, pen_mode: PenMode) {
        self.cancel_action();
        self.flush_pending();
        self.last_auto = None;
        self.tool = tool;
        if tool == Tool::Pen {
            self.pen_mode = pen_mode;
        }
        if tool != Tool::Select {
            self.doc.select(None);
        }
        self.overlay_dirty = true;
        self.events |= EV_STATE;
    }

    /// Changes the current style and restyles the selection.
    pub fn apply_style(&mut self, color: Option<u32>, width: Option<f64>, filled: Option<bool>) {
        self.color = color.unwrap_or(self.color);
        self.width = width.filter(|w| w.is_finite() && *w > 0.0).unwrap_or(self.width);
        self.filled = filled.unwrap_or(self.filled);
        let restyled: Vec<Shape> = self
            .doc
            .selected_shapes()
            .iter()
            .map(|s| {
                let mut n = (**s).clone();
                n.color = color.unwrap_or(n.color);
                n.width = width.unwrap_or(n.width);
                n.filled = filled.unwrap_or(n.filled);
                n
            })
            .filter(|n| !self.doc.get(&n.id).is_some_and(|old| **old == *n))
            .collect();
        if !restyled.is_empty() {
            let applied = self.doc.replace_many(restyled);
            self.on_applied(applied);
        }
        self.events |= EV_STATE;
    }

    pub fn set_grid(&mut self, grid: bool) {
        if self.grid != grid {
            self.grid = grid;
            self.invalidate_all();
        }
    }

    pub fn set_snap_shapes(&mut self, snap: bool) {
        self.snap_shapes = snap;
    }

    /// When on, pen strokes that look like a shape become that shape.
    pub fn set_auto_convert(&mut self, auto: bool) {
        self.auto_convert = auto;
        self.events |= EV_STATE;
    }

    /// When on, handwriting becomes typed text after a short pause.
    pub fn set_auto_text(&mut self, auto: bool) {
        if !auto {
            self.forget_pending();
        }
        self.auto_text = auto;
        self.events |= EV_STATE;
    }

    pub fn auto_text(&self) -> bool {
        self.auto_text
    }

    /// `size` is the smallest contact, in logical pixels, treated as a palm.
    pub fn set_palm_rejection(&mut self, on: bool, size: Option<f64>) {
        self.palm_rejection = on;
        if let Some(size) = size.filter(|s| s.is_finite() && *s >= 8.0) {
            self.palm_size = size;
        }
        self.events |= EV_STATE;
    }

    /// Chooses the font for new text and for the selected text.
    /// With a stylus in use, fingers pan and zoom instead of drawing.
    pub fn set_pen_priority(&mut self, on: bool) {
        self.pen_priority = on;
    }

    /// Lets several fingers write separate strokes at the same time.
    /// Filters touch-panel jitter out of freehand ink.
    pub fn set_smoothing(&mut self, on: bool) {
        self.smoothing = on;
    }

    pub fn smoothing(&self) -> bool {
        self.smoothing
    }

    pub fn set_multi_ink(&mut self, on: bool) {
        self.multi_ink = on;
    }

    /// Whether a wide touch on an IR panel (the pen's back nib, or a finger:
    /// the frame cannot tell them apart) erases instead of drawing.
    pub fn set_wide_erases(&mut self, on: bool) {
        self.wide_erases = on;
    }

    /// The contact erases: a stylus eraser, or a wide touch when set to.
    /// When several fingers write at once a wide touch is a finger, which
    /// writes: the IR frame reports fingers and the back nib alike.
    fn erases(&self, c: Contact) -> bool {
        c.eraser || (self.wide_touch_erases() && c.wide())
    }

    fn wide_touch_erases(&self) -> bool {
        self.wide_erases && !self.multi_ink
    }

    /// The contact is the pen, by width: its tip, or its back nib when wide
    /// touches erase.
    fn pen_sized(&self, c: Contact) -> bool {
        c.tip() || (self.wide_touch_erases() && c.wide())
    }

    /// Draws the live stroke `ms` milliseconds ahead of the pen (0 = off).
    pub fn set_prediction(&mut self, ms: f64) {
        self.predict_ms = ms.clamp(0.0, 60.0);
    }

    pub fn set_font(&mut self, font: u8) {
        if font as usize >= text::FONTS.len() {
            return;
        }
        self.font = font;
        let changed: Vec<Shape> = self
            .doc
            .selected_shapes()
            .iter()
            .filter(|s| matches!(s.tool, Tool::Text | Tool::Formula) && s.font != font)
            .map(|s| (**s).clone().with_font(font))
            .collect();
        if !changed.is_empty() {
            let applied = self.doc.replace_many(changed);
            self.on_applied(applied);
        }
        self.events |= EV_STATE;
    }

    /// Makes the selected text larger or smaller.
    pub fn scale_text(&mut self, factor: f64) {
        if !factor.is_finite() || factor <= 0.0 {
            return;
        }
        let changed: Vec<Shape> = self
            .doc
            .selected_shapes()
            .iter()
            .filter(|s| matches!(s.tool, Tool::Text | Tool::Formula))
            .map(|s| (**s).clone().with_text(s.text.clone(), (s.font_size * factor).clamp(6.0, 2000.0)))
            .collect();
        if !changed.is_empty() {
            let applied = self.doc.replace_many(changed);
            self.on_applied(applied);
        }
    }

    pub fn undo(&mut self) {
        if self.mode != Mode::None {
            return;
        }
        self.forget_pending();
        let applied = self.doc.undo();
        self.on_applied(applied);
    }

    pub fn redo(&mut self) {
        if self.mode != Mode::None {
            return;
        }
        self.forget_pending();
        let applied = self.doc.redo();
        self.on_applied(applied);
    }

    pub fn delete_selection(&mut self) {
        let ids: Vec<String> = self.doc.selected_ids().to_vec();
        self.delete_shapes(ids);
    }

    pub(crate) fn delete_shapes(&mut self, ids: Vec<String>) {
        let applied = self.doc.remove_many(ids);
        self.on_applied(applied);
    }

    pub(crate) fn insert_table(&mut self, rows: usize, columns: usize) {
        let (w, h) = self.viewport();
        let center = self.world(Pt::new(w / 2.0, h / 2.0));
        let shape = table::centered(self.next_id(), center, rows, columns).with_font(self.font);
        self.insert_selected(shape);
    }

    pub(crate) fn resize_table_grid(&mut self, row_delta: isize, column_delta: isize) {
        let [id] = self.doc.selected_ids() else { return };
        let Some(shape) = self.doc.get(id) else { return };
        let Some(table) = shape.table.as_deref() else { return };
        let rows = table.rows().saturating_add_signed(row_delta).clamp(1, table::MAX_SIZE);
        let columns = table.columns().saturating_add_signed(column_delta).clamp(1, table::MAX_SIZE);
        if (rows, columns) == (table.rows(), table.columns()) {
            return;
        }
        let bounds = shape.bounds();
        let size = table.cell_size(bounds);
        let start = bounds.top_left();
        let end = start + Pt::new(size.x * columns as f64, size.y * rows as f64);
        let next = shape.with_points(vec![start, end]).with_table(table.resized(rows, columns));
        let applied = self.doc.replace(next);
        self.on_applied(applied);
    }

    pub(crate) fn insert_mind_map(&mut self) {
        let (w, h) = self.viewport();
        let center = self.world(Pt::new(w / 2.0, h / 2.0));
        let shape = mindmap::root(self.next_id(), center).with_font(self.font);
        self.insert_selected(shape);
    }

    pub(crate) fn add_mind_topic(&mut self, sibling: bool) {
        let [selected] = self.doc.selected_ids() else { return };
        let Some(node) = self.doc.get(selected).filter(|s| s.tool == Tool::MindNode) else { return };
        let parent = if sibling {
            node.parent_id.as_deref().and_then(|id| self.doc.get(id)).filter(|s| s.tool == Tool::MindNode)
        } else {
            Some(node)
        };
        let Some(parent) = parent.cloned() else { return };
        let id = self.next_id();
        let shape = mindmap::child(self.doc.shapes(), &parent, id).with_font(parent.font);
        self.insert_selected(shape);
    }

    fn insert_selected(&mut self, shape: Shape) {
        if self.tool != Tool::Select {
            self.set_tool(Tool::Select, PenMode::Ink);
        }
        let id = shape.id.clone();
        let applied = self.doc.add(shape);
        self.doc.select(Some(&id));
        self.on_applied(applied);
    }

    /// Fits the whole board in view.
    pub fn fit(&mut self) {
        let (vw, vh) = self.viewport();
        match self.doc.bounds() {
            Some(b) if vw > 0.0 && vh > 0.0 => {
                self.zoom = f64::min(
                    (vw - 160.0) / f64::max(b.width(), 1.0),
                    (vh - 200.0) / f64::max(b.height(), 1.0),
                )
                .clamp(0.1, 2.0);
                self.camera = Pt::new(vw / 2.0, vh / 2.0) - b.center() * self.zoom;
            }
            _ => {
                self.zoom = 1.0;
                self.camera = Pt::ZERO;
            }
        }
        self.view_changed();
    }

    /// Zooms about `focus` (logical screen point), or the centre of the view.
    pub fn zoom_to(&mut self, next: f64, focus: Option<Pt>) {
        let (vw, vh) = self.viewport();
        let point = focus.unwrap_or(Pt::new(vw / 2.0, vh / 2.0));
        let anchor = self.world(point);
        self.zoom = next.clamp(0.1, 8.0);
        self.camera = point - anchor * self.zoom;
        self.view_changed();
    }

    /// Adds new text at world (`x`, `y`), or changes the text of shape `id`.
    pub fn commit_text(&mut self, id: &str, x: f64, y: f64, text: &str) {
        if let Some(tool) = self.doc.get(id).map(|shape| shape.tool).filter(|t| matches!(t, Tool::Formula | Tool::Graph)) {
            let _ = self.commit_math(id, tool, text);
            return;
        }
        if !id.is_empty() {
            if let Some(existing) = self.doc.get(id).cloned() {
                if let Some(table) = existing.table.as_deref() {
                    if let Some(cell) = table.cell_at(existing.bounds(), Pt::new(x, y)) {
                        self.commit_cell_text(id, cell, text);
                    }
                } else if (existing.tool == Tool::MindNode || !text.is_empty()) && existing.text != text {
                    let mut next = (*existing).clone();
                    let calculation = (existing.tool == Tool::Text).then(|| math::calculation_source(text)).flatten();
                    if calculation.is_some() {
                        next.tool = Tool::Formula;
                    }
                    let next = next.with_text(calculation.as_deref().unwrap_or(text), existing.font_size);
                    let applied = self.doc.replace(next);
                    self.on_applied(applied);
                }
            }
            return;
        }
        if text.is_empty() {
            return;
        }
        let id = self.next_id();
        let size = (28.0 / self.zoom).clamp(12.0, 400.0);
        let calculation = math::calculation_source(text);
        let tool = if calculation.is_some() { Tool::Formula } else { Tool::Text };
        let shape = Shape::new(id.clone(), tool, vec![Pt::new(x, y)])
            .styled(self.color, 3.0, false)
            .with_font(self.font)
            .with_text(calculation.as_deref().unwrap_or(text), size);
        let applied = self.doc.add(shape);
        self.on_applied(applied);
        self.tool = Tool::Select;
        self.doc.select(Some(&id));
        self.overlay_dirty = true;
        self.events |= EV_STATE;
    }

    pub(crate) fn commit_cell_text(&mut self, id: &str, cell: Cell, text: &str) {
        let Some(shape) = self.doc.get(id) else { return };
        let mut next = (**shape).clone();
        if next.table.as_mut().is_some_and(|table| table.set_text(cell, text)) {
            let applied = self.doc.replace(next);
            self.on_applied(applied);
        }
    }

    // -------------------------------------------------------------- pointers

    /// `pos` is in buffer pixels; `time` is a monotonic clock in milliseconds.
    pub fn pointer_down(&mut self, id: i32, pos: Pt, time: u64) {
        self.pointer_down_with(id, pos, time, Contact::default());
    }

    fn is_palm(&self, contact: Contact) -> bool {
        self.palm_rejection && !contact.stylus && (contact.size / self.dpr >= self.palm_size || contact.big())
    }

    /// Stops whatever the touches on the panel were doing and ignores them
    /// until they lift.
    fn dismiss_pointers(&mut self) {
        self.cancel_action();
        let ids: Vec<i32> = self.pointers.drain(..).map(|(id, _)| id).collect();
        self.ignored.extend(ids);
        for e in std::mem::take(&mut self.extras) {
            self.ignored.push(e.pointer);
            self.damage_world(&e.bounds, self.width / 2.0);
        }
        self.pinch_zooming = false;
        self.end_pinch_preview();
        self.mode = Mode::None;
        self.primary = None;
    }

    fn end_pinch_preview(&mut self) {
        if self.pinch_snapshot.take().is_some() {
            self.damage = self.scene.bounds();
            self.overlay_dirty = true;
        }
    }

    fn update_pinch(&mut self) {
        if self.pointers.len() < 2 {
            return;
        }
        let (a, b) = (self.pointers[0].1, self.pointers[1].1);
        let focal = (a + b) / 2.0;
        let dist = a.dist(b);
        if !self.pinch_zooming {
            let delta = (dist - self.pinch_distance).abs();
            let ratio = (dist / self.pinch_distance - 1.0).abs();
            if delta >= PINCH_ZOOM_PX && ratio >= PINCH_ZOOM_RATIO {
                self.pinch_zooming = true;
            }
        }
        self.zoom = if self.pinch_zooming {
            (self.start_zoom * dist / self.pinch_distance).clamp(0.1, 8.0)
        } else {
            self.start_zoom
        };
        self.camera = focal - self.anchor_world * self.zoom;
        self.view_changed();
    }

    /// How far (logical pixels) an extra stroke has spread.
    fn extra_travel(&self, e: &Extra) -> f64 {
        (e.bounds.width() + e.bounds.height()) * self.zoom
    }

    /// Ignores, until they lift, the touches that have lain still since they
    /// landed: a resting hand. Whatever they drew or zoomed is undone.
    fn drop_resting(&mut self, time: u64) {
        let rests = |born: u64, travel: f64| time.saturating_sub(born) >= REST_MS && travel < REST_TRAVEL;
        let primary_rests = match self.mode {
            Mode::Draw | Mode::Erase | Mode::Shape => self.pointers.len() == 1 && rests(self.down_stamp, self.travel),
            Mode::Pinch => {
                self.pointers.len() == 2
                    && self.pointers.iter().zip(self.pinch_origin).all(|((_, p), o)| rests(self.down_stamp, p.dist(o)))
            }
            _ => false,
        };
        if primary_rests {
            if self.mode == Mode::Pinch {
                self.zoom = self.start_zoom;
                self.camera = self.start_camera;
                self.view_changed();
            }
            self.cancel_action();
            let ids: Vec<i32> = self.pointers.drain(..).map(|(id, _)| id).collect();
            self.ignored.extend(ids);
            self.mode = Mode::None;
            self.primary = None;
        }
        let mut i = 0;
        while i < self.extras.len() {
            if rests(self.extras[i].born, self.extra_travel(&self.extras[i])) {
                let e = self.extras.remove(i);
                self.ignored.push(e.pointer);
                self.damage_world(&e.bounds, self.width / 2.0);
                self.overlay_dirty = true;
            } else {
                i += 1;
            }
        }
    }

    pub fn pointer_down_with(&mut self, id: i32, pos: Pt, time: u64, contact: Contact) {
        let pos = pos / self.dpr;
        if contact.stylus {
            self.stylus_seen = true;
        }
        if self.palm_rejection {
            if contact.stylus {
                // The pen always wins: anything already touching is the hand
                // holding it.
                if self.stylus.is_none() && !self.pointers.is_empty() {
                    self.dismiss_pointers();
                }
                self.stylus = Some(id);
            } else if self.stylus.is_some() || self.stylus_lifted.is_some_and(|t| time.saturating_sub(t) < 350) {
                self.ignored.push(id);
                return;
            }
            if self.is_palm(contact) {
                self.ignored.push(id);
                return;
            }
            // A contact far bigger than the finger that is drawing is a palm.
            let size = contact.size / self.dpr;
            if !self.pointers.is_empty() && self.primary_size > 0.0 && size >= self.primary_size * 2.5 && size >= 28.0 {
                self.ignored.push(id);
                return;
            }
            // While the pen (tip or nib, known by width) is down, anything
            // else near it is the hand or the frame seeing the pen twice.
            // A second pen well away from it is another writer.
            let near = self.pointers.iter().any(|(_, q)| q.dist(pos) < PALM_REACH);
            if self.primary_pen && !self.pointers.is_empty() && !contact.stylus && (near || !self.pen_sized(contact)) {
                self.ignored.push(id);
                return;
            }
            // Most people rest the hand first and then write: what has been
            // lying still is that hand, and the new touch is the writer.
            if !contact.stylus {
                self.drop_resting(time);
            }
        }
        if self.pointers.is_empty() {
            self.ignored.clear();
            if self.mode == Mode::Blocked {
                self.mode = Mode::None;
            }
        }
        let fingers_pan = self.pen_priority && self.stylus_seen && !contact.stylus;
        if self.palm_rejection && !contact.stylus && self.writer_hand(id, pos, time) {
            return;
        }
        if self.multi_ink && self.mode == Mode::Draw && !fingers_pan && !self.erases(contact) && self.extras.len() < 9 {
            // Another writer: start a stroke of its own.
            let w = self.world(pos);
            let shape_id = self.next_id();
            self.extras.push(Extra {
                pointer: id,
                id: shape_id,
                points: vec![w],
                bounds: Rect::from_points(w, w),
                last_screen: pos,
                born: time,
                smooth: Smoother { last: Some((pos, Pt::ZERO, time)) },
            });
            self.damage_world(&Rect::from_points(w, w), self.width / 2.0);
            return;
        }
        if self.mode == Mode::Blocked || self.pointers.len() >= 2 {
            self.ignored.push(id);
            return;
        }
        if self.pointers.len() == 1 {
            // A second touch only becomes a pinch if it lands right as the
            // gesture starts. Later contacts (a resting palm or sleeve on a
            // large touch panel) are ignored so they cannot interrupt a stroke.
            let early = time.saturating_sub(self.down_stamp) < 220 && self.travel < 36.0;
            let interruptible = matches!(self.mode, Mode::Pan | Mode::None | Mode::Text | Mode::Marquee);
            if !interruptible && !early {
                self.ignored.push(id);
                return;
            }
            self.cancel_action();
            self.pointers.push((id, pos));
            let (a, b) = (self.pointers[0].1, self.pointers[1].1);
            self.pinch_distance = f64::max(24.0, a.dist(b));
            self.anchor_world = self.world((a + b) / 2.0);
            self.start_zoom = self.zoom;
            self.start_camera = self.camera;
            self.pinch_zooming = false;
            self.pinch_snapshot = None;
            self.pinch_origin = [a, b];
            self.mode = Mode::Pinch;
            return;
        }
        self.pointers.push((id, pos));
        self.primary = Some(id);
        self.primary_size = contact.size / self.dpr;
        self.primary_pen = self.pen_sized(contact);
        self.down_stamp = time;
        self.travel = 0.0;
        self.trail.clear();
        self.trail.push((pos, time));
        let draws = !matches!(self.tool, Tool::Select | Tool::Hand);
        self.tool_override = if self.erases(contact) && draws {
            Some(Tool::Eraser)
        } else if fingers_pan && draws {
            Some(Tool::Hand)
        } else {
            None
        };
        self.begin(pos, time);
    }

    /// Palm rejection by position and motion, for touch panels that report
    /// every contact at the same size. Returns true when the new contact has
    /// been dealt with (ignored, or paired with the writer).
    ///
    /// - Landing together with the writer: wait and see which one writes.
    /// - Landing later just below or beside the writer: the palm.
    /// - Landing later just above the writer: the pen, and what was drawing
    ///   is the hand that came down first.
    fn writer_hand(&mut self, id: i32, pos: Pt, time: u64) -> bool {
        if self.multi_ink && self.mode == Mode::Draw && self.extras.iter().any(|e| e.last_screen.dist(pos) < PALM_REACH) {
            self.ignored.push(id);
            return true;
        }
        if !matches!(self.mode, Mode::Draw | Mode::Shape) || self.pointers.len() != 1 {
            return false;
        }
        let (first, first_pos) = self.pointers[0];
        if first_pos.dist(pos) >= PALM_REACH {
            return false;
        }
        let early = time.saturating_sub(self.down_stamp) < 220 && self.travel < 36.0;
        if early {
            let mut trail = std::mem::take(&mut self.trail);
            if trail.is_empty() {
                trail.push((first_pos, self.down_stamp));
            }
            let travelled = self.travel;
            self.cancel_action();
            self.mode = Mode::Pair;
            self.pointers.push((id, pos));
            self.pair = Some(Pair { since: time, tracks: [(first, trail, travelled), (id, vec![(pos, time)], 0.0)] });
            return true;
        }
        if pos.y < first_pos.y - PALM_BELOW {
            // The hand came down first and is dragging along: drop what it
            // drew and let the newcomer write.
            self.cancel_action();
            self.ignored.push(first);
            self.pointers.clear();
            self.primary = None;
            self.mode = Mode::None;
            return false;
        }
        self.ignored.push(id);
        true
    }

    fn pair_sample(&mut self, id: i32, pos: Pt, time: u64) {
        if let Some(t) = self.pair.as_mut().and_then(|p| p.tracks.iter_mut().find(|t| t.0 == id)) {
            if let Some(&(last, _)) = t.1.last() {
                t.2 += pos.dist(last);
            }
            if t.1.len() < 256 {
                t.1.push((pos, time));
            }
        }
        if let Some(slot) = self.pointers.iter_mut().find(|(p, _)| *p == id) {
            slot.1 = pos;
        }
    }

    /// Settles a [`Pair`]: both moving is a pinch; otherwise the one that
    /// moves (or, if neither has, the upper one) writes and the other is the
    /// palm. The writer's samples so far are replayed so no ink is lost.
    fn decide_pair(&mut self, time: u64, force: bool) {
        let Some(pair) = &self.pair else { return };
        let (ta, tb) = (pair.tracks[0].2, pair.tracks[1].2);
        let (most, least) = (ta.max(tb), ta.min(tb));
        let pinch = least >= 10.0 && least >= 0.4 * most;
        // Fingers of a pinch rarely move in the same frame: only an obvious
        // pinch, or one contact far ahead of the other, settles it early.
        let settled = pinch && most >= PAIR_TRAVEL || most >= 3.0 * PAIR_TRAVEL && least < 10.0;
        if !force && !settled && time.saturating_sub(pair.since) < PAIR_MS {
            return;
        }
        let Some(Pair { since, tracks: [a, b] }) = self.pair.take() else { return };
        if pinch && self.pointers.len() == 2 {
            let at = |t: &(i32, Vec<(Pt, u64)>, f64)| {
                t.1.iter().rev().find(|s| s.1 <= since).or(t.1.first()).map_or(Pt::ZERO, |s| s.0)
            };
            let (pa, pb) = (at(&a), at(&b));
            self.pointers.sort_by_key(|(p, _)| if *p == a.0 { 0 } else { 1 });
            self.pinch_distance = f64::max(24.0, pa.dist(pb));
            self.anchor_world = self.world((pa + pb) / 2.0);
            self.start_zoom = self.zoom;
            self.start_camera = self.camera;
            self.pinch_zooming = false;
            self.pinch_snapshot = None;
            self.pinch_origin = [pa, pb];
            self.mode = Mode::Pinch;
            self.update_pinch();
            return;
        }
        let last_y = |t: &(i32, Vec<(Pt, u64)>, f64)| t.1.last().map_or(0.0, |s| s.0.y);
        let a_writes = if most < 10.0 { last_y(&a) <= last_y(&b) } else { ta >= tb };
        let (writer, palm) = if a_writes { (a, b) } else { (b, a) };
        self.ignored.push(palm.0);
        self.pointers.retain(|(p, _)| *p == writer.0);
        let Some(&(p0, t0)) = writer.1.first() else { return };
        if !self.pointers.iter().any(|(p, _)| *p == writer.0) {
            // The writer already lifted; its up will finish the stroke.
            self.pointers.push((writer.0, writer.1.last().map_or(p0, |s| s.0)));
        }
        self.mode = Mode::None;
        self.primary = Some(writer.0);
        self.down_stamp = t0;
        self.travel = 0.0;
        self.begin(p0, t0);
        let mut prev = p0;
        for &(p, t) in &writer.1[1..] {
            self.travel += p.dist(prev);
            prev = p;
            self.raw_screen = p;
            let q = if self.smoothing && self.mode == Mode::Draw { self.smooth.filter(p, t) } else { p };
            self.continue_(q, t);
        }
        self.trail = writer.1;
    }

    pub fn pointer_move(&mut self, id: i32, pos: Pt, time: u64) {
        self.pointer_move_with(id, pos, time, Contact::default());
    }

    pub fn pointer_move_with(&mut self, id: i32, pos: Pt, time: u64, contact: Contact) {
        let pos = pos / self.dpr;
        if self.ignored.contains(&id) {
            return;
        }
        if self.mode == Mode::Pair {
            self.pair_sample(id, pos, time);
            self.decide_pair(time, false);
            return;
        }
        if let Some(i) = self.extras.iter().position(|e| e.pointer == id) {
            if self.is_palm(contact) && self.extra_travel(&self.extras[i]) < 2.0 * REST_TRAVEL {
                let e = self.extras.remove(i);
                self.ignored.push(id);
                self.damage_world(&e.bounds, self.width / 2.0);
                self.overlay_dirty = true;
                return;
            }
            let pos = if self.smoothing { self.extras[i].smooth.filter(pos, time) } else { pos };
            self.extend_extra(i, pos);
            return;
        }
        // A hand that lands edge first only reports its full size a moment
        // later, and one that rests may spread at any time: drop what it
        // started.
        if self.is_palm(contact)
            && (contact.big() || time.saturating_sub(self.down_stamp) < 450 || self.travel < 2.0 * REST_TRAVEL)
            && self.pointers.iter().any(|(p, _)| *p == id)
        {
            if Some(id) == self.primary || self.mode == Mode::Pinch {
                self.dismiss_pointers();
            } else {
                self.pointers.retain(|(p, _)| *p != id);
                self.ignored.push(id);
            }
            return;
        }
        let Some(slot) = self.pointers.iter_mut().find(|(p, _)| *p == id) else {
            return;
        };
        let previous = slot.1;
        slot.1 = pos;
        if self.mode == Mode::Pinch {
            self.update_pinch();
            return;
        }
        if Some(id) != self.primary {
            return;
        }
        // The back nib touches edge first and only shows its width a moment
        // later: turn the young stroke into an erase along the same path.
        if self.erases(contact) && self.mode == Mode::Draw && self.tool_override.is_none()
            && time.saturating_sub(self.down_stamp) < 250
        {
            let mut trail = std::mem::take(&mut self.trail);
            trail.push((pos, time));
            self.cancel_action();
            self.tool_override = Some(Tool::Eraser);
            self.travel = 0.0;
            self.begin(trail[0].0, trail[0].1);
            let mut prev = trail[0].0;
            for &(p, t) in &trail[1..] {
                self.travel += p.dist(prev);
                prev = p;
                self.raw_screen = p;
                self.continue_(p, t);
            }
            self.trail = trail;
            return;
        }
        self.travel += pos.dist(previous);
        if self.trail.len() < 256 {
            self.trail.push((pos, time));
        }
        self.raw_screen = pos;
        let pos = if self.smoothing && self.mode == Mode::Draw { self.smooth.filter(pos, time) } else { pos };
        self.continue_(pos, time);
    }

    pub fn pointer_up(&mut self, id: i32, pos: Pt, time: u64, cancelled: bool) {
        let pos = pos / self.dpr;
        let was_stylus = self.stylus == Some(id);
        if was_stylus {
            self.stylus = None;
            self.stylus_lifted = Some(time);
        }
        let palm_rejection = self.palm_rejection;
        let rested = |born: u64, travel: f64| {
            palm_rejection && !was_stylus && time.saturating_sub(born) >= REST_DOT_MS && travel < 3.0
        };
        if self.mode == Mode::Pair && self.pair.as_ref().is_some_and(|p| p.tracks.iter().any(|t| t.0 == id)) {
            self.pair_sample(id, pos, time);
            self.decide_pair(time, true);
        }
        if let Some(i) = self.ignored.iter().position(|p| *p == id) {
            self.ignored.remove(i);
            return;
        }
        if let Some(i) = self.extras.iter().position(|e| e.pointer == id) {
            self.extend_extra(i, pos);
            let dropped = rested(self.extras[i].born, self.extra_travel(&self.extras[i]));
            let e = self.extras.remove(i);
            self.damage_world(&e.bounds, self.width / 2.0);
            if !cancelled && !dropped {
                let shape = Shape::new(e.id, Tool::Pen, e.points).styled(self.color, self.width, false);
                let applied = self.doc.add(shape);
                self.on_applied(applied);
            }
            self.overlay_dirty = true;
            return;
        }
        let Some(i) = self.pointers.iter().position(|(p, _)| *p == id) else {
            return;
        };
        self.pointers.remove(i);
        if self.mode == Mode::Pinch {
            self.pinch_zooming = false;
            self.end_pinch_preview();
            self.mode = if self.pointers.is_empty() { Mode::None } else { Mode::Blocked };
            return;
        }
        if self.mode == Mode::Blocked {
            if self.pointers.is_empty() {
                self.mode = Mode::None;
            }
            return;
        }
        if Some(id) == self.primary {
            let resting_mark = self.mode == Mode::Draw && rested(self.down_stamp, self.travel);
            if cancelled || resting_mark {
                self.cancel_action();
            } else {
                // The filtered ink trails the finger a little: end where it lifted.
                if self.mode == Mode::Draw {
                    self.raw_screen = pos;
                    self.continue_(pos, time);
                }
                self.finish(pos, time);
            }
            self.mode = Mode::None;
            self.primary = None;
            if !self.pending.is_empty() && self.ink_deadline.is_none() {
                self.ink_deadline = Some(time + INK_PAUSE);
            }
        }
    }

    /// Preserve the last sample when Android backgrounds the app mid-stroke.
    pub fn finish_input(&mut self, now: u64) {
        let extras: Vec<(i32, Pt)> = self.extras.iter().map(|e| (e.pointer, e.last_screen)).collect();
        for (id, pos) in extras {
            self.pointer_up(id, pos * self.dpr, now, false);
        }
        let pointers = self.pointers.clone();
        for (id, pos) in pointers {
            self.pointer_up(id, pos * self.dpr, now, false);
        }
        self.ignored.clear();
    }

    /// Runs timers; call when [`Editor::next_deadline`] has passed.
    pub fn tick(&mut self, now: u64) {
        if self.snap_deadline.is_some_and(|d| now >= d) {
            self.snap_deadline = None;
            self.try_snap();
        }
        if self.ink_deadline.is_some_and(|d| now >= d) {
            if self.pointers.is_empty() {
                self.flush_pending();
            } else {
                self.ink_deadline = Some(now + 300);
            }
        }
    }

    // ----------------------------------------------- automatic conversion

    fn forget_pending(&mut self) {
        self.pending.clear();
        self.ink_deadline = None;
        self.last_auto = None;
    }

    /// Height used to judge whether a stroke belongs to the writing so far.
    fn line_height(&self, r: &Rect) -> f64 {
        r.height().max(36.0 / self.zoom)
    }

    /// True when `b` continues the line of writing that covers `group`.
    fn continues(&self, b: &Rect, group: &Rect) -> bool {
        let h = self.line_height(group);
        let reach = Rect::ltrb(group.l - h * 1.5, group.t - h * 0.7, group.r + h * 2.5, group.b + h * 0.7);
        reach.overlaps(b) && b.height() <= h * 2.2
    }

    /// The clean shape a finished stroke should become, if any.
    fn auto_shape(&self, raw: &Shape) -> Option<Shape> {
        let shape = recognize_shape(raw)?;
        let b = raw.bounds();
        let straight = shape.tool != Tool::Rectangle && shape.tool != Tool::Ellipse;
        if !self.pending.is_empty() && self.continues(&b, &self.pending_bounds) {
            // In the middle of writing only a frame around the words or a
            // long underline is a shape; "o", "l" and "-" are letters.
            let g = self.pending_bounds;
            let encloses = b.l <= g.l && b.r >= g.r && b.t <= g.t && b.b >= g.b;
            let long = straight && b.longest_side() >= self.line_height(&g) * 2.5;
            return if encloses || long { Some(shape) } else { None };
        }
        let (vw, vh) = self.viewport();
        let side = vw.min(vh);
        let least = if straight { f64::max(130.0, side * 0.15) } else { f64::max(90.0, side * 0.10) };
        if b.longest_side() * self.zoom >= least {
            Some(shape)
        } else {
            None
        }
    }

    /// Adds a finished ink stroke to the handwriting waiting to be read.
    fn note_ink(&mut self, id: &str, bounds: Rect, time: u64) {
        if !self.pending.is_empty() {
            let cell = |bounds| table::containing_cell(self.doc.shapes(), bounds).map(|(s, cell)| (&s.id, cell));
            if !self.continues(&bounds, &self.pending_bounds) || cell(bounds) != cell(self.pending_bounds) {
                self.flush_pending();
            }
        }
        self.pending_bounds = if self.pending.is_empty() { bounds } else { self.pending_bounds.union(&bounds) };
        self.pending.push(id.to_string());
        self.ink_deadline = Some(time + INK_PAUSE);
    }

    /// Hands the waiting handwriting to the host.
    fn flush_pending(&mut self) {
        self.ink_deadline = None;
        let ids = std::mem::take(&mut self.pending);
        let shapes: Vec<Arc<Shape>> =
            ids.iter().filter_map(|id| self.doc.get(id)).filter(|s| s.tool == Tool::Pen).cloned().collect();
        let Some(bounds) = scene_bounds(&shapes) else { return };
        // Something that fills much of the screen is a drawing, not writing.
        let (_, vh) = self.viewport();
        if vh > 0.0 && bounds.height() * self.zoom > vh * 0.45 {
            return;
        }
        self.ink_token = self.ink_token.wrapping_add(1);
        self.ink_requests.push_back(InkRequest {
            token: self.ink_token,
            ids: shapes.iter().map(|s| s.id.clone()).collect(),
            strokes: shapes.iter().map(|s| s.points.clone()).collect(),
            bounds,
        });
        self.events |= EV_INK;
    }

    /// Number of selected pen strokes.
    pub fn selected_ink_count(&self) -> usize {
        self.doc.selected_shapes().iter().filter(|s| s.tool == Tool::Pen).count()
    }

    /// Asks the host to read the selected pen strokes as handwriting, even
    /// when automatic conversion is off.
    pub fn convert_selection(&mut self) {
        let shapes: Vec<Arc<Shape>> = self.doc.selected_shapes().into_iter().filter(|s| s.tool == Tool::Pen).collect();
        let Some(bounds) = scene_bounds(&shapes) else { return };
        self.ink_token = self.ink_token.wrapping_add(1);
        self.ink_requests.push_back(InkRequest {
            token: self.ink_token,
            ids: shapes.iter().map(|s| s.id.clone()).collect(),
            strokes: shapes.iter().map(|s| s.points.clone()).collect(),
            bounds,
        });
        self.events |= EV_INK;
    }

    /// Tidies the selected handwriting in place: levels each line, settles
    /// words on one baseline, evens their size, slant and spacing, and calms
    /// shaky strokes. The ink stays ink, and it is a single undo step.
    pub fn refine_selection(&mut self) {
        let shapes: Vec<Arc<Shape>> = self.doc.selected_shapes().into_iter().filter(|s| s.tool == Tool::Pen).collect();
        if shapes.is_empty() {
            return;
        }
        let strokes: Vec<Vec<Pt>> = shapes.iter().map(|s| s.points.clone()).collect();
        let refined = crate::refine::refine(&strokes);
        let mut drop = Vec::new();
        let mut add = Vec::new();
        for (shape, points) in shapes.iter().zip(refined) {
            if points != shape.points {
                drop.push(shape.id.clone());
                add.push(shape.with_points(points));
            }
        }
        if add.is_empty() {
            return;
        }
        let selected: Vec<String> = self.doc.selected_shapes().iter().map(|s| s.id.clone()).collect();
        let applied = self.doc.swap(drop, add);
        self.on_applied(applied);
        if self.doc.select_many(selected) {
            self.events |= EV_STATE;
            self.overlay_dirty = true;
        }
    }

    /// Replaces the strokes of an [`InkRequest`] with typed `text`, as one
    /// undo step. Does nothing if any of the strokes has since changed.
    pub fn commit_ink(&mut self, ids: &[String], text: &str) {
        self.commit_ink_candidates(ids, &[text]);
    }

    pub(crate) fn commit_ink_candidates(&mut self, ids: &[String], candidates: &[&str]) {
        let text = candidates.first().map_or("", |text| text.trim());
        if text.is_empty() || ids.is_empty() {
            return;
        }
        let mut shapes = Vec::new();
        for id in ids {
            match self.doc.get(id) {
                Some(s) if s.tool == Tool::Pen => shapes.push(s.clone()),
                _ => return,
            }
        }
        let bounds = scene_bounds(&shapes).unwrap();
        if let Some((shape, cell)) = table::containing_cell(self.doc.shapes(), bounds) {
            let mut next = shape.clone();
            let table = next.table.as_mut().unwrap();
            let previous = table.text(cell).unwrap();
            let combined = if previous.is_empty() { text.to_string() } else { format!("{previous} {text}") };
            table.set_text(cell, &combined);
            let applied = self.doc.swap(ids.to_vec(), vec![next]);
            self.on_applied(applied);
            return;
        }
        let formula = math::recognize(candidates, &shapes);
        let letters = text.chars().count().max(1) as f64;
        // Tall enough to match the handwriting; a lone dash or dot has no
        // height, so its width is used instead.
        let by_height = bounds.height() / 0.8;
        let by_width = bounds.width() / (letters * 0.55) * 0.8;
        let size = by_height.max(by_width.min(by_height * 3.0 + 14.0 / self.zoom)).clamp(8.0, 2000.0);
        let id = self.next_id();
        let (tool, text, size, h) = if let Some(source) = &formula {
            let height = math::formula_layout(source, 24.0, self.font).height;
            let size = (size * 24.0 * text::LINE_HEIGHT / height).clamp(8.0, 2000.0);
            (Tool::Formula, source.as_str(), size, math::formula_layout(source, size, self.font).height)
        } else {
            (Tool::Text, text, size, text::measure(text, size, self.font).1)
        };
        let origin = Pt::new(bounds.l, bounds.center().y - h / 2.0);
        let typed = Shape::new(id, tool, vec![origin])
            .styled(shapes[0].color, 3.0, false)
            .with_font(self.font)
            .with_text(text, size);
        let applied = self.doc.swap(ids.to_vec(), vec![typed]);
        self.on_applied(applied);
    }

    fn begin(&mut self, position: Pt, time: u64) {
        let w = self.world(position);
        self.start_world = w;
        self.prediction.clear();
        self.recent.clear();
        self.recent.push_back((position, time));
        match self.tool_override.unwrap_or(self.tool) {
            Tool::Hand => {
                self.mode = Mode::Pan;
                self.anchor_world = w;
            }
            Tool::Pen => {
                self.mode = Mode::Draw;
                self.draft_id = self.next_id();
                self.draft = Some(vec![w]);
                self.draft_bounds = Rect::from_points(w, w);
                self.last_screen = position;
                self.smooth.reset(position, time);
                self.raw_screen = position;
                self.snapped = None;
                self.ink_deadline = None;
                self.reset_still(position, time);
                self.damage_world(&Rect::from_points(w, w), self.width / 2.0);
            }
            Tool::Rectangle | Tool::Ellipse | Tool::Arrow | Tool::Line
            | Tool::Triangle | Tool::Diamond | Tool::Star | Tool::Hexagon => {
                self.mode = Mode::Shape;
                self.draft_end = w;
            }
            Tool::Text => self.mode = Mode::Text,
            Tool::Eraser => {
                self.mode = Mode::Erase;
                self.last_erase = w;
                self.erase_at(w);
            }
            Tool::Select | Tool::Table | Tool::MindNode | Tool::Image | Tool::Formula | Tool::Graph => self.begin_select(position, w),
        }
    }

    fn begin_select(&mut self, position: Pt, w: Pt) {
        let selected = self.doc.selected_shapes();
        let bounds = scene_bounds(&selected);
        self.dragging = false;
        self.delta = Pt::ZERO;
        self.scale_x = 1.0;
        self.scale_y = 1.0;
        self.overlay_dirty = true;
        if let Some(b) = bounds {
            let frame = b.inflate(6.0 / self.zoom);
            let cs = corners(&frame);
            for (i, c) in cs.iter().enumerate() {
                if (*c * self.zoom + self.camera).dist(position) <= 22.0 {
                    self.mode = Mode::Resize;
                    self.handle = i;
                    self.resize_bounds = frame;
                    self.resize_origin = cs[3 - i];
                    self.moving = selected;
                    return;
                }
            }
        }
        if let Some(hit) = self.doc.hit_test(w, 12.0 / self.zoom) {
            if !self.doc.is_selected(&hit.id) {
                self.doc.select(Some(&hit.id));
                self.events |= EV_STATE;
            }
            self.moving = self.move_selection(self.doc.selected_shapes());
            self.mode = Mode::Move;
        } else if bounds.is_some_and(|b| selected.len() > 1 && b.inflate(6.0 / self.zoom).contains(w)) {
            self.moving = self.move_selection(selected);
            self.mode = Mode::Move;
        } else {
            if self.doc.select(None) {
                self.events |= EV_STATE;
            }
            self.mode = Mode::Marquee;
            self.marquee = Some(Rect::from_points(w, w));
        }
    }

    fn move_selection(&self, selected: Vec<Arc<Shape>>) -> Vec<Arc<Shape>> {
        if !selected.iter().any(|s| s.tool == Tool::MindNode) {
            return selected;
        }
        let ids = selected.iter().map(|s| s.id.clone()).collect();
        let ids = mindmap::subtree_ids(self.doc.shapes(), ids);
        self.doc.shapes().iter().filter(|s| ids.contains(&s.id)).cloned().collect()
    }

    fn continue_(&mut self, position: Pt, time: u64) {
        let w = self.world(position);
        match self.mode {
            Mode::Draw => {
                if self.snapped.is_some() || (position - self.last_screen).len2() < 1.0 {
                    return;
                }
                let Some(draft) = self.draft.as_mut() else { return };
                draft.push(w);
                let n = draft.len();
                // The new curve piece replaces the provisional tail, so the
                // last three points bound everything that changed.
                let mut area = Rect::from_points(draft[n - 2], w);
                if n >= 3 {
                    area = area.union(&Rect::from_points(draft[n - 3], draft[n - 3]));
                }
                self.draft_bounds = self.draft_bounds.union(&Rect::from_points(w, w));
                self.last_screen = position;
                self.damage_world(&area, self.width / 2.0);
                if position.dist(self.still_anchor) > 16.0 {
                    self.reset_still(position, time);
                }
                self.predict(position, time);
            }
            Mode::Shape => {
                self.draft_end = w;
                self.overlay_dirty = true;
            }
            Mode::Erase => {
                let steps = ((w.dist(self.last_erase) * self.zoom / 8.0).ceil() as i64).max(1);
                for i in 1..=steps {
                    let p = self.last_erase + (w - self.last_erase) * (i as f64 / steps as f64);
                    self.erase_at(p);
                }
                self.last_erase = w;
            }
            Mode::Move => {
                self.delta = w - self.start_world;
                self.start_dragging();
                self.overlay_dirty = true;
            }
            Mode::Resize => {
                let corner = corners(&self.resize_bounds)[self.handle];
                let (dx, dy) = (corner.x - self.resize_origin.x, corner.y - self.resize_origin.y);
                self.scale_x =
                    if dx.abs() < 1.0 { 1.0 } else { ((w.x - self.resize_origin.x) / dx).clamp(0.05, 100.0) };
                self.scale_y =
                    if dy.abs() < 1.0 { 1.0 } else { ((w.y - self.resize_origin.y) / dy).clamp(0.05, 100.0) };
                self.start_dragging();
                self.overlay_dirty = true;
            }
            Mode::Marquee => {
                self.marquee = Some(Rect::from_points(self.start_world, w));
                self.overlay_dirty = true;
            }
            Mode::Pan => {
                self.camera = position - self.anchor_world * self.zoom;
                self.view_changed();
            }
            Mode::None | Mode::Pinch | Mode::Text | Mode::Blocked | Mode::Pair => {}
        }
    }

    /// Extends the live stroke ahead of the pen along its recent velocity, so
    /// the ink appears to keep up with the hand.
    fn predict(&mut self, position: Pt, time: u64) {
        self.recent.push_back((position, time));
        while self.recent.len() > 6 || (self.recent.len() > 3 && time.saturating_sub(self.recent[0].1) > 40) {
            self.recent.pop_front();
        }
        let had = !self.prediction.is_empty();
        self.prediction.clear();
        // Filtered ink trails the finger slightly; a live tail to the finger
        // hides that, and the stroke catches up when it lifts.
        let mut position = position;
        if self.smoothing && self.raw_screen.dist(position) > 0.5 {
            position = self.raw_screen;
            self.prediction.push(self.world(position));
        }
        if let Some([half, full]) = extrapolate(&self.recent, self.predict_ms) {
            self.prediction.push(self.world(position + half));
            self.prediction.push(self.world(position + full));
        }
        if had || !self.prediction.is_empty() {
            self.overlay_dirty = true;
        }
    }

    fn extend_extra(&mut self, i: usize, pos: Pt) {
        let w = self.world(pos);
        let e = &mut self.extras[i];
        if (pos - e.last_screen).len2() < 1.0 {
            return;
        }
        e.points.push(w);
        e.last_screen = pos;
        e.bounds = e.bounds.union(&Rect::from_points(w, w));
        let n = e.points.len();
        let mut area = Rect::from_points(e.points[n - 2], w);
        if n >= 3 {
            area = area.union(&Rect::from_points(e.points[n - 3], e.points[n - 3]));
        }
        self.damage_world(&area, self.width / 2.0);
    }

    fn start_dragging(&mut self) {
        if self.dragging || self.travel < 4.0 {
            return;
        }
        self.dragging = true;
        self.last_text_tap = None;
        let moving = std::mem::take(&mut self.moving);
        self.hide_shapes(&moving);
        self.moving = moving;
    }

    fn hide_shapes(&mut self, shapes: &[Arc<Shape>]) {
        let mut nodes = HashSet::new();
        for s in shapes {
            if self.hidden.insert(s.id.clone()) {
                self.scene_damage.push(render::paint_bounds(s));
                if s.tool == Tool::MindNode {
                    nodes.insert(s.id.clone());
                }
            }
        }
        self.damage_connections(&nodes);
    }

    fn damage_connections(&mut self, nodes: &HashSet<String>) {
        if nodes.is_empty() {
            return;
        }
        for (parent, child) in mindmap::connections(self.doc.shapes()) {
            if nodes.contains(&parent.id) || nodes.contains(&child.id) {
                self.scene_damage.push(mindmap::connection_bounds(parent.bounds(), child.bounds()));
            }
        }
    }

    fn finish(&mut self, position: Pt, time: u64) {
        let w = self.world(position);
        self.overlay_dirty = true;
        self.prediction.clear();
        match self.mode {
            Mode::Draw => {
                self.snap_deadline = None;
                let Some(points) = self.draft.take() else { return };
                let raw = Shape::new(std::mem::take(&mut self.draft_id), Tool::Pen, points)
                    .styled(self.color, self.width, false);
                let held = self.snapped.take();
                let mut auto = false;
                // A shape that is followed straight away by more writing was
                // a letter after all: put its ink back and read it as text.
                if let Some((first, at)) = self.last_auto.take() {
                    let tidy = self.doc.get(&first.id).is_some_and(|s| s.tool != Tool::Pen || s.points.len() == 2);
                    if self.auto_text
                        && held.is_none()
                        && tidy
                        && self.pending.is_empty()
                        && time.saturating_sub(at) < 1600
                        && self.continues(&raw.bounds(), &first.bounds())
                        && first.bounds().height() <= self.line_height(&raw.bounds()) * 2.2
                    {
                        let (id, bounds) = (first.id.clone(), first.bounds());
                        let applied = self.doc.replace(first);
                        self.on_applied(applied);
                        self.note_ink(&id, bounds, time);
                    }
                }
                let snapped = held.or_else(|| {
                    if self.pen_mode == PenMode::Shape {
                        recognize_shape(&raw)
                    } else if self.pen_mode == PenMode::Write {
                        None
                    } else if self.auto_convert {
                        auto = true;
                        self.auto_shape(&raw)
                    } else {
                        None
                    }
                });
                if snapped.is_some() {
                    // The rough ink on screen is replaced by the clean shape.
                    self.damage_world(&self.draft_bounds.clone(), self.width / 2.0);
                    if auto {
                        self.last_auto = Some((raw.clone(), time));
                    }
                } else if self.auto_text || self.pen_mode == PenMode::Write {
                    self.note_ink(&raw.id, raw.bounds(), time);
                }
                let applied = self.doc.add(snapped.unwrap_or(raw));
                self.on_applied(applied);
            }
            Mode::Shape => {
                if w.dist(self.start_world) * self.zoom > 6.0 {
                    let id = self.next_id();
                    let shape = Shape::new(id.clone(), self.tool, vec![self.start_world, w])
                        .styled(self.color, self.width, self.filled);
                    let applied = self.doc.add(shape);
                    self.on_applied(applied);
                    // Like tldraw, drawing a shape returns to Select with it selected.
                    self.tool = Tool::Select;
                    self.doc.select(Some(&id));
                    self.events |= EV_STATE;
                }
            }
            Mode::Text => {
                if self.travel < 16.0 {
                    self.text_request = Some(TextRequest {
                        id: String::new(), x: w.x, y: w.y, text: String::new(), cell: None,
                    });
                    self.events |= EV_TEXT;
                }
            }
            Mode::Erase => {
                let ids: Vec<String> = self.hidden.drain().collect();
                let applied = self.doc.remove_many(ids);
                self.on_applied(applied);
            }
            Mode::Move => {
                if self.dragging {
                    let delta = self.delta;
                    let moved = self
                        .moving
                        .iter()
                        .map(|s| s.with_points(s.points.iter().map(|p| *p + delta).collect()))
                        .collect();
                    self.clear_transform();
                    let applied = self.doc.replace_many(moved);
                    self.on_applied(applied);
                } else {
                    self.tap_text(w, time);
                    self.clear_transform();
                }
            }
            Mode::Resize => {
                if self.dragging {
                    let (o, sx, sy) = (self.resize_origin, self.scale_x, self.scale_y);
                    let resized = self.moving.iter().map(|s| scaled(s, o, sx, sy)).collect();
                    self.clear_transform();
                    let applied = self.doc.replace_many(resized);
                    self.on_applied(applied);
                } else {
                    self.clear_transform();
                }
            }
            Mode::Marquee => {
                if let Some(area) = self.marquee.take() {
                    if area.longest_side() * self.zoom > 6.0 {
                        let ids: Vec<String> = self.doc.shapes_in(&area).iter().map(|s| s.id.clone()).collect();
                        self.doc.select_many(ids);
                    }
                }
                self.events |= EV_STATE;
            }
            Mode::Pan | Mode::None | Mode::Pinch | Mode::Blocked | Mode::Pair => {}
        }
    }

    fn tap_text(&mut self, point: Pt, time: u64) {
        let target = self.doc.hit_test(point, 12.0 / self.zoom).and_then(|shape| {
            let cell = match shape.tool {
                Tool::Text | Tool::MindNode | Tool::Formula | Tool::Graph => None,
                Tool::Table => Some(shape.table.as_deref()?.cell_at(shape.bounds(), point)?),
                _ => return None,
            };
            Some((shape, cell))
        });
        let Some((shape, cell)) = target else {
            self.last_text_tap = None;
            return;
        };
        if self.last_text_tap.as_ref().is_some_and(|(id, last_cell)| *id == shape.id && *last_cell == cell)
            && time.saturating_sub(self.last_tap_time) < 450
        {
            let text = match cell {
                Some(cell) => shape.table.as_deref().and_then(|table| table.text(cell)).unwrap_or(""),
                None => &shape.text,
            };
            let anchor = if cell.is_some() { point } else { shape.points[0] };
            self.text_request = Some(TextRequest {
                id: shape.id.clone(),
                x: anchor.x,
                y: anchor.y,
                text: text.to_string(),
                cell,
            });
            self.events |= EV_TEXT;
            self.last_text_tap = None;
        } else {
            self.last_text_tap = Some((shape.id.clone(), cell));
            self.last_tap_time = time;
        }
    }

    fn unhide_all(&mut self) {
        let hidden = std::mem::take(&mut self.hidden);
        let mut nodes = HashSet::new();
        for id in hidden {
            if let Some(s) = self.doc.get(&id) {
                self.scene_damage.push(render::paint_bounds(s));
                if s.tool == Tool::MindNode {
                    nodes.insert(id);
                }
            }
        }
        self.damage_connections(&nodes);
    }

    fn clear_transform(&mut self) {
        self.moving.clear();
        self.dragging = false;
        self.delta = Pt::ZERO;
        self.scale_x = 1.0;
        self.scale_y = 1.0;
        self.unhide_all();
    }

    /// Abandons the in-progress action without changing the document.
    fn cancel_action(&mut self) {
        self.snap_deadline = None;
        self.pair = None;
        if self.draft.take().is_some() || self.snapped.is_some() {
            self.damage_world(&self.draft_bounds.clone(), self.width / 2.0);
        }
        self.snapped = None;
        self.prediction.clear();
        self.marquee = None;
        self.clear_transform();
        if self.mode != Mode::Pinch && self.mode != Mode::Blocked {
            self.mode = Mode::None;
        }
        self.overlay_dirty = true;
    }

    fn erase_at(&mut self, w: Pt) {
        let tolerance = 12.0 / self.zoom;
        let hits: Vec<_> = self.doc.shapes().iter().filter(|s| {
            !self.hidden.contains(&s.id) && match s.table.as_deref() {
                Some(table) => table.hits_grid(s.bounds(), w, tolerance + s.width / 2.0),
                None => s.hits(w, tolerance),
            }
        }).cloned().collect();
        if !hits.is_empty() {
            let shapes = self.move_selection(hits);
            self.hide_shapes(&shapes);
            self.overlay_dirty = true;
        }
    }

    fn reset_still(&mut self, position: Pt, time: u64) {
        self.still_anchor = position;
        self.snap_deadline = if self.snap_shapes { Some(time + 500) } else { None };
    }

    /// Holding the pen still at the end of a stroke replaces it with the clean
    /// shape it resembles (line, arrow, rectangle or ellipse).
    fn try_snap(&mut self) {
        let Some(draft) = &self.draft else { return };
        if self.mode != Mode::Draw || draft.len() < 6 {
            return;
        }
        let raw = Shape::new(self.draft_id.clone(), Tool::Pen, draft.clone()).styled(self.color, self.width, false);
        if let Some(shape) = recognize_shape(&raw) {
            self.damage_world(&self.draft_bounds.clone(), self.width / 2.0);
            self.prediction.clear();
            self.snapped = Some(shape);
            self.overlay_dirty = true;
            self.events |= EV_HAPTIC;
        }
    }

    // ------------------------------------------------------------ rendering

    fn view_changed(&mut self) {
        self.overlay_dirty = true;
        self.events |= EV_VIEW;
    }

    /// Marks a world rectangle (grown by `pad` world units) of the frame stale.
    fn damage_world(&mut self, r: &Rect, pad: f64) {
        let px = IRect::covering(&self.view().rect_px(&r.inflate(pad)), 2.0);
        self.damage = self.damage.union(&px);
    }

    fn on_applied(&mut self, applied: Applied) {
        if !applied.changed {
            return;
        }
        self.events |= EV_DOC | EV_STATE;
        self.overlay_dirty = true;
        let view = self.view();
        let direct = !applied.appended.is_empty()
            && self.scene_view == Some(view)
            && self.scene_damage.is_empty()
            && self.hidden.is_empty();
        if direct {
            // New shapes sit on top of everything: paint them straight onto
            // the cached scene instead of repainting what is underneath.
            let (w, h) = (self.scene.w, self.scene.h);
            let full = self.scene.bounds();
            if let Some(mut t) = Target::new(&mut self.scene.data, full, view) {
                for s in &applied.appended {
                    render::draw_shape(&mut t, s, 1.0, None);
                }
            }
            let _ = (w, h);
            for s in &applied.appended {
                self.damage_world(&render::paint_bounds(s), 0.0);
            }
        } else {
            for (bounds, width) in &applied.areas {
                // Generous padding covers arrow heads and text overhang.
                self.scene_damage.push(bounds.inflate(width * 4.5 + 16.0));
            }
        }
    }

    fn paint_scene(scene: &mut Layer, doc: &Document, hidden: &HashSet<String>, view: View, grid: bool, r: IRect) {
        let r = r.intersect(&scene.bounds());
        if r.is_empty() {
            return;
        }
        let whole = r == scene.bounds();
        let mut scratch = if whole { Vec::new() } else { vec![0u8; r.width() as usize * r.height() as usize * 4] };
        {
            let buf: &mut [u8] = if whole { &mut scene.data } else { &mut scratch };
            let Some(mut t) = Target::new(buf, r, view) else { return };
            render::draw_background(&mut t, grid);
            for (parent, child) in mindmap::connections(doc.shapes()) {
                if hidden.contains(&parent.id) || hidden.contains(&child.id) {
                    continue;
                }
                let edge = mindmap::connection_bounds(parent.bounds(), child.bounds());
                if IRect::covering(&view.rect_px(&edge), 2.0).intersects(&r) {
                    render::draw_connection(&mut t, parent.bounds(), child.bounds(), child.color);
                }
            }
            for s in doc.shapes() {
                if hidden.contains(&s.id) {
                    continue;
                }
                let px = IRect::covering(&view.rect_px(&render::paint_bounds(s)), 2.0);
                if px.intersects(&r) {
                    render::draw_shape(&mut t, s, 1.0, None);
                }
            }
        }
        if !whole {
            scene.write(r, &scratch);
        }
    }

    fn transform_extra(&self) -> Option<Transform> {
        if !self.dragging {
            return None;
        }
        match self.mode {
            Mode::Move => Some(Transform::from_translate(self.delta.x as f32, self.delta.y as f32)),
            Mode::Resize => {
                let o = self.resize_origin;
                Some(
                    Transform::from_translate(-o.x as f32, -o.y as f32)
                        .post_scale(self.scale_x as f32, self.scale_y as f32)
                        .post_translate(o.x as f32, o.y as f32),
                )
            }
            _ => None,
        }
    }

    fn map_rect(&self, r: &Rect) -> Rect {
        if !self.dragging {
            return *r;
        }
        match self.mode {
            Mode::Move => r.shift(self.delta),
            Mode::Resize => {
                let o = self.resize_origin;
                let m = |p: Pt| Pt::new(o.x + (p.x - o.x) * self.scale_x, o.y + (p.y - o.y) * self.scale_y);
                Rect::from_points(m(r.top_left()), m(r.bottom_right()))
            }
            _ => *r,
        }
    }

    fn draft_shape(&self) -> Option<Shape> {
        if self.mode != Mode::Shape {
            return None;
        }
        Some(
            Shape::new("draft", self.tool, vec![self.start_world, self.draft_end])
                .styled(self.color, self.width, self.filled),
        )
    }

    fn selection_for_overlay(&self) -> Vec<Arc<Shape>> {
        if self.tool != Tool::Select {
            return Vec::new();
        }
        if self.dragging && matches!(self.mode, Mode::Move | Mode::Resize) {
            self.moving.iter().filter(|s| self.doc.is_selected(&s.id)).cloned().collect()
        } else {
            self.doc.selected_shapes()
        }
    }

    fn live_connections(&self) -> impl Iterator<Item = (Rect, Rect, u32)> + '_ {
        let connected = self.dragging && self.moving.iter().any(|s| s.tool == Tool::MindNode);
        connected.then(|| mindmap::connections(self.doc.shapes()))
            .into_iter().flatten().filter_map(|(parent, child)| {
                let parent_moves = self.hidden.contains(&parent.id);
                let child_moves = self.hidden.contains(&child.id);
                if !parent_moves && !child_moves {
                    return None;
                }
                let bounds = |s: &Shape, moves| if moves { self.map_rect(&s.bounds()) } else { s.bounds() };
                Some((bounds(parent, parent_moves), bounds(child, child_moves), child.color))
            })
    }

    /// Screen pixels covered by live feedback other than the pen stroke.
    fn overlay_bounds(&self) -> IRect {
        let view = self.view();
        let mut out = EMPTY;
        let mut add = |r: Rect, pad_px: f64| out = out.union(&IRect::covering(&view.rect_px(&r), pad_px));
        if self.mode == Mode::Erase {
            for s in self.doc.shapes().iter().filter(|s| self.hidden.contains(&s.id)) {
                add(render::paint_bounds(s), 2.0);
            }
        }
        if self.dragging {
            for (parent, child, _) in self.live_connections() {
                add(mindmap::connection_bounds(parent, child), 3.0);
            }
            for s in &self.moving {
                // Resizing also scales stroke widths and arrow heads.
                let grow = self.scale_x.abs().max(self.scale_y.abs()).max(1.0);
                let pad = (render::paint_bounds(s).width() - s.bounds().width()) / 2.0 * grow;
                add(self.map_rect(&s.bounds()).inflate(pad), 3.0);
            }
        }
        if let Some(s) = &self.snapped {
            add(render::paint_bounds(s), 2.0);
        }
        if let (Some(draft), false) = (&self.draft, self.prediction.is_empty()) {
            // The guessed tail and the real tail it reshapes.
            let mut r = Rect::from_points(self.prediction[0], self.prediction[0]);
            for p in self.prediction.iter().chain(draft.iter().rev().take(3)) {
                r = r.union(&Rect::from_points(*p, *p));
            }
            add(r.inflate(self.width / 2.0), 3.0);
        }
        if let Some(s) = self.draft_shape() {
            add(render::paint_bounds(&s), 2.0);
        }
        let selected = self.selection_for_overlay();
        if !selected.is_empty() {
            let mut all: Option<Rect> = None;
            for s in &selected {
                let r = self.map_rect(&s.bounds());
                all = Some(all.map_or(r, |a| a.union(&r)));
            }
            add(all.unwrap().inflate(12.0 / self.zoom), 3.0);
        }
        if let Some(m) = &self.marquee {
            add(*m, 3.0);
        }
        out
    }

    fn paint_live(&self, t: &mut Target) {
        if self.mode == Mode::Erase {
            for s in self.doc.shapes().iter().filter(|s| self.hidden.contains(&s.id)) {
                render::draw_shape(t, s, 0.22, None);
            }
        }
        if let Some(extra) = self.transform_extra() {
            for (parent, child, color) in self.live_connections() {
                render::draw_connection(t, parent, child, color);
            }
            for s in &self.moving {
                render::draw_shape(t, s, 1.0, Some(extra));
            }
        }
        if let Some(s) = &self.snapped {
            render::draw_shape(t, s, 1.0, None);
        } else if let Some(draft) = &self.draft {
            if self.prediction.is_empty() {
                render::draw_draft(t, draft, self.color, self.width);
            } else {
                let mut ahead = Vec::with_capacity(draft.len() + self.prediction.len());
                ahead.extend_from_slice(draft);
                ahead.extend_from_slice(&self.prediction);
                render::draw_draft(t, &ahead, self.color, self.width);
            }
        }
        for e in &self.extras {
            render::draw_draft(t, &e.points, self.color, self.width);
        }
        if let Some(s) = self.draft_shape() {
            render::draw_shape(t, &s, 1.0, None);
        }
        let selected = self.selection_for_overlay();
        if !selected.is_empty() {
            let z = self.zoom;
            let mut all: Option<Rect> = None;
            for s in &selected {
                let r = self.map_rect(&s.bounds());
                all = Some(all.map_or(r, |a| a.union(&r)));
                if selected.len() > 1 {
                    render::draw_frame(t, &r.inflate(2.0 / z), 1.5, None, 0.0);
                }
            }
            let frame = all.unwrap().inflate(6.0 / z);
            render::draw_frame(t, &frame, 1.5, None, 0.0);
            let half = 5.0 / z;
            for c in corners(&frame) {
                let handle = Rect::ltrb(c.x - half, c.y - half, c.x + half, c.y + half);
                render::draw_frame(t, &handle, 1.5, Some(0xffffffff), 1.5 / z);
            }
        }
        if let Some(m) = &self.marquee {
            render::draw_frame(t, m, 1.0, Some(0x1a2f80ed), 0.0);
        }
    }

    fn has_live(&self) -> bool {
        self.draft.is_some()
            || !self.extras.is_empty()
            || self.snapped.is_some()
            || self.marquee.is_some()
            || self.dragging
            || self.mode == Mode::Shape
            || (self.mode == Mode::Erase && !self.hidden.is_empty())
            || (self.tool == Tool::Select && !self.doc.selected_ids().is_empty())
    }

    /// Brings `frame` up to date. Returns the pixel rectangle that changed,
    /// or `None` when there is nothing new to show.
    pub fn render(&mut self) -> Option<IRect> {
        let full = self.scene.bounds();
        if full.is_empty() {
            return None;
        }
        let view = self.view();
        // During an active pinch-zoom, scale a snapshot of the cached scene
        // instead of re-rasterising the whole board on every touch frame.
        if self.mode == Mode::Pinch && self.scene_damage.is_empty() {
            if let Some(old) = self.scene_view {
                if old.zoom != view.zoom && old.dpr == view.dpr {
                    let (snap, snap_view) = self
                        .pinch_snapshot
                        .get_or_insert_with(|| (self.scene.clone(), old));
                    render::blit_scaled(&mut self.frame, snap, *snap_view, view);
                    self.damage = EMPTY;
                    self.overlay_dirty = false;
                    return Some(full);
                }
            }
        } else {
            self.pinch_snapshot = None;
        }
        // 1. Bring the cached scene to the current camera.
        match self.scene_view {
            Some(old) if old == view => {}
            Some(old) if old.zoom == view.zoom && old.dpr == view.dpr => {
                let (dx, dy) = (
                    (view.offset_px().0 - old.offset_px().0) as i64,
                    (view.offset_px().1 - old.offset_px().1) as i64,
                );
                if dx.abs() < full.x1 as i64 && dy.abs() < full.y1 as i64 {
                    // Panning: slide the picture and paint only what scrolled in.
                    let (dx, dy) = (dx as i32, dy as i32);
                    if dx != 0 || dy != 0 {
                        self.scene.scroll(dx, dy);
                        let (w, h) = (full.x1, full.y1);
                        let mut strips = Vec::new();
                        let (mut x0, mut x1) = (0, w);
                        if dx > 0 {
                            strips.push(IRect::new(0, 0, dx, h));
                            x0 = dx;
                        } else if dx < 0 {
                            strips.push(IRect::new(w + dx, 0, w, h));
                            x1 = w + dx;
                        }
                        if dy > 0 {
                            strips.push(IRect::new(x0, 0, x1, dy));
                        } else if dy < 0 {
                            strips.push(IRect::new(x0, h + dy, x1, h));
                        }
                        for strip in strips {
                            Self::paint_scene(&mut self.scene, &self.doc, &self.hidden, view, self.grid, strip);
                        }
                        self.damage = full;
                    }
                } else {
                    Self::paint_scene(&mut self.scene, &self.doc, &self.hidden, view, self.grid, full);
                    self.full_repaints += 1;
                    self.scene_damage.clear();
                    self.damage = full;
                }
            }
            _ => {
                Self::paint_scene(&mut self.scene, &self.doc, &self.hidden, view, self.grid, full);
                self.full_repaints += 1;
                self.scene_damage.clear();
                self.damage = full;
            }
        }
        self.scene_view = Some(view);
        // 2. Repaint the parts of the scene whose shapes changed.
        if !self.scene_damage.is_empty() {
            let mut rects: Vec<IRect> = std::mem::take(&mut self.scene_damage)
                .iter()
                .map(|r| IRect::covering(&view.rect_px(r), 3.0).intersect(&full))
                .filter(|r| !r.is_empty())
                .collect();
            if rects.len() > 6 {
                let all = rects.iter().fold(EMPTY, |a, r| a.union(r));
                rects = vec![all];
            }
            for r in rects {
                Self::paint_scene(&mut self.scene, &self.doc, &self.hidden, view, self.grid, r);
                self.damage = self.damage.union(&r);
            }
        }
        // 3. Live feedback that moved: repaint where it was and where it is.
        if self.overlay_dirty {
            self.overlay_dirty = false;
            let now = self.overlay_bounds();
            self.damage = self.damage.union(&self.overlay_prev).union(&now);
            self.overlay_prev = now;
        }
        // 4. Compose the damaged rectangle of the frame.
        let d = std::mem::replace(&mut self.damage, EMPTY).intersect(&full);
        if d.is_empty() {
            return None;
        }
        if self.has_live() {
            let mut scratch = self.scene.read(d);
            if let Some(mut t) = Target::new(&mut scratch, d, view) {
                self.paint_live(&mut t);
            }
            self.frame.write(d, &scratch);
        } else {
            self.frame.copy_from(&self.scene, d);
        }
        Some(d)
    }
}

#[cfg(test)]
mod board_objects_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dragging_the_diamond_tool_creates_one_styled_shape() {
        let mut editor = Editor::new();
        let start = Pt::new(140.0, 120.0);
        let end = Pt::new(30.0, 50.0);
        editor.set_tool(Tool::Diamond, PenMode::Ink);
        editor.apply_style(Some(0xff123456), Some(6.0), Some(true));

        editor.pointer_down(0, start, 0);
        editor.pointer_move(0, Pt::new(40.0, 60.0), 20);
        editor.pointer_up(0, end, 30, false);

        let [shape] = editor.doc.shapes() else {
            panic!("expected one diamond");
        };
        assert_eq!(shape.tool, Tool::Diamond);
        assert_eq!(shape.bounds(), Rect::from_points(start, end));
        assert_eq!((shape.color, shape.width, shape.filled), (0xff123456, 6.0, true));
        assert_eq!(editor.tool(), Tool::Select);
        assert!(editor.doc.is_selected(&shape.id));
    }
}

#[cfg(test)]
mod predict_tests {
    use super::*;

    fn window(points: impl Iterator<Item = (f64, f64)>) -> VecDeque<(Pt, u64)> {
        points.enumerate().map(|(i, (x, y))| (Pt::new(x, y), i as u64 * 8)).collect()
    }

    #[test]
    fn straight_line_runs_ahead_by_the_horizon() {
        let w = window((0..6).map(|i| (i as f64 * 4.0, 0.0))); // 0.5 px/ms
        let [half, full] = extrapolate(&w, 24.0).unwrap();
        assert!((full.x - 12.0).abs() < 0.01 && full.y.abs() < 0.01, "{full:?}");
        assert!((half.x - 6.0).abs() < 0.01);
    }

    #[test]
    fn a_circle_is_predicted_along_the_circle() {
        // Steady circle of radius 100 px, 0.05 rad per 8 ms sample.
        let r = 100.0;
        let at = |i: usize| (r * (i as f64 * 0.05).cos(), r * (i as f64 * 0.05).sin());
        let w = window((0..6).map(at));
        let [_, full] = extrapolate(&w, 24.0).unwrap();
        let last = Pt::new(at(5).0, at(5).1);
        let guess = last + full;
        // Three samples ahead on the true circle (24 ms = 3 samples).
        let truth = Pt::new(at(8).0, at(8).1);
        let tangent_only = last + (last - Pt::new(at(4).0, at(4).1)) * 3.0;
        assert!(guess.dist(truth) < 0.5, "arc guess {guess:?} vs {truth:?}");
        assert!(guess.dist(truth) < tangent_only.dist(truth) / 3.0);
    }

    #[test]
    fn slowing_down_shortens_the_guess() {
        let xs = [0.0, 8.0, 16.0, 24.0, 28.0, 30.0];
        let w = window(xs.iter().map(|&x| (x, 0.0)));
        let [_, full] = extrapolate(&w, 24.0).unwrap();
        let steady = 0.5 * 24.0; // newer-half speed times the horizon, untrimmed
        assert!(full.x > 0.0 && full.x < steady * 0.5, "{full:?}");
    }

    #[test]
    fn corners_and_slow_strokes_get_no_guess() {
        let corner = window([(0.0, 0.0), (8.0, 0.0), (16.0, 0.0), (16.0, 8.0), (16.0, 16.0), (16.0, 24.0)].into_iter());
        assert!(extrapolate(&corner, 24.0).is_none());
        let slow = window((0..6).map(|i| (i as f64 * 0.4, 0.0)));
        assert!(extrapolate(&slow, 24.0).is_none());
    }

    #[test]
    fn the_tail_is_capped() {
        let w = window((0..6).map(|i| (i as f64 * 40.0, 0.0)));
        let [_, full] = extrapolate(&w, 36.0).unwrap();
        assert!((full.len() - PREDICT_CAP).abs() < 1e-6);
    }
}
