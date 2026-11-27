//! GTA VI's phone, held up in the player's hand: its screen found in the frame wherever it is (the
//! phone moves and tilts with the hand), its text read in pieces (a message, a caption, a button), each
//! translated and drawn in a column beside the phone, level with its piece (the WhatUp and Snapmatic
//! apps: chats, messages, posts).

use crate::ocr::{self, Crop, Ocr, Word};

/// A piece of the phone's text: where it is (frame pixels: x, y, w, h) and what it says.
#[derive(Clone, Debug, PartialEq)]
pub struct Piece {
    pub rect: (u32, u32, u32, u32),
    pub en: String,
    /// A heading (the chat's contact name, an app's title): drawn as a title, not in a bubble.
    pub title: bool,
    /// A chat list's item: the contact's name, its own line above the message's start (`en`).
    pub name: Option<String>,
    /// How it looks on the phone (measured in the frame: see `look_of`).
    pub look: Look,
    /// Its first line and the rest, when it has more than one (a chat list's row: the contact, the message).
    pub split: Option<(String, String)>,
    /// A chat list's row: the contact's round picture at its start (x, y offset from `rect`'s top left,
    /// and its side; frame pixels).
    pub avatar: Option<(i32, i32, u32)>,
    /// A chat list's row whose message is no text (an emoji): where it is, to show as it is (x, y
    /// offset from `rect`'s top left, w, h; frame pixels).
    pub preview: Option<(i32, i32, u32, u32)>,
    /// A chat list's row's mark at its end (a pin, a reply arrow): where it is, as `preview`.
    pub badge: Option<(i32, i32, u32, u32)>,
    /// The emojis by its text (frame pixels, in reading order; see chat::emoji_in).
    pub emoji: Vec<(u32, u32, u32, u32)>,
    /// A chat message's bubble as it is on the phone, round its text (frame pixels): where its colour gives way to
    /// the screen's (see bubble_box). None for a header, a list's row, or a bubble its edges weren't found for.
    pub bubble: Option<(u32, u32, u32, u32)>,
}

/// A piece's look on the phone: its bubble's colour and its text's (a message from the other person:
/// white, dark text; one the character sent: light blue; a reply to pick: dark blue, white text), or
/// no bubble (`label`: a header, "Suggested", "New message": text on the screen's own background);
/// `end`: on the screen's right side (the character's own messages).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Look {
    pub bg: [u8; 3],
    pub fg: [u8; 3],
    pub label: bool,
    pub end: bool,
    /// A coloured outline round it (a reply to pick: the chosen one's, a suggestion's), when it has one.
    pub edge: Option<[u8; 3]>,
    /// A card with a border round it (a chat's "Map Pin Unavailable", "Map Pin Expired").
    pub boxed: bool,
}

fn px(rgba: &[u8], fw: u32, x: u32, y: u32) -> [u8; 3] {
    let i = ((y * fw + x) * 4) as usize;
    [rgba[i], rgba[i + 1], rgba[i + 2]]
}

/// Each channel's median.
fn median(v: &mut [[u8; 3]]) -> [u8; 3] {
    if v.is_empty() {
        return [0, 0, 0];
    }
    let mut out = [0u8; 3];
    for (c, o) in out.iter_mut().enumerate() {
        v.sort_by_key(|p| p[c]);
        *o = v[v.len() / 2][c];
    }
    out
}

fn dist(a: [u8; 3], b: [u8; 3]) -> i32 {
    (0..3).map(|c| (a[c] as i32 - b[c] as i32).abs()).max().unwrap_or(0)
}

/// The screen's own background colour (beside its bubbles, along its left edge, from `y0` to `y1`).
pub fn screen_bg(rgba: &[u8], fw: u32, fh: u32, screen: (u32, u32, u32, u32), y0: u32, y1: u32) -> [u8; 3] {
    let (sx, sy, sw, sh) = screen;
    let (a, b) = (y0.max(sy), y1.min(sy + sh).min(fh - 1));
    // (on each row, a little in from where the screen's light starts: held tilted, walking, the box's left
    // edge is the ground beside the phone)
    let light = |p: [u8; 3]| {
        let (lo, hi) = (*p.iter().min().unwrap(), *p.iter().max().unwrap());
        lo >= 90 && hi - lo <= 45
    };
    let mut v: Vec<[u8; 3]> = Vec::new();
    for y in (a..b.max(a + 1)).step_by(2) {
        let y = y.min(fh - 1);
        if let Some(x) = (sx..(sx + sw / 3).min(fw)).find(|&x| light(px(rgba, fw, x, y))) {
            let p = px(rgba, fw, (x + 6).min(fw - 1), y);
            if light(p) {
                v.push(p);
            }
        }
    }
    if v.is_empty() {
        let x = (sx + 6).min(fw - 1);
        v = (a..b.max(a + 1)).step_by(2).map(|y| px(rgba, fw, x, y.min(fh - 1))).collect();
    }
    median(&mut v)
}

/// The character's own message, white on a bright blue: much of the box round its words is that blue
/// (measured inside the words' box: held tilted, the margin round it is partly off the bubble).
fn own_blue(rgba: &[u8], fw: u32, r: (u32, u32, u32, u32)) -> bool {
    let (x, y, w, h) = r;
    let (mut n, mut blue) = (0u32, 0u32);
    for yy in (y..y + h).step_by(2) {
        for xx in (x..(x + w).min(fw)).step_by(2) {
            n += 1;
            let p = px(rgba, fw, xx, yy);
            if p[2] as i32 - p[0] as i32 > 100 && p[2] > 140 {
                blue += 1;
            }
        }
    }
    n > 0 && blue * 10 >= n * 3
}

/// A piece's look, measured in the frame: its bubble's colour (around its text, a margin out), its
/// text's (the pixels in it furthest from the bubble's), whether it has a bubble at all (the colour
/// around it is the screen's own), and its side (centred right of the screen's middle).
pub fn look_of(rgba: &[u8], fw: u32, fh: u32, r: (u32, u32, u32, u32), screen: (u32, u32, u32, u32)) -> Look {
    let (x, y, w, h) = r;
    let m = (h / 3).clamp(4, 10);
    let (x0, y0, x1, y1) = (x.saturating_sub(m), y.saturating_sub(m), (x + w + m).min(fw - 1), (y + h + m).min(fh - 1));
    let mut ring = Vec::new();
    for yy in (y0..=y1).step_by(2) {
        for xx in (x0..=x1).step_by(2) {
            if xx < x || xx >= x + w || yy < y || yy >= y + h {
                ring.push(px(rgba, fw, xx, yy));
            }
        }
    }
    let bg = median(&mut ring);
    let mut ink: Vec<[u8; 3]> = Vec::new();
    for yy in y..(y + h).min(fh) {
        for xx in (x..(x + w).min(fw)).step_by(2) {
            let p = px(rgba, fw, xx, yy);
            if dist(p, bg) > 70 {
                ink.push(p);
            }
        }
    }
    // (the text's solid middle, not its soft edges: the third of its pixels furthest from the bubble)
    ink.sort_by_key(|p| std::cmp::Reverse(dist(*p, bg)));
    ink.truncate((ink.len() / 3).max(1));
    let fg = if ink.is_empty() { if bg.iter().map(|&c| c as u32).sum::<u32>() > 380 { [40, 42, 50] } else { [255, 255, 255] } } else { median(&mut ink) };
    let sbg = screen_bg(rgba, fw, fh, screen, y, y + h);
    let (sx, _, sw, _) = screen;
    // (a white bubble in sunset shade is only some 10 brighter than the screen around it: a label is within 8)
    let edge = edge_of(rgba, fw, fh, r, bg, m);
    // (a white box with a blue outline is a reply to pick, not text on the screen)
    // (a label stands clear of both sides, "New message" in the middle; a message in a bubble as light as
    // the screen, in the sun, starts near one side)
    // (and short, in the middle: a tilted phone's box is wider than its screen, a bubble looks clear of it)
    let mid = (x + w / 2) as i32 - (sx + sw / 2) as i32;
    let clear = x > sx + sw * 15 / 100 && x + w + sw * 15 / 100 < sx + sw && w * 100 < sw * 45 && mid.unsigned_abs() * 100 < sw * 4;
    // (the character's own messages, a bright blue: on the right, the reply to pick too)
    let own = bg[2] as i32 - bg[0] as i32 > 100 || own_blue(rgba, fw, r);
    Look { bg, fg, label: dist(bg, sbg) < 8 && edge.is_none() && clear, end: own || (x + w / 2 > sx + sw * 58 / 100 && x > sx + sw / 5), edge, boxed: false }
}

/// A blue outline round the text's bubble (a reply to pick: a suggestion's, the chosen one's).
/// Looking out from the text's middle (left, right, up, down), past the bubble's padding: on a bubble not
/// blue itself, the first pixels unlike it are blue; on a blue one (the chosen reply), a light gap then
/// blue (a blue bubble with no outline just fades into the screen). One when 3 of the 4 find one.
fn edge_of(rgba: &[u8], fw: u32, fh: u32, r: (u32, u32, u32, u32), bg: [u8; 3], m: u32) -> Option<[u8; 3]> {
    let (x, y, w, h) = r;
    let (cx, cy) = (x + w / 2, y + h / 2);
    let reach = (m * 4 + 6) as i32;
    let blue = |p: [u8; 3]| p[2] as i32 - p[0] as i32 > 45 && p[2] as i32 > p[1] as i32 + 20;
    let light = |p: [u8; 3]| *p.iter().min().unwrap() > 140 && !blue(p);
    let bubble_blue = blue(bg);
    let mut found: Vec<[u8; 3]> = Vec::new();
    let rays: [(i32, i32, i32, i32); 4] = [(x as i32 - 1, cy as i32, -1, 0), ((x + w) as i32, cy as i32, 1, 0), (cx as i32, y as i32 - 1, 0, -1), (cx as i32, (y + h) as i32, 0, 1)];
    for (x0, y0, dx, dy) in rays {
        let at = |k: i32| {
            let (xx, yy) = (x0 + dx * k, y0 + dy * k);
            (xx >= 0 && yy >= 0 && xx < fw as i32 && yy < fh as i32).then(|| px(rgba, fw, xx as u32, yy as u32))
        };
        let Some(k) = (0..reach).find(|&k| at(k).is_some_and(|p| dist(p, bg) > 40)) else { continue };
        let next: Vec<[u8; 3]> = (k..k + 6).filter_map(at).collect();
        let hit = if bubble_blue {
            next.iter().position(|&p| light(p)).and_then(|g| next[g..].iter().find(|&&p| blue(p) && dist(p, bg) > 30).copied())
        } else {
            next.iter().take(3).find(|&&p| blue(p)).copied()
        };
        found.extend(hit);
    }
    (found.len() >= 3).then(|| median(&mut found))
}

/// The size the frame is looked at to find the screen (one cell = SCALE x SCALE pixels).
const SCALE: u32 = 6;

/// The phone's screen in the frame (x, y, w, h, frame pixels), when one is up: the largest bright,
/// colourless region shaped like a phone held upright (as wide as an eighth to nearly half a 1080p
/// picture, at least as tall as it is wide), filled for the most part. Its photos (Snapmatic) are not
/// bright: the screen is then its bright parts above and below, in the same column, together.
pub fn find_screen(rgba: &[u8], fw: u32, fh: u32) -> Option<(u32, u32, u32, u32)> {
    // (a white screen in sunset shade is about 200; at night, in a dark car, 150 down to 80: the brighter
    // first, the dimmer only when no phone is found so)
    let ts = [175, 150, 130, 115, 100, 88, 75];
    let (i, first) = ts.iter().enumerate().find_map(|(i, &t)| find_screen_at(rgba, fw, fh, t, true).map(|r| (i, r)))?;
    // (and as much of it as a dimmer light still finds, round what was found: at night the screen's top is a
    // little dimmer than the rest, and some frames found only its lower part — the panel jumped, its top rows
    // came and went. The dimmer one holding the first, no more than half as big again)
    let area = |r: (u32, u32, u32, u32)| r.2 as u64 * r.3 as u64;
    let holds = |big: (u32, u32, u32, u32), small: (u32, u32, u32, u32)| {
        let (x0, y0) = (big.0.max(small.0), big.1.max(small.1));
        let (x1, y1) = ((big.0 + big.2).min(small.0 + small.2), (big.1 + big.3).min(small.1 + small.3));
        x1 > x0 && y1 > y0 && (x1 - x0) as u64 * (y1 - y0) as u64 * 10 >= area(small) * 9
    };
    let mut best = first;
    for &t in &ts[i + 1..] {
        if let Some(r) = find_screen_at(rgba, fw, fh, t, true) {
            if holds(r, best) && area(r) > area(best) && area(r) * 2 <= area(first) * 3 {
                best = r;
            }
        }
    }
    Some(best)
}

