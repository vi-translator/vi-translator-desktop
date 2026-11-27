//! GTA VI's phone read whole, once its screen has stopped moving: its picture, enlarged, is read in one go,
//! its lines are put together as the chat's messages, and each is translated as a line of the game's is
//! (engine.rs's `phone_whole`). The overlay draws them in a second phone put beside the game's once and left
//! there (ui/common.js's `renderPhoneRead`).
//! Read every frame, piece by piece, and drawn through its screen's corners (phone.rs), a long message in
//! small text came out as a word or two on a capture card's 720p picture, the second phone swayed with the
//! hand, and a white phone by day or a screen under a street lamp wasn't found at all.
//! Here: when to read (the screen still, never in the middle of a scroll), the picture to read, and its lines
//! put together.

use std::time::{Duration, Instant};

use crate::frame::FrameData;
use crate::ocr::Crop;

/// The screen's rows in this many bands.
const BANDS: usize = 96;
/// Two signatures a sixth of a second apart this far from each other: the screen moved. On 720p recordings: a
/// scroll's frames 20 to 44 apart; a still phone's 2 to 19, in a sunset's light too; walking with the phone up,
/// 10 to 18. VI_TRANSLATOR_PHONE_MOVED sets another, to tune it on another picture.
fn moved_from() -> f64 {
    std::env::var("VI_TRANSLATOR_PHONE_MOVED").ok().and_then(|v| v.parse().ok()).unwrap_or(20.0)
}
/// A screen with less detail than this is blurred (behind the pause menu: 0.00) or has no text (0.01): not
/// read. A chat's is 0.05 and up, 0.024 at the least in a scroll's frames.
pub const DETAIL_MIN: f64 = 0.018;

/// The screen's signature — how much of each band of its rows is dark (its text, its bubbles' edges), by
/// its own light — and its detail (the share of neighbouring pixels far apart: text is, a blur isn't).
/// Read between the screen's four corners (top left, top right, bottom right, bottom left; frame pixels:
/// its box's, `corners` — phone.rs's `screen_quad` finds a white phone's in one frame and not the next).
pub fn sig(rgba: &[u8], fw: u32, fh: u32, q: [(f64, f64); 4]) -> (Vec<u8>, f64) {
    const SUB: usize = 4;
    let wide = ((q[1].0 - q[0].0).hypot(q[1].1 - q[0].1) * 0.8) as usize;
    if wide < 16 || fw == 0 || fh == 0 {
        return (Vec::new(), 0.0);
    }
    let lum = |u: f64, v: f64| {
        let (tx, ty) = (q[0].0 + (q[1].0 - q[0].0) * u, q[0].1 + (q[1].1 - q[0].1) * u);
        let (bx, by) = (q[3].0 + (q[2].0 - q[3].0) * u, q[3].1 + (q[2].1 - q[3].1) * u);
        let (x, y) = ((tx + (bx - tx) * v).clamp(0.0, fw as f64 - 1.0) as u32, (ty + (by - ty) * v).clamp(0.0, fh as f64 - 1.0) as u32);
        let i = ((y * fw + x) * 4) as usize;
        ((rgba[i] as u32 * 3 + rgba[i + 1] as u32 * 6 + rgba[i + 2] as u32) / 10) as u8
    };
    // (the screen's middle: its edges are where the corners are a pixel or two off)
    let mut px = Vec::with_capacity(BANDS * SUB * wide);
    for r in 0..BANDS * SUB {
        let v = 0.04 + 0.92 * (r as f64 + 0.5) / (BANDS * SUB) as f64;
        px.extend((0..wide).map(|i| lum(0.1 + 0.8 * (i as f64 + 0.5) / wide as f64, v)));
    }
    // (dark: well under the bubbles' white — the lighter quarter of the screen — not under its mean. A chat's
    // patterned background, under some light (a sunset), sat right at four fifths of the mean: from one frame to
    // the next, a still phone's background went dark and light, and it was taken for one scrolling)
    let light = {
        let mut hist = [0usize; 256];
        px.iter().for_each(|&l| hist[l as usize] += 1);
        let (mut seen, want) = (0usize, px.len() / 4);
        (0..256usize).rev().find(|&l| {
            seen += hist[l];
            seen >= want
        }).unwrap_or(255) as u32
    };
    // (half of it: the text alone. VI_TRANSLATOR_PHONE_INK sets another share, in hundredths)
    let frac: u32 = std::env::var("VI_TRANSLATOR_PHONE_INK").ok().and_then(|v| v.parse().ok()).unwrap_or(50);
    let dark = light * frac / 100;
    // (far apart by the screen's own light too, a sixth of it — 40 by day: at night the same text's pixels are
    // half as far apart, and a dim phone was taken for a blurred one, with nothing shown for it)
    let far = (light / 6).max(8) as u8;
    let mut edges = 0usize;
    let sig = px
        .chunks_exact(SUB * wide)
        .map(|band| {
            edges += band.chunks_exact(wide).map(|row| row.windows(2).filter(|w| w[0].abs_diff(w[1]) > far).count()).sum::<usize>();
            let ink = band.iter().filter(|&&l| (l as u32) < dark).count();
            (ink * 255 * 3 / band.len()).min(255) as u8
        })
        .collect();
    (sig, edges as f64 / px.len() as f64)
}

