//! The game's controller icons, carried by the app (cut from GTA VI's own screens, PlayStation 5): drawn in
//! a translation where the game shows a button, clean and the same every time, instead of a picture cut
//! from the frame (which picks up the background, or loses a D-pad's grey arms).
//!
//! - Pages scripts ask for one by name (`vi.gameIcon("L2")` → a PNG data URL).
//! - The subtitle reader recognises an icon it cut from a line (`identify`) and draws the clean one.

use std::collections::HashMap;
use std::sync::OnceLock;

/// A picture to look in: its size and RGBA pixels (a crop, or a whole frame).
pub struct Picture<'a> {
    pub w: u32,
    pub h: u32,
    pub rgba: &'a [u8],
}

impl<'a> From<&'a crate::ocr::Crop> for Picture<'a> {
    fn from(c: &'a crate::ocr::Crop) -> Self {
        Picture { w: c.w, h: c.h, rgba: &c.rgba }
    }
}

/// Name ("X", "L2", "DPAD-UP" …) and PNG.
const PS5: &[(&str, &[u8])] = &[
    ("X", include_bytes!("../game-icons/ps5/x.png")),
    ("CIRCLE", include_bytes!("../game-icons/ps5/circle.png")),
    ("SQUARE", include_bytes!("../game-icons/ps5/square.png")),
    ("TRIANGLE", include_bytes!("../game-icons/ps5/triangle.png")),
    ("L1", include_bytes!("../game-icons/ps5/l1.png")),
    ("R1", include_bytes!("../game-icons/ps5/r1.png")),
    ("L2", include_bytes!("../game-icons/ps5/l2.png")),
    ("R2", include_bytes!("../game-icons/ps5/r2.png")),
    ("L3", include_bytes!("../game-icons/ps5/l3.png")),
    ("R3", include_bytes!("../game-icons/ps5/r3.png")),
    ("TOUCHPAD", include_bytes!("../game-icons/ps5/touchpad.png")),
    ("DPAD-UP", include_bytes!("../game-icons/ps5/dpad-up.png")),
    ("DPAD-DOWN", include_bytes!("../game-icons/ps5/dpad-down.png")),
    ("DPAD-LEFT", include_bytes!("../game-icons/ps5/dpad-left.png")),
    ("DPAD-RIGHT", include_bytes!("../game-icons/ps5/dpad-right.png")),
    ("LEFT-STICK", include_bytes!("../game-icons/ps5/left-stick.png")),
    ("RIGHT-STICK", include_bytes!("../game-icons/ps5/right-stick.png")),
];

/// The size icons are compared at (a small grey picture of each).
const N: u32 = 32;

struct Icon {
    url: String,
    /// grey level and coverage (0..1), N × N
    grey: Vec<f32>,
    alpha: Vec<f32>,
    /// width / height
    aspect: f32,
}

fn icons() -> &'static HashMap<&'static str, Icon> {
    static ICONS: OnceLock<HashMap<&'static str, Icon>> = OnceLock::new();
    ICONS.get_or_init(|| {
        use base64::Engine as _;
        PS5.iter()
            .filter_map(|(name, png)| {
                let img = image::load_from_memory(png).ok()?.to_rgba8();
                let aspect = img.width() as f32 / img.height().max(1) as f32;
                let small = image::imageops::resize(&img, N, N, image::imageops::FilterType::Triangle);
                let grey = small.pixels().map(|p| (p[0] as f32 * 0.299 + p[1] as f32 * 0.587 + p[2] as f32 * 0.114) / 255.0).collect();
                let alpha = small.pixels().map(|p| p[3] as f32 / 255.0).collect();
                let url = format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(png));
                Some((*name, Icon { url, grey, alpha, aspect }))
            })
            .collect()
    })
}

/// The icon by name, as a PNG data URL.
pub fn data_url(name: &str) -> Option<&'static str> {
    icons().get(name.to_ascii_uppercase().as_str()).map(|i| i.url.as_str())
}