/// The screen's place made steady from one frame to the next: one found inside the last one (held
/// within a second), less of it found this time, is the last one (the panel didn't jump, its rows
/// didn't go); a different or bigger one is taken as found.
pub fn steady_screen(last: &mut Option<((u32, u32, u32, u32), std::time::Instant)>, found: (u32, u32, u32, u32), now: std::time::Instant) -> (u32, u32, u32, u32) {
    if let Some((prev, at)) = *last {
        let (x0, y0) = (prev.0.max(found.0), prev.1.max(found.1));
        let (x1, y1) = ((prev.0 + prev.2).min(found.0 + found.2), (prev.1 + prev.3).min(found.1 + found.3));
        let inside = x1 > x0 && y1 > y0 && (x1 - x0) as u64 * (y1 - y0) as u64 * 10 >= found.2 as u64 * found.3 as u64 * 9;
        let smaller = (found.2 as u64 * found.3 as u64) < prev.2 as u64 * prev.3 as u64;
        // (held a moment from when it was last really found, not from when it was last held: renewed each frame
        // the held box never let go — one frame that took the white phone's body in with its screen, and the
        // screen stayed that big, its corners unfound and the overlay's phone bigger with it)
        if inside && smaller && now.saturating_duration_since(at) < std::time::Duration::from_millis(1200) {
            return prev;
        }
    }
    *last = Some((found, now));
    found
}

/// A light part of a screen too short to be sure of by its shape alone: a Snapmatic post's caption
/// under its photo (the photo fills the screen's top). Only a phone when its buttons are up too
/// (see `phone_buttons`).
pub fn find_screen_unsure(rgba: &[u8], fw: u32, fh: u32) -> Option<(u32, u32, u32, u32)> {
    find_screen_at(rgba, fw, fh, 175, false)
}

/// The phone's own buttons at the bottom right ("VIEW COMMENTS", "OPTIONS", "SCROLL", "SEND", or
/// "SELECT" with "BACK"), read: what tells a phone's short light part from a light wall.
pub fn phone_buttons(buttons: &[((u32, u32, u32, u32), String)]) -> bool {
    let has = |w: &str| buttons.iter().any(|(_, t)| t == w);
    ["VIEW COMMENTS", "OPTIONS", "SCROLL", "SEND"].iter().any(|w| has(w)) || (has("SELECT") && has("BACK"))
}

fn find_screen_at(rgba: &[u8], fw: u32, fh: u32, light_from: i32, sure: bool) -> Option<(u32, u32, u32, u32)> {
    find_screen_as(rgba, fw, fh, light_from, sure, false, 30)
}

/// The phone for a reader that needs no exact screen (phone_read.rs sends its picture to a model): the screen
/// as `find_screen` finds it, or else
/// - the white phone whole — by day its body is as light as its screen, the two are one light region, wider
///   than a screen may be and less tall for its width, and no screen was found (`body`: to 55 % of the
///   picture's height wide, 1.1 times as tall as wide at the least);
/// - a screen coloured by the light it's in — under a street lamp, at sunset, its white is (185, 162, 128),
///   55 between its most and least colour: no "colourless" light, and no screen was found (`tint`: 75 apart
///   at the most).
/// - a screen dimmer still — in a dark room, a night club's light, its least colour 67 (half the picture's
///   light, 55 %), 54 at 45 %: none of the above finds it. Last, when nothing else is a phone: the reader also
///   wants the phone's buttons on the picture (engine.rs, `phone_whole`), a dim wall alone shows nothing.
pub fn find_phone(rgba: &[u8], fw: u32, fh: u32) -> Option<(u32, u32, u32, u32)> {
    find_screen(rgba, fw, fh)
        .or_else(|| [175, 150, 130].iter().find_map(|&t| find_screen_as(rgba, fw, fh, t, true, true, 30)))
        .or_else(|| [140, 105, 80].iter().find_map(|&t| find_screen_as(rgba, fw, fh, t, true, true, 75)))
        .or_else(|| [65, 52].iter().find_map(|&t| find_screen_as(rgba, fw, fh, t, true, false, 75)))
}

fn find_screen_as(rgba: &[u8], fw: u32, fh: u32, light_from: i32, sure: bool, body: bool, tint: i32) -> Option<(u32, u32, u32, u32)> {
    let (cw, ch) = (fw / SCALE, fh / SCALE);
    if cw < 16 || ch < 16 {
        return None;
    }
    // a cell is bright when most of its sampled pixels are near-white and colourless
    let mut light = vec![false; (cw * ch) as usize];
    for cy in 0..ch {
        for cx in 0..cw {
            let mut n = 0;
            for (dx, dy) in [(1, 1), (4, 1), (1, 4), (4, 4), (2, 2)] {
                let (x, y) = (cx * SCALE + dx, cy * SCALE + dy);
                let i = ((y * fw + x) * 4) as usize;
                let (r, g, b) = (rgba[i] as i32, rgba[i + 1] as i32, rgba[i + 2] as i32);
                let (lo, hi) = (r.min(g).min(b), r.max(g).max(b));
                // (lit enough, with little colour: see find_screen)
                if lo >= light_from && hi - lo <= tint {
                    n += 1;
                }
            }
            light[(cy * cw + cx) as usize] = n >= 3;
        }
    }
    // connected bright regions (4-neighbours): their boxes and sizes
    let mut seen = vec![false; light.len()];
    let mut label = vec![usize::MAX; light.len()];
    let mut regions: Vec<(u32, u32, u32, u32, u32)> = Vec::new(); // x0, y0, x1, y1, cells
    for start in 0..light.len() {
        if !light[start] || seen[start] {
            continue;
        }
        let (mut x0, mut y0, mut x1, mut y1, mut cells) = (u32::MAX, u32::MAX, 0, 0, 0);
        let mut stack = vec![start];
        seen[start] = true;
        while let Some(i) = stack.pop() {
            label[i] = regions.len();
            let (x, y) = (i as u32 % cw, i as u32 / cw);
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
            cells += 1;
            let mut push = |j: usize| {
                if light[j] && !seen[j] {
                    seen[j] = true;
                    stack.push(j);
                }
            };
            if x > 0 {
                push(i - 1);
            }
            if x + 1 < cw {
                push(i + 1);
            }
            if y > 0 {
                push(i - cw as usize);
            }
            if y + 1 < ch {
                push(i + cw as usize);
            }
        }
        regions.push((x0, y0, x1, y1, cells));
    }
    // the phone screen's width, in cells: an eighth to 45 % of the picture's height (1080p: 135-486 px)
    let (min_w, max_w) = (ch / 8, if body { ch * 11 / 20 } else { ch * 9 / 20 });
    // (the parts of its box the region holds inside it: a chat's photo is a dark hole in the screen's light, its
    // bubble's light all round it — the screen with photos filled less than half its box, and wasn't found)
    let holes = |idx: usize, r: &(u32, u32, u32, u32, u32)| -> u32 {
        let (x0, y0, x1, y1) = (r.0, r.1, r.2, r.3);
        let (bw, bh) = (x1 - x0 + 1, y1 - y0 + 1);
        let mut out = vec![false; (bw * bh) as usize];
        let mut stack: Vec<(u32, u32)> = Vec::new();
        let other = |x: u32, y: u32| label[(y * cw + x) as usize] != idx;
        for x in x0..=x1 {
            for y in [y0, y1] {
                if other(x, y) && !out[((y - y0) * bw + x - x0) as usize] {
                    out[((y - y0) * bw + x - x0) as usize] = true;
                    stack.push((x, y));
                }
            }
        }
        for y in y0..=y1 {
            for x in [x0, x1] {
                if other(x, y) && !out[((y - y0) * bw + x - x0) as usize] {
                    out[((y - y0) * bw + x - x0) as usize] = true;
                    stack.push((x, y));
                }
            }
        }
        while let Some((x, y)) = stack.pop() {
            for (dx, dy) in [(-1i32, 0i32), (1, 0), (0, -1), (0, 1)] {
                let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                if nx < x0 as i32 || ny < y0 as i32 || nx > x1 as i32 || ny > y1 as i32 {
                    continue;
                }
                let (nx, ny) = (nx as u32, ny as u32);
                let k = ((ny - y0) * bw + nx - x0) as usize;
                if !out[k] && other(nx, ny) {
                    out[k] = true;
                    stack.push((nx, ny));
                }
            }
        }
        let outside = out.iter().filter(|&&o| o).count() as u32;
        (bw * bh).saturating_sub(outside).saturating_sub(r.4)
    };
    let fits = |idx: usize, r: &(u32, u32, u32, u32, u32)| {
        let (w, h) = (r.2 - r.0 + 1, r.3 - r.1 + 1);
        (min_w..=max_w).contains(&w) && (r.4 * 10 >= w * h * 5 || (r.4 + holes(idx, r)) * 10 >= w * h * 7)
    };
    let main = regions.iter().enumerate().filter(|(i, r)| fits(*i, r) && r.3 - r.1 + 1 >= (r.2 - r.0 + 1) / 2).max_by_key(|(_, r)| r.4).map(|(_, r)| r)?;
    // the screen's other bright parts in its column (above or below a photo)
    let (mut x0, mut y0, mut x1, mut y1) = (main.0, main.1, main.2, main.3);
    let w = x1 - x0 + 1;
    for r in &regions {
        if r == main || r.4 < w {
            continue;
        }
        let overlap = r.2.min(x1).saturating_sub(r.0.max(x0)) + 1;
        let near = r.1 <= y1 + w * 2 && r.3 + w * 2 >= y0;
        if overlap * 10 >= w * 8 && r.2 - r.0 + 1 <= w + w / 5 && near {
            x0 = x0.min(r.0);
            y0 = y0.min(r.1);
            x1 = x1.max(r.2);
            y1 = y1.max(r.3);
        }
    }
    // a phone held upright: its bright part at least 0.6 as tall as wide (a Snapmatic post's caption
    // under its photo is wider than tall; its text must be read to tell it from scenery anyway)
    let (bw, bh) = (x1 - x0 + 1, y1 - y0 + 1);
    // a phone held up: tall and narrow as one (its screen 1.6–1.7 times as tall as wide, more than half
    // the picture's height in GTA VI's hand; 1.25 and 45 % at the least, for a tilt or a scroll) — a
    // light wall, a shirt, the message bubble, a lit window are wider or shorter
    // (`sure` false: at least 0.6 as tall as wide and a fifth of the picture, for a post's caption)
    if (sure && (bh * 20 < bw * if body { 22 } else { 25 } || bh * 100 < ch * 45)) || (!sure && (bh * 10 < bw * 6 || bh * 5 < ch)) {
        return None;
    }
    // (below the picture's top, clear of its sides: a phone in the hand never reaches them; a bright sky
    // running down into the message bubble, a wall at the side, do)
    if y0 == 0 || x0 == 0 || x1 + 1 >= cw {
        return None;
    }
    let rect = (x0 * SCALE, y0 * SCALE, (bw * SCALE).min(fw - x0 * SCALE), (bh * SCALE).min(fh - y0 * SCALE));
    // a dim screen (at night): lit in the dark, its sides (the phone's black edge, the hand, the car)
    // much darker than it, and all of it in the picture: dim sky, road or wall is none of that
    if light_from < 175 {
        if x0 == 0 || y0 == 0 || x1 + 1 >= cw || y1 + 1 >= ch {
            return None;
        }
        let luma = |x: u32, y: u32| {
            let i = ((y * fw + x) * 4) as usize;
            (rgba[i] as u32 * 3 + rgba[i + 1] as u32 * 6 + rgba[i + 2] as u32) / 10
        };
        let (rx, ry, rw, rh) = rect;
        // (the screen's light: its brighter samples, not its mean — a chat's big dark photo pulled the mean down;
        // the sides past the phone's own edge, a quarter of the screen out: a white phone's body by day is as light
        // as its screen, and the dim screen beside it was turned down)
        let mut ins: Vec<u32> = Vec::new();
        let (mut sides, mut n_out) = (0u32, 0u32);
        let band = (rw / 4).max(6);
        for k in 1..20 {
            let y = ry + rh * k / 20;
            for j in 1..8 {
                ins.push(luma(rx + rw * j / 8, y));
            }
            for x in [rx.saturating_sub(band), (rx + rw + band).min(fw - 1)] {
                sides += luma(x, y);
                n_out += 1;
            }
        }
        ins.sort_unstable();
        let (inside, sides) = (ins[ins.len() * 3 / 4], sides / n_out.max(1));
        if sides + 50 > inside {
            return None;
        }
    }
    Some(rect)
}

/// The screen's text in pieces (each in frame pixels), top to bottom: lines one under the other (a
/// line pitch apart) are one piece; a gap wider than a line starts the next (the next message's bubble,
/// a button). The first piece, a few words near the screen's top, is its title (the chat's contact).
/// The phone's screen read at any tilt: its pieces, how far it is turned (degrees, clockwise, from its lines
/// of text) and — turned more than a degree — the frame with the screen turned upright, which the pieces'
/// places and crops are in (the reading's lines, rows and pictures are found as on a phone held straight).
pub struct Read {
    pub pieces: Vec<Piece>,
    pub angle: f64,
    pub upright: Option<Vec<u8>>,
    /// The screen itself, measured in the frame the pieces are in (upright when turned): its own size, not
    /// the box round it tilted.
    pub screen: Option<(u32, u32, u32, u32)>,
    /// The screen's four corners in the frame as it is (see screen_quad), and the same corners in the frame
    /// the pieces are in (turned upright with them): where each piece is on the screen, across and down.
    pub quad: Option<[(f64, f64); 4]>,
    pub quad_in_pieces: Option<[(f64, f64); 4]>,
    /// The photos on the screen (a chat's pictures), in the frame the pieces are in: shown as placeholders.
    pub photos: Vec<(u32, u32, u32, u32)>,
    /// The emojis sent on their own (no words by them), in the same frame: shown as they are.
    pub stickers: Vec<(u32, u32, u32, u32)>,
}