/// A screen's box as its corners.
pub fn corners(b: (u32, u32, u32, u32)) -> [(f64, f64); 4] {
    let (x0, y0, x1, y1) = (b.0 as f64, b.1 as f64, (b.0 + b.2) as f64, (b.1 + b.3) as f64);
    [(x0, y0), (x1, y0), (x1, y1), (x0, y1)]
}

/// How far two signatures are apart: the mean difference of their bands, at the best of a band's shift
/// either way (the hand moves the phone a few pixels up and down).
pub fn apart(a: &[u8], b: &[u8]) -> f64 {
    if a.len() != b.len() || a.is_empty() {
        return f64::MAX;
    }
    (-1i32..=1)
        .map(|s| {
            let (mut sum, mut n) = (0u32, 0u32);
            for i in 0..a.len() as i32 {
                if let Some(v) = b.get((i + s) as usize).filter(|_| i + s >= 0) {
                    sum += a[i as usize].abs_diff(*v) as u32;
                    n += 1;
                }
            }
            sum as f64 / n.max(1) as f64
        })
        .fold(f64::MAX, f64::min)
}

/// When to read the phone: once the screen has been still for half a second — never in the middle of a
/// scroll — and then
/// - at once the first time, and after it moved (a scroll: what's shown is the screen before it), no sooner
///   than a second after the last reading;
/// - else again after a wait, to catch a change that went unseen: 2.5 s, twice as long each time the reading
///   is the same screen's, 10 s at most.
/// (Told apart by the picture alone, a scroll and the hand's sway were too alike to rely on: the white
/// phone's body runs into its screen, and the screen's box moves with every frame.)
pub struct Watch {
    seen: Vec<u8>,
    seen_at: Option<Instant>,
    still_from: Option<Instant>,
    asked_at: Option<Instant>,
    wait: Duration,
    moved: bool,
}

const WAIT: Duration = Duration::from_millis(2500);

impl Default for Watch {
    fn default() -> Self {
        Watch { seen: Vec::new(), seen_at: None, still_from: None, asked_at: None, wait: WAIT, moved: false }
    }
}

