//! English text recognition, plus detection of words drawn in a colour (game highlights) instead
//! of white. The OCR engine itself is the host's (`TextEngine`): on the desktop the platform's
//! (Windows OCR, Apple Vision, Tesseract), on a phone ML Kit or Apple Vision. It only reads words in
//! a prepared dark-on-light picture; everything around that (the passes, which reading is kept,
//! speaker names, icons, colours) is here.

use anyhow::Result;

/// The host's text recognizer: the words in a dark-on-light picture (BGRA, 4 bytes per pixel, rows
/// `w * 4` bytes; grey, so the channel order doesn't matter), with their line numbers and boxes in
/// that picture's pixels. Called several times per area and frame (see `Ocr::recognize_checked`).
pub trait TextEngine {
    fn read(&self, bgra: &[u8], w: u32, h: u32) -> Result<Vec<RawWord>>;
    /// The largest picture side the engine accepts (bigger pictures are halved until they fit).
    fn max_dim(&self) -> u32 {
        2600
    }
    /// The picture's lines of text, each as it lies (see `TextLine`): for text that isn't level — GTA VI's
    /// phone, held tilted in the hand. The engine's own where it knows a line's corners (Apple Vision); else
    /// from `read`'s words (`lines_of`).
    fn read_lines(&self, bgra: &[u8], w: u32, h: u32) -> Result<Vec<TextLine>> {
        Ok(lines_of(self.read(bgra, w, h)?))
    }
}

/// A reading's words as its lines, each as it lies: a line runs from its first word's middle to its last
/// one's (Windows' reader and Tesseract give each word its own box: along a tilted line they step down or up),
/// and its text is as tall as its words' boxes are, most of them (the box round a whole tilted line is twice
/// as tall as its letters).
pub fn lines_of(words: Vec<RawWord>) -> Vec<TextLine> {
    let mut by_line: std::collections::BTreeMap<u32, Vec<RawWord>> = Default::default();
    for word in words {
        by_line.entry(word.line).or_default().push(word);
    }
    by_line
        .into_values()
        .map(|mut ws| {
            ws.sort_by(|a, b| a.x.total_cmp(&b.x));
            let (first, last) = (&ws[0], &ws[ws.len() - 1]);
            let mut hs: Vec<f64> = ws.iter().map(|w| w.h).collect();
            hs.sort_by(f64::total_cmp);
            TextLine {
                t: ws.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" "),
                l: [first.x, first.y + first.h / 2.0],
                r: [last.x + last.w, last.y + last.h / 2.0],
                h: hs[hs.len() / 2],
            }
        })
        .collect()
}

/// A word as the engine reports it, in the (prepared) image's pixels.
pub struct RawWord {
    pub line: u32,
    pub text: String,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// A line of text as it lies in a picture: its words, the middle of its left and right ends and its text's
/// height — the picture's pixels, from its top left.
#[derive(Clone, Debug)]
pub struct TextLine {
    pub t: String,
    pub l: [f64; 2],
    pub r: [f64; 2],
    pub h: f64,
}

/// An RGBA image crop.
pub struct Crop {
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
}

impl Crop {
    /// Cut a rectangle given as fractions (0..1) out of a full RGBA frame.
    pub fn from_frame(rgba: &[u8], fw: u32, fh: u32, x: f64, y: f64, w: f64, h: f64) -> Option<Crop> {
        let x0 = ((x.clamp(0.0, 1.0)) * fw as f64) as u32;
        let y0 = ((y.clamp(0.0, 1.0)) * fh as f64) as u32;
        let x1 = (((x + w).clamp(0.0, 1.0)) * fw as f64) as u32;
        let y1 = (((y + h).clamp(0.0, 1.0)) * fh as f64) as u32;
        let (cw, ch) = (x1.saturating_sub(x0), y1.saturating_sub(y0));
        if cw < 8 || ch < 8 {
            return None;
        }
        let mut out = Vec::with_capacity((cw * ch * 4) as usize);
        for row in y0..y1 {
            let start = ((row * fw + x0) * 4) as usize;
            out.extend_from_slice(&rgba[start..start + (cw * 4) as usize]);
        }
        Some(Crop { w: cw, h: ch, rgba: out })
    }

    pub fn px(&self, x: u32, y: u32) -> (i32, i32, i32) {
        let i = ((y * self.w + x) * 4) as usize;
        (self.rgba[i] as i32, self.rgba[i + 1] as i32, self.rgba[i + 2] as i32)
    }
}

#[derive(Clone)]
pub struct Word {
    pub text: String,
    /// Box in crop pixels: x, y, w, h.
    pub rect: (u32, u32, u32, u32),
    /// Which text line the OCR put this word on (0 = first line).
    pub line: u32,
}

pub struct Ocr {
    engine: Box<dyn TextEngine>,
}

impl Ocr {
    /// Reading with the host's recognizer.
    pub fn with_engine(engine: Box<dyn TextEngine>) -> Self {
        Self { engine }
    }

    /// Prepare for OCR: take each pixel's brightest channel (so coloured words are as bright as white
    /// ones), invert to dark-on-light, and upscale tiny text. Returns BGRA bytes, size, and the factor
    /// that maps OCR-image pixels back to crop pixels.
    fn preprocess(crop: &Crop, mode: Prep, max_dim: u32) -> (Vec<u8>, u32, u32, f64) {
        let scale = if crop.h < 60 { 2 } else { 1 };
        let (w, h) = (crop.w * scale, crop.h * scale);
        let inside = matches!(mode, Prep::Outlined).then(|| outlined_letters(crop));
        let mut out = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                let v = match &inside {
                    Some(m) => if m[((y / scale) * crop.w + x / scale) as usize] { 0 } else { 255 },
                    None => {
                        let (r, g, b) = crop.px(x / scale, y / scale);
                        255 - prep_value(mode, r, g, b)
                    }
                };
                out.extend_from_slice(&[v, v, v, 255]);
            }
        }
        if w > max_dim || h > max_dim {
            // Very wide crops: halve until it fits (rare; OCR limit is ~2600 px).
            let mut img = image::RgbaImage::from_raw(w, h, out).unwrap();
            while img.width() > max_dim || img.height() > max_dim {
                img = image::imageops::resize(&img, img.width() / 2, img.height() / 2, image::imageops::FilterType::Triangle);
            }
            let (nw, nh) = img.dimensions();
            return (img.into_raw(), nw, nh, crop.w as f64 / nw as f64);
        }
        (out, w, h, 1.0 / scale as f64)
    }

    /// Read the crop twice — once as is (best for outlined subtitles), once keeping only near-white
    /// and strongly coloured pixels (best when a lamp or sky shows through GTA's see-through
    /// instructions box) — and keep whichever reading has more real-looking words. A reading can
    /// look clean yet have silently lost words, so both always run.
    pub fn recognize(&self, crop: &Crop) -> Result<Vec<Word>> {
        Ok(self.recognize_checked(crop)?.0)
    }

    /// Snapshots: every word on a whole frame, read both ways (bright text; white or coloured text),
    /// the second reading adding the words the first one missed.
    pub fn read_screen(&self, crop: &Crop) -> Result<Vec<Word>> {
        let mut words = drop_junk(self.recognize_with(crop, Prep::Brightest)?);
        let overlaps = |a: (u32, u32, u32, u32), b: (u32, u32, u32, u32)| {
            let w = (a.0 + a.2).min(b.0 + b.2) as i64 - a.0.max(b.0) as i64;
            let h = (a.1 + a.3).min(b.1 + b.3) as i64 - a.1.max(b.1) as i64;
            w > 0 && h > 0 && (w * h) as f64 > 0.3 * (a.2 as f64 * a.3 as f64).min(b.2 as f64 * b.3 as f64)
        };
        for w in drop_junk(self.recognize_with(crop, Prep::WhiteOrColour)?) {
            if !words.iter().any(|o| overlaps(o.rect, w.rect)) {
                words.push(w);
            }
        }
        Ok(words)
    }

    /// `recognize`, and whether the reading is sure: the two ways of reading agree (or one only
    /// missed a line the other has, like a grey speaker's). Not sure: text still animating in —
    /// GTA VI's instructions start with a white glow around their outline that fades away.
    pub fn recognize_checked(&self, crop: &Crop) -> Result<(Vec<Word>, bool)> {
        let (words, sure, first) = self.recognize_passes(crop)?;
        Ok((with_speaker_name(words, &first), sure))
    }

    /// The reading taken, whether it is sure, and the first pass's reading (see `with_speaker_name`).
    fn recognize_passes(&self, crop: &Crop) -> Result<(Vec<Word>, bool, Vec<Word>)> {
        let first = drop_junk(self.recognize_with(crop, Prep::Brightest)?);
        let second = drop_junk(self.recognize_with(crop, Prep::WhiteOrColour)?);
        let score = |w: &[Word]| {
            let (g, b) = quality(w);
            g - 2 * b
        };
        if std::env::var_os("VI_TRANSLATOR_DEBUG").is_some() {
            let txt = |w: &[Word]| w.iter().map(|x| x.text.as_str()).collect::<Vec<_>>().join(" ");
            eprintln!("[ocr] pass1 {:?} (score {}) | pass2 {:?} (score {})", txt(&first), score(&first), txt(&second), score(&second));
        }
        // The second reading must clearly win: a 1-character edge is usually a misread ("time" for "the").
        // (a pass that read nothing never replaces one that read something: "nothing" scores 0, which
        // can beat a junky but useful read such as "gussian FaiLedj" for the Pricedown title)
        let txt = |w: &[Word]| w.iter().map(|x| x.text.as_str()).collect::<Vec<_>>().join(" ");
        // the two readings disagree, or the one taken looks wrong ("to aim uold", "aim7ät")
        let unsure = !agree(&txt(&first), &txt(&second));
        let (t1, t2) = (txt(&first), txt(&second));
        // (the first read nothing, the second read words: a rough reading of what's there beats nothing —
        // white letters over sunlit gravel, "Drive t0'the prisgn": as nothing, the line on screen went and
        // came back every few frames, its translation flickering)
        let rough = first.is_empty() && quality(&second).0 >= 6 && looks_like_words(&txt(&second));
        let took_second = !second.is_empty() && (score(&second) >= score(&first) + 3 || rough);
        let first_kept = first.clone();
        let second_kept = second.clone();
        let best = if took_second { second } else { first };
        // Outlined letters over a bright floor (GTA VI's instructions under the subtitles): read once
        // more with only the letters inside their outline.
        // (and when neither pass read anything: white letters in a thin outline over sunlit sand or
        // concrete are as bright as the ground, so the plain passes see no text at all)
        if unsure || best.is_empty() || quality(&best).1 > 0 || words_out_of_order(&best) {
            let third = drop_junk(self.recognize_with(crop, Prep::Outlined)?);
            if std::env::var_os("VI_TRANSLATOR_DEBUG").is_some() {
                eprintln!("[ocr] pass3 {:?} (score {})", txt(&third), score(&third));
            }
            // (and only with as many words the other passes read too: "tix myeleg" scores as plain
            // letters, but no other pass saw those words)
            let t3 = txt(&third);
            let best_others = if took_second { [t1.as_str(), &t3] } else { [t2.as_str(), &t3] }.join(" ");
            let third_ok = agreed(&third, &[t1.as_str(), &t2].join(" ")) >= agreed(&best, &best_others);
            // (a clear edge, like the second pass: "lget's" beat "Let's" by one point)
            if !third.is_empty() && score(&third) >= score(&best) + 2 && !words_out_of_order(&third) && third_ok {
                return Ok((third, false, first_kept));
            }
            // Half the line over sunlit ground, half over shade: each pass reads its own half
            // ("Good that good" | "You don't fix it too good"). Clean words the others read where
            // the reading taken has a gap go in.
            let filled = fill_gaps(&best, &[&first_kept, &third, if took_second { &[][..] } else { &second_kept }]);
            if filled.len() > best.len() {
                return Ok((filled, false, first_kept));
            }
        }
        Ok((best, !unsure, first_kept))
    }

    /// The crop's lines of text, each as it lies (`TextEngine::read_lines`), in the crop's pixels.
    pub fn lines_with(&self, crop: &Crop, mode: Prep) -> Result<Vec<TextLine>> {
        let (bgra, w, h, back) = Self::preprocess(crop, mode, self.engine.max_dim());
        let scaled = |p: [f64; 2]| [p[0] * back, p[1] * back];
        Ok(self.engine.read_lines(&bgra, w, h)?.into_iter().map(|l| TextLine { t: l.t, l: scaled(l.l), r: scaled(l.r), h: l.h * back }).collect())
    }

    pub fn recognize_with(&self, crop: &Crop, mode: Prep) -> Result<Vec<Word>> {
        let (bgra, w, h, back) = Self::preprocess(crop, mode, self.engine.max_dim());
        Ok(self
            .engine
            .read(&bgra, w, h)?
            .into_iter()
            .map(|r| Word {
                line: r.line,
                text: r.text,
                rect: ((r.x * back) as u32, (r.y * back) as u32, (r.w * back).ceil() as u32 + 1, (r.h * back).ceil() as u32 + 1),
            })
            .collect())
    }
}

/// Two words OCR glued with a dot it saw in the gap: "my.leg" -> "my leg" (both sides at least two
/// lower-case letters, so "e.g." and "U.S." stay).
fn split_glued(w: &str) -> String {
    if let Some((a, b)) = w.split_once('.') {
        let low = |t: &str, n: usize| t.chars().take_while(|c| c.is_ascii_lowercase()).count() >= n;
        let a_tail: String = a.chars().rev().take_while(|c| c.is_ascii_lowercase()).collect();
        if a_tail.len() >= 2 && a_tail.len() == a.trim_start_matches(|c: char| !c.is_alphanumeric()).len() && low(b, 2) && !b.contains('.') {
            return format!("{a} {b}");
        }
    }
    w.to_string()
}

