//! Inkboard's board library, canvas controls and overlays. The host supplies
//! a window, input and system services through [`Platform`].

use super::font::{self, Face};
use super::icons::Ico;
use super::cloud::{self, Account, CloudBoard, Members, Role};
use super::net;
use super::paint::{self, Painter, Rf};
use crate::editor::{Contact, Editor, PenMode, EV_DOC, EV_HAPTIC, EV_INK, EV_TEXT, EV_VIEW};
use crate::geom::{IRect, Pt};
use crate::import::{FileKind, Imported};
use crate::render::{self, Layer};
use crate::shape::{Shape, Tool};
use crate::store::{self, BoardFile, BoardMeta};
use crate::table::{self, Cell};
use crate::text;
use serde_json::{json, Map, Value};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::sync::Arc;
use tiny_skia::PixmapMut;

mod board_objects;
mod math_ui;
mod media;
mod team;
use team::{Live, SignIn, Tab};
use media::PendingFile;

// ------------------------------------------------------------------ colours

const ACCENT: u32 = 0xff2748e8;
const ACCENT_PRESSED: u32 = 0xff1b35b8;
const ACCENT_TINT: u32 = 0xffe7ecff;
const SURFACE: u32 = 0xffffffff;
const SURFACE2: u32 = 0xfff1f2f6;
const BORDER: u32 = 0xffe1e4ea;
const TEXT1: u32 = 0xff14161c;
const TEXT2: u32 = 0xff4a5160;
const TEXT3: u32 = 0xff6b7280;
const PAGE: u32 = 0xfff4f5f8;
const DANGER: u32 = 0xffc62828;
const DANGER_TINT: u32 = 0xfffdecec;
const OK: u32 = 0xff1e8e5a;
const DISABLED: u32 = 0xffb4bac6;
const TOGGLE_OFF: u32 = 0xffc4cad6;
const SCRIM: u32 = 0x8014161c;
const SCRIM_LIGHT: u32 = 0x1f14161c;
const TOAST_BG: u32 = 0xff1b1e26;
const TOAST_ACTION: u32 = 0xffa9b8ff;

const PALETTE: [u32; 12] = [
    0xff1d1d1d, 0xff9fa8b2, 0xffe03131, 0xffe16919, 0xfff1ac4b, 0xff4cb05e, 0xff099268, 0xff4ba1f1,
    0xff4465e9, 0xffae3ec9, 0xffe085f4, 0xfff87777,
];
/// How long the phone pad's box stays on the board after the phone last spoke.
const PAD_TTL: u64 = 15_000;
const PEN_WIDTHS: [f64; 4] = [2.0, 3.5, 5.0, 10.0];
const HL_WIDTHS: [f64; 4] = [8.0, 14.0, 22.0, 32.0];
const SHAPES: [(Tool, Ico, &str); 9] = [
    (Tool::Rectangle, Ico::Rect, "Rectangle"),
    (Tool::Ellipse, Ico::Ellipse, "Ellipse"),
    (Tool::Arrow, Ico::Arrow, "Arrow"),
    (Tool::Line, Ico::Line, "Line"),
    (Tool::Triangle, Ico::Triangle, "Triangle"),
    (Tool::Diamond, Ico::Diamond, "Diamond"),
    (Tool::Star, Ico::Star, "Star"),
    (Tool::Hexagon, Ico::Hexagon, "Hexagon"),
    (Tool::Pen, Ico::Smart, "Smart shapes"),
];
const INSERT: &[(Ico, &str, InsertKind)] = &[
    (Ico::Image, "Image", InsertKind::File(FileKind::Image)),
    (Ico::Pdf, "PDF", InsertKind::File(FileKind::Pdf)),
    (Ico::Page, "Document", InsertKind::File(FileKind::Document)),
    (Ico::Table, "Table", InsertKind::Table),
    (Ico::MindMap, "Mind map", InsertKind::MindMap),
    (Ico::Formula, "Formula", InsertKind::Formula),
    (Ico::Graph, "Graph", InsertKind::Graph),
];
const LANGS: [(&str, &str); 1] = [("en", "English")];

// ----------------------------------------------------------- host side

/// System services the app asks its host for. Every call comes from the
/// thread that drives the [`App`].
pub trait Platform: Send {
    fn haptic(&mut self) {}
    /// Shows the on-screen keyboard editing `text`. The host reports every
    /// change with [`App::text_input`].
    fn show_keyboard(&mut self, _text: &str, _multiline: bool) {}
    fn hide_keyboard(&mut self) {}
    /// Gets handwriting reading ready for a language ("en" or "hi").
    fn prepare_handwriting(&mut self, _lang: &str) {}
    /// Reads handwriting and answers with [`App::ink_result`]. `data` is
    /// [token, strokes, then per stroke its point count and x, y pairs].
    fn read_ink(&mut self, _token: u32, _lang: &str, _data: &[f32]) {}
    /// Looks for a newer release and answers with [`App::update_result`].
    fn check_updates(&mut self, _manual: bool) {}
    fn install_update(&mut self, _url: &str) {}
    /// Draw at the panel's full resolution instead of a capped one.
    fn set_full_resolution(&mut self, _on: bool) {}
    /// Show live ink through a dedicated low-latency layer.
    fn set_fast_ink(&mut self, _on: bool) {}
    /// Puts `text` on the clipboard.
    fn copy_text(&mut self, _text: &str) {}
    /// Opens `url` in a browser. Hosts that cannot leave it alone.
    fn open_url(&mut self, _url: &str) {}
    /// Starts "Sign in with Google" in a browser against `server` and
    /// answers with [`App::finish_sign_in`] (or [`App::sign_in_failed`]).
    /// Returns false when the host can't, and the app shows a code instead.
    fn sign_in_in_browser(&mut self, _server: &str) -> bool {
        false
    }
    /// Saves a PNG picture to the device's Pictures.
    fn save_png(&mut self, _name: &str, _png: &[u8]) {}
    /// Lets the user choose board files to bring in. The host answers with
    /// [`App::import_boards`].
    fn pick_boards(&mut self) {}
    /// Opens the system picker and reports its result through `App::file_result`.
    fn pick_file(&mut self, _token: i32, _kind: FileKind) {}
    fn exit(&mut self) {}
}

/// One touch sample. Positions are buffer pixels; `time` is milliseconds on
/// the host's monotonic clock.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Sample {
    pub id: i32,
    pub x: f32,
    pub y: f32,
    pub time: u64,
    pub size: f32,
    pub stylus: bool,
    /// The eraser end of a stylus.
    pub eraser: bool,
    /// Android's normalised contact size (`MotionEvent.getSize`), sent only
    /// for IR panels whose sizes are known to mean something; else 0.
    pub norm: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Down,
    Move,
    Up,
    Cancel,
}

/// Keyboard shortcuts a desktop host can send.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shortcut {
    Undo,
    Redo,
    Delete,
    ZoomIn,
    ZoomOut,
    Fit,
}

/// Android key codes the app understands.
pub mod keys {
    pub const UP: i32 = 19;
    pub const DOWN: i32 = 20;
    pub const LEFT: i32 = 21;
    pub const RIGHT: i32 = 22;
    pub const CENTER: i32 = 23;
    pub const ENTER: i32 = 66;
    pub const NUMPAD_ENTER: i32 = 160;
}

// ---------------------------------------------------------------- disk

enum Job {
    /// Moves a board that now lives in the person's account out of the list.
    Archive(String),
    List,
    Thumb {
        id: String,
        updated: String,
        w: u32,
        h: u32,
    },
    Load(String),
    Save {
        id: String,
        title: String,
        updated: String,
        shapes: Vec<Arc<Shape>>,
        extra: Map<String, Value>,
        pages: Vec<Page>,
        seq: u64,
        ack: Option<SyncSender<()>>,
    },
    Create {
        id: String,
        title: String,
        updated: String,
        extra: Map<String, Value>,
    },
    Delete(String),
    Rename {
        id: String,
        title: String,
        updated: String,
    },
    /// The text of board files to copy in.
    Import(Vec<String>),
    /// Copies a board to a new file that caches team board `cloud`.
    Copy {
        from: String,
        to: String,
        title: String,
        cloud: String,
    },
}

enum Done {
    Listed(Vec<BoardMeta>),
    Imported {
        added: usize,
        skipped: usize,
        failed: usize,
        list: Vec<BoardMeta>,
    },
    Thumb {
        id: String,
        updated: String,
        w: u32,
        h: u32,
        rgba: Vec<u8>,
    },
    Loaded(String, Result<BoardFile, String>),
    Saved {
        seq: u64,
        ok: bool,
    },
    Created(String, bool),
    File(i32, Result<Imported, String>),
}

#[cfg(not(target_arch = "wasm32"))]
fn disk_worker(
    dir: PathBuf,
    jobs: Receiver<Job>,
    done: Sender<Done>,
    wake: Arc<dyn Fn() + Send + Sync>,
) {
    while let Ok(job) = jobs.recv() {
        let Some(out) = disk_job(&dir, job) else { continue };
        if done.send(out).is_err() {
            return;
        }
        wake();
    }
}

/// Does one piece of disk work. `None` when there is nothing to report.
fn disk_job(dir: &Path, job: Job) -> Option<Done> {
    let dir = dir.to_path_buf();
    Some({
        match job {
            Job::List => Done::Listed(store::list(&dir)),
            Job::Thumb { id, updated, w, h } => match store::load_pages(&dir, &id, false) {
                Ok(board) => {
                    let shapes: Vec<Arc<Shape>> = board.shapes.into_iter().map(Arc::new).collect();
                    Done::Thumb {
                        id,
                        updated,
                        w,
                        h,
                        rgba: render::thumbnail(&shapes, w, h),
                    }
                }
                Err(_) => return None,
            },
            Job::Load(id) => {
                let result = store::load(&dir, &id);
                Done::Loaded(id, result)
            }
            Job::Save {
                id,
                title,
                updated,
                shapes,
                mut extra,
                pages,
                seq,
                ack,
            } => {
                let mut save = || -> Result<(), String> {
                    let images = store::ImageRefs::new(&dir, &id)?;
                    if !pages.is_empty() {
                        let pages = pages.iter().map(|page| page.encode(&images)).collect::<Result<_, _>>()?;
                        extra.insert("pages".into(), Value::Array(pages));
                    }
                    let contents = store::encode_board(&id, &title, &updated, images.encode(&shapes)?, &extra);
                    store::save_atomic(&dir, &id, &contents)
                };
                let ok = save().is_ok();
                if let Some(ack) = ack {
                    let _ = ack.send(());
                }
                Done::Saved { seq, ok }
            }
            Job::Create {
                id,
                title,
                updated,
                extra,
            } => {
                let contents = store::encode(&id, &title, &updated, &[], &extra);
                let ok = store::save_atomic(&dir, &id, &contents).is_ok();
                Done::Created(id, ok)
            }
            Job::Copy { from, to, title, cloud } => {
                let copy = || -> Result<(), String> {
                    let board = store::load(&dir, &from)?;
                    let mut extra = board.extra;
                    for key in ["lanHost", "lanShare", "share", "synced"] {
                        extra.remove(key);
                    }
                    extra.insert("cloud".into(), json!({ "id": cloud }));
                    let shapes: Vec<Arc<Shape>> = board.shapes.into_iter().map(Arc::new).collect();
                    let contents = store::encode(&to, &title, &board.updated, &shapes, &extra);
                    store::save_atomic(&dir, &to, &contents)
                };
                let ok = copy().is_ok();
                Done::Created(to, ok)
            }
            Job::Archive(id) => {
                let _ = store::archive(&dir, &id);
                Done::Listed(store::list(&dir))
            }
            Job::Delete(id) => {
                let _ = store::delete(&dir, &id);
                Done::Listed(store::list(&dir))
            }
            Job::Rename { id, title, updated } => {
                let _ = store::rename(&dir, &id, &title, &updated);
                Done::Listed(store::list(&dir))
            }
            Job::Import(files) => {
                let (mut added, mut skipped, mut failed) = (0, 0, 0);
                for (i, source) in files.iter().enumerate() {
                    match store::import(&dir, source, || format!("{}{i}", App::new_board_id())) {
                        Ok(Some(_)) => added += 1,
                        Ok(None) => skipped += 1,
                        Err(_) => failed += 1,
                    }
                }
                Done::Imported {
                    added,
                    skipped,
                    failed,
                    list: store::list(&dir),
                }
            }
        }
    })
}

// ------------------------------------------------------------ settings

/// How far ahead the live stroke is drawn for each prediction level, in ms.
const PREDICT_MS: [f64; 4] = [0.0, 12.0, 24.0, 36.0];

#[derive(Clone, PartialEq, Debug)]
struct Settings {
    tidy: bool,
    typed: bool,
    palm: bool,
    grid: bool,
    snap: bool,
    font: u8,
    full_res: bool,
    /// Low-latency ink layer.
    fast_ink: bool,
    smooth: bool,
    dock: Dock,
    /// Ink prediction level, 0 (off) to 3.
    predict: u8,
    /// With a stylus, fingers pan instead of drawing.
    pen_priority: bool,
    /// Several fingers can write at once.
    multi_ink: bool,
    /// A wide touch (finger or the pen's back nib) erases.
    wide_erase: bool,
    lang: String,
    /// Secret part of the Claude (MCP) address.
    mcp_key: String,
    /// Secret part of the phone remote address.
    remote_key: String,
    /// The Inkboard server for team boards.
    server: String,
    /// The signed-in Google Workspace account, if any.
    account: Option<Account>,
}

/// A random lowercase token, for keys that only need to be unguessable.
fn random_token(len: usize) -> String {
    use std::hash::{BuildHasher, Hasher};
    let digits = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut out = String::with_capacity(len);
    let mut round = 0u64;
    while out.len() < len {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u64(round);
        h.write_i64(wall_millis());
        h.write_usize(&out as *const String as usize);
        let mut v = h.finish();
        for _ in 0..10 {
            if out.len() < len {
                out.push(digits[(v % 36) as usize] as char);
                v /= 36;
            }
        }
        round += 1;
    }
    out
}

impl Settings {
    fn load(path: &Path) -> Settings {
        let v: Value = crate::sys::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or(Value::Null);
        let flag = |k: &str| v.get(k).and_then(Value::as_bool).unwrap_or(true);
        let token = |k: &str, min: usize, len: usize| {
            v.get(k)
                .and_then(Value::as_str)
                .filter(|t| {
                    t.len() >= min
                        && t.len() <= 32
                        && t.chars()
                            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
                })
                .map(str::to_string)
                .unwrap_or_else(|| random_token(len))
        };
        Settings {
            tidy: flag("tidy"),
            typed: flag("typed"),
            palm: flag("palm"),
            grid: flag("grid"),
            snap: flag("snap"),
            font: v
                .get("font")
                .and_then(Value::as_u64)
                .unwrap_or(0)
                .min(text::FONT_NAMES.len() as u64 - 1) as u8,
            full_res: v.get("fullRes").and_then(Value::as_bool).unwrap_or(false),
            fast_ink: flag("fastInk"),
            smooth: flag("smooth"),
            dock: v
                .get("dock")
                .and_then(Value::as_u64)
                .and_then(|i| Dock::ALL.get(i as usize).copied())
                .unwrap_or(Dock::Left),
            predict: v.get("predict").and_then(Value::as_u64).unwrap_or(1).min(3) as u8,
            pen_priority: v
                .get("penPriority")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            multi_ink: v.get("multiInk").and_then(Value::as_bool).unwrap_or(false),
            // Fingers writing together and fingers erasing exclude each
            // other on an IR frame; writing wins.
            wide_erase: v.get("wideErase").and_then(Value::as_bool).unwrap_or(true)
                && !v.get("multiInk").and_then(Value::as_bool).unwrap_or(false),
            lang: v
                .get("lang")
                .and_then(Value::as_str)
                .filter(|l| LANGS.iter().any(|(c, _)| c == l))
                .unwrap_or("en")
                .to_string(),
            mcp_key: token("mcpKey", 24, 24),
            remote_key: token("remoteKey", 16, 16),
            server: std::env::var("INKBOARD_SERVER")
                .ok()
                .or_else(|| v.get("server").and_then(Value::as_str).map(str::to_string))
                .filter(|s| s.starts_with("https://") || s.starts_with("http://"))
                .unwrap_or_else(|| cloud::DEFAULT_SERVER.to_string()),
            account: v.get("account").and_then(|a| {
                Some(Account {
                    server: a["server"].as_str()?.to_string(),
                    token: a["token"].as_str()?.to_string(),
                    user: cloud::User::from_json(&a["user"])?,
                })
            }),
        }
    }

    fn save(&self, path: &Path) {
        let v = serde_json::json!({
            "tidy": self.tidy, "typed": self.typed, "palm": self.palm, "grid": self.grid,
            "snap": self.snap, "font": self.font, "fullRes": self.full_res, "lang": self.lang,
            "fastInk": self.fast_ink, "predict": self.predict, "penPriority": self.pen_priority,
            "multiInk": self.multi_ink, "wideErase": self.wide_erase, "smooth": self.smooth, "dock": self.dock as u8,
            "mcpKey": self.mcp_key, "remoteKey": self.remote_key, "server": self.server,
            "account": self.account.as_ref().map(|a| serde_json::json!({ "server": a.server, "token": a.token, "user": a.user.to_json() })),
        });
        let tmp = path.with_extension("tmp");
        if crate::sys::fs::write(&tmp, v.to_string()).is_ok() {
            let _ = crate::sys::fs::rename(&tmp, path);
        }
    }
}

