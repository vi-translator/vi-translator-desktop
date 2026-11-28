//! Game pages ("screens"): whole pages of a game (a menu, a "mission failed" page, a caption card)
//! that get their own translated layout instead of plain subtitles. Pages scripts find and read
//! them (see `pagescript.rs`); this holds what they read — positions as fractions of the frame, so
//! the overlay places the translation exactly relative to the English at any window size — and
//! what the app needs to follow a page while it's up: small pictures of its parts, to see what
//! changed.

use serde::{Deserialize, Serialize};

use crate::frame::FrameData;
use crate::ocr;

/// Rectangle as fractions (0..1) of the frame.
#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Button {
    pub en: String,
    pub ar: String,
    /// The game's own button icon, cut out of the frame (PNG data URL), so the replica is exact.
    pub icon: Option<String>,
    pub icon_rect: Option<Rect>,
    /// The English word's box (its height sets the Arabic font size).
    pub text_rect: Rect,
    /// More icons after the first ("Browse [L1] [R1]").
    pub more_icons: Vec<String>,
    /// The word's colour in the game (a greyed-out button that can't be used now); unset = white.
    pub fg: String,
    /// Its translation beside the English (left of it), not above: buttons stacked one under the other
    /// (a failed mission's "RETRY CHECKPOINT ✕" …), with no room above each.
    pub beside: bool,
}

/// A piece of the game's UI covered by a translated replica (e.g. a menu tab): same box, same
/// background colour, the translation in the matching text colour.
#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Label {
    pub en: String,
    pub ar: String,
    pub rect: Rect,
    pub bg: String,
    pub fg: String,
    /// the tab the player is on (a menu's pages are told apart by it)
    pub selected: bool,
    /// what the game showed under it when read (see `region_sig`)
    #[serde(skip)]
    pub sig: Vec<u8>,
}

#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct ScreenLayout {
    pub id: String,
    pub title: Rect,
    pub title_color: String,
    /// the title's English (translated by the app), and its translation (or given by the script)
    pub title_en: String,
    pub title_ar: String,
    pub reason: Option<Rect>,
    /// Height of one line of the reason text (the reason can wrap onto several lines).
    pub reason_line_h: f64,
    pub reason_en: String,
    /// Buttons from right to left (the order an Arabic reader meets them).
    pub buttons: Vec<Button>,
    pub buttons_row: Option<Rect>,
    /// UI pieces replaced by translated replicas (menu tabs).
    pub labels: Vec<Label>,
    /// A block of short lines (a menu's character name, day and time): translated just beside the English.
    pub player: Option<PlayerInfo>,
    /// Text boxes: menu items and paragraphs, or any text a script places (see `MenuText`).
    pub menu: Vec<MenuText>,
    /// Boxes drawn again under the game's, translated (see `Card`).
    pub cards: Vec<Card>,
    /// Followed like a menu while it's up ("menu"): each text box and tab watched every frame, the
    /// page read again when they change, and a tab's earlier states brought back at once (see
    /// `menu_rects`).
    pub follow: String,
    /// for a followed menu: where its content is, and its button row (frame fractions)
    pub content_rect: Option<Rect>,
    pub buttons_rect: Option<Rect>,
    /// a fine picture of the whole menu area when it was read (see `content_sig`)
    #[serde(skip)]
    pub content_sig: Vec<u8>,
    /// a picture of the button row (bottom right) when it was read
    #[serde(skip)]
    pub buttons_sig: Vec<u8>,
}