/// GTA VI's speaker names ("Dobbs: Not to be annoying…"): a pale colour that only the first pass
/// (brightest channel) reads when it sits over something bright. When that pass starts a line with
/// "Name:" left of where the reading taken starts that line, the name goes in front of it.
fn with_speaker_name(mut words: Vec<Word>, first: &[Word]) -> Vec<Word> {
    let Some((n, name, (nx, ny, nw, nh))) = name_words(first) else { return words };
    if name_words(&words).is_some() {
        return words;
    }
    // the same name read without its colon ("Cal I will"): give it back
    let bare: Vec<String> = name.trim_end_matches(':').split_whitespace().map(String::from).collect();
    let line: Vec<(u32, String)> = top_line(&words).iter().map(|w| (w.rect.0, w.text.clone())).collect();
    if line.len() > n && line[..n].iter().map(|(_, t)| t).eq(bare.iter()) && line[0].0 < nx + nw {
        let x = line[n - 1].0;
        if let Some(w) = words.iter_mut().find(|w| w.rect.0 == x && w.text == bare[n - 1]) {
            w.text.push(':');
        }
        return words;
    }
    let cy = ny + nh / 2;
    // the words of the same line, all to the right of the name
    let same_line: Vec<&Word> = words.iter().filter(|w| w.rect.1 <= cy && cy <= w.rect.1 + w.rect.3).collect();
    if same_line.is_empty() || same_line.iter().any(|w| w.rect.0 < nx + nw) {
        return words;
    }
    let line = same_line[0].line;
    let at = words.iter().position(|w| w.line == line).unwrap_or(0);
    // (each word of the name, from the first pass, in front)
    let named: Vec<Word> = top_line(first)[..n].iter().map(|w| Word { line, text: w.text.clone(), rect: w.rect }).collect();
    for (k, w) in named.into_iter().enumerate() {
        words.insert(at + k, w);
    }
    words
}

/// How many words at the start of a line make a speaker name: "Dobbs:" (1) or "Real Dimez:",
/// "Officer Smith:" (2) — capitalised words, the last ending with a colon.
pub fn name_len(tokens: &[&str]) -> Option<usize> {
    let first = tokens.first()?;
    if speaker_name(first).is_some() {
        return Some(1);
    }
    let capital_word = first.chars().next().is_some_and(|c| c.is_ascii_uppercase())
        && first.chars().count() >= 2
        && first.chars().all(|c| c.is_alphabetic() || c == '\'' || c == '-' || c == '.');
    (capital_word && tokens.get(1).is_some_and(|t| speaker_name(t).is_some())).then_some(2)
}

/// A line starting with a speaker name: (the name with its colon, "Real Dimez:"; the rest of the line).
pub fn split_name(text: &str) -> Option<(String, &str)> {
    let tokens: Vec<&str> = text.split_whitespace().collect();
    let n = name_len(&tokens)?;
    let last = tokens[n - 1];
    let end = text.find(last)? + last.len();
    Some((tokens[..n].join(" "), text[end..].trim_start()))
}

/// The words of the top line, left to right (by position: the OCR can number two stacked lines alike).
fn top_line(words: &[Word]) -> Vec<&Word> {
    let Some(top) = words.iter().min_by_key(|w| w.rect.1 + w.rect.3 / 2) else { return Vec::new() };
    let (ty0, ty1) = (top.rect.1, top.rect.1 + top.rect.3);
    let mut line: Vec<&Word> = words.iter().filter(|w| (ty0..=ty1).contains(&(w.rect.1 + w.rect.3 / 2))).collect();
    line.sort_by_key(|w| w.rect.0);
    line
}

/// The speaker name at the start of the top line: (how many words, their text with the colon, their box).
fn name_words(words: &[Word]) -> Option<(usize, String, (u32, u32, u32, u32))> {
    let line = top_line(words);
    let n = name_len(&line.iter().map(|w| w.text.as_str()).collect::<Vec<_>>())?;
    let (x0, y0) = (line[..n].iter().map(|w| w.rect.0).min()?, line[..n].iter().map(|w| w.rect.1).min()?);
    let x1 = line[..n].iter().map(|w| w.rect.0 + w.rect.2).max()?;
    let y1 = line[..n].iter().map(|w| w.rect.1 + w.rect.3).max()?;
    Some((n, line[..n].iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" "), (x0, y0, x1 - x0, y1 - y0)))
}

/// The text block (crop pixels) of a reading, words and icons together; None without a crop or words.
pub fn text_with_icons_block(c: Option<&Crop>, words: &[Word]) -> Option<(u32, u32, u32, u32)> {
    c.and_then(|c| text_with_icons(c, words).2)
}

/// The game's own box around a text block (GTA's see-through help box): its edges found where the
/// brightness leaves the box's own level, scanning out from the text (crop pixels: x, y, w, h). The
/// level inside is the median brightness of the block's columns (letters are a minority of each
/// column); an edge is the first of 3 columns (rows) in a row that differ from it by 14 or more,
/// within 3 line heights of the text. None when an edge isn't clear (no box, or busy scenery).
pub fn box_around(c: &Crop, block: (u32, u32, u32, u32), line_h: u32) -> Option<(u32, u32, u32, u32)> {
    let (bx, by, bw, bh) = block;
    if bw == 0 || bh == 0 || c.w == 0 || c.h == 0 {
        return None;
    }
    let lum = |x: u32, y: u32| {
        let i = ((y * c.w + x) * 4) as usize;
        c.rgba[i].max(c.rgba[i + 1]).max(c.rgba[i + 2]) as i32
    };
    let median = |mut v: Vec<i32>| {
        v.sort_unstable();
        v[v.len() / 2]
    };
    let (y0, y1) = (by, (by + bh).min(c.h));
    let col = |x: u32| median((y0..y1).map(|y| lum(x, y)).collect());
    let (x0, x1) = (bx, (bx + bw).min(c.w));
    let reach = 3 * line_h.max(4);
    // walks out from the text: the box's own level is taken right next to it (its padding; the box
    // is lighter over a bright sky), and the edge is the first of 3 values in a row 14 or more off it
    let edge = |steps: Vec<u32>, at: &dyn Fn(u32) -> i32| -> Option<u32> {
        if steps.len() < 8 {
            return None;
        }
        let inside = median(steps[1..4].iter().map(|&s| at(s)).collect());
        let mut run = 0;
        for (i, &s) in steps.iter().enumerate().skip(1) {
            if (at(s) - inside).abs() >= 14 {
                run += 1;
                if run == 3 {
                    return Some(steps[i - 2]);
                }
            } else {
                run = 0;
            }
        }
        None
    };
    // The left edge must be clear (GTA VI's box has a darker rim there): no box, no edge — credits or
    // scenery read as text have none. The game pads the text alike on both sides, so a right or bottom
    // edge lost in dark scenery (a step of only ~15) is taken from that padding.
    let left = edge((x0.saturating_sub(reach)..x0).rev().collect(), &col)?; // first column outside, going left
    let lx = left + 1;
    let pad = x0.saturating_sub(lx);
    let lh = line_h.max(4) as f64;
    if !(0.3 * lh..=2.5 * lh).contains(&(pad as f64)) {
        return None; // not a box's padding
    }
    let rx = edge((x1..(x1 + reach).min(c.w)).collect(), &col)
        .filter(|&r| r.saturating_sub(x1) <= 2 * pad + 4)
        .unwrap_or((x1 + pad).min(c.w));
    // To the pixel: an edge is where the brightness crosses halfway from the box's to the scene's (the
    // step above takes the first column well off the box's level, a pixel or two out: the box's rim
    // and its soft edge went outside it, and its translation's box came out narrower on the left).
    let lx = {
        let (out, inn) = (col(lx.saturating_sub(4)), col((lx + 4).min(c.w - 1)));
        (lx.saturating_sub(3)..=(lx + 3).min(c.w - 1)).find(|&x| (col(x) - inn).abs() < (col(x) - out).abs()).unwrap_or(lx)
    };
    let rx = {
        let (out, inn) = (col((rx + 3).min(c.w - 1)), col(rx.saturating_sub(5)));
        (rx.saturating_sub(4)..(rx + 3).min(c.w)).rev().find(|&x| (col(x) - inn).abs() < (col(x) - out).abs()).map(|x| x + 1).unwrap_or(rx)
    };
    let row = |y: u32| median((lx..rx).step_by(2).map(|x| lum(x, y)).collect());
    let bottom = edge((y1..(y1 + reach).min(c.h)).collect(), &row)
        .filter(|&b| b.saturating_sub(y1) <= 2 * pad + 4)
        .unwrap_or((y1 + pad * 3 / 5).min(c.h));
    let top = edge((y0.saturating_sub(reach)..y0).rev().collect(), &row).map(|t| t + 1).unwrap_or(y0.saturating_sub(pad * 3 / 5));
    (rx > lx && bottom > top).then_some((lx, top, rx - lx, bottom - top))
}

/// "Dobbs:" -> "Dobbs": a capitalised name of letters (a ' or - inside) ending with a colon.
pub fn speaker_name(word: &str) -> Option<&str> {
    let n = word.strip_suffix(':')?;
    let mut ch = n.chars();
    let ok = ch.next().is_some_and(|c| c.is_ascii_uppercase())
        && n.chars().count() >= 2
        && n.chars().all(|c| c.is_alphabetic() || c == '\'' || c == '-' || c == '.');
    ok.then_some(n)
}

/// Letters in the words of `words` that `other` (another reading of the same text) has too.
fn agreed(words: &[Word], other: &str) -> usize {
    let norm = |t: &str| t.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase();
    let seen: std::collections::HashSet<String> = other.split_whitespace().map(norm).filter(|t| !t.is_empty()).collect();
    words.iter().map(|w| norm(&w.text)).filter(|t| seen.contains(t)).map(|t| t.chars().count()).sum()
}

/// Areas that read only centred text (subtitles): the words of the centred part. Another block of text at
/// the side of the area (a garage's or shop's panel on the right: "STRENGTH", "RATING") is dropped, so it
/// doesn't push the subtitle off-centre and lose it. Words are grouped in columns (a gap wider than three
/// text heights splits them); while the columns together are off-centre, the outermost one on that side
/// goes; a column whose middle is far to the side goes first. Nothing is left when even one column is
/// off-centre (a menu's "Browse" at the bottom right).
/// `crop_x` is where the crop starts in the frame and `frame_w` the frame's width, in pixels.
pub fn centered_words(words: &[Word], crop_x: f64, frame_w: f64) -> Vec<Word> {
    if words.is_empty() || frame_w <= 0.0 {
        return Vec::new();
    }
    let mut hs: Vec<u32> = words.iter().map(|w| w.rect.3).collect();
    hs.sort_unstable();
    let gap = (hs[hs.len() / 2] as f64 * 3.0).max(24.0);
    // columns: runs of words (by x) with no gap wider than `gap` between them
    let mut order: Vec<usize> = (0..words.len()).collect();
    order.sort_by_key(|&i| words[i].rect.0);
    let mut cols: Vec<(f64, f64, Vec<usize>)> = Vec::new();
    for i in order {
        let (x0, x1) = (words[i].rect.0 as f64, (words[i].rect.0 + words[i].rect.2) as f64);
        match cols.last_mut() {
            Some(c) if x0 <= c.1 + gap => {
                c.1 = c.1.max(x1);
                c.2.push(i);
            }
            _ => cols.push((x0, x1, vec![i])),
        }
    }
    // side panels first: with more than one column, a column whose own middle is far from the
    // picture's (a quarter of its width) isn't part of a subtitle, even when a panel on each side
    // balances out in the middle (a garage: the upgrade's name on the left, the car's stats on the right)
    if cols.len() > 1 {
        cols.retain(|c| ((crop_x + (c.0 + c.1) / 2.0) / frame_w - 0.5).abs() <= 0.25);
        if cols.is_empty() {
            return Vec::new();
        }
    }
    loop {
        let x0 = cols.first().map_or(0.0, |c| c.0);
        let x1 = cols.iter().map(|c| c.1).fold(0.0, f64::max);
        let cx = (crop_x + (x0 + x1) / 2.0) / frame_w;
        if (cx - 0.5).abs() <= 0.04 {
            let keep: std::collections::HashSet<usize> = cols.iter().flat_map(|c| c.2.iter().copied()).collect();
            return words.iter().enumerate().filter(|(i, _)| keep.contains(i)).map(|(_, w)| w.clone()).collect();
        }
        if cols.len() <= 1 {
            return Vec::new();
        }
        if cx > 0.5 {
            cols.pop();
        } else {
            cols.remove(0);
        }
    }
}

/// Join words and drop OCR noise (almost no letters).
pub fn clean(words: &[Word]) -> String {
    let text = words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ");
    let text = text.split_whitespace().map(split_glued).collect::<Vec<_>>().join(" ");
    let text = drop_icon_misreads(&text);
    let letters = text.chars().filter(|c| c.is_alphabetic()).count();
    let total = text.chars().count().max(1);
    if letters >= 4 && letters as f64 / total as f64 > 0.5 { fix_contractions(&text) } else { String::new() }
}

/// Real words made of the letters an icon is misread as: alone at a line's start they're kept.
const ICON_LIKE_WORDS: &[&str] = &["coco", "cocoa", "cocoon", "coon", "coop", "goo", "goon", "goop", "noon", "poo", "poop", "pogo", "capo", "agog"];

/// The icon before a subtitle (a music note on the radio's lyrics, a TV or a speaker off screen) read
/// as letters: "0000 to do today?", "cocoa pooo and the smoke…", "cocoa nooc Especially you." Leading
/// words made only of the letters such a round icon reads as (o 0 c n p g a), at least three long with
/// two o's or zeros, are dropped when one of them isn't a real word, so the line finds its translation
/// and doesn't flip between the clean reading and the one with the prefix.
pub fn drop_icon_misreads(text: &str) -> String {
    let icon_like = |w: &str| {
        let n = w.chars().count();
        (3..=6).contains(&n)
            && w.chars().all(|c| matches!(c, 'o' | '0' | 'c' | 'n' | 'p' | 'g' | 'a'))
            && w.chars().filter(|&c| c == 'o' || c == '0').count() >= 2
    };
    let words: Vec<&str> = text.split_whitespace().collect();
    let run = words.iter().take_while(|w| icon_like(w)).count();
    // (the run is the whole line: nothing after the icon, nothing to keep)
    if run == 0 || run == words.len() || words[..run].iter().all(|w| ICON_LIKE_WORDS.contains(w)) {
        return text.to_string();
    }
    words[run..].join(" ")
}

