//! Experimental presentation paths that avoid locking the window buffer.
//!
//! `ANativeWindow_lock` maps a whole window buffer for CPU reading and
//! writing on every frame. On large panels that costs far more than drawing
//! the few pixels a pen stroke changes. These paths keep our own hardware
//! buffers on a child `SurfaceControl` layer (API 29+) and only write the
//! changed rectangle into them.
//!
//! Selected with the system property `debug.inkboard.present`:
//! * `0` or unset: the classic `ANativeWindow_lock` path.
//! * `1`: one persistent buffer that stays on screen ("front buffer").
//! * `2`: two buffers used in turn, written only where they are out of date.

use super::{log, ANativeWindow, ARect};
use canvas_core::geom::IRect;
use canvas_core::render::Layer;
use std::ffi::{c_char, c_int, c_void, CString};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

#[repr(C)]
#[derive(Clone, Copy)]
struct Desc {
    width: u32,
    height: u32,
    layers: u32,
    format: u32,
    usage: u64,
    stride: u32,
    rfu0: u32,
    rfu1: u64,
}

const CPU_WRITE_OFTEN: u64 = 3 << 4;
const GPU_SAMPLED: u64 = 1 << 8;
const COMPOSER_OVERLAY: u64 = 1 << 11;
const FRONT_BUFFER: u64 = 1 << 32;

#[allow(clippy::type_complexity)]
struct Api {
    sc_from_window: unsafe extern "C" fn(*mut ANativeWindow, *const c_char) -> *mut c_void,
    sc_release: unsafe extern "C" fn(*mut c_void),
    tx_create: unsafe extern "C" fn() -> *mut c_void,
    tx_delete: unsafe extern "C" fn(*mut c_void),
    tx_apply: unsafe extern "C" fn(*mut c_void),
    tx_buffer: unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void, c_int),
    tx_damage: unsafe extern "C" fn(*mut c_void, *mut c_void, *const ARect, u32),
    tx_visible: unsafe extern "C" fn(*mut c_void, *mut c_void, i8),
    tx_opaque: unsafe extern "C" fn(*mut c_void, *mut c_void, i8),
    hb_alloc: unsafe extern "C" fn(*const Desc, *mut *mut c_void) -> c_int,
    hb_release: unsafe extern "C" fn(*mut c_void),
    hb_describe: unsafe extern "C" fn(*mut c_void, *mut Desc),
    hb_lock: unsafe extern "C" fn(*mut c_void, u64, i32, *const ARect, *mut *mut c_void) -> c_int,
    hb_unlock: unsafe extern "C" fn(*mut c_void, *mut i32) -> c_int,
    hb_supported: Option<unsafe extern "C" fn(*const Desc) -> c_int>,
    tx_on_complete: unsafe extern "C" fn(*mut c_void, *mut c_void, unsafe extern "C" fn(*mut c_void, *mut c_void)),
    stats_present_fence: unsafe extern "C" fn(*mut c_void) -> c_int,
}
unsafe impl Send for Api {}
unsafe impl Sync for Api {}