/// A box of the game's (the map's objective: a picture and a name over a sentence, on a dark box under a
/// coloured line; the map's info box of the place under the cursor, with a heading, short lines and a table)
/// drawn again just under it, translated, in its look.
#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Card {
    /// the game's box (frame fractions); the translation goes under it
    pub rect: Rect,
    /// its background (a CSS colour, see-through as the game's), its top line's colour and height
    pub bg: String,
    pub border: String,
    pub border_h: f64,
    /// the picture beside the name (PNG data URL), when there is one
    pub pic: String,
    pub title_en: String,
    pub title_ar: String,
    /// the name shown as the script gives it, not translated (a place's name stays in English)
    pub title_fixed: bool,
    /// one line's height (frame fractions)
    pub title_h: f64,
    pub text_en: String,
    pub text_ar: String,
    pub text_h: f64,
    /// a twin of the game's box: the same width, at least its height, right under it (above it when there's no
    /// room under it), each part in its place (the map's info box)
    pub twin: bool,
    /// the band behind the heading (a CSS colour), darker than the rest of the box
    pub head_bg: String,
    /// a heading over the name (a mission's giver, "DUNK'S ROBBERY TIPS"), and its letter height
    pub head_en: String,
    pub head_ar: String,
    pub head_h: f64,
    /// short lines under the name, each after its icon ("Goods to sell", "Come back at night")
    pub sub: Vec<CardLine>,
    /// the table under the sentence: a label on the left, its value against the right edge ("SECURITY … LOW")
    pub rows: Vec<CardRow>,
    pub rows_h: f64,
}

/// A short line of a card, after its icon.
#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct CardLine {
    pub en: String,
    pub ar: String,
    /// its icon (PNG data URL), or ""
    pub icon: String,
    /// letter height (frame fraction)
    pub h: f64,
    /// in capitals (the game's small capitals: "'84 BOBCAT XL")
    pub caps: bool,
}

/// A row of a card's table.
#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct CardRow {
    pub en: String,
    pub ar: String,
    /// its value's text ("LOW", "$1,800 - $2,200"), its translation, its colour
    pub value_en: String,
    pub value_ar: String,
    pub value_fg: String,
    /// a value that is only icons ("👕🔫"): the game's, each cut out to its shape (PNG data URLs)
    pub value_icons: Vec<String>,
}

/// A text box: a menu's list item or paragraph covered by a translated replica, or any text a script
/// places, in its own look.
#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct MenuText {
    pub en: String,
    /// filled in by the app (built-in words, memory, or the translator); empty = not yet
    pub ar: String,
    /// the English text block, frame fractions
    pub rect: Rect,
    pub bg: String,
    pub fg: String,
    /// one line's height (frame fraction)
    pub line_h: f64,
    /// a list item (left column) rather than a paragraph
    pub item: bool,
    /// the English is right-aligned (values, labels left of the controller): keep that edge
    pub right: bool,
    /// how many lines the English took (1 = keep the Arabic on one line)
    pub lines: u32,
    /// a heading much bigger than the menu text ("Replay"): keeps its own size
    pub heading: bool,
    /// reads in a row this item was not seen again (kept on screen for a moment meanwhile)
    pub missed: u32,
    // How the box looks, for pages scripts (all optional: unset = the pause menu's look). With these a
    // script can put any text anywhere, the way the game shows it:
    /// "left" | "center" | "right" (unset: the English's edge, see `right`)
    pub align: String,
    /// "title" (the profile's title font) | a font family | unset (the profile's text font)
    pub font: String,
    /// letter size as a share of `line_h` (unset: 1.2)
    pub size: f64,
    /// 100 … 900 (unset: 500)
    pub weight: u32,
    /// a dark shadow under the letters (for text straight on the picture, no box)
    pub shadow: bool,
    /// an outline around the letters, in this colour
    pub outline: String,
    /// "yes" | "no" (unset: list items on one line, paragraphs wrap)
    pub wrap: String,
    /// shown as it is (`ar`, else `en`): not translated (numbers, names, the script's own words)
    pub fixed: bool,
    /// other people's text (player nicknames …): never translated, sent or kept; not covered either
    pub private: bool,
    /// the game's button icons for this text (PNG data URLs), drawn where the translation says ⟦1⟧, ⟦2⟧ …
    pub icons: Vec<String>,
    /// what the game showed under it when read (see `region_sig`)
    #[serde(skip)]
    pub sig: Vec<u8>,
}