impl Read {
    /// Where a piece (its rect, in the frame the pieces are in) is on the screen: across and down, 0 to 1 —
    /// its left, top, width and height as parts of the screen's.
    pub fn on_screen(&self, r: (u32, u32, u32, u32)) -> Option<[f64; 4]> {
        on_screen(self.quad_in_pieces?, r)
    }
}

/// Where a box is on a screen with these corners (in the same frame): across and down, 0 to 1.
pub fn on_screen(q: [(f64, f64); 4], r: (u32, u32, u32, u32)) -> Option<[f64; 4]> {
    {
        let (left, right) = ((q[0].0 + q[3].0) / 2.0, (q[1].0 + q[2].0) / 2.0);
        let (top, bottom) = ((q[0].1 + q[1].1) / 2.0, (q[3].1 + q[2].1) / 2.0);
        let (w, h) = (right - left, bottom - top);
        if w < 8.0 || h < 8.0 {
            return None;
        }
        Some([(r.0 as f64 - left) / w, (r.1 as f64 - top) / h, r.2 as f64 / w, r.3 as f64 / h])
    }
}

pub fn read_screen_any(ocr: &Ocr, rgba: &[u8], fw: u32, fh: u32, screen: (u32, u32, u32, u32)) -> Read {
    let quad = screen_quad(rgba, fw, fh, screen);
    let (pieces, angle) = read_screen_at(ocr, rgba, fw, fh, screen);
    if angle.abs() < 1.0 || angle.abs() > 35.0 {
        let (photos, stickers) = photos_and_stickers(rgba, fw, fh, quad.map(|q| quad_box(q)).unwrap_or(screen), &pieces);
        return Read { pieces, angle, upright: None, screen: Some(screen_edges(rgba, fw, fh, screen)), quad, quad_in_pieces: quad, photos, stickers };
    }
    let up = upright(rgba, fw, fh, screen, angle);
    let (again, _) = read_screen_at(ocr, &up, fw, fh, screen);
    // (read upright, at least as much as before: else as read)
    if again.len() + 1 < pieces.len() {
        let (photos, stickers) = photos_and_stickers(rgba, fw, fh, quad.map(|q| quad_box(q)).unwrap_or(screen), &pieces);
        return Read { pieces, angle, upright: None, screen: None, quad, quad_in_pieces: quad, photos, stickers };
    }
    let edges = screen_edges(&up, fw, fh, screen);
    // (the corners turned upright as the frame was: upright() takes each point from c + R(angle)(p - c), so a
    // corner q is at c + R(-angle)(q - c) in it)
    let (ccx, ccy) = (screen.0 as f64 + screen.2 as f64 / 2.0, screen.1 as f64 + screen.3 as f64 / 2.0);
    let (sin, cos) = (-angle).to_radians().sin_cos();
    let quad_in_pieces = quad.map(|q| q.map(|(x, y)| (ccx + (x - ccx) * cos - (y - ccy) * sin, ccy + (x - ccx) * sin + (y - ccy) * cos)));
    let (photos, stickers) = photos_and_stickers(&up, fw, fh, quad_in_pieces.map(|q| quad_box(q)).unwrap_or(screen), &again);
    Read { pieces: again, angle, upright: Some(up), screen: Some(edges), quad, quad_in_pieces, photos, stickers }
}

/// A message's bubble round its text `r` (frame pixels): from the text outward, each way, as far as the pixels are
/// nearer the bubble's colour than the screen's (several lines across it each way, the median: a letter or an
/// emoji on one of them is no edge). None when no edge is found within a few lines of the text.
pub fn bubble_box(rgba: &[u8], fw: u32, fh: u32, r: (u32, u32, u32, u32), bg: [u8; 3], sbg: [u8; 3], screen: (u32, u32, u32, u32)) -> Option<(u32, u32, u32, u32)> {
    // (a white bubble on the grey screen at night differs from it by a few levels only: enough, side by side)
    if dist(bg, sbg) < 4 {
        return None;
    }
    let (x, y, w, h) = (r.0 as i32, r.1 as i32, r.2 as i32, r.3 as i32);
    let lh = (h.min(40)).max(10);
    let reach = lh * 2;
    let (s0x, s0y, s1x, s1y) = (screen.0 as i32, screen.1 as i32, (screen.0 + screen.2) as i32, (screen.1 + screen.3) as i32);
    let bubble_at = |px_: i32, py_: i32| -> Option<bool> {
        if px_ < s0x.max(0) || py_ < s0y.max(0) || px_ >= s1x.min(fw as i32) || py_ >= s1y.min(fh as i32) {
            return None;
        }
        let c = px(rgba, fw, px_ as u32, py_ as u32);
        Some(dist(c, bg) < dist(c, sbg))
    };
    // (from `from` along `step`, past the text's own last pixels, until three in a row are the screen's)
    let edge = |from: (i32, i32), step: (i32, i32)| -> Option<i32> {
        let mut screen_run = 0;
        let mut last_bubble = 0;
        for k in 1..=reach {
            match bubble_at(from.0 + step.0 * k, from.1 + step.1 * k) {
                None => return None,
                Some(true) => {
                    screen_run = 0;
                    last_bubble = k;
                }
                Some(false) => {
                    screen_run += 1;
                    if screen_run >= 3 && last_bubble > 0 {
                        return Some(last_bubble);
                    }
                }
            }
        }
        None
    };
    let med = |mut v: Vec<i32>| -> Option<i32> {
        if v.len() < 2 {
            return None;
        }
        v.sort_unstable();
        Some(v[v.len() / 2])
    };
    let rows: Vec<i32> = (1..6).map(|k| y + h * k / 6).collect();
    let cols: Vec<i32> = (1..8).map(|k| x + w * k / 8).collect();
    let left = med(rows.iter().filter_map(|&yy| edge((x, yy), (-1, 0))).collect())?;
    let right = med(rows.iter().filter_map(|&yy| edge((x + w, yy), (1, 0))).collect())?;
    let top = med(cols.iter().filter_map(|&xx| edge((xx, y), (0, -1))).collect())?;
    let bottom = med(cols.iter().filter_map(|&xx| edge((xx, y + h), (0, 1))).collect())?;
    let (bx0, by0) = ((x - left).max(0) as u32, (y - top).max(0) as u32);
    Some((bx0, by0, (w + left + right) as u32, (h + top + bottom) as u32))
}

/// The box inside four corners (an upright screen's): its middles' left, top, right and bottom.
fn quad_box(q: [(f64, f64); 4]) -> (u32, u32, u32, u32) {
    let (left, right) = ((q[0].0 + q[3].0) / 2.0, (q[1].0 + q[2].0) / 2.0);
    let (top, bottom) = ((q[0].1 + q[1].1) / 2.0, (q[3].1 + q[2].1) / 2.0);
    (left.max(0.0) as u32, top.max(0.0) as u32, (right - left).max(1.0) as u32, (bottom - top).max(1.0) as u32)
}

/// The photos on a screen (`b`: the screen, frame pixels): big, mostly solid parts that are neither the screen's
/// light nor its text — colourful or dark, a fifth of the screen's width and more, away from the pieces' words.
pub fn photos_in(rgba: &[u8], fw: u32, fh: u32, b: (u32, u32, u32, u32), pieces: &[Piece]) -> Vec<(u32, u32, u32, u32)> {
    photos_and_stickers(rgba, fw, fh, b, pieces).0
}

/// As photos_in, and the small pictures on their own (an emoji sent as a message, with no words by it: the reading
/// has nothing there) — a twentieth to a sixth of the screen's width, about square.
pub fn photos_and_stickers(rgba: &[u8], fw: u32, fh: u32, b: (u32, u32, u32, u32), pieces: &[Piece]) -> (Vec<(u32, u32, u32, u32)>, Vec<(u32, u32, u32, u32)>) {
    let cell = 6u32;
    let (bx, by) = (b.0 + b.2 / 40, b.1 + b.3 / 40);
    let (bw, bh) = (b.2 - b.2 / 20, b.3 - b.3 / 20);
    let (gw, gh) = (bw / cell, bh / cell);
    if gw < 8 || gh < 8 {
        return (Vec::new(), Vec::new());
    }
    let lum = |p: [u8; 3]| (p[0] as u32 * 3 + p[1] as u32 * 6 + p[2] as u32) / 10;
    // the screen's light: the brighter samples
    let mut v: Vec<u32> = Vec::new();
    for k in 1..12 {
        for j in 1..8 {
            let (x, y) = (bx + bw * j / 8, by + bh * k / 12);
            if x < fw && y < fh {
                v.push(lum(px(rgba, fw, x, y)));
            }
        }
    }
    v.sort_unstable();
    let light = v.get(v.len() * 3 / 4).copied().unwrap_or(200);
    let in_text = |x: u32, y: u32| pieces.iter().any(|p| x + 3 >= p.rect.0 && x <= p.rect.0 + p.rect.2 + 3 && y + 3 >= p.rect.1 && y <= p.rect.1 + p.rect.3 + 3 || p.avatar.is_some_and(|a| {
        let (ax, ay) = (p.rect.0 as i32 + a.0, p.rect.1 as i32 + a.1);
        (x as i32) >= ax && (x as i32) <= ax + a.2 as i32 && (y as i32) >= ay && (y as i32) <= ay + a.2 as i32
    }));
    let mut pic = vec![false; (gw * gh) as usize];
    for gy in 0..gh {
        for gx in 0..gw {
            let (x0, y0) = (bx + gx * cell, by + gy * cell);
            if in_text(x0 + cell / 2, y0 + cell / 2) {
                continue;
            }
            let mut n = 0;
            for (dx, dy) in [(1, 1), (4, 1), (1, 4), (4, 4), (2, 3)] {
                let (x, y) = ((x0 + dx).min(fw - 1), (y0 + dy).min(fh - 1));
                let p = px(rgba, fw, x, y);
                let (lo, hi) = (*p.iter().min().unwrap() as u32, *p.iter().max().unwrap() as u32);
                if hi - lo > 30 || lum(p) * 100 < light * 62 {
                    n += 1;
                }
            }
            pic[(gy * gw + gx) as usize] = n >= 3;
        }
    }
    let mut seen = vec![false; pic.len()];
    let mut out = Vec::new();
    let mut stickers = Vec::new();
    let mut parts: Vec<(u32, u32, u32, u32, u32)> = Vec::new();
    for start in 0..pic.len() {
        if !pic[start] || seen[start] {
            continue;
        }
        let (mut x0, mut y0, mut x1, mut y1, mut n) = (u32::MAX, u32::MAX, 0u32, 0u32, 0u32);
        let mut stack = vec![start];
        seen[start] = true;
        while let Some(i) = stack.pop() {
            let (x, y) = (i as u32 % gw, i as u32 / gw);
            (x0, y0, x1, y1, n) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y), n + 1);
            for (dx, dy) in [(-1i32, 0i32), (1, 0), (0, -1), (0, 1)] {
                let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                if nx < 0 || ny < 0 || nx as u32 >= gw || ny as u32 >= gh {
                    continue;
                }
                let j = (ny as u32 * gw + nx as u32) as usize;
                if pic[j] && !seen[j] {
                    seen[j] = true;
                    stack.push(j);
                }
            }
        }
        parts.push((x0, y0, x1, y1, n));
    }
    // (a photo's parts as one: a bright strip in it — a white railing, a sky — split it in two now and then, and
    // its placeholder went from one box to two and back. Parts within two cells of each other merged)
    let mut merged = true;
    while merged {
        merged = false;
        'outer: for i in 0..parts.len() {
            for j in i + 1..parts.len() {
                let (a, b) = (parts[i], parts[j]);
                let close = a.0 <= b.2 + 2 && b.0 <= a.2 + 2 && a.1 <= b.3 + 2 && b.1 <= a.3 + 2;
                if close {
                    parts[i] = (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3), a.4 + b.4);
                    parts.remove(j);
                    merged = true;
                    break 'outer;
                }
            }
        }
    }
    for (mut x0, mut y0, mut x1, mut y1, _) in parts {
        // (its solid part only: rows and columns at its edges mostly empty are something round it — the chosen
        // bubble's thin coloured outline, a word's fringe — not the photo; the outline made one box of the photo
        // and the bubble round it, words and all)
        let filled = |ax: u32, ay: u32, bx_: u32, by_: u32| -> u32 { (ay..=by_).map(|y| (ax..=bx_).filter(|&x| pic[(y * gw + x) as usize]).count() as u32).sum() };
        loop {
            let (w, h) = (x1 - x0 + 1, y1 - y0 + 1);
            if w < 2 || h < 2 {
                break;
            }
            if filled(x0, y0, x1, y0) * 10 < w * 5 {
                y0 += 1;
            } else if filled(x0, y1, x1, y1) * 10 < w * 5 {
                y1 -= 1;
            } else if filled(x0, y0, x0, y1) * 10 < h * 5 {
                x0 += 1;
            } else if filled(x1, y0, x1, y1) * 10 < h * 5 {
                x1 -= 1;
            } else {
                break;
            }
        }
        let n = filled(x0, y0, x1, y1);
        let (w, h) = (x1 - x0 + 1, y1 - y0 + 1);
        // (a photo's size: a fifth of the screen's width, a sixteenth of its height; mostly solid)
        if w * 5 >= gw && h * 16 >= gh && n * 100 >= w * h * 50 {
            out.push((bx + x0 * cell, by + y0 * cell, w * cell, h * cell));
        } else if w * 20 >= gw && w * 6 <= gw && h * 10 >= w * 7 && h * 7 <= w * 10 && n * 100 >= w * h * 45 {
            // (an emoji on its own: a little round of colour, a cell's margin round it)
            stickers.push(((bx + x0 * cell).saturating_sub(2), (by + y0 * cell).saturating_sub(2), w * cell + 4, h * cell + 4));
        }
    }
    out.sort_by_key(|r| (r.1, r.0));
    stickers.sort_by_key(|r| (r.1, r.0));
    (out, stickers)
}

