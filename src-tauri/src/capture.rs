//! Picture sources: a game window (the platform's window capture, see capture_windows.rs /
//! capture_xcap.rs), a capture card (device.rs) or an OBS source (obs.rs), all feeding the same
//! shared frame slot.

use std::sync::atomic::{AtomicIsize, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::config::Config;

#[cfg_attr(windows, path = "capture_windows.rs")]
#[cfg_attr(not(windows), path = "capture_xcap.rs")]
mod platform;
/// Prefix that marks a whole screen (not a window) in the config's source and overlay fields.
pub const SCREEN_PREFIX: &str = "screen:";

/// A screen the app can read or draw over.
#[derive(Serialize, Clone)]
pub struct ScreenInfo {
    /// the system's id for it (Windows: "\\.\DISPLAY1")
    pub id: String,
    /// its number (1, 2 …) and its own name ("DELL U2720Q"), if known
    pub number: usize,
    pub name: String,
    pub w: u32,
    pub h: u32,
}

#[allow(unused_imports)] // (foreground_window: used inside the platforms only)
pub use platform::{client_rect, find_window, focused_client_rect, foreground_window, grab_once, list_screens, screen_rect, grab_screen_region, list_windows};

/// One captured frame of the window's client area, RGBA.
pub use vi_translator_core::frame::FrameData;

#[derive(Default)]
pub struct Shared {
    /// What the overlays show now, the game page on screen, the lines for the home-network page and
    /// the connected TVs: the core's (vi_translator_core::live), shared with its reading. Its fields
    /// read as Shared's own (`shared.screen`, `shared.tvs` …).
    pub live: Arc<vi_translator_core::live::Live>,
    pub frame: Mutex<Option<Arc<FrameData>>>,
    /// HWND of the window being captured (0 = none).
    pub hwnd: AtomicIsize,
    /// The part of the captured window that is the game picture (x, y, w, h fractions); frames keep
    /// only that part. None: the whole frame.
    pub source_crop: Mutex<Option<[f64; 4]>>,
    /// The window the Arabic is drawn over (the source window unless the player chose another; 0 =
    /// none) and the part of it showing the game.
    pub overlay_hwnd: AtomicIsize,
    pub overlay_crop: Mutex<Option<[f64; 4]>>,
    /// …or a whole screen the Arabic is drawn over (its place on the desktop), instead of a window
    pub overlay_screen: Mutex<Option<(i32, i32, i32, i32)>>,
    /// The source is a whole screen: the overlays keep out of screen captures (else the app would
    /// read its own Arabic back)
    pub reading_screen: std::sync::atomic::AtomicBool,
    /// The source is read from the screen's pixels (the compatibility capture, where Windows Graphics
    /// Capture can't start): the overlays keep out of screen captures too, for the same reason.
    pub compat_capture: std::sync::atomic::AtomicBool,
    /// The virtual camera's state for the Show on tab ("" while it's off).
    pub vcam_status: Mutex<String>,
    /// The capture card's sound: what it's doing ("" while it's off or there's no card).
    pub sound_status: Mutex<String>,
    pub seq: AtomicU64,
    pub status: Mutex<String>,
    /// Live preview, separate from the full frames read for text: a frame (≤ PREVIEW_W wide) at
    /// the preview rate, made only while someone watches (see `watching`), and its JPEG.
    pub preview_raw: Mutex<Option<Arc<FrameData>>>,
    pub preview_jpeg: Mutex<Option<(u64, Arc<Vec<u8>>)>>,
    /// Or, from a capture card that sends YUY2: the card's own frame as it came, no conversion or
    /// JPEG (the window turns it into colour on the GPU, the way OBS does). Numbered with `preview_seq`.
    pub preview_yuv: Mutex<Option<(u64, Arc<YuvFrame>)>>,
    pub preview_seq: AtomicU64,
    pub preview_clients: AtomicUsize,
    /// when the app's window last asked for a preview frame (ms since the app started)
    pub preview_pulled_ms: AtomicU64,
    /// the app window is minimised or hidden: no preview frames
    pub preview_paused: std::sync::atomic::AtomicBool,
    /// ms between full frames (for reading text) and between preview frames (0 = default)
    pub capture_ms: AtomicU64,
    pub preview_ms: AtomicU64,
}

impl std::ops::Deref for Shared {
    type Target = vi_translator_core::live::Live;
    fn deref(&self) -> &Self::Target {
        &self.live
    }
}

/// Preview frames are at most this wide: a 1080p game is shown at full size, a 4K one at half
/// (unless the 4K picture is on: `preview_w`).
pub const PREVIEW_W: u32 = 1920;
/// The 4K picture's limit (Config::picture_4k).
pub const PREVIEW_W_4K: u32 = 3840;
static PREVIEW_MAX: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(PREVIEW_W);

/// How wide the app's picture may be now (PREVIEW_W, or PREVIEW_W_4K with the 4K picture on).
pub fn preview_w() -> u32 {
    PREVIEW_MAX.load(Ordering::Relaxed)
}

/// How a card's YUV numbers map to colour: which standard, and whether black is 16 or 0.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct YuvColor {
    pub bt709: bool,
    pub full_range: bool,
}