/// What a script read, kept to what a page can show (a runaway script can't fill memory or the
/// overlay): at most 300 text boxes with 8 icons each, 48 tabs, 16 buttons with 4 more icons each,
/// icons of at most 256 KB, 8 player lines; places and sizes that are numbers.
pub fn limit(l: &mut ScreenLayout) {
    const ICON: usize = 256 * 1024;
    l.menu.truncate(300);
    for m in l.menu.iter_mut() {
        m.icons.retain(|i| i.len() <= ICON && i.starts_with("data:image/png;base64,"));
        m.icons.truncate(8);
    }
    l.labels.truncate(48);
    l.cards.truncate(8);
    let png = |p: &mut String| {
        if p.len() > ICON || !p.starts_with("data:image/png;base64,") {
            p.clear();
        }
    };
    for c in l.cards.iter_mut() {
        png(&mut c.pic);
        c.sub.truncate(8);
        c.rows.truncate(16);
        for x in c.sub.iter_mut() {
            png(&mut x.icon);
        }
        for r in c.rows.iter_mut() {
            r.value_icons.retain(|i| i.len() <= ICON && i.starts_with("data:image/png;base64,"));
            r.value_icons.truncate(8);
        }
    }
    l.buttons.truncate(16);
    for b in l.buttons.iter_mut() {
        if b.icon.as_ref().is_some_and(|i| i.len() > ICON) {
            b.icon = None;
        }
        b.more_icons.retain(|i| i.len() <= ICON);
        b.more_icons.truncate(4);
    }
    if let Some(p) = l.player.as_mut() {
        p.en.truncate(8);
        p.ar.truncate(8);
    }
    // (NaN or infinite places would only confuse the overlay and the pictures taken of them)
    let fix = |r: &mut Rect| {
        for v in [&mut r.x, &mut r.y, &mut r.w, &mut r.h] {
            *v = if v.is_finite() { v.clamp(-1.0, 2.0) } else { 0.0 };
        }
    };
    fix(&mut l.title);
    for r in [&mut l.reason, &mut l.buttons_row, &mut l.content_rect, &mut l.buttons_rect].into_iter().flatten() {
        fix(r);
    }
    for b in l.buttons.iter_mut() {
        fix(&mut b.text_rect);
        if let Some(r) = b.icon_rect.as_mut() {
            fix(r);
        }
    }
    for x in l.labels.iter_mut() {
        fix(&mut x.rect);
    }
    for c in l.cards.iter_mut() {
        fix(&mut c.rect);
    }
    for m in l.menu.iter_mut() {
        fix(&mut m.rect);
        if !m.line_h.is_finite() {
            m.line_h = 0.0;
        }
        if !m.size.is_finite() {
            m.size = 0.0;
        }
    }
    if let Some(p) = l.player.as_mut() {
        fix(&mut p.rect);
    }
    if !l.reason_line_h.is_finite() {
        l.reason_line_h = 0.0;
    }
}

/// A layout with nothing to show: no title, reason, buttons, tabs, player lines or text (other
/// people's text, which is never shown, doesn't count). Taking over the screen with it would only
/// hide the subtitles.
pub fn is_blank(l: &ScreenLayout) -> bool {
    l.title.w <= 0.0
        && l.title_en.trim().is_empty()
        && l.title_ar.trim().is_empty()
        && l.reason_en.trim().is_empty()
        && l.buttons.is_empty()
        && l.labels.is_empty()
        && l.menu.iter().all(|m| m.private)
        && l.cards.iter().all(|c| c.title_en.trim().is_empty() && c.text_en.trim().is_empty() && c.head_en.trim().is_empty() && c.rows.is_empty())
        && l.player.as_ref().is_none_or(|p| p.en.is_empty())
}

/// The tab the player is on.
pub fn selected_tab_name(l: &ScreenLayout) -> Option<&str> {
    l.labels.iter().find(|x| x.selected).map(|x| x.en.as_str())
}

/// A block of short lines (a character's name, day and time): translations drawn just left of the English.
#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct PlayerInfo {
    pub en: Vec<String>,
    pub ar: Vec<String>,
    /// the English block, frame fractions
    pub rect: Rect,
    /// one line's letter height and the distance between line tops (frame fractions)
    pub line_h: f64,
    pub pitch: f64,
}