impl Watch {
    /// A frame's signature: is the phone to be read now? (`asked` then says it was.)
    pub fn due(&mut self, sig: Vec<u8>, now: Instant) -> bool {
        // (against the frame of a sixth of a second ago: from one frame to the next of sixty a second, a
        // scroll is a pixel or two)
        if self.seen_at.is_none_or(|t| now.saturating_duration_since(t) >= Duration::from_millis(150)) {
            if self.seen_at.is_some() && apart(&sig, &self.seen) >= moved_from() {
                self.still_from = Some(now);
                self.moved = true;
            }
            self.still_from.get_or_insert(now);
            (self.seen, self.seen_at) = (sig, Some(now));
        }
        let since = |t: Option<Instant>| t.map(|t| now.saturating_duration_since(t));
        let still = since(self.still_from).is_some_and(|d| d >= Duration::from_millis(500));
        still && since(self.asked_at).is_none_or(|d| d >= if self.moved { Duration::from_millis(1000) } else { self.wait })
    }
    /// The screen moved since it was read: what's shown may be the screen before (drawn faded).
    pub fn moved(&self) -> bool {
        self.moved
    }
    pub fn asked(&mut self, now: Instant) {
        self.asked_at = Some(now);
        self.moved = false;
    }
    /// The reading came: the same screen as the one shown, or another.
    pub fn answered(&mut self, same: bool) {
        self.wait = if same { (self.wait * 2).min(Duration::from_secs(10)) } else { WAIT };
    }
}

/// The phone's picture for the reader: the phone's box and a good deal round it (the box found is often the
/// screen's lower half, below its header bar: three quarters more above), two or three times as big when it's
/// small (a 720p picture's phone text is some 7 px tall: read far better enlarged); and what places its lines
/// when they come back.
pub struct Shot {
    /// the picture to read
    pub crop: Crop,
    /// the part of the frame it shows, as it was (RGB, `w` wide), and how many times bigger the picture is
    rgb: Vec<u8>,
    w: u32,
    k: u32,
    /// in the picture's pixels: the phone's box's sides, and how far down its text can be
    pub col: (f64, f64),
    pub bottom: f64,
}

impl Shot {
    /// How blue a line's bubble is (blue less red): the player's own messages are on light blue, the others' on
    /// white — whatever the light's colour. Measured between the line's letters — along it, the lighter third of
    /// what's there: beside the line is the bubble's edge, blue round the message picked.
    pub fn tint(&self, l: &Line) -> f64 {
        let h = (self.rgb.len() as u32 / 3 / self.w.max(1)) as i64;
        let k = self.k as f64;
        let mut v: Vec<(i32, i32)> = Vec::new();
        for i in 0..=24 {
            let t = i as f64 / 24.0;
            let (x, y) = (l.l[0] + (l.r[0] - l.l[0]) * t, l.l[1] + (l.r[1] - l.l[1]) * t);
            for off in [-0.3, 0.0, 0.3] {
                let (px, py) = (((x / k) as i64).clamp(0, self.w as i64 - 1), (((y + off * l.h) / k) as i64).clamp(0, h - 1));
                let p = &self.rgb[((py * self.w as i64 + px) * 3) as usize..][..3];
                v.push((p[0] as i32 * 3 + p[1] as i32 * 6 + p[2] as i32, p[2] as i32 - p[0] as i32));
            }
        }
        v.sort_unstable_by_key(|p| std::cmp::Reverse(p.0));
        let mut light: Vec<i32> = v[..v.len() / 3].iter().map(|p| p.1).collect();
        light.sort_unstable();
        light[light.len() / 2] as f64
    }