/// The text of the game's box that's always in one place (GTA VI's help, top left), told from a sign
/// or a shop's name by where it is and how it's laid out, not by the box's edges (its see-through dark
/// panel has none over a dark street): its first line starts at the box's top-left corner (a margin
/// in), each next line one line pitch under the one before and starting at the same left edge (or a
/// button icon's width in), none past the box's widest. `anchor`: the box's left, top and widest, in
/// the crop's pixels. (the box around the lines, its words in reading order), or None.
pub fn in_anchored_box(words: &[Word], anchor: (u32, u32, u32)) -> Option<((u32, u32, u32, u32), Vec<Word>)> {
    if words.is_empty() {
        return None;
    }
    let (left, top, widest) = anchor;
    let mut hs: Vec<u32> = words.iter().map(|w| w.rect.3).collect();
    hs.sort_unstable();
    let lh = hs[(hs.len() * 9 / 10).min(hs.len() - 1)].max(6);
    // visual lines, top first
    let mut order: Vec<usize> = (0..words.len()).collect();
    order.sort_by_key(|&i| words[i].rect.1 + words[i].rect.3 / 2);
    let mut lines: Vec<(u32, Vec<usize>)> = Vec::new(); // (first word's middle, words)
    for i in order {
        let m = words[i].rect.1 + words[i].rect.3 / 2;
        match lines.last_mut() {
            Some(l) if m <= l.0 + lh / 2 => l.1.push(i),
            _ => lines.push((m, vec![i])),
        }
    }
    let span = |idx: &[usize]| {
        let x0 = idx.iter().map(|&i| words[i].rect.0).min().unwrap_or(0);
        let x1 = idx.iter().map(|&i| words[i].rect.0 + words[i].rect.2).max().unwrap_or(0);
        let y0 = idx.iter().map(|&i| words[i].rect.1).min().unwrap_or(0);
        let y1 = idx.iter().map(|&i| words[i].rect.1 + words[i].rect.3).max().unwrap_or(0);
        (x0, x1, y0, y1)
    };
    // (a line starts at the box's left, a margin in: up to an icon's width further at the most)
    let starts_left = |x0: u32| x0 >= left && x0 <= left + 3 * lh;
    let within = |x1: u32| x1 <= left + widest + lh;
    let first = lines.iter().position(|(_, idx)| {
        let (x0, x1, y0, _) = span(idx);
        y0 >= top && y0 <= top + 2 * lh && starts_left(x0) && within(x1)
    })?;
    let mut kept: Vec<(u32, Vec<usize>)> = vec![lines[first].clone()];
    for l in &lines[first + 1..] {
        let (x0, x1, _, _) = span(&l.1);
        let pitch = l.0 - kept.last().expect("one").0;
        if !(lh..=2 * lh).contains(&pitch) || !starts_left(x0) || !within(x1) {
            break;
        }
        kept.push(l.clone());
    }
    let mut out = Vec::new();
    let (mut right, mut bottom) = (0, 0);
    for (n, (_, idx)) in kept.iter().enumerate() {
        let (_, x1, _, y1) = span(idx);
        right = right.max(x1);
        bottom = bottom.max(y1);
        let mut idx = idx.clone();
        idx.sort_by_key(|&i| words[i].rect.0);
        out.extend(idx.into_iter().map(|i| Word { line: n as u32, ..words[i].clone() }));
    }
    // (the box's right edge a line height past the text's, its bottom about half a line under it:
    // measured where its edges show)
    Some(((left, top, (right + lh).saturating_sub(left).min(widest), (bottom + lh * 9 / 20).saturating_sub(top)), out))
}

/// The words inside the game's own box (GTA VI's help box), when there is one: (the box, its words,
/// whether its edges were found in this picture). Text around it in the area (a shop's
/// "SUPERMARKET" above it, a sign under it) is left out. With that text, the words' block reaches
/// past the box and its edges may not be found: then each group of lines on its own (lines a line
/// pitch or less apart), the biggest first.
///
/// `known`: where the box was found last (it doesn't move: GTA VI's help is always there, as wide).
/// Its see-through dark panel over a dark street has no edges to find, and its text would be lost:
/// then the lines in the known box's left and right edges, from its top down while they follow one
/// another, are its text. And edges found elsewhere than the known box (a window or a sign's frame
/// in the text's block) give way to it.
pub fn in_game_box(c: &Crop, words: &[Word], known: Option<(u32, u32, u32, u32)>) -> Option<((u32, u32, u32, u32), Vec<Word>, bool)> {
    let found = in_game_box_found(c, words);
    let Some(k) = known else { return found.map(|(b, w)| (b, w, true)) };
    let mut hs: Vec<u32> = words.iter().map(|w| w.rect.3).collect();
    hs.sort_unstable();
    let mh = hs.get(hs.len() / 2).copied().unwrap_or(20).max(4);
    let like_known = |b: (u32, u32, u32, u32)| b.0.abs_diff(k.0) <= 2 * mh && b.2.abs_diff(k.2) <= 4 * mh && b.1.abs_diff(k.1) <= 2 * mh;
    if let Some((b, w)) = found.as_ref().filter(|(b, _)| like_known(*b)) {
        return Some((*b, w.clone(), true));
    }
    // the lines in the known box: from its top down while one follows the other
    let mut cand: Vec<&Word> = words.iter().filter(|w| (k.0..k.0 + k.2).contains(&(w.rect.0 + w.rect.2 / 2)) && w.rect.1 + w.rect.3 / 2 >= k.1).collect();
    cand.sort_by_key(|w| w.rect.1 + w.rect.3 / 2);
    let mut kept: Vec<Word> = Vec::new();
    let mut last = None::<u32>;
    for w in cand {
        let m = w.rect.1 + w.rect.3 / 2;
        let ok = match last {
            None => m <= k.1 + 3 * mh, // (the text starts at the box's top)
            Some(l) => m <= l + 2 * mh,
        };
        if !ok {
            break;
        }
        last = Some(last.map_or(m, |l| l.max(m)));
        kept.push(w.clone());
    }
    if kept.is_empty() {
        return found.map(|(b, w)| (b, w, true));
    }
    // in reading order: line by line (middles within half a line), each left to right
    let (mut line, mut first) = (0u32, None::<u32>);
    for w in kept.iter_mut() {
        let m = w.rect.1 + w.rect.3 / 2;
        if first.is_some_and(|f| m > f + mh / 2) {
            line += 1;
            first = None;
        }
        first.get_or_insert(m);
        w.line = line;
    }
    kept.sort_by_key(|w| (w.line, w.rect.0));
    let bottom = kept.iter().map(|w| w.rect.1 + w.rect.3).max().unwrap_or(k.1) + mh;
    Some(((k.0, k.1, k.2, bottom.saturating_sub(k.1)), kept, false))
}

fn in_game_box_found(c: &Crop, words: &[Word]) -> Option<((u32, u32, u32, u32), Vec<Word>)> {
    if words.is_empty() {
        return None;
    }
    let mut hs: Vec<u32> = words.iter().map(|w| w.rect.3).collect();
    hs.sort_unstable();
    let mh = hs[hs.len() / 2].max(4);
    let inside = |b: (u32, u32, u32, u32)| -> Vec<Word> {
        words
            .iter()
            .filter(|w| {
                let (cx, cy) = (w.rect.0 + w.rect.2 / 2, w.rect.1 + w.rect.3 / 2);
                cx >= b.0 && cx < b.0 + b.2 && cy >= b.1 && cy < b.1 + b.3
            })
            .cloned()
            .collect()
    };
    let found = |ws: &[Word]| text_with_icons(c, ws).2.and_then(|blk| box_around(c, blk, mh));
    if let Some(b) = found(words) {
        let kept = inside(b);
        if !kept.is_empty() {
            return Some((b, kept));
        }
    }
    // groups of lines, top to bottom: a line joins the group above when it's at most 2 line heights under it
    let mut order: Vec<usize> = (0..words.len()).collect();
    order.sort_by_key(|&i| words[i].rect.1 + words[i].rect.3 / 2);
    let mut groups: Vec<(u32, Vec<usize>)> = Vec::new(); // (the lowest middle, words)
    for i in order {
        let m = words[i].rect.1 + words[i].rect.3 / 2;
        match groups.last_mut() {
            Some(g) if m <= g.0 + 2 * mh => {
                g.0 = g.0.max(m);
                g.1.push(i);
            }
            _ => groups.push((m, vec![i])),
        }
    }
    if groups.len() < 2 {
        return None;
    }
    groups.sort_by_key(|g| std::cmp::Reverse(g.1.len()));
    for (_, idx) in groups {
        let ws: Vec<Word> = idx.iter().map(|&i| words[i].clone()).collect();
        if let Some(b) = found(&ws) {
            let kept = inside(b);
            if !kept.is_empty() {
                return Some((b, kept));
            }
        }
    }
    None
}

/// Reads as one of the game's instructions: a sentence (it ends with . ! ? or …) or five words or more.
/// A shop menu's item where the help box would be ("Tactical Rifles", "Curated Liveries") is neither.
pub fn reads_as_sentences(text: &str) -> bool {
    let t = text.trim_end_matches(|c: char| matches!(c, '"' | '\'' | ')') || c.is_whitespace());
    t.ends_with(['.', '!', '?', '…']) || t.split_whitespace().filter(|w| w.chars().any(|c| c.is_alphabetic())).count() >= 5
}

/// Text in capitals for the most part (4 letters or more, nine in ten capitals): a sign or a
/// billboard ("BOAT STORAGE … CALL TODAY!", a road sign's "NORTH WEST"), never GTA VI's help, which
/// is written in sentences.
pub fn mostly_capitals(text: &str) -> bool {
    let letters: Vec<char> = text.chars().filter(|c| c.is_alphabetic()).collect();
    letters.len() >= 4 && letters.iter().filter(|c| c.is_uppercase()).count() * 10 >= letters.len() * 9
}

/// Lines stacked up from a bottom line (GTA VI's subtitles, up to six lines and more), in a crop
/// that ends where the area ends: the lowest line must have its middle within two line heights of
/// the crop's bottom (where line 1 is), and the lines above count while each sits one line pitch
/// over the one under it (GTA VI: 1.9 line heights; up to 2.4 between two speakers' blocks), centred
/// like it, and reads as words. Text with no line at the bottom (a car's "KEY LENTO POLICE" up in
/// the picture) is none of it: empty; a plate or a sign next to the lines is left out.
///
/// The line height is the tall words' (capitals and descenders: the 90th percentile), not the median
/// word's: short words ("on", "re") are a third smaller, and with lines of mostly short ones the pitch
/// between two lines of one block read as a gap now and then, dropping the top line from one frame
/// to the next.
pub fn bottom_stack(words: &[Word], crop_h: f64) -> Vec<Word> {
    if words.is_empty() {
        return Vec::new();
    }
    let mut hs: Vec<u32> = words.iter().map(|w| w.rect.3).collect();
    hs.sort_unstable();
    let lh = hs[(hs.len() * 9 / 10).min(hs.len() - 1)].max(6) as f64;
    // visual lines: words whose middles are within half a line height of the line's, bottom first
    let cy = |w: &Word| w.rect.1 as f64 + w.rect.3 as f64 / 2.0;
    let mut order: Vec<usize> = (0..words.len()).collect();
    order.sort_by(|&a, &b| cy(&words[b]).total_cmp(&cy(&words[a])));
    struct Line {
        mid: f64,
        n: f64,
        x0: f64,
        x1: f64,
        idx: Vec<usize>,
    }
    let mut lines: Vec<Line> = Vec::new();
    for i in order {
        let w = &words[i];
        let (x0, x1) = (w.rect.0 as f64, (w.rect.0 + w.rect.2) as f64);
        match lines.last_mut() {
            Some(l) if (l.mid - cy(w)).abs() < 0.5 * lh => {
                l.mid = (l.mid * l.n + cy(w)) / (l.n + 1.0);
                l.n += 1.0;
                l.x0 = l.x0.min(x0);
                l.x1 = l.x1.max(x1);
                l.idx.push(i);
            }
            _ => lines.push(Line { mid: cy(w), n: 1.0, x0, x1, idx: vec![i] }),
        }
    }
    // (a line of no words: a plate "LS5252", a stray mark)
    let text_of = |l: &Line| {
        let mut idx = l.idx.clone();
        idx.sort_by_key(|&i| words[i].rect.0);
        idx.iter().map(|&i| words[i].text.as_str()).collect::<Vec<_>>().join(" ")
    };
    let wordy = |t: &str| {
        let letters = |w: &str| w.trim_matches(|c: char| !c.is_alphabetic()).chars().filter(|c| c.is_alphabetic()).count();
        t.split_whitespace().any(|w| letters(w) >= 2 && w.chars().filter(|c| c.is_ascii_digit()).count() == 0) && looks_like_words(t)
    };
    lines.retain(|l| wordy(&text_of(l)));
    let Some(first) = lines.first() else { return Vec::new() };
    if first.mid < crop_h - 2.0 * lh {
        return Vec::new();
    }
    let centre = |l: &Line| (l.x0 + l.x1) / 2.0;
    let mut kept: Vec<Vec<usize>> = vec![first.idx.clone()];
    let (mut below, c) = (first.mid, centre(first));
    for l in &lines[1..] {
        let pitch = below - l.mid;
        // (closer than a line: a piece of the same line the OCR read a little higher, a tall misread
        // letter pulling it up; "Local: Just-just" next to "fuckin' get outta here")
        if pitch < 0.9 * lh {
            kept.last_mut().expect("one").extend(l.idx.iter().copied());
            continue;
        }
        // (one speaker's wrapped lines are 34 px apart at 1080p, two speakers' blocks 44: a plate or a
        // sign as close is no words, left out above)
        if !(1.15 * lh..=2.4 * lh).contains(&pitch) || (centre(l) - c).abs() > 3.0 * lh {
            break;
        }
        below = l.mid;
        kept.push(l.idx.clone());
    }
    // Top to bottom, each line left to right and numbered as seen: the OCR can read a pale coloured
    // name as a line of its own ("Coca's:" after "…C'mon!"), and `clean` takes the words in order.
    let mut out = Vec::new();
    // (a stray mark the OCR read before a speaker's name — "- Local:", "9 . Driver:", "S Tourist:" — made
    // the same line a new one, translated and shown again: what's before a name on its line goes when
    // it's no word)
    let stray = |w: &Word| w.text.chars().filter(|c| c.is_alphabetic()).count() <= 1 && !matches!(w.text.as_str(), "I" | "A" | "a");
    for (n, l) in kept.iter().rev().enumerate() {
        let mut idx = l.clone();
        idx.sort_by_key(|&i| words[i].rect.0);
        if let Some(j) = idx.iter().position(|&i| speaker_name(&words[i].text).is_some()) {
            if j > 0 && idx[..j].iter().all(|&i| stray(&words[i])) {
                idx.drain(..j);
            }
        }
        out.extend(idx.into_iter().map(|i| Word { line: n as u32, ..words[i].clone() }));
    }
    out
}