/// The screen's four corners in the frame (top left, top right, bottom right, bottom left; frame pixels), as it
/// is: turned, slanted, seen in perspective. The screen's light, reached from its middle (its text, bubbles
/// and pictures are holes in it, its edges are not), and that light's far points each way. None when the light
/// runs out to the box round it (a bright wall beside the phone) or is not a screen's shape.
pub fn screen_quad(rgba: &[u8], fw: u32, fh: u32, b: (u32, u32, u32, u32)) -> Option<[(f64, f64); 4]> {
    // (the screen's light from half of its own up: a white phone's body by day as light as a dim screen ran the
    // light out round the box at the lowest, and the corners went unfound — the overlay's phone drawn bigger)
    [11u32, 14, 16, 18].iter().find_map(|&part| screen_quad_at(rgba, fw, fh, b, part))
}

fn screen_quad_at(rgba: &[u8], fw: u32, fh: u32, b: (u32, u32, u32, u32), part: u32) -> Option<[(f64, f64); 4]> {
    let (bx, by, bw, bh) = b;
    let (mx, my) = (bw / 10, bh / 16);
    let (x0, y0) = (bx.saturating_sub(mx), by.saturating_sub(my));
    let (x1, y1) = ((bx + bw + mx).min(fw - 1), (by + bh + my).min(fh - 1));
    let step = 2u32;
    let (gw, gh) = ((x1 - x0) / step + 1, (y1 - y0) / step + 1);
    let lum = |x: u32, y: u32| {
        let i = ((y.min(fh - 1) * fw + x.min(fw - 1)) * 4) as usize;
        (rgba[i] as u32 * 3 + rgba[i + 1] as u32 * 6 + rgba[i + 2] as u32) / 10
    };
    // the screen's light: the brighter samples of its middle (its background, not its text)
    let mut v: Vec<u32> = Vec::new();
    for k in 1..12 {
        for j in 1..8 {
            v.push(lum(bx + bw * j / 8, by + bh * k / 12));
        }
    }
    v.sort_unstable();
    let light = v[v.len() * 3 / 4];
    if light < 60 {
        return None;
    }
    let t = light * part / 20;
    let at = |gx: u32, gy: u32| lum(x0 + gx * step, y0 + gy * step) >= t;
    // (from the light nearest each of a few points down the screen's middle — the light nearest its very middle was
    // at times a small island in a chat's big photo — the largest light reached)
    let mut seen = vec![false; (gw * gh) as usize];
    let mut best: Option<(u32, u32, [(f64, f64); 4])> = None; // (cells, edge cells, corners)
    for frac in [2i32, 1, 3, 4, 0, 5] {
        let (cx, cy) = ((gw / 2) as i32, (gh as i32 * (2 * frac + 1)) / 12);
        let seed = (0..gw as i32 / 4).find_map(|r| {
            (-r..=r).flat_map(|dy| (-r..=r).map(move |dx| (dx, dy))).find(|&(dx, dy)| {
                let (x, y) = (cx + dx, cy + dy);
                x >= 0 && y >= 0 && (x as u32) < gw && (y as u32) < gh && at(x as u32, y as u32) && !seen[(y as u32 * gw + x as u32) as usize]
            })
        });
        let Some(seed) = seed else { continue };
        let start = ((cx + seed.0) as u32, (cy + seed.1) as u32);
        let mut stack = vec![start];
        seen[(start.1 * gw + start.0) as usize] = true;
        let (mut tl, mut tr, mut br, mut bl) = ((f64::MAX, (0.0, 0.0)), (f64::MIN, (0.0, 0.0)), (f64::MIN, (0.0, 0.0)), (f64::MAX, (0.0, 0.0)));
        let (mut edge_hit, mut n) = (0u32, 0u32);
        while let Some((gx, gy)) = stack.pop() {
            n += 1;
            let (x, y) = ((x0 + gx * step) as f64, (y0 + gy * step) as f64);
            if gx == 0 || gy == 0 || gx + 1 == gw || gy + 1 == gh {
                edge_hit += 1;
            }
            if x + y < tl.0 {
                tl = (x + y, (x, y));
            }
            if x + y > br.0 {
                br = (x + y, (x, y));
            }
            if x - y > tr.0 {
                tr = (x - y, (x, y));
            }
            if x - y < bl.0 {
                bl = (x - y, (x, y));
            }
            for (dx, dy) in [(-1i32, 0i32), (1, 0), (0, -1), (0, 1)] {
                let (nx, ny) = (gx as i32 + dx, gy as i32 + dy);
                if nx < 0 || ny < 0 || nx as u32 >= gw || ny as u32 >= gh {
                    continue;
                }
                let i = (ny as u32 * gw + nx as u32) as usize;
                if !seen[i] && at(nx as u32, ny as u32) {
                    seen[i] = true;
                    stack.push((nx as u32, ny as u32));
                }
            }
        }
        if best.as_ref().is_none_or(|b| n > b.0) {
            best = Some((n, edge_hit, [tl.1, tr.1, br.1, bl.1]));
        }
    }
    let (n, edge_hit, q) = best?;
    // (the light reaching round the box's edge for long: it is not the screen's alone)
    if edge_hit > (gw + gh) / 4 {
        return None;
    }
    // (a screen's shape: about as big as the box round it, taller than wide, its opposite sides alike)
    let len = |a: (f64, f64), b: (f64, f64)| ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt();
    let (top, left) = (len(q[0], q[1]), len(q[0], q[3]));
    let (bottom, right) = (len(q[3], q[2]), len(q[1], q[2]));
    let area = (n * step * step) as f64;
    let boxa = (bw * bh) as f64;
    if top < bw as f64 * 0.5 || left < bh as f64 * 0.5 || left < top || area < boxa * 0.5 || area > boxa * 1.2 || (top / bottom - 1.0).abs() > 0.25 || (left / right - 1.0).abs() > 0.25 {
        return None;
    }
    Some(q)
}

/// The screen's own edges inside the box round it (frame pixels): from its middle outward along the middle
/// row and column, as far as the screen's light goes (past its text and pictures: a run of dark is the phone's
/// edge). Upright, the box round a tilted screen is bigger than the screen; this is the screen.
pub fn screen_edges(rgba: &[u8], fw: u32, fh: u32, b: (u32, u32, u32, u32)) -> (u32, u32, u32, u32) {
    let (bx, by, bw, bh) = b;
    let lum = |x: u32, y: u32| {
        let i = ((y.min(fh - 1) * fw + x.min(fw - 1)) * 4) as usize;
        (rgba[i] as u32 * 3 + rgba[i + 1] as u32 * 6 + rgba[i + 2] as u32) / 10
    };
    let (cx, cy) = (bx + bw / 2, by + bh / 2);
    // the screen's light: the median along the middle row's inner half
    let mut v: Vec<u32> = (bx + bw / 4..bx + bw * 3 / 4).map(|x| lum(x, cy)).collect();
    v.sort_unstable();
    let light = v.get(v.len() / 2).copied().unwrap_or(200);
    let dark = light / 2;
    // (each edge: several rows or columns, the median: a picture or a bubble on one of them goes dark too)
    let edge = |from: i32, to: i32, step: i32, at: &dyn Fn(i32) -> u32| -> i32 {
        let mut i = from;
        let mut run = 0;
        let mut last_light = from;
        while (step > 0 && i <= to) || (step < 0 && i >= to) {
            if at(i) < dark {
                run += 1;
                if run >= 6 {
                    break;
                }
            } else {
                run = 0;
                last_light = i;
            }
            i += step;
        }
        last_light
    };
    let med = |mut v: Vec<i32>| {
        v.sort_unstable();
        v[v.len() / 2]
    };
    let margin = (bw / 10) as i32;
    let rows: Vec<u32> = (1..8).map(|k| by + bh * k / 8).collect();
    let cols: Vec<u32> = (1..6).map(|k| bx + bw * k / 6).collect();
    let (x0b, x1b) = (bx as i32 - margin, (bx + bw) as i32 + margin);
    let (y0b, y1b) = (by as i32 - margin, (by + bh) as i32 + margin);
    let left = med(rows.iter().map(|&y| edge(cx as i32, x0b.max(0), -1, &|x| lum(x as u32, y))).collect());
    let right = med(rows.iter().map(|&y| edge(cx as i32, x1b.min(fw as i32 - 1), 1, &|x| lum(x as u32, y))).collect());
    let top = med(cols.iter().map(|&x| edge(cy as i32, y0b.max(0), -1, &|y| lum(x, y as u32))).collect());
    let bottom = med(cols.iter().map(|&x| edge(cy as i32, y1b.min(fh as i32 - 1), 1, &|y| lum(x, y as u32))).collect());
    if right - left < bw as i32 / 2 || bottom - top < bh as i32 / 2 {
        return b;
    }
    (left as u32, top as u32, (right - left + 1) as u32, (bottom - top + 1) as u32)
}

/// How far a reading's lines are turned (degrees, clockwise): each line of two words or more, from its first
/// word's middle to its last's; the median of the long ones. 0 with fewer than two.
fn lines_angle(words: &[Word]) -> f64 {
    let mut hs: Vec<u32> = words.iter().map(|w| w.rect.3).collect();
    hs.sort_unstable();
    let lh = hs.get(hs.len() / 2).copied().unwrap_or(10).max(6) as f64;
    let mut by_line: std::collections::BTreeMap<u32, Vec<&Word>> = std::collections::BTreeMap::new();
    for w in words {
        by_line.entry(w.line).or_default().push(w);
    }
    let mut a: Vec<f64> = by_line
        .values()
        .filter(|l| l.len() >= 2)
        .filter_map(|l| {
            let first = l.iter().min_by_key(|w| w.rect.0)?;
            let last = l.iter().max_by_key(|w| w.rect.0 + w.rect.2)?;
            let mid = |w: &Word| (w.rect.0 as f64 + w.rect.2 as f64 / 2.0, w.rect.1 as f64 + w.rect.3 as f64 / 2.0);
            let ((x0, y0), (x1, y1)) = (mid(first), mid(last));
            (x1 - x0 >= lh * 4.0).then(|| (y1 - y0).atan2(x1 - x0).to_degrees())
        })
        .collect();
    if a.len() < 2 {
        return 0.0;
    }
    a.sort_by(|x, y| x.total_cmp(y));
    a[a.len() / 2]
}