/// A YUY2 frame: 2 bytes per pixel, each pair of pixels stored as Y0 U Y1 V, rows `w * 2` bytes.
pub struct YuvFrame {
    pub w: u32,
    pub h: u32,
    pub color: YuvColor,
    pub data: Vec<u8>,
}

/// YUY2 → RGBA (for reading text; the preview does the same on the GPU).
pub fn yuy2_to_rgba(src: &[u8], w: u32, h: u32, c: YuvColor) -> Vec<u8> {
    // R = Y + kr·V, G = Y − kgu·U − kgv·V, B = Y + kb·U, in fixed point (×1024)
    let (kr, kgu, kgv, kb) = if c.bt709 { (1.5748, 0.1873, 0.4681, 1.8556) } else { (1.402, 0.3441, 0.7141, 1.772) };
    let (ys, yo, cs): (f64, i32, f64) = if c.full_range { (1.0, 0, 1.0) } else { (255.0 / 219.0, 16, 255.0 / 224.0) };
    let f = |k: f64| (k * cs * 1024.0).round() as i32;
    let (ym, cr, cgu, cgv, cb) = ((ys * 1024.0).round() as i32, f(kr), f(kgu), f(kgv), f(kb));
    let clamp = |v: i32| ((v + 512) >> 10).clamp(0, 255) as u8; // rounded
    let n = (w * h) as usize;
    let mut out = vec![0u8; n * 4];
    for (i, q) in src[..n * 2].chunks_exact(4).enumerate() {
        let (u, v) = (q[1] as i32 - 128, q[3] as i32 - 128);
        let (dr, dg, db) = (cr * v, cgu * u + cgv * v, cb * u);
        for (j, y) in [q[0], q[2]].into_iter().enumerate() {
            let y = (y as i32 - yo) * ym;
            let o = (i * 2 + j) * 4;
            out[o] = clamp(y + dr);
            out[o + 1] = clamp(y - dg);
            out[o + 2] = clamp(y + db);
            out[o + 3] = 255;
        }
    }
    out
}