/// Stacked lines on their way up (GTA VI slides its lines up as a new one comes): for a moment no line
/// is where line 1 is, the lines above it a line's place up. Not the end of the lines: the reading keeps
/// what's up until line 1 is back (else the translation went for a moment, and came back).
pub fn stack_moving(words: &[Word], crop_h: f64) -> bool {
    if words.is_empty() {
        return false;
    }
    let mut hs: Vec<u32> = words.iter().map(|w| w.rect.3).collect();
    hs.sort_unstable();
    let lh = hs[(hs.len() * 9 / 10).min(hs.len() - 1)].max(6) as f64;
    let base = crop_h - 2.0 * lh;
    let lowest = words.iter().map(|w| w.rect.1 as f64 + w.rect.3 as f64 / 2.0).fold(f64::MIN, f64::max);
    lowest < base && lowest >= base - 2.6 * lh && looks_like_words(&clean(words))
}

/// GTA VI stacks subtitle lines: two people's, the one speaking now in white and the other in grey
/// ("Do not move." / "Idiot."), or two white ones, the second coming and going under the first.
/// Each is split off as its own part — lines that differ clearly in brightness, or a line that ends
/// a sentence — so each is translated (and remembered) on its own: (text, brightness relative to the
/// brightest part, 0..1]. A line that only wrapped stays one part. Fewer than two parts: empty.
pub fn speaker_lines(c: &Crop, words: &[Word]) -> Vec<(String, f64)> {
    if words.len() < 2 {
        return Vec::new();
    }
    let mut hs: Vec<u32> = words.iter().map(|w| w.rect.3).collect();
    hs.sort_unstable();
    let mh = hs[hs.len() / 2].max(4) as f64;
    // the OCR's lines, top to bottom (pieces of one visual line at the same height go together)
    let mut by_line: std::collections::BTreeMap<u32, Vec<usize>> = std::collections::BTreeMap::new();
    for (i, w) in words.iter().enumerate() {
        by_line.entry(w.line).or_default().push(i);
    }
    let cy = |i: usize| words[i].rect.1 as f64 + words[i].rect.3 as f64 / 2.0;
    let mut lines: Vec<(f64, Vec<usize>)> = by_line
        .into_values()
        .map(|idx| {
            let mut ys: Vec<f64> = idx.iter().map(|&i| cy(i)).collect();
            ys.sort_by(|a, b| a.total_cmp(b));
            (ys[ys.len() / 2], idx)
        })
        .collect();
    lines.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut joined: Vec<(f64, Vec<usize>)> = Vec::new();
    for (y, idx) in lines {
        match joined.last_mut() {
            Some(last) if (y - last.0).abs() < 0.5 * mh => last.1.extend(idx),
            _ => joined.push((y, idx)),
        }
    }
    if joined.len() < 2 {
        return Vec::new();
    }
    // A word's brightness: its bright letter pixels (the 90th percentile; the rest of its box is
    // the scene behind it). A line's: the median of its words'.
    let word_lum = |r: (u32, u32, u32, u32)| -> f64 {
        let (x1, y1) = ((r.0 + r.2).min(c.w), (r.1 + r.3).min(c.h));
        let mut v: Vec<u8> = Vec::new();
        for y in r.1..y1 {
            for x in r.0..x1 {
                let i = ((y * c.w + x) * 4) as usize;
                v.push(c.rgba[i].max(c.rgba[i + 1]).max(c.rgba[i + 2]));
            }
        }
        if v.is_empty() {
            return 0.0;
        }
        v.sort_unstable();
        v[(v.len() * 9 / 10).min(v.len() - 1)] as f64
    };
    let line_lum = |idx: &[usize]| {
        let mut v: Vec<f64> = idx.iter().map(|&i| word_lum(words[i].rect)).collect();
        v.sort_by(|a, b| a.total_cmp(b));
        v[v.len() / 2]
    };
    // A line that ends a sentence ("Sit tight. We're gonna check on a couple things.") is a line of
    // its own: GTA VI stacks the next one under it ("What's he doing with the bag?") and takes it
    // away again, both white. A line that ends mid-sentence only wrapped.
    let ends_sentence = |idx: &[usize]| {
        let last = idx.iter().max_by_key(|&&i| (words[i].line, words[i].rect.0)).map(|&i| words[i].text.trim_end_matches(['"', '\'', '”', '’', ')']));
        last.is_some_and(|t| t.ends_with(['.', '?', '!', '…']))
    };
    let starts_lower = |idx: &[usize]| {
        let first = idx.iter().min_by_key(|&&i| (words[i].line, words[i].rect.0)).map(|&i| words[i].text.as_str());
        first.and_then(|t| t.chars().find(|c| c.is_alphabetic())).is_some_and(|c| c.is_lowercase())
    };
    // consecutive lines of about the same brightness, the upper one not ending a sentence, are one line
    let mut parts: Vec<(Vec<usize>, f64)> = Vec::new();
    for (_, mut idx) in joined {
        let l = line_lum(&idx);
        match parts.last_mut() {
            // (a line going on in lower case only wrapped, even after "not..." : "…which is not… / much.")
            Some((pidx, pl)) if l.min(*pl) >= 0.82 * l.max(*pl) && (!ends_sentence(pidx) || starts_lower(&idx)) => {
                pidx.append(&mut idx);
                *pl = pl.max(l);
            }
            _ => parts.push((idx, l)),
        }
    }
    if parts.len() < 2 {
        return Vec::new();
    }
    let top = parts.iter().map(|p| p.1).fold(1.0f64, f64::max);
    let out: Vec<(String, f64)> = parts
        .into_iter()
        .map(|(idx, l)| {
            // (each part's words are already top to bottom, left to right within a line)
            let text = idx.iter().map(|&i| words[i].text.as_str()).collect::<Vec<_>>().join(" ");
            (fix_contractions(&text.split_whitespace().collect::<Vec<_>>().join(" ")), l / top)
        })
        .filter(|(t, _)| t.chars().any(|ch| ch.is_alphabetic()))
        .collect();
    if out.len() < 2 { Vec::new() } else { out }
}

/// Could this be real English, or is it the OCR spelling out something that isn't English letters
/// at all — text in another script (the console's Arabic menu "وسائط" read as "bjlwg"), icons, a map?
/// False only when the line has words made of letters and none of them could be a word: no vowel
/// ("bjlwg"), six consonants in a row, or a capital in the middle of lowercase ("ulnJiJl").
/// Acronyms ("LSPD", "FIB") and the usual vowel-less words ("Mr", "hmm", "shh") are fine.
pub fn looks_like_words(text: &str) -> bool {
    let fixes = FIXES_EXTRA.read().unwrap();
    const NO_VOWEL_OK: [&str; 16] = ["mr", "mrs", "ms", "dr", "st", "jr", "hmm", "hm", "mm", "shh", "sh", "psst", "pst", "brr", "grr", "pfft"];
    let plausible = |w: &str| {
        let lower = w.to_lowercase();
        // (the game's layout update can name its own words, and junk it knows the OCR makes)
        if fixes.junk.contains(&lower) {
            return false;
        }
        if fixes.ok.contains(&lower) {
            return true;
        }
        if NO_VOWEL_OK.contains(&lower.as_str()) || (w.chars().count() <= 5 && w.chars().all(|c| c.is_uppercase())) {
            return true; // "Hmm", "LSPD"
        }
        // accented letters ("ö", "é") count as vowels: "Röckstar" is a word
        let vowel = |c: char| "aeiouy".contains(c) || !c.is_ascii();
        let has_vowel = lower.chars().any(vowel);
        let mut run = 0;
        let long_run = lower.chars().any(|c| {
            run = if vowel(c) { 0 } else { run + 1 };
            run >= 6
        });
        // "ulnJiJl": lowercase then a capital inside the word ("McKay" and "iPhone" are rare in games)
        let chars: Vec<char> = w.chars().collect();
        let odd_caps = chars.windows(2).skip(1).any(|p| p[0].is_lowercase() && p[1].is_uppercase());
        has_vowel && !long_run && !odd_caps
    };
    let words: Vec<&str> = text
        .split(|c: char| c.is_whitespace() || ".,!?;:\"()[]…-–—".contains(c))
        .map(|w| w.trim_matches('\''))
        .filter(|w| !w.is_empty() && w.chars().all(|c| c.is_alphabetic() || c == '\''))
        .collect();
    words.is_empty() || words.iter().any(|w| plausible(w))
}

#[cfg(test)]
mod speaker_tests {
    use super::*;

    /// A dark picture with "letters" (filled boxes) of the given brightness where the words are.
    fn picture(words: &[(&str, (u32, u32, u32, u32), u32, u8)]) -> (Crop, Vec<Word>) {
        let (w, h) = (400u32, 100u32);
        let mut rgba = vec![30u8; (w * h * 4) as usize];
        for &(_, r, _, lum) in words {
            for y in r.1..r.1 + r.3 {
                for x in r.0..r.0 + r.2 {
                    let i = ((y * w + x) * 4) as usize;
                    rgba[i..i + 3].copy_from_slice(&[lum, lum, lum]);
                }
            }
        }
        let ws = words.iter().map(|&(t, rect, line, _)| Word { text: t.into(), rect, line }).collect();
        (Crop { w, h, rgba }, ws)
    }

    #[test]
    fn a_dim_icon_is_cut_out_visible() {
        // a grey (185) square on a dark (40) background, with a dark symbol inside; the line's
        // threshold (220) is above the icon's brightness
        let (w, h) = (20u32, 20u32);
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                let v = if (4..16).contains(&x) && (4..16).contains(&y) && !((8..12).contains(&x) && (8..12).contains(&y)) { 185 } else { 40 };
                let i = ((y * w + x) * 4) as usize;
                rgba[i..i + 4].copy_from_slice(&[v, v, v, 255]);
            }
        }
        let c = Crop { rgba, w, h };
        let url = icon_png(&c, (4, 4, 12, 12), 220).expect("an icon");
        use base64::Engine as _;
        let png = base64::engine::general_purpose::STANDARD.decode(url.trim_start_matches("data:image/png;base64,")).unwrap();
        let img = image::load_from_memory(&png).unwrap().to_rgba8();
        let solid = img.pixels().filter(|p| p[3] == 255).count();
        assert!(solid >= 12 * 12, "the icon must stay visible, {solid} solid pixels");
    }

    #[test]
    fn a_white_and_a_grey_line_are_two_speakers() {
        let (c, w) = picture(&[("Do", (100, 10, 30, 20), 0, 210), ("not", (140, 10, 40, 20), 0, 215), ("move.", (190, 10, 60, 20), 0, 205), ("Idiot.", (150, 50, 60, 20), 1, 122)]);
        let parts = speaker_lines(&c, &w);
        assert_eq!(parts.iter().map(|p| p.0.as_str()).collect::<Vec<_>>(), ["Do not move.", "Idiot."]);
        assert_eq!(parts[0].1, 1.0);
        assert!((0.5..0.65).contains(&parts[1].1), "{}", parts[1].1);
    }

    #[test]
    fn stacked_white_lines_are_two_subtitles() {
        let (c, w) = picture(&[
            ("Sit", (60, 10, 30, 20), 0, 210), ("tight.", (100, 10, 50, 20), 0, 212), ("We're", (160, 10, 50, 20), 0, 208), ("off.", (220, 10, 40, 20), 0, 210),
            ("What's", (100, 50, 60, 20), 1, 209), ("that?", (170, 50, 50, 20), 1, 211),
        ]);
        let parts = speaker_lines(&c, &w);
        assert_eq!(parts.iter().map(|p| p.0.as_str()).collect::<Vec<_>>(), ["Sit tight. We're off.", "What's that?"]);
        assert!(parts.iter().all(|p| p.1 > 0.95), "both white: neither drawn grey");
    }

    #[test]
    fn readings_that_agree() {
        assert!(agree("Do not move. Idiot.", "Do not move."), "the grey line missed");
        assert!(agree("It's your fault.", "It's your fault"));
        assert!(agree("", ""));
        assert!(!agree("", "to aim uold"), "one read nothing");
        assert!(!agree("Press toathreaten.the teller", "Press to threaten the tene"));
    }

    #[test]
    fn a_wrapped_line_is_one_speaker() {
        let (c, w) = picture(&[("You", (100, 10, 40, 20), 0, 210), ("never", (150, 10, 60, 20), 0, 200), ("listen.", (120, 50, 70, 20), 1, 205)]);
        assert!(speaker_lines(&c, &w).is_empty());
        // one line: nothing to split
        let (c, w) = picture(&[("Okay,", (100, 10, 60, 20), 0, 210), ("pull", (170, 10, 40, 20), 0, 120)]);
        assert!(speaker_lines(&c, &w).is_empty());
    }
}