/// "0k" / "0K" read for "Ok" at a word's start (the OCR takes the round O for a zero).
fn zero_ok(t: &str) -> String {
    t.split(' ')
        .map(|w| {
            let core = w.trim_end_matches(['.', ',', '!', '?']);
            if core == "0k" || core == "0K" || core == "0kay" { format!("O{}", &w[1..]) } else { w.to_string() }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The frame with the phone's screen (and a margin round it) turned upright: turned back by `angle` round
/// the screen's middle (bilinear). The rest of the frame as it is.
pub fn upright(rgba: &[u8], fw: u32, fh: u32, screen: (u32, u32, u32, u32), angle: f64) -> Vec<u8> {
    let mut out = rgba.to_vec();
    let (sx, sy, sw, sh) = screen;
    let (mx, my) = (sw / 8, sh / 12);
    let (x0, y0) = (sx.saturating_sub(mx), sy.saturating_sub(my));
    let (x1, y1) = ((sx + sw + mx).min(fw), (sy + sh + my).min(fh));
    let (cx, cy) = (sx as f64 + sw as f64 / 2.0, sy as f64 + sh as f64 / 2.0);
    let (sin, cos) = angle.to_radians().sin_cos();
    for y in y0..y1 {
        for x in x0..x1 {
            let (dx, dy) = (x as f64 - cx, y as f64 - cy);
            let (u, v) = (cx + dx * cos - dy * sin, cy + dx * sin + dy * cos);
            if u < 0.0 || v < 0.0 || u >= (fw - 1) as f64 || v >= (fh - 1) as f64 {
                continue;
            }
            let (ux, vy) = (u.floor() as u32, v.floor() as u32);
            let (fx, fy) = (u - ux as f64, v - vy as f64);
            let at = |xx: u32, yy: u32, c: usize| rgba[((yy * fw + xx) * 4) as usize + c] as f64;
            let o = ((y * fw + x) * 4) as usize;
            for c in 0..3 {
                let top = at(ux, vy, c) * (1.0 - fx) + at(ux + 1, vy, c) * fx;
                let bot = at(ux, vy + 1, c) * (1.0 - fx) + at(ux + 1, vy + 1, c) * fx;
                out[o + c] = (top * (1.0 - fy) + bot * fy).round() as u8;
            }
        }
    }
    out
}

pub fn read_screen(ocr: &Ocr, rgba: &[u8], fw: u32, fh: u32, screen: (u32, u32, u32, u32)) -> Vec<Piece> {
    read_screen_at(ocr, rgba, fw, fh, screen).0
}

fn read_screen_at(ocr: &Ocr, rgba: &[u8], fw: u32, fh: u32, screen: (u32, u32, u32, u32)) -> (Vec<Piece>, f64) {
    let (sx, sy, sw, sh) = screen;
    let Some(crop) = Crop::from_frame(rgba, fw, fh, sx as f64 / fw as f64, sy as f64 / fh as f64, sw as f64 / fw as f64, sh as f64 / fh as f64) else { return (Vec::new(), 0.0) };
    let mut words = match ocr.recognize_checked(&crop) {
        Ok((w, _)) => w,
        Err(_) => return (Vec::new(), 0.0),
    };
    // the character's own messages, white on a bright blue: read again by the darkest channel (by the
    // brightest, the blue is as bright as the letters), what the first reading missed added
    let (mut blue, mut all) = (0u32, 0u32);
    for yy in (sy..sy + sh).step_by(6) {
        for xx in (sx..sx + sw).step_by(6) {
            let i = ((yy * fw + xx) * 4) as usize;
            all += 1;
            if rgba[i + 2] as i32 - rgba[i] as i32 > 100 {
                blue += 1;
            }
        }
    }
    if blue * 50 >= all {
        if let Ok(more) = ocr.recognize_with(&crop, ocr::Prep::Darkest) {
            let overlaps = |a: &Word, b: &Word| a.rect.0 < b.rect.0 + b.rect.2 && b.rect.0 < a.rect.0 + a.rect.2 && a.rect.1 < b.rect.1 + b.rect.3 && b.rect.1 < a.rect.1 + a.rect.3;
            let new: Vec<Word> = more.into_iter().filter(|m| m.text.chars().any(|c| c.is_alphanumeric()) && !words.iter().any(|w| overlaps(w, m))).collect();
            words.extend(new);
        }
    }
    let angle = lines_angle(&words);
    if std::env::var_os("VIT_DISK_DEBUG").is_some() {
        eprintln!("[phone] lines turned {angle:.2}°");
        for w in &words {
            eprintln!("[word] {:?} line {} at {:?}", w.text, w.line, w.rect);
        }
    }
    // (a line the screen's edge cuts — scrolled half out: "pln her riee from tho" — is no text to translate)
    let edge = 4;
    let mut out: Vec<Piece> = pieces(&words, crop.w, crop.h)
        .into_iter()
        .filter(|p| p.rect.1 > edge && p.rect.1 + p.rect.3 + edge < crop.h)
        .map(|p| {
            let rect = (p.rect.0 + sx, p.rect.1 + sy, p.rect.2, p.rect.3);
            Piece { rect, look: look_of(rgba, fw, fh, rect, screen), ..p }
        })
        .collect();
    for p in &mut out {
        // a map pin's card ("Map Pin Unavailable", "Map Pin Expired"): bordered, with its pin
        if !p.title && p.en.to_lowercase().starts_with("map pin") {
            p.look.boxed = true;
            p.look.label = false;
            p.look.end = false;
        }
        // the chat's header: its contact's picture at the top right
        if p.title {
            let lh = p.rect.3.max(10);
            let area = (sx + sw * 4 / 5, p.rect.1.saturating_sub(lh), sw / 5 - 4, lh * 3);
            // (as the circle it is, going right from the name; else as before)
            let sbg = screen_bg(rgba, fw, fh, screen, p.rect.1, p.rect.1 + p.rect.3);
            let reach = (sx + sw).saturating_sub(p.rect.0 + p.rect.2) as i32;
            p.avatar = disk_from(rgba, fw, fh, p.rect.0 + p.rect.2, p.rect.1 + p.rect.3 / 2, p.look.bg, sbg, lh * 2, 1, reach)
                .map(|(cx, cy, d)| {
                    let s = d * 23 / 20;
                    (cx as i32 - s as i32 / 2 - p.rect.0 as i32, cy as i32 - s as i32 / 2 - p.rect.1 as i32, s)
                })
                .or_else(|| {
                    ink_box(rgba, fw, fh, area, p.look.bg, 40)
                        .filter(|b| b.2 >= 10 && b.3 >= 10 && b.2 * 10 <= b.3 * 16 && b.3 * 10 <= b.2 * 16)
                        .map(|b| (b.0 as i32 - p.rect.0 as i32, b.1 as i32 - p.rect.1 as i32, b.2.max(b.3)))
                });
        }
    }
    // A chat list (WhatUp's chats): rows with their contacts' pictures at the start. Each row is its
    // contact's name over the message's start; the rest are its section headers ("Suggested", "Messages").
    // (a row has two lines: the name and the message; a one-line header's icon is no contact's picture)
    for p in &mut out {
        // (the character's own messages, blue, are never a list's rows)
        let own = p.look.bg[2] as i32 - p.look.bg[0] as i32 > 100 || own_blue(rgba, fw, p.rect);
        p.avatar = if (p.split.is_some() || p.name.is_some()) && !own { avatar_of(rgba, fw, fh, p.rect, p.look.bg, screen) } else { None };
    }
    if out.iter().filter(|p| p.avatar.is_some()).count() >= 2 {
        // (a dark photo is missed now and then: a two-line piece starting where the rows' text starts is one
        // too, its picture where theirs are)
        let mut xs: Vec<u32> = out.iter().filter(|p| p.avatar.is_some()).map(|p| p.rect.0).collect();
        xs.sort_unstable();
        let row_x = xs[xs.len() / 2];
        let mut avs: Vec<(i32, i32, u32)> = out.iter().filter_map(|p| p.avatar).collect();
        avs.sort_by_key(|a| a.2);
        let typical = avs[avs.len() / 2];
        let side = typical.2;
        // (the rows whose picture was found where it is, not given the usual one)
        let mut found: Vec<bool> = out.iter().map(|p| p.avatar.is_some()).collect();
        for (i, p) in out.iter_mut().enumerate() {
            if p.avatar.is_none() && (p.split.is_some() || p.name.is_some()) && p.rect.0.abs_diff(row_x) <= sw / 30 {
                p.avatar = Some(typical);
                found[i] = false;
            }
            // a row of one line read (its message an emoji, or missed): the contact's name, and the
            // message's place under it shown as it is
            if p.avatar.is_none() && p.split.is_none() && p.name.is_none() && p.rect.0.abs_diff(row_x) <= sw / 30 && p.en.split_whitespace().count() <= 4 {
                // (its picture as the circle it is, level with the name's foot, the row's middle; else as before)
                let sbg = screen_bg(rgba, fw, fh, screen, p.rect.1, p.rect.1 + p.rect.3 * 2);
                let a = disk_at(rgba, fw, fh, p.rect.0, p.rect.1 + p.rect.3, p.look.bg, sbg, side)
                    .map(|(dx, dy, d)| (dx as i32 + d as i32 / 2 - p.rect.0 as i32, dy as i32 + d as i32 / 2 - p.rect.1 as i32, d))
                    .or_else(|| avatar_of(rgba, fw, fh, (p.rect.0, p.rect.1, p.rect.2, p.rect.3 * 2), p.look.bg, screen));
                if a.is_some() {
                    p.avatar = a;
                    found[i] = true;
                    p.split = Some((p.en.clone(), String::new()));
                    let below = (p.rect.0, p.rect.1 + p.rect.3 + p.rect.3 / 5, (sw * 55 / 100).min(sx + sw - p.rect.0), p.rect.3 * 7 / 5);
                    p.preview = ink_box(rgba, fw, fh, below, p.look.bg, 70).filter(|b| b.2 <= b.3 * 4).map(|b| (b.0 as i32 - p.rect.0 as i32, b.1 as i32 - p.rect.1 as i32, b.2, b.3));
                }
            }
        }
        // (each picture as a square of the rows' usual size, from its bottom right corner, a little wider:
        // the unread dot on its corner in it too)
        // (where the pictures end, the rows' usual; level with each row's text)
        // (where each row's own picture ends, found; else by its text, the rows' usual gap before it. Held
        // tilted, the rows don't line up: one place for them all cut the top rows' pictures from the phone's
        // edge and the bottom rows' from their names, and the gap itself changes down the screen)
        let mut gaps: Vec<i32> = out.iter().zip(&found).filter(|(_, f)| **f).filter_map(|(p, _)| p.avatar.map(|a| -a.0)).collect();
        gaps.sort_unstable();
        let gap = gaps.get(gaps.len() / 2).copied().unwrap_or(side as i32 / 4);
        let m = (side / 10) as i32;
        // (the pictures found as the circles they are, where they are: any tilt, the phone's edge beside them
        // or not; all of the size most of them are, each a square round its own circle, a little bigger)
        let disks: Vec<Option<(u32, u32, u32)>> = out
            .iter()
            .map(|p| {
                p.avatar?;
                let one_line = p.split.as_ref().is_some_and(|s| s.1.is_empty()) || p.en.is_empty();
                let cy = p.rect.1 + if one_line { p.rect.3 * 11 / 10 } else { p.rect.3 / 2 };
                let sbg = screen_bg(rgba, fw, fh, screen, p.rect.1, p.rect.1 + p.rect.3);
                disk_at(rgba, fw, fh, p.rect.0, cy, p.look.bg, sbg, side)
            })
            .collect();
        let mut ds: Vec<u32> = disks.iter().flatten().map(|d| d.2).collect();
        ds.sort_unstable();
        let disk_d = ds.get(ds.len() / 2).copied();
        for (i, p) in out.iter_mut().enumerate() {
            if let Some(a) = p.avatar {
                if let (Some((dx, dy, _)), Some(d)) = (disks[i], disk_d) {
                    let s = d * 23 / 20;
                    p.avatar = Some((dx as i32 - s as i32 / 2 - p.rect.0 as i32, dy as i32 - s as i32 / 2 - p.rect.1 as i32, s));
                    continue;
                }
                let end = if found[i] { a.0 } else { -gap };
                let one_line = p.split.as_ref().is_some_and(|s| s.1.is_empty()) || p.en.is_empty();
                let mid = if found[i] { a.1 - side as i32 / 2 } else { (p.rect.1 as i32 + if one_line { (p.rect.3 * 11 / 10) as i32 } else { (p.rect.3 / 2) as i32 }) - p.rect.1 as i32 };
                p.avatar = Some((end - side as i32 - m, mid - side as i32 / 2 - m, side + 2 * m as u32));
            }
        }
        // (a row scrolled half out of the screen: its picture cut by the screen's edge; left out)
        out.retain(|p| {
            p.avatar.is_none_or(|a| {
                let (ty, s) = (p.rect.1 as i32 + a.1, a.2 as i32);
                ty >= sy as i32 - s / 6 && ty + s <= (sy + sh) as i32 + s / 6
            })
        });
        // (one row is the chosen one: the one its outline hugs; the next row's search finds it too)
        let edges: Vec<Option<([u8; 3], i32)>> = out.iter().map(|p| if p.avatar.is_some() { row_edge(rgba, fw, p.rect, p.look.bg, screen) } else { None }).collect();
        let chosen = edges.iter().enumerate().filter_map(|(i, e)| e.map(|e| (i, e.1))).min_by_key(|e| e.1).map(|e| e.0);
        for (i, p) in out.iter_mut().enumerate() {
            if p.avatar.is_some() {
                p.look.edge = if Some(i) == chosen { edges[i].map(|e| e.0) } else { None };
            }
        }
        for p in &mut out {
            if p.avatar.is_some() {
                // (its mark at the end, level with the name: a pin, a reply arrow)
                let x0 = sx + sw * 4 / 5;
                let lh = if p.split.as_ref().is_some_and(|s| s.1.is_empty()) { p.rect.3 } else { p.rect.3 / 2 };
                let area = (x0, p.rect.1.saturating_sub(lh / 4), (sx + sw).saturating_sub(x0 + sw / 40), lh + lh / 4);
                p.badge = ink_box(rgba, fw, fh, area, p.look.bg, 40)
                    .filter(|b| b.2 >= 8 && b.3 >= 8 && b.2 * 10 <= b.3 * 16 && b.3 * 10 <= b.2 * 16 && !mostly_dark(rgba, fw, *b))
                    .map(|b| (b.0 as i32 - p.rect.0 as i32, b.1 as i32 - p.rect.1 as i32, b.2, b.3));
            }
        }
        for p in &mut out {
            p.look.end = false;
            p.title = false;
            if p.avatar.is_some() {
                if let Some((name, rest)) = p.split.take() {
                    p.name = Some(name);
                    p.en = rest;
                }
                // (a name the OCR lost: the row without one)
                if p.name.as_deref().is_some_and(|n| n.trim().is_empty()) {
                    p.name = None;
                }
                p.look.label = false;
            } else {
                p.name = None;
                p.look.label = true;
                // a section's header: its icon before it ("Suggested"), read as a stray letter: "e Suggested"
                let mut words: Vec<&str> = p.en.split_whitespace().collect();
                let stray = words.len() > 1 && words[0].chars().filter(|c| c.is_alphanumeric()).count() <= 1;
                if stray {
                    words.remove(0);
                    p.en = words.join(" ");
                }
                let lh = p.rect.3;
                // (the icon read as that letter is the text's start itself: looked for there, not before it,
                // where the phone's edge is)
                let area = if stray {
                    (p.rect.0.saturating_sub(lh / 2).max(sx), p.rect.1.saturating_sub(lh / 4), lh * 2, lh + lh / 2)
                } else {
                    (p.rect.0.saturating_sub(lh * 2).max(sx), p.rect.1.saturating_sub(lh / 4), lh * 2 - 3, lh + lh / 2)
                };
                // (only with room for one before it: "Messages" starts at the screen's edge)
                let room = stray || p.rect.0 > sx + lh * 3 / 2;
                p.badge = ink_box(rgba, fw, fh, area, p.look.bg, 40)
                    .filter(|b| room && b.2 >= 8 && b.3 >= 8 && b.2 * 10 <= b.3 * 16 && b.3 * 10 <= b.2 * 16 && !mostly_dark(rgba, fw, *b))
                    .map(|b| (b.0 as i32 - p.rect.0 as i32, b.1 as i32 - p.rect.1 as i32, b.2, b.3));
            }
        }
    } else {
        for p in &mut out {
            p.avatar = None;
        }
    }
    // the emojis by each piece's text (the OCR skips them): after its words on its lines, or on a line
    // of their own under them; in a chat list's row, short of its mark at the end
    let lh = (sw / 20).max(8);
    for p in &mut out {
        if p.title || p.look.label {
            continue;
        }
        let right = if p.avatar.is_some() { sx + sw * 4 / 5 } else { sx + sw - 4 };
        let x1 = (p.rect.0 + p.rect.2 + lh * 3).min(right);
        let area = (p.rect.0.saturating_sub(2), p.rect.1.saturating_sub(2), x1.saturating_sub(p.rect.0 - 2), p.rect.3 + lh + 4);
        p.emoji = crate::chat::emoji_in(rgba, fw, fh, area, p.look.bg, p.look.fg, lh);
    }
    // (a line right under a chat's title, closer than a bubble ever starts — one scrolled up under the title bar,
    // cut in half by it: "services? Active duty?" read as "ACtjve 'f" — is no message to translate)
    if let Some(t) = out.iter().find(|p| p.title).map(|p| p.rect) {
        out.retain(|p| p.title || p.split.is_some() || !(p.rect.1 >= t.1 + t.3 && p.rect.1 < t.1 + t.3 + t.3 * 13 / 10 && p.rect.3 < t.3 * 8 / 5));
    }
    // (a chat's header — its first line, in the screen's top eighth, no list's rows round it — whether the reading
    // took it for one or not: its contact's picture to the right of the name, as the circle it is)
    if !out.iter().any(|p| p.avatar.is_some() && !p.title) {
        if let Some(p) = out.first_mut().filter(|p| p.rect.1 < sy + sh / 9 && p.split.is_none() && p.en.split_whitespace().count() <= 5) {
            p.title = true;
            if p.avatar.is_none() {
                let sbg = screen_bg(rgba, fw, fh, screen, p.rect.1, p.rect.1 + p.rect.3);
                let reach = (sx + sw).saturating_sub(p.rect.0 + p.rect.2) as i32;
                p.avatar = disk_from(rgba, fw, fh, p.rect.0 + p.rect.2, p.rect.1 + p.rect.3 / 2, p.look.bg, sbg, p.rect.3.max(10) * 2, 1, reach).map(|(cx, cy, d)| {
                    let s = d * 23 / 20;
                    (cx as i32 - s as i32 / 2 - p.rect.0 as i32, cy as i32 - s as i32 / 2 - p.rect.1 as i32, s)
                });
            }
        }
    }
    // (each chat message's bubble as it is: the overlay draws its own bubble the same size, in the same place)
    let (sx, sy, sw, sh) = screen;
    for p in out.iter_mut().filter(|p| !p.title && !p.look.label && p.avatar.is_none()) {
        let sbg = screen_bg(rgba, fw, fh, screen, p.rect.1, p.rect.1 + p.rect.3);
        p.bubble = bubble_box(rgba, fw, fh, p.rect, p.look.bg, sbg, (sx, sy, sw, sh));
    }
    // ("Ok" read with a zero, "0k. There's some cash…": the word it is, so the line is found and translated)
    for p in &mut out {
        p.en = zero_ok(&p.en);
        if let Some((a, b)) = p.split.as_mut() {
            *a = zero_ok(a);
            *b = zero_ok(b);
        }
    }
    (out, angle)
}

/// The contact's round picture before a chat list's row (`r`: its text): a block about the text's
/// height, just left of it, mostly unlike the row's colour (a face, a photo, not the row's padding).
fn avatar_of(rgba: &[u8], fw: u32, fh: u32, r: (u32, u32, u32, u32), bg: [u8; 3], screen: (u32, u32, u32, u32)) -> Option<(i32, i32, u32)> {
    let (x, y, _, h) = r;
    let side = (h * 11 / 10).max(20);
    let x1 = x.checked_sub(side / 6)?;
    let x0 = x1.checked_sub(side + side / 3)?.max(screen.0);
    let (y0, y1) = (y.saturating_sub(side / 4), (y + h + side / 4).min(fh - 1));
    if x1 <= x0 + side / 2 {
        return None;
    }
    // (unlike the row and unlike the screen too: left of a blue bubble's words is its padding, then the
    // light screen, no picture)
    let sbg = screen_bg(rgba, fw, fh, screen, y0, y1);
    let (mut n, mut hit) = (0u32, 0u32);
    let (mut bx0, mut by0, mut bx1, mut by1) = (u32::MAX, u32::MAX, 0, 0);
    for yy in (y0..y1).step_by(2) {
        for xx in (x0..x1.min(fw)).step_by(2) {
            n += 1;
            let p = px(rgba, fw, xx, yy);
            if dist(p, bg) > 45 && dist(p, sbg) > 45 {
                hit += 1;
                (bx0, by0, bx1, by1) = (bx0.min(xx), by0.min(yy), bx1.max(xx), by1.max(yy));
            }
        }
    }
    if n == 0 || hit * 100 < n * 18 {
        return None;
    }
    let (bw, bh) = (bx1 - bx0, by1 - by0);
    // (round: about as wide as it is tall)
    if bw * 10 < bh * 6 || bh * 10 < bw * 6 || bh < h / 2 {
        return None;
    }
    // (a contact's picture, not a small icon: "Suggested"'s; about 6 % of the screen's height at least)
    if bw.min(bh) * 100 < screen.3 * 6 {
        return None;
    }
    // (on the screen, the row or the screen's light round it on both sides: held tilted, the ground beside
    // the phone is in the screen's box, dark against the grey bubbles, and no picture)
    let cy = (by0 + by1) / 2;
    let lit = |xx: u32| {
        let p = px(rgba, fw, xx.min(fw - 1), cy);
        let (lo, hi) = (*p.iter().min().unwrap(), *p.iter().max().unwrap());
        lo >= 90 && hi - lo <= 45
    };
    if !lit(bx0.saturating_sub(4)) || !lit(bx1 + 4) {
        return None;
    }
    // (its bottom right corner, and its size: an unread dot on its top left corner makes it look bigger)
    Some((bx1 as i32 - x as i32, by1 as i32 - y as i32, bw.min(bh)))
}

/// The outline round a chat list's chosen row (`r`: its text): a thin line close above it and below it,
/// unlike the row, of whatever colour (blue on the phone; the game's light tints it, brown in the sun).
fn row_edge(rgba: &[u8], fw: u32, r: (u32, u32, u32, u32), bg: [u8; 3], screen: (u32, u32, u32, u32)) -> Option<([u8; 3], i32)> {
    let (x, y, w, h) = r;
    let (top, bottom) = (screen.1 as i32, (screen.1 + screen.3) as i32);
    let mut near = 0;
    let reach = (h * 3 / 4).max(12) as i32;
    let mut found = Vec::new();
    for cx in [x + w / 4, x + w / 2] {
        for (y0, dy) in [(y as i32 - 1, -1), ((y + h) as i32, 1)] {
            let at = |k: i32| {
                let yy = y0 + dy * k;
                // (inside the screen: its edge is no outline)
                (yy >= top && yy < bottom && cx < fw).then(|| px(rgba, fw, cx, yy as u32))
            };
            let hit = (0..reach).find(|&k| at(k).is_some_and(|p| dist(p, bg) > 50));
            match hit {
                Some(k) => {
                    found.extend(at(k));
                    near += k;
                }
                None => return None,
            }
        }
    }
    Some((median(&mut found), near))
}

/// How far the phone is turned (degrees, clockwise as CSS turns things), from a chat list's pictures: down a
/// straight phone they are one above the other; held tilted, each lower one is further left (or right).
/// None with fewer than three pictures, or ones that don't line up.
pub fn tilt_of(pieces: &[Piece]) -> Option<f64> {
    let c: Vec<(f64, f64)> = pieces
        .iter()
        .filter_map(|p| p.avatar.map(|a| ((p.rect.0 as i32 + a.0) as f64 + a.2 as f64 / 2.0, (p.rect.1 as i32 + a.1) as f64 + a.2 as f64 / 2.0)))
        .collect();
    if c.len() < 3 {
        return None;
    }
    // x = a + b·y, least squares; and how far the pictures are off that line
    let n = c.len() as f64;
    let (my, mx) = (c.iter().map(|p| p.1).sum::<f64>() / n, c.iter().map(|p| p.0).sum::<f64>() / n);
    let syy: f64 = c.iter().map(|p| (p.1 - my).powi(2)).sum();
    if syy < 1.0 {
        return None;
    }
    let b = c.iter().map(|p| (p.1 - my) * (p.0 - mx)).sum::<f64>() / syy;
    let off = (c.iter().map(|p| (p.0 - mx - b * (p.1 - my)).powi(2)).sum::<f64>() / n).sqrt();
    let side = pieces.iter().filter_map(|p| p.avatar.map(|a| a.2 as f64)).sum::<f64>() / n;
    // (pictures a third of their size or more off the line: not one column of them; and no phone is held
    // more than 30° over)
    let deg = (-b).atan().to_degrees();
    (off <= side / 3.0 && deg.abs() <= 30.0).then_some(deg)
}

/// A contact's round picture before a row's text, found as the circle it is: from the text's start
/// (`from_x`) leftwards along the row's middle (`cy`), the first run of pixels unlike the row and the
/// screen is the picture; its middle up and down gives its height, and across again its width. Its
/// centre and diameter (frame pixels). Held tilted, the phone's edge and the ground beside it come
/// after the row's own colour, past the picture: the run stops at that gap.
pub fn disk_at(rgba: &[u8], fw: u32, fh: u32, from_x: u32, cy: u32, bg: [u8; 3], sbg: [u8; 3], expect: u32) -> Option<(u32, u32, u32)> {
    disk_from(rgba, fw, fh, from_x, cy, bg, sbg, expect, -1, expect as i32 * 2)
}

/// As disk_at, going `dir` (-1 left, 1 right) from `from_x`, up to `reach` pixels: a chat's header has its
/// contact's picture to the right of its name.
pub fn disk_from(rgba: &[u8], fw: u32, fh: u32, from_x: u32, cy: u32, bg: [u8; 3], sbg: [u8; 3], expect: u32, dir: i32, reach: i32) -> Option<(u32, u32, u32)> {
    let unlike = |x: u32, y: u32| x < fw && y < fh && {
        let p = px(rgba, fw, x, y);
        dist(p, bg) > 40 && dist(p, sbg) > 40
    };
    // (a run of the picture along a line from `start`, `step` each way: where it ends, through gaps of a
    // few pixels — a light beard, a white border — and no further than `max`)
    let run = |start: i32, fixed: u32, along_x: bool, step: i32, max: i32| -> i32 {
        let (mut last, mut gap, mut i) = (start, 0, start);
        while (i - start).abs() <= max {
            let hit = if along_x { i >= 0 && unlike(i as u32, fixed) } else { i >= 0 && unlike(fixed, i as u32) };
            if hit {
                last = i;
                gap = 0;
            } else {
                gap += 1;
                if gap > (expect as i32 / 12).max(3) {
                    break;
                }
            }
            i += step;
        }
        last
    };
    // the picture: going left from the text, the first run of unlike pixels as wide as a third of a picture
    // at least (through gaps of a few pixels) — not a letter's stroke (held tilted, the line under starts
    // further left), not the chosen row's thin outline
    let start = from_x as i32 + 2 * dir;
    let (mut x, mut found) = (start, None);
    while (x - start).abs() <= reach && x > 1 && x < fw as i32 - 1 {
        if !unlike(x as u32, cy) {
            x += dir;
            continue;
        }
        let end = run(x, cy, true, dir, expect as i32 * 3 / 2);
        if ((x - end).abs() + 1) as u32 >= (expect * 35 / 100).max(8) {
            found = Some((x, end));
            break;
        }
        x = end + dir;
    }
    // (its near and far edges along the line: x1 the one nearer the text)
    let (x1, x0) = found.map(|(a, b)| if dir < 0 { (a, b) } else { (b, a) })?;
    // (its middle across, on this line; up and down from there its whole height: the diameter, through
    // a picture whose inside is white too — a logo — where across breaks)
    let mx = ((x0 + x1) / 2) as u32;
    let (y0, y1) = (run(cy as i32, mx, false, -1, expect as i32), run(cy as i32, mx, false, 1, expect as i32));
    let my = ((y0 + y1) / 2) as u32;
    let (bx0, bx1) = (run(mx as i32, my, true, -1, expect as i32), run(mx as i32, my, true, 1, expect as i32));
    let (w, h) = ((bx1 - bx0 + 1) as u32, (y1 - y0 + 1) as u32);
    if std::env::var_os("VIT_DISK_DEBUG").is_some() {
        eprintln!("[disk] from {from_x},{cy} expect {expect}: x1 {x1} x0 {x0} y {y0}..{y1} x {bx0}..{bx1} -> {w}x{h}");
    }
    // (round: no wider than tall; and about the size a row's picture is)
    if w > h * 5 / 4 || h * 2 < expect || h > expect * 2 || ((x1 - x0 + 1) as u32) > h * 5 / 4 {
        return None;
    }
    // (across through its middle, when it held: the middle of that; else of the first line across)
    let cx = if w * 5 >= h * 4 { ((bx0 + bx1) / 2) as u32 } else { mx };
    Some((cx, my, h))
}

/// Whether a box (frame pixels) is mostly dark: the phone's edge, or the ground beside a phone held tilted
/// (a mark on the light screen — a pin, an arrow, an icon — is mostly light or coloured).
fn mostly_dark(rgba: &[u8], fw: u32, b: (u32, u32, u32, u32)) -> bool {
    let (mut n, mut dark) = (0u32, 0u32);
    for yy in (b.1..b.1 + b.3).step_by(2) {
        for xx in (b.0..(b.0 + b.2).min(fw)).step_by(2) {
            if ((yy * fw + xx) * 4 + 2) as usize >= rgba.len() {
                continue;
            }
            n += 1;
            let p = px(rgba, fw, xx, yy);
            if (p[0] as u32 + p[1] as u32 + p[2] as u32) < 3 * 70 {
                dark += 1;
            }
        }
    }
    n > 0 && dark * 2 > n
}

/// What's drawn in `area` (frame pixels) on a row of colour `bg`: the box round its pixels unlike it.
fn ink_box(rgba: &[u8], fw: u32, fh: u32, area: (u32, u32, u32, u32), bg: [u8; 3], unlike: i32) -> Option<(u32, u32, u32, u32)> {
    let (x, y, w, h) = area;
    let (w, h) = (w.min(fw.saturating_sub(x)), h.min(fh.saturating_sub(y)));
    let (mut cols, mut rows) = (vec![0u32; w as usize], vec![0u32; h as usize]);
    for yy in 0..h {
        for xx in 0..w {
            if dist(px(rgba, fw, x + xx, y + yy), bg) > unlike {
                cols[xx as usize] += 1;
                rows[yy as usize] += 1;
            }
        }
    }
    // (columns and rows with a few such pixels: a stray one, the row's soft edge, is none of it)
    // (and the first thing in it only, from its start: not the row's edge further along)
    let span = |v: &[u32]| {
        let a = v.iter().position(|&n| n >= 3)?;
        let mut b = a;
        for (i, &n) in v.iter().enumerate().skip(a) {
            if n >= 3 {
                b = i;
            } else if i > b + 6 {
                break;
            }
        }
        Some((a as u32, b as u32))
    };
    let ((x0, x1), (y0, y1)) = (span(&cols)?, span(&rows)?);
    (x1 > x0 + 3 && y1 > y0 + 3).then(|| ((x + x0).saturating_sub(2), (y + y0).saturating_sub(2), x1 - x0 + 5, y1 - y0 + 5))
}

/// A part of the frame (offset `o` from `r`'s top left) as a PNG image, `h` pixels tall.
pub fn part_png(rgba: &[u8], fw: u32, fh: u32, r: (u32, u32, u32, u32), o: (i32, i32, u32, u32), h: u32) -> Option<Vec<u8>> {
    let (ax, ay) = ((r.0 as i32 + o.0).max(0) as u32, (r.1 as i32 + o.1).max(0) as u32);
    let (w0, h0) = (o.2.min(fw.saturating_sub(ax)), o.3.min(fh.saturating_sub(ay)));
    if w0 < 4 || h0 < 4 {
        return None;
    }
    let mut img = image::RgbaImage::new(w0, h0);
    for yy in 0..h0 {
        for xx in 0..w0 {
            let i = (((ay + yy) * fw + ax + xx) * 4) as usize;
            img.put_pixel(xx, yy, image::Rgba([rgba[i], rgba[i + 1], rgba[i + 2], 255]));
        }
    }
    let img = image::imageops::resize(&img, (w0 * h / h0).max(1), h, image::imageops::FilterType::Triangle);
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).ok()?;
    Some(out.into_inner())
}

/// The buttons' words at the picture's bottom right while the phone is up ("SCROLL", "SEND", "BACK",
/// one under the other, each with its icon after it): each line's place (frame pixels) and its word(s).
pub fn read_buttons(ocr: &Ocr, rgba: &[u8], fw: u32, fh: u32) -> Vec<((u32, u32, u32, u32), String)> {
    let (x, y, w, h) = (0.86, 0.84, 0.135, 0.15);
    let Some(crop) = Crop::from_frame(rgba, fw, fh, x, y, w, h) else { return Vec::new() };
    let (x0, y0) = ((x * fw as f64) as u32, (y * fh as f64) as u32);
    let words = match ocr.recognize_checked(&crop) {
        Ok((w, _)) => w,
        Err(_) => return Vec::new(),
    };
    // (capital words only: the icons read as stray marks, "®", "(X)")
    let mut words: Vec<&Word> = words.iter().filter(|w| w.text.chars().filter(|c| c.is_alphabetic()).count() >= 2 && w.text.chars().all(|c| !c.is_alphabetic() || c.is_uppercase())).collect();
    words.sort_by_key(|w| w.rect.1 + w.rect.3 / 2);
    let mut lines: Vec<Vec<&Word>> = Vec::new();
    for wd in words {
        let m = wd.rect.1 + wd.rect.3 / 2;
        match lines.last_mut() {
            Some(l) if m <= l[0].rect.1 + l[0].rect.3 / 2 + l[0].rect.3 / 2 => l.push(wd),
            _ => lines.push(vec![wd]),
        }
    }
    lines
        .into_iter()
        .map(|mut l| {
            l.sort_by_key(|w| w.rect.0);
            let (a, b) = (l.iter().map(|w| w.rect.0).min().unwrap_or(0), l.iter().map(|w| w.rect.0 + w.rect.2).max().unwrap_or(0));
            let (t, u) = (l.iter().map(|w| w.rect.1).min().unwrap_or(0), l.iter().map(|w| w.rect.1 + w.rect.3).max().unwrap_or(0));
            let text = l.iter().map(|w| w.text.trim_matches(|c: char| !c.is_alphabetic())).collect::<Vec<_>>().join(" ");
            ((a + x0, t + y0, b - a, u - t), text)
        })
        .collect()
}

/// A button's icon just after its word ("SEND ✕"): the box round its white pixels there (frame pixels).
pub fn button_icon(rgba: &[u8], fw: u32, fh: u32, word: (u32, u32, u32, u32)) -> Option<(u32, u32, u32, u32)> {
    let (x, y, w, h) = word;
    let (ax, ay) = (x + w + 2, y.saturating_sub(h / 2));
    let (aw, ah) = ((h * 5 / 2).min(fw.saturating_sub(ax)), (h * 2).min(fh.saturating_sub(ay)));
    let (mut x0, mut y0, mut x1, mut y1, mut n) = (u32::MAX, u32::MAX, 0, 0, 0);
    for yy in ay..ay + ah {
        for xx in ax..ax + aw {
            let i = ((yy * fw + xx) * 4) as usize;
            let (lo, hi) = (rgba[i].min(rgba[i + 1]).min(rgba[i + 2]), rgba[i].max(rgba[i + 1]).max(rgba[i + 2]));
            if lo >= 190 && hi - lo <= 40 {
                (x0, y0, x1, y1, n) = (x0.min(xx), y0.min(yy), x1.max(xx), y1.max(yy), n + 1);
            }
        }
    }
    // (round, about as wide as tall: a box wider than that took the word's last letter too, its edge a pixel
    // or two past the reading's box: the icon is its right part)
    let bh = y1.saturating_sub(y0) + 1;
    if n >= 12 && x1 + 1 > x0 + bh * 13 / 10 {
        x0 = x1 + 1 - bh;
    }
    let bw = x1.saturating_sub(x0) + 1;
    (n >= 12 && bh * 10 >= h * 8 && bw * 10 >= h * 6 && bw <= h * 2).then(|| (x0.saturating_sub(1), y0.saturating_sub(1), bw + 2, bh + 2))
}

/// Words (crop pixels) into pieces (crop pixels): see read_screen.
pub fn pieces(words: &[Word], crop_w: u32, crop_h: u32) -> Vec<Piece> {
    let words: Vec<&Word> = words.iter().filter(|w| w.text.chars().any(|c| c.is_alphanumeric())).collect();
    if words.is_empty() {
        return Vec::new();
    }
    let mut hs: Vec<u32> = words.iter().map(|w| w.rect.3).collect();
    hs.sort_unstable();
    let lh = hs[hs.len() / 2].max(6);
    // visual lines, top to bottom (middles within half a line)
    // (the reading's own lines, when it gives them: held tilted, a line's first word sits lower or higher
    // than its last, and by height alone "Ok." went to a line of its own after "There's some cash")
    let mut lines: Vec<Vec<&Word>> = Vec::new();
    if words.iter().any(|w| w.line > 0) {
        let mut by: std::collections::BTreeMap<u32, Vec<&Word>> = std::collections::BTreeMap::new();
        for w in &words {
            by.entry(w.line).or_default().push(w);
        }
        lines = by.into_values().collect();
        lines.sort_by_key(|l| l.iter().map(|w| w.rect.1 + w.rect.3 / 2).sum::<u32>() / l.len() as u32);
    } else {
        let mut order: Vec<&Word> = words.clone();
        order.sort_by_key(|w| w.rect.1 + w.rect.3 / 2);
        for w in order {
            let m = w.rect.1 + w.rect.3 / 2;
            match lines.last_mut() {
                Some(l) if m <= l[0].rect.1 + l[0].rect.3 / 2 + lh / 2 => l.push(w),
                _ => lines.push(vec![w]),
            }
        }
    }
    let span = |ws: &[&Word]| {
        let x0 = ws.iter().map(|w| w.rect.0).min().unwrap_or(0);
        let y0 = ws.iter().map(|w| w.rect.1).min().unwrap_or(0);
        let x1 = ws.iter().map(|w| w.rect.0 + w.rect.2).max().unwrap_or(0);
        let y1 = ws.iter().map(|w| w.rect.1 + w.rect.3).max().unwrap_or(0);
        (x0, y0, x1.saturating_sub(x0), y1.saturating_sub(y0))
    };
    // lines into pieces: a gap of more than a line's height between them starts the next
    let mut groups: Vec<Vec<Vec<&Word>>> = Vec::new();
    for mut l in lines {
        l.sort_by_key(|w| w.rect.0);
        let starts_new = groups.last().and_then(|g| g.last()).is_none_or(|prev| {
            let (_, py, _, ph) = span(prev);
            let (_, y, _, _) = span(&l);
            y.saturating_sub(py + ph) > lh * 4 / 5
        });
        if starts_new {
            groups.push(vec![l]);
        } else {
            groups.last_mut().expect("one").push(l);
        }
    }
    let mut out: Vec<Piece> = Vec::new();
    for (n, g) in groups.iter().enumerate() {
        let all: Vec<&Word> = g.iter().flatten().copied().collect();
        let owned: Vec<Word> = all.iter().map(|w| (*w).clone()).collect();
        let en = ocr::clean(&owned);
        // (a contact's short name alone on its line, its message an emoji the reading has no words for — "Cal"
        // over 😍: the clean-up takes a lone short word for noise, and the row was lost. A capitalised word
        // of three letters or more, as read)
        let en = if en.is_empty() && all.len() == 1 && {
            let t = all[0].text.trim_matches(|c: char| !c.is_alphanumeric());
            t.chars().count() >= 3 && t.chars().all(char::is_alphabetic) && t.chars().next().is_some_and(char::is_uppercase)
        } {
            all[0].text.trim_matches(|c: char| !c.is_alphanumeric()).to_string()
        } else {
            en
        };
        if en.is_empty() || !ocr::looks_like_words(&en) {
            continue;
        }
        // (a count — likes, comments: "VIK D 3" — is no text to translate)
        if en.chars().any(|c| c.is_ascii_digit()) && en.chars().filter(|c| c.is_alphabetic()).count() < 6 {
            continue;
        }
        let rect = span(&all);
        // (the first piece, a few words in the screen's top eighth: the contact's name, the app's title)
        let title = n == 0 && g.len() == 1 && en.split_whitespace().count() <= 4 && rect.1 < crop_h / 8;
        // a chat list's item: a short first line with no sentence end (the contact), then the message's start
        // (an unread dot or a mark at its end, read as "c", "C)", "0": no part of the name)
        let mut name_words: Vec<&Word> = g[0].clone();
        while name_words.len() > 1 && name_words.last().is_some_and(|w| w.text.chars().filter(|c| c.is_alphanumeric()).count() <= 2) {
            name_words.pop();
        }
        let first: Vec<Word> = name_words.iter().map(|w| (*w).clone()).collect();
        let first_text = ocr::clean(&first);
        // (a short name the clean-up takes for noise, "Cal": as read)
        let first_text = if first_text.trim().is_empty() { first.iter().map(|w| w.text.trim_matches(|c: char| !c.is_alphanumeric())).collect::<Vec<_>>().join(" ").trim().to_string() } else { first_text };
        let rest: Vec<Word> = g[1..].iter().flatten().map(|w| (*w).clone()).collect();
        // (and clearly shorter than the lines under it: a message's first line runs the bubble's width)
        let first_w = span(&name_words).2;
        let rest_w = g[1..].iter().map(|l| span(l).2).max().unwrap_or(0);
        // (a chat list's row starts after the contact's picture, a fifth of the way across; a message's
        // bubble near the screen's edge: "Someone just showed / me a crazy story" is no contact)
        let named = g.len() >= 2
            && rect.0 * 100 > crop_w * 18
            && first_text.split_whitespace().count() <= 4
            && !first_text.contains(['.', '!', '?', ','])
            && first_text.chars().next().is_some_and(|c| c.is_uppercase())
            && first_w * 10 < rest_w * 7;
        if named && !rest.is_empty() {
            out.push(Piece { rect, en: ocr::clean(&rest), title: false, name: Some(first_text), look: Look::default(), split: None, avatar: None, preview: None, badge: None, emoji: Vec::new(), bubble: None });
        } else {
            let split = (g.len() >= 2 && !rest.is_empty()).then(|| (first_text.clone(), ocr::clean(&rest)));
            out.push(Piece { rect, en, title, name: None, look: Look::default(), split, avatar: None, preview: None, badge: None, emoji: Vec::new(), bubble: None });
        }
    }
    out
}

/// A piece as read before (see `steady`): where it is on the screen (relative to the screen's top
/// left, frame pixels), its first reading, and when it was last read.
#[derive(Clone, Debug)]
pub struct Seen {
    pub at: (i32, i32, u32, u32),
    pub piece: Piece,
    pub when: std::time::Instant,
}

/// How long a piece not read this time stays (the OCR misses the white-on-blue bubbles now and then).
pub const KEEP: std::time::Duration = std::time::Duration::from_millis(600);

/// How clean a reading looks: plain words count for it, odd ones against it (letters outside English,
/// a digit inside a word, a capital in a word's middle: "Whateveryöu", "screW", "up4hd").
fn clean_score(t: &str) -> i32 {
    t.split_whitespace()
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()))
        .filter(|w| !w.is_empty())
        .map(|w| {
            let odd = w.chars().any(|c| c.is_alphabetic() && !c.is_ascii())
                || (w.chars().any(|c| c.is_ascii_digit()) && w.chars().any(|c| c.is_alphabetic()))
                || w.chars().skip(1).zip(w.chars()).any(|(c, prev)| c.is_uppercase() && prev.is_lowercase());
            if odd { -2 } else { 1 }
        })
        .sum()
}