/// BGRX (rows `w * 4` bytes) → YUY2 in BT.709 video range, the colour `yuy2_to_rgba` and the preview's
/// shader turn back into the same picture (each pair of pixels shares the average of their colour).
#[cfg_attr(not(windows), allow(dead_code))] // (the Windows capture card reader's)
pub fn bgrx_to_yuy2(src: &[u8], w: u32, h: u32) -> Vec<u8> {
    // Y = 16 + 219·(Kr R + Kg G + Kb B)/255, U and V: 128 + 224·(B−Y', R−Y')/(2·(1−K))/255, ×1024
    let (kr, kb) = (0.2126f64, 0.0722f64);
    let kg = 1.0 - kr - kb;
    let fy = |k: f64| (k * 219.0 / 255.0 * 1024.0).round() as i32;
    let (yr, yg, yb) = (fy(kr), fy(kg), fy(kb));
    let fc = |k: f64| (k * 224.0 / 255.0 * 1024.0).round() as i32;
    let (ur, ug, ub) = (fc(-kr / (2.0 * (1.0 - kb))), fc(-kg / (2.0 * (1.0 - kb))), fc(0.5));
    let (vr, vg, vb) = (fc(0.5), fc(-kg / (2.0 * (1.0 - kr))), fc(-kb / (2.0 * (1.0 - kr))));
    let (w, h) = (w as usize & !1, h as usize);
    let mut out = vec![0u8; w * h * 2];
    for y in 0..h {
        let row = &src[y * (w * 4)..];
        let o = &mut out[y * w * 2..(y + 1) * w * 2];
        for x in (0..w).step_by(2) {
            let (b0, g0, r0) = (row[x * 4] as i32, row[x * 4 + 1] as i32, row[x * 4 + 2] as i32);
            let (b1, g1, r1) = (row[x * 4 + 4] as i32, row[x * 4 + 5] as i32, row[x * 4 + 6] as i32);
            let luma = |r: i32, g: i32, b: i32| ((16 << 10) + yr * r + yg * g + yb * b + 512) >> 10;
            let (r, g, b) = ((r0 + r1) / 2, (g0 + g1) / 2, (b0 + b1) / 2);
            let u = ((128 << 10) + ur * r + ug * g + ub * b + 512) >> 10;
            let v = ((128 << 10) + vr * r + vg * g + vb * b + 512) >> 10;
            o[x * 2] = luma(r0, g0, b0).clamp(0, 255) as u8;
            o[x * 2 + 1] = u.clamp(0, 255) as u8;
            o[x * 2 + 2] = luma(r1, g1, b1).clamp(0, 255) as u8;
            o[x * 2 + 3] = v.clamp(0, 255) as u8;
        }
    }
    out
}

/// Milliseconds since the app started (a steady clock for "when did X last happen").
fn app_ms() -> u64 {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
}

impl Shared {
    pub fn latest(&self) -> Option<Arc<FrameData>> {
        self.frame.lock().unwrap().clone()
    }
    /// Time for the next full frame (the one text is read from)?
    pub fn full_due(&self, last: Instant) -> bool {
        last.elapsed() >= Duration::from_millis(self.capture_ms.load(Ordering::Relaxed).max(15))
    }
    /// Is anyone watching the preview: an open stream, or the window asked for a frame in the last 2 s?
    pub fn watching(&self) -> bool {
        self.preview_clients.load(Ordering::Relaxed) > 0
            || app_ms().saturating_sub(self.preview_pulled_ms.load(Ordering::Relaxed)) < 2000
                && self.preview_pulled_ms.load(Ordering::Relaxed) > 0
    }
    /// The window just asked for a preview frame.
    pub fn mark_preview_pulled(&self) {
        self.preview_pulled_ms.store(app_ms().max(1), Ordering::Relaxed);
    }
    /// Time for the next preview frame — and is anyone watching?
    pub fn preview_due(&self, last: Instant) -> bool {
        self.watching()
            && !self.preview_paused.load(Ordering::Relaxed)
            // ¾ of the interval: frames arrive every ~16.7 ms, and a strict 33 ms test would often just
            // miss one and wait a whole extra frame (30 fps would become ~20)
            && last.elapsed() >= Duration::from_millis(self.preview_ms.load(Ordering::Relaxed).max(16) * 3 / 4)
    }
    /// The preview interval exactly (for sources we pull from, like OBS, rather than ones that push frames).
    pub fn preview_interval(&self) -> Duration {
        Duration::from_millis(self.preview_ms.load(Ordering::Relaxed).max(16))
    }
    /// Store a full frame (only the game's part of it, when the window shows more than the game).
    pub fn put_full(&self, w: u32, h: u32, rgba: Vec<u8>) {
        // (every reader of a frame takes its size from w and h: an empty or short one is no frame)
        if w == 0 || h == 0 || rgba.len() < w as usize * h as usize * 4 {
            return;
        }
        let crop = *self.source_crop.lock().unwrap();
        let (w, h, rgba) = match crop {
            Some(c) => match crop_rgba(&rgba, w, h, c) {
                Some(f) => f,
                None => return,
            },
            None => (w, h, rgba),
        };
        let seq = self.seq.fetch_add(1, Ordering::Relaxed) + 1;
        *self.frame.lock().unwrap() = Some(Arc::new(FrameData { w, h, rgba, seq }));
    }
    /// Store a preview frame (small RGBA; the preview thread turns it into JPEG).
    pub fn put_preview(&self, f: FrameData) {
        let crop = *self.source_crop.lock().unwrap();
        let f = match crop {
            Some(c) => match crop_rgba(&f.rgba, f.w, f.h, c) {
                Some((w, h, rgba)) => FrameData { w, h, rgba, seq: f.seq },
                None => return,
            },
            None => f,
        };
        *self.preview_raw.lock().unwrap() = Some(Arc::new(f));
    }
    /// Store a raw YUY2 preview frame (a capture card's own picture).
    #[cfg_attr(not(windows), allow(dead_code))] // (the Windows capture card reader's)
    pub fn put_preview_yuv(&self, f: YuvFrame) {
        let seq = self.preview_seq.fetch_add(1, Ordering::Relaxed) + 1;
        *self.preview_yuv.lock().unwrap() = Some((seq, Arc::new(f)));
    }
    /// Store a ready-made preview JPEG (OBS sends one).
    pub fn put_preview_jpeg(&self, jpeg: Vec<u8>) {
        let seq = self.preview_seq.fetch_add(1, Ordering::Relaxed) + 1;
        *self.preview_jpeg.lock().unwrap() = Some((seq, Arc::new(jpeg)));
    }
    pub fn set_status(&self, s: impl Into<String>) {
        *self.status.lock().unwrap() = s.into();
    }
}