extern "C" {
    fn dlopen(name: *const c_char, flags: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void;
    fn __system_property_get(name: *const c_char, value: *mut c_char) -> c_int;
    fn ioctl(fd: c_int, req: u64, ...) -> c_int;
    fn close(fd: c_int) -> c_int;
}

#[repr(C)]
struct SyncFileInfo {
    name: [u8; 32],
    status: i32,
    flags: u32,
    num_fences: u32,
    pad: u32,
    fences: u64,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct SyncFenceInfo {
    obj_name: [u8; 32],
    driver_name: [u8; 32],
    status: i32,
    flags: u32,
    timestamp_ns: u64,
}
const SYNC_IOC_FILE_INFO: u64 = 0xC038_3E04;

/// When a signalled fence fired, in monotonic milliseconds.
fn fence_time(fd: c_int) -> Option<f64> {
    if fd < 0 {
        return None;
    }
    unsafe {
        let mut info = SyncFileInfo { name: [0; 32], status: 0, flags: 0, num_fences: 0, pad: 0, fences: 0 };
        let mut out = None;
        if ioctl(fd, SYNC_IOC_FILE_INFO, &mut info) == 0 && info.status == 1 && info.num_fences > 0 {
            let mut list = vec![SyncFenceInfo { obj_name: [0; 32], driver_name: [0; 32], status: 0, flags: 0, timestamp_ns: 0 }; info.num_fences as usize];
            info.fences = list.as_mut_ptr() as u64;
            if ioctl(fd, SYNC_IOC_FILE_INFO, &mut info) == 0 {
                out = list.iter().map(|f| f.timestamp_ns).max().map(|t| t as f64 / 1e6);
            }
        }
        close(fd);
        out
    }
}

fn api() -> Option<&'static Api> {
    static API: OnceLock<Option<Api>> = OnceLock::new();
    API.get_or_init(|| unsafe {
        let libs = [dlopen(c"libandroid.so".as_ptr(), 2), dlopen(c"libnativewindow.so".as_ptr(), 2)];
        let find = |name: &str| -> *mut c_void {
            let n = CString::new(name).unwrap();
            libs.iter().filter(|l| !l.is_null()).map(|l| dlsym(*l, n.as_ptr())).find(|p| !p.is_null()).unwrap_or(std::ptr::null_mut())
        };
        macro_rules! get {
            ($n:literal) => {{
                let p = find($n);
                if p.is_null() {
                    log(5, concat!("fast present: missing ", $n));
                    return None;
                }
                std::mem::transmute(p)
            }};
        }
        let supported = find("AHardwareBuffer_isSupported");
        Some(Api {
            sc_from_window: get!("ASurfaceControl_createFromWindow"),
            sc_release: get!("ASurfaceControl_release"),
            tx_create: get!("ASurfaceTransaction_create"),
            tx_delete: get!("ASurfaceTransaction_delete"),
            tx_apply: get!("ASurfaceTransaction_apply"),
            tx_buffer: get!("ASurfaceTransaction_setBuffer"),
            tx_damage: get!("ASurfaceTransaction_setDamageRegion"),
            tx_visible: get!("ASurfaceTransaction_setVisibility"),
            tx_opaque: get!("ASurfaceTransaction_setBufferTransparency"),
            hb_alloc: get!("AHardwareBuffer_allocate"),
            hb_release: get!("AHardwareBuffer_release"),
            hb_describe: get!("AHardwareBuffer_describe"),
            hb_lock: get!("AHardwareBuffer_lock"),
            hb_unlock: get!("AHardwareBuffer_unlock"),
            tx_on_complete: get!("ASurfaceTransaction_setOnComplete"),
            stats_present_fence: get!("ASurfaceTransactionStats_getPresentFenceFd"),
            hb_supported: if supported.is_null() { None } else { Some(std::mem::transmute(supported)) },
        })
    })
    .as_ref()
}

static ENABLED: AtomicBool = AtomicBool::new(true);

/// Turns the low-latency layer on or off (the in-app setting).
pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

/// 0 classic, 1 single (front) buffer, 2 double buffer, 3 automatic.
/// `debug.inkboard.present` overrides the setting, for testing.
pub fn mode() -> u32 {
    let mut v = [0 as c_char; 92];
    unsafe { __system_property_get(c"debug.inkboard.present".as_ptr(), v.as_mut_ptr()) };
    match v[0] as u8 {
        b'0'..=b'2' => (v[0] as u8 - b'0') as u32,
        _ if ENABLED.load(Ordering::Relaxed) => 3,
        _ => 0,
    }
}

/// True when buffers can be written while the display scans them out.
fn front_buffer_ok(api: &Api) -> bool {
    static OK: OnceLock<bool> = OnceLock::new();
    *OK.get_or_init(|| {
        let d = Desc { width: 64, height: 64, layers: 1, format: 1, usage: CPU_WRITE_OFTEN | GPU_SAMPLED | COMPOSER_OVERLAY | FRONT_BUFFER, stride: 0, rfu0: 0, rfu1: 0 };
        api.hb_supported.map(|f| unsafe { f(&d) } != 0).unwrap_or(false)
    })
}

struct Buf {
    hb: *mut c_void,
    stride: usize,
    /// What changed on screen since this buffer was last written.
    stale: IRect,
}

struct Fast {
    window: *mut ANativeWindow,
    sc: *mut c_void,
    bufs: Vec<Buf>,
    next: usize,
    w: u32,
    h: u32,
}
unsafe impl Send for Fast {}

static FAILED: AtomicUsize = AtomicUsize::new(0);
static FULL_REDRAW: AtomicBool = AtomicBool::new(false);

/// True once after the layer went away: the window's own buffers are stale
/// and need a full redraw.
pub fn take_full_redraw() -> bool {
    FULL_REDRAW.swap(false, Ordering::Relaxed)
}

impl Fast {
    fn hide(&self, api: &Api) {
        if self.sc.is_null() {
            return;
        }
        unsafe {
            let tx = (api.tx_create)();
            (api.tx_visible)(tx, self.sc, 0);
            (api.tx_apply)(tx);
            (api.tx_delete)(tx);
        }
    }

    fn release(&mut self, api: &Api) {
        unsafe {
            for b in &self.bufs {
                (api.hb_release)(b.hb);
            }
            if !self.sc.is_null() {
                (api.sc_release)(self.sc);
            }
        }
        self.bufs.clear();
        self.sc = std::ptr::null_mut();
    }
}

static STATE: Mutex<Option<Fast>> = Mutex::new(None);

fn create(api: &Api, window: *mut ANativeWindow, w: u32, h: u32, count: usize) -> Option<Fast> {
    unsafe {
        let sc = (api.sc_from_window)(window, c"inkboard-ink".as_ptr());
        if sc.is_null() {
            return None;
        }
        let mut desc = Desc { width: w, height: h, layers: 1, format: 1, usage: CPU_WRITE_OFTEN | GPU_SAMPLED | COMPOSER_OVERLAY, stride: 0, rfu0: 0, rfu1: 0 };
        if count == 1 {
            let mut front = desc;
            front.usage |= FRONT_BUFFER;
            if api.hb_supported.map(|f| f(&front) != 0).unwrap_or(false) {
                desc = front;
                log(4, "fast present: front-buffer usage supported");
            } else {
                log(4, "fast present: front-buffer usage not supported, using plain buffer");
            }
        }
        let mut fast = Fast { window, sc, bufs: Vec::new(), next: 0, w, h };
        for _ in 0..count {
            let mut hb = std::ptr::null_mut();
            if (api.hb_alloc)(&desc, &mut hb) != 0 || hb.is_null() {
                fast.release(api);
                return None;
            }
            let mut d = desc;
            (api.hb_describe)(hb, &mut d);
            fast.bufs.push(Buf { hb, stride: d.stride as usize, stale: IRect { x0: 0, y0: 0, x1: w as i32, y1: h as i32 } });
        }
        log(4, &format!("fast present: mode {} with {} buffer(s) {}x{}", mode(), count, w, h));
        Some(fast)
    }
}

/// Releases the layer that belongs to `window`, before the window goes.
pub fn forget(window: *mut ANativeWindow) {
    let Some(api) = api() else { return };
    let mut st = STATE.lock().unwrap();
    if st.as_ref().map(|f| f.window == window).unwrap_or(false) {
        if let Some(mut f) = st.take() {
            f.release(api);
        }
    }
}

/// Shows `frame` through a child layer. `None` means use the classic path.
pub fn present(window: *mut ANativeWindow, frame: &Layer, dirty: IRect) -> Option<bool> {
    let m = mode();
    if m == 0 {
        // Switched off: take the layer down so the window shows again.
        if let Some(api) = api() {
            if let Some(mut f) = STATE.lock().unwrap().take() {
                f.hide(api);
                f.release(api);
                FULL_REDRAW.store(true, Ordering::Relaxed);
            }
        }
        return None;
    }
    if FAILED.load(Ordering::Relaxed) == window as usize {
        return None;
    }
    let api = api()?;
    let mut st = STATE.lock().unwrap();
    let count = match m {
        1 => 1,
        2 => 2,
        _ => if front_buffer_ok(api) { 1 } else { 2 },
    };
    let fits = st.as_ref().map(|f| f.window == window && f.w == frame.w && f.h == frame.h && f.bufs.len() == count).unwrap_or(false);
    if !fits {
        if let Some(mut f) = st.take() {
            f.release(api);
        }
        match create(api, window, frame.w, frame.h, count) {
            Some(f) => *st = Some(f),
            None => {
                // Not available on this device: stay on the classic path.
                log(5, "fast present: unavailable, using the classic path");
                FAILED.store(window as usize, Ordering::Relaxed);
                FULL_REDRAW.store(true, Ordering::Relaxed);
                return None;
            }
        }
    }
    let fast = st.as_mut().unwrap();
    let full = IRect { x0: 0, y0: 0, x1: frame.w as i32, y1: frame.h as i32 };
    for b in fast.bufs.iter_mut() {
        b.stale = if b.stale.is_empty() { dirty } else { b.stale.union(&dirty) };
    }
    let i = fast.next;
    fast.next = (fast.next + 1) % fast.bufs.len();
    let b = &mut fast.bufs[i];
    let r = b.stale.intersect(&full);
    b.stale = IRect { x0: 0, y0: 0, x1: 0, y1: 0 };
    let rect = ARect { left: r.x0, top: r.y0, right: r.x1, bottom: r.y1 };
    unsafe {
        if r.x1 > r.x0 && r.y1 > r.y0 {
            let mut ptr: *mut c_void = std::ptr::null_mut();
            if (api.hb_lock)(b.hb, CPU_WRITE_OFTEN, -1, &rect, &mut ptr) != 0 || ptr.is_null() {
                return Some(false);
            }
            if r.x0 == 0 && r.x1 == frame.w as i32 && b.stride == frame.w as usize {
                let s = (r.y0 as usize * frame.w as usize) * 4;
                let len = ((r.y1 - r.y0) as usize * frame.w as usize) * 4;
                let d = (ptr as *mut u8).add(s);
                std::ptr::copy_nonoverlapping(frame.data[s..s + len].as_ptr(), d, len);
            } else {
                let n = (r.x1 - r.x0) as usize * 4;
                for y in r.y0..r.y1 {
                    let s = (y as usize * frame.w as usize + r.x0 as usize) * 4;
                    let d = (ptr as *mut u8).add((y as usize * b.stride + r.x0 as usize) * 4);
                    std::ptr::copy_nonoverlapping(frame.data[s..s + n].as_ptr(), d, n);
                }
            }
            let mut fence = -1;
            (api.hb_unlock)(b.hb, &mut fence);
            let tx = (api.tx_create)();
            (api.tx_buffer)(tx, fast.sc, b.hb, fence);
            let damage = ARect { left: dirty.x0, top: dirty.y0, right: dirty.x1, bottom: dirty.y1 };
            (api.tx_damage)(tx, fast.sc, &damage, 1);
            (api.tx_visible)(tx, fast.sc, 1);
            (api.tx_opaque)(tx, fast.sc, 2);
            let posted = Box::into_raw(Box::new(super::now_ms())) as *mut c_void;
            (api.tx_on_complete)(tx, posted, on_complete);
            (api.tx_apply)(tx);
            (api.tx_delete)(tx);
        }
    }
    Some(true)
}

/// Post-to-present (from the present fence) and post-to-callback times.
static SHOWN: Mutex<(u32, f64, f64, f64, u32)> = Mutex::new((0, 0.0, 0.0, 0.0, 0));

unsafe extern "C" fn on_complete(ctx: *mut c_void, stats: *mut c_void) {
    let posted = *Box::from_raw(ctx as *mut f64);
    let now = super::now_ms();
    let shown = api().and_then(|a| fence_time((a.stats_present_fence)(stats)));
    let mut s = SHOWN.lock().unwrap();
    s.0 += 1;
    if let Some(t) = shown {
        s.4 += 1;
        s.1 += t - posted;
        s.3 = s.3.max(t - posted);
    }
    s.2 += now - posted;
    if s.0 == 100 {
        let n = s.4.max(1) as f64;
        log(4, &format!("fast present: post->present(fence) avg {:.1} ms max {:.1} ms ({} fences), post->callback avg {:.1} ms", s.1 / n, s.3, s.4, s.2 / 100.0));
        *s = (0, 0.0, 0.0, 0.0, 0);
    }
}