#[cfg(test)]
mod word_tests {
    use super::looks_like_words as ok;
    #[test]
    fn tells_words_from_misread_script() {
        assert!(!ok("bjlwg")); // the PS5's Arabic «وسائط» read by the English OCR
        assert!(!ok("ulnJiJl hjluJ9"));
        assert!(!ok("bjlwg qrtzm"));
        assert!(ok("Go to the guard."));
        assert!(ok("Hmm."));
        assert!(ok("LSPD"));
        assert!(ok("Mr. K"));
        assert!(ok("C'mon mister"));
        assert!(ok("Press {1} to bring up the phone"));
        assert!(ok("Trevor"));
        assert!(ok("Strengths")); // "ngths": a long but real consonant run
        assert!(ok("Röckstar Editör"));
        assert!(ok("$500")); // no words at all: left as it was
    }
}

/// OCR fixes from the game's layout update:
/// `"ocr": {"words": {"cmon": "c'mon"}, "junk": ["bjlwg"], "ok": ["hmm"]}` — whole words, any case
/// (a replacement keeps a leading capital); junk words make a line not text, ok words are real words.
#[derive(Default, Clone, Debug)]
pub struct Fixes {
    pub words: Vec<(String, String)>,
    pub junk: Vec<String>,
    pub ok: Vec<String>,
}

static FIXES_EXTRA: std::sync::RwLock<Fixes> = std::sync::RwLock::new(Fixes { words: Vec::new(), junk: Vec::new(), ok: Vec::new() });

pub fn set_fixes(f: Fixes) {
    *FIXES_EXTRA.write().unwrap() = f;
}

/// Read a layout's `"ocr"` block.
pub fn fixes_from(v: &serde_json::Value) -> Fixes {
    let list = |k: &str| v[k].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).map(|x| x.to_lowercase()).collect()).unwrap_or_default();
    Fixes {
        words: v["words"].as_object().map(|m| m.iter().filter_map(|(k, x)| Some((k.to_lowercase(), x.as_str()?.to_string()))).collect()).unwrap_or_default(),
        junk: list("junk"),
        ok: list("ok"),
    }
}

/// Windows OCR drops the apostrophe in GTA's subtitle font ("Cmon mister, were giving you ...").
/// Put it back where the word can't be anything else; and "were" + an "-ing" verb at the start of a
/// sentence or after a comma ("mister, were giving") is "we're" ("they were giving" stays).
pub fn fix_contractions(text: &str) -> String {
    const FIXES: [(&str, &str); 34] = [
        ("cmon", "c'mon"), ("im", "i'm"), ("ive", "i've"), ("youre", "you're"), ("youve", "you've"),
        ("youll", "you'll"), ("theyre", "they're"), ("theyve", "they've"), ("theyll", "they'll"),
        ("weve", "we've"), ("dont", "don't"), ("doesnt", "doesn't"), ("didnt", "didn't"), ("cant", "can't"),
        ("wont", "won't"), ("isnt", "isn't"), ("arent", "aren't"), ("wasnt", "wasn't"), ("werent", "weren't"),
        ("wouldnt", "wouldn't"), ("couldnt", "couldn't"), ("shouldnt", "shouldn't"), ("havent", "haven't"),
        ("hasnt", "hasn't"), ("aint", "ain't"), ("thats", "that's"), ("whats", "what's"), ("hes", "he's"),
        ("shes", "she's"), ("theres", "there's"), ("heres", "here's"), ("wheres", "where's"), ("whos", "who's"),
        ("yall", "y'all"),
    ];
    let words: Vec<&str> = text.split(' ').collect();
    let mut out: Vec<String> = Vec::with_capacity(words.len());
    for (i, w) in words.iter().enumerate() {
        let start = w.find(|c: char| c.is_alphanumeric()).unwrap_or(w.len());
        // (char boundaries, not bytes: OCR text can hold "ö", "é" ...)
        let end = w.char_indices().rev().find(|(_, c)| c.is_alphanumeric()).map(|(e, c)| e + c.len_utf8()).unwrap_or(start).max(start);
        let (pre, core, post) = (&w[..start], &w[start..end], &w[end..]);
        let lower = core.to_lowercase();
        let mut fixed = FIXES.iter().find(|(k, _)| *k == lower).map(|(_, v)| v.to_string());
        if fixed.is_none() {
            fixed = FIXES_EXTRA.read().unwrap().words.iter().find(|(k, _)| *k == lower).map(|(_, v)| v.clone());
        }
        if fixed.is_none() && lower == "were" {
            let next_ing = words
                .get(i + 1)
                .is_some_and(|n| n.trim_matches(|c: char| !c.is_alphabetic()).to_lowercase().ends_with("ing"));
            // a clause starts here: the line's start, or after , . ! ? ; :
            let clause_start = i == 0 || words[i - 1].ends_with([',', '.', '!', '?', ';', ':']);
            if next_ing && clause_start {
                fixed = Some("we're".into());
            }
        }
        match fixed {
            Some(f) => {
                // keep the capital ("Cmon" -> "C'mon"); "I'm" is always capital
                let f = if core.starts_with(|c: char| c.is_uppercase()) || f.starts_with("i'") {
                    let mut ch = f.chars();
                    ch.next().map(|c| c.to_uppercase().collect::<String>() + ch.as_str()).unwrap_or_default()
                } else {
                    f
                };
                out.push(format!("{pre}{f}{post}"));
            }
            None => out.push(w.to_string()),
        }
    }
    out.join(" ")
}

#[cfg(test)]
mod icon_misread_tests {
    use super::drop_icon_misreads as d;

    #[test]
    fn an_icon_read_as_letters_before_a_line_is_dropped() {
        assert_eq!(d("0000 to do today?"), "to do today?");
        assert_eq!(d("pooo and the smoke of weed is in the air, from corner to corner."), "and the smoke of weed is in the air, from corner to corner.");
        assert_eq!(d("cocoa pooo and the smoke of weed is in the air"), "and the smoke of weed is in the air");
        assert_eq!(d("cocoa poooc nooc and the smoke of weed is in the air"), "and the smoke of weed is in the air");
        assert_eq!(d("poooa poooc goog until a fat, dank-ass joint is citizen"), "until a fat, dank-ass joint is citizen");
        assert_eq!(d("cocoa nooc Especially you. Don't you say anything, okay?"), "Especially you. Don't you say anything, okay?");
    }

    #[test]
    fn real_words_and_clean_lines_stay() {
        for s in [
            "and the smoke of weed is in the air",
            "noon tomorrow, at the docks",
            "cocoa and cookies",
            "Coco, get in the car.",
            "goon squad's here",
            "oo",
            "pooo",
            "too good to be true",
            "on and on",
        ] {
            assert_eq!(d(s), s, "{s}");
        }
    }
}

#[cfg(test)]
mod contraction_tests {
    use super::fix_contractions as f;
    #[test]
    fn puts_lost_apostrophes_back() {
        assert_eq!(f("Cmon mister, were giving you everything you want."), "C'mon mister, we're giving you everything you want.");
        assert_eq!(f("They were giving it away."), "They were giving it away."); // a real "were"
        assert_eq!(f("Dont even think about it. Im serious, youre dead."), "Don't even think about it. I'm serious, you're dead.");
        assert_eq!(f("We were going home."), "We were going home.");
        assert_eq!(f("Were going in!"), "We're going in!"); // line start, no subject
        assert_eq!(f("The cops were waiting."), "The cops were waiting.");
        assert_eq!(f("Go to the guard."), "Go to the guard."); // untouched
        assert_eq!(f("Röckstar Editör, dont stop"), "Röckstar Editör, don't stop"); // non-ASCII letters
    }

    #[test]
    fn layout_fixes() {
        super::set_fixes(super::fixes_from(&serde_json::json!({"words": {"vvanted": "wanted"}, "junk": ["Sogwa"], "ok": ["grrk"]})));
        assert_eq!(f("Vvanted level"), "Wanted level");
        assert!(!super::looks_like_words("sogwa"));
        assert!(super::looks_like_words("Grrk"));
        super::set_fixes(Default::default());
        assert_eq!(f("Vvanted level"), "Vvanted level");
    }
}

/// Similarity 0..1 from edit distance, case-insensitive (used to tell "same line" from "new line").
pub fn similar(a: &str, b: &str) -> bool {
    if a.is_empty() || b.is_empty() {
        return false;
    }
    let a: Vec<char> = a.to_lowercase().chars().collect();
    let b: Vec<char> = b.to_lowercase().chars().collect();
    // (the edit distance is at least the difference in length: too different to be alike, without
    // working it out — the same answer as below, for far less work)
    if a.len().abs_diff(b.len()) * 20 >= a.len().max(b.len()) * 3 {
        return false;
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0; b.len() + 1];
    for i in 1..=a.len() {
        cur[0] = i;
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    let dist = prev[b.len()] as f64;
    1.0 - dist / a.len().max(b.len()) as f64 > 0.85
}

/// Highlighted phrases: consecutive words drawn in the same non-white colour → (phrase, "#rrggbb").
/// Only pixels right next to the dark text outline count as letter pixels, so a colourful background
/// behind white text is not mistaken for a highlight.
pub fn colored_phrases(crop: &Crop, words: &[Word]) -> Vec<(String, String)> {
    let (w, h) = (crop.w as usize, crop.h as usize);
    let mut maxc = vec![0u8; w * h];
    let mut minc = vec![0u8; w * h];
    for i in 0..w * h {
        let p = &crop.rgba[i * 4..i * 4 + 3];
        maxc[i] = p[0].max(p[1]).max(p[2]);
        minc[i] = p[0].min(p[1]).min(p[2]);
    }
    // "near outline": dilate the dark mask by 3 px (separable max filter).
    let dark: Vec<bool> = maxc.iter().map(|&m| m < 70).collect();
    let mut horiz = vec![false; w * h];
    for y in 0..h {
        for x in 0..w {
            let (a, b) = (x.saturating_sub(3), (x + 3).min(w - 1));
            horiz[y * w + x] = (a..=b).any(|xx| dark[y * w + xx]);
        }
    }
    let mut near = vec![false; w * h];
    for y in 0..h {
        let (a, b) = (y.saturating_sub(3), (y + 3).min(h - 1));
        for x in 0..w {
            near[y * w + x] = (a..=b).any(|yy| horiz[yy * w + x]);
        }
    }

    let word_color = |rect: (u32, u32, u32, u32)| -> Option<(i32, i32, i32)> {
        let (x0, y0) = (rect.0 as usize, rect.1 as usize);
        let (x1, y1) = ((x0 + rect.2 as usize).min(w), (y0 + rect.3 as usize).min(h));
        // the letters' own pixels: the brightest quarter of the bright ones (by r+g+b), so a bright
        // coloured background inside the word's box (an orange shirt behind white text with no
        // outline, as in GTA VI) isn't taken for the letters' colour: white text outshines it
        let mut bright: Vec<(i32, i32, i32)> = Vec::new();
        for y in y0..y1 {
            for x in x0..x1 {
                let i = y * w + x;
                if near[i] && maxc[i] > 150 {
                    let p = &crop.rgba[i * 4..i * 4 + 3];
                    bright.push((p[0] as i32, p[1] as i32, p[2] as i32));
                }
            }
        }
        if bright.len() < 12 {
            return None;
        }
        bright.sort_by_key(|p| -(p.0 + p.1 + p.2));
        bright.truncate((bright.len() / 4).max(12));
        let letters = bright.len();
        let colored: Vec<(i32, i32, i32)> = bright.into_iter().filter(|p| p.0.max(p.1).max(p.2) - p.0.min(p.1).min(p.2) > 60).collect();
        if (colored.len() as f64) < 0.5 * letters as f64 {
            return None;
        }
        let med = |f: fn(&(i32, i32, i32)) -> i32| {
            let mut v: Vec<i32> = colored.iter().map(f).collect();
            v.sort_unstable();
            v[v.len() / 2]
        };
        let (r, g, b) = (med(|p| p.0), med(|p| p.1), med(|p| p.2));
        let m = r.max(g).max(b).max(1) as f64;
        let k = 255.0 / m; // show it at full brightness
        Some(((r as f64 * k) as i32, (g as f64 * k) as i32, (b as f64 * k) as i32))
    };
    let close = |a: (i32, i32, i32), b: (i32, i32, i32)| {
        let d = (a.0 - b.0).pow(2) + (a.1 - b.1).pow(2) + (a.2 - b.2).pow(2);
        d < 80 * 80
    };
    let hex = |c: (i32, i32, i32)| format!("#{:02x}{:02x}{:02x}", c.0.clamp(0, 255), c.1.clamp(0, 255), c.2.clamp(0, 255));

    let mut phrases = Vec::new();
    let mut cur: Vec<&str> = Vec::new();
    let mut cur_c: Option<(i32, i32, i32)> = None;
    for word in words {
        let c = word_color(word.rect);
        match (c, cur_c) {
            (Some(c), Some(cc)) if close(c, cc) => {
                cur.push(&word.text);
                continue;
            }
            _ => {}
        }
        if let Some(cc) = cur_c {
            phrases.push((cur.join(" "), hex(cc)));
        }
        cur.clear();
        cur_c = c;
        if c.is_some() {
            cur.push(&word.text);
        }
    }
    if let Some(cc) = cur_c {
        if !cur.is_empty() {
            phrases.push((cur.join(" "), hex(cc)));
        }
    }
    // a speaker name's pale colour ("Kyle:" pink, "Jason:" yellow) is too soft for the highlight test
    if let Some((name, color)) = name_color(crop, words) {
        if !phrases.iter().any(|(p, _)| p.starts_with(name.as_str())) {
            phrases.insert(0, (name, color));
        }
    }
    phrases
}

/// A speaker name in front of a subtitle ("Kyle: Hey, yeah!") and its colour, when it is drawn in a
/// colour of its own: the brightest letter pixels of the name against those of the words after it.
pub fn name_color(crop: &Crop, words: &[Word]) -> Option<(String, String)> {
    let (n, name_text, name_rect) = name_words(words)?;
    let line = top_line(words);
    let rest: Vec<&Word> = line[n..].iter().take(4).copied().collect();
    let letters = |rects: &[(u32, u32, u32, u32)]| -> Option<(i32, i32, i32)> {
        let mut px: Vec<(i32, i32, i32)> = Vec::new();
        for &(x0, y0, w, h) in rects {
            for y in y0..(y0 + h).min(crop.h) {
                for x in x0..(x0 + w).min(crop.w) {
                    let p = crop.px(x, y);
                    if p.0.max(p.1).max(p.2) >= 150 {
                        px.push(p);
                    }
                }
            }
        }
        if px.len() < 12 {
            return None;
        }
        // the letters' own colour: the brightest part (edges blend into the background)
        px.sort_by_key(|p| -(p.0 + p.1 + p.2));
        px.truncate((px.len() * 2 / 5).max(12));
        let med = |f: fn(&(i32, i32, i32)) -> i32| {
            let mut v: Vec<i32> = px.iter().map(f).collect();
            v.sort_unstable();
            v[v.len() / 2]
        };
        Some((med(|p| p.0), med(|p| p.1), med(|p| p.2)))
    };
    let name = letters(&[name_rect])?;
    let chroma = name.0.max(name.1).max(name.2) - name.0.min(name.1).min(name.2);
    if chroma < 22 {
        return None; // white or grey: not a coloured name
    }
    if let Some(other) = letters(&rest.iter().map(|w| w.rect).collect::<Vec<_>>()) {
        // (tinted against the words after it: Peggy's pale green 211,232,201 against 220,220,218)
        let other_chroma = other.0.max(other.1).max(other.2) - other.0.min(other.1).min(other.2);
        let diff = (name.0 - other.0).abs().max((name.1 - other.1).abs()).max((name.2 - other.2).abs());
        if chroma - other_chroma < 15 && diff < 25 {
            return None; // the whole line is that colour: not a name of its own
        }
    }
    Some((name_text, format!("#{:02x}{:02x}{:02x}", name.0, name.1, name.2)))
}

// ------------------------------------------------------------------------------------- button icons

/// Placeholder for the n-th button icon (1-based) inside English text, e.g. "Press ⟦1⟧ to jump."
pub fn icon_token(n: usize) -> String {
    format!("⟦{n}⟧")
}

/// Cut an icon out of a crop as a PNG data URL. `thr` is the brightness that counts as "icon" (it
/// depends on the background: ~140 on a black page, higher on GTA's see-through grey help box).
/// Inside the icon's outline everything stays solid (the dark "R3" / ✕ drawn inside a white disc is
/// kept); outside it the background becomes fully transparent, with a soft 1-pixel edge.
pub fn icon_png(c: &Crop, r: (u32, u32, u32, u32), thr: u8) -> Option<String> {
    use base64::Engine as _;
    let pad = 2;
    let (x0, y0) = (r.0.saturating_sub(pad), r.1.saturating_sub(pad));
    let (x1, y1) = ((r.0 + r.2 + pad).min(c.w), (r.1 + r.3 + pad).min(c.h));
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let lum = |x: u32, y: u32| {
        let i = ((y * c.w + x) * 4) as usize;
        c.rgba[i].max(c.rgba[i + 1]).max(c.rgba[i + 2])
    };
    // A dimmer icon than the line's text (a video's compression greys GTA VI's L2 to ~185): the cut-out
    // line goes halfway between the icon's brightest and the background around it, or nothing at all
    // would count as icon and it would come out fully transparent.
    let brightest = (y0..y1).flat_map(|y| (x0..x1).map(move |x| (x, y))).map(|(x, y)| lum(x, y)).max().unwrap_or(0);
    // The cut-out line is halfway between the icon's brightest and the background around it, or the
    // line's own threshold if that is lower: a thin ring (✕ ⓧ) a video's compression dims to ~200 would
    // otherwise come out as a few dots, and a dim icon (a grey L2 ~185) fully transparent.
    let mut edge: Vec<u8> = (x0..x1).flat_map(|x| [lum(x, y0), lum(x, y1 - 1)]).chain((y0..y1).flat_map(|y| [lum(x0, y), lum(x1 - 1, y)])).collect();
    edge.sort_unstable();
    let bg = edge[edge.len() / 2];
    // (a solid crop, `thr` 0, keeps everything: a grey disc on a bright purple stands out by its colour, not its
    // brightness, and must still come out)
    if thr > 0 && brightest < bg.saturating_add(40) {
        return None; // no shape that stands out: nothing to cut
    }
    let thr = thr.min(((brightest as u32 + bg as u32) / 2) as u8);
    let mut img = image::RgbaImage::new(x1 - x0, y1 - y0);
    for y in y0..y1 {
        let first = (x0..x1).find(|&x| lum(x, y) > thr);
        let last = (x0..x1).rev().find(|&x| lum(x, y) > thr);
        for x in x0..x1 {
            let i = ((y * c.w + x) * 4) as usize;
            let (rr, g, b) = (c.rgba[i], c.rgba[i + 1], c.rgba[i + 2]);
            let a = match (first, last) {
                (Some(f), Some(l)) if x >= f && x <= l => 255,
                (Some(f), Some(l)) if x + 1 == f || x == l + 1 => 110, // anti-aliased rim
                _ => 0,
            };
            img.put_pixel(x - x0, y - y0, image::Rgba([rr, g, b, a]));
        }
    }
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(img).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).ok()?;
    Some(format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(png)))
}