/// A part (x, y, w, h fractions) of a rectangle of `w` × `h` pixels, in whole pixels: (x, y, w, h).
pub fn crop_px(w: u32, h: u32, c: [f64; 4]) -> (u32, u32, u32, u32) {
    let x0 = ((c[0] * w as f64).round() as u32).min(w.saturating_sub(1));
    let y0 = ((c[1] * h as f64).round() as u32).min(h.saturating_sub(1));
    let cw = ((c[2] * w as f64).round() as u32).clamp(1, w - x0);
    let ch = ((c[3] * h as f64).round() as u32).clamp(1, h - y0);
    (x0, y0, cw, ch)
}

/// The part `c` of an RGBA image (rows `w * 4` bytes): its size and pixels.
pub fn crop_rgba(rgba: &[u8], w: u32, h: u32, c: [f64; 4]) -> Option<(u32, u32, Vec<u8>)> {
    if w == 0 || h == 0 || rgba.len() < (w * h * 4) as usize {
        return None;
    }
    let (x0, y0, cw, ch) = crop_px(w, h, c);
    let mut out = Vec::with_capacity((cw * ch * 4) as usize);
    for y in y0..y0 + ch {
        let start = ((y * w + x0) * 4) as usize;
        out.extend_from_slice(&rgba[start..start + (cw * 4) as usize]);
    }
    Some((cw, ch, out))
}

/// A part of a screen rectangle (x, y, w, h in pixels): the whole of it without a part.
pub fn crop_screen_rect(r: (i32, i32, i32, i32), c: Option<[f64; 4]>) -> (i32, i32, i32, i32) {
    let Some(c) = c else { return r };
    let (x, y, w, h) = r;
    let (cx, cy, cw, ch) = crop_px(w.max(1) as u32, h.max(1) as u32, c);
    (x + cx as i32, y + cy as i32, cw as i32, ch as i32)
}

/// Where the Arabic goes on this PC: the game's part of the overlay window, in screen pixels; with
/// `front_only`, only while that window is the one in front (not over other apps).
pub fn overlay_rect(shared: &Shared, front_only: bool) -> Option<(i32, i32, i32, i32)> {
    // a whole screen: always there (it has no "in front")
    if let Some(r) = *shared.overlay_screen.lock().unwrap() {
        return Some(crop_screen_rect(r, *shared.overlay_crop.lock().unwrap()));
    }
    let hwnd = shared.overlay_hwnd.load(Ordering::Relaxed);
    let r = if front_only { focused_client_rect(hwnd) } else { client_rect(hwnd) }?;
    Some(crop_screen_rect(r, *shared.overlay_crop.lock().unwrap()))
}