fn px(f: &FrameData, x: u32, y: u32) -> (u8, u8, u8) {
    let i = ((y * f.w + x) * 4) as usize;
    (f.rgba[i], f.rgba[i + 1], f.rgba[i + 2])
}

/// Is the frame (almost) black? (a page can end when the screen goes dark: `hold.endWhenDark`)
pub fn is_dark(f: &FrameData) -> bool {
    let (mut sum, mut n) = (0u64, 0u64);
    for y in ((f.h / 10)..(f.h * 9 / 10)).step_by(8) {
        for x in ((f.w / 10)..(f.w * 9 / 10)).step_by(8) {
            let (r, g, b) = px(f, x, y);
            sum += r.max(g).max(b) as u64;
            n += 1;
        }
    }
    n > 0 && sum / n < 25
}

pub(crate) fn letters(s: &str) -> String {
    s.chars().filter(|c| c.is_alphabetic()).collect::<String>().to_lowercase()
}


/// A page read again while it's up: keep what was measured when it appeared (re-measuring as the
/// scene behind fades to black would make the translation jump in size and place); only the words
/// change (a new reason, Yes / No buttons). A part the new read missed is kept from before.
pub fn keep_measured(l: &mut ScreenLayout, old: &ScreenLayout) {
    if l.reason_en.is_empty() {
        (l.reason, l.reason_line_h, l.reason_en) = (old.reason.clone(), old.reason_line_h, old.reason_en.clone());
    }
    if l.buttons.is_empty() {
        (l.buttons, l.buttons_row) = (old.buttons.clone(), old.buttons_row.clone());
    }
    l.title = old.title.clone();
    if l.reason_en == old.reason_en {
        (l.reason, l.reason_line_h) = (old.reason.clone(), old.reason_line_h);
    }
    if l.buttons.len() == old.buttons.len() {
        for (b, o) in l.buttons.iter_mut().zip(&old.buttons) {
            (b.icon, b.icon_rect, b.text_rect) = (o.icon.clone(), o.icon_rect.clone(), o.text_rect.clone());
        }
        l.buttons_row = old.buttons_row.clone();
    }
}

/// A page read by a script: the pictures the app compares later to see what changed (a script
/// can't take them: they never leave the app). Taken now, from the frame it was read from.
pub fn take_sigs(f: &FrameData, l: &mut ScreenLayout) {
    for m in l.menu.iter_mut() {
        m.sig = region_sig(f, &m.rect);
    }
    for lb in l.labels.iter_mut() {
        lb.sig = region_sig(f, &lb.rect);
    }
    if let Some((content, buttons)) = menu_rects(l) {
        l.content_sig = content_sig_in(f, &content);
        l.buttons_sig = if buttons.w > 0.0 && buttons.h > 0.0 { region_sig(f, &buttons) } else { Vec::new() };
    }
}

/// A page followed like a menu: its content area (the whole picture if it names none) and its
/// button row (an empty box if none: then not watched).
pub fn menu_rects(l: &ScreenLayout) -> Option<(Rect, Rect)> {
    (l.follow == "menu").then(|| (l.content_rect.clone().unwrap_or(Rect { x: 0.0, y: 0.0, w: 1.0, h: 1.0 }), l.buttons_rect.clone().unwrap_or_default()))
}

/// For a script re-reading a page that's up: which of its parts still show what they did.
pub fn unchanged(f: &FrameData, old: &ScreenLayout) -> serde_json::Value {
    serde_json::json!({
        "menu": old.menu.iter().map(|m| region_same(f, &m.rect, &m.sig)).collect::<Vec<_>>(),
        "labels": old.labels.iter().map(|l| region_same(f, &l.rect, &l.sig)).collect::<Vec<_>>(),
        "buttons": menu_rects(old).is_none_or(|r| region_same(f, &r.1, &old.buttons_sig)),
    })
}