/// Edit distance over characters, for "the same text read a little differently".
fn distance(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cur = row[j + 1];
            row[j + 1] = (prev + usize::from(ca != cb)).min(row[j] + 1).min(cur + 1);
            prev = cur;
        }
    }
    row[b.len()]
}

/// The phone's pieces made steady from one read to the next. A piece in the same place on the screen
/// as one read before, with much the same text, is that piece: its first reading stays (a misreading —
/// "J won'tscreW.itup. 'Ahd I'll remember. Thahks." — went to the translator again, and its translation
/// changed). A piece read before and missed this time stays for KEEP (its bubble went for a frame).
/// `seen`: the pieces as read before (kept up to date); `screen`: where the screen is now.
pub fn steady(seen: &mut Vec<Seen>, read: Vec<Piece>, screen: (u32, u32, u32, u32), now: std::time::Instant) -> Vec<Piece> {
    let (sx, sy) = (screen.0 as i32, screen.1 as i32);
    let norm = |t: &str| t.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect::<String>();
    let mut used = vec![false; seen.len()];
    let mut out = Vec::new();
    for p in read {
        let at = (p.rect.0 as i32 - sx, p.rect.1 as i32 - sy, p.rect.2, p.rect.3);
        // (much the same text in the same column, the nearest one: a chat scrolling moves every piece up
        // or down, a repeated "Locating..." each takes its own)
        let near = |s: &Seen| (s.at.0 - at.0).abs() <= 30 && s.piece.title == p.title;
        let full = |p: &Piece| format!("{} {}", p.name.as_deref().unwrap_or(""), p.en);
        let alike = |s: &Seen| {
            let close = |a: String, b: String| !a.is_empty() && distance(&a, &b) * 10 <= a.len().max(b.len()) * 4;
            // (or the message alike, a row read without its name this time — "Shirley" in the chosen row's
            // outline, missed now and then: the same row, not a new one in its place)
            close(norm(&full(&s.piece)), norm(&full(&p))) || (s.piece.avatar.is_some() && p.avatar.is_some() && (s.piece.name.is_none() || p.name.is_none()) && close(norm(&s.piece.en), norm(&p.en)))
        };
        let found = seen.iter().enumerate().filter(|(i, s)| !used[*i] && near(s) && alike(s)).min_by_key(|(_, s)| (s.at.1 - at.1).abs()).map(|(i, _)| i);
        match found {
            Some(i) => {
                used[i] = true;
                seen[i].at = at;
                seen[i].when = now;
                // (a clearly cleaner reading of it replaces the first: its translation changes once, for the better)
                // (a chat list's row read as one line, its name and message run together, keeps the row)
                let row_lost = seen[i].piece.avatar.is_some() && p.avatar.is_none();
                if !row_lost && (clean_score(&p.en) > clean_score(&seen[i].piece.en) + 1 || (p.avatar.is_some() && seen[i].piece.avatar.is_none())) {
                    seen[i].piece = p.clone();
                }
                // (its name read now, missed before: the row with it)
                if seen[i].piece.name.is_none() && p.name.is_some() && p.avatar.is_some() {
                    seen[i].piece.name = p.name.clone();
                }
                if p.preview.is_some() {
                    seen[i].piece.preview = p.preview;
                }
                // (its first reading's text; how it looks and what's around it as now: the chosen row's
                // outline moves, a picture is where it is now)
                let first = &seen[i].piece;
                if first.avatar.is_some() && p.avatar.is_none() {
                    out.push(Piece { rect: p.rect, look: Look { edge: p.look.edge, ..first.look.clone() }, ..first.clone() });
                } else {
                    // (an emoji missed this time: as last seen)
                    let preview = p.preview.or(first.preview);
                    out.push(Piece { en: first.en.clone(), name: first.name.clone(), title: first.title, split: first.split.clone(), preview, ..p });
                }
            }
            None => {
                seen.push(Seen { at, piece: p.clone(), when: now });
                used.push(true);
                out.push(p);
            }
        }
    }
    // (read before, missed this time: where it is now, for a moment; not where a piece read now is —
    // a chat scrolling: its new reading there, the old one gone)
    let read_now: Vec<(u32, u32)> = out.iter().map(|p| (p.rect.1, p.rect.1 + p.rect.3)).collect();
    for (i, s) in seen.iter().enumerate() {
        if !used[i] && now.saturating_duration_since(s.when) < KEEP {
            let rect = ((sx + s.at.0).max(0) as u32, (sy + s.at.1).max(0) as u32, s.at.2, s.at.3);
            let covered = read_now.iter().any(|&(a, b)| rect.1 < b && a < rect.1 + rect.3);
            if !covered {
                out.push(Piece { rect, ..s.piece.clone() });
            }
        }
    }
    seen.retain(|s| now.saturating_duration_since(s.when) < KEEP * 4);
    out.sort_by_key(|p| p.rect.1);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dim_screen_in_the_dark_is_a_phone() {
        // a screen held up in a dark room, its least colour 74 (under every light find_screen tries), round it 20
        let (w, h) = (1280u32, 720u32);
        let mut f = vec![20u8; (w * h * 4) as usize];
        for y in 110..610 {
            for x in 500..800 {
                let i = ((y * w + x) * 4) as usize;
                f[i..i + 3].copy_from_slice(&[80, 78, 74]);
            }
        }
        assert!(find_screen(&f, w, h).is_none(), "a screen this dim was found already");
        let r = find_phone(&f, w, h).expect("the dim screen is a phone");
        assert!(r.0.abs_diff(500) <= 6 && r.1.abs_diff(110) <= 6 && r.2.abs_diff(300) <= 12 && r.3.abs_diff(500) <= 12, "{r:?}");
        // a dim light region as wide as it is tall is none
        let mut wall = vec![20u8; (w * h * 4) as usize];
        for y in 110..610 {
            for x in 300..800 {
                let i = ((y * w + x) * 4) as usize;
                wall[i..i + 3].copy_from_slice(&[80, 78, 74]);
            }
        }
        assert!(find_phone(&wall, w, h).is_none());
    }

    #[test]
    fn ok_read_with_a_zero_is_ok() {
        assert_eq!(super::zero_ok("0k. There's some cash"), "Ok. There's some cash");
        assert_eq!(super::zero_ok("Room 0k"), "Room Ok");
        assert_eq!(super::zero_ok("10k cash"), "10k cash");
    }

    #[test]
    fn a_tilted_reading_s_lines_give_its_turn() {
        // two lines going down to the right, 5 px over 100 px: about 2.9°
        let at = |t: &str, x: u32, y: u32, line: u32| Word { text: t.into(), rect: (x, y, 20, 12), line };
        let words = vec![at("a", 0, 100, 0), at("b", 100, 105, 0), at("c", 0, 140, 1), at("d", 100, 145, 1)];
        let a = super::lines_angle(&words);
        assert!((a - 2.86).abs() < 0.1, "{a}");
        // straight, or one line only: none
        assert_eq!(super::lines_angle(&words[..2]), 0.0);
    }

    #[test]
    fn a_turned_screen_is_turned_back_upright() {
        // a dark dot turned 10° round the middle comes back where it was
        let (w, h) = (200u32, 200u32);
        let mut rgba = vec![255u8; (w * h * 4) as usize];
        let (sin, cos) = 10f64.to_radians().sin_cos();
        let (dx, dy) = (60.0, 0.0);
        let (x, y) = ((100.0 + dx * cos - dy * sin) as u32, (100.0 + dx * sin + dy * cos) as u32);
        for yy in y - 2..=y + 2 {
            for xx in x - 2..=x + 2 {
                let i = ((yy * w + xx) * 4) as usize;
                rgba[i..i + 3].copy_from_slice(&[0, 0, 0]);
            }
        }
        let up = super::upright(&rgba, w, h, (20, 20, 160, 160), 10.0);
        let i = ((100 * w + 160) * 4) as usize;
        assert!(up[i] < 80, "the dot upright at (160, 100): {}", up[i]);
    }

    fn w(t: &str, x: u32, y: u32, wd: u32) -> Word {
        Word { text: t.into(), rect: (x, y, wd, 18), line: 0 }
    }

    #[test]
    fn a_chat_reads_as_its_title_and_messages() {
        // Kenton's chat: the name at the top, a two-line message, then "Locating..." further down
        let words = vec![
            w("Kenton", 120, 20, 70),
            w("Hey", 20, 80, 40), w("Jason!", 64, 80, 70), w("Was", 140, 80, 40),
            w("good", 20, 106, 50), w("meeting", 74, 106, 80),
            w("Locating...", 40, 170, 110),
        ];
        let p = pieces(&words, 400, 700);
        assert_eq!(p.iter().map(|p| (p.en.as_str(), p.title)).collect::<Vec<_>>(), vec![("Kenton", true), ("Hey Jason! Was good meeting", false), ("Locating...", false)]);
    }

    #[test]
    fn a_misread_or_missed_piece_stays_as_first_read() {
        let t = std::time::Instant::now();
        let p = |en: &str, y: u32| Piece { rect: (824, y, 205, 48), en: en.into(), title: false, name: None, look: Look::default(), split: None, avatar: None, preview: None, badge: None, emoji: Vec::new(), bubble: None };
        let mut seen = Vec::new();
        let screen = (690, 480, 402, 420);
        steady(&mut seen, vec![p("I won't screw it up. And I'll remember. Thanks.", 738)], screen, t);
        // misread the next time, the phone moved 6 px: the first reading
        let next = steady(&mut seen, vec![p("J won'tscreW.itup. 'Ahd I'll remember. Thahks.", 744)], (696, 486, 402, 420), t + std::time::Duration::from_millis(100));
        assert_eq!(next[0].en, "I won't screw it up. And I'll remember. Thanks.");
        // a cleaner reading replaces a poor first one
        let mut seen2 = Vec::new();
        steady(&mut seen2, vec![p("Whateveryöu Say.", 821)], screen, t);
        let better = steady(&mut seen2, vec![p("Whatever you say.", 821)], screen, t + std::time::Duration::from_millis(100));
        assert_eq!(better[0].en, "Whatever you say.");
        // missed: still there for a moment, where the phone is now
        let missed = steady(&mut seen, vec![], (700, 490, 402, 420), t + std::time::Duration::from_millis(300));
        assert_eq!((missed.len(), missed[0].rect.1), (1, 748));
        assert!(steady(&mut seen, vec![], (700, 490, 402, 420), t + std::time::Duration::from_millis(1000)).is_empty());
        // another message in that place (scrolled): new
        let other = steady(&mut seen, vec![p("Whatever you say.", 748)], (700, 490, 402, 420), t + std::time::Duration::from_millis(1100));
        assert_eq!(other[0].en, "Whatever you say.");
    }

    #[test]
    fn a_chat_list_item_is_its_contact_and_message() {
        let words = vec![
            w("Suggested", 20, 20, 90),
            w("Kenton", 90, 80, 70), w("c", 300, 80, 10),
            w("Still", 90, 104, 40), w("can't", 134, 104, 50), w("believe", 188, 104, 70),
            w("1K", 20, 200, 20), w("3", 60, 200, 10),
        ];
        let p = pieces(&words, 400, 700);
        assert_eq!(p.len(), 2, "{p:?}"); // (the counts left out)
        assert_eq!((p[1].name.as_deref(), p[1].en.as_str()), (Some("Kenton"), "Still can't believe"));
    }
}