    /// The colour a line is written in ("#rrggbb"): a sender's name has its own — Moses purple, Jason green. Of
    /// the line's pixels, the tenth furthest from what it's written on (its lighter third), as they'd be on white:
    /// the light where the phone is held colours both (a street lamp's yellow), and letters this small are half
    /// their background.
    pub fn ink(&self, l: &Line) -> String {
        let h = (self.rgb.len() as u32 / 3 / self.w.max(1)) as i64;
        let k = self.k as f64;
        let n = (((l.r[0] - l.l[0]).hypot(l.r[1] - l.l[1]) / k) as usize).max(2);
        let rows = ((0.7 * l.h / k) as i64).max(1);
        let mut px: Vec<[i32; 3]> = Vec::new();
        for i in 0..=n {
            let t = i as f64 / n as f64;
            let (x, y) = ((l.l[0] + (l.r[0] - l.l[0]) * t) / k, (l.l[1] + (l.r[1] - l.l[1]) * t) / k);
            for dy in -rows / 2..=rows / 2 {
                let (px_, py) = ((x as i64).clamp(0, self.w as i64 - 1), (y as i64 + dy).clamp(0, h - 1));
                let p = &self.rgb[((py * self.w as i64 + px_) * 3) as usize..][..3];
                px.push([p[0] as i32, p[1] as i32, p[2] as i32]);
            }
        }
        let mean = |v: &[[i32; 3]]| [0, 1, 2].map(|c| v.iter().map(|p| p[c]).sum::<i32>() / v.len().max(1) as i32);
        px.sort_unstable_by_key(|p| std::cmp::Reverse(p[0] * 3 + p[1] * 6 + p[2]));
        let bg = mean(&px[..(px.len() / 3).max(1)]);
        px.sort_unstable_by_key(|p| std::cmp::Reverse((0..3).map(|c| (p[c] - bg[c]).abs()).sum::<i32>()));
        let ink = mean(&px[..(px.len() / 10).max(1)]);
        // (as on white: each colour by how much of its background's it is; then a little further from white — the
        // letters' edges are part background. VI_TRANSLATOR_NAME_INK sets how much, to tune it on another picture)
        let far = std::env::var("VI_TRANSLATOR_NAME_INK").ok().and_then(|v| v.parse().ok()).unwrap_or(1.25);
        let c = [0, 1, 2].map(|c| {
            let on_white = ink[c] as f64 / bg[c].max(1) as f64;
            ((1.0 - far * (1.0 - on_white)).clamp(0.0, 1.0) * 255.0) as u8
        });
        format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
    }
}

pub fn shot(f: &FrameData, scr: (u32, u32, u32, u32)) -> Option<Shot> {
    let (x0, y0) = (scr.0.saturating_sub(scr.2 * 20 / 100), scr.1.saturating_sub(scr.3 * 75 / 100));
    let (x1, y1) = ((scr.0 + scr.2 * 120 / 100).min(f.w), (scr.1 + scr.3 * 115 / 100).min(f.h));
    let (w, h) = (x1.checked_sub(x0)?, y1.checked_sub(y0)?);
    let mut rgb = Vec::with_capacity((w * h * 3) as usize);
    for y in y0..y1 {
        let row = &f.rgba[((y * f.w + x0) * 4) as usize..((y * f.w + x1) * 4) as usize];
        rgb.extend(row.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]));
    }
    let img = image::RgbImage::from_raw(w, h, rgb.clone())?;
    let k = ((1600 + h / 2) / h.max(1)).clamp(1, 3);
    let img = if k > 1 { image::imageops::resize(&img, w * k, h * k, image::imageops::FilterType::CatmullRom) } else { img };
    let at = |v: u32, from: u32| (v.saturating_sub(from) * k) as f64;
    Some(Shot {
        crop: Crop { w: w * k, h: h * k, rgba: img.pixels().flat_map(|p| [p[0], p[1], p[2], 255]).collect() },
        rgb,
        w,
        k,
        col: (at(scr.0, x0), at(scr.0 + scr.2, x0)),
        bottom: at(scr.1 + scr.3, y0) + 0.03 * (scr.3 * k) as f64,
    })
}

/// A line of text read on the phone's picture, as it lies there (tilted as the phone is): its words, the
/// middle of its left and right ends and its text's height — the picture's pixels, from its top left.
pub use crate::ocr::TextLine as Line;

/// A piece of the phone's screen, as the overlay draws it (ui/common.js's renderPhoneRead): a message
/// ("msg", the player's own "me"), an attachment's name ("card"), a centred note ("label"); who it's from,
/// when its name is over it, and the colour the name is written in; its English.
#[derive(Debug, PartialEq)]
pub struct Item {
    pub k: &'static str,
    pub from: String,
    pub color: String,
    pub en: String,
}