/// Words from the game's downloaded layout (`"words": {"<lang>": {"menu", "buttons", "tabs"}}`), for one
/// game and language ("game/lang"): menu words, button words and tab names with their translation.
/// Set with `set_layout_words`; used only while that game and language are the ones in use (see
/// `use_words_for`), so after a switch no word of the other language shows until the new ones come.
static LAYOUT_WORDS: std::sync::RwLock<(String, [Vec<(&'static str, &'static str)>; 3])> =
    std::sync::RwLock::new((String::new(), [Vec::new(), Vec::new(), Vec::new()]));
/// The game and language in use ("game/lang").
static WORDS_FOR: std::sync::RwLock<String> = std::sync::RwLock::new(String::new());

/// Replace the layout's words (menu, buttons, tabs) for a game and language. Layout updates are rare
/// and small, so the strings are simply kept for the rest of the run.
pub fn set_layout_words(game: &str, lang: &str, menu: Vec<(String, String)>, buttons: Vec<(String, String)>, tabs: Vec<(String, String)>) {
    let keep = |v: Vec<(String, String)>| -> Vec<(&'static str, &'static str)> {
        v.into_iter().map(|(e, a)| (&*Box::leak(e.into_boxed_str()), &*Box::leak(a.into_boxed_str()))).collect()
    };
    *LAYOUT_WORDS.write().unwrap() = (format!("{game}/{lang}"), [keep(menu), keep(buttons), keep(tabs)]);
}

/// The game and language in use now (the detection loop says so as they change).
pub fn use_words_for(game: &str, lang: &str) {
    let tag = format!("{game}/{lang}");
    if *WORDS_FOR.read().unwrap() != tag {
        *WORDS_FOR.write().unwrap() = tag;
    }
}

/// The language in use ("ar" from "gta6/ar"), for page scripts' own words.
pub fn words_lang() -> String {
    WORDS_FOR.read().unwrap().rsplit('/').next().unwrap_or("").to_string()
}

fn layout_words(i: usize) -> Vec<(&'static str, &'static str)> {
    let w = LAYOUT_WORDS.read().unwrap();
    if w.0 == *WORDS_FOR.read().unwrap() { w.1[i].clone() } else { Vec::new() }
}

fn menu_words() -> Vec<(&'static str, &'static str)> {
    layout_words(0)
}

pub(crate) fn button_words() -> Vec<(&'static str, &'static str)> {
    layout_words(1)
}

pub(crate) fn tab_words() -> Vec<(&'static str, &'static str)> {
    layout_words(2)
}

/// The layout's translation of a menu word or phrase.
pub fn menu_word(en: &str) -> Option<&'static str> {
    let k = letters(en);
    let words = menu_words();
    words
        .iter()
        .find(|(e, _)| letters(e) == k)
        // a small OCR slip ("Rockstar Edito/") still counts, for longer phrases
        .or_else(|| words.iter().find(|(e, _)| k.len() >= 8 && ocr::similar(&letters(e), &k)))
        .map(|(_, a)| *a)
}

/// The layout's translation of a button's word ("Select", "Back").
pub fn button_word(en: &str) -> Option<&'static str> {
    let k = letters(en);
    button_words().iter().find(|(e, _)| letters(e) == k).map(|(_, a)| *a)
}

/// A coarse picture of a region: 24 x 6 brightness samples. Cheap enough for every frame.
pub fn region_sig(f: &FrameData, r: &Rect) -> Vec<u8> {
    let (cols, rows) = (24u32, 6u32);
    let x0 = (r.x * f.w as f64) as u32;
    let y0 = (r.y * f.h as f64) as u32;
    let (w, h) = (((r.w * f.w as f64) as u32).max(1), ((r.h * f.h as f64) as u32).max(1));
    let mut out = Vec::with_capacity((cols * rows) as usize);
    for j in 0..rows {
        for i in 0..cols {
            let x = (x0 + (2 * i + 1) * w / (2 * cols)).min(f.w - 1);
            let y = (y0 + (2 * j + 1) * h / (2 * rows)).min(f.h - 1);
            let (r_, g, b) = px(f, x, y);
            out.push(((r_ as u32 * 3 + g as u32 * 6 + b as u32) / 10) as u8);
        }
    }
    out
}