/// Which of the game's icons a part of a picture shows (`r`: its box, x, y, w, h), if one clearly does: the
/// icons drawn over the background around it, compared with it at a small size (normalised correlation of
/// grey levels: at least 0.7, and well ahead of the next best; on 711 icons from saved frames: 343 recognised,
/// none wrong — the others keep the picture cut from the frame).
pub fn identify(c: &Picture, r: (u32, u32, u32, u32)) -> Option<&'static str> {
    let scores = ranked(c, r)?;
    match scores.as_slice() {
        [(name, best), rest @ ..] if *best >= 0.7 && rest.first().is_none_or(|(_, next)| best - next >= 0.05) => Some(name),
        _ => None,
    }
}

/// How alike each carried icon is to the picture in box `r`, best first (for a caller that knows which icon to
/// expect and only needs to tell whether these are the game's PS5 icons at all).
pub fn ranked(c: &Picture, r: (u32, u32, u32, u32)) -> Option<Vec<(&'static str, f32)>> {
    let (x, y, w, h) = frame(c, r)?;
    // (the box found can be a pixel or two short of the icon's own on any side: a soft edge fading into the
    // background, or a black outline on a dark one; the carried icons have them: a few framings tried)
    let mut scores: Vec<(&'static str, f32)> = Vec::new();
    for left in 0..=3u32 {
        for right in 0..=3u32 {
            for gy in 0..=2u32 {
                let (fx, fy) = (x.saturating_sub(left), y.saturating_sub(gy));
                let r = (fx, fy, (w + left + right).min(c.w - fx), (h + 2 * gy).min(c.h - fy));
                for (name, s) in scores_at(c, r) {
                    match scores.iter_mut().find(|e| e.0 == name) {
                        Some(e) => e.1 = e.1.max(s),
                        None => scores.push((name, s)),
                    }
                }
            }
        }
    }
    scores.sort_by(|a, b| b.1.total_cmp(&a.1));
    if std::env::var("VIT_ICON_DEBUG").is_ok() {
        eprintln!("ICONDBG {:?} -> {:?}", (x, y, w, h), &scores[..scores.len().min(3)]);
    }
    Some(scores)
}

/// How alike each carried icon is to the picture in box `r`.
fn scores_at(c: &Picture, r: (u32, u32, u32, u32)) -> Vec<(&'static str, f32)> {
    let (x, y, w, h) = r;
    if w < 6 || h < 6 || x + w > c.w || y + h > c.h {
        return Vec::new();
    }
    let lum = |px: u32, py: u32| {
        let i = ((py * c.w + px) * 4) as usize;
        (c.rgba[i] as f32 * 0.299 + c.rgba[i + 1] as f32 * 0.587 + c.rgba[i + 2] as f32 * 0.114) / 255.0
    };
    // the background: the box's edge, a pixel outside it where there is one
    let mut edge: Vec<f32> = Vec::new();
    let (ex0, ey0, ex1, ey1) = (x.saturating_sub(1), y.saturating_sub(1), (x + w).min(c.w - 1), (y + h).min(c.h - 1));
    for px in ex0..=ex1 {
        edge.push(lum(px, ey0));
        edge.push(lum(px, ey1));
    }
    for py in ey0..=ey1 {
        edge.push(lum(ex0, py));
        edge.push(lum(ex1, py));
    }
    edge.sort_by(f32::total_cmp);
    let bg = edge[edge.len() / 2];
    // the part, N × N (area average)
    let mut seen = vec![0f32; (N * N) as usize];
    for j in 0..N {
        for i in 0..N {
            let (sx0, sx1) = (x + i * w / N, (x + (i + 1) * w / N).max(x + i * w / N + 1));
            let (sy0, sy1) = (y + j * h / N, (y + (j + 1) * h / N).max(y + j * h / N + 1));
            let (mut s, mut n) = (0f32, 0f32);
            for py in sy0..sy1.min(c.h) {
                for px in sx0..sx1.min(c.w) {
                    s += lum(px, py);
                    n += 1.0;
                }
            }
            seen[(j * N + i) as usize] = if n > 0.0 { s / n } else { bg };
        }
    }
    let aspect = w as f32 / h as f32;
    icons()
        .iter()
        .filter(|(_, ic)| (ic.aspect / aspect - 1.0).abs() < 0.35)
        .map(|(name, ic)| {
            let drawn: Vec<f32> = ic.grey.iter().zip(&ic.alpha).map(|(g, a)| g * a + bg * (1.0 - a)).collect();
            (*name, correlation(&seen, &drawn))
        })
        .collect()
}

/// The icon's own box: a box found by its bright parts (a ring, a letter) can leave out its dark outline, the
/// way the carried icons are cut. Looked for a little around `r`: everything that differs from the background.
fn frame(c: &Picture, r: (u32, u32, u32, u32)) -> Option<(u32, u32, u32, u32)> {
    let (x, y, w, h) = r;
    if w < 6 || h < 6 || x + w > c.w || y + h > c.h {
        return None;
    }
    // (the outline is a couple of pixels: further out are the next letters)
    let m = (h / 8).max(2);
    let (x0, y0, x1, y1) = (x.saturating_sub(m), y.saturating_sub(m), (x + w + m).min(c.w - 1), (y + h + m).min(c.h - 1));
    let px = |px: u32, py: u32| {
        let i = ((py * c.w + px) * 4) as usize;
        [c.rgba[i] as i32, c.rgba[i + 1] as i32, c.rgba[i + 2] as i32]
    };
    let mut edge: Vec<[i32; 3]> = Vec::new();
    for qx in x0..=x1 {
        edge.push(px(qx, y0));
        edge.push(px(qx, y1));
    }
    for qy in y0..=y1 {
        edge.push(px(x0, qy));
        edge.push(px(x1, qy));
    }
    let med = |k: usize| {
        let mut v: Vec<i32> = edge.iter().map(|p| p[k]).collect();
        v.sort_unstable();
        v[v.len() / 2]
    };
    let bg = [med(0), med(1), med(2)];
    let (mut bx0, mut by0, mut bx1, mut by1) = (u32::MAX, u32::MAX, 0, 0);
    for qy in y0..=y1 {
        for qx in x0..=x1 {
            let p = px(qx, qy);
            let d = (p[0] - bg[0]).abs() + (p[1] - bg[1]).abs() + (p[2] - bg[2]).abs();
            if d > 70 {
                (bx0, by0, bx1, by1) = (bx0.min(qx), by0.min(qy), bx1.max(qx), by1.max(qy));
            }
        }
    }
    if bx0 == u32::MAX {
        return None;
    }
    // (never much bigger than the part found: a neighbouring letter isn't the icon)
    let (fw, fh) = (bx1 - bx0 + 1, by1 - by0 + 1);
    if fw > w + 2 * m || fh > h + 2 * m || fw < 6 || fh < 6 {
        return Some(r);
    }
    Some((bx0, by0, fw, fh))
}

fn correlation(a: &[f32], b: &[f32]) -> f32 {
    let n = a.len() as f32;
    let (ma, mb) = (a.iter().sum::<f32>() / n, b.iter().sum::<f32>() / n);
    let (mut ab, mut aa, mut bb) = (0f32, 0f32, 0f32);
    for (x, y) in a.iter().zip(b) {
        ab += (x - ma) * (y - mb);
        aa += (x - ma) * (x - ma);
        bb += (y - mb) * (y - mb);
    }
    if aa <= 1e-6 || bb <= 1e-6 { 0.0 } else { ab / (aa * bb).sqrt() }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An icon drawn at another size over a purple background, as the game would show it.
    fn shown(name: &str, scale: f32, bg: [u8; 3]) -> (crate::ocr::Crop, (u32, u32, u32, u32)) {
        let png = PS5.iter().find(|(n, _)| *n == name).unwrap().1;
        let img = image::load_from_memory(png).unwrap().to_rgba8();
        let (w, h) = ((img.width() as f32 * scale) as u32, (img.height() as f32 * scale) as u32);
        let img = image::imageops::resize(&img, w, h, image::imageops::FilterType::Triangle);
        let (cw, ch) = (w + 20, h + 20);
        let mut rgba = Vec::with_capacity((cw * ch * 4) as usize);
        for _ in 0..cw * ch {
            rgba.extend_from_slice(&[bg[0], bg[1], bg[2], 255]);
        }
        for (px, py, p) in img.enumerate_pixels() {
            let i = (((py + 10) * cw + px + 10) * 4) as usize;
            let a = p[3] as f32 / 255.0;
            for k in 0..3 {
                rgba[i + k] = (p[k] as f32 * a + rgba[i + k] as f32 * (1.0 - a)) as u8;
            }
        }
        (crate::ocr::Crop { w: cw, h: ch, rgba }, (10, 10, w, h))
    }

    #[test]
    fn every_icon_is_carried_and_never_taken_for_another() {
        let mut known = 0;
        for (name, _) in PS5 {
            assert!(data_url(name).is_some_and(|u| u.starts_with("data:image/png;base64,")), "{name}");
            // (as big as the game draws them at 1080p and up; one not recognised keeps the picture cut from the
            // frame, but one taken for another button would show the wrong one)
            for scale in [1.0, 1.3] {
                let (c, r) = shown(name, scale, [110, 50, 120]);
                if let Some(n) = identify(&(&c).into(), r) {
                    assert_eq!(n, *name, "{name} at {scale}");
                    known += 1;
                }
            }
        }
        assert!(known >= PS5.len() * 2 * 8 / 10, "most recognised: {known}");
    }

    /// Real icons from saved frames: `VIT_ICON_SAMPLES` names a JSON list of [png path, name, x, y, w, h] (a name
    /// "BTN:<button word>" is the button row's icon). How many are recognised, and as what.
    #[test]
    #[ignore]
    fn recognises_icons_from_saved_frames() {
        let Ok(path) = std::env::var("VIT_ICON_SAMPLES") else { return };
        let list: Vec<(String, String, u32, u32, u32, u32)> = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let btn = |w: &str| match w {
            "BACK" | "EXIT" | "CLOSE MENU" => "CIRCLE",
            "SELECT" => "X",
            "PHOTO STUDIO" | "RESTORE DEFAULT" => "SQUARE",
            "RESTORE ALL" => "TRIANGLE",
            "SCROLL" => "RIGHT-STICK",
            "PAUSE PREVIEW" => "R3",
            _ => "",
        };
        let (mut right, mut wrong, mut none) = (0, 0, 0);
        let mut frames: HashMap<String, crate::ocr::Crop> = HashMap::new();
        for (png, name, x, y, w, h) in list {
            let want = name.strip_prefix("BTN:").map(btn).unwrap_or(name.as_str()).to_string();
            if want.is_empty() || want == "ICON" {
                continue;
            }
            let c = frames.entry(png.clone()).or_insert_with(|| {
                let img = image::open(&png).unwrap().to_rgba8();
                crate::ocr::Crop { w: img.width(), h: img.height(), rgba: img.into_raw() }
            });
            if std::env::var("VIT_ICON_DEBUG").is_ok() {
                eprintln!("WANT {want}");
            }
            match identify(&(&*c).into(), (x, y, w, h)) {
                Some(n) if n == want => right += 1,
                Some(n) => {
                    wrong += 1;
                    println!("WRONG {want} read as {n}: {png} {x},{y} {w}x{h}");
                }
                None => none += 1,
            }
        }
        println!("right {right}, wrong {wrong}, not recognised {none}");
    }

    #[test]
    fn a_plain_area_or_a_letter_is_no_icon() {
        let (c, _) = shown("X", 1.0, [110, 50, 120]);
        assert_eq!(identify(&(&c).into(), (0, 0, 9, 9)), None, "background only");
    }
}