/// The phone's screen from a reading's lines: its title (the chat's name, in the bar at its top) and its pieces,
/// top to bottom. `col`, `bottom`: where the phone is on the picture (lines beside or below it are the game's own:
/// a subtitle); `tint`: how blue a line's bubble is (`Shot::tint`); `ink`: the colour a line is written in
/// (`Shot::ink`).
pub fn page(mut lines: Vec<Line>, col: (f64, f64), bottom: f64, tint: &dyn Fn(&Line) -> f64, ink: &dyn Fn(&Line) -> String) -> (String, Vec<Item>) {
    let slack = 0.12 * (col.1 - col.0);
    lines.retain(|l| l.t.trim().chars().count() >= 2 && l.l[0] >= col.0 - slack && l.r[0] <= col.1 + slack && l.l[1] <= bottom);
    lines.sort_by(|a, b| a.l[1].total_cmp(&b.l[1]));
    // (a picture before a line — a sender's, an attachment's icon — is read as a mark with it, "• Moses": one
    // mark for them all, "•", told from the line's words below)
    for l in lines.iter_mut() {
        if let Some((mark, rest)) = l.t.trim().split_once(' ') {
            if !mark.chars().any(|c| c.is_alphanumeric() || "?!.…".contains(c)) {
                l.t = format!("• {}", rest.trim());
            }
        }
    }
    let bare = |t: &str| t.trim().trim_start_matches('•').trim().to_string();
    if lines.is_empty() {
        return (String::new(), Vec::new());
    }
    let mut hs: Vec<f64> = lines.iter().map(|l| l.h).collect();
    hs.sort_by(f64::total_cmp);
    let h = hs[hs.len() / 2].max(1.0);
    // (a message's lines are 1.2 to 1.4 of their height apart — a sender's name over it too; the next bubble, 2.3 up)
    let mut paras: Vec<Vec<Line>> = Vec::new();
    for l in lines {
        match paras.last_mut() {
            Some(p) if l.l[1] - p.last().unwrap().l[1] <= 1.8 * h => p.push(l),
            _ => paras.push(vec![l]),
        }
    }
    let words = |t: &str| t.split_whitespace().count();
    // (not a sentence's end: no stop — or a name cut short, "Moses Watkins, Ja...", read as ".…." too)
    let cut = |t: &str| t.trim_end().chars().rev().take_while(|c| ['.', '…'].contains(c)).map(|c| if c == '…' { 3 } else { 1 }).sum::<usize>() >= 2;
    let open_end = |t: &str| !t.trim_end().ends_with(['.', '?', '!', ',', ';', ':', '…']) || cut(t);
    let middle = |l: &Line| (l.l[0] + l.r[0]) / 2.0;
    // (the least blue bubble is a white one: the player's own are clearly bluer than it)
    let white = paras.iter().flatten().map(tint).fold(f64::MAX, f64::min);
    let mut title = String::new();
    let mut items: Vec<Item> = Vec::new();
    let mut margin = col.0;
    // the bar at the top: the chat's name, a few words with no sentence's end ("Mia", "Moses Watkins, Jason, L..." —
    // its back arrow read with it, "‹ Moses"); a word or three under its middle ("3 Members"), a note. The rest of
    // what's near it is the first message, scrolled up to the bar.
    let name = paras[0][0].t.trim().trim_start_matches(['‹', '<', '(', '•', ' ']).to_string();
    if words(&name) <= 5 && open_end(&name) && !name.starts_with(|c: char| c.is_ascii_digit()) {
        let bar = paras[0].remove(0);
        title = if cut(&name) { format!("{}…", name.trim_end_matches(['.', '…'])) } else { name };
        while paras[0].first().is_some_and(|l| words(&l.t) <= 3 && open_end(&l.t) && (middle(l) - middle(&bar)).abs() < 1.5 * h) {
            items.push(Item { k: "label", from: String::new(), color: String::new(), en: paras[0].remove(0).t.trim().to_string() });
        }
    }
    // (a line cut by the bar at the screen's top, or by its bottom edge: half of it is there, read as letters that
    // aren't its own — "new account is 10 @ 10v,00v." for "…new account limit is $100,000." — and each reading
    // reads it differently. It's shorter than the others: 18 px to their 28. Left out)
    let cut_off = |l: &Line| l.h < 0.75 * h;
    if let Some(p) = paras.iter_mut().find(|p| !p.is_empty()) {
        if cut_off(&p[0]) {
            p.remove(0);
        }
    }
    if let Some(p) = paras.iter_mut().rev().find(|p| !p.is_empty()) {
        if p.last().is_some_and(cut_off) {
            p.pop();
        }
    }
    for mut p in paras.into_iter().filter(|p| !p.is_empty()) {
        // (its sender's name over a message: a word or three, starting further in than the message — its picture
        // before it, or read with it as a mark)
        let first = bare(&p[0].t);
        let (from, color) = if p.len() >= 2 && words(&first) <= 3 && open_end(&first) && (p[0].t.starts_with('•') || p[0].l[0] - p[1].l[0] > 0.8 * h) {
            let mut name = p.remove(0);
            // (its colour off its letters, not off the picture read with it: past the mark's share of the line)
            if name.t.starts_with('•') {
                let past = 2.0 / name.t.chars().count().max(3) as f64;
                name.l = [name.l[0] + (name.r[0] - name.l[0]) * past, name.l[1] + (name.r[1] - name.l[1]) * past];
            }
            (first, ink(&name))
        } else {
            (String::new(), String::new())
        };
        // (an attachment — a place's pin, a contact: one short line of names, further in than the message over it,
        // or its icon read with it: a mark, or a word in brackets, "(MC) Los Bollos Deli")
        let one = p[0].t.trim();
        let (icon, name) = match one.split_once(' ') {
            Some(("•", b)) => (true, b.trim()),
            Some((a, b)) if a.ends_with([')', ']']) && a.len() <= 4 => (true, b.trim()),
            _ => (false, one),
        };
        let names = words(name) <= 4 && open_end(name) && name.split_whitespace().all(|w| w.starts_with(|c: char| c.is_uppercase() || c.is_ascii_digit()));
        let k = if from.is_empty() && p.len() == 1 && names && (icon || p[0].l[0] - margin > 0.8 * h) {
            "card"
        } else if tint(&p[0]) - white > 14.0 {
            "me"
        } else {
            "msg"
        };
        if k != "card" {
            margin = p.last().unwrap().l[0];
        }
        let en = if k == "card" { name.to_string() } else { p.iter().map(|l| bare(&l.t)).collect::<Vec<_>>().join(" ") };
        items.push(Item { k, from, color, en });
    }
    (title, items)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dim_screen_s_text_is_detail_as_a_bright_one_s_is() {
        // (a screen of text: a dark pixel in every eight, on white; its light as by day, and as at night)
        let detail = |white: u8, text: u8| {
            let (w, h) = (200u32, 400u32);
            let rgba: Vec<u8> = (0..w * h).flat_map(|i| [if i % w % 8 == 0 { text } else { white }; 4]).collect();
            sig(&rgba, w, h, corners((0, 0, w, h))).1
        };
        assert!(detail(240, 60) >= DETAIL_MIN && detail(100, 70) >= DETAIL_MIN, "day {} night {}", detail(240, 60), detail(100, 70));
        // (a blur's pixels are a level or two apart, whatever its light)
        assert!(detail(100, 97) < DETAIL_MIN && detail(240, 237) < DETAIL_MIN);
    }

    #[test]
    fn read_once_a_screen_is_still_and_again_only_when_it_changes() {
        let (a, b): (Vec<u8>, Vec<u8>) = ((0..96).map(|i| if i % 3 == 0 { 0 } else { 120 }).collect(), (0..96).map(|i| if i % 4 == 0 { 0 } else { 60 }).collect());
        // (the hand's sway: a few bands a little different)
        let a2: Vec<u8> = a.iter().enumerate().map(|(i, v)| if i % 10 == 0 { v.saturating_add(20) } else { *v }).collect();
        assert!(apart(&a, &a2) < moved_from() && apart(&a, &b) > moved_from());
        let t = Instant::now();
        let at = |ms: u64| t + Duration::from_millis(ms);
        let mut w = Watch::default();
        assert!(!w.due(a.clone(), at(0)), "just seen: not still yet");
        assert!(!w.due(a2.clone(), at(300)), "not still long enough");
        assert!(w.due(a2.clone(), at(600)), "still: read");
        w.asked(at(600));
        w.answered(false);
        assert!(!w.due(a.clone(), at(2000)), "the same screen, swaying: not read again so soon");
        assert!(w.due(a.clone(), at(3200)), "read again after the wait, in case a change went unseen");
        w.asked(at(3200));
        w.answered(true);
        assert!(!w.due(a.clone(), at(6000)), "the same screen's reading: the wait twice as long");
        assert!(!w.due(b.clone(), at(6200)) && w.moved(), "scrolled: not still yet");
        assert!(!w.due(b.clone(), at(6500)));
        assert!(w.due(b.clone(), at(6800)), "the new screen, still: read at once");
        w.asked(at(6800));
        assert!(!w.moved() && !w.due(b, at(7000)));
    }

    /// A reader that gives words, each with its own box (Windows', Tesseract): a tilted message's lines run
    /// from their first word to their last, as tall as their words — and are put together as one message, the
    /// next bubble as another.
    #[test]
    fn words_with_their_own_boxes_give_the_lines_as_they_lie() {
        use crate::ocr::RawWord;
        // (the chat's name in its bar; then each line 60 px under the last and running 6 px down a word; the next
        // bubble 140 px further down)
        let texts = [(-200.0, "Mia"), (0.0, "Hey Mo, it's Lucia and"), (60.0, "Jason here, had to get out"), (120.0, "of PG."), (260.0, "Thanks man, all good.")];
        let words: Vec<RawWord> = texts
            .iter()
            .enumerate()
            .flat_map(|(n, (y, t))| t.split(' ').enumerate().map(move |(i, w)| RawWord { line: n as u32, text: w.into(), x: 300.0 + 90.0 * i as f64, y: 400.0 + y + 6.0 * i as f64, w: 80.0, h: 46.0 }).collect::<Vec<_>>())
            .collect();
        let lines = crate::ocr::lines_of(words);
        assert_eq!(lines.len(), 5);
        assert_eq!((lines[2].l, lines[2].r, lines[2].h), ([300.0, 483.0], [830.0, 513.0], 46.0), "from its first word's middle to its last one's");
        let (title, items) = page(lines, (250.0, 1000.0), 1500.0, &|_| 4.0, &|_| String::new());
        assert_eq!(title, "Mia");
        let got: Vec<&str> = items.iter().map(|i| i.en.as_str()).collect();
        assert_eq!(got, ["Hey Mo, it's Lucia and Jason here, had to get out of PG.", "Thanks man, all good."]);
    }

    /// A chat as Apple's reader read it on a 720p picture's phone held tilted (its lines run down to the right):
    /// its title, who each message is from, the player's own, an attachment.
    #[test]
    fn a_chat_s_lines_are_its_title_and_messages() {
        let line = |t: &str, l: [f64; 2], r: [f64; 2], h: f64| Line { t: t.into(), l, r, h };
        let lines = vec![
            line("Moses Watkins, Jason, L...", [470.0, -22.0], [900.0, 3.0], 44.0),
            line("3 Members", [594.0, 28.0], [823.0, 45.0], 42.0),
            line("I'm sure you know the rest.", [418.0, 111.0], [969.0, 148.0], 45.0),
            line("Buckskin Swamp", [504.0, 226.0], [857.0, 258.0], 45.0),
            line("Lucia", [451.0, 393.0], [576.0, 402.0], 38.0),
            line("Hey Mo, it's Lucia and", [365.0, 464.0], [804.0, 497.0], 52.0),
            line("Jason here, had to get out", [349.0, 525.0], [892.0, 570.0], 54.0),
            line("of PG. Do you know anyone", [344.0, 579.0], [904.0, 635.0], 50.0),
            line("who can handle cars in", [335.0, 637.0], [795.0, 680.0], 41.0),
            line("Vice?", [324.0, 701.0], [438.0, 711.0], 52.0),
            line("Thanks man.", [620.0, 870.0], [870.0, 890.0], 46.0),
            // (a sender's picture and an attachment's icon read as a mark with their line)
            line("• Moses", [300.0, 1010.0], [470.0, 1022.0], 40.0),
            line("All good.", [296.0, 1068.0], [470.0, 1080.0], 50.0),
            line("@ Map Pin Expired", [300.0, 1190.0], [640.0, 1214.0], 50.0),
            // (the game's own, beside the phone: not the phone's)
            line("SCROLL", [1100.0, 900.0], [1140.0, 900.0], 30.0),
            // (the last line, cut by the screen's bottom edge: half as tall, read as other letters)
            line("Hey ho, wo ane", [300.0, 1290.0], [520.0, 1300.0], 24.0),
        ];
        // (the player's own message on blue, the rest on white)
        let tint = |l: &Line| if l.t.starts_with("Thanks") { 30.0 } else { 4.0 };
        let (title, items) = page(lines, (250.0, 950.0), 1500.0, &tint, &|_| "#5c8259".to_string());
        assert_eq!(title, "Moses Watkins, Jason, L…");
        let got: Vec<(&str, &str, &str)> = items.iter().map(|i| (i.k, i.from.as_str(), i.en.as_str())).collect();
        assert_eq!(
            got,
            [
                ("label", "", "3 Members"),
                ("msg", "", "I'm sure you know the rest."),
                ("card", "", "Buckskin Swamp"),
                ("msg", "Lucia", "Hey Mo, it's Lucia and Jason here, had to get out of PG. Do you know anyone who can handle cars in Vice?"),
                ("me", "", "Thanks man."),
                ("msg", "Moses", "All good."),
                ("card", "", "Map Pin Expired"),
            ]
        );
        assert_eq!(items[3].color, "#5c8259", "the sender's name in its own colour");
    }

    /// Run on a folder of frames (JPEGs in order) to see what `sig` tells apart, to tune the limits:
    /// VI_PHONE_FRAMES=<folder> cargo test -p vi-translator-core phone_read::tests::frames -- --nocapture
    #[test]
    fn frames() {
        let Ok(dir) = std::env::var("VI_PHONE_FRAMES") else { return };
        let mut names: Vec<_> = std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().is_some_and(|e| e == "jpg")).collect();
        names.sort();
        let mut prev: Vec<u8> = Vec::new();
        for p in names {
            let img = image::open(&p).unwrap().into_rgba8();
            let f = FrameData { w: img.width(), h: img.height(), rgba: img.into_raw(), seq: 0 };
            match crate::phone::find_phone(&f.rgba, f.w, f.h) {
                Some(scr) => {
                    let (s, detail) = sig(&f.rgba, f.w, f.h, corners(scr));
                    println!("{} scr {:?} detail {:.3} apart {:.1}", p.file_name().unwrap().to_string_lossy(), scr, detail, if prev.is_empty() { 0.0 } else { apart(&s, &prev) });
                    prev = s;
                }
                None => println!("{} no screen", p.file_name().unwrap().to_string_lossy()),
            }
        }
    }
}