/// Shrink a 4-byte-per-pixel image (rows `pitch` bytes apart) to ≤ PREVIEW_W wide, averaging 2×2
/// samples per output pixel. `bgr`: the source is BGRA/BGRX (output is always RGBA).
pub fn shrink(src: &[u8], w: u32, h: u32, pitch: usize, bgr: bool) -> FrameData {
    let f = w.div_ceil(preview_w()).max(1);
    if f == 1 {
        // already small enough: copy the rows as they are (RGBA out)
        let row = (w * 4) as usize;
        let mut out = Vec::with_capacity(row * h as usize);
        for y in 0..h as usize {
            out.extend_from_slice(&src[y * pitch..y * pitch + row]);
        }
        for p in out.chunks_exact_mut(4) {
            if bgr {
                p.swap(0, 2);
            }
            p[3] = 255;
        }
        return FrameData { w, h, rgba: out, seq: 0 };
    }
    let (ow, oh) = (w / f, h / f);
    let mut out = vec![255u8; (ow * oh * 4) as usize];
    let half = (f / 2) as usize;
    let (r, b) = if bgr { (2, 0) } else { (0, 2) };
    for y in 0..oh as usize {
        let y0 = y * f as usize;
        let y1 = (y0 + half).min(h as usize - 1);
        for x in 0..ow as usize {
            let x0 = x * f as usize * 4;
            let x1 = ((x * f as usize + half).min(w as usize - 1)) * 4;
            let o = (y * ow as usize + x) * 4;
            for (c, sc) in [(0, r), (1, 1), (2, b)] {
                let sum = src[y0 * pitch + x0 + sc] as u32
                    + src[y0 * pitch + x1 + sc] as u32
                    + src[y1 * pitch + x0 + sc] as u32
                    + src[y1 * pitch + x1 + sc] as u32;
                out[o + c] = (sum / 4) as u8;
            }
        }
    }
    FrameData { w: ow, h: oh, rgba: out, seq: 0 }
}

/// Save a frame as PNG.
pub fn save_png(f: &FrameData, path: &std::path::Path) -> anyhow::Result<()> {
    let img = image::RgbaImage::from_raw(f.w, f.h, f.rgba.clone()).ok_or_else(|| anyhow::anyhow!("bad frame"))?;
    img.save(path)?;
    Ok(())
}