/// Does the region still show what it did when its signature was taken? A different line, a
/// highlight moving onto it, or the text gone all change it a lot; the blurred game behind
/// the menu barely moves.
pub fn region_same(f: &FrameData, r: &Rect, sig: &[u8]) -> bool {
    if sig.is_empty() {
        return true;
    }
    let now = region_sig(f, r);
    let diff: u32 = now.iter().zip(sig).map(|(a, b)| a.abs_diff(*b) as u32).sum();
    diff / (sig.len() as u32) < 6 // unchanged regions measure 0-1, changed ones 8 and up
}

/// The same fine picture of any area.
pub fn content_sig_in(f: &FrameData, area: &Rect) -> Vec<u8> {
    let (cols, rows) = (64u32, 32u32);
    let (x0, y0) = ((area.x * f.w as f64) as u32, (area.y * f.h as f64) as u32);
    let (w, h) = (((area.w * f.w as f64) as u32).max(1), ((area.h * f.h as f64) as u32).max(1));
    let mut out = Vec::with_capacity((cols * rows) as usize);
    for j in 0..rows {
        for i in 0..cols {
            let (r, g, b) = px(f, (x0 + (2 * i + 1) * w / (2 * cols)).min(f.w - 1), (y0 + (2 * j + 1) * h / (2 * rows)).min(f.h - 1));
            out.push(((r as u32 * 3 + g as u32 * 6 + b as u32) / 10) as u8);
        }
    }
    out
}

/// Has the menu's content changed since `sig` was taken? (a few samples changed a lot)
pub fn content_changed_in(f: &FrameData, area: &Rect, sig: &[u8]) -> bool {
    !sig.is_empty() && content_sig_in(f, area).iter().zip(sig).filter(|(a, b)| a.abs_diff(**b) > 40).count() >= 3
}

/// Two readings of one line: at least half their words in common (OCR reads the same line a little
/// differently each time; another line shares far fewer).
pub fn same_words(a: &str, b: &str) -> bool {
    let words = |s: &str| -> std::collections::HashSet<String> { s.split_whitespace().map(letters).filter(|w| w.len() > 1).collect() };
    let (wa, wb) = (words(a), words(b));
    if wa.is_empty() || wb.is_empty() {
        return letters(a) == letters(b);
    }
    let common = wa.intersection(&wb).count();
    common * 2 >= wa.len().min(wb.len())
}

/// Letter-level similarity 0..1 (edit distance).
fn ratio(a: &str, b: &str) -> f64 {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut cur = vec![i; b.len() + 1];
        for j in 1..=b.len() {
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + usize::from(a[i - 1] != b[j - 1]));
        }
        prev = cur;
    }
    1.0 - prev[b.len()] as f64 / a.len().max(b.len()) as f64
}

/// A list item as one of the menu's known words (the layout's), even when OCR garbled it badly — a
/// greyed-out item's faint letters come out as "Join cruööds" for "Join Friends". The list is a
/// closed set of words, so a looser match is safe there: same first letters, ~60 % alike.
/// Returns (the English as the game writes it, its translation).
pub fn menu_item_word(en: &str) -> Option<(&'static str, &'static str)> {
    // faint letters come back accented ("Join croénds"): compare them plain
    let fold = |s: &str| -> String {
        letters(s)
            .chars()
            .map(|c| match c {
                'é' | 'è' | 'ê' | 'ë' => 'e',
                'ö' | 'ó' | 'ò' | 'ô' => 'o',
                'ä' | 'á' | 'à' | 'â' | 'å' => 'a',
                'ü' | 'ú' | 'ù' => 'u',
                'í' | 'ì' | 'ï' => 'i',
                c => c,
            })
            .collect()
    };
    let k = fold(en);
    if k.len() < 3 {
        return None;
    }
    let words = menu_words();
    if let Some(hit) = words.iter().find(|(e, _)| fold(e) == k) {
        return Some(*hit);
    }
    words
        .iter()
        .map(|w| (w, fold(w.0)))
        .filter(|(_, l)| l.len() >= 4 && l.get(..3) == k.get(..3) && l.len().abs_diff(k.len()) <= 4)
        // ~60 % alike; ~50 % when the first four letters agree ("join…")
        .map(|(w, l)| (w, ratio(&l, &k), l.get(..4) == k.get(..4)))
        .filter(|(_, r, four)| *r >= if *four { 0.5 } else { 0.6 })
        .map(|(w, r, _)| (w, r))
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(w, _)| *w)
}