/// A button found by the space it leaves between words: its square cut out as it is (opaque).
fn icon_png_plain(c: &Crop, r: (u32, u32, u32, u32)) -> Option<String> {
    use base64::Engine as _;
    let (x0, y0, w, h) = r;
    if w == 0 || h == 0 || x0 + w > c.w || y0 + h > c.h {
        return None;
    }
    let mut img = image::RgbaImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let i = (((y0 + y) * c.w + x0 + x) * 4) as usize;
            img.put_pixel(x, y, image::Rgba([c.rgba[i], c.rgba[i + 1], c.rgba[i + 2], 255]));
        }
    }
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(img).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).ok()?;
    Some(format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(png)))
}

/// Find controller button icons inside text and rebuild the line with placeholders where they sit:
/// "Press ⟦1⟧ to enter the vehicle." plus the icons cut from the picture (PNG data URLs).
///
/// Icons are found two ways: square-ish bright shapes in the gaps OCR left between words, and
/// "words" OCR made out of an icon (e.g. "@", "©", "8") that are square and taller than the text.
/// Also returns the box (crop pixels: x, y, w, h) the whole text block occupies, words and icons
/// together: GTA's instructions box hugs this block, so the Arabic box can line up with it.
pub fn text_with_icons(c: &Crop, words: &[Word]) -> (String, Vec<String>, Option<(u32, u32, u32, u32)>) {
    if words.is_empty() {
        return (String::new(), Vec::new(), None);
    }
    let mut hs: Vec<u32> = words.iter().map(|w| w.rect.3).collect();
    hs.sort_unstable();
    let mh = hs[hs.len() / 2].max(4) as f64; // typical text height

    let square = |w: u32, h: u32| {
        let r = w as f64 / h.max(1) as f64;
        (0.6..=1.8).contains(&r) // round icons ~1:1, shoulder buttons (L2/R2) up to ~1.7:1
    };
    // Is there a solid bright shape at this OCR box (an icon), rather than letters? Measured on the
    // tight box of near-white pixels around it: an icon fills ~0.7 of it, letters ≤ ~0.45 (thin
    // strokes), and an icon is about as tall as capitals.
    let solid_shape = |r: (u32, u32, u32, u32)| {
        let (x0, y0) = (r.0.saturating_sub(4), r.1.saturating_sub(4));
        let (x1, y1) = ((r.0 + r.2 + 4).min(c.w), (r.1 + r.3 + 4).min(c.h));
        let (mut tx0, mut ty0, mut tx1, mut ty1, mut n) = (u32::MAX, u32::MAX, 0u32, 0u32, 0u32);
        for y in y0..y1 {
            for x in x0..x1 {
                let i = ((y * c.w + x) * 4) as usize;
                if c.rgba[i].max(c.rgba[i + 1]).max(c.rgba[i + 2]) > 200 {
                    (tx0, ty0, tx1, ty1, n) = (tx0.min(x), ty0.min(y), tx1.max(x), ty1.max(y), n + 1);
                }
            }
        }
        if n == 0 {
            return false;
        }
        let (w, h) = (tx1 - tx0 + 1, ty1 - ty0 + 1);
        square(w, h) && h as f64 >= 0.6 * mh && n as f64 / (w * h) as f64 >= 0.6
    };
    // OCR "words" that are really icons
    let misread = |w: &Word| {
        let t = w.text.trim();
        // shoulder/stick buttons OCR reads as text: "L1", "R2", "R3", "LB", "RT" ...
        let label = t.len() == 2
            && matches!(t.as_bytes()[0], b'L' | b'R')
            && matches!(t.as_bytes()[1], b'1' | b'2' | b'3' | b'B' | b'T' | b'I' | b'l' | b'|'); // OCR often reads 1 as I or l
        let letters = t.chars().filter(|ch| ch.is_alphanumeric()).count();
        // an icon OCR read as a letter or two ("e" for L2 at 1080p): the box is a solid bright shape
        let solid = t.chars().count() <= 2 && solid_shape(w.rect);
        label
            || solid
            || (t.chars().count() <= 2
                && square(w.rect.2, w.rect.3)
                && w.rect.3 as f64 >= 1.1 * mh
                && (letters == 0 || matches!(t, "O" | "o" | "0" | "X" | "x" | "8" | "Q" | "R" | "L" | "®" | "©")))
    };

    // Items of each line, left to right: Ok(word index) or Err(icon box)
    let mut lines: Vec<(f64, Vec<(u32, Result<usize, (u32, u32, u32, u32)>)>)> = Vec::new();
    // Use the OCR's own lines: grouping by height would split a line whose words have tall
    // ("called") and low ("you") letters, scrambling the word order.
    let mut by_line: std::collections::BTreeMap<u32, Vec<usize>> = std::collections::BTreeMap::new();
    for (i, w) in words.iter().enumerate() {
        by_line.entry(w.line).or_default().push(i);
    }
    for idxs in by_line.into_values() {
        let mut cys: Vec<f64> = idxs.iter().map(|&i| words[i].rect.1 as f64 + words[i].rect.3 as f64 / 2.0).collect();
        cys.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let cy = cys[cys.len() / 2];
        let items = idxs.iter().map(|&i| (words[i].rect.0, if misread(&words[i]) { Err(words[i].rect) } else { Ok(i) })).collect();
        lines.push((cy, items));
    }
    // Windows OCR sometimes splits one visual line in two around an icon ("Press" | "to zoom in…"):
    // put pieces at the same height, side by side, back together (otherwise both pieces would find
    // the same icon between them).
    lines.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    let x_range = |items: &Vec<(u32, Result<usize, (u32, u32, u32, u32)>)>| {
        let ends = items.iter().map(|(_, it)| match it {
            Ok(i) => (words[*i].rect.0, words[*i].rect.0 + words[*i].rect.2),
            Err(r) => (r.0, r.0 + r.2),
        });
        ends.fold((u32::MAX, 0), |acc, (a, b)| (acc.0.min(a), acc.1.max(b)))
    };
    let mut joined: Vec<(f64, Vec<(u32, Result<usize, (u32, u32, u32, u32)>)>)> = Vec::new();
    for (cy, items) in lines {
        if let Some(last) = joined.last_mut() {
            let (a, b) = (x_range(&last.1), x_range(&items));
            if (cy - last.0).abs() < 0.5 * mh && (a.1 <= b.0 || b.1 <= a.0) {
                last.1.extend(items);
                continue;
            }
        }
        joined.push((cy, items));
    }
    let mut lines = joined;

    // Bright shapes in the gaps between words on each line. "Bright" is relative to the line's own
    // background: GTA's help box is see-through grey, often over lamps or sky.
    let lum = |x: u32, y: u32| {
        let i = ((y * c.w + x) * 4) as usize;
        c.rgba[i].max(c.rgba[i + 1]).max(c.rgba[i + 2])
    };
    let mut thr_used: u8 = 150;
    let text_boxes: Vec<(u32, u32, u32, u32)> = words.iter().filter(|w| !misread(w)).map(|w| w.rect).collect();
    let in_text = |x: u32, y: u32| {
        text_boxes.iter().any(|&(bx, by, bw, bh)| x + 2 >= bx && x <= bx + bw + 2 && y + 2 >= by && y <= by + bh + 2)
    };
    let mut found_icons: Vec<(u32, u32, u32, u32)> = Vec::new(); // x0, y0, x1, y1 of accepted icons
    let mut plain_icons: Vec<(u32, u32, u32, u32)> = Vec::new(); // buttons found by their gap: cut as they are
    for (cy, items) in lines.iter_mut() {
        // icons are about as tall as the text; a taller band would reach past GTA's help box
        let y0 = (*cy - 0.85 * mh).max(0.0) as u32;
        let y1 = ((*cy + 0.85 * mh) as u32).min(c.h - 1);
        // only look near this line's words (icons sit between or right next to them)
        let word_x: Vec<(u32, u32)> =
            items.iter().filter_map(|(_, it)| it.as_ref().ok().map(|&i| (words[i].rect.0, words[i].rect.0 + words[i].rect.2))).collect();
        let lx0 = word_x.iter().map(|w| w.0).min().unwrap_or(0).saturating_sub((3.0 * mh) as u32);
        let lx1 = (word_x.iter().map(|w| w.1).max().unwrap_or(c.w) + (3.0 * mh) as u32).min(c.w - 1);
        // background level of the line (text is a small part of it) -> what counts as icon-bright
        let mut samples: Vec<u8> = (y0..=y1).step_by(2).flat_map(|y| (lx0..=lx1).step_by(3).map(move |x| (x, y))).map(|(x, y)| lum(x, y)).collect();
        samples.sort_unstable();
        let bg = samples.get(samples.len() / 2).copied().unwrap_or(0) as u32;
        // icons and text are near-white; lamps or sky behind the see-through box are dimmer
        let thr = (bg + 80).clamp(220, 245) as u8;
        thr_used = thr;
        let bright = |x: u32, y: u32| lum(x, y) > thr;
        // Connected groups of near-white pixels that aren't part of a word. (Grouping by columns
        // instead would glue an icon to a lamp right under GTA's see-through box.)
        let on = |x: u32, y: u32| bright(x, y) && !in_text(x, y);
        let (bw, bh) = (lx1 - lx0 + 1, y1 - y0 + 1);
        let at = |x: u32, y: u32| ((y - y0) * bw + (x - lx0)) as usize;
        let mut seen = vec![false; (bw * bh) as usize];
        let mut comps: Vec<(u32, u32, u32, u32, u32)> = Vec::new(); // x0, y0, x1, y1, pixel count
        for y in y0..=y1 {
            for x in lx0..=lx1 {
                if seen[at(x, y)] || !on(x, y) {
                    continue;
                }
                seen[at(x, y)] = true;
                let mut stack = vec![(x, y)];
                let mut cc = (x, y, x, y, 0u32);
                while let Some((cx, cy)) = stack.pop() {
                    cc = (cc.0.min(cx), cc.1.min(cy), cc.2.max(cx), cc.3.max(cy), cc.4 + 1);
                    let mut push = |nx: u32, ny: u32| {
                        if nx >= lx0 && nx <= lx1 && ny >= y0 && ny <= y1 && !seen[at(nx, ny)] && on(nx, ny) {
                            seen[at(nx, ny)] = true;
                            stack.push((nx, ny));
                        }
                    };
                    if cx > 0 {
                        push(cx - 1, cy);
                    }
                    push(cx + 1, cy);
                    if cy > 0 {
                        push(cx, cy - 1);
                    }
                    push(cx, cy + 1);
                }
                comps.push(cc);
            }
        }
        // An icon's white parts can be split by its dark symbol (e.g. ✕): merge small groups that
        // touch. Big groups (a lamp, the sky) never merge, or they would swallow a nearby icon.
        let small = |c: &(u32, u32, u32, u32, u32)| ((c.2 - c.0 + 1) as f64) <= 2.5 * mh && ((c.3 - c.1 + 1) as f64) <= 2.5 * mh;
        let mut merged = true;
        while merged {
            merged = false;
            'outer: for i in 0..comps.len() {
                for j in i + 1..comps.len() {
                    let (a, b) = (comps[i], comps[j]);
                    if small(&a) && small(&b) && a.0 <= b.2 + 2 && b.0 <= a.2 + 2 && a.1 <= b.3 + 2 && b.1 <= a.3 + 2 {
                        comps[i] = (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3), a.4 + b.4);
                        comps.remove(j);
                        merged = true;
                        break 'outer;
                    }
                }
            }
        }
        for (s, ty, e, by, count) in comps {
            let (w, h) = (e - s + 1, by - ty + 1);
            if count < 6 {
                continue;
            }
            if std::env::var_os("VI_TRANSLATOR_DEBUG").is_some() {
                eprintln!("[icons] line bg {bg} thr {thr} mh {mh:.0}: blob x {s}-{e} y {ty}-{by} ({w}x{h})");
            }
            if (h as f64) < 0.55 * mh || (h as f64) > 2.5 * mh || !square(w, h) {
                continue;
            }
            // an icon sits inside the line; something spanning the whole band is letters/scenery
            if ty == y0 && by == y1 {
                continue;
            }
            let fill = count as f64 / (w * h) as f64;
            if std::env::var_os("VI_TRANSLATOR_DEBUG").is_some() {
                eprintln!("[icons] line bg {bg} thr {thr} mh {mh:.0}: candidate x {s}-{e} y {ty}-{by} ({w}x{h}) fill {fill:.2}");
            }
            // icons are solid shapes (≥ ~0.45, e.g. R2's white box with dark letters); stray letter strokes ~0.3
            if !(0.42..=0.95).contains(&fill) {
                continue;
            }
            // ...with a dark symbol inside (✕ △ "R3" "L2"); a plain bright patch of scenery has none
            let (ix0, ix1) = (s + w / 5, e - w / 5);
            let (iy0, iy1) = (ty + h / 5, by - h / 5);
            let inner = ((ix1 - ix0 + 1) * (iy1 - iy0 + 1)).max(1) as f64;
            let dark = (iy0..=iy1).flat_map(|y| (ix0..=ix1).map(move |x| (x, y))).filter(|&(x, y)| lum(x, y) < 110).count();
            if std::env::var_os("VI_TRANSLATOR_DEBUG").is_some() {
                eprintln!("[icons]    dark symbol share {:.2}", dark as f64 / inner);
            }
            // (inside GTA's solid black box a bright solid shape can only be an icon — e.g. the touchpad,
            // a light bar with faint lines; over see-through scenery it could be a lamp)
            if (dark as f64) < 0.04 * inner && !(bg < 60 && fill >= 0.7) {
                continue;
            }
            // a real icon has a space on both sides; a stroke OCR left outside a word ("w?nna") touches it
            let line_words: Vec<(u32, u32)> = items
                .iter()
                .filter_map(|(_, it)| it.as_ref().ok().map(|&i| (words[i].rect.0, words[i].rect.0 + words[i].rect.2)))
                .collect();
            let min_gap = (0.2 * mh) as u32;
            let left_gap = line_words.iter().filter(|w| w.1 <= s).map(|w| s - w.1).min().unwrap_or(u32::MAX);
            let right_gap = line_words.iter().filter(|w| w.0 >= e).map(|w| w.0 - e).min().unwrap_or(u32::MAX);
            let overlaps_word = line_words.iter().any(|w| w.0 < e && w.1 > s);
            if std::env::var_os("VI_TRANSLATOR_DEBUG").is_some() {
                eprintln!("[icons]    gaps left {left_gap} right {right_gap} (need {min_gap}), overlaps word {overlaps_word}");
            }
            if overlaps_word || left_gap < min_gap || right_gap < min_gap {
                continue;
            }
            // skip if it overlaps an icon already found as a misread word
            if items.iter().any(|(_, it)| matches!(it, Err(r) if s < r.0 + r.2 && e > r.0)) {
                continue;
            }
            // ...or one another line already took
            if found_icons.iter().any(|f| s <= f.2 && e >= f.0 && ty <= f.3 && by >= f.1) {
                continue;
            }
            found_icons.push((s, ty, e, by));
            items.push((s, Err((s, ty, w, h))));
        }
        items.sort_by_key(|(x, _)| *x);
        // A button the shape test missed (a grey ✕ disc on a bright, glowing floor): a gap between two
        // words about one button wide (normal spaces are ~0.3 of the text height, a menu's columns far
        // wider) with something drawn in its middle. Its picture is cut as it is, background and all.
        let mut gaps = Vec::new();
        for pair in items.windows(2) {
            let (Ok(a), Ok(b)) = (&pair[0].1, &pair[1].1) else { continue };
            let (ra, rb) = (words[*a].rect, words[*b].rect);
            let (ge0, gs1) = (ra.0 + ra.2, rb.0);
            if gs1 <= ge0 {
                continue;
            }
            let gap = (gs1 - ge0) as f64;
            if !(1.1 * mh..=3.2 * mh).contains(&gap) {
                continue;
            }
            // the button's square, centred in the gap on the line
            let side = (1.1 * mh) as u32;
            let gx = ge0 + ((gs1 - ge0).saturating_sub(side)) / 2;
            let gy = ((*cy - side as f64 / 2.0).max(0.0) as u32).min(c.h.saturating_sub(side));
            if gx + side > c.w || side < 6 {
                continue;
            }
            let vals: Vec<f64> = (gy..gy + side).flat_map(|y| (gx..gx + side).map(move |x| (x, y))).map(|(x, y)| lum(x, y) as f64).collect();
            let mean = vals.iter().sum::<f64>() / vals.len() as f64;
            let sd = (vals.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / vals.len() as f64).sqrt();
            if sd < 30.0 {
                continue; // nothing drawn there: just a wide space
            }
            gaps.push((gx, Err((gx, gy, side, side))));
            plain_icons.push((gx, gy, side, side));
        }
        items.extend(gaps);
        items.sort_by_key(|(x, _)| *x);
    }
    lines.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());

    let mut parts: Vec<String> = Vec::new();
    let mut icons = Vec::new();
    let mut ext: Option<(u32, u32, u32, u32)> = None; // x0, y0, x1, y1
    let mut grow = |r: (u32, u32, u32, u32)| {
        let (x1, y1) = (r.0 + r.2, r.1 + r.3);
        ext = Some(match ext {
            None => (r.0, r.1, x1, y1),
            Some(e) => (e.0.min(r.0), e.1.min(r.1), e.2.max(x1), e.3.max(y1)),
        });
    };
    for (_, items) in &lines {
        for (_, it) in items {
            match it {
                Ok(i) => {
                    grow(words[*i].rect);
                    parts.push(words[*i].text.clone())
                }
                Err(r) => {
                    grow(*r);
                    // one of the game's own icons, recognised: the clean one the app carries; else cut from the line
                    let known = crate::game_icons::identify(&c.into(), *r).and_then(crate::game_icons::data_url).map(String::from);
                    let png = known.or_else(|| if plain_icons.contains(r) { icon_png_plain(c, *r) } else { icon_png(c, *r, thr_used) });
                    if let Some(png) = png {
                        // "<height relative to the text>|<png>": the Arabic draws it at the same proportion
                        // (GTA's L2/R2 are smaller than the text, ✕/△ discs a bit larger)
                        // (+4: the cut-out has a 2 px transparent margin above and below the icon)
                        icons.push(format!("{:.3}|{png}", (r.3 + 4) as f64 / mh));
                        parts.push(icon_token(icons.len()));
                    }
                }
            }
        }
    }
    (fix_contractions(&parts.join(" ")), icons, ext.map(|e| (e.0, e.1, e.2 - e.0, e.3 - e.1)))
}