// -------------------------------------------------------------- the tree

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Opt {
    Grid,
    Snap,
    Tidy,
    Typed,
    Palm,
    FastInk,
    PenPriority,
    MultiInk,
    WideErase,
    Smooth,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Dock {
    Left,
    Right,
    Bottom,
    Top,
}

impl Dock {
    const ALL: [Self; 4] = [Self::Left, Self::Right, Self::Bottom, Self::Top];

    fn vertical(self) -> bool {
        matches!(self, Self::Left | Self::Right)
    }
}

#[derive(Clone, Copy)]
struct InkStyle {
    color: u32,
    width: f64,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum InsertKind {
    File(FileKind),
    Table,
    MindMap,
    Formula,
    Graph,
}

/// What a control does.
#[derive(Clone, PartialEq, Debug)]
enum Act {
    /// Opens a board on this device (tests, and boards not yet moved to the account).
    #[cfg_attr(not(test), allow(dead_code))]
    Open(String),
    NewBoard,
    Settings,
    Import,
    Updates,
    Back,
    Menu,
    Undo,
    Redo,
    Delete,
    Convert,
    Refine,
    Bigger,
    Smaller,
    Style,
    Color(u32),
    Width(usize),
    Fill(bool),
    ZoomOut,
    ZoomIn,
    Zoom100,
    Fit,
    Tool(Tool, PenMode),
    Highlighter,
    Shapes,
    Shape(usize),
    Insert,
    InsertItem(InsertKind),
    ImportPdf(bool),
    TableDimensions(isize, isize),
    AddTopic { sibling: bool },
    EditMath,
    Plot,
    GraphRange,
    GraphAdjust { pan: Pt, scale: Option<f64> },
    PagePrev,
    PageNext,
    PageAdd,
    PageMenu,
    AskDeletePage,
    Retry,
    Toggle(Opt),
    FontPanel,
    Font(u8),
    FullRes,
    Predict(u8),
    Dock(Dock),
    Rename(String),
    #[cfg_attr(not(test), allow(dead_code))]
    AskDelete(String),
    Field,
    Cancel,
    Done,
    Share,
    Claude,
    /// Team boards.
    SignIn,
    SignOut,
    Account,
    /// The avatar's menu on the library.
    AccountMenu,
    /// Chooses which boards the library lists.
    Filter(Tab),
    OpenTeam(String),
    TeamMenu(String),
    /// Opens a team board with its share panel on top.
    ShareTeam(String),
    /// Copies the web address that opens a team board.
    CopyBoardLink(String),
    RenameTeam(String),
    DeleteTeam(String),
    AddPerson,
    MemberMenu(String),
    SetRole(String, Role),
    RemoveMember(String),
    OrgAccess(&'static str),
    Remote,
    Export,
    /// Ends the phone pad and makes its link stop working.
    PadDisconnect,
    Copy(String),
    /// Opens a web address in the system browser (web: a new tab).
    OpenUrl(String),
    /// Sign in with a code entered on another device instead of a browser.
    SignInCode,
    /// A surface that takes touches so they do not fall through.
    Nothing,
    Outside,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum ButtonKind {
    Primary,
    Secondary,
    White,
    AccentTint,
    Danger,
    DangerTint,
    Disabled,
}

#[derive(Clone, PartialEq, Debug)]
enum Look {
    /// Where the phone pad writes.
    PadArea,
    /// A QR code of the text, black on white.
    Qr(String),
    Surface {
        color: u32,
        radius: f32,
        level: u8,
        border: bool,
    },
    IconBtn {
        ico: Ico,
        size: f32,
        selected: bool,
        enabled: bool,
    },
    ColorChip {
        color: u32,
        open: bool,
    },
    Label {
        text: String,
        face: Face,
        size: f32,
        color: u32,
        center: bool,
    },
    Para {
        text: String,
        face: Face,
        size: f32,
        line: f32,
        color: u32,
    },
    Overline(String),
    Swatch {
        color: u32,
        chosen: bool,
    },
    Thick {
        width: f32,
        chosen: bool,
    },
    SegOpt {
        text: String,
        chosen: bool,
    },
    Toggle {
        on: bool,
        label: String,
        helper: Option<&'static str>,
    },
    Row {
        icon: Option<Ico>,
        text: String,
        value: String,
        chevron: bool,
        danger: bool,
    },
    Logo,
    EmptyLogo,
    Thumb {
        id: String,
        gen: u64,
    },
    Button {
        icon: Option<Ico>,
        text: String,
        kind: ButtonKind,
        pill: bool,
        level: u8,
    },
    Field {
        text: String,
        hint: String,
        multiline: bool,
    },
    Code(String),
    /// Someone else's pointer, with their name.
    Cursor { color: u32, name: String },
    /// A box around shapes someone else selected or changed.
    Outline { color: u32, label: String, below: bool },
    Avatar { text: String, color: u32 },
    /// Who can open a board, on its card.
    Badge { access: team::Access },
    /// Someone with access to a board, with their role.
    Person { name: String, email: String, role: String, menu: bool },
    StatusDot {
        text: String,
        online: bool,
        chip: bool,
    },
    FontTile {
        font: u8,
        chosen: bool,
    },
    TitleBlock {
        title: String,
        status: String,
        failed: bool,
    },
    Toast {
        text: String,
        error: bool,
        compact: bool,
        action_width: f32,
    },
    Rule,
    Notch(Dock),
    Scrim(u32),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Hl {
    None,
    Pressed,
    Focused,
}

#[derive(Clone, PartialEq, Debug)]
struct Node {
    rect: Rf,
    look: Look,
    act: Option<Act>,
    hl: Hl,
    u: f32,
    clip: Option<IRect>,
    overlay: bool,
    opacity: u8,
}

impl Node {
    fn paint_bounds(&self) -> IRect {
        if self.look == Look::Scrim(0) && self.hl == Hl::None {
            return IRect::new(0, 0, 0, 0);
        }
        let level = match &self.look {
            Look::Surface { level, .. } | Look::Button { level, .. } => *level,
            Look::Toast { .. } => 2,
            _ => 0,
        };
        let bounds = self.rect.covering(2.0 + paint::elev_pad(self.u, level));
        self.clip.map_or(bounds, |clip| bounds.intersect(&clip))
    }

    fn radius(&self) -> f32 {
        match &self.look {
            Look::Surface { radius, .. } => radius * self.u,
            Look::Button { pill: true, .. } | Look::StatusDot { chip: true, .. } => {
                self.rect.h / 2.0
            }
            Look::Button { .. } | Look::Field { .. } => 24.0 * self.u,
            _ => 20.0 * self.u,
        }
    }
}

struct BoardChrome {
    left: Rf,
    right: Rf,
    history: Rf,
    zoom: Rf,
    dock: Rf,
    dock_scale: f32,
    buttons: [Rf; 9],
    dividers: [Rf; 2],
    pages: Rf,
    selection: Rf,
}

/// A board card in the library.
struct Card {
    open: Act,
    menu: Act,
    title: String,
    subtitle: String,
    /// The local board file whose picture to show.
    thumb: Option<String>,
    access: team::Access,
}

#[derive(Clone, Copy)]
struct LibraryGrid {
    margin: f32,
    header: f32,
    columns: usize,
    card_width: f32,
    card_height: f32,
    top: f32,
    content: f32,
}

fn square(x: f32, y: f32, size: f32) -> Rf {
    Rf::new(x, y, size, size)
}

fn overlaps(a: Rf, b: Rf) -> bool {
    a.x < b.right() && a.right() > b.x && a.y < b.bottom() && a.bottom() > b.y
}

fn place_in(value: f32, min: f32, max: f32) -> f32 {
    value.clamp(min, max.max(min))
}

/// Collects interactive and decorative nodes from logical coordinates.
struct Ui<'a> {
    nodes: Vec<Node>,
    u: f32,
    pressed: Option<&'a Act>,
    focus: Option<&'a Act>,
    clip: Option<IRect>,
    overlay: bool,
    opacity: u8,
    offset_y: f32,
}

impl Ui<'_> {
    fn push(&mut self, rect: Rf, look: Look, act: Option<Act>) {
        self.scaled(rect, look, act, 1.0);
    }

    fn scaled(&mut self, rect: Rf, look: Look, act: Option<Act>, scale: f32) {
        if rect.w <= 0.0 || rect.h <= 0.0 {
            return;
        }
        let hl = match act.as_ref() {
            Some(Act::Nothing | Act::Outside) => Hl::None,
            Some(a) if Some(a) == self.pressed => Hl::Pressed,
            Some(a) if Some(a) == self.focus => Hl::Focused,
            _ => Hl::None,
        };
        let mut rect = scale_rect(rect, self.u);
        rect.y += self.offset_y * self.u;
        self.nodes.push(Node {
            rect,
            look,
            act,
            hl,
            u: self.u * scale,
            clip: self.clip,
            overlay: self.overlay,
            opacity: self.opacity,
        });
    }

    fn surface(&mut self, r: Rf, radius: f32, level: u8, act: Option<Act>) {
        self.push(
            r,
            Look::Surface {
                color: SURFACE,
                radius,
                level,
                border: true,
            },
            act,
        );
    }

    fn icon(&mut self, r: Rf, ico: Ico, selected: bool, enabled: bool, act: Act) {
        self.scaled_icon(r, ico, 1.0, selected, enabled, act);
    }

    fn scaled_icon(
        &mut self,
        r: Rf,
        ico: Ico,
        scale: f32,
        selected: bool,
        enabled: bool,
        act: Act,
    ) {
        self.scaled(
            r,
            Look::IconBtn {
                ico,
                size: 40.0,
                selected,
                enabled,
            },
            enabled.then_some(act),
            scale,
        );
    }

    fn label(
        &mut self,
        r: Rf,
        text: impl Into<String>,
        face: Face,
        size: f32,
        color: u32,
        center: bool,
    ) {
        self.push(
            r,
            Look::Label {
                text: text.into(),
                face,
                size,
                color,
                center,
            },
            None,
        );
    }

    fn button(
        &mut self,
        r: Rf,
        icon: Option<Ico>,
        text: impl Into<String>,
        kind: ButtonKind,
        pill: bool,
        act: Act,
    ) {
        self.push(
            r,
            Look::Button {
                icon,
                text: text.into(),
                kind,
                pill,
                level: 0,
            },
            Some(act),
        );
    }
}

fn scale_rect(r: Rf, scale: f32) -> Rf {
    Rf::new(r.x * scale, r.y * scale, r.w * scale, r.h * scale)
}

#[derive(Clone, Debug)]
enum MenuKind {
    Main,
    Pages,
    TeamCard(String),
    Member(String),
    /// The avatar's menu on the library.
    Account,
}

#[derive(Clone, Debug)]
enum AskFor {
    Rename(String),
    RenameTeam(String),
    AddPerson,
    Text { id: String, x: f64, y: f64 },
    Cell { id: String, cell: Cell },
    Math { id: String, tool: Tool },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum PanelKind {
    Share,
    Account,
    Claude,
    Remote,
    Fonts,
    Settings,
}

#[derive(Clone, Debug)]
enum ConfirmFor {
    Delete(String),
    DeleteTeam(String),
    RemoveMember(String),
    DeletePage,
    Install(String),
}

enum Overlay {
    Menu {
        kind: MenuKind,
        anchor: Rf,
    },
    Style,
    Shapes,
    Insert { pdf: bool },
    TableSize { rows: usize, columns: usize },
    GraphRange,
    Ask {
        title: String,
        text: String,
        hint: String,
        multiline: bool,
        purpose: AskFor,
    },
    Confirm {
        title: String,
        body: String,
        cancel: String,
        ok: String,
        purpose: ConfirmFor,
    },
    Panel {
        kind: PanelKind,
        scroll: f32,
    },
}

enum PageChange {
    Open(usize),
    Add,
    Delete,
}

#[derive(Clone, Default)]
struct Page {
    shapes: Vec<Arc<Shape>>,
    view: Option<(Pt, f64)>,
    /// Names the page on team boards.
    key: String,
}

impl Page {
    fn decode(value: &Value) -> Result<Self, String> {
        let shapes = value["shapes"]
            .as_array()
            .ok_or("Page has no shapes list.")?;
        let mut ids = HashSet::new();
        let shapes = shapes
            .iter()
            .map(|value| {
                let shape = Shape::from_json(value)?;
                if !ids.insert(shape.id.clone()) {
                    return Err("Duplicate shape IDs on a page.".into());
                }
                Ok(Arc::new(shape))
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(Self {
            shapes,
            view: Self::decode_view(value),
            key: value["key"].as_str().filter(|k| !k.is_empty() && !k.contains('/')).map_or_else(App::new_page_key, str::to_string),
        })
    }

    fn decode_view(value: &Value) -> Option<(Pt, f64)> {
        let camera = value["camera"].as_array()?;
        let point = Pt::new(camera.first()?.as_f64()?, camera.get(1)?.as_f64()?);
        let zoom = value["zoom"].as_f64()?;
        (point.is_finite() && zoom.is_finite()).then_some((point, zoom))
    }

    fn encode(&self, images: &store::ImageRefs) -> Result<Value, String> {
        let (camera, zoom) = self.view.unwrap_or((Pt::ZERO, 1.0));
        let shapes = images.encode(&self.shapes)?;
        Ok(json!({ "shapes": shapes, "camera": [camera.x, camera.y], "zoom": zoom, "key": self.key }))
    }
}

struct Toast {
    text: String,
    error: bool,
    action: Option<(&'static str, Act)>,
    start: u64,
    end: u64,
}

// ----------------------------------------------------------------- board

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SaveState {
    Saved,
    Saving,
    Failed,
}

struct OpenBoard {
    id: String,
    title: String,
    extra: Map<String, Value>,
}

struct Thumb {
    updated: String,
    w: u32,
    h: u32,
    rgba: Vec<u8>,
    gen: u64,
}

enum Capture {
    Editor,
    /// Pressing a control; on the library a drag turns into scrolling.
    Control {
        act: Act,
        start: (f32, f32),
        last_y: f32,
        scrolling: bool,
        held: bool,
        hold_at: Option<u64>,
    },
    Scroll {
        last_y: f32,
    },
    /// Pressed outside an open menu or dialog.
    Outside,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Screen {
    Library,
    Board,
}

pub struct App {
    pub ed: Editor,
    platform: Box<dyn Platform>,
    version: String,
    utc_offset_min: i32,
    settings_path: PathBuf,
    settings: Settings,

    jobs: Sender<Job>,
    /// Without threads, disk work runs at the start of each frame.
    #[cfg(target_arch = "wasm32")]
    disk: (PathBuf, Receiver<Job>),
    done: Receiver<Done>,
    file_done: Sender<Done>,

    w: u32,
    h: u32,
    d: f32,
    out: Layer,
    nodes: Vec<Node>,
    full: bool,
    now: u64,

    screen: Screen,
    boards: Vec<BoardMeta>,
    listed: bool,
    scroll: f32,
    thumbs: HashMap<String, Thumb>,
    thumbs_wanted: HashSet<String>,
    thumb_gen: u64,

    board: Option<OpenBoard>,
    opening: Option<String>,
    open_after_save: Option<String>,
    save_state: SaveState,
    unsaved: bool,
    save_due: Option<u64>,
    save_seq: u64,
    close_after_save: bool,
    pages: Vec<Page>,
    page: usize,
    file_token: i32,
    pending_file: Option<PendingFile>,
    pen_style: InkStyle,
    hl_style: InkStyle,
    highlighter: bool,
    last_shape: Option<usize>,
    shape_fill: bool,

    overlay: Option<Overlay>,
    overlay_start: u64,
    toast: Option<Toast>,
    tool_tip: Option<(&'static str, Rf, u64)>,
    update: Option<(String, String)>,
    update_offered: bool,
    updates_current: bool,

    captures: HashMap<i32, Capture>,
    pressed: Option<Act>,
    focus: Option<Act>,
    ink: HashMap<u32, Vec<String>>,

    wake: Arc<dyn Fn() + Send + Sync>,
    /// Kept so the request channel and the listener live as long as the app.
    #[allow(dead_code)]
    net_tx: Sender<net::Req>,
    net_rx: Receiver<net::Req>,
    #[allow(dead_code)]
    server: Option<net::Server>,
    /// Counts changes to the open board; joiners poll by it.
    doc_gen: u64,
    /// Counts repaints of the output; the phone remote polls by it.
    out_gen: u64,
    /// Time of the last touch from the phone remote.
    remote_t: u64,
    /// Where the phone pad writes, as fractions of the screen (x, y, w, h).
    pad_area: [f32; 4],
    /// When the phone pad last spoke; its area shows on the board for a while.
    pad_seen: Option<u64>,
    /// Replies from the team board server.
    cloud_tx: Sender<cloud::Reply>,
    cloud_rx: Receiver<cloud::Reply>,
    /// Board images by SHA-256 name, shared with the room connection.
    images: cloud::room::Images,
    /// The open team board's live connection.
    live: Option<Live>,
    sign_in: Option<SignIn>,
    team_boards: Vec<CloudBoard>,
    team_listed: bool,
    tab: Tab,
    /// Team boards known to have been shared with named people.
    invited: HashSet<String>,
    /// A team board to show the share panel for once it opens.
    share_on_open: Option<String>,
    /// When New board was last tapped, so a second tap waits.
    new_board_at: Option<u64>,
    /// Who can open the team board whose share panel is showing.
    members: Option<(String, Members)>,
    /// A local board being copied to Team boards.
    copying: Option<String>,
    /// A board on this device being moved into the person's account: (from, to).
    moving: Option<(String, String)>,
    /// Boards that could not be moved this session, so they are not retried.
    move_failed: Vec<String>,
    /// A board link to open once signed in.
    pending_link: Option<String>,
    lan_ip: Option<std::net::Ipv4Addr>,
    font_tiles: RefCell<HashMap<(u8, u32), Option<text::TextBitmap>>>,
}

/// Floats per stroke point in an ink request.
fn ink_data(token: u32, strokes: &[Vec<Pt>], origin: Pt) -> Vec<f32> {
    let mut out = vec![token as f32, strokes.len() as f32];
    for s in strokes {
        out.push(s.len() as f32);
        for p in s {
            out.push((p.x - origin.x) as f32);
            out.push((p.y - origin.y) as f32);
        }
    }
    out
}

/// True when dotted version `a` is higher than `b`.
pub fn newer(a: &str, b: &str) -> bool {
    let parse = |s: &str| {
        s.split(['.', '-', '+'])
            .map(|p| p.parse::<i64>().unwrap_or(0))
            .collect::<Vec<_>>()
    };
    let (x, y) = (parse(a), parse(b));
    for i in 0..x.len().max(y.len()) {
        let d = x.get(i).copied().unwrap_or(0) - y.get(i).copied().unwrap_or(0);
        if d != 0 {
            return d > 0;
        }
    }
    false
}

fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + if m <= 2 { 1 } else { 0 }, m, d)
}

fn wall_millis() -> i64 {
    crate::sys::now_ms()
}

fn base36(mut v: u64) -> String {
    let digits = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut out = Vec::new();
    loop {
        out.push(digits[(v % 36) as usize]);
        v /= 36;
        if v == 0 {
            break;
        }
    }
    out.reverse();
    String::from_utf8(out).unwrap()
}

impl App {
    /// `dir` holds the boards, `settings_path` the app's settings.
    /// `wake` must make the host call [`App::frame`] soon; it is called from
    /// a background thread when disk work finishes.
    pub fn new(
        platform: Box<dyn Platform>,
        dir: PathBuf,
        settings_path: PathBuf,
        version: String,
        utc_offset_min: i32,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> App {
        let _ = crate::sys::fs::create_dir_all(&dir);
        let (jobs, job_rx) = mpsc::channel();
        let (done_tx, done) = mpsc::channel();
        let file_done = done_tx.clone();
        let worker_dir = dir.clone();
        let disk_wake = wake.clone();
        #[cfg(not(target_arch = "wasm32"))]
        let _ = std::thread::Builder::new()
            .name("boards".into())
            .spawn(move || disk_worker(worker_dir, job_rx, done_tx, disk_wake));
        #[cfg(target_arch = "wasm32")]
        let disk = {
            let _ = (done_tx, disk_wake);
            (worker_dir, job_rx)
        };
        let settings = Settings::load(&settings_path);
        settings.save(&settings_path);
        let (net_tx, net_rx) = mpsc::channel();
        let server = if cfg!(test) { None } else { net::serve(net_tx.clone(), wake.clone()).ok() };
        let (cloud_tx, cloud_rx) = mpsc::channel();
        let mut app = App {
            ed: Editor::new(),
            platform,
            version,
            utc_offset_min,
            settings_path,
            settings,
            jobs,
            #[cfg(target_arch = "wasm32")]
            disk,
            done,
            file_done,
            w: 0,
            h: 0,
            d: 1.0,
            out: Layer::new(0, 0),
            nodes: Vec::new(),
            full: true,
            now: 0,
            screen: Screen::Library,
            boards: Vec::new(),
            listed: false,
            scroll: 0.0,
            thumbs: HashMap::new(),
            thumbs_wanted: HashSet::new(),
            thumb_gen: 0,
            board: None,
            opening: None,
            open_after_save: None,
            save_state: SaveState::Saved,
            unsaved: false,
            save_due: None,
            save_seq: 0,
            close_after_save: false,
            pages: Vec::new(),
            page: 0,
            file_token: 0,
            pending_file: None,
            pen_style: InkStyle {
                color: PALETTE[0],
                width: PEN_WIDTHS[1],
            },
            hl_style: InkStyle {
                color: PALETTE[4],
                width: HL_WIDTHS[1],
            },
            highlighter: false,
            last_shape: None,
            shape_fill: false,
            overlay: None,
            overlay_start: 0,
            toast: None,
            tool_tip: None,
            update: None,
            update_offered: false,
            updates_current: false,
            captures: HashMap::new(),
            pressed: None,
            focus: None,
            ink: HashMap::new(),
            wake,
            net_tx,
            net_rx,
            server,
            doc_gen: 1,
            out_gen: 1,
            remote_t: 0,
            pad_area: [0.25, 0.25, 0.5, 0.5],
            pad_seen: None,
            cloud_tx,
            cloud_rx,
            images: Default::default(),
            live: None,
            sign_in: None,
            team_boards: Vec::new(),
            team_listed: false,
            tab: Tab::All,
            invited: HashSet::new(),
            share_on_open: None,
            new_board_at: None,
            members: None,
            copying: None,
            moving: None,
            move_failed: Vec::new(),
            pending_link: None,
            lan_ip: None,
            font_tiles: RefCell::new(HashMap::new()),
        };
        app.apply_settings();
        app.ed.set_font(app.settings.font);
        app.choose_tool(Tool::Pen, PenMode::Ink, false);
        if app.settings.typed {
            let lang = app.settings.lang.clone();
            app.platform.prepare_handwriting(&lang);
        }
        app.platform.set_full_resolution(app.settings.full_res);
        app.platform.set_fast_ink(app.settings.fast_ink);
        app.platform.check_updates(false);
        app.send(Job::List);
        app.refresh_team();
        app
    }

    fn send(&self, job: Job) {
        let _ = self.jobs.send(job);
    }

    fn apply_settings(&mut self) {
        let s = self.settings.clone();
        self.apply_ink_settings();
        self.ed.set_smoothing(s.smooth);
        self.ed.set_palm_rejection(s.palm, None);
        self.ed.set_pen_priority(s.pen_priority);
        self.ed.set_multi_ink(s.multi_ink);
        self.ed.set_wide_erases(s.wide_erase);
        self.ed
            .set_prediction(PREDICT_MS[s.predict.min(3) as usize]);
        self.ed.set_grid(s.grid);
    }

    fn apply_ink_settings(&mut self) {
        self.ed
            .set_auto_convert(self.settings.tidy && !self.highlighter);
        self.ed
            .set_auto_text(self.settings.typed && !self.highlighter);
        self.ed
            .set_snap_shapes(self.settings.snap && !self.highlighter);
    }

    fn timestamp(&self) -> String {
        let ms = wall_millis() + self.utc_offset_min as i64 * 60_000;
        let secs = ms.div_euclid(1000);
        let (y, m, d) = civil(secs.div_euclid(86_400));
        let sod = secs.rem_euclid(86_400);
        format!(
            "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{:03}",
            sod / 3600,
            sod % 3600 / 60,
            sod % 60,
            ms.rem_euclid(1000)
        )
    }

    fn show_toast(&mut self, text: &str) {
        self.set_toast(text, false, None);
    }

    fn error_toast(&mut self, text: &str) {
        self.set_toast(text, true, None);
    }

    fn set_toast(&mut self, text: &str, error: bool, action: Option<(&'static str, Act)>) {
        let duration = if action.is_some() { 5000 } else { 3000 };
        self.toast = Some(Toast {
            text: text.into(),
            error,
            action,
            start: self.now,
            end: self.now + duration,
        });
    }

    /// A message from the host, shown like the app's own.
    pub fn toast(&mut self, text: &str) {
        self.show_toast(text);
    }

    /// Board files the user picked after [`Platform::pick_boards`].
    pub fn import_boards(&mut self, files: Vec<String>) {
        if files.is_empty() {
            return;
        }
        self.show_toast("Importing boards…");
        self.send(Job::Import(files));
    }

    pub fn is_board_open(&self) -> bool {
        self.screen == Screen::Board
    }

    // ------------------------------------------------------------- frame

    /// Sets the drawing buffer size and how many buffer pixels make one dp.
    pub fn resize(&mut self, w: u32, h: u32, d: f32) {
        self.w = w;
        self.h = h;
        self.d = d.max(0.5);
        self.out = Layer::new(w, h);
        self.ed.resize(w, h, self.d as f64);
        self.ed.invalidate_all();
        self.full = true;
        self.clamp_scroll();
    }

    /// The finished picture; [`App::frame`] says which part changed.
    pub fn output(&self) -> &Layer {
        &self.out
    }

    /// Synchronize timers before handling an asynchronous platform callback.
    pub fn advance_clock(&mut self, now: u64) {
        self.now = self.now.max(now);
    }

    fn animation_deadline(&self) -> Option<u64> {
        if self.settings.fast_ink {
            return None;
        }
        let next = |start| (self.now < start + 120).then_some((self.now + 16).min(start + 120));
        let overlay = self.overlay.as_ref().and_then(|_| next(self.overlay_start));
        let toast = self.toast.as_ref().and_then(|t| {
            next(t.start).or_else(|| {
                let fade = t.end - 150;
                (self.now < t.end).then_some(if self.now < fade {
                    fade
                } else {
                    (self.now + 20).min(t.end)
                })
            })
        });
        overlay.into_iter().chain(toast).min()
    }

    /// When [`App::frame`] next needs to run even without input.
    pub fn next_deadline(&self) -> Option<u64> {
        [
            self.ed.next_deadline(),
            self.save_due,
            self.toast.as_ref().map(|t| t.end),
            self.tool_tip.as_ref().map(|t| t.2),
            self.hold_deadline(),
            self.animation_deadline(),
            self.pad_seen.filter(|_| self.pad_shown()).map(|t| t + PAD_TTL),
            self.live_deadline(),
        ]
        .into_iter()
        .flatten()
        .min()
    }

    /// Brings everything up to date at `now` (host milliseconds) and
    /// repaints. Returns the area of [`App::output`] that changed.
    pub fn frame(&mut self, now: u64) -> Option<IRect> {
        self.now = self.now.max(now);
        self.take_disk();
        self.take_net();
        self.take_cloud();
        self.take_room();
        self.ed.tick(self.now);
        self.take_editor_events();
        self.send_presence();
        if self.toast.as_ref().is_some_and(|t| self.now >= t.end) {
            self.toast = None;
        }
        if self.tool_tip.is_some_and(|t| self.now >= t.2) {
            self.tool_tip = None;
        }
        self.take_holds();
        if self.pad_seen.is_some() && !self.pad_shown() {
            self.pad_seen = None;
        }
        if self.save_due.is_some_and(|due| self.now >= due) {
            if self.ed.pointer_down_now() {
                self.save_due = Some(self.now + 700);
            } else {
                self.save_due = None;
                self.queue_save(None);
            }
        }
        if self.w == 0 || self.h == 0 {
            return None;
        }

        let editor_dirty = if self.screen == Screen::Board {
            self.ed.render()
        } else {
            None
        };
        let fresh = self.layout();
        let mut regions: Vec<IRect> = Vec::new();
        if self.full {
            regions.push(self.out.bounds());
            self.full = false;
        } else {
            if let Some(r) = editor_dirty {
                regions.push(r);
            }
            if let Some(r) = self.diff(&fresh) {
                regions.push(r);
            }
        }
        self.nodes = fresh;
        self.want_thumbs();

        let bounds = self.out.bounds();
        let mut total: Option<IRect> = None;
        for r in regions {
            let r = r.intersect(&bounds);
            if r.is_empty() {
                continue;
            }
            self.compose(r);
            total = Some(total.map_or(r, |t| t.union(&r)));
        }
        if total.is_some() {
            self.out_gen += 1;
        }
        total
    }

    /// Area covered by nodes that differ between the shown tree and `fresh`.
    fn diff(&self, fresh: &[Node]) -> Option<IRect> {
        let mut dirty: Option<IRect> = None;
        let mut add = |r: IRect| {
            if !r.is_empty() {
                dirty = Some(dirty.map_or(r, |t: IRect| t.union(&r)));
            }
        };
        let n = self.nodes.len().max(fresh.len());
        for i in 0..n {
            match (self.nodes.get(i), fresh.get(i)) {
                (Some(a), Some(b)) if a == b => {}
                (a, b) => {
                    if let Some(a) = a {
                        add(a.paint_bounds());
                    }
                    if let Some(b) = b {
                        add(b.paint_bounds());
                    }
                }
            }
        }
        dirty
    }

    fn compose(&mut self, r: IRect) {
        let (w, h) = (r.width() as u32, r.height() as u32);
        let whole = r == self.out.bounds();
        let mut out_data = if whole {
            std::mem::take(&mut self.out.data)
        } else {
            Vec::new()
        };
        {
            let frame = self.ed.frame();
            let board_frame =
                self.screen == Screen::Board && frame.w == self.out.w && frame.h == self.out.h;
            let mut scratch = Vec::new();
            let buf: &mut [u8] = if whole {
                if board_frame {
                    out_data.copy_from_slice(&frame.data);
                } else {
                    let px = [(PAGE >> 16) as u8, (PAGE >> 8) as u8, PAGE as u8, 0xff];
                    for chunk in out_data.chunks_exact_mut(4) {
                        chunk.copy_from_slice(&px);
                    }
                }
                &mut out_data
            } else {
                scratch = if board_frame {
                    frame.read(r)
                } else {
                    let px = [(PAGE >> 16) as u8, (PAGE >> 8) as u8, PAGE as u8, 0xff];
                    px.repeat(w as usize * h as usize)
                };
                &mut scratch
            };
            if let Some(mut target) = PixmapMut::from_bytes(buf, w, h) {
                let mut rest = self.nodes.as_slice();
                while let Some(first) = rest.first() {
                    let count = rest
                        .iter()
                        .take_while(|n| n.clip == first.clip && n.opacity == first.opacity)
                        .count();
                    let (group, tail) = rest.split_at(count);
                    rest = tail;
                    let area = first.clip.map_or(r, |clip| r.intersect(&clip));
                    if first.opacity == 0
                        || area.is_empty()
                        || !group.iter().any(|n| n.paint_bounds().intersects(&area))
                    {
                        continue;
                    }
                    if area == r && first.opacity == 255 {
                        if let Some(pm) = PixmapMut::from_bytes(target.data_mut(), w, h) {
                            self.draw_nodes(pm, area, group);
                        }
                    } else if let Some(mut layer) =
                        tiny_skia::Pixmap::new(area.width() as u32, area.height() as u32)
                    {
                        self.draw_nodes(layer.as_mut(), area, group);
                        let paint = tiny_skia::PixmapPaint {
                            opacity: f32::from(first.opacity) / 255.0,
                            ..Default::default()
                        };
                        target.draw_pixmap(
                            area.x0 - r.x0,
                            area.y0 - r.y0,
                            layer.as_ref(),
                            &paint,
                            tiny_skia::Transform::identity(),
                            None,
                        );
                    }
                }
            }
            if !whole {
                self.out.write(r, &scratch);
            }
        }
        if whole {
            self.out.data = out_data;
        }
    }

    fn draw_nodes(&self, pm: PixmapMut<'_>, area: IRect, nodes: &[Node]) {
        let mut painter = Painter {
            pm,
            ox: area.x0 as f32,
            oy: area.y0 as f32,
        };
        for node in nodes {
            if node.paint_bounds().intersects(&area) {
                self.draw(&mut painter, node);
            }
        }
    }

    // ------------------------------------------------------------- disk

    fn take_disk(&mut self) {
        #[cfg(target_arch = "wasm32")]
        {
            let mut worked = false;
            while let Ok(job) = self.disk.1.try_recv() {
                worked = true;
                if let Some(out) = disk_job(&self.disk.0, job) {
                    let _ = self.file_done.send(out);
                }
            }
            // Results may queue more work; ask for another frame as the worker thread would.
            if worked {
                (self.wake)();
            }
        }
        while let Ok(done) = self.done.try_recv() {
            match done {
                Done::File(token, result) => self.apply_file(token, result),
                Done::Listed(list) => {
                    self.boards = list;
                    self.listed = true;
                    self.clamp_scroll();
                    self.move_next_local_board();
                }
                Done::Imported {
                    added,
                    skipped,
                    failed,
                    list,
                } => {
                    self.boards = list;
                    self.listed = true;
                    self.clamp_scroll();
                    let plural = |n: usize| if n == 1 { "board" } else { "boards" };
                    let mut text = format!("Imported {added} {}", plural(added));
                    if skipped > 0 {
                        text += &format!(", {skipped} already here");
                    }
                    if failed > 0 {
                        text += &format!(", {failed} not readable");
                    }
                    self.show_toast(&text);
                }
                Done::Thumb {
                    id,
                    updated,
                    w,
                    h,
                    rgba,
                } => {
                    self.thumbs_wanted.remove(&id);
                    self.thumb_gen += 1;
                    self.thumbs.insert(
                        id,
                        Thumb {
                            updated,
                            w,
                            h,
                            rgba,
                            gen: self.thumb_gen,
                        },
                    );
                }
                Done::Loaded(id, result) => {
                    if self.opening.as_deref() != Some(id.as_str()) {
                        continue;
                    }
                    self.opening = None;
                    match result.and_then(|mut board| {
                        let mut pages = vec![Page {
                            shapes: Vec::new(),
                            view: board
                                .extra
                                .remove("pageView")
                                .as_ref()
                                .and_then(Page::decode_view),
                            key: board
                                .extra
                                .remove("pageKey")
                                .and_then(|k| k.as_str().filter(|k| !k.is_empty()).map(str::to_string))
                                .unwrap_or_else(App::new_page_key),
                        }];
                        if let Some(Value::Array(values)) = board.extra.remove("pages") {
                            for value in &values {
                                pages.push(Page::decode(value)?);
                            }
                        }
                        self.ed.load(board.shapes)?;
                        Ok((board.title, board.extra, pages))
                    }) {
                        Ok((title, extra, pages)) => {
                            self.ink.clear();
                            self.pages = pages;
                            self.page = 0;
                            self.restore_view();
                            self.ed.take_events();
                            self.ed.invalidate_all();
                            self.board = Some(OpenBoard { id, title, extra });
                            self.screen = Screen::Board;
                            self.save_state = SaveState::Saved;
                            self.unsaved = false;
                            self.save_due = None;
                            self.dismiss();
                            self.focus = None;
                            self.full = true;
                            self.doc_gen += 1;
                            self.start_live();
                        }
                        Err(_) => self.error_toast("This board could not be opened"),
                    }
                }
                Done::Saved { seq, ok } => {
                    if seq == self.save_seq {
                        self.save_state = if ok {
                            SaveState::Saved
                        } else {
                            SaveState::Failed
                        };
                        if !ok {
                            self.unsaved = true;
                        }
                        if self.close_after_save && ok && self.unsaved {
                            self.save_due = None;
                            self.queue_save(None);
                        } else if self.close_after_save {
                            self.close_after_save = false;
                            if ok {
                                self.return_to_library();
                            } else {
                                self.open_after_save = None;
                                self.set_toast(
                                    "Could not save your board.",
                                    true,
                                    Some(("Retry", Act::Retry)),
                                );
                            }
                        }
                    }
                }
                Done::Created(id, ok) if self.moving.as_ref().is_some_and(|(_, to)| *to == id) => {
                    let (from, _) = self.moving.take().unwrap();
                    if ok {
                        self.send(Job::Archive(from));
                    } else {
                        self.move_failed.push(from);
                        self.move_next_local_board();
                    }
                }
                Done::Created(id, ok) => {
                    if ok && self.board.is_some() {
                        self.open_after_save = Some(id);
                        self.close_board();
                    } else if ok {
                        self.open(id);
                    } else {
                        self.error_toast("Could not create the board");
                    }
                }
            }
        }
    }

    fn open(&mut self, id: String) {
        self.opening = Some(id.clone());
        self.send(Job::Load(id));
    }

    fn queue_save(&mut self, ack: Option<SyncSender<()>>) {
        if self.board.is_none() {
            if let Some(ack) = ack {
                let _ = ack.send(());
            }
            return;
        };
        self.store_page();
        let cloud_state = self.cloud_state();
        let board = self.board.as_ref().unwrap();
        let mut extra = board.extra.clone();
        extra.insert("pageKey".into(), Value::String(self.pages[0].key.clone()));
        if let Some(state) = cloud_state {
            extra.insert("cloud".into(), state);
        }
        if let Some((camera, zoom)) = self.pages[0].view {
            extra.insert(
                "pageView".into(),
                json!({ "camera": [camera.x, camera.y], "zoom": zoom }),
            );
        }
        self.save_seq += 1;
        self.unsaved = false;
        self.save_state = SaveState::Saving;
        let job = Job::Save {
            id: board.id.clone(),
            title: board.title.clone(),
            updated: self.timestamp(),
            shapes: self.pages[0].shapes.clone(),
            extra,
            pages: self.pages[1..].to_vec(),
            seq: self.save_seq,
            ack,
        };
        self.send(job);
    }

    fn store_page(&mut self) {
        if let Some(page) = self.pages.get_mut(self.page) {
            page.shapes = self.ed.doc.shapes().to_vec();
            page.view = Some((self.ed.camera(), self.ed.zoom()));
        }
    }

    fn restore_view(&mut self) {
        if let Some((camera, zoom)) = self.pages[self.page].view {
            self.ed.set_view(camera, zoom);
        } else {
            self.ed.fit();
        }
    }

    fn mark_unsaved(&mut self) {
        self.unsaved = true;
        self.save_due = Some(self.now + 1200);
    }

    fn change_page(&mut self, change: PageChange) {
        if self.board.is_none() {
            return;
        }
        if !matches!(change, PageChange::Open(_)) && !self.can_change_pages() {
            self.show_toast("You can view this board but not change it");
            return;
        }
        let len = self.pages.len();
        let target = match change {
            PageChange::Open(page) if page != self.page && page < len => page,
            PageChange::Add => len,
            PageChange::Delete if len > 1 => self.page.min(len - 2),
            _ => return,
        };
        self.pending_file = None;
        self.ed.finish_input(self.now);
        self.take_editor_events();
        self.captures.clear();
        self.pressed = None;
        self.ink.clear();
        self.dismiss();
        self.store_page();
        match change {
            PageChange::Add => self.pages.push(Page {
                shapes: Vec::new(),
                view: Some((Pt::ZERO, 1.0)),
                key: App::new_page_key(),
            }),
            PageChange::Delete => {
                self.pages.remove(self.page);
            }
            PageChange::Open(_) => {}
        }
        self.page = target;
        let shapes = self.pages[self.page]
            .shapes
            .iter()
            .map(|shape| (**shape).clone())
            .collect();
        // Both saved and in-memory pages have already passed duplicate-ID checks.
        self.ed.load(shapes).expect("valid page");
        self.restore_view();
        self.doc_gen += 1;
        self.mark_unsaved();
        self.after_page_change(&change);
    }

    /// The host is going to the background. `ack` is answered once the open
    /// board is safely on disk.
    pub fn pause(&mut self, ack: SyncSender<()>) {
        self.ed.finish_input(self.now);
        self.take_editor_events();
        self.captures.clear();
        self.pressed = None;
        if self.board.is_some() {
            self.save_due = None;
            self.queue_save(Some(ack));
        } else {
            let _ = ack.send(());
        }
    }

    fn close_board(&mut self) {
        if self.close_after_save {
            return;
        }
        self.pending_file = None;
        self.ed.finish_input(self.now);
        self.take_editor_events();
        if self.unsaved
            || self.save_due.is_some()
            || matches!(self.save_state, SaveState::Saving | SaveState::Failed)
        {
            self.save_due = None;
            self.close_after_save = true;
            self.queue_save(None);
            return;
        }
        self.return_to_library();
    }

    fn return_to_library(&mut self) {
        self.board = None;
        self.pages.clear();
        self.page = 0;
        self.ink.clear();
        self.tool_tip = None;
        self.live = None;
        self.members = None;
        self.screen = Screen::Library;
        self.dismiss();
        self.full = true;
        self.send(Job::List);
        self.refresh_team();
        if let Some(id) = self.open_after_save.take() {
            self.open(id);
        }
    }

    fn want_thumbs(&mut self) {
        let mut wanted = Vec::new();
        for node in &self.nodes {
            if let Look::Thumb { id, .. } = &node.look {
                if node.paint_bounds().is_empty() {
                    continue;
                }
                let (w, h) = (node.rect.w.round() as u32, node.rect.h.round() as u32);
                if w == 0 || h == 0 || w > 2048 || h > 2048 || self.thumbs_wanted.contains(id) {
                    continue;
                }
                let Some(meta) = self.boards.iter().find(|b| &b.id == id) else {
                    continue;
                };
                let fresh = self
                    .thumbs
                    .get(id)
                    .is_some_and(|t| t.updated == meta.updated && t.w == w && t.h == h);
                if !fresh {
                    wanted.push((id.clone(), meta.updated.clone(), w, h));
                }
            }
        }
        for (id, updated, w, h) in wanted {
            self.thumbs_wanted.insert(id.clone());
            self.send(Job::Thumb { id, updated, w, h });
        }
    }

    // ------------------------------------------------------------ editor

    fn take_editor_events(&mut self) {
        let events = self.ed.take_events();
        if events & EV_HAPTIC != 0 {
            self.platform.haptic();
        }
        if events & (EV_DOC | EV_VIEW) != 0 && self.board.is_some() {
            self.mark_unsaved();
        }
        if events & EV_DOC != 0 && self.board.is_some() {
            self.doc_gen += 1;
            self.push_local();
        }
        if events & EV_TEXT != 0 {
            if let Some(request) = self.ed.take_text_request() {
                if self.board.is_some() && self.overlay.is_none() && !self.close_after_save {
                    let math_tool = self.ed.doc.get(&request.id).map(|s| s.tool)
                        .filter(|tool| matches!(tool, Tool::Formula | Tool::Graph));
                    if let Some(tool) = math_tool {
                        self.ask_math(tool, request.id, &request.text);
                    } else if let Some(cell) = request.cell {
                        self.ask("Edit cell", &request.text, "Type here", true, AskFor::Cell { id: request.id, cell });
                    } else {
                        let title = if request.id.is_empty() { "Add text" } else { "Edit text" };
                        let purpose = AskFor::Text {
                            id: request.id,
                            x: request.x,
                            y: request.y,
                        };
                        self.ask(title, &request.text, "Type here", true, purpose);
                    }
                }
            }
        }
        if events & EV_INK != 0 {
            while let Some(request) = self.ed.take_ink_request() {
                let data = ink_data(
                    request.token,
                    &request.strokes,
                    Pt::new(request.bounds.l, request.bounds.t),
                );
                if self.ink.len() > 64 {
                    self.ink.clear();
                }
                self.ink.insert(request.token, request.ids);
                let lang = self.settings.lang.clone();
                self.platform.read_ink(request.token, &lang, &data);
            }
        }
    }

    /// The text handwriting `token` reads as; empty leaves the ink alone.
    pub fn ink_result(&mut self, token: u32, text: &str) {
        if let Some(ids) = self.ink.remove(&token) {
            if let Ok(candidates) = serde_json::from_str::<Vec<String>>(text) {
                let candidates: Vec<_> = candidates.iter().take(5).map(String::as_str).collect();
                self.ed.commit_ink_candidates(&ids, &candidates);
            } else {
                self.ed.commit_ink(&ids, text);
            }
        }
    }

    /// The answer to [`Platform::check_updates`]: the newest release's
    /// version and download address, if one was found.
    pub fn update_result(&mut self, found: Option<(String, String)>, failed: bool, manual: bool) {
        match found {
            Some((version, url)) if newer(&version, &self.version) => {
                let offer = manual || !self.update_offered;
                self.update = Some((version, url));
                self.updates_current = false;
                if offer {
                    self.offer_update();
                }
            }
            _ if failed => {
                if manual {
                    self.error_toast("Could not check for updates");
                }
            }
            _ => {
                self.update = None;
                self.updates_current = true;
                if manual {
                    self.show_toast(&format!("You have the latest version ({})", self.version));
                }
            }
        }
    }

    fn offer_update(&mut self) {
        let Some((version, url)) = self.update.clone() else {
            return;
        };
        if self.overlay.is_some() {
            return;
        }
        self.update_offered = true;
        self.show_overlay(Overlay::Confirm {
            title: format!("Install {version}?"),
            body: "Inkboard will restart.".into(),
            cancel: "Later".into(),
            ok: "Install".into(),
            purpose: ConfirmFor::Install(url),
        });
    }

    // ------------------------------------------------------------- input

    pub fn touch(&mut self, phase: Phase, samples: &[Sample]) {
        for s in samples {
            self.now = self.now.max(s.time);
            match phase {
                Phase::Down => self.down(s),
                Phase::Move => self.moved(s),
                Phase::Up => self.up(s, false),
                Phase::Cancel => self.up(s, true),
            }
        }
    }

    fn hit(&self, x: f32, y: f32) -> Option<Act> {
        let hit = |n: &&Node| {
            n.act.is_some()
                && n.rect.contains(x, y)
                && n.clip.is_none_or(|clip| {
                    clip.x0 as f32 <= x
                        && x < clip.x1 as f32
                        && clip.y0 as f32 <= y
                        && y < clip.y1 as f32
                })
        };
        let mut nodes = self.nodes.iter().rev().filter(hit);
        let top = nodes.next()?.act.as_ref()?;
        if *top == Act::Outside && matches!(self.overlay, Some(Overlay::Style | Overlay::Shapes | Overlay::Insert { .. } | Overlay::GraphRange)) {
            let dock = scale_rect(self.board_chrome().dock, self.unit());
            if dock.contains(x, y) {
                return nodes
                    .find_map(|n| n.act.as_ref().filter(|act| Self::hold_label(act).is_some()))
                    .cloned()
                    .or(Some(Act::Outside));
            }
        }
        Some(top.clone())
    }

    fn hold_label(act: &Act) -> Option<&'static str> {
        match act {
            Act::Tool(Tool::Select, _) => Some("Select"),
            Act::Tool(Tool::Hand, _) => Some("Pan"),
            Act::Tool(Tool::Pen, PenMode::Ink) => Some("Pen"),
            Act::Tool(Tool::Pen, PenMode::Write) => Some("Convert handwriting to text"),
            Act::Tool(Tool::Eraser, _) => Some("Eraser"),
            Act::Tool(Tool::Text, _) => Some("Text"),
            Act::Highlighter => Some("Highlighter"),
            Act::Shapes => Some("Shapes"),
            Act::Insert => Some("Insert"),
            Act::Shape(i) => Some(SHAPES[*i].2),
            Act::Style => Some("Colour and thickness"),
            Act::PageMenu => Some("Pages"),
            _ => None,
        }
    }

    fn hold_deadline(&self) -> Option<u64> {
        self.captures
            .values()
            .filter_map(|capture| match capture {
                Capture::Control { hold_at, .. } => *hold_at,
                _ => None,
            })
            .min()
    }

    fn take_holds(&mut self) {
        let mut held = None;
        for capture in self.captures.values_mut() {
            if let Capture::Control {
                act,
                hold_at,
                held: fired,
                ..
            } = capture
            {
                if hold_at.is_some_and(|at| self.now >= at) {
                    *hold_at = None;
                    if self.pressed.as_ref() == Some(act) {
                        *fired = true;
                        held = Some(act.clone());
                    }
                }
            }
        }
        if let Some(act) = held {
            if act == Act::PageMenu {
                self.pressed = None;
                self.run(act);
            } else if let Some(label) = Self::hold_label(&act) {
                let anchor = self.node_rect(&act);
                self.tool_tip = Some((label, anchor, self.now + 1500));
            }
        }
    }

    fn contact(s: &Sample) -> Contact {
        Contact {
            size: s.size.max(0.0) as f64,
            stylus: s.stylus,
            eraser: s.eraser,
            norm: s.norm.max(0.0) as f64,
        }
    }

    fn down(&mut self, s: &Sample) {
        if self.close_after_save {
            return;
        }
        self.focus = None;
        let hit = self.hit(s.x, s.y);
        let capture = match hit {
            Some(Act::Outside) => Capture::Outside,
            None if self.overlay.is_some() => Capture::Outside,
            Some(act) => {
                self.pressed = Some(act.clone());
                Capture::Control {
                    hold_at: Self::hold_label(&act).map(|_| s.time + 400),
                    held: false,
                    act,
                    start: (s.x, s.y),
                    last_y: s.y,
                    scrolling: false,
                }
            }
            None if self.screen == Screen::Board => {
                self.track_pointer(s.x, s.y);
                self.ed.pointer_down_with(
                    s.id,
                    Pt::new(s.x as f64, s.y as f64),
                    s.time,
                    Self::contact(s),
                );
                Capture::Editor
            }
            None => Capture::Scroll { last_y: s.y },
        };
        self.captures.insert(s.id, capture);
    }

    fn moved(&mut self, s: &Sample) {
        let d = self.unit();
        let panel = matches!(self.overlay, Some(Overlay::Panel { .. }));
        let library = (self.screen == Screen::Library && self.overlay.is_none()) || panel;
        let mut scroll_by = 0.0;
        let mut release = false;
        match self.captures.get_mut(&s.id) {
            Some(Capture::Editor) => {
                self.track_pointer(s.x, s.y);
                self.ed.pointer_move_with(
                    s.id,
                    Pt::new(s.x as f64, s.y as f64),
                    s.time,
                    Self::contact(s),
                );
            }
            Some(Capture::Scroll { last_y }) => {
                scroll_by = *last_y - s.y;
                *last_y = s.y;
            }
            Some(Capture::Control {
                start,
                last_y,
                scrolling,
                hold_at,
                ..
            }) => {
                let far = (s.x - start.0).hypot(s.y - start.1) > 10.0 * d;
                if far {
                    *hold_at = None;
                }
                if library && (far || *scrolling) {
                    *scrolling = true;
                    scroll_by = *last_y - s.y;
                    release = true;
                }
                *last_y = s.y;
                if far && !library {
                    release = true;
                }
            }
            _ => {}
        }
        if release {
            self.pressed = None;
        }
        if scroll_by != 0.0 {
            if panel {
                let max = self.panel_max_scroll();
                if let Some(Overlay::Panel { scroll, .. }) = &mut self.overlay {
                    *scroll = (*scroll + scroll_by).clamp(0.0, max);
                }
            } else {
                self.scroll += scroll_by;
                self.clamp_scroll();
            }
        }
    }

    fn up(&mut self, s: &Sample, cancelled: bool) {
        if !cancelled {
            self.take_holds();
        }
        let Some(capture) = self.captures.remove(&s.id) else {
            return;
        };
        match capture {
            Capture::Editor => {
                self.ed
                    .pointer_up(s.id, Pt::new(s.x as f64, s.y as f64), s.time, cancelled)
            }
            Capture::Control {
                act,
                scrolling,
                held,
                ..
            } => {
                let still = self.pressed.as_ref() == Some(&act);
                self.pressed = None;
                if !cancelled
                    && !scrolling
                    && !held
                    && still
                    && self.hit(s.x, s.y).as_ref() == Some(&act)
                {
                    self.run(act);
                }
            }
            Capture::Outside => {
                if !cancelled
                    && matches!(
                        self.overlay,
                        Some(Overlay::Menu { .. } | Overlay::Style | Overlay::Shapes | Overlay::Insert { .. } | Overlay::GraphRange)
                    )
                    && matches!(self.hit(s.x, s.y), Some(Act::Outside) | None)
                {
                    self.dismiss();
                }
            }
            Capture::Scroll { .. } => {}
        }
    }

    /// The host's back button. Returns false when the app should close.
    pub fn back(&mut self) -> bool {
        if self.overlay.is_some() {
            self.dismiss();
            true
        } else if self.screen == Screen::Board {
            self.close_board();
            true
        } else {
            false
        }
    }

    /// A mouse wheel or trackpad scroll at (`x`, `y`), in screen pixels.
    /// On a board it pans, or zooms when `zoom` (Ctrl or pinch) is set; in
    /// the library and panels it scrolls.
    pub fn wheel(&mut self, x: f32, y: f32, dx: f32, dy: f32, zoom: bool) {
        if let Some(Overlay::Panel { .. }) = self.overlay {
            let max = self.panel_max_scroll();
            if let Some(Overlay::Panel { scroll, .. }) = &mut self.overlay {
                *scroll = (*scroll + dy).clamp(0.0, max);
            }
            return;
        }
        if self.overlay.is_some() {
            return;
        }
        match self.screen {
            Screen::Library => {
                self.scroll += dy;
                self.clamp_scroll();
            }
            Screen::Board if zoom => {
                let factor = (-(dy as f64) * 0.0025).exp().clamp(0.5, 2.0);
                self.ed.pan_zoom(Pt::ZERO, factor, Pt::new(x as f64, y as f64));
            }
            Screen::Board => self.ed.pan_zoom(Pt::new(-dx as f64, -dy as f64), 1.0, Pt::new(x as f64, y as f64)),
        }
    }

    /// Keyboard shortcuts from desktop hosts.
    pub fn shortcut(&mut self, shortcut: Shortcut) {
        if self.screen != Screen::Board || self.overlay.is_some() {
            return;
        }
        let act = match shortcut {
            Shortcut::Undo => Act::Undo,
            Shortcut::Redo => Act::Redo,
            Shortcut::Delete => Act::Delete,
            Shortcut::ZoomIn => Act::ZoomIn,
            Shortcut::ZoomOut => Act::ZoomOut,
            Shortcut::Fit => Act::Fit,
        };
        self.run(act);
    }

    /// A mouse or hovering pen moved over the board without pressing, in
    /// screen pixels. Others on a team board see it as this person's cursor.
    pub fn hover(&mut self, x: f32, y: f32) {
        if self.screen == Screen::Board && self.captures.is_empty() {
            self.track_pointer(x, y);
        }
    }

    /// The mouse left the window.
    pub fn hover_exit(&mut self) {
        self.forget_pointer();
    }

    pub fn request_back(&mut self) {
        if !self.back() {
            self.platform.exit();
        }
    }

    /// A remote control or keyboard key. Returns true when it was used.
    pub fn key(&mut self, code: i32) -> bool {
        let dir = match code {
            keys::UP => (0.0, -1.0),
            keys::DOWN => (0.0, 1.0),
            keys::LEFT => (-1.0, 0.0),
            keys::RIGHT => (1.0, 0.0),
            keys::CENTER | keys::ENTER | keys::NUMPAD_ENTER => {
                if let Some(act) = self.focus.clone() {
                    self.run(act);
                    return true;
                }
                return false;
            }
            _ => return false,
        };
        let candidates: Vec<&Node> = self
            .nodes
            .iter()
            .filter(|n| {
                n.overlay == self.overlay.is_some()
                    && n.act.is_some()
                    && n.act != Some(Act::Outside)
                    && n.act != Some(Act::Nothing)
            })
            .collect();
        if candidates.is_empty() {
            return false;
        }
        let current = self
            .focus
            .as_ref()
            .and_then(|f| candidates.iter().find(|n| n.act.as_ref() == Some(f)));
        let next = match current {
            None => candidates
                .iter()
                .filter(|n| !n.paint_bounds().is_empty())
                .min_by(|a, b| {
                    let along = |n: &Node| n.rect.cx() * dir.0 + n.rect.cy() * dir.1;
                    along(a).total_cmp(&along(b))
                })
                .map(|n| (n.act.clone(), n.rect, n.clip)),
            Some(cur) => {
                let (cx, cy) = (cur.rect.cx(), cur.rect.cy());
                candidates
                    .iter()
                    .filter(|n| n.act != cur.act)
                    .filter_map(|n| {
                        let (dx, dy) = (n.rect.cx() - cx, n.rect.cy() - cy);
                        let along = dx * dir.0 + dy * dir.1;
                        let across = (dx * dir.1 - dy * dir.0).abs();
                        if along <= 1.0 {
                            None
                        } else {
                            Some((along + across * 2.0, n))
                        }
                    })
                    .min_by(|a, b| a.0.total_cmp(&b.0))
                    .map(|(_, n)| (n.act.clone(), n.rect, n.clip))
            }
        };
        if let Some((act, rect, clip)) = next {
            self.focus = act;
            if let Some(clip) = clip {
                let top = clip.y0 as f32;
                let bottom = clip.y1 as f32;
                let delta = if rect.y < top {
                    rect.y - top
                } else if rect.bottom() > bottom {
                    rect.bottom() - bottom
                } else {
                    0.0
                };
                let max = self.panel_max_scroll();
                if let Some(Overlay::Panel { scroll, .. }) = &mut self.overlay {
                    *scroll = (*scroll + delta).clamp(0.0, max);
                } else if self.screen == Screen::Library && self.overlay.is_none() {
                    self.scroll += delta;
                    self.clamp_scroll();
                }
            }
        }
        true
    }

    /// Text typed into the open field. `submit` is the keyboard's done key.
    pub fn text_input(&mut self, value: &str, submit: bool) {
        if let Some(Overlay::Ask {
            text, multiline, ..
        }) = &mut self.overlay
        {
            *text = value
                .chars()
                .take(if *multiline { 2000 } else { 300 })
                .collect();
            if submit && !*multiline {
                self.run(Act::Done);
            }
        }
    }

    // ------------------------------------------------------------ actions

    fn ask(&mut self, title: &str, text: &str, hint: &str, multiline: bool, purpose: AskFor) {
        self.show_overlay(Overlay::Ask {
            title: title.into(),
            text: text.into(),
            hint: hint.into(),
            multiline,
            purpose,
        });
        self.platform.show_keyboard(text, multiline);
    }

    fn dismiss(&mut self) {
        if let Some(Overlay::Ask { .. }) = self.overlay {
            self.platform.hide_keyboard();
        }
        self.overlay = None;
        self.focus = None;
    }

    fn show_overlay(&mut self, overlay: Overlay) {
        self.overlay = Some(overlay);
        self.overlay_start = self.now;
        self.tool_tip = None;
        self.focus = None;
    }

    fn menu(&mut self, kind: MenuKind, act: &Act) {
        let anchor = scale_rect(self.node_rect(act), 1.0 / self.unit());
        self.show_overlay(Overlay::Menu { kind, anchor });
    }

    fn node_rect(&self, act: &Act) -> Rf {
        self.nodes
            .iter()
            .find(|n| n.act.as_ref() == Some(act))
            .map(|n| n.rect)
            .unwrap_or_default()
    }

    fn set(&mut self, f: impl FnOnce(&mut Settings)) {
        let before = self.settings.clone();
        f(&mut self.settings);
        if self.settings != before {
            self.settings.save(&self.settings_path);
            self.apply_settings();
            if self.settings.typed && (!before.typed || before.lang != self.settings.lang) {
                let lang = self.settings.lang.clone();
                self.platform.prepare_handwriting(&lang);
            }
            if self.settings.fast_ink != before.fast_ink {
                self.platform.set_fast_ink(self.settings.fast_ink);
            }
            if self.settings.full_res != before.full_res {
                self.platform.set_full_resolution(self.settings.full_res);
            }
        }
    }

    fn choose_tool(&mut self, tool: Tool, mode: PenMode, highlighter: bool) {
        self.ed.set_tool(tool, mode);
        if self.highlighter != highlighter {
            self.highlighter = highlighter;
            self.apply_ink_settings();
        }
        let style = if highlighter {
            InkStyle {
                color: (self.hl_style.color & 0x00ff_ffff) | 0x7300_0000,
                ..self.hl_style
            }
        } else {
            self.pen_style
        };
        // Select can retain an existing selection. Choosing it should not restyle it.
        if self.ed.doc.selected_ids().is_empty() {
            self.ed
                .apply_style(Some(style.color), Some(style.width), Some(self.shape_fill));
        }
        if tool == Tool::Pen && mode == PenMode::Write {
            let lang = self.settings.lang.clone();
            self.platform.prepare_handwriting(&lang);
        }
    }

    fn shape_tool_active(&self) -> bool {
        let tool = self.ed.tool();
        if tool == Tool::Pen {
            self.ed.pen_mode() == PenMode::Shape
        } else {
            SHAPES.iter().any(|(shape, _, _)| *shape == tool)
        }
    }

    fn style_color(&self) -> u32 {
        let color = self
            .ed
            .doc
            .selected_shapes()
            .first()
            .map_or(self.ed.color(), |s| s.color);
        color | 0xff00_0000
    }

    fn run(&mut self, act: Act) {
        if self.run_team(&act) {
            return;
        }
        match act {
            Act::Open(id) => self.open(id),
            Act::NewBoard => {
                // The board exists at once; tap its title to rename it.
                if self.account().is_some() {
                    self.create_team_board("Untitled board".into());
                } else {
                    self.create_local_board("Untitled board".into());
                }
            }
            Act::Settings => self.panel(PanelKind::Settings),
            Act::Import => {
                self.dismiss();
                self.platform.pick_boards();
            }
            Act::Updates => {
                self.dismiss();
                if self.update.is_some() {
                    self.offer_update();
                } else {
                    self.show_toast("Checking for updates…");
                    self.platform.check_updates(true);
                }
            }
            Act::Back => self.close_board(),
            Act::Menu => self.menu(MenuKind::Main, &Act::Menu),
            Act::Undo => self.ed.undo(),
            Act::Redo => self.ed.redo(),
            Act::Delete => self.ed.delete_selection(),
            Act::Convert => self.ed.convert_selection(),
            Act::Refine => self.ed.refine_selection(),
            Act::EditMath => self.edit_math(),
            Act::Plot => self.ed.plot_selected_formula(),
            Act::GraphRange => {
                if matches!(self.overlay, Some(Overlay::GraphRange)) {
                    self.dismiss();
                } else {
                    self.show_overlay(Overlay::GraphRange);
                }
            }
            Act::GraphAdjust { pan, scale } => self.ed.change_graph_range(pan, scale),
            Act::Bigger => self.ed.scale_text(1.25),
            Act::Smaller => self.ed.scale_text(0.8),
            Act::Style => {
                if matches!(self.overlay, Some(Overlay::Style)) {
                    self.dismiss();
                } else {
                    self.show_overlay(Overlay::Style);
                }
            }
            Act::Color(color) => {
                let selected_color = if self.highlighter {
                    self.hl_style.color = color;
                    (color & 0x00ff_ffff) | 0x7300_0000
                } else {
                    self.pen_style.color = color;
                    color
                };
                self.ed.apply_style(Some(selected_color), None, None);
            }
            Act::Width(i) => {
                let style = if self.highlighter {
                    &mut self.hl_style
                } else {
                    &mut self.pen_style
                };
                style.width = if self.highlighter {
                    HL_WIDTHS[i]
                } else {
                    PEN_WIDTHS[i]
                };
                self.ed.apply_style(None, Some(style.width), None);
            }
            Act::Fill(fill) => {
                self.shape_fill = fill;
                self.ed.apply_style(None, None, Some(fill));
            }
            Act::ZoomOut => self.ed.zoom_to(self.ed.zoom() / 1.25, None),
            Act::ZoomIn => self.ed.zoom_to(self.ed.zoom() * 1.25, None),
            Act::Zoom100 => self.ed.zoom_to(1.0, None),
            Act::Fit => self.ed.fit(),
            Act::Tool(tool, mode) => {
                if tool == Tool::Pen
                    && mode == PenMode::Ink
                    && !self.highlighter
                    && self.ed.tool() == tool
                    && self.ed.pen_mode() == mode
                {
                    self.run(Act::Style);
                } else {
                    self.dismiss();
                    let mode = if tool == Tool::Pen
                        && self.ed.tool() == tool
                        && mode == PenMode::Write
                        && self.ed.pen_mode() == PenMode::Write
                    {
                        PenMode::Ink
                    } else {
                        mode
                    };
                    self.choose_tool(tool, mode, false);
                }
            }
            Act::Highlighter => {
                if self.highlighter {
                    self.run(Act::Style);
                } else {
                    self.dismiss();
                    self.choose_tool(Tool::Pen, PenMode::Ink, true);
                }
            }
            Act::Shapes => {
                if matches!(self.overlay, Some(Overlay::Shapes)) {
                    self.dismiss();
                } else {
                    self.show_overlay(Overlay::Shapes);
                }
            }
            Act::Shape(index) => {
                let (tool, _, _) = SHAPES[index];
                self.last_shape = Some(index);
                self.dismiss();
                self.choose_tool(tool, PenMode::Shape, false);
            }
            Act::Insert => {
                if matches!(self.overlay, Some(Overlay::Insert { .. })) {
                    self.dismiss();
                } else {
                    self.show_overlay(Overlay::Insert { pdf: false });
                }
            }
            Act::InsertItem(kind) => match kind {
                InsertKind::File(FileKind::Pdf) if self.can_change_pages() => self.show_overlay(Overlay::Insert { pdf: true }),
                InsertKind::File(kind) => self.pick_file(kind, true),
                InsertKind::Table => self.show_overlay(Overlay::TableSize {
                    rows: table::DEFAULT_SIZE,
                    columns: table::DEFAULT_SIZE,
                }),
                InsertKind::MindMap => {
                    self.dismiss();
                    self.choose_tool(Tool::Select, PenMode::Ink, false);
                    self.ed.insert_mind_map();
                }
                InsertKind::Formula => self.ask_math(Tool::Formula, String::new(), ""),
                InsertKind::Graph => self.ask_math(Tool::Graph, String::new(), ""),
            },
            Act::ImportPdf(current_page) => self.pick_file(FileKind::Pdf, current_page),
            Act::TableDimensions(d_rows, d_columns) => {
                if let Some(Overlay::TableSize { rows, columns }) = &mut self.overlay {
                    *rows = rows.saturating_add_signed(d_rows).clamp(1, table::MAX_SIZE);
                    *columns = columns.saturating_add_signed(d_columns).clamp(1, table::MAX_SIZE);
                } else {
                    self.ed.resize_table_grid(d_rows, d_columns);
                }
            }
            Act::AddTopic { sibling } => self.ed.add_mind_topic(sibling),
            Act::PagePrev => self.change_page(PageChange::Open(self.page.saturating_sub(1))),
            Act::PageNext => self.change_page(PageChange::Open(self.page + 1)),
            Act::PageAdd => self.change_page(PageChange::Add),
            Act::PageMenu if self.pages.len() > 1 && self.can_change_pages() => {
                self.menu(MenuKind::Pages, &Act::PageMenu)
            }
            Act::AskDeletePage => self.show_overlay(Overlay::Confirm {
                title: format!("Delete page {}?", self.page + 1),
                body: "This page and everything on it will be removed from the board.".into(),
                cancel: "Cancel".into(),
                ok: "Delete".into(),
                purpose: ConfirmFor::DeletePage,
            }),
            Act::PageMenu => {}
            Act::Retry => {
                self.toast = None;
                self.ed.finish_input(self.now);
                self.take_editor_events();
                self.save_due = None;
                self.queue_save(None);
            }
            Act::Toggle(opt) => self.set(|s| match opt {
                Opt::Grid => s.grid = !s.grid,
                Opt::Snap => s.snap = !s.snap,
                Opt::Tidy => s.tidy = !s.tidy,
                Opt::Typed => s.typed = !s.typed,
                Opt::Palm => s.palm = !s.palm,
                Opt::FastInk => s.fast_ink = !s.fast_ink,
                Opt::PenPriority => s.pen_priority = !s.pen_priority,
                Opt::MultiInk => {
                    s.multi_ink = !s.multi_ink;
                    s.wide_erase &= !s.multi_ink;
                }
                Opt::WideErase => {
                    s.wide_erase = !s.wide_erase;
                    s.multi_ink &= !s.wide_erase;
                }
                Opt::Smooth => s.smooth = !s.smooth,
            }),
            Act::FontPanel => self.panel(PanelKind::Fonts),
            Act::Font(f) => {
                self.set(|s| s.font = f);
                self.ed.set_font(f);
                self.dismiss();
            }
            Act::Predict(level) => self.set(|s| s.predict = level.min(3)),
            Act::FullRes => self.set(|s| s.full_res = !s.full_res),
            Act::Dock(dock) => self.set(|s| s.dock = dock),
            Act::Rename(id) => {
                let title = self
                    .board
                    .as_ref()
                    .filter(|b| b.id == id)
                    .map(|b| b.title.clone())
                    .or_else(|| {
                        self.boards
                            .iter()
                            .find(|b| b.id == id)
                            .map(|b| b.title.clone())
                    })
                    .unwrap_or_default();
                self.ask(
                    "Rename board",
                    &title,
                    "Board name",
                    false,
                    AskFor::Rename(id),
                );
            }
            Act::AskDelete(id) => {
                let title = self
                    .boards
                    .iter()
                    .find(|b| b.id == id)
                    .map(|b| b.title.as_str())
                    .unwrap_or_default();
                self.show_overlay(Overlay::Confirm {
                    title: format!("Delete “{title}”?"),
                    body: "This board and everything on it will be removed from this panel. This cannot be undone.".into(),
                    cancel: "Cancel".into(),
                    ok: "Delete".into(),
                    purpose: ConfirmFor::Delete(id),
                });
            }
            Act::Field => {
                if let Some(Overlay::Ask {
                    text, multiline, ..
                }) = &self.overlay
                {
                    self.platform.show_keyboard(text, *multiline);
                }
            }
            Act::Cancel | Act::Outside => self.dismiss(),
            Act::Done => self.done(),
            Act::Share => self.share_panel(),
            Act::Claude => {
                self.lan_ip = net::local_ip();
                self.panel(PanelKind::Claude);
            }
            Act::Remote => {
                self.lan_ip = net::local_ip();
                self.panel(PanelKind::Remote);
            }
            Act::PadDisconnect => {
                self.set(|s| s.remote_key = random_token(16));
                self.pad_seen = None;
                self.show_toast("Phone disconnected");
            }
            Act::Export => {
                self.dismiss();
                let title = self
                    .board
                    .as_ref()
                    .map(|b| b.title.clone())
                    .unwrap_or_default();
                match board_png(self.ed.doc.shapes(), 4096) {
                    Some(png) => self.platform.save_png(&title, &png),
                    None => self.error_toast("Could not make the picture"),
                }
            }
            Act::OpenUrl(url) => self.platform.open_url(&url),
            Act::SignInCode => self.sign_in_with_code(),
            Act::Copy(text) => {
                self.platform.copy_text(&text);
                self.show_toast("Copied");
            }
            Act::Nothing => {}
            Act::SignIn | Act::SignOut | Act::Account | Act::AccountMenu | Act::Filter(_) | Act::OpenTeam(_) | Act::TeamMenu(_)
            | Act::ShareTeam(_) | Act::CopyBoardLink(_) | Act::RenameTeam(_) | Act::DeleteTeam(_) | Act::AddPerson | Act::MemberMenu(_) | Act::SetRole(..)
            | Act::RemoveMember(_) | Act::OrgAccess(_) => unreachable!("team actions run above"),
        }
    }

    fn panel(&mut self, kind: PanelKind) {
        self.show_overlay(Overlay::Panel { kind, scroll: 0.0 });
    }

    /// Makes a board on this device and opens it.
    pub(crate) fn create_local_board(&mut self, title: String) {
        let id = Self::new_board_id();
        let updated = self.timestamp();
        self.send(Job::Create {
            id,
            title,
            updated,
            extra: Map::new(),
        });
    }

    fn new_board_id() -> String {
        let salt = (wall_millis() as u64)
            .wrapping_mul(6_364_136_223_846_793_005)
            .rotate_left(17)
            % 46_656;
        format!("b{}{}", base36(wall_millis() as u64), base36(salt))
    }

    fn done(&mut self) {
        let Some(overlay) = self.overlay.take() else {
            return;
        };
        self.focus = None;
        match overlay {
            Overlay::Ask { purpose: AskFor::Math { ref id, tool }, ref text, .. } => {
                match self.ed.commit_math(id, tool, text) {
                    Ok(()) => {
                        if id.is_empty() {
                            self.choose_tool(Tool::Select, PenMode::Ink, false);
                        }
                        self.platform.hide_keyboard();
                        self.take_editor_events();
                    }
                    Err(error) => {
                        self.overlay = Some(overlay);
                        self.show_toast(error);
                    }
                }
            }
            Overlay::Ask { text, purpose, .. } => {
                self.platform.hide_keyboard();
                let name = text.trim();
                match purpose {
                    AskFor::RenameTeam(id) => {
                        if !name.is_empty() {
                            self.rename_team(id, name.to_string());
                        }
                    }
                    AskFor::AddPerson => {
                        if name.contains('@') {
                            self.invite(name.to_lowercase(), Role::Editor);
                            self.share_panel();
                        } else if !name.is_empty() {
                            self.error_toast("Enter an email address");
                        }
                    }
                    AskFor::Rename(id) => {
                        if !name.is_empty() {
                            if self.board.as_ref().is_some_and(|b| b.id == id) && self.rename_live(name) {
                                return;
                            }
                            if let Some(board) = self.board.as_mut().filter(|b| b.id == id) {
                                board.title = name.to_string();
                                self.mark_unsaved();
                                return;
                            }
                            let updated = self.timestamp();
                            self.send(Job::Rename {
                                id,
                                title: name.to_string(),
                                updated,
                            });
                        }
                    }
                    AskFor::Text { id, x, y } => {
                        self.ed.commit_text(&id, x, y, name);
                        self.take_editor_events();
                    }
                    AskFor::Cell { id, cell } => {
                        self.ed.commit_cell_text(&id, cell, &text);
                        self.take_editor_events();
                    }
                    AskFor::Math { .. } => unreachable!("handled above"),
                }
            }
            Overlay::Confirm { purpose, .. } => match purpose {
                ConfirmFor::Delete(id) => {
                    self.thumbs.remove(&id);
                    self.send(Job::Delete(id));
                }
                ConfirmFor::DeletePage => self.change_page(PageChange::Delete),
                ConfirmFor::DeleteTeam(id) => self.delete_team(id),
                ConfirmFor::RemoveMember(email) => {
                    self.remove_member(email);
                    self.share_panel();
                }
                ConfirmFor::Install(url) => {
                    self.show_toast("Downloading the update…");
                    self.platform.install_update(&url);
                }
            },
            Overlay::TableSize { rows, columns } => {
                self.choose_tool(Tool::Select, PenMode::Ink, false);
                self.ed.insert_table(rows, columns);
                self.take_editor_events();
            }
            Overlay::Menu { .. } | Overlay::Panel { .. } | Overlay::Style | Overlay::Shapes | Overlay::Insert { .. } | Overlay::GraphRange => {}
        }
    }

    // ------------------------------------------------------------ layout

    fn unit(&self) -> f32 {
        self.d * (2.0 / 3.0)
    }

    fn extent(&self) -> (f32, f32) {
        (self.w as f32 / self.unit(), self.h as f32 / self.unit())
    }

    fn layout(&self) -> Vec<Node> {
        let mut ui = Ui {
            nodes: Vec::with_capacity(96),
            u: self.unit(),
            pressed: self.pressed.as_ref(),
            focus: self.focus.as_ref(),
            clip: None,
            overlay: false,
            opacity: 255,
            offset_y: 0.0,
        };
        let chrome = match self.screen {
            Screen::Library => {
                self.layout_library(&mut ui);
                None
            }
            Screen::Board => {
                let chrome = self.board_chrome();
                self.layout_board(&mut ui, &chrome);
                Some(chrome)
            }
        };
        if let Some(overlay) = &self.overlay {
            ui.overlay = true;
            self.layout_overlay(&mut ui, overlay, chrome.as_ref());
        }
        if let Some((label, anchor, _)) = self.tool_tip {
            ui.opacity = 255;
            ui.offset_y = 0.0;
            let anchor = scale_rect(anchor, 1.0 / ui.u);
            let width = font::measure(Face::Medium, 22.0 * ui.u, label).0 / ui.u + 40.0;
            let (r, _) = self.popover(anchor, width, 52.0, anchor, 14.0, true);
            ui.push(
                r,
                Look::Toast {
                    text: label.into(),
                    error: false,
                    compact: true,
                    action_width: 0.0,
                },
                None,
            );
        }
        if let Some(toast) = &self.toast {
            self.layout_toast(&mut ui, toast, chrome.as_ref());
        }
        ui.nodes
    }

    fn layout_toast(&self, ui: &mut Ui, toast: &Toast, chrome: Option<&BoardChrome>) {
        let (w, h) = self.extent();
        let (action_width, action) = toast.action.as_ref().map_or((0.0, None), |(name, act)| {
            let width = (font::measure(Face::Bold, 24.0 * ui.u, name).0 / ui.u + 40.0).max(100.0);
            (width, Some((*name, act.clone())))
        });
        let text_width = font::measure(Face::Medium, 24.0 * ui.u, &toast.text).0 / ui.u;
        let width = (text_width + 128.0 + action_width)
            .clamp(360.0, 1200.0)
            .min(w - 32.0);
        let mut r = Rf::new((w - width) / 2.0, h - 104.0, width, 80.0);
        if let Some(c) = chrome {
            if self.settings.dock == Dock::Bottom {
                r.y = r.y.min(c.dock.y - 96.0);
            }
            for bar in [c.history, c.zoom, c.pages] {
                if overlaps(r, bar) {
                    r.y = r.y.min(bar.y - 96.0);
                }
            }
        } else if r.right() > w - self.library_metrics().margin - 260.0 {
            r.y -= 104.0;
        }
        let fade = self.fade_in(toast.start);
        let fade_out = if self.settings.fast_ink {
            1.0
        } else {
            (toast.end.saturating_sub(self.now) as f32 / 150.0).min(1.0)
        };
        ui.opacity = (255.0 * fade.min(fade_out)).round() as u8;
        ui.offset_y = 8.0 * (1.0 - fade);
        ui.push(
            r,
            Look::Toast {
                text: toast.text.clone(),
                error: toast.error,
                compact: false,
                action_width,
            },
            action.as_ref().map(|_| Act::Nothing),
        );
        if let Some((label, act)) = action {
            ui.push(
                Rf::new(
                    r.right() - action_width - 8.0,
                    r.y + 4.0,
                    action_width,
                    72.0,
                ),
                Look::Label {
                    text: label.into(),
                    size: 24.0,
                    face: Face::Bold,
                    color: TOAST_ACTION,
                    center: true,
                },
                Some(act),
            );
        }
    }

    fn fade_in(&self, start: u64) -> f32 {
        if self.settings.fast_ink {
            1.0
        } else {
            (self.now.saturating_sub(start) as f32 / 120.0).min(1.0)
        }
    }

    /// The boards the library shows: the signed-in person's team boards,
    /// narrowed by the filter chips. Signed out there are none; the welcome
    /// screen takes the library's place.
    fn cards(&self) -> Vec<Card> {
        if self.me().is_none() {
            return Vec::new();
        }
        team::team_cards(self)
            .into_iter()
            .map(|c| {
                let local = team::cache_id(&c.board.id);
                Card {
                    open: Act::OpenTeam(c.board.id.clone()),
                    menu: Act::TeamMenu(c.board.id.clone()),
                    title: c.board.title.clone(),
                    subtitle: c.subtitle,
                    thumb: self.boards.iter().any(|b| b.id == local).then_some(local),
                    access: c.access,
                }
            })
            .collect()
    }

    fn library_metrics(&self) -> LibraryGrid {
        let (w, _) = self.extent();
        let margin = if w >= 1200.0 { 96.0 } else { 24.0 };
        let header = 136.0;
        let avail = (w - 2.0 * margin).max(1.0);
        let columns = (((avail + 32.0) / 340.0).floor() as usize).clamp(1, 4);
        let card_width = ((avail - 32.0 * (columns - 1) as f32) / columns as f32).min(if columns == 1 { 640.0 } else { 408.0 });
        let card_height = card_width * 336.0 / 408.0;
        let top = 344.0;
        let rows = self.cards().len().div_ceil(columns);
        let content = if rows == 0 {
            0.0
        } else {
            top + rows as f32 * card_height + rows.saturating_sub(1) as f32 * 32.0 + 160.0
        };
        LibraryGrid {
            margin,
            header,
            columns,
            card_width,
            card_height,
            top,
            content,
        }
    }

    fn clamp_scroll(&mut self) {
        let max = (self.library_metrics().content * self.unit() - self.h as f32).max(0.0);
        self.scroll = self.scroll.clamp(0.0, max);
    }

    /// The update button, drawn left of `right`; returns the new `right`.
    fn update_button(&self, ui: &mut Ui, right: f32, wide: bool) -> f32 {
        let Some((version, _)) = &self.update else { return right };
        let width = if wide { 268.0 } else { 72.0 };
        ui.button(
            Rf::new(right - width, 40.0, width, 72.0),
            Some(Ico::Update),
            if wide { format!("Update {version}") } else { String::new() },
            ButtonKind::AccentTint,
            true,
            Act::Updates,
        );
        right - width - 16.0
    }

    /// Signed out: the library has nothing to show, so it welcomes instead.
    fn layout_welcome(&self, ui: &mut Ui) {
        let (w, h) = self.extent();
        let margin = if w >= 1200.0 { 96.0 } else { 24.0 };
        let wide = w - 2.0 * margin >= 1030.0;
        // Settings stay reachable, quietly.
        ui.button(Rf::new(w - margin - 72.0, 40.0, 72.0, 72.0), Some(Ico::Gear), "", ButtonKind::White, true, Act::Settings);
        self.update_button(ui, w - margin - 88.0, wide);

        let col = (w - 2.0 * margin).clamp(240.0, 760.0);
        let x = (w - col) / 2.0;
        let headline = "Sketch ideas together.";
        let line = "Every board you make is saved to your account and stays private until you share it.";
        let u = ui.u;
        let measured = font::measure(Face::Bold, 64.0 * u, headline).0 / u;
        let size = (64.0 * 0.88 * col / measured.max(1.0)).min(64.0);
        let line_count = paint::wrap(Face::Regular, 28.0 * u, line, col * u).len() as f32;
        let button_w = 520.0_f32.min(col);
        let total = 120.0 + 48.0 + 80.0 + 20.0 + line_count * 40.0 + 56.0 + 96.0 + 48.0 + 32.0;
        let mut y = ((h - total) / 2.0 - 24.0).max(margin + 88.0);

        ui.push(square((w - 120.0) / 2.0, y, 120.0), Look::Logo, None);
        y += 120.0 + 48.0;
        ui.label(Rf::new(x, y, col, 80.0), headline, Face::Bold, size, TEXT1, true);
        y += 80.0 + 20.0;
        for (i, text) in paint::wrap(Face::Regular, 28.0 * u, line, col * u).iter().enumerate() {
            ui.label(Rf::new(x, y + 40.0 * i as f32, col, 40.0), text.as_str(), Face::Regular, 28.0, TEXT2, true);
        }
        y += line_count * 40.0 + 56.0;
        ui.push(
            Rf::new((w - button_w) / 2.0, y, button_w, 96.0),
            Look::Button { icon: None, text: "Sign in with Google".into(), kind: ButtonKind::Primary, pill: true, level: 2 },
            Some(Act::SignIn),
        );
        y += 96.0 + 48.0;
        ui.label(Rf::new(x, y, col, 32.0), "For Kenpath Google Workspace accounts.", Face::Regular, 22.0, TEXT3, true);
    }

    fn layout_library(&self, ui: &mut Ui) {
        let Some(me) = self.me() else {
            self.layout_welcome(ui);
            return;
        };
        let (w, h) = self.extent();
        let grid = self.library_metrics();
        let margin = grid.margin;
        let avail = w - 2.0 * margin;
        let wide = avail >= 1030.0;
        let scroll = self.scroll / ui.u;
        let listed = self.team_listed;
        // Nothing at all yet: one friendly call to action instead of chips.
        let nothing = listed && self.team_boards.is_empty();

        ui.push(
            Rf::new(0.0, 0.0, w, grid.header),
            Look::Surface {
                color: PAGE,
                radius: 0.0,
                level: if scroll > 0.0 { 1 } else { 0 },
                border: false,
            },
            Some(Act::Nothing),
        );
        ui.push(square(margin, 40.0, 56.0), Look::Logo, None);
        let mut right = w - margin - 72.0;
        ui.push(
            Rf::new(right, 40.0, 72.0, 72.0),
            Look::Avatar { text: me.initials(), color: me.color },
            Some(Act::AccountMenu),
        );
        right = self.update_button(ui, right - 16.0, wide);
        if wide && !nothing {
            right -= 232.0;
            ui.button(Rf::new(right, 40.0, 232.0, 72.0), Some(Ico::Plus), "New board", ButtonKind::Primary, true, Act::NewBoard);
            right -= 16.0;
        }
        ui.label(Rf::new(margin + 72.0, 40.0, (right - margin - 72.0).max(0.0), 72.0), "Inkboard", Face::Bold, 32.0, TEXT1, false);

        ui.clip = Some(scale_rect(Rf::new(0.0, grid.header, w, (h - grid.header).max(0.0)), ui.u).covering(0.0));
        let cards = self.cards();
        ui.label(Rf::new(margin, 160.0 - scroll, avail, 64.0), "Boards", Face::Bold, 56.0, TEXT1, false);
        if !nothing {
            let mut x = margin;
            for tab in Tab::ALL {
                let text = tab.label();
                let width = font::measure(Face::Bold, 24.0 * ui.u, text).0 / ui.u + 48.0;
                let kind = if tab == self.tab { ButtonKind::AccentTint } else { ButtonKind::White };
                ui.button(Rf::new(x, 244.0 - scroll, width, 64.0), None, text, kind, true, Act::Filter(tab));
                x += width + 12.0;
            }
        }
        if !listed {
            ui.label(Rf::new(margin, 352.0 - scroll, avail, 40.0), "Loading boards…", Face::Regular, 24.0, TEXT2, false);
        } else if nothing {
            let y = ((h - 392.0) / 2.0).max(220.0);
            ui.push(square(w / 2.0 - 80.0, y, 160.0), Look::EmptyLogo, None);
            ui.label(Rf::new(margin, y + 190.0, avail, 52.0), "Create your first board", Face::Bold, 40.0, TEXT1, true);
            ui.label(Rf::new(margin, y + 250.0, avail, 32.0), "Boards are private to you until you share them.", Face::Regular, 24.0, TEXT2, true);
            let bw = 320.0_f32.min(avail);
            ui.button(Rf::new((w - bw) / 2.0, y + 312.0, bw, 80.0), Some(Ico::Plus), "New board", ButtonKind::Primary, true, Act::NewBoard);
        } else if cards.is_empty() {
            let (title, line) = match self.tab {
                Tab::Shared => ("Nothing shared with you yet", "Boards other people share with you show up here."),
                _ => ("No boards of yours yet", "Tap New board to start one."),
            };
            let y = grid.top + 40.0 - scroll;
            ui.push(square(w / 2.0 - 56.0, y, 112.0), Look::EmptyLogo, None);
            ui.label(Rf::new(margin, y + 136.0, avail, 44.0), title, Face::Bold, 32.0, TEXT1, true);
            ui.label(Rf::new(margin, y + 184.0, avail, 32.0), line, Face::Regular, 22.0, TEXT2, true);
        }

        let cw = grid.card_width;
        let ch = grid.card_height;
        for (i, card) in cards.iter().enumerate() {
            let x = margin + (i % grid.columns) as f32 * (cw + 32.0);
            let y = grid.top + (i / grid.columns) as f32 * (ch + 32.0) - scroll;
            // One row on each side of the viewport supports keyboard scrolling.
            if y + ch < grid.header - ch - 32.0 || y > h + ch + 32.0 {
                continue;
            }
            let rect = Rf::new(x, y, cw, ch);
            let factor = cw / 408.0;
            ui.surface(rect, 28.0, 1, Some(card.open.clone()));
            let gen = card
                .thumb
                .as_ref()
                .and_then(|id| {
                    let meta = self.boards.iter().find(|b| &b.id == id)?;
                    self.thumbs.get(id).filter(|t| t.updated == meta.updated).map(|t| t.gen)
                })
                .unwrap_or(0);
            ui.push(
                Rf::new(x + 8.0, y + 8.0, cw - 16.0, (236.0 * factor).min(ch - 100.0)),
                Look::Thumb { id: card.thumb.clone().unwrap_or_default(), gen },
                None,
            );
            let label = card.access.label();
            let badge_w = font::measure(Face::Bold, 20.0 * ui.u, &label).0 / ui.u + 60.0;
            ui.push(Rf::new(x + 22.0, y + 22.0, badge_w.min(cw - 44.0), 40.0), Look::Badge { access: card.access }, None);
            let info_y = rect.bottom() - 92.0;
            let info_w = cw - 104.0;
            ui.label(Rf::new(x + 24.0, info_y + 6.0, info_w, 36.0), &card.title, Face::Bold, 26.0, TEXT1, false);
            ui.label(Rf::new(x + 24.0, info_y + 44.0, info_w, 28.0), &card.subtitle, Face::Regular, 20.0, TEXT3, false);
            ui.push(
                square(rect.right() - 80.0, rect.bottom() - 84.0, 72.0),
                Look::IconBtn { ico: Ico::Kebab, size: 32.0, selected: false, enabled: true },
                Some(card.menu.clone()),
            );
        }
        ui.clip = None;
        if !wide && !nothing {
            let fw = 244.0_f32.min(avail);
            ui.push(
                Rf::new(w - margin - fw, h - 136.0, fw, 88.0),
                Look::Button { icon: Some(Ico::Plus), text: "New board".into(), kind: ButtonKind::Primary, pill: true, level: 2 },
                Some(Act::NewBoard),
            );
        }
    }

    fn object_actions(&self) -> Vec<(&'static str, f32, Act, bool)> {
        let [id] = self.ed.doc.selected_ids() else { return Vec::new() };
        let Some(shape) = self.ed.doc.get(id) else { return Vec::new() };
        if let Some(table) = shape.table.as_deref() {
            vec![
                ("Add row", 126.0, Act::TableDimensions(1, 0), table.rows() < table::MAX_SIZE),
                ("Remove row", 170.0, Act::TableDimensions(-1, 0), table.rows() > 1),
                ("Add column", 164.0, Act::TableDimensions(0, 1), table.columns() < table::MAX_SIZE),
                ("Remove column", 210.0, Act::TableDimensions(0, -1), table.columns() > 1),
            ]
        } else if shape.tool == Tool::MindNode {
            let has_parent = shape.parent_id.as_deref().and_then(|parent| self.ed.doc.get(parent))
                .is_some_and(|parent| parent.tool == Tool::MindNode);
            vec![
                ("Add child", 142.0, Act::AddTopic { sibling: false }, true),
                ("Add sibling", 164.0, Act::AddTopic { sibling: true }, has_parent),
            ]
        } else if matches!(shape.tool, Tool::Formula | Tool::Graph) {
            Self::math_actions(shape)
        } else {
            Vec::new()
        }
    }

    fn board_chrome(&self) -> BoardChrome {
        let (w, h) = self.extent();
        let dock_side = self.settings.dock;
        let margin = 24.0_f32.min(w / 12.0).min(h / 12.0);
        let compact = (w / 460.0).min(h / 380.0).min(1.0);
        let bar_h = 88.0 * compact;
        let share_chip = if w >= 760.0 && self.live.is_some() { 300.0 } else { 0.0 };
        let right_w = 160.0 * compact + share_chip;
        let left_w = 400.0_f32.min(w - 3.0 * margin - right_w);
        let left = Rf::new(margin, margin, left_w, bar_h);
        let right = Rf::new(w - margin - right_w, margin, right_w, bar_h);
        let bottom_scale = ((w - 3.0 * margin) / 528.0).min(compact);
        let bottom_h = 88.0 * bottom_scale;
        let bottom_y = h - 8.0 * compact - bottom_h;
        let mut history = Rf::new(margin, bottom_y, 168.0 * bottom_scale, bottom_h);
        let mut zoom = Rf::new(
            w - margin - 360.0 * bottom_scale,
            bottom_y,
            360.0 * bottom_scale,
            bottom_h,
        );

        let top = left.bottom() + 16.0 * compact;
        let available = if dock_side.vertical() {
            history.y - 16.0 * compact - top
        } else {
            w - 2.0 * margin
        };
        let k = (available / 754.0).min(compact).max(0.0);
        let long = 754.0 * k;
        let short = 96.0 * k;
        let dock = match dock_side {
            Dock::Left => Rf::new(margin, top + (available - long) / 2.0, short, long),
            Dock::Right => Rf::new(
                w - margin - short,
                top + (available - long) / 2.0,
                short,
                long,
            ),
            Dock::Bottom => {
                let r = Rf::new((w - long) / 2.0, h - margin - short, long, short);
                if overlaps(r, history) || overlaps(r, zoom) {
                    history.y = r.y - 16.0 * compact - history.h;
                    zoom.y = history.y;
                }
                r
            }
            Dock::Top => {
                let mut r = Rf::new((w - long) / 2.0, margin, long, short);
                if overlaps(r, left) || overlaps(r, right) {
                    r.y = top;
                }
                r
            }
        };
        let point = |along: f32, cross: f32, length: f32, thickness: f32| {
            if dock_side.vertical() {
                Rf::new(
                    dock.x + cross * k,
                    dock.y + along * k,
                    thickness * k,
                    length * k,
                )
            } else {
                Rf::new(
                    dock.x + along * k,
                    dock.y + cross * k,
                    length * k,
                    thickness * k,
                )
            }
        };
        let offsets = [12.0, 90.0, 184.0, 262.0, 340.0, 418.0, 496.0, 670.0, 574.0];
        let buttons = offsets.map(|offset| point(offset, 12.0, 72.0, 72.0));
        let dividers = [point(172.0, 20.0, 2.0, 56.0), point(658.0, 20.0, 2.0, 56.0)];

        let centered =
            |width: f32, y: f32| Rf::new((w - width * compact) / 2.0, y, width * compact, bar_h);
        let mut pages = if dock_side == Dock::Bottom {
            let mut r = centered(296.0, margin);
            if overlaps(r, left) || overlaps(r, right) {
                r.y = top;
            }
            r
        } else {
            let mut r = centered(296.0, bottom_y);
            if overlaps(r, history) || overlaps(r, zoom) {
                r.y = history.y - r.h - 16.0 * compact;
            }
            r
        };
        if self.board.is_none() {
            pages = Rf::default();
        }
        let text = self.ed.selected_text_count() > 0;
        let inks = self.ed.selected_ink_count() > 0;
        let count = 2 + if text { 3 } else { 0 } + if inks { 2 } else { 0 };
        let actions_width: f32 = self.object_actions().iter().map(|(_, width, _, _)| width + 6.0).sum();
        let selection_width = 16.0 + 78.0 * count as f32 + actions_width;
        let selection_scale = compact.min((w - 2.0 * margin) / selection_width);
        let sw = selection_width * selection_scale;
        let mut selection = Rf::new((w - sw) / 2.0, margin, sw, 88.0 * selection_scale);
        for top_control in [
            left,
            right,
            if dock_side == Dock::Top {
                dock
            } else {
                Rf::default()
            },
            pages,
        ] {
            if overlaps(selection, top_control) {
                selection.y = top_control.bottom() + 16.0 * compact;
            }
        }

        BoardChrome {
            left,
            right,
            history,
            zoom,
            dock,
            dock_scale: k,
            buttons,
            dividers,
            pages,
            selection,
        }
    }

    fn layout_board(&self, ui: &mut Ui, c: &BoardChrome) {
        self.layout_live(ui);
        if self.pad_shown() {
            let [x, y, w, h] = self.pad_area;
            let (sw, sh) = (self.w as f32, self.h as f32);
            let area = scale_rect(Rf::new(x * sw, y * sh, w * sw, h * sh), 1.0 / ui.u);
            ui.push(area, Look::PadArea, None);
            if area.w > 300.0 && area.h > 80.0 {
                ui.push(
                    Rf::new(area.x + 12.0, area.y + 12.0, 272.0, 52.0),
                    Look::Button {
                        icon: Some(Ico::Close),
                        text: "Disconnect phone".into(),
                        kind: ButtonKind::Secondary,
                        pill: true,
                        level: 1,
                    },
                    Some(Act::PadDisconnect),
                );
            }
        }
        let s = c.left.h / 88.0;
        let relative = |r: Rf, x: f32, y: f32, w: f32, h: f32, f: f32| {
            Rf::new(r.x + x * f, r.y + y * f, w * f, h * f)
        };
        let bar_button = |r, x| relative(r, x, 8.0, 72.0, 72.0, s);
        ui.surface(c.left, 28.0, 1, Some(Act::Nothing));
        ui.scaled_icon(
            bar_button(c.left, 8.0),
            Ico::Back,
            s,
            false,
            true,
            Act::Back,
        );
        if let Some(board) = &self.board {
            let (status, failed) = match self.save_state {
                SaveState::Failed => ("Save failed. Tap to retry", true),
                SaveState::Saving => ("Saving…", false),
                SaveState::Saved if self.unsaved => ("Saving…", false),
                SaveState::Saved => ("Saved", false),
            };
            let x = c.left.x + 96.0 * s;
            let rw = c.left.right() - x - 12.0 * s;
            let status_look = Look::TitleBlock {
                title: board.title.clone(),
                status: status.into(),
                failed,
            };
            ui.scaled(
                Rf::new(x, c.left.y, rw, c.left.h),
                status_look,
                Some(Act::Rename(board.id.clone())),
                s,
            );
            if failed {
                ui.push(
                    Rf::new(x, c.left.y + 48.0 * s, rw, c.left.h - 48.0 * s),
                    Look::Scrim(0),
                    Some(Act::Retry),
                );
            }
        }

        let menu = matches!(
            self.overlay,
            Some(Overlay::Menu {
                kind: MenuKind::Main,
                ..
            })
        );
        ui.surface(c.right, 28.0, 1, Some(Act::Nothing));
        let chip_w = c.right.w - 160.0 * s;
        if chip_w > 0.0 {
            self.layout_people(ui, Rf::new(c.right.x + 4.0, c.right.y + 8.0, chip_w - 4.0, c.right.h - 16.0));
        }
        ui.scaled_icon(
            bar_button(c.right, chip_w / s + 4.0),
            Ico::Share,
            s,
            false,
            true,
            Act::Share,
        );
        ui.scaled_icon(
            bar_button(c.right, chip_w / s + 80.0),
            Ico::More,
            s,
            menu,
            true,
            Act::Menu,
        );

        for bar in [c.history, c.zoom] {
            ui.surface(bar, 28.0, 1, Some(Act::Nothing));
        }
        let b = c.history.h / 88.0;
        ui.scaled_icon(
            relative(c.history, 8.0, 8.0, 72.0, 72.0, b),
            Ico::Undo,
            b,
            false,
            self.ed.doc.can_undo(),
            Act::Undo,
        );
        ui.scaled_icon(
            relative(c.history, 88.0, 8.0, 72.0, 72.0, b),
            Ico::Redo,
            b,
            false,
            self.ed.doc.can_redo(),
            Act::Redo,
        );
        ui.scaled_icon(
            relative(c.zoom, 8.0, 8.0, 72.0, 72.0, b),
            Ico::Minus,
            b,
            false,
            true,
            Act::ZoomOut,
        );
        ui.scaled(
            relative(c.zoom, 88.0, 8.0, 104.0, 72.0, b),
            Look::Label {
                text: format!("{:.0}%", self.ed.zoom() * 100.0),
                face: Face::Medium,
                size: 22.0,
                color: TEXT1,
                center: true,
            },
            Some(Act::Zoom100),
            b,
        );
        ui.scaled_icon(
            relative(c.zoom, 200.0, 8.0, 72.0, 72.0, b),
            Ico::Plus,
            b,
            false,
            true,
            Act::ZoomIn,
        );
        ui.scaled_icon(
            relative(c.zoom, 280.0, 8.0, 72.0, 72.0, b),
            Ico::Fit,
            b,
            false,
            true,
            Act::Fit,
        );

        let tool = self.ed.tool();
        let mode = self.ed.pen_mode();
        let selected = !self.ed.doc.selected_ids().is_empty();
        let shape_ico = self.last_shape.map_or(Ico::Shapes, |i| SHAPES[i].1);
        let tools = [
            (
                Ico::Select,
                tool == Tool::Select,
                Act::Tool(Tool::Select, PenMode::Ink),
            ),
            (
                Ico::Hand,
                tool == Tool::Hand,
                Act::Tool(Tool::Hand, PenMode::Ink),
            ),
            (
                Ico::Pen,
                tool == Tool::Pen && mode == PenMode::Ink && !self.highlighter,
                Act::Tool(Tool::Pen, PenMode::Ink),
            ),
            (Ico::Highlighter, self.highlighter, Act::Highlighter),
            (
                Ico::Eraser,
                tool == Tool::Eraser,
                Act::Tool(Tool::Eraser, PenMode::Ink),
            ),
            (
                shape_ico,
                self.shape_tool_active() || matches!(self.overlay, Some(Overlay::Shapes)),
                Act::Shapes,
            ),
            (
                Ico::Convert,
                tool == Tool::Pen && mode == PenMode::Write,
                Act::Tool(Tool::Pen, PenMode::Write),
            ),
        ];
        ui.scaled(
            c.dock,
            Look::Surface {
                color: SURFACE,
                radius: 32.0,
                level: 1,
                border: true,
            },
            Some(Act::Nothing),
            c.dock_scale,
        );
        for (r, (icon, active, act)) in c.buttons.iter().zip(tools) {
            ui.scaled_icon(*r, icon, c.dock_scale, active, true, act);
        }
        ui.scaled_icon(
            c.buttons[8],
            Ico::Plus,
            c.dock_scale,
            matches!(self.overlay, Some(Overlay::Insert { .. })),
            true,
            Act::Insert,
        );
        for r in c.dividers {
            ui.push(r, Look::Rule, None);
        }
        let style_open = matches!(self.overlay, Some(Overlay::Style));
        ui.scaled(
            c.buttons[7],
            Look::ColorChip {
                color: self.style_color(),
                open: style_open,
            },
            Some(Act::Style),
            c.dock_scale,
        );

        if c.pages.w > 0.0 {
            let r = c.pages;
            ui.surface(r, 28.0, 1, Some(Act::Nothing));
            ui.scaled_icon(
                relative(r, 8.0, 8.0, 64.0, 72.0, s),
                Ico::ChevronLeft,
                s,
                false,
                self.page > 0,
                Act::PagePrev,
            );
            ui.scaled(
                relative(r, 76.0, 8.0, 72.0, 72.0, s),
                Look::Label {
                    text: format!("{} / {}", self.page + 1, self.pages.len()),
                    face: Face::Medium,
                    size: 22.0,
                    color: TEXT1,
                    center: true,
                },
                (self.pages.len() > 1).then_some(Act::PageMenu),
                s,
            );
            ui.scaled_icon(
                relative(r, 152.0, 8.0, 64.0, 72.0, s),
                Ico::Chevron,
                s,
                false,
                self.page + 1 < self.pages.len(),
                Act::PageNext,
            );
            ui.scaled_icon(
                relative(r, 224.0, 8.0, 64.0, 72.0, s),
                Ico::Plus,
                s,
                false,
                true,
                Act::PageAdd,
            );
        }

        if selected {
            let r = c.selection;
            let f = r.h / 88.0;
            let mut x = 8.0;
            if let Some(line) = self.attribution_line() {
                let tw = font::measure(Face::Medium, 20.0, &line).0 + 40.0;
                let tag = Rf::new(r.cx() - tw / 2.0, r.bottom() + 10.0, tw, 44.0);
                ui.push(tag, Look::Surface { color: SURFACE, radius: 22.0, level: 1, border: false }, None);
                ui.label(tag, line, Face::Medium, 20.0, TEXT2, true);
            }
            ui.surface(r, 28.0, 1, Some(Act::Nothing));
            ui.scaled(
                relative(r, x, 8.0, 72.0, 72.0, f),
                Look::ColorChip {
                    color: self.style_color(),
                    open: style_open,
                },
                Some(Act::Style),
                f,
            );
            x += 78.0;
            for (label, width, act, enabled) in self.object_actions() {
                ui.scaled(
                    relative(r, x, 8.0, width, 72.0, f),
                    Look::Button {
                        icon: None,
                        text: label.into(),
                        kind: if enabled { ButtonKind::Secondary } else { ButtonKind::Disabled },
                        pill: false,
                        level: 0,
                    },
                    enabled.then_some(act),
                    f,
                );
                x += width + 6.0;
            }
            let mut items = Vec::with_capacity(5);
            if self.ed.selected_ink_count() > 0 {
                items.push((Ico::Convert, Act::Convert));
                items.push((Ico::Refine, Act::Refine));
            }
            if self.ed.selected_text_count() > 0 {
                items.extend([
                    (Ico::Font, Act::FontPanel),
                    (Ico::Bigger, Act::Bigger),
                    (Ico::Smaller, Act::Smaller),
                ]);
            }
            items.push((Ico::Trash, Act::Delete));
            for (icon, act) in items {
                ui.scaled_icon(
                    relative(r, x, 8.0, 72.0, 72.0, f),
                    icon,
                    f,
                    false,
                    true,
                    act,
                );
                x += 78.0;
            }
        }
    }

    fn popover(
        &self,
        dock: Rf,
        width: f32,
        height: f32,
        anchor: Rf,
        gap: f32,
        centered: bool,
    ) -> (Rf, f32) {
        let (w, h) = self.extent();
        let side = self.settings.dock;
        let edge = 16.0;
        let (avail_w, avail_h) = match side {
            Dock::Left => (w - dock.right() - gap - edge, h - 2.0 * edge),
            Dock::Right => (dock.x - gap - edge, h - 2.0 * edge),
            Dock::Bottom => (w - 2.0 * edge, dock.y - gap - edge),
            Dock::Top => (w - 2.0 * edge, h - dock.bottom() - gap - edge),
        };
        let scale = (avail_w / width).min(avail_h / height).clamp(0.0, 1.0);
        let (pw, ph) = (width * scale, height * scale);
        let x = match side {
            Dock::Left => dock.right() + gap,
            Dock::Right => dock.x - gap - pw,
            _ => place_in(anchor.cx() - pw / 2.0, edge, w - edge - pw),
        };
        let y = match side {
            Dock::Bottom => dock.y - gap - ph,
            Dock::Top => dock.bottom() + gap,
            _ => place_in(
                if centered {
                    anchor.cy() - ph / 2.0
                } else {
                    anchor.bottom() - ph
                },
                edge,
                h - edge - ph,
            ),
        };
        (Rf::new(x, y, pw, ph), scale)
    }

    fn layout_style(&self, ui: &mut Ui, c: &BoardChrome) {
        let fill = self.shape_tool_active();
        let height = if fill { 448.0 } else { 352.0 };
        let anchor = c.buttons[7];
        let (r, k) = self.popover(c.dock, 536.0, height, anchor, 20.0, false);
        if k <= 0.0 {
            return;
        }
        let at = |x, y, w, h| Rf::new(r.x + x * k, r.y + y * k, w * k, h * k);
        ui.scaled(
            r,
            Look::Surface {
                color: SURFACE,
                radius: 28.0,
                level: 2,
                border: true,
            },
            Some(Act::Nothing),
            k,
        );
        let n = 12.0 * k;
        let side = self.settings.dock;
        let notch = if side.vertical() {
            let cy = place_in(anchor.cy(), r.y + 25.0 * k, r.bottom() - 25.0 * k);
            let x = if side == Dock::Left {
                r.x - n
            } else {
                r.right() - 2.0 * k
            };
            Rf::new(x, cy - n, n + 2.0 * k, 2.0 * n)
        } else {
            let cx = place_in(anchor.cx(), r.x + 25.0 * k, r.right() - 25.0 * k);
            let y = if side == Dock::Top {
                r.y - n
            } else {
                r.bottom() - 2.0 * k
            };
            Rf::new(cx - n, y, 2.0 * n, n + 2.0 * k)
        };
        ui.scaled(notch, Look::Notch(side), Some(Act::Nothing), k);
        ui.scaled(
            at(24.0, 18.0, 488.0, 22.0),
            Look::Overline("COLOR".into()),
            None,
            k,
        );
        let current_color = self.style_color();
        for (i, color) in PALETTE.into_iter().enumerate() {
            let chosen = current_color == color;
            ui.scaled(
                at(
                    24.0 + (i % 6) as f32 * 80.0,
                    46.0 + (i / 6) as f32 * 80.0,
                    72.0,
                    72.0,
                ),
                Look::Swatch { color, chosen },
                Some(Act::Color(color)),
                k,
            );
        }
        ui.scaled(
            at(24.0, 218.0, 488.0, 24.0),
            Look::Overline("THICKNESS".into()),
            None,
            k,
        );
        let widths = if self.highlighter {
            HL_WIDTHS
        } else {
            PEN_WIDTHS
        };
        let samples = if self.highlighter {
            [8.0, 14.0, 22.0, 32.0]
        } else {
            [3.0, 6.0, 10.0, 18.0]
        };
        let width = self
            .ed
            .doc
            .selected_shapes()
            .first()
            .map_or(self.ed.stroke_width(), |s| s.width);
        for (i, preview) in samples.into_iter().enumerate() {
            ui.scaled(
                at(24.0 + i as f32 * 122.0, 248.0, 112.0, 80.0),
                Look::Thick {
                    width: preview,
                    chosen: (widths[i] - width).abs() < 0.01,
                },
                Some(Act::Width(i)),
                k,
            );
        }
        if fill {
            ui.scaled(
                at(24.0, 352.0, 152.0, 64.0),
                Look::Label {
                    text: "Fill".into(),
                    face: Face::Medium,
                    size: 24.0,
                    color: TEXT1,
                    center: false,
                },
                None,
                k,
            );
            for (rr, look, act) in segment(
                Rf::new(184.0, 352.0, 328.0, 64.0),
                [("Outline", Act::Fill(false)), ("Filled", Act::Fill(true))],
                usize::from(self.ed.filled()),
            ) {
                ui.scaled(at(rr.x, rr.y, rr.w, rr.h), look, act, k);
            }
        }
    }

    fn layout_shapes(&self, ui: &mut Ui, c: &BoardChrome) {
        let anchor = c.buttons[5];
        let (r, k) = self.popover(c.dock, 408.0, 174.0, anchor, 20.0, true);
        ui.scaled(
            r,
            Look::Surface {
                color: SURFACE,
                radius: 28.0,
                level: 2,
                border: true,
            },
            Some(Act::Nothing),
            k,
        );
        for (i, (_, icon, _)) in SHAPES.iter().enumerate() {
            let rr = square(
                r.x + (12.0 + 78.0 * (i % 5) as f32) * k,
                r.y + (12.0 + 78.0 * (i / 5) as f32) * k,
                72.0 * k,
            );
            ui.scaled_icon(
                rr,
                *icon,
                k,
                self.last_shape == Some(i),
                true,
                Act::Shape(i),
            );
        }
    }

    fn layout_overlay(&self, ui: &mut Ui, overlay: &Overlay, chrome: Option<&BoardChrome>) {
        let (w, h) = self.extent();
        let screen = Rf::new(0.0, 0.0, w, h);
        let fade = self.fade_in(self.overlay_start);
        ui.opacity = (255.0 * fade).round() as u8;
        match overlay {
            Overlay::TableSize { rows, columns } => {
                ui.push(screen, Look::Scrim(SCRIM), Some(Act::Nothing));
                self.layout_table_size(ui, *rows, *columns);
            }
            Overlay::Style | Overlay::Shapes => {
                ui.push(screen, Look::Scrim(0), Some(Act::Outside));
                ui.offset_y = 8.0 * (1.0 - fade);
                if let Some(chrome) = chrome {
                    if matches!(overlay, Overlay::Style) {
                        self.layout_style(ui, chrome);
                    } else {
                        self.layout_shapes(ui, chrome);
                    }
                }
            }
            Overlay::Insert { pdf } => {
                ui.push(screen, Look::Scrim(0), Some(Act::Outside));
                ui.offset_y = 8.0 * (1.0 - fade);
                if let Some(chrome) = chrome {
                    self.layout_insert(ui, chrome, *pdf);
                }
            }
            Overlay::GraphRange => {
                ui.push(screen, Look::Scrim(0), Some(Act::Outside));
                if let Some(chrome) = chrome {
                    self.layout_graph_range(ui, chrome.selection);
                }
            }
            Overlay::Menu { kind, anchor } => {
                let scrim = if matches!(kind, MenuKind::Main) {
                    SCRIM_LIGHT
                } else {
                    0
                };
                ui.push(screen, Look::Scrim(scrim), Some(Act::Outside));
                ui.offset_y = 8.0 * (1.0 - fade);
                self.layout_menu(ui, kind, *anchor);
            }
            Overlay::Panel { kind, scroll } => {
                ui.push(screen, Look::Scrim(SCRIM), Some(Act::Nothing));
                self.layout_panel(ui, *kind, *scroll);
            }
            Overlay::Ask {
                title,
                text,
                hint,
                multiline,
                purpose,
            } => {
                ui.push(screen, Look::Scrim(SCRIM), Some(Act::Nothing));
                let dw = 800.0_f32.min(w - 48.0);
                let dh = if *multiline { 520.0_f32 } else { 400.0 }.min(h - 32.0);
                let r = Rf::new((w - dw) / 2.0, 150.0_f32.min((h - dh) * 0.36), dw, dh);
                let inset = 48.0_f32.min(dw / 10.0);
                ui.surface(r, 32.0, 3, Some(Act::Nothing));
                ui.label(
                    Rf::new(r.x + inset, r.y + 32.0, dw - 2.0 * inset, 52.0),
                    title,
                    Face::Bold,
                    36.0,
                    TEXT1,
                    false,
                );
                let height = if *multiline { 224.0_f32 } else { 96.0 }.min((dh - 232.0).max(64.0));
                ui.push(
                    Rf::new(r.x + inset, r.y + 120.0, dw - 2.0 * inset, height),
                    Look::Field {
                        text: text.clone(),
                        hint: hint.clone(),
                        multiline: *multiline,
                    },
                    Some(Act::Field),
                );
                let ok = match purpose {
                    AskFor::Rename(_) | AskFor::RenameTeam(_) => "Save",
                    AskFor::AddPerson => "Invite",
                    AskFor::Text { .. } | AskFor::Cell { .. } | AskFor::Math { .. } => "Done",
                };
                self.dialog_buttons(ui, r, inset, "Cancel", ok, ButtonKind::Primary);
            }
            Overlay::Confirm {
                title,
                body,
                cancel,
                ok,
                purpose,
            } => {
                ui.push(screen, Look::Scrim(SCRIM), Some(Act::Nothing));
                let dw = 800.0_f32.min(w - 48.0);
                let dh = 380.0_f32.min(h - 32.0);
                let r = Rf::new((w - dw) / 2.0, (h - dh) / 2.0, dw, dh);
                let inset = 48.0_f32.min(dw / 10.0);
                let inner = dw - 2.0 * inset;
                ui.surface(r, 32.0, 3, Some(Act::Nothing));
                ui.label(
                    Rf::new(r.x + inset, r.y + 32.0, inner, 52.0),
                    title,
                    Face::Bold,
                    36.0,
                    TEXT1,
                    false,
                );
                ui.push(
                    Rf::new(r.x + inset, r.y + 114.0, inner, (dh - 224.0).max(34.0)),
                    Look::Para {
                        text: body.clone(),
                        face: Face::Regular,
                        size: 24.0,
                        line: 34.0,
                        color: TEXT2,
                    },
                    None,
                );
                let kind = if matches!(purpose, ConfirmFor::Install(_)) {
                    ButtonKind::Primary
                } else {
                    ButtonKind::Danger
                };
                self.dialog_buttons(ui, r, inset, cancel, ok, kind);
            }
        }
        ui.opacity = 255;
        ui.offset_y = 0.0;
    }

    fn layout_menu(&self, ui: &mut Ui, kind: &MenuKind, anchor: Rf) {
        let (w, h) = self.extent();
        let selected_font = text::FONT_NAMES[self.settings.font as usize];
        let row = |icon, label: &str, value: &str, chevron, danger, act| {
            Some((
                Look::Row {
                    icon: Some(icon),
                    text: label.into(),
                    value: value.into(),
                    chevron,
                    danger,
                },
                act,
            ))
        };
        let (width, rows) = match kind {
            MenuKind::Main => {
                let update = match &self.update {
                    Some((version, _)) => format!("{version} ready"),
                    None if self.updates_current => "Up to date".to_string(),
                    None => String::new(),
                };
                (
                    560.0_f32,
                    vec![
                        row(Ico::Share, if self.live.is_some() { "Share and people…" } else { "Share board…" }, "", false, false, Act::Share),
                        row(
                            Ico::Claude,
                            "Connect Claude…",
                            "",
                            false,
                            false,
                            Act::Claude,
                        ),
                        row(Ico::Hand, "Phone remote…", "", false, false, Act::Remote),
                        row(Ico::Export, "Export as PNG", "", false, false, Act::Export),
                        row(Ico::Import, "Import boards…", "", false, false, Act::Import),
                        row(
                            Ico::Join,
                            if self.me().is_some() { "Account…" } else { "Sign in…" },
                            self.me().map_or("", |u| u.email.as_str()),
                            false,
                            false,
                            Act::Account,
                        ),
                        None,
                        row(Ico::Gear, "Settings…", "", true, false, Act::Settings),
                        row(
                            Ico::Font,
                            "Text font…",
                            selected_font,
                            true,
                            false,
                            Act::FontPanel,
                        ),
                        row(
                            Ico::Update,
                            "Check for updates",
                            &update,
                            false,
                            false,
                            Act::Updates,
                        ),
                    ],
                )
            }
            MenuKind::TeamCard(_) | MenuKind::Member(_) | MenuKind::Account => team::menu_rows(self, kind).unwrap_or((320.0, Vec::new())),
            MenuKind::Pages => (
                320.0,
                vec![row(
                    Ico::Trash,
                    "Delete page…",
                    "",
                    false,
                    true,
                    Act::AskDeletePage,
                )],
            ),
        };
        let width = width.min(w - 32.0);
        // The account menu opens with who is signed in.
        let head = if matches!(kind, MenuKind::Account) && self.me().is_some() { 120.0 } else { 0.0 };
        let height = 24.0
            + head
            + rows
                .iter()
                .map(|row| if row.is_some() { 72.0 } else { 16.0 })
                .sum::<f32>();
        let k = ((h - 32.0) / height).clamp(0.0, 1.0);
        let right = if matches!(kind, MenuKind::Main) {
            w - 24.0
        } else {
            anchor.right()
        };
        let x = place_in(right - width, 16.0, w - width - 16.0);
        let below = anchor.bottom() + 20.0;
        let y = place_in(
            if below + height * k <= h - 16.0 {
                below
            } else if matches!(kind, MenuKind::Main) {
                anchor.bottom() + 8.0
            } else {
                anchor.y - height * k - 12.0
            },
            16.0,
            h - height * k - 16.0,
        );
        let rect = Rf::new(x, y, width, height * k);
        ui.scaled(
            rect,
            Look::Surface {
                color: SURFACE,
                radius: 28.0,
                level: 2,
                border: true,
            },
            Some(Act::Nothing),
            k,
        );
        let mut y = y + 12.0 * k;
        if let Some(me) = self.me().filter(|_| head > 0.0) {
            ui.scaled(square(x + 24.0, y + 20.0, 72.0 * k), Look::Avatar { text: me.initials(), color: me.color }, None, k);
            let text_x = x + 24.0 + 90.0 * k;
            let text_w = (x + width - 24.0 - text_x).max(0.0);
            let name = if me.name.is_empty() { me.email.as_str() } else { me.name.as_str() };
            ui.scaled(Rf::new(text_x, y + 22.0 * k, text_w, 36.0 * k), Look::Label { text: name.into(), face: Face::Bold, size: 28.0, color: TEXT1, center: false }, None, k);
            ui.scaled(Rf::new(text_x, y + 60.0 * k, text_w, 32.0 * k), Look::Label { text: me.email.clone(), face: Face::Regular, size: 22.0, color: TEXT3, center: false }, None, k);
            y += head * k;
            ui.push(Rf::new(x + 24.0, y - 6.0 * k, width - 48.0, 2.0 * k), Look::Rule, None);
        }
        for row in rows {
            if let Some((look, act)) = row {
                ui.scaled(
                    Rf::new(x + 12.0, y, width - 24.0, 72.0 * k),
                    look,
                    Some(act),
                    k,
                );
                y += 72.0 * k;
            } else {
                ui.push(
                    Rf::new(x + 24.0, y + 7.0 * k, width - 48.0, 2.0 * k),
                    Look::Rule,
                    None,
                );
                y += 16.0 * k;
            }
        }
    }

    fn remote_url(&self) -> Option<String> {
        let ip = self.lan_ip?;
        Some(format!("http://{ip}:{}/r#k={}", net::PORT, self.settings.remote_key))
    }

    /// The phone remote: a page that mirrors the screen and sends touches back.
    fn remote(&mut self, path: &str, req: &net::Req) -> net::Resp {
        if path == "/r" {
            return net::Resp {
                status: 200,
                ctype: "text/html; charset=utf-8",
                body: include_str!("remote.html").as_bytes().to_vec(),
            };
        }
        if req.query.get("k").map(String::as_str) != Some(self.settings.remote_key.as_str()) {
            return net::Resp::empty(403);
        }
        let body = serde_json::from_slice::<Value>(&req.body).unwrap_or(Value::Null);
        let was_shown = self.pad_shown();
        self.pad_seen = Some(self.now);
        let board = self.screen == Screen::Board && self.board.is_some();
        let (w, h) = (self.w as f32, self.h as f32);
        let num = |v: &Value, k: &str| v.get(k).and_then(Value::as_f64).map(|f| f as f32);
        match path {
            "/r/state" => {}
            "/r/bye" => {
                self.pad_seen = None;
                return net::Resp::empty(204);
            }
            "/r/area" if req.method == "POST" => {
                // The phone sends the box's x, y and width; its height follows
                // the pad's shape so writing is not stretched.
                let aspect = num(&body, "aspect").unwrap_or(1.0).clamp(0.2, 5.0);
                let bw = num(&body, "w").unwrap_or(self.pad_area[2]).clamp(0.05, 1.0);
                let mut bh = bw * w.max(1.0) / h.max(1.0) / aspect;
                let mut bw = bw;
                if bh > 1.0 {
                    bw /= bh;
                    bh = 1.0;
                }
                let x = num(&body, "x").unwrap_or(self.pad_area[0]).clamp(0.0, 1.0 - bw);
                let y = num(&body, "y").unwrap_or(self.pad_area[1]).clamp(0.0, 1.0 - bh);
                self.pad_area = [x, y, bw, bh];
            }
            "/r/pan" if req.method == "POST" && board => {
                let fx = num(&body, "fx").unwrap_or(0.5) * w;
                let fy = num(&body, "fy").unwrap_or(0.5) * h;
                let dx = num(&body, "dx").unwrap_or(0.0) * w;
                let dy = num(&body, "dy").unwrap_or(0.0) * h;
                let scale = num(&body, "scale").unwrap_or(1.0).clamp(0.5, 2.0);
                self.ed.pan_zoom(
                    Pt::new(dx as f64, dy as f64),
                    scale as f64,
                    Pt::new(fx as f64, fy as f64),
                );
            }
            "/r/cmd" if req.method == "POST" && board => {
                match body.get("cmd").and_then(Value::as_str) {
                    Some("undo") => self.ed.undo(),
                    Some("redo") => self.ed.redo(),
                    _ => {}
                }
                match body.get("tool").and_then(Value::as_str) {
                    Some("pen") => self.choose_tool(Tool::Pen, PenMode::Ink, false),
                    Some("highlighter") => self.choose_tool(Tool::Pen, PenMode::Ink, true),
                    Some("shapes") => self.choose_tool(Tool::Pen, PenMode::Shape, false),
                    Some("text") => self.choose_tool(Tool::Pen, PenMode::Write, false),
                    _ => {}
                }
                if let Some(c) = body.get("color").and_then(Value::as_u64) {
                    if let Some(&c) = PALETTE.iter().find(|p| **p == c as u32) {
                        self.run(Act::Color(c));
                    }
                }
                if let Some(i) = body.get("width").and_then(Value::as_u64) {
                    if (i as usize) < PEN_WIDTHS.len() {
                        self.run(Act::Width(i as usize));
                    }
                }
                self.take_editor_events();
            }
            "/r/view" if board => {
                let want = req.query.get("w").and_then(|s| s.parse::<u32>().ok()).unwrap_or(1000).clamp(200, 1600);
                return match self.pad_view(want) {
                    Some((w, h, jpg)) => {
                        use base64::Engine;
                        let b64 = base64::engine::general_purpose::STANDARD.encode(jpg);
                        net::Resp::json(200, &json!({ "doc": self.doc_gen, "w": w, "h": h, "jpg": b64 }))
                    }
                    None => net::Resp::empty(503),
                };
            }
            "/r/input" if req.method == "POST" && board => {
                let erase = body.get("erase").and_then(Value::as_bool).unwrap_or(false);
                let events = body.get("events").and_then(Value::as_array).cloned().unwrap_or_default();
                let [ax, ay, aw, ah] = self.pad_area;
                let base = self.now.max(self.remote_t + 1);
                for e in events.iter().take(4000) {
                    let n = |i: usize| e.get(i).and_then(Value::as_f64);
                    let (Some(phase), Some(id), Some(x), Some(y)) = (n(0), n(1), n(2), n(3)) else {
                        continue;
                    };
                    let time = base + n(4).unwrap_or(0.0).clamp(0.0, 10000.0) as u64;
                    self.remote_t = self.remote_t.max(time);
                    let id = 1000 + (id as i32).clamp(0, 9);
                    let pos = Pt::new(
                        ((ax + x.clamp(0.0, 1.0) as f32 * aw) * w) as f64,
                        ((ay + y.clamp(0.0, 1.0) as f32 * ah) * h) as f64,
                    );
                    let contact = Contact { size: 0.0, stylus: false, eraser: erase, norm: 0.0 };
                    match phase as i64 {
                        0 => self.ed.pointer_down_with(id, pos, time, contact),
                        1 => self.ed.pointer_move_with(id, pos, time, contact),
                        p => self.ed.pointer_up(id, pos, time, p != 2),
                    }
                }
                self.take_editor_events();
            }
            _ if !board && req.method == "POST" => {}
            _ => return net::Resp::empty(404),
        }
        if !was_shown {
            self.full = true;
        }
        let v = self.ed.screen_view();
        let (stroke, widths) = if self.highlighter {
            (&self.hl_style, &HL_WIDTHS)
        } else {
            (&self.pen_style, &PEN_WIDTHS)
        };
        let width = widths.iter().position(|x| (*x - stroke.width).abs() < 1e-6);
        let color = stroke.color;
        net::Resp::json(
            200,
            &json!({
                "board": board,
                "area": self.pad_area,
                "screen": [self.w, self.h],
                "title": self.board.as_ref().map(|b| b.title.clone()),
                "doc": self.doc_gen,
                "view": [v.cam.x, v.cam.y, v.zoom, v.dpr],
                "strokeWidth": self.ed.stroke_width(),
                "tool": self.pad_tool(),
                "color": color,
                "width": width,
                "palette": PALETTE,
            }),
        )
    }

    /// The tool as the phone pad names it.
    fn pad_tool(&self) -> &'static str {
        match (self.ed.tool(), self.ed.pen_mode()) {
            (Tool::Pen, _) if self.highlighter => "highlighter",
            (Tool::Pen, PenMode::Ink) => "pen",
            (Tool::Pen, PenMode::Shape) => "shapes",
            (Tool::Pen, PenMode::Write) => "text",
            (Tool::Eraser, _) => "eraser",
            _ => "other",
        }
    }

    /// The board under the phone pad's box, `want` pixels wide, as JPEG.
    fn pad_view(&self, want: u32) -> Option<(u32, u32, Vec<u8>)> {
        let [ax, ay, aw, ah] = self.pad_area;
        let (bw, bh) = (aw * self.w as f32, ah * self.h as f32);
        if bw < 1.0 || bh < 1.0 {
            return None;
        }
        let w = want;
        let h = ((w as f32 * bh / bw).round() as u32).clamp(1, 4000);
        let scale = (w as f32 / bw) as f64;
        let v = self.ed.screen_view();
        let origin = Pt::new((ax * self.w as f32) as f64, (ay * self.h as f32) as f64);
        let view = render::View {
            cam: (v.cam * v.dpr - origin) * scale,
            zoom: v.zoom * v.dpr * scale,
            dpr: 1.0,
        };
        let rgba = render::view_image(self.ed.doc.shapes(), w, h, view);
        let img = image::RgbaImage::from_raw(w, h, rgba)?;
        let rgb = image::DynamicImage::ImageRgba8(img).to_rgb8();
        let mut jpg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpg, 82)
            .encode_image(&rgb)
            .ok()?;
        Some((w, h, jpg))
    }

    /// The phone pad's box shows on the board while the pad is in use.
    fn pad_shown(&self) -> bool {
        self.pad_seen.is_some_and(|t| self.now.saturating_sub(t) < PAD_TTL)
    }

    fn lan_mcp(&self) -> Option<String> {
        let ip = self.lan_ip?;
        Some(format!(
            "http://{ip}:{}/mcp/{}",
            net::PORT,
            self.settings.mcp_key
        ))
    }

    fn panel_frame(&self, kind: PanelKind) -> Rf {
        let (w, h) = self.extent();
        let (width, height) = match kind {
            PanelKind::Settings => (1360.0_f32, 824.0_f32),
            PanelKind::Claude => (960.0, 940.0),
            PanelKind::Remote => (760.0, 900.0),
            PanelKind::Fonts => (960.0, 760.0),
            PanelKind::Share => (880.0, 980.0),
            PanelKind::Account => (720.0, 920.0),
        };
        let dw = width.min(w - 48.0).max(0.0);
        let dh = height.min(h - 32.0).max(0.0);
        Rf::new((w - dw) / 2.0, (h - dh) / 2.0, dw, dh)
    }

    fn panel_viewport(&self, kind: PanelKind, r: Rf) -> Rf {
        let inset = match kind {
            PanelKind::Share | PanelKind::Account => 48.0_f32,
            PanelKind::Settings => 40.0,
            PanelKind::Claude | PanelKind::Fonts | PanelKind::Remote => 24.0,
        }
        .min(r.w / 10.0);
        let footer = if kind == PanelKind::Share {
            144.0
        } else {
            28.0
        };
        Rf::new(
            r.x + inset,
            r.y + 112.0,
            r.w - 2.0 * inset,
            (r.h - 112.0 - footer).max(0.0),
        )
    }

    fn panel_max_scroll(&self) -> f32 {
        match self.overlay {
            Some(Overlay::Panel { kind, .. }) => {
                let view = self.panel_viewport(kind, self.panel_frame(kind));
                let (_, content) = self.panel_rows(kind, view.w);
                ((content - view.h) * self.unit()).max(0.0)
            }
            _ => 0.0,
        }
    }

    /// Content of a panel, in logical coordinates relative to its viewport.
    fn panel_rows(&self, kind: PanelKind, inner: f32) -> (Vec<UiRow>, f32) {
        let mut content = PanelRows::new(inner, self.unit());
        match kind {
            PanelKind::Settings => return self.settings_rows(inner),
            PanelKind::Fonts => {
                let cols = if inner >= 744.0 {
                    3
                } else if inner >= 360.0 {
                    2
                } else {
                    1
                };
                let gap = 12.0;
                let width = (inner - gap * (cols - 1) as f32) / cols as f32;
                for (i, _) in text::FONT_NAMES.iter().enumerate() {
                    let x = (i % cols) as f32 * (width + gap);
                    let y = (i / cols) as f32 * (80.0 + gap);
                    content.rows.push((
                        Rf::new(x, y, width, 80.0),
                        Look::FontTile {
                            font: i as u8,
                            chosen: self.settings.font == i as u8,
                        },
                        Some(Act::Font(i as u8)),
                    ));
                }
                content.y = text::FONT_NAMES.len().div_ceil(cols) as f32 * (80.0 + gap) - gap;
            }
            PanelKind::Remote => {
                content.para("Scan with a phone on the same Wi-Fi to use it as a writing pad. The phone shows the part of the board inside the blue box, and what you write lands there. Pick the pen, colour and size, or move the box or the board, from the phone.", TEXT2);
                if let Some(url) = self.remote_url() {
                    content.qr(&url);
                    content.copy_code(&url);
                    content.push(
                        64.0,
                        Look::Button {
                            icon: Some(Ico::Close),
                            text: "Disconnect phone".into(),
                            kind: ButtonKind::Secondary,
                            pill: false,
                            level: 0,
                        },
                        Some(Act::PadDisconnect),
                    );
                    content.y += 8.0;
                    content.para("Disconnecting gives this board a new code, so the old link stops working.", TEXT2);
                } else {
                    content.para("Connect to Wi-Fi or Ethernet to use the phone remote.", TEXT2);
                }
            }
            PanelKind::Claude => {
                content.para("Add Inkboard as an MCP connector so Claude can read and edit the open board, add diagrams and get a picture of it.", TEXT2);
                content.heading("On this network");
                if let Some(url) = self.lan_mcp() {
                    content.status("Local address available", true);
                    content.copy_code(&url);
                } else {
                    content.para(
                        "Connect to Wi-Fi or Ethernet to get a local address.",
                        TEXT2,
                    );
                }
                content.heading("Claude Code");
                if let Some(url) = self.lan_mcp() {
                    content.copy_code(&format!("claude mcp add --transport http inkboard {url}"));
                }
                content.para("In Claude, open Settings → Connectors → Add custom connector. Name it Inkboard and paste the local address. The computer running Claude must be on the same network.", TEXT2);
                content.para(
                    "Anyone with these addresses can change the open board. Keep them private.",
                    DANGER,
                );
            }
            PanelKind::Share => self.share_rows(&mut content),
            PanelKind::Account => self.account_rows(&mut content),
        }
        (content.rows, content.y)
    }

    fn settings_rows(&self, inner: f32) -> (Vec<UiRow>, f32) {
        let s = &self.settings;
        let large = inner >= 1000.0;
        let gap = 80.0;
        let width = if large {
            ((inner - gap) / 2.0).min(580.0)
        } else {
            inner
        };
        let mut left = PanelRows::new(width, self.unit());
        left.heading("WRITING");
        if self.is_board_open() {
            left.toggle("Tidy drawn shapes", None, s.tidy, Act::Toggle(Opt::Tidy));
        }
        left.toggle(
            "Handwriting to typed text",
            None,
            s.typed,
            Act::Toggle(Opt::Typed),
        );
        left.push(
            80.0,
            Look::Row {
                icon: None,
                text: "Text font".into(),
                value: text::FONT_NAMES[s.font as usize].into(),
                chevron: true,
                danger: false,
            },
            Some(Act::FontPanel),
        );
        left.y += 16.0;
        left.heading("TOUCH");
        left.toggle("Ignore palm touches", None, s.palm, Act::Toggle(Opt::Palm));
        left.toggle(
            "Pen draws and fingers move the board",
            None,
            s.pen_priority,
            Act::Toggle(Opt::PenPriority),
        );
        left.toggle(
            "Several fingers can write at once",
            None,
            s.multi_ink,
            Act::Toggle(Opt::MultiInk),
        );
        left.toggle(
            "Finger and pen back erase",
            None,
            s.wide_erase,
            Act::Toggle(Opt::WideErase),
        );

        let mut right = PanelRows::new(width, self.unit());
        right.heading("CANVAS");
        right.toggle("Show grid", None, s.grid, Act::Toggle(Opt::Grid));
        if self.is_board_open() {
            right.toggle("Hold to snap shapes", None, s.snap, Act::Toggle(Opt::Snap));
        }
        right.segments(
            "Dock on",
            356.0,
            ["Left", "Right", "Bottom", "Top"]
                .into_iter()
                .zip(Dock::ALL.map(Act::Dock)),
            s.dock as usize,
        );
        right.y += 16.0;
        right.heading("DISPLAY");
        right.toggle(
            "Full panel resolution",
            Some("Sharper, but slower on 4K"),
            s.full_res,
            Act::FullRes,
        );
        right.toggle(
            "Low-latency ink",
            Some("Draws the pen tip ahead of the screen"),
            s.fast_ink,
            Act::Toggle(Opt::FastInk),
        );
        right.toggle("Smooth ink", None, s.smooth, Act::Toggle(Opt::Smooth));
        right.segments(
            "Ink prediction",
            380.0,
            ["Off", "Low", "Medium", "High"]
                .into_iter()
                .enumerate()
                .map(|(i, label)| (label, Act::Predict(i as u8))),
            s.predict as usize,
        );
        if !self.is_board_open() {
            right.y += 8.0;
            right.push(
                72.0,
                Look::Row {
                    icon: Some(Ico::Import),
                    text: "Import boards…".into(),
                    value: String::new(),
                    chevron: false,
                    danger: false,
                },
                Some(Act::Import),
            );
        }
        let (x, y) = if large {
            (width + gap, 0.0)
        } else {
            (0.0, left.y + 24.0)
        };
        left.rows
            .extend(right.rows.into_iter().map(|(mut r, look, act)| {
                r.x += x;
                r.y += y;
                (r, look, act)
            }));
        (left.rows, left.y.max(y + right.y))
    }

    fn layout_panel(&self, ui: &mut Ui, kind: PanelKind, scroll: f32) {
        let r = self.panel_frame(kind);
        let view = self.panel_viewport(kind, r);
        let title = match kind {
            PanelKind::Share => "Share",
            PanelKind::Account => {
                if self.me().is_some() {
                    "Your account"
                } else {
                    "Sign in"
                }
            }
            PanelKind::Claude => "Connect Claude",
            PanelKind::Remote => "Phone remote",
            PanelKind::Fonts => "Text font",
            PanelKind::Settings => "Settings",
        };
        ui.surface(r, 32.0, 3, Some(Act::Nothing));
        ui.label(
            Rf::new(view.x, r.y + 24.0, view.w - 76.0, 60.0),
            title,
            Face::Bold,
            36.0,
            TEXT1,
            false,
        );
        ui.icon(
            square(view.right() - 72.0, r.y + 16.0, 72.0),
            Ico::Close,
            false,
            true,
            Act::Cancel,
        );

        let (rows, content) = self.panel_rows(kind, view.w);
        let scroll = (scroll / ui.u).clamp(0.0, (content - view.h).max(0.0));
        ui.clip = Some(scale_rect(view, ui.u).covering(0.0));
        for (rr, look, act) in rows {
            let r = Rf::new(view.x + rr.x, view.y + rr.y - scroll, rr.w, rr.h);
            ui.push(r, look, act);
        }
        ui.clip = None;

        if kind == PanelKind::Share {
            let y = r.bottom() - 120.0;
            let scale = (view.w / 632.0).min(1.0);
            if let Some(live) = &self.live {
                ui.button(
                    Rf::new(view.x, y, (view.w - 176.0 * scale - 16.0).min(240.0), 72.0),
                    Some(Ico::Link),
                    "Copy link",
                    ButtonKind::Secondary,
                    false,
                    Act::CopyBoardLink(live.cloud_id.clone()),
                );
            }
            ui.button(
                Rf::new(view.right() - 176.0 * scale, y, 176.0 * scale, 72.0),
                None,
                "Done",
                ButtonKind::Primary,
                false,
                Act::Done,
            );
        }
    }

    // ------------------------------------------------------------ network

    fn take_net(&mut self) {
        while let Ok(req) = self.net_rx.try_recv() {
            let resp = self.answer(&req);
            let _ = req.reply.send(resp);
        }
    }

    fn answer(&mut self, req: &net::Req) -> net::Resp {
        let path = req.path.trim_end_matches('/');
        if let Some(key) = path.strip_prefix("/mcp/") {
            if key != self.settings.mcp_key {
                return net::Resp::empty(404);
            }
            let version = self.version.clone();
            return net::mcp(self, &req.method, &req.body, &version);
        }
        if path == "/r" || path.starts_with("/r/") {
            return self.remote(path, req);
        }
        if path == "/health" {
            return net::Resp::json(200, &json!({ "app": "Inkboard", "version": self.version }));
        }
        net::Resp::empty(404)
    }

    fn dialog_buttons(
        &self,
        ui: &mut Ui,
        r: Rf,
        inset: f32,
        cancel: &str,
        ok: &str,
        kind: ButtonKind,
    ) {
        let width = 176.0_f32.min((r.w - 2.0 * inset - 16.0) / 2.0);
        let x = r.right() - inset - width;
        let y = r.bottom() - 120.0;
        ui.button(
            Rf::new(x - width - 16.0, y, width, 72.0),
            None,
            cancel,
            ButtonKind::Secondary,
            false,
            Act::Cancel,
        );
        ui.button(Rf::new(x, y, width, 72.0), None, ok, kind, false, Act::Done);
    }

    // ------------------------------------------------------------ drawing

    fn draw(&self, p: &mut Painter, n: &Node) {
        let r = n.rect;
        let u = n.u;
        let radius = n.radius();
        let pressed = n.hl == Hl::Pressed;
        match &n.look {
            Look::Scrim(color) => {
                if *color != 0 {
                    p.rrect(r, 0.0, *color);
                }
            }
            Look::Surface {
                color,
                level,
                border,
                ..
            } => {
                draw_surface(p, r, radius, u, *level, *color, *border);
                if pressed {
                    p.rrect(r, radius, 0x0d14161c);
                }
            }
            Look::IconBtn {
                ico,
                size,
                selected,
                enabled,
            } => {
                let color = if *selected {
                    p.rrect(r, radius, if pressed { ACCENT_PRESSED } else { ACCENT });
                    SURFACE
                } else if *enabled {
                    if pressed {
                        p.rrect(r, radius, SURFACE2);
                    }
                    TEXT1
                } else {
                    DISABLED
                };
                p.ico(*ico, size * u, r.cx(), r.cy(), color);
            }
            Look::ColorChip { color, open } => {
                if pressed || *open {
                    p.rrect(r, radius, SURFACE2);
                }
                p.circle(r.cx(), r.cy(), 27.0 * u, BORDER);
                p.circle(r.cx(), r.cy(), 25.0 * u, SURFACE);
                p.circle(
                    r.cx(),
                    r.cy(),
                    if pressed { 20.0 } else { 22.0 } * u,
                    *color,
                );
            }
            Look::Label {
                text,
                face,
                size,
                color,
                center,
            } => {
                if pressed {
                    p.rrect(r, radius, 0x1214161c);
                }
                line_text(p, r, *face, size * u, text, *color, *center);
            }
            Look::Para {
                text,
                face,
                size,
                line,
                color,
            } => {
                let line = line * u;
                for (i, text) in paint::wrap(*face, size * u, text, r.w).iter().enumerate() {
                    let row = Rf::new(r.x, r.y + line * i as f32, r.w, line);
                    if row.bottom() > r.bottom() + 0.5 {
                        break;
                    }
                    line_text(p, row, *face, size * u, text, *color, false);
                }
            }
            Look::Overline(text) => {
                let size = 18.0 * u;
                let y = baseline(r, Face::Bold, size);
                let mut x = r.x;
                for c in text.chars() {
                    let mut encoded = [0; 4];
                    let c = c.encode_utf8(&mut encoded);
                    let width = font::measure(Face::Bold, size, c).0;
                    if x + width > r.right() {
                        break;
                    }
                    p.text(Face::Bold, size, c, x, y, TEXT3);
                    x += width + 1.4 * u;
                }
            }
            Look::Swatch { color, chosen } => {
                if *chosen || pressed {
                    p.rrect(r, radius, SURFACE2);
                }
                if *chosen {
                    p.rrect_line(r, radius, ACCENT, 3.0 * u);
                }
                p.circle(
                    r.cx(),
                    r.cy(),
                    if pressed { 22.0 } else { 24.0 } * u,
                    *color,
                );
            }
            Look::Thick { width, chosen } => {
                if *chosen || pressed {
                    p.rrect(r, radius, if *chosen { ACCENT_TINT } else { SURFACE2 });
                }
                if *chosen {
                    p.rrect_line(r, radius, ACCENT, 3.0 * u);
                }
                let width = width * u;
                p.rrect(
                    Rf::new(r.cx() - 32.0 * u, r.cy() - width / 2.0, 64.0 * u, width),
                    width / 2.0,
                    TEXT1,
                );
            }
            Look::SegOpt { text, chosen } => {
                if *chosen || pressed {
                    let color = match (*chosen, pressed) {
                        (true, true) => ACCENT_PRESSED,
                        (true, false) => ACCENT,
                        _ => 0xffe3e6ee,
                    };
                    p.rrect(r, radius, color);
                }
                line_text(
                    p,
                    r.inset(4.0 * u),
                    Face::Medium,
                    22.0 * u,
                    text,
                    if *chosen { SURFACE } else { TEXT2 },
                    true,
                );
            }
            Look::Toggle { on, label, helper } => {
                if pressed {
                    p.rrect(r, radius, SURFACE2);
                }
                let text = Rf::new(r.x, r.y, (r.w - 104.0 * u).max(0.0), r.h);
                if let Some(helper) = helper {
                    line_text(
                        p,
                        Rf::new(text.x, r.cy() - 29.0 * u, text.w, 31.0 * u),
                        Face::Medium,
                        24.0 * u,
                        label,
                        TEXT1,
                        false,
                    );
                    line_text(
                        p,
                        Rf::new(text.x, r.cy() + 2.0 * u, text.w, 28.0 * u),
                        Face::Regular,
                        18.0 * u,
                        helper,
                        TEXT3,
                        false,
                    );
                } else {
                    let lines = paint::wrap(Face::Medium, 24.0 * u, label, text.w);
                    let height = 28.0 * u;
                    let top = r.cy() - height * lines.len() as f32 / 2.0;
                    for (i, text_line) in lines.iter().enumerate() {
                        line_text(
                            p,
                            Rf::new(text.x, top + height * i as f32, text.w, height),
                            Face::Medium,
                            24.0 * u,
                            text_line,
                            TEXT1,
                            false,
                        );
                    }
                }
                let track = Rf::new(r.right() - 88.0 * u, r.cy() - 26.0 * u, 88.0 * u, 52.0 * u);
                p.toggle(
                    track,
                    *on,
                    if pressed { ACCENT_PRESSED } else { ACCENT },
                    TOGGLE_OFF,
                    SURFACE,
                );
            }
            Look::Row {
                icon,
                text,
                value,
                chevron,
                danger,
            } => {
                if pressed {
                    p.rrect(r, radius, SURFACE2);
                }
                let fg = if *danger { DANGER } else { TEXT1 };
                let x = if let Some(icon) = icon {
                    p.ico(*icon, 36.0 * u, r.x + 38.0 * u, r.cy(), fg);
                    r.x + 68.0 * u
                } else {
                    r.x
                };
                let end = if *chevron {
                    p.ico(Ico::Chevron, 28.0 * u, r.right() - 16.0 * u, r.cy(), TEXT3);
                    r.right() - 36.0 * u
                } else {
                    r.right() - 12.0 * u
                };
                let max = (end - x).max(0.0);
                let value_width = font::measure(Face::Medium, 20.0 * u, value)
                    .0
                    .min(max * 0.42);
                if value_width > 0.0 {
                    line_text(
                        p,
                        Rf::new(end - value_width, r.y, value_width, r.h),
                        Face::Medium,
                        20.0 * u,
                        value,
                        if matches!(n.act, Some(Act::Updates)) && self.update.is_some() {
                            ACCENT
                        } else {
                            TEXT3
                        },
                        false,
                    );
                }
                line_text(
                    p,
                    Rf::new(x, r.y, (max - value_width - 8.0 * u).max(0.0), r.h),
                    Face::Medium,
                    24.0 * u,
                    text,
                    fg,
                    false,
                );
            }
            Look::Logo => p.logo_accent(r, ACCENT),
            Look::EmptyLogo => draw_empty_logo(p, r),
            Look::Thumb { id, gen } => {
                p.rrect(r, 20.0 * u, render::BACKGROUND);
                if let Some(t) = self.thumbs.get(id).filter(|t| {
                    t.gen == *gen && t.w == r.w.round() as u32 && t.h == r.h.round() as u32
                }) {
                    p.image(&t.rgba, t.w, t.h, r.x, r.y);
                }
                p.rrect_line(r, 20.0 * u, BORDER, 1.5 * u);
            }
            Look::Button {
                icon,
                text,
                kind,
                level,
                ..
            } => {
                let (bg, fg, down) = match kind {
                    ButtonKind::Primary => (ACCENT, SURFACE, ACCENT_PRESSED),
                    ButtonKind::Secondary => (SURFACE2, TEXT2, 0xffe3e6ee),
                    ButtonKind::White => (SURFACE, TEXT1, SURFACE2),
                    ButtonKind::AccentTint => (ACCENT_TINT, ACCENT, 0xffd4ddff),
                    ButtonKind::Danger => (DANGER, SURFACE, 0xffa71e1e),
                    ButtonKind::DangerTint => (DANGER_TINT, DANGER, 0xfff8d8d8),
                    ButtonKind::Disabled => (SURFACE2, DISABLED, SURFACE2),
                };
                draw_surface(
                    p,
                    r,
                    radius,
                    u,
                    *level,
                    if pressed { down } else { bg },
                    matches!(kind, ButtonKind::White | ButtonKind::AccentTint),
                );
                let isize = if r.h >= 84.0 * u { 40.0 } else { 32.0 } * u;
                let icon_area = if icon.is_some() && !text.is_empty() {
                    isize + 12.0 * u
                } else {
                    0.0
                };
                let size = if r.h >= 84.0 * u { 26.0 } else { 24.0 } * u;
                let width = font::measure(Face::Bold, size, text)
                    .0
                    .min((r.w - icon_area - 20.0 * u).max(0.0));
                let x = r.cx() - (width + icon_area) / 2.0;
                if let Some(icon) = icon {
                    p.ico(
                        *icon,
                        isize,
                        if text.is_empty() {
                            r.cx()
                        } else {
                            x + isize / 2.0
                        },
                        r.cy(),
                        fg,
                    );
                }
                line_text(
                    p,
                    Rf::new(x + icon_area, r.y, width, r.h),
                    Face::Bold,
                    size,
                    text,
                    fg,
                    false,
                );
            }
            Look::TitleBlock {
                title,
                status,
                failed,
            } => {
                if pressed {
                    p.rrect(r, radius, SURFACE2);
                }
                line_text(
                    p,
                    Rf::new(r.x, r.y + 10.0 * u, r.w, 36.0 * u),
                    Face::Bold,
                    26.0 * u,
                    title,
                    TEXT1,
                    false,
                );
                line_text(
                    p,
                    Rf::new(r.x, r.y + 46.0 * u, r.w, 24.0 * u),
                    Face::Regular,
                    18.0 * u,
                    status,
                    if *failed { DANGER } else { TEXT3 },
                    false,
                );
            }
            Look::Field {
                text,
                hint,
                multiline,
            } => {
                p.rrect(r, radius, SURFACE);
                p.rrect_line(r, radius, ACCENT, 3.0 * u);
                let inner = r.inset(18.0 * u);
                let size = 32.0 * u;
                let row_h = 42.0 * u;
                let x = inner.x;
                let mut caret = (
                    x,
                    if *multiline {
                        inner.y
                    } else {
                        r.cy() - row_h / 2.0
                    },
                );
                if text.is_empty() {
                    line_text(
                        p,
                        Rf::new(x + 6.0 * u, caret.1, inner.w - 6.0 * u, row_h),
                        Face::Medium,
                        size,
                        hint,
                        TEXT3,
                        false,
                    );
                } else if *multiline {
                    let lines = paint::wrap(Face::Medium, size, text, inner.w - 5.0 * u);
                    let shown = (inner.h / row_h).floor().max(1.0) as usize;
                    for (i, line) in lines[lines.len().saturating_sub(shown)..]
                        .iter()
                        .enumerate()
                    {
                        caret.1 = inner.y + i as f32 * row_h;
                        let row = Rf::new(x, caret.1, inner.w - 5.0 * u, row_h);
                        line_text(p, row, Face::Medium, size, line, TEXT1, false);
                        caret.0 = x + font::measure(Face::Medium, size, line).0.min(row.w);
                    }
                } else {
                    let mut tail = text.as_str();
                    let mut prefix = "";
                    while font::measure(Face::Medium, size, &format!("{prefix}{tail}")).0
                        > inner.w - 5.0 * u
                        && !tail.is_empty()
                    {
                        let mut chars = tail.chars();
                        chars.next();
                        tail = chars.as_str();
                        prefix = "…";
                    }
                    let row = Rf::new(x, caret.1, inner.w - 5.0 * u, row_h);
                    caret.0 = x + p.text_fit(
                        Face::Medium,
                        size,
                        &format!("{prefix}{tail}"),
                        x,
                        baseline(row, Face::Medium, size),
                        row.w,
                        TEXT1,
                    );
                }
                p.rrect(
                    Rf::new(caret.0 + 2.0 * u, caret.1, 3.0 * u, row_h),
                    1.0 * u,
                    ACCENT,
                );
            }
            Look::Qr(text) => {
                p.rrect(r, 16.0 * u, 0xffffffff);
                if let Ok(code) = qrcode::QrCode::new(text.as_bytes()) {
                    let n = code.width();
                    let quiet = 2.0;
                    let side = r.w.min(r.h);
                    let cell = side / (n as f32 + 2.0 * quiet);
                    let x0 = r.x + (r.w - side) / 2.0 + quiet * cell;
                    let y0 = r.y + (r.h - side) / 2.0 + quiet * cell;
                    for (i, c) in code.to_colors().iter().enumerate() {
                        if *c == qrcode::Color::Dark {
                            let (cx, cy) = ((i % n) as f32, (i / n) as f32);
                            p.rrect(
                                Rf::new(x0 + cx * cell, y0 + cy * cell, cell + 0.5, cell + 0.5),
                                0.0,
                                0xff000000,
                            );
                        }
                    }
                }
            }
            Look::Code(text) => {
                p.rrect(r, 20.0 * u, SURFACE2);
                let font = 22.0 * u;
                let line_height = 28.0 * u;
                for (i, line) in wrap_chars(font, text, r.w - 32.0 * u).iter().enumerate() {
                    line_text(
                        p,
                        Rf::new(
                            r.x + 16.0 * u,
                            r.y + 12.0 * u + i as f32 * line_height,
                            r.w - 32.0 * u,
                            line_height,
                        ),
                        Face::Bold,
                        font,
                        line,
                        TEXT1,
                        false,
                    );
                }
            }
            Look::Cursor { .. } | Look::Outline { .. } | Look::Avatar { .. } | Look::Badge { .. } | Look::Person { .. } => {
                team::draw_look(p, &n.look, r, u);
            }
            Look::StatusDot { text, online, chip } => {
                let bg = if *chip { ACCENT_TINT } else { SURFACE2 };
                let radius = if *chip { r.h / 2.0 } else { 20.0 * u };
                p.rrect(r, radius, if pressed { 0xffd4ddff } else { bg });
                p.circle(
                    r.x + 24.0 * u,
                    r.cy(),
                    7.0 * u,
                    if *online { OK } else { DANGER },
                );
                line_text(
                    p,
                    Rf::new(r.x + 40.0 * u, r.y, r.w - 56.0 * u, r.h),
                    Face::Medium,
                    if *chip { 22.0 } else { 20.0 } * u,
                    text,
                    if *chip { ACCENT } else { TEXT2 },
                    false,
                );
            }
            Look::FontTile { font: f, chosen } => {
                p.rrect(r, radius, if pressed { 0xffe3e6ee } else { SURFACE2 });
                if *chosen {
                    p.rrect_line(r, radius, ACCENT, 3.0 * u);
                    p.ico(Ico::Check, 24.0 * u, r.right() - 22.0 * u, r.cy(), ACCENT);
                }
                let inset = if *chosen { 44.0 } else { 16.0 } * u;
                let name = text::FONT_NAMES[*f as usize];
                let px = (26.0 * u).round() as u32;
                let mut cache = self.font_tiles.borrow_mut();
                let bitmap = cache
                    .entry((*f, px))
                    .or_insert_with(|| text::rasterize(name, px as f64, TEXT1, *f));
                let text_rect = Rf::new(r.x + 16.0 * u, r.y, r.w - inset - 16.0 * u, r.h);
                match bitmap {
                    Some(b) if b.width as f32 <= text_rect.w && b.height as f32 <= r.h => {
                        p.image(
                            &b.rgba,
                            b.width,
                            b.height,
                            text_rect.cx() - b.width as f32 / 2.0,
                            r.cy() - b.height as f32 / 2.0,
                        );
                    }
                    _ => line_text(p, text_rect, Face::Regular, 22.0 * u, name, TEXT1, true),
                }
            }
            Look::Toast {
                text,
                error,
                compact,
                action_width,
            } => {
                let radius = r.h / 2.0;
                p.elev(r, radius, u, 2);
                p.rrect(r, radius, TOAST_BG);
                let (inset, size) = if *compact { (20.0, 22.0) } else { (80.0, 24.0) };
                if !*compact {
                    let x = r.x + 48.0 * u;
                    if *error {
                        let color = 0xffffb4a9;
                        p.ico(Ico::Triangle, 32.0 * u, x, r.cy(), color);
                        p.rrect(Rf::new(x - u, r.cy() - 3.0 * u, 2.0 * u, 7.0 * u), u, color);
                        p.circle(x, r.cy() + 7.0 * u, u, color);
                    } else {
                        p.ico(Ico::Check, 32.0 * u, x, r.cy(), 0xff7fe3b0);
                    }
                }
                line_text(
                    p,
                    Rf::new(
                        r.x + inset * u,
                        r.y,
                        r.w - (inset + 20.0 + action_width) * u,
                        r.h,
                    ),
                    Face::Medium,
                    size * u,
                    text,
                    SURFACE,
                    false,
                );
            }
            Look::Rule => p.rrect(r, r.h.min(r.w) / 2.0, BORDER),
            Look::PadArea => {
                p.rrect(r, 12.0 * u, 0x0f2563eb);
                p.rrect_line(r, 12.0 * u, 0xcc2563eb, 3.0 * u);
            }
            Look::Notch(side) => {
                let vertices = match side {
                    Dock::Left => [(r.x, r.cy()), (r.right(), r.y), (r.right(), r.bottom())],
                    Dock::Right => [(r.right(), r.cy()), (r.x, r.y), (r.x, r.bottom())],
                    Dock::Bottom => [(r.cx(), r.bottom()), (r.x, r.y), (r.right(), r.y)],
                    Dock::Top => [(r.cx(), r.y), (r.x, r.bottom()), (r.right(), r.bottom())],
                };
                p.polygon(&vertices, BORDER);
                let (mut inner, mut a, mut b) = (vertices[0], vertices[1], vertices[2]);
                let step = 2.0 * u;
                match side {
                    Dock::Left | Dock::Right => {
                        inner.0 += if *side == Dock::Left { step } else { -step };
                        a.1 += step;
                        b.1 -= step;
                    }
                    Dock::Bottom | Dock::Top => {
                        inner.1 += if *side == Dock::Top { step } else { -step };
                        a.0 += step;
                        b.0 -= step;
                    }
                }
                p.polygon(&[inner, a, b], SURFACE);
            }
        }
        if n.hl == Hl::Focused {
            p.rrect_line(r, radius, ACCENT, 3.0 * u);
        }
    }
}

fn draw_empty_logo(p: &mut Painter, r: Rf) {
    p.circle(r.cx(), r.cy(), r.w / 2.0, ACCENT_TINT);
    let wave = [
        (-24.0, 14.0),
        (-17.0, 7.0),
        (-7.0, 14.0),
        (2.0, 2.0),
        (13.0, 8.0),
        (25.0, -13.0),
    ];
    let mut path = tiny_skia::PathBuilder::new();
    paint::push_polyline(&mut path, &wave);
    if let Some(path) = path.finish() {
        let mut fill = tiny_skia::Paint::default();
        fill.set_color_rgba8(0x27, 0x48, 0xe8, 0xff);
        fill.anti_alias = true;
        let stroke = tiny_skia::Stroke {
            width: 6.0,
            line_cap: tiny_skia::LineCap::Round,
            line_join: tiny_skia::LineJoin::Round,
            ..Default::default()
        };
        let scale = r.w / 160.0;
        let transform = tiny_skia::Transform::from_scale(scale, scale)
            .post_translate(r.cx() - p.ox, r.cy() - p.oy);
        p.pm.stroke_path(&path, &fill, &stroke, transform, None);
    }
}

/// Draws a raised panel. Its stacked shadow costs several blended passes
/// over the panel's area, and panels are redrawn on every full frame (each
/// pan or zoom step), so the finished panel is cached as a sprite and copied.
fn draw_surface(p: &mut Painter, r: Rf, radius: f32, u: f32, level: u8, color: u32, border: bool) {
    thread_local! {
        static SPRITES: RefCell<HashMap<[u32; 10], Arc<tiny_skia::Pixmap>>> = RefCell::new(HashMap::new());
    }
    let pad = paint::elev_pad(u, level) + 2.0;
    let (bx, by) = ((r.x - pad).floor(), (r.y - pad).floor());
    let inner = Rf::new(r.x - bx, r.y - by, r.w, r.h);
    let (sw, sh) = ((inner.x + r.w + pad).ceil() as u32, (inner.y + r.h + pad).ceil() as u32);
    let key = [
        inner.x.to_bits(),
        inner.y.to_bits(),
        r.w.to_bits(),
        r.h.to_bits(),
        radius.to_bits(),
        u.to_bits(),
        u32::from(level),
        color,
        u32::from(border),
        sw,
    ];
    let sprite = SPRITES.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some(sprite) = cache.get(&key) {
            return Some(sprite.clone());
        }
        let mut pm = tiny_skia::Pixmap::new(sw, sh)?;
        draw_surface_direct(&mut Painter { pm: pm.as_mut(), ox: 0.0, oy: 0.0 }, inner, radius, u, level, color, border);
        if cache.len() >= 64 {
            cache.clear();
        }
        let pm = Arc::new(pm);
        cache.insert(key, pm.clone());
        Some(pm)
    });
    match sprite {
        Some(sprite) => blend_sprite(&mut p.pm, &sprite, (bx - p.ox) as i32, (by - p.oy) as i32),
        None => draw_surface_direct(p, r, radius, u, level, color, border),
    }
}

/// Source-over of a premultiplied sprite onto `dst` at an integer offset.
fn blend_sprite(dst: &mut PixmapMut<'_>, src: &tiny_skia::Pixmap, x: i32, y: i32) {
    let (dw, dh) = (dst.width() as i32, dst.height() as i32);
    let (sw, sh) = (src.width() as i32, src.height() as i32);
    let (x0, y0, x1, y1) = (x.max(0), y.max(0), (x + sw).min(dw), (y + sh).min(dh));
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    let (s, d) = (src.data(), dst.data_mut());
    let n = (x1 - x0) as usize * 4;
    for row in y0..y1 {
        let so = (((row - y) * sw + (x0 - x)) * 4) as usize;
        let doff = ((row * dw + x0) * 4) as usize;
        for (o, i) in d[doff..doff + n].chunks_exact_mut(4).zip(s[so..so + n].chunks_exact(4)) {
            match i[3] {
                0 => {}
                255 => o.copy_from_slice(i),
                a => {
                    let keep = 255 - u32::from(a);
                    for c in 0..4 {
                        o[c] = (u32::from(i[c]) + (u32::from(o[c]) * keep + 127) / 255) as u8;
                    }
                }
            }
        }
    }
}

fn draw_surface_direct(p: &mut Painter, r: Rf, radius: f32, u: f32, level: u8, color: u32, border: bool) {
    p.elev(r, radius, u, level);
    p.rrect(r, radius, color);
    if border {
        p.rrect_line(r, radius, BORDER, 1.5 * u);
    }
}

fn baseline(r: Rf, face: Face, size: f32) -> f32 {
    let (_, ascent, descent) = font::measure(face, size, "Hg");
    r.cy() + (ascent - descent) / 2.0
}

fn line_text(p: &mut Painter, r: Rf, face: Face, size: f32, text: &str, color: u32, center: bool) {
    if r.w <= 0.0 {
        return;
    }
    let x = if center {
        r.cx() - font::measure(face, size, text).0.min(r.w) / 2.0
    } else {
        r.x
    };
    p.text_fit(face, size, text, x, baseline(r, face, size), r.w, color);
}


type UiRow = (Rf, Look, Option<Act>);

fn segment<'a>(
    r: Rf,
    items: impl IntoIterator<Item = (&'a str, Act)>,
    selected: usize,
) -> Vec<UiRow> {
    let items: Vec<_> = items.into_iter().collect();
    let width = (r.w - 8.0) / items.len() as f32;
    let track = Look::Surface {
        color: SURFACE2,
        radius: 24.0,
        level: 0,
        border: false,
    };
    let mut rows = vec![(r, track, None)];
    rows.extend(items.into_iter().enumerate().map(|(i, (text, act))| {
        (
            Rf::new(r.x + 4.0 + width * i as f32, r.y + 4.0, width, r.h - 8.0),
            Look::SegOpt {
                text: text.into(),
                chosen: selected == i,
            },
            Some(act),
        )
    }));
    rows
}

struct PanelRows {
    rows: Vec<UiRow>,
    width: f32,
    y: f32,
    u: f32,
}

impl PanelRows {
    fn new(width: f32, u: f32) -> Self {
        Self {
            rows: Vec::new(),
            width,
            y: 0.0,
            u,
        }
    }

    fn push(&mut self, h: f32, look: Look, act: Option<Act>) {
        self.rows
            .push((Rf::new(0.0, self.y, self.width, h), look, act));
        self.y += h;
    }

    fn para(&mut self, text: &str, color: u32) {
        let line = 34.0;
        let count = paint::wrap(Face::Regular, 24.0 * self.u, text, self.width * self.u).len();
        self.push(
            count as f32 * line,
            Look::Para {
                text: text.into(),
                face: Face::Regular,
                size: 24.0,
                line,
                color,
            },
            None,
        );
        self.y += 16.0;
    }

    fn heading(&mut self, text: &str) {
        self.push(30.0, Look::Overline(text.into()), None);
        self.y += 6.0;
    }

    fn status(&mut self, text: &str, online: bool) {
        self.push(
            72.0,
            Look::StatusDot {
                text: text.into(),
                online,
                chip: false,
            },
            None,
        );
        self.y += 16.0;
    }

    fn qr(&mut self, text: &str) {
        let side = 360.0_f32.min(self.width);
        self.rows.push((
            Rf::new((self.width - side) / 2.0, self.y, side, side),
            Look::Qr(text.into()),
            None,
        ));
        self.y += side + 16.0;
    }

    fn copy_code(&mut self, text: &str) {
        let button_width = 160.0_f32.min(self.width * 0.31);
        let width = self.width - button_width - 12.0;
        let count = wrap_chars(22.0 * self.u, text, (width - 32.0) * self.u).len();
        let height = (count as f32 * 28.0 + 24.0).max(64.0);
        self.rows.push((
            Rf::new(0.0, self.y, width, height),
            Look::Code(text.into()),
            None,
        ));
        self.rows.push((
            Rf::new(
                width + 12.0,
                self.y + (height - 64.0) / 2.0,
                button_width,
                64.0,
            ),
            Look::Button {
                icon: Some(Ico::Copy),
                text: "Copy".into(),
                kind: ButtonKind::Secondary,
                pill: false,
                level: 0,
            },
            Some(Act::Copy(text.into())),
        ));
        self.y += height + 16.0;
    }

    fn toggle(&mut self, label: &str, helper: Option<&'static str>, on: bool, act: Act) {
        self.push(
            if helper.is_some() { 96.0 } else { 80.0 },
            Look::Toggle {
                on,
                label: label.into(),
                helper,
            },
            Some(act),
        );
    }

    fn segments<'a>(
        &mut self,
        label: &str,
        width: f32,
        items: impl IntoIterator<Item = (&'a str, Act)>,
        selected: usize,
    ) {
        let label_width = font::measure(Face::Medium, 24.0 * self.u, label).0 / self.u;
        let inline = self.width >= label_width + width + 24.0;
        let label_height = if inline { 80.0 } else { 34.0 };
        let label_rect = Rf::new(
            0.0,
            self.y,
            if inline {
                self.width - width - 16.0
            } else {
                self.width
            },
            label_height,
        );
        self.rows.push((
            label_rect,
            Look::Label {
                text: label.into(),
                face: Face::Medium,
                size: 24.0,
                color: TEXT1,
                center: false,
            },
            None,
        ));
        let track = if inline {
            Rf::new(self.width - width, self.y + 8.0, width, 64.0)
        } else {
            Rf::new(0.0, self.y + label_height, self.width, 64.0)
        };
        self.rows.extend(segment(track, items, selected));
        self.y += if inline { 80.0 } else { label_height + 72.0 };
    }
}

/// Splits `text` into lines no wider than `width`, breaking anywhere (for
/// addresses and commands, which have few spaces).
fn wrap_chars(size: f32, text: &str, width: f32) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for c in text.chars() {
        line.push(c);
        if line.chars().count() > 1 && font::measure(Face::Bold, size, &line).0 > width {
            line.pop();
            lines.push(std::mem::take(&mut line));
            line.push(c);
        }
    }
    lines.push(line);
    lines
}

/// The whole board as a PNG no larger than `max_side` on either side.
fn board_png(shapes: &[Arc<Shape>], max_side: u32) -> Option<Vec<u8>> {
    let b = crate::document::scene_bounds(shapes)?;
    let (bw, bh) = (b.width() + 32.0, b.height() + 32.0);
    let zoom = (max_side as f64 / bw.max(bh)).min(1.5);
    let w = ((bw * zoom).ceil() as u32).clamp(16, max_side);
    let h = ((bh * zoom).ceil() as u32).clamp(16, max_side);
    let rgba = render::thumbnail(shapes, w, h);
    let size = tiny_skia::IntSize::from_wh(w, h)?;
    tiny_skia::Pixmap::from_vec(rgba, size)?.encode_png().ok()
}

impl net::Board for App {
    fn info(&self) -> Option<(String, usize, u64)> {
        let b = self.board.as_ref()?;
        Some((b.title.clone(), self.ed.doc.shapes().len(), self.doc_gen))
    }

    fn shapes(&self) -> Vec<Value> {
        if self.board.is_none() {
            return Vec::new();
        }
        self.ed.doc.shapes().iter().map(|s| s.to_json()).collect()
    }

    fn has(&self, id: &str) -> bool {
        self.ed.doc.contains(id)
    }

    fn apply(&mut self, put: Vec<Shape>, remove: Vec<String>) -> u64 {
        if self.board.is_some() && (!put.is_empty() || !remove.is_empty()) {
            if put.is_empty() {
                self.ed.delete_shapes(remove);
            } else {
                self.ed.apply_shapes(put, remove);
            }
            self.take_editor_events();
        }
        self.doc_gen
    }

    fn png(&mut self, max_side: u32) -> Option<Vec<u8>> {
        self.board.as_ref()?;
        board_png(self.ed.doc.shapes(), max_side)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Quiet;
    impl Platform for Quiet {}

    fn app() -> (App, PathBuf) {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "canvas-ui-{}-{}-{}",
            std::process::id(),
            wall_millis(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let app = App::new(
            Box::new(Quiet),
            dir.join("boards"),
            dir.join("settings.json"),
            "0.5.0".into(),
            330,
            Arc::new(|| {}),
        );
        (app, dir)
    }

    fn settle(app: &mut App) {
        for i in 0..50 {
            app.frame(1000 + i);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    fn tap(app: &mut App, x: f32, y: f32, t: u64) {
        let s = Sample {
            id: 0,
            x,
            y,
            time: t,
            size: 0.0,
            stylus: false,
            eraser: false,
            norm: 0.0,
        };
        app.touch(Phase::Down, &[s]);
        app.touch(Phase::Up, &[s]);
        app.frame(t);
    }

    #[test]
    fn versions_compare() {
        assert!(newer("0.5.0", "0.4.9"));
        assert!(newer("1.0", "0.9.9"));
        assert!(!newer("0.5.0", "0.5.0"));
        assert!(!newer("0.4.0", "0.5"));
    }

    #[test]
    fn pause_saves_a_stroke_without_an_intervening_frame() {
        let (mut app, dir) = app();
        app.resize(1280, 800, 1.0);
        app.create_local_board("Before background".into());
        settle(&mut app);
        let mut sample = Sample {
            id: 9,
            x: 600.0,
            y: 400.0,
            time: 5000,
            stylus: true,
            eraser: false,
            ..Sample::default()
        };
        app.touch(Phase::Down, &[sample]);
        sample.x += 100.0;
        sample.time += 100;
        app.touch(Phase::Move, &[sample]);
        let (tx, rx) = mpsc::sync_channel(1);
        app.pause(tx);
        rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        let board = store::load(&dir.join("boards"), &app.board.as_ref().unwrap().id).unwrap();
        assert_eq!(board.shapes.len(), 1);
        assert!(!app.ed.pointer_down_now());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn failed_save_keeps_the_board_open_until_retry_succeeds() {
        let (mut app, dir) = app();
        app.resize(1280, 800, 1.0);
        app.create_local_board("Keep my work".into());
        settle(&mut app);
        let sample = Sample {
            id: 1,
            x: 600.0,
            y: 400.0,
            time: 5000,
            stylus: true,
            eraser: false,
            ..Sample::default()
        };
        app.touch(Phase::Down, &[sample]);
        app.touch(
            Phase::Up,
            &[Sample {
                x: 700.0,
                time: 5010,
                ..sample
            }],
        );
        let path = dir.join("boards");
        let backup = dir.join("boards-backup");
        std::fs::rename(&path, &backup).unwrap();
        std::fs::write(&path, b"cannot create a board under a regular file").unwrap();
        app.run(Act::Back);
        settle(&mut app);
        assert!(app.is_board_open());
        assert!(matches!(app.save_state, SaveState::Failed));
        assert_eq!(app.ed.doc.shapes().len(), 1);
        std::fs::remove_file(&path).unwrap();
        std::fs::rename(&backup, &path).unwrap();
        app.run(Act::Back);
        settle(&mut app);
        assert!(!app.is_board_open());
        let _ = std::fs::remove_dir_all(dir);
    }

    fn req(
        app: &mut App,
        method: &str,
        path: &str,
        query: &[(&str, &str)],
        body: Value,
    ) -> (u16, Value) {
        let (reply, _rx) = mpsc::sync_channel(1);
        let r = net::Req {
            method: method.into(),
            path: path.into(),
            query: query
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            body: if body.is_null() {
                Vec::new()
            } else {
                body.to_string().into_bytes()
            },
            reply,
        };
        let resp = app.answer(&r);
        (
            resp.status,
            serde_json::from_slice(&resp.body).unwrap_or(Value::Null),
        )
    }

    #[test]
    fn a_phone_pad_writes_into_its_box_moves_it_and_pans_the_board() {
        let (mut app, dir) = app();
        app.resize(1280, 800, 1.5);
        settle(&mut app);
        let key = app.settings.remote_key.clone();
        assert_eq!(key.len(), 16);
        assert_eq!(req(&mut app, "GET", "/r", &[], Value::Null).0, 200);
        assert_eq!(req(&mut app, "GET", "/r/state", &[("k", "nope")], Value::Null).0, 403);
        let (status, state) = req(&mut app, "GET", "/r/state", &[("k", &key)], Value::Null);
        assert_eq!(status, 200);
        assert_eq!(state["board"], false);

        app.create_local_board("Pad".into());
        settle(&mut app);
        // A square pad on a 1280x800 screen: the box is square on screen too.
        let (_, state) = req(&mut app, "POST", "/r/area", &[("k", &key)], json!({"x": 0.1, "y": 0.2, "w": 0.25, "aspect": 1.0}));
        let area: Vec<f64> = state["area"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
        assert!((area[0] - 0.1).abs() < 1e-4 && (area[1] - 0.2).abs() < 1e-4);
        assert!((area[2] * 1280.0 - area[3] * 800.0).abs() < 0.5, "{area:?}");
        settle(&mut app);
        assert!(app.nodes.iter().any(|n| n.look == Look::PadArea));

        let mut events = vec![json!([0, 0, 0.1, 0.5, 0])];
        for i in 1..=20 {
            events.push(json!([1, 0, 0.1 + i as f64 * 0.04, 0.5 + (i % 3) as f64 * 0.01, i * 8]));
        }
        events.push(json!([2, 0, 0.9, 0.5, 170]));
        let (status, _) = req(&mut app, "POST", "/r/input", &[("k", &key)], json!({"events": events}));
        assert_eq!(status, 200);
        settle(&mut app);
        assert_eq!(app.ed.doc.shapes().len(), 1);
        // The stroke sits inside the box on screen.
        let b = app.ed.doc.shapes()[0].bounds();
        let left = app.ed.camera().x + b.l * app.ed.zoom();
        let right = app.ed.camera().x + b.r * app.ed.zoom();
        let (x0, x1) = (area[0] * 1280.0 / 1.5, (area[0] + area[2]) * 1280.0 / 1.5);
        assert!(left >= x0 - 2.0 && right <= x1 + 2.0, "{left}..{right} not in {x0}..{x1}");

        let cam = app.ed.camera();
        req(&mut app, "POST", "/r/pan", &[("k", &key)], json!({"dx": 0.1, "dy": 0.0}));
        assert!((app.ed.camera().x - cam.x - 1280.0 * 0.1 / 1.5).abs() < 0.01);
        req(&mut app, "POST", "/r/cmd", &[("k", &key)], json!({"cmd": "undo"}));
        settle(&mut app);
        assert_eq!(app.ed.doc.shapes().len(), 0);

        // Tools, colour and size come from the phone too.
        let (_, state) = req(&mut app, "POST", "/r/cmd", &[("k", &key)], json!({"tool": "highlighter", "color": PALETTE[2], "width": 1}));
        assert_eq!(state["tool"], "highlighter");
        assert_eq!(state["color"], PALETTE[2]);
        assert_eq!(state["width"], 1);
        let (_, state) = req(&mut app, "POST", "/r/cmd", &[("k", &key)], json!({"tool": "shapes"}));
        assert_eq!(state["tool"], "shapes");
        // The phone sees the board under its box.
        let (status, view) = req(&mut app, "GET", "/r/view", &[("k", &key), ("w", "400")], Value::Null);
        assert_eq!(status, 200);
        assert_eq!(view["w"], 400);
        assert_eq!(view["h"], 400);
        assert_eq!(&net::unbase64(view["jpg"].as_str().unwrap())[..2], &[0xff, 0xd8]);

        // Closing the page hides the box; it also goes on its own.
        assert!(app.pad_shown());
        assert_eq!(req(&mut app, "POST", "/r/bye", &[("k", &key)], Value::Null).0, 204);
        assert!(!app.pad_shown());
        req(&mut app, "GET", "/r/state", &[("k", &key)], Value::Null);
        assert!(app.pad_shown());
        app.now += PAD_TTL + 1;
        settle(&mut app);
        assert!(!app.pad_shown() && app.pad_seen.is_none());
        // Disconnecting from the TV retires the link.
        app.run(Act::PadDisconnect);
        assert_ne!(app.settings.remote_key, key);
        assert_eq!(req(&mut app, "GET", "/r/state", &[("k", &key)], Value::Null).0, 403);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn claude_reaches_the_open_board_and_sharing_moves_to_team_boards() {
        let (mut app, dir) = app();
        app.resize(1280, 800, 1.5);
        settle(&mut app);
        app.create_local_board("Team".into());
        settle(&mut app);
        assert!(app.is_board_open());
        let key = app.settings.mcp_key.clone();
        assert_eq!(key.len(), 24);
        // A wrong key is not found; the right one is an MCP server.
        assert_eq!(req(&mut app, "POST", "/mcp/nope", &[], json!({})).0, 404);
        let mcp = format!("/mcp/{key}");
        let (status, init) = req(
            &mut app,
            "POST",
            &mcp,
            &[],
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26"}}),
        );
        assert_eq!(status, 200);
        assert_eq!(init["result"]["serverInfo"]["name"], "inkboard");
        let add = json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"add_shapes","arguments":{"shapes":[
            {"id":"c1","tool":"rectangle","points":[[0,0],[200,120]]},
            {"id":"c2","tool":"arrow","points":[[220,60],[400,60]]}]}}});
        let (_, added) = req(&mut app, "POST", &mcp, &[], add);
        assert!(added["result"].get("isError").is_none(), "{added}");
        assert_eq!(app.ed.doc.shapes().len(), 2);
        assert!(app.unsaved);
        let (_, image) = req(
            &mut app,
            "POST",
            &mcp,
            &[],
            json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"get_board_image","arguments":{}}}),
        );
        assert_eq!(image["result"]["content"][0]["type"], "image");
        let png = net::unbase64(image["result"]["content"][0]["data"].as_str().unwrap());
        assert_eq!(&png[1..4], b"PNG");

        // The old local-network sharing endpoints are gone.
        assert_eq!(req(&mut app, "GET", "/canvas/hello", &[("code", "123456")], Value::Null).0, 404);
        // Signed out, the share panel asks to sign in instead of copying the
        // board anywhere, and the library shows no boards.
        app.run(Act::Share);
        assert!(matches!(app.overlay, Some(Overlay::Panel { kind: PanelKind::Share, .. })));
        let (rows, _) = app.panel_rows(PanelKind::Share, 700.0);
        assert!(rows.iter().any(|(_, _, act)| matches!(act, Some(Act::SignIn))));
        app.dismiss();
        assert!(app.cards().is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn panels_lay_out_and_choose_fonts() {
        let (mut app, dir) = app();
        app.resize(720, 1280, 2.0);
        settle(&mut app);
        app.run(Act::Claude);
        app.frame(3000);
        assert!(app
            .nodes
            .iter()
            .any(|n| matches!(&n.act, Some(Act::Copy(t)) if t.contains("/mcp/"))));
        app.run(Act::Cancel);
        app.panel(PanelKind::Fonts);
        app.frame(3100);
        let tiles = app
            .nodes
            .iter()
            .filter(|n| matches!(n.look, Look::FontTile { .. }))
            .count();
        assert!(tiles > 4);
        assert!(app.panel_max_scroll() >= 0.0);
        app.run(Act::Font(3));
        assert_eq!(app.settings.font, 3);
        assert!(app.overlay.is_none());
        // The settings keep the generated keys.
        let saved: Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("settings.json")).unwrap())
                .unwrap();
        assert_eq!(saved["mcpKey"], app.settings.mcp_key.as_str());
        assert_eq!(wrap_chars(14.0, "abcdefghij", 30.0).concat(), "abcdefghij");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn dates_are_civil() {
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(20_773), (2026, 11, 16));
    }

    #[test]
    fn create_open_draw_and_return() {
        let (mut app, dir) = app();
        app.resize(1280, 800, 1.5);
        settle(&mut app);
        assert!(app.listed);
        // New board, named through the dialog.
        app.create_local_board("Plans".into());
        settle(&mut app);
        assert!(app.is_board_open());
        assert_eq!(app.board.as_ref().unwrap().title, "Plans");
        // A stroke in the middle of the board reaches the editor.
        let (cx, cy) = (640.0, 400.0);
        let mut s = Sample {
            id: 1,
            x: cx,
            y: cy,
            time: 2000,
            size: 0.0,
            stylus: true,
            eraser: false,
            norm: 0.0,
        };
        app.touch(Phase::Down, &[s]);
        for i in 0..20 {
            s.x += 6.0;
            s.y += (i % 3) as f32;
            s.time += 8;
            app.touch(Phase::Move, &[s]);
            assert!(app.frame(s.time).is_some() || i == 0);
        }
        app.touch(Phase::Up, &[s]);
        app.frame(s.time);
        assert_eq!(app.ed.doc.shapes().len(), 1);
        // Back saves and returns to the library, which lists the mark.
        let (tx, rx) = mpsc::sync_channel(1);
        app.pause(tx);
        rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        assert!(app.back());
        settle(&mut app);
        assert!(!app.is_board_open());
        assert_eq!(app.boards.len(), 1);
        assert_eq!(app.boards[0].shapes, 1);
        assert!(!app.back());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn menu_toggles_and_remote_focus() {
        let (mut app, dir) = app();
        app.resize(1920, 1080, 2.0);
        app.run(Act::NewBoard);
        settle(&mut app);
        assert!(app.is_board_open());
        // No name asked for: it starts as "Untitled board" and is renamed from its title.
        assert_eq!(app.board.as_ref().unwrap().title, "Untitled board");
        let menu = app.node_rect(&Act::Menu);
        tap(&mut app, menu.cx(), menu.cy(), 3000);
        assert!(matches!(app.overlay, Some(Overlay::Menu { .. })));
        let settings = app.node_rect(&Act::Settings);
        tap(&mut app, settings.cx(), settings.cy(), 3050);
        assert!(matches!(
            app.overlay,
            Some(Overlay::Panel {
                kind: PanelKind::Settings,
                ..
            })
        ));
        let grid = app.node_rect(&Act::Toggle(Opt::Grid));
        tap(&mut app, grid.cx(), grid.cy(), 3100);
        assert!(matches!(
            app.overlay,
            Some(Overlay::Panel {
                kind: PanelKind::Settings,
                ..
            })
        ));
        assert!(!app.settings.grid);
        assert!(!Settings::load(&app.settings_path).grid);
        // The remote moves focus between the controls.
        assert!(app.key(keys::RIGHT));
        app.frame(3200);
        assert!(app.focus.is_some());
        assert!(app.key(keys::RIGHT));
        app.frame(3300);
        assert!(app.nodes.iter().any(|n| n.hl == Hl::Focused));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn dock_moves_to_each_edge_without_covering_canvas_centre() {
        let (mut app, dir) = app();
        app.resize(1920, 1080, 1.5);
        app.create_local_board("Dock positions".into());
        settle(&mut app);
        let (w, h) = app.extent();
        for dock in Dock::ALL {
            app.run(Act::Dock(dock));
            let chrome = app.board_chrome();
            let r = chrome.dock;
            assert!(!r.contains(w / 2.0, h / 2.0));
            assert!(chrome
                .buttons
                .iter()
                .all(|button| r.contains(button.cx(), button.cy())));
            match dock {
                Dock::Left => assert_eq!(r.x, 24.0),
                Dock::Right => assert_eq!(r.right(), w - 24.0),
                Dock::Bottom => assert_eq!(r.bottom(), h - 24.0),
                Dock::Top => assert_eq!(r.y, 24.0),
            }
            let (p, _) = app.popover(r, 536.0, 352.0, chrome.buttons[7], 20.0, false);
            match dock {
                Dock::Left => assert!(p.x > r.right()),
                Dock::Right => assert!(p.right() < r.x),
                Dock::Bottom => assert!(p.bottom() < r.y),
                Dock::Top => assert!(p.y > r.bottom()),
            }
            assert_eq!(Settings::load(&app.settings_path).dock, dock);
        }
        // Both sets of corner bars move out of the way of a crowded horizontal dock.
        app.resize(720, 1280, 2.0);
        for dock in [Dock::Top, Dock::Bottom] {
            app.run(Act::Dock(dock));
            let chrome = app.board_chrome();
            for bar in [
                chrome.left,
                chrome.right,
                chrome.history,
                chrome.zoom,
                chrome.pages,
            ] {
                assert!(!overlaps(chrome.dock, bar), "{dock:?}, {bar:?}");
            }
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn pages_save_and_reopen_with_independent_shapes_and_views() {
        let (mut app, dir) = app();
        app.resize(1280, 800, 1.0);
        app.create_local_board("Pages".into());
        settle(&mut app);
        let board_id = app.board.as_ref().unwrap().id.clone();
        let shape = |id| Shape::new(id, Tool::Rectangle, vec![Pt::ZERO, Pt::new(100.0, 60.0)]);
        app.ed.apply_remote(vec![shape("first")], Vec::new());
        app.ed.set_view(Pt::new(10.0, -20.0), 1.25);
        app.run(Act::PageAdd);
        assert_eq!(app.page, 1);
        assert!(app.ed.doc.shapes().is_empty());
        app.ed.apply_remote(vec![shape("second")], Vec::new());
        app.ed.set_view(Pt::new(-90.0, 91.0), 2.0);
        app.run(Act::PagePrev);
        assert_eq!(app.ed.doc.shapes()[0].id, "first");
        assert_eq!(app.ed.camera(), Pt::new(10.0, -20.0));
        app.run(Act::PageNext);
        assert_eq!(app.ed.doc.shapes()[0].id, "second");
        assert_eq!(app.ed.zoom(), 2.0);
        let (tx, rx) = mpsc::sync_channel(1);
        app.pause(tx);
        rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        let saved = store::load(&dir.join("boards"), &board_id).unwrap();
        assert_eq!(saved.shapes[0].id, "first");
        let second = Page::decode(&saved.extra["pages"][0]).unwrap();
        assert_eq!(second.shapes[0].id, "second");
        assert_eq!(second.view, Some((Pt::new(-90.0, 91.0), 2.0)));
        app.run(Act::Back);
        settle(&mut app);
        app.run(Act::Open(board_id));
        settle(&mut app);
        assert_eq!(app.page, 0);
        assert_eq!(app.ed.doc.shapes()[0].id, "first");
        assert_eq!(app.ed.camera(), Pt::new(10.0, -20.0));
        app.run(Act::PageNext);
        assert_eq!(app.ed.doc.shapes()[0].id, "second");
        assert_eq!(app.ed.camera(), Pt::new(-90.0, 91.0));
        app.run(Act::AskDeletePage);
        app.run(Act::Done);
        assert_eq!(app.pages.len(), 1);
        assert_eq!(app.ed.doc.shapes()[0].id, "first");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn highlighter_draws_translucent_ink_and_keeps_its_own_style() {
        let (mut app, dir) = app();
        app.resize(1280, 800, 1.0);
        app.create_local_board("Highlights".into());
        settle(&mut app);
        app.run(Act::Color(PALETTE[8]));
        app.run(Act::Width(2));
        app.run(Act::Highlighter);
        app.run(Act::Highlighter);
        assert!(matches!(app.overlay, Some(Overlay::Style)));
        app.run(Act::Color(PALETTE[2]));
        app.run(Act::Width(3));
        assert_eq!(app.ed.color(), 0x73e0_3131);
        assert_eq!(app.ed.stroke_width(), 32.0);
        assert!(app.layout().iter().any(
            |node| matches!(node.look, Look::Swatch { color, chosen: true } if color == PALETTE[2])
        ));
        app.run(Act::Cancel);
        app.frame(2000);
        let sample = Sample {
            id: 1,
            x: 600.0,
            y: 400.0,
            time: 2010,
            stylus: true,
            ..Sample::default()
        };
        app.touch(Phase::Down, &[sample]);
        app.touch(
            Phase::Move,
            &[Sample {
                x: 650.0,
                time: 2050,
                ..sample
            }],
        );
        app.touch(
            Phase::Up,
            &[Sample {
                x: 700.0,
                time: 2100,
                ..sample
            }],
        );
        assert_eq!(app.ed.doc.shapes()[0].color, 0x73e0_3131);
        assert_eq!(app.ed.doc.shapes()[0].tool, Tool::Pen);
        app.run(Act::Tool(Tool::Pen, PenMode::Ink));
        assert_eq!(app.ed.color(), PALETTE[8]);
        assert_eq!(app.ed.stroke_width(), 5.0);
        app.run(Act::Highlighter);
        assert_eq!(app.ed.color(), 0x73e0_3131);
        assert_eq!(app.ed.stroke_width(), 32.0);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn text_request_opens_multiline_input_and_commits_opaque_text() {
        let (mut app, dir) = app();
        app.resize(1280, 800, 1.0);
        app.create_local_board("Typing".into());
        settle(&mut app);
        app.run(Act::Color(PALETTE[9]));
        app.run(Act::Highlighter);
        app.run(Act::Tool(Tool::Text, PenMode::Ink));
        app.frame(2000);
        tap(&mut app, 640.0, 400.0, 2100);
        assert!(matches!(
            app.overlay,
            Some(Overlay::Ask {
                multiline: true,
                purpose: AskFor::Text { .. },
                ..
            })
        ));
        app.text_input("First line\nSecond line", false);
        app.run(Act::Done);
        assert!(app.overlay.is_none());
        let shape = &app.ed.doc.shapes()[0];
        assert_eq!(shape.tool, Tool::Text);
        assert_eq!(shape.color, PALETTE[9]);
        assert_eq!(shape.text, "First line\nSecond line");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn math_inserts_edits_plots_and_changes_range_through_board_controls() {
        let (mut app, dir) = app();
        app.resize(1280, 800, 1.0);
        app.create_local_board("Math test".into());
        settle(&mut app);
        let tap_button = |app: &mut App, act: Act, time| {
            let button = app.node_rect(&act);
            assert!(button.w > 0.0, "missing button {act:?}");
            tap(app, button.cx(), button.cy(), time);
        };
        tap_button(&mut app, Act::Insert, 1200);
        tap_button(&mut app, Act::InsertItem(InsertKind::Formula), 1300);
        app.text_input("x^2 +", false);
        tap_button(&mut app, Act::Done, 1400);
        assert!(matches!(app.overlay, Some(Overlay::Ask { purpose: AskFor::Math { tool: Tool::Formula, .. }, .. })));
        assert!(app.ed.doc.shapes().is_empty());
        app.text_input("y = x^2", false);
        tap_button(&mut app, Act::Done, 1500);
        let formula = app.ed.selected_math().unwrap().clone();
        assert_eq!(formula.tool, Tool::Formula);
        let point = formula.bounds().center();
        tap(&mut app, point.x as f32, point.y as f32, 1600);
        tap(&mut app, point.x as f32, point.y as f32, 1700);
        assert!(matches!(
            app.overlay,
            Some(Overlay::Ask { ref text, purpose: AskFor::Math { tool: Tool::Formula, .. }, .. }) if text == "y = x^2"
        ));
        app.text_input("y = x^2 - 2", false);
        tap_button(&mut app, Act::Done, 1800);
        tap_button(&mut app, Act::Plot, 1900);
        let graph = app.ed.selected_math().unwrap().clone();
        assert_eq!(graph.text, "y = x^2 - 2");
        tap_button(&mut app, Act::GraphRange, 2000);
        tap_button(&mut app, Act::GraphAdjust { pan: Pt::ZERO, scale: Some(0.8) }, 2100);
        assert_eq!(app.ed.doc.get(&graph.id).unwrap().graph_range.x(), [-8.0, 8.0]);
        tap_button(&mut app, Act::GraphRange, 2200);
        tap_button(&mut app, Act::EditMath, 2300);
        app.text_input("y = sin(x)\ny = cos(x)", false);
        tap_button(&mut app, Act::Done, 2400);
        let edited = app.ed.doc.get(&graph.id).unwrap();
        assert_eq!(edited.text, "y = sin(x)\ny = cos(x)");
        assert_eq!(edited.graph_range.x(), [-8.0, 8.0]);
        tap_button(&mut app, Act::Insert, 2500);
        tap_button(&mut app, Act::InsertItem(InsertKind::Graph), 2600);
        app.text_input("1/x; cos(x)", false);
        tap_button(&mut app, Act::Done, 2700);
        assert_eq!(app.ed.selected_math().unwrap().text, "1/x; cos(x)");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn insert_dock_imports_and_persists_rendered_pdf_pages() {
        let (mut app, dir) = app();
        app.resize(1280, 800, 1.0);
        app.create_local_board("PDF annotations".into());
        settle(&mut app);
        let id = app.board.as_ref().unwrap().id.clone();
        let insert = app.node_rect(&Act::Insert);
        tap(&mut app, insert.cx(), insert.cy(), 1100);
        let pdf = app.node_rect(&Act::InsertItem(InsertKind::File(FileKind::Pdf)));
        assert!(pdf.w > 0.0);
        tap(&mut app, pdf.cx(), pdf.cy(), 1200);
        let new_pages = app.node_rect(&Act::ImportPdf(false));
        assert!(new_pages.w > 0.0);
        tap(&mut app, new_pages.cx(), new_pages.cy(), 1300);
        let mut pixmap = tiny_skia::Pixmap::new(8, 4).unwrap();
        pixmap.fill(tiny_skia::Color::from_rgba8(255, 0, 0, 255));
        let png = pixmap.encode_png().unwrap();
        let token = app.file_token;
        app.file_result(token, crate::import::PickerFile::Pdf(vec![png.clone(), png]));
        settle(&mut app);
        assert_eq!((app.pages.len(), app.page), (3, 1));
        assert!(app.ed.doc.shapes()[0].page_background);
        let background_id = app.ed.doc.shapes()[0].id.clone();
        assert!(!app.ed.doc.select(Some(&background_id)));

        let (tx, rx) = mpsc::sync_channel(1);
        app.pause(tx);
        rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        let saved = store::load(&dir.join("boards"), &id).unwrap();
        assert_eq!(saved.extra["pages"].as_array().unwrap().len(), 2);
        let exported = board_png(app.ed.doc.shapes(), 80).unwrap();
        let exported = tiny_skia::Pixmap::decode_png(&exported).unwrap();
        let pixel = exported.pixel(exported.width() / 2, exported.height() / 2).unwrap();
        assert_eq!((pixel.red(), pixel.green(), pixel.blue()), (255, 0, 0));

        let shared = Shape::from_json(&app.ed.doc.shapes()[0].to_json()).unwrap();
        assert_eq!(&shared, app.ed.doc.shapes()[0].as_ref());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn table_picker_and_cell_editor_use_the_board_controls() {
        let (mut app, dir) = app();
        app.resize(1280, 800, 1.0);
        app.create_local_board("Table test".into());
        settle(&mut app);
        let tap_button = |app: &mut App, act: Act, time| {
            let button = app.node_rect(&act);
            assert!(button.w > 0.0, "missing button {act:?}");
            tap(app, button.cx(), button.cy(), time);
        };
        tap_button(&mut app, Act::Insert, 1000);
        tap_button(&mut app, Act::InsertItem(InsertKind::Table), 1100);
        assert!(matches!(app.overlay, Some(Overlay::TableSize { rows: 3, columns: 3 })));
        tap_button(&mut app, Act::TableDimensions(-1, 0), 1200);
        tap_button(&mut app, Act::TableDimensions(0, 1), 1300);
        tap_button(&mut app, Act::Done, 1400);
        let table = app.ed.doc.shapes()[0].clone();
        let grid = table.table.as_deref().unwrap();
        assert_eq!((grid.rows(), grid.columns()), (2, 4));
        assert_eq!(app.ed.tool(), Tool::Select);
        assert!(app.ed.doc.is_selected(&table.id));

        let cell = Cell { row: 0, column: 1 };
        let point = grid.cell_bounds(table.bounds(), cell).center();
        tap(&mut app, point.x as f32, point.y as f32, 1500);
        tap(&mut app, point.x as f32, point.y as f32, 1600);
        assert!(matches!(app.overlay, Some(Overlay::Ask { purpose: AskFor::Cell { cell: hit, .. }, .. }) if hit == cell));
        app.text_input("Monday\n09:00", false);
        tap_button(&mut app, Act::Done, 1700);
        assert_eq!(
            app.ed.doc.get(&table.id).unwrap().table.as_deref().unwrap().text(cell),
            Some("Monday\n09:00")
        );
        tap_button(&mut app, Act::TableDimensions(1, 0), 1800);
        tap_button(&mut app, Act::TableDimensions(0, -1), 1900);
        let shape = app.ed.doc.get(&table.id).unwrap();
        let adjusted = shape.table.as_deref().unwrap();
        assert_eq!((adjusted.rows(), adjusted.columns()), (3, 3));
        assert_eq!(adjusted.text(cell), Some("Monday\n09:00"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn library_scrolls_and_deletes() {
        let (mut app, dir) = app();
        app.resize(720, 1280, 2.0);
        for i in 0..9 {
            let id = format!("b{i}");
            let contents = store::encode(
                &id,
                &format!("Board {i}"),
                &format!("2026-11-16T10:00:0{i}.000"),
                &[],
                &Map::new(),
            );
            store::save_atomic(&dir.join("boards"), &id, &contents).unwrap();
        }
        app.send(Job::List);
        settle(&mut app);
        assert_eq!(app.boards.len(), 9);
        // The library lists the signed-in person's team boards.
        app.settings.account = Some(Account {
            server: "http://127.0.0.1:1".into(),
            token: "t".into(),
            user: cloud::User { id: "u".into(), email: "a@kenpath.com".into(), name: "A B".into(), color: 0xff2748e8 },
        });
        app.team_boards = (0..9)
            .map(|i| CloudBoard {
                id: format!("t{i}"),
                title: format!("Team board {i}"),
                role: Role::Owner,
                org_access: "none".into(),
                owner_name: "A B".into(),
                owner_email: "a@kenpath.com".into(),
                updated_at: 1_000 + i,
                members: 0,
            })
            .collect();
        app.team_listed = true;
        let s = Sample {
            id: 0,
            x: 300.0,
            y: 1000.0,
            time: 5000,
            size: 0.0,
            stylus: false,
            eraser: false,
            norm: 0.0,
        };
        app.touch(Phase::Down, &[s]);
        app.touch(
            Phase::Move,
            &[Sample {
                y: 600.0,
                time: 5050,
                ..s
            }],
        );
        app.touch(
            Phase::Up,
            &[Sample {
                y: 600.0,
                time: 5060,
                ..s
            }],
        );
        assert!(app.scroll > 100.0);
        let id = app.boards[0].id.clone();
        app.run(Act::AskDelete(id));
        app.run(Act::Done);
        settle(&mut app);
        assert_eq!(app.boards.len(), 8);
        let _ = std::fs::remove_dir_all(dir);
    }
}