/// Height in pixels of the letters inside a box: the span of rows that hold the bright text
/// (rows whose brightest pixel is above halfway between the dimmest and brightest rows).
pub(crate) fn letter_height(f: &FrameData, x0: u32, y0: u32, x1: u32, y1: u32) -> u32 {
    let lum = |x: u32, y: u32| {
        let (r, g, b) = px(f, x, y);
        (r as u32 * 3 + g as u32 * 6 + b as u32) / 10
    };
    let rows: Vec<u32> = (y0..=y1.min(f.h - 1)).map(|y| (x0..=x1.min(f.w - 1)).step_by(2).map(|x| lum(x, y)).max().unwrap_or(0)).collect();
    let (lo, hi) = (*rows.iter().min().unwrap_or(&0), *rows.iter().max().unwrap_or(&0));
    let thr = lo + (hi - lo) / 2;
    let first = rows.iter().position(|&v| v > thr);
    let last = rows.iter().rposition(|&v| v > thr);
    match (first, last) {
        (Some(a), Some(b)) if b > a => (b - a + 1) as u32,
        _ => y1.saturating_sub(y0),
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_words_come_from_the_layout() {
        assert_eq!(menu_word("Hide Menu"), None);
        use_words_for("gta6", "ar");
        set_layout_words("gta6", "ar", vec![("Join Friends".into(), "انضم إلى الأصدقاء".into())], vec![("Back".into(), "رجوع".into())], vec![]);
        for garbled in ["Join cruööds", "Join croénds", "Join erids"] {
            assert_eq!(menu_item_word(garbled).map(|x| x.0), Some("Join Friends"), "{garbled}");
        }
        assert_eq!(menu_word("join friends!"), Some("انضم إلى الأصدقاء"));
        assert_eq!(button_words().len(), 1);
        // another language in use: the Arabic words don't show until its own come
        use_words_for("gta6", "fr");
        assert_eq!(menu_word("join friends!"), None);
        assert!(button_words().is_empty());
        use_words_for("gta6", "ar");
        assert_eq!(button_words().len(), 1, "back to Arabic: its words again");
        set_layout_words("", "", vec![], vec![], vec![]);
    }

    #[test]
    fn one_line_read_twice_has_the_same_words_another_line_not() {
        assert!(same_words("Jason and Lucia's criminal profiles are affected", "Jason and Lucias criminal profi1es are affected"));
        assert!(!same_words("Using extreme violence can have the opposite effect", "A criminal with a professional profile commits crimes"));
        assert!(same_words("Pool", "pool"));
    }

    #[test]
    fn a_followed_page_watches_its_own_areas() {
        let f = FrameData { w: 100, h: 100, rgba: vec![80; 100 * 100 * 4], seq: 1 };
        let mut l = ScreenLayout { follow: "menu".into(), labels: vec![Label { en: "MAP".into(), selected: true, rect: Rect { x: 0.1, y: 0.1, w: 0.2, h: 0.05 }, ..Default::default() }], ..Default::default() };
        take_sigs(&f, &mut l);
        assert_eq!(selected_tab_name(&l), Some("MAP"));
        assert!(!l.content_sig.is_empty() && l.buttons_sig.is_empty(), "no button row named: not watched");
        assert!(!content_changed_in(&f, &menu_rects(&l).unwrap().0, &l.content_sig));
        assert_eq!(unchanged(&f, &l)["buttons"], true);
        assert!(menu_rects(&ScreenLayout::default()).is_none());
    }
}