// ------------------------------------------------------------------------------------- OCR passes

/// Two readings of the same picture agree: alike, or one is the other with a line missing (the
/// white-only reading drops a grey speaker's line). Nothing read by both agrees too.
fn agree(a: &str, b: &str) -> bool {
    if a.trim().is_empty() && b.trim().is_empty() {
        return true;
    }
    if a.trim().is_empty() || b.trim().is_empty() {
        return false;
    }
    if similar(a, b) {
        return true;
    }
    let (short, long) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    long.contains(short)
}

/// Words of one line that the OCR gave right to left ("to aim uold" for "Hold LT to aim"): a reading
/// of scenery, not of the text.
fn words_out_of_order(words: &[Word]) -> bool {
    // (also across the OCR's lines when they are at the same height: "uold" put on a line of its own)
    words.windows(2).any(|p| {
        let same_row = (p[0].rect.1 as i64 - p[1].rect.1 as i64).abs() < (p[0].rect.3.max(p[1].rect.3) as i64) / 2;
        (p[0].line == p[1].line || same_row) && p[1].rect.0 + p[1].rect.2 < p[0].rect.0
    })
}

/// Letters drawn with a dark outline (GTA VI's instructions): the bright pixels that have the outline
/// close on both sides, across or up and down — inside a letter's stroke. A bright floor or wall
/// behind the text has the outline on one side only, so it drops out.
fn outlined_letters(c: &Crop) -> Vec<bool> {
    let (w, h) = (c.w as usize, c.h as usize);
    let lum: Vec<i32> = (0..w * h).map(|i| { let (r, g, b) = c.px((i % w) as u32, (i / w) as u32); r.max(g).max(b) }).collect();
    // (the outline is dark grey, lighter where video compression smeared it)
    let dark = |i: usize| lum[i] < 130;
    // a stroke is a small part of the text's height; the area holds about one line
    let reach = (h / 9).clamp(3, 12);
    let mut m = vec![false; w * h];
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            if lum[i] < 170 {
                continue;
            }
            let side = |dx: isize, dy: isize| {
                (1..=reach as isize).any(|k| {
                    let (nx, ny) = (x as isize + dx * k, y as isize + dy * k);
                    nx >= 0 && ny >= 0 && (nx as usize) < w && (ny as usize) < h && dark(ny as usize * w + nx as usize)
                })
            };
            m[i] = (side(-1, 0) && side(1, 0)) || (side(0, -1) && side(0, 1));
        }
    }
    // specks that pass too (gravel, glints, the smeared outline between two letters) make the OCR
    // lose whole words, so only pieces the size of a letter part stay
    drop_specks(&mut m, w, h, (h / 16).max(5), 12);
    // one pixel thicker: the strokes come out thin, and the OCR reads solid letters best
    let mut thick = m.clone();
    for y in 1..h.saturating_sub(1) {
        for x in 1..w.saturating_sub(1) {
            let i = y * w + x;
            if !m[i] && (m[i - 1] || m[i + 1] || m[i - w] || m[i + w]) && lum[i] >= 150 {
                thick[i] = true;
            }
        }
    }
    thick
}

/// `best` with the clean words (plain letters, two or more) of other readings put in where `best` has
/// nothing: on one of its lines (overlapping it up and down) and not touching any word already there.
/// Unchanged when `best` is empty: a word from another pass alone is too little to go on.
fn fill_gaps(best: &[Word], others: &[&[Word]]) -> Vec<Word> {
    let mut out: Vec<Word> = best.to_vec();
    if best.is_empty() {
        return out;
    }
    let overlaps = |a: (u32, u32), b: (u32, u32)| a.0 < b.1 && b.0 < a.1;
    for other in others {
        for w in other.iter() {
            let (g, b) = quality(std::slice::from_ref(w));
            if b > 0 || g < 2 {
                continue;
            }
            let (x0, x1) = (w.rect.0, w.rect.0 + w.rect.2);
            let (y0, y1) = (w.rect.1, w.rect.1 + w.rect.3);
            // the line it belongs to: a word taken whose height overlaps most of this one's
            let Some(host) = out.iter().find(|o| {
                let (oy0, oy1) = (o.rect.1, o.rect.1 + o.rect.3);
                oy0.max(y0) + (y1 - y0) / 2 <= oy1.min(y1)
            }) else {
                continue;
            };
            let line = host.line;
            if out.iter().any(|o| overlaps((o.rect.0, o.rect.0 + o.rect.2), (x0, x1))) {
                continue;
            }
            out.push(Word { text: w.text.clone(), rect: w.rect, line });
        }
    }
    // in reading order: by line, then left to right
    out.sort_by_key(|w| (w.line, w.rect.0));
    out
}