/// A Mac nobody touches goes to sleep, and with a console the player never touches it: the picture,
/// the reading and the translations on the TV or the phone all stop. So while a picture is coming
/// in, macOS's own `caffeinate` holds idle sleep off (-i). It ends with the app (-w), however the
/// app ends. (The screen still turns off, which does no harm; a closed lid still sleeps the Mac.
/// A card with its console off still sends a picture, "No Signal": the Mac stays awake then too.)
#[cfg(target_os = "macos")]
fn keep_awake(on: bool, held: &mut Option<std::process::Child>) {
    // (one that was ended from outside is started again)
    let running = held.as_mut().is_some_and(|c| matches!(c.try_wait(), Ok(None)));
    if on && !running {
        *held = std::process::Command::new("caffeinate").args(["-i", "-w", &std::process::id().to_string()]).spawn().ok();
    } else if !on {
        if let Some(mut c) = held.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

/// Keeps a capture running on the window named in the config; restarts it when the title changes
/// or the window is closed and reopened.
pub fn run_manager(shared: Arc<Shared>, cfg: Arc<Mutex<Config>>) {
    let mut active: Option<(String, platform::WindowCapture)> = None;
    let mut device: Option<(String, crate::device::DeviceCapture)> = None;
    let mut obs: Option<(String, crate::obs::ObsCapture)> = None;
    let mut ps5: Option<crate::ps5::Ps5Capture> = None;
    #[cfg(target_os = "macos")]
    let (mut awake, mut pictured) = (None, (0u64, None::<Instant>));
    loop {
        let (want, obs_port, obs_password, source_crop, overlay_window, overlay_crop, picture_4k) = {
            let c = cfg.lock().unwrap();
            shared.capture_ms.store(1000 / c.capture_fps.clamp(1, 60) as u64, Ordering::Relaxed);
            shared.preview_ms.store(1000 / c.preview_fps.clamp(1, 60) as u64, Ordering::Relaxed);
            PREVIEW_MAX.store(if c.picture_4k { PREVIEW_W_4K } else { PREVIEW_W }, Ordering::Relaxed);
            (c.window_title.clone(), c.obs_port, c.obs_password.clone(), c.source_crop, c.overlay_window.clone(), c.overlay_crop, c.picture_4k)
        };
        #[cfg(target_os = "macos")]
        {
            let seq = shared.seq.load(Ordering::Relaxed);
            if seq != pictured.0 {
                pictured = (seq, Some(Instant::now()));
            }
            // (a frame in the last minute: a short gap in the picture doesn't let the Mac sleep)
            keep_awake(pictured.1.is_some_and(|t| t.elapsed() < Duration::from_secs(60)), &mut awake);
        }
        let screen = want.strip_prefix(SCREEN_PREFIX).map(str::to_string);
        let is_window = !want.starts_with(crate::obs::PREFIX) && !want.starts_with(crate::device::PREFIX) && !want.starts_with(crate::ps5::PREFIX);
        shared.reading_screen.store(screen.is_some(), Ordering::Relaxed);
        // (a part of the picture only for a window or a screen: a card's or OBS's picture is the game already)
        *shared.source_crop.lock().unwrap() = if is_window { source_crop } else { None };
        // The overlays' window or screen: the one the player chose, else the source's (none for a
        // card or OBS)
        if overlay_window.is_empty() {
            shared.overlay_hwnd.store(if is_window { shared.hwnd.load(Ordering::Relaxed) } else { 0 }, Ordering::Relaxed);
            *shared.overlay_screen.lock().unwrap() = screen.as_deref().and_then(screen_rect);
            *shared.overlay_crop.lock().unwrap() = if is_window { source_crop } else { None };
        } else if let Some(id) = overlay_window.strip_prefix(SCREEN_PREFIX) {
            shared.overlay_hwnd.store(0, Ordering::Relaxed);
            *shared.overlay_screen.lock().unwrap() = screen_rect(id);
            *shared.overlay_crop.lock().unwrap() = overlay_crop;
        } else {
            shared.overlay_hwnd.store(find_window(&overlay_window).unwrap_or(0), Ordering::Relaxed);
            *shared.overlay_screen.lock().unwrap() = None;
            *shared.overlay_crop.lock().unwrap() = overlay_crop;
        }
        // An OBS source ("obs:<name>"): OBS hands us snapshots of it
        if let Some(name) = want.strip_prefix(crate::obs::PREFIX) {
            if let Some((_, c)) = active.take() {
                let _ = c.stop();
            }
            if let Some((_, d)) = device.take() {
                d.stop();
            }
            if let Some(p) = ps5.take() {
                p.stop();
            }
            let key = format!("{name}|{obs_port}|{obs_password}");
            if obs.as_ref().is_none_or(|(k, o)| *k != key || o.is_finished()) {
                if let Some((_, o)) = obs.take() {
                    o.stop();
                }
                shared.hwnd.store(0, Ordering::Relaxed);
                *shared.frame.lock().unwrap() = None;
                shared.set_status(format!("Connecting to OBS for \"{name}\" ..."));
                obs = Some((key, crate::obs::start(name.to_string(), obs_port, obs_password, shared.clone())));
            }
            std::thread::sleep(Duration::from_secs(1));
            continue;
        }
        if let Some((_, o)) = obs.take() {
            o.stop();
            *shared.frame.lock().unwrap() = None;
        }
        // The PS5 in the app ("ps5:"): chiaki-ng's stream, decoded here
        if want.starts_with(crate::ps5::PREFIX) {
            if let Some((_, c)) = active.take() {
                let _ = c.stop();
            }
            if let Some((_, d)) = device.take() {
                d.stop();
            }
            if ps5.as_ref().is_none_or(|p| p.is_finished()) {
                if let Some(p) = ps5.take() {
                    p.stop();
                }
                shared.hwnd.store(0, Ordering::Relaxed); // no window: the Arabic goes to the preview / OBS / the TV
                *shared.frame.lock().unwrap() = None;
                ps5 = Some(crate::ps5::start(shared.clone()));
            }
            std::thread::sleep(Duration::from_secs(1));
            continue;
        }
        if let Some(p) = ps5.take() {
            p.stop();
            *shared.frame.lock().unwrap() = None;
            crate::ps5::may_wake_again(); // the PS5 chosen again later wakes it as at the start
        }
        // A capture card / video device ("device:<name>") instead of a window
        if let Some(name) = want.strip_prefix(crate::device::PREFIX) {
            if let Some((_, c)) = active.take() {
                let _ = c.stop();
            }
            // (the 4K picture switched on or off: the card is opened again at its new size)
            let key = format!("{name}|{picture_4k}");
            let restart = device.as_ref().is_none_or(|(k, d)| *k != key || d.is_finished());
            if restart {
                if let Some((_, d)) = device.take() {
                    d.stop();
                }
                shared.hwnd.store(0, Ordering::Relaxed); // no window: the Arabic goes to OBS / the preview
                *shared.frame.lock().unwrap() = None;
                shared.set_status(format!("Opening capture device \"{name}\" ..."));
                device = Some((key, crate::device::start(name.to_string(), shared.clone(), picture_4k)));
            }
            std::thread::sleep(Duration::from_secs(1));
            continue;
        }
        if let Some((_, d)) = device.take() {
            d.stop();
            *shared.frame.lock().unwrap() = None;
        }
        // A whole screen ("screen:<id>")
        if let Some(id) = &screen {
            let finished = active.as_ref().map(|(_, c)| c.is_finished()).unwrap_or(true);
            let changed = active.as_ref().map(|(t, _)| *t != want).unwrap_or(true);
            if changed || finished {
                if let Some((_, c)) = active.take() {
                    let _ = c.stop();
                }
                shared.hwnd.store(0, Ordering::Relaxed);
                *shared.frame.lock().unwrap() = None;
                match platform::start_screen(id, shared.clone()) {
                    Ok(c) => {
                        shared.set_status(if c.is_compat() {
                            format!("Capturing screen \"{id}\" (compatibility mode)")
                        } else {
                            format!("Capturing screen \"{id}\"")
                        });
                        active = Some((want, c));
                    }
                    Err(e) => shared.set_status(e.to_string()),
                }
            }
            std::thread::sleep(Duration::from_secs(1));
            continue;
        }
        let finished = active.as_ref().map(|(_, c)| c.is_finished()).unwrap_or(true);
        let changed = active.as_ref().map(|(t, _)| *t != want).unwrap_or(true);
        if changed || finished {
            if let Some((_, c)) = active.take() {
                let _ = c.stop();
            }
            shared.hwnd.store(0, Ordering::Relaxed);
            *shared.frame.lock().unwrap() = None;
            match platform::start_window(&want, shared.clone()) {
                Ok((c, hwnd)) => {
                    shared.hwnd.store(hwnd, Ordering::Relaxed);
                    if overlay_window.is_empty() {
                        shared.overlay_hwnd.store(hwnd, Ordering::Relaxed);
                    }
                    shared.set_status(if c.is_compat() {
                        format!("Capturing \"{want}\" (compatibility mode: only while it's the window in front, and what covers it is read too)")
                    } else {
                        format!("Capturing \"{want}\"")
                    });
                    active = Some((want, c));
                }
                // (the Xbox player is the app's own window: it isn't open)
                Err(_) if want == crate::xbox::TITLE => shared.set_status("The Xbox player is closed: open it in Source (Open the Xbox player)."),
                Err(e) => shared.set_status(e.to_string()),
            }
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

/// The program behind a window source the app knows by name, for a window whose title the system
/// translated: (Windows executable, macOS app name).
pub fn program_of(title: &str) -> Option<&'static str> {
    match title {
        "PS Remote Play" => Some(if cfg!(windows) { "RemotePlay.exe" } else { "PS Remote Play" }),
        _ => None,
    }
}

#[derive(Serialize)]
pub struct WindowInfo {
    pub title: String,
    pub process: String,
}


#[cfg(test)]
mod crop_tests {
    use super::*;

    #[test]
    fn a_part_of_the_picture() {
        // a 4 x 2 picture whose pixels are numbered 0..8 (in the red byte)
        let rgba: Vec<u8> = (0..8u8).flat_map(|i| [i, 0, 0, 255]).collect();
        let (w, h, px) = crop_rgba(&rgba, 4, 2, [0.5, 0.5, 0.5, 0.5]).unwrap();
        assert_eq!((w, h), (2, 1));
        assert_eq!(px.chunks(4).map(|p| p[0]).collect::<Vec<_>>(), vec![6, 7]);
        // on screen: a window at (100, 50), 400 x 200, its right half
        assert_eq!(crop_screen_rect((100, 50, 400, 200), Some([0.5, 0.0, 0.5, 1.0])), (300, 50, 200, 200));
        assert_eq!(crop_screen_rect((100, 50, 400, 200), None), (100, 50, 400, 200));
    }

    #[test]
    fn frames_keep_only_the_game_part() {
        let s = Shared::default();
        *s.source_crop.lock().unwrap() = Some([0.25, 0.0, 0.5, 1.0]);
        s.put_full(4, 2, vec![9; 32]);
        let f = s.latest().unwrap();
        assert_eq!((f.w, f.h, f.rgba.len()), (2, 2, 16));
    }
}

#[cfg(test)]
mod yuv_tests {
    use super::*;

    #[test]
    fn rgb_goes_to_yuy2_and_back() {
        // pairs of equal pixels (YUY2 shares colour between the two): white, black, red, a grey-blue
        let px: [[u8; 3]; 4] = [[255, 255, 255], [0, 0, 0], [255, 0, 0], [60, 90, 140]];
        let mut bgrx = Vec::new();
        for [r, g, b] in px {
            for _ in 0..2 {
                bgrx.extend_from_slice(&[b, g, r, 0]);
            }
        }
        let yuy2 = bgrx_to_yuy2(&bgrx, 8, 1);
        let back = yuy2_to_rgba(&yuy2, 8, 1, YuvColor { bt709: true, full_range: false });
        for (i, [r, g, b]) in px.iter().enumerate() {
            let o = i * 8;
            for (got, want) in [(back[o], *r), (back[o + 1], *g), (back[o + 2], *b)] {
                assert!((got as i32 - want as i32).abs() <= 3, "pixel {i}: {:?} vs {:?}", &back[o..o + 3], px[i]);
            }
        }
    }

    #[test]
    fn yuy2_limited_bt709_colours() {
        let c = YuvColor { bt709: true, full_range: false };
        // white, black, then BT.709 limited-range red (Y 63, U 102, V 240)
        let px = |y: u8, u: u8, v: u8| yuy2_to_rgba(&[y, u, y, v], 2, 1, c)[..4].to_vec();
        assert_eq!(px(235, 128, 128), [255, 255, 255, 255]);
        assert_eq!(px(16, 128, 128), [0, 0, 0, 255]);
        let r = px(63, 102, 240);
        assert!(r[0] > 245 && r[1] < 10 && r[2] < 10, "red came out as {r:?}");
    }
}

#[cfg(test)]
mod preview_size_tests {
    use super::*;

    #[test]
    fn the_4k_picture_lifts_the_preview_to_3840_wide() {
        let px = |w: u32, h: u32| vec![0u8; (w * h * 4) as usize];
        PREVIEW_MAX.store(PREVIEW_W, Ordering::Relaxed);
        let f = shrink(&px(3840, 2160), 3840, 2160, 3840 * 4, false);
        assert_eq!((f.w, f.h), (1920, 1080), "4K at half without it");
        PREVIEW_MAX.store(PREVIEW_W_4K, Ordering::Relaxed);
        let f = shrink(&px(3840, 2160), 3840, 2160, 3840 * 4, false);
        assert_eq!((f.w, f.h), (3840, 2160), "4K at full size with it");
        let f = shrink(&px(1920, 1080), 1920, 1080, 1920 * 4, false);
        assert_eq!((f.w, f.h), (1920, 1080), "1080p as it is");
        PREVIEW_MAX.store(PREVIEW_W, Ordering::Relaxed);
    }
}

#[cfg(all(test, target_os = "macos"))]
mod awake_tests {
    use super::keep_awake;

    #[test]
    fn idle_sleep_is_held_off_while_a_picture_comes_in() {
        let mut held = None;
        keep_awake(true, &mut held);
        let first = held.as_ref().expect("caffeinate starts").id();
        keep_awake(true, &mut held);
        assert_eq!(held.as_ref().unwrap().id(), first, "one is enough");
        // ended from outside (`pkill caffeinate`): started again
        let c = held.as_mut().unwrap();
        c.kill().unwrap();
        c.wait().unwrap();
        keep_awake(true, &mut held);
        assert_ne!(held.as_ref().unwrap().id(), first);
        keep_awake(false, &mut held);
        assert!(held.is_none(), "no picture: the Mac may sleep again");
    }
}