/// Clear the pieces of a mask (8-connected) smaller than `min_px` pixels or shorter than `min_h` rows.
fn drop_specks(m: &mut [bool], w: usize, h: usize, min_h: usize, min_px: usize) {
    let mut seen = vec![false; w * h];
    let mut stack = Vec::new();
    let mut piece = Vec::new();
    for start in 0..w * h {
        if !m[start] || seen[start] {
            continue;
        }
        seen[start] = true;
        stack.push(start);
        piece.clear();
        let (mut y0, mut y1) = (start / w, start / w);
        while let Some(i) = stack.pop() {
            piece.push(i);
            let (x, y) = (i % w, i / w);
            (y0, y1) = (y0.min(y), y1.max(y));
            for dy in -1isize..=1 {
                for dx in -1isize..=1 {
                    let (nx, ny) = (x as isize + dx, y as isize + dy);
                    if nx < 0 || ny < 0 || nx as usize >= w || ny as usize >= h {
                        continue;
                    }
                    let j = ny as usize * w + nx as usize;
                    if m[j] && !seen[j] {
                        seen[j] = true;
                        stack.push(j);
                    }
                }
            }
        }
        if piece.len() < min_px || y1 - y0 + 1 < min_h {
            for &i in &piece {
                m[i] = false;
            }
        }
    }
}

/// How pixels are turned into the black-on-white picture the OCR reads.
#[derive(Clone, Copy)]
pub enum Prep {
    /// Brightest colour channel (white AND coloured text stay bright). Best for outlined subtitles.
    Brightest,
    /// Only near-white or strongly coloured pixels, stretched: drops a lamp or sky showing through
    /// GTA's grey see-through box, and dull red/brown scenery, while keeping white and highlight text.
    WhiteOrColour,
    /// Only the bright pixels inside a dark outline (see `outlined_letters`).
    Outlined,
    /// Darkest colour channel: white letters on a bright blue (GTA VI's phone, the character's own
    /// messages), whose brightest channel is as bright as the letters.
    Darkest,
}

fn prep_value(mode: Prep, r: i32, g: i32, b: i32) -> u8 {
    match mode {
        Prep::Brightest => r.max(g).max(b) as u8,
        Prep::WhiteOrColour => {
            let (mx, mn) = (r.max(g).max(b), r.min(g).min(b));
            let v = if mx - mn > 80 { mx } else { mn }; // coloured: its brightness; grey/white: its whiteness
            (((v - 150).max(0) * 255) / 105).min(255) as u8
        }
        Prep::Outlined => r.max(g).max(b) as u8, // (prepared as a whole picture in preprocess)
        Prep::Darkest => r.min(g).min(b) as u8,
    }
}

/// (good, bad) characters in an OCR reading: words made of plain letters with sane capitals are
/// good; accented letters in English, stray symbols, punctuation inside words, "shoZOith"-style
/// capitals are bad. Lone symbols (often a misread button icon) count as neither.
pub fn quality(words: &[Word]) -> (i32, i32) {
    let (mut good, mut bad) = (0, 0);
    for w in words {
        let t = w.text.trim_matches(|c: char| ".,!?;:\"'()".contains(c));
        let n = t.chars().count() as i32;
        if n == 0 {
            continue;
        }
        if n == 1 && !t.chars().all(|c| c.is_ascii_alphanumeric()) {
            continue; // "@", "©": an icon, judged elsewhere
        }
        let letters_ok = t.chars().all(|c| c.is_ascii_alphabetic() || c == '\'');
        let rest: String = t.chars().skip(1).filter(|c| c.is_ascii_alphabetic()).collect();
        let case_ok = rest.chars().all(|c| c.is_ascii_lowercase()) || t.chars().all(|c| !c.is_ascii_lowercase());
        let number = t.chars().all(|c| c.is_ascii_digit() || c == ',' || c == '$');
        if (letters_ok && case_ok) || number {
            good += n;
        } else {
            bad += n;
        }
    }
    (good, bad)
}


/// Remove scenery read as text: tiny fragments carrying quote marks or bullets (`"c'`, `Y'"`, `•`).
fn drop_junk(words: Vec<Word>) -> Vec<Word> {
    words
        .into_iter()
        .filter(|w| {
            let t = w.text.trim();
            let letters = t.chars().filter(|c| c.is_alphabetic()).count();
            let junky = t.chars().any(|c| "\"“”•|=~_^`".contains(c))
                || ((t.starts_with('\'') || t.ends_with('\'')) && letters <= 2);
            !(junky && letters <= 2)
        })
        .collect()
}

#[cfg(test)]
mod centered_tests {
    use super::*;

    fn w(text: &str, x: u32, y: u32, width: u32) -> Word {
        Word { text: text.into(), rect: (x, y, width, 24), line: 0 }
    }
    fn texts(ws: &[Word]) -> String {
        ws.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ")
    }

    #[test]
    fn the_help_box_is_told_by_where_its_lines_are() {
        // GTA VI's help box in the help area's crop: left 15, top 94, at most 445 wide; its lines 24 px
        // tall, 33 apart, starting 14 px in
        let anchor = (15, 94, 445);
        let w = |t: &str, x: u32, y: u32, wd: u32| Word { text: t.into(), rect: (x, y, wd, 24), line: 0 };
        let help = vec![
            w("Getting", 29, 108, 90), w("a", 125, 108, 12), w("wanted", 143, 108, 80), w("level", 229, 108, 55), w("doesn't", 290, 108, 85),
            w("affect", 29, 141, 70), w("your", 105, 141, 50), w("criminal", 161, 141, 90), w("profile,", 257, 141, 85), w("but", 348, 141, 40),
            w("cops", 29, 174, 55), w("will.", 90, 174, 45),
        ];
        let (b, kept) = in_anchored_box(&help, anchor).unwrap();
        assert_eq!(texts(&kept), "Getting a wanted level doesn't affect your criminal profile, but cops will.");
        assert_eq!((b.0, b.1), (15, 94));
        assert!(b.2 <= 445 && b.3 >= 174 + 24 - 94);
        // a shop's name above it and a sign under it: left out
        let mut busy = help.clone();
        busy.push(w("SUPERMARKET", 0, 40, 150));
        busy.push(w("786-555-0109", 10, 300, 160));
        assert_eq!(texts(&in_anchored_box(&busy, anchor).unwrap().1), texts(&kept));
        // a sign's words elsewhere in the area: none of it
        let sign = vec![w("Visitor", 200, 30, 80), w("Center", 290, 30, 80)];
        assert!(in_anchored_box(&sign, anchor).is_none());
        // a line too wide for the box (a billboard right where the box is): not the box
        let wide = vec![w("BOAT", 29, 108, 300), w("STORAGE", 340, 108, 300)];
        assert!(in_anchored_box(&wide, anchor).is_none());
    }

    #[test]
    fn help_reads_as_sentences_and_a_menu_item_does_not() {
        assert!(reads_as_sentences("Hold to interact."));
        assert!(reads_as_sentences("Cops will recognize you faster in areas with heat if you haven't changed your appearance"));
        assert!(!reads_as_sentences("Tactical Rifles"));
        assert!(!reads_as_sentences("Curated Liveries"));
    }

    #[test]
    fn a_billboard_in_capitals_is_not_help() {
        assert!(mostly_capitals("BOAT STORAG HURRICANE SAFE 786-555-0109 BOAT SERVICING STORAGE PARTS"));
        assert!(!mostly_capitals("Use the GPS to find the LSPD station."));
        assert!(mostly_capitals("NORTH WEST"));
        assert!(!mostly_capitals("OK")); // (too short to tell)
    }

    #[test]
    fn stacked_lines_count_from_the_bottom_line_up() {
        // GTA VI at 1080p, in a crop ending at the area's bottom (991): lines 24 px tall, 44 apart (in
        // the crop: line 1's middle at 976 - 602), with short words a third smaller ("on", "re")
        let at = |t: &str, x: u32, mid: u32, h: u32| Word { text: t.into(), rect: (x, mid - h / 2, 10 * t.len() as u32, h), line: 0 };
        // a line centred in the picture (x 960), its words 10 px a letter and a space apart
        let line = |ws: &[(&str, u32)], mid: u32| {
            let width: u32 = ws.iter().map(|(t, _)| 10 * t.len() as u32 + 10).sum::<u32>() - 10;
            let mut x = 960 - width / 2;
            ws.iter()
                .map(|(t, h)| {
                    let w = at(t, x, mid + (24 - h) / 2, *h);
                    x += 10 * t.len() as u32 + 10;
                    w
                })
                .collect::<Vec<_>>()
        };
        let three: Vec<Word> = [
            line(&[("Locals:", 24), ("Fuck", 24), ("you!", 22)], 286),
            line(&[("Cops:", 24), ("Show", 24), ("us", 16), ("your", 22), ("hands!", 24)], 330),
            line(&[("Locals:", 24), ("You", 24), ("won't", 24), ("see", 16), ("us", 16), ("again!", 24)], 374),
        ]
        .concat();
        let all = texts(&bottom_stack(&three, 389.0));
        assert_eq!(all, three.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" "));
        // a car's lettering up in the picture, a plate right above line 1: not subtitles
        let mut busy = three[8..].to_vec(); // (line 1)
        busy.push(at("KEY", 700, 200, 20));
        busy.push(at("LENTO", 760, 200, 20));
        busy.push(at("POLICE", 860, 200, 20));
        busy.push(at("LS5252", 880, 340, 22));
        assert_eq!(texts(&bottom_stack(&busy, 389.0)), "Locals: You won't see us again!");
        // only the car's lettering, low in the area but not where line 1 is: nothing
        let car = vec![at("KEY", 700, 289, 22), at("LENTO", 760, 289, 22), at("POLICE", 860, 289, 22)];
        assert!(bottom_stack(&car, 389.0).is_empty());
        // one line read as two pieces 9 px apart (a tall misread letter): still one line, in order
        let mut split = line(&[("Local:", 24), ("Just-just", 24), ("fuckin'", 24), ("get", 24), ("outta", 24), ("here,", 24), ("okay?", 24)], 374);
        for w in &mut split[..2] {
            w.rect.1 -= 9;
        }
        assert_eq!(texts(&bottom_stack(&split, 389.0)), "Local: Just-just fuckin' get outta here, okay?");
        // one speaker's wrapped lines, 34 px apart (closer than two speakers' 44): all of them
        let wrapped = [
            line(&[("Local:", 24), ("How", 24), ("'bout", 24), ("you,", 24), ("uh,", 24)], 306),
            line(&[("just", 24), ("leave", 24), ("me", 24), ("alone", 24)], 340),
            line(&[("Sounds", 24), ("nice,", 24), ("huh?", 24)], 374),
        ]
        .concat();
        assert_eq!(texts(&bottom_stack(&wrapped, 389.0)), "Local: How 'bout you, uh, just leave me alone Sounds nice, huh?");
        // the lines sliding up as a new one comes: nothing at line 1 for a moment, the rest a place up
        let sliding = [line(&[("Local:", 24), ("Go", 24), ("away.", 24)], 286), line(&[("Cops:", 24), ("Stop!", 24)], 330)].concat();
        assert!(bottom_stack(&sliding, 389.0).is_empty() && stack_moving(&sliding, 389.0));
        assert!(!stack_moving(&three, 389.0)); // (line 1 there: not moving)
        // a stray mark before a name goes
        let marked = line(&[("-", 10), ("Local:", 24), ("What", 24), ("the", 24), ("hell?", 24)], 374);
        assert_eq!(texts(&bottom_stack(&marked, 389.0)), "Local: What the hell?");
        // a line off to the side above line 1 isn't joined to it
        let side = [line(&[("Locals:", 24), ("again!", 24)], 374), vec![at("Garage", 100, 330, 24), at("menu", 180, 330, 24)]].concat();
        assert_eq!(texts(&bottom_stack(&side, 389.0)), "Locals: again!");
    }

    #[test]
    fn a_panel_at_the_side_is_left_out_of_the_subtitle() {
        // GTA VI's garage: the area (from x 192 of a 1920 frame) has the subtitle in the middle and the car's
        // stats panel on the right; the OCR missed the middle words of the line
        let ws = vec![w("STRENGTH", 1291, 10, 110), w("Joey:", 490, 95, 58), w("to", 900, 95, 20), w("the", 925, 95, 36), w("action", 966, 95, 80), w("RATING", 1291, 70, 76)];
        assert_eq!(texts(&centered_words(&ws, 192.0, 1920.0)), "Joey: to the action");
    }

    #[test]
    fn panels_on_both_sides_are_not_a_subtitle() {
        // the upgrade's name on the left and the car's stats on the right: together they're centred
        let ws = vec![w("ADE", 3, 41, 45), w("STAGE", 57, 41, 74), w("2", 140, 41, 14), w("STRENGTH", 1292, 28, 90), w("RATING", 1292, 93, 64), w("350", 1481, 93, 33)];
        assert!(centered_words(&ws, 192.0, 1920.0).is_empty());
    }

    #[test]
    fn a_centred_line_is_kept_whole() {
        let ws = vec![w("Come", 700, 10, 60), w("on,", 768, 10, 40), w("let's", 816, 10, 60), w("go.", 884, 10, 40)];
        assert_eq!(centered_words(&ws, 192.0, 1920.0).len(), 4);
    }

    #[test]
    fn text_only_at_the_side_is_not_a_subtitle() {
        let ws = vec![w("Browse", 1400, 10, 90)];
        assert!(centered_words(&ws, 192.0, 1920.0).is_empty());
    }
}
