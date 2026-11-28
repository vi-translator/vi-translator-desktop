//! The reading and translating, as the desktop app has always done it, for any host: the host
//! hands in the game picture (or each area's crop with the words its OCR read), and gets back,
//! through its `Sink`, the lines read and translated ("translation"), the game pages ("screen") and
//! status messages ("status-msg"), with the same timing as the desktop's overlays.
//!
//! - `Reader::step`: one pass over the areas of a frame (and the game's pages), as the desktop's
//!   reading thread does a few times a second;
//! - a translator thread: translation memory first, then the chosen translator (translate.rs);
//! - a presenter thread: decides when each area's translation is shown and hidden (`Presenter`).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::frame::FrameData;
use crate::live::Live;
use crate::memory;
use crate::ocr::{self, Crop, Ocr, Word};
use crate::perf::{PerfKind, perf_note};
use crate::translate::{self, Segments, Translator};
use crate::{Paths, Sink, langs, pagescript, phrasebook, screens, speaker};

/// Translation-job id for the reason line of a profile screen.
pub const SCREEN_REASON: &str = "__screen_reason";
/// Translation jobs for pause-menu text (results go to Live::menu_cache).
pub const MENU_TEXT: &str = "__menu";

/// What the reading shares with the host and the other threads.
#[derive(Clone)]
pub struct Context {
    pub cfg: Arc<Mutex<Config>>,
    pub live: Arc<Live>,
    pub memory: Arc<Mutex<memory::Memory>>,
    pub sink: Arc<dyn Sink>,
    pub paths: Paths,
}


#[derive(Serialize, Clone)]
pub struct TranslationEvent {
    pub area: String,
    pub en: String,
    /// The English with the game's button icons in place of its "⟦n⟧" marks, drawn like the
    /// translation (`en` has 🎮 there instead)
    pub en_segments: Segments,
    pub segments: Segments,
    pub stats: String,
    pub error: Option<String>,
    /// Height of the English text line, as a fraction of the frame height (0 = unknown). The
    /// overlay sizes the Arabic from it so both look the same size.
    pub line_h: f64,
    /// The English text block (words + icons) as [x, y, w, h] fractions of the frame, so the Arabic
    /// can line up with the game's own box ("below the game's text box" placement).
    pub text_box: Option<[f64; 4]>,
    /// The game's own box around that text (GTA's help box) as [x, y, w, h] fractions, when its edges
    /// were found: the Arabic box below it takes exactly its left and right edges.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub game_box: Option<[f64; 4]>,
    /// came from the translation memory (shown tinted while testing)
    pub cached: bool,
    /// when the English line appeared on screen (Unix ms; 0 = unknown)
    pub at: u64,
}

// ------------------------------------------------------------------------------------- size lock
//
// The game draws each kind of text (subtitles, instructions, the Mission Failed reason) in one fixed
// font size, but the measured height of a line changes with its letters ("we are" vs "Back off!").
// So the size is measured on the first few lines, then locked to their median — every Arabic line of
// that kind has the same size. Locked sizes are kept in text-sizes.json for the next sessions, with
// the picture's resolution: a picture of another resolution (another capture card or source, the
// card set to another mode) has its text at other sizes, so they're measured again. So are they
// when the player picks another subtitle size in the game (Small, Medium, Large): several lines in
// a row that are all clearly bigger, or all clearly smaller, than the locked size.

const SIZE_SAMPLES: usize = 5;
/// Lines in a row, all this much off the locked size on the same side, that lock a new size.
/// (A single line can be ~30% off by its letters alone: "we are" has no tall ones. Five in a row
/// aren't; GTA's subtitle sizes are ~20% apart.)
const SIZE_DRIFT_LINES: usize = 5;
const SIZE_DRIFT: f64 = 0.15;

#[derive(Serialize, Deserialize, Default)]
pub struct SizeLock {
    locked: HashMap<String, f64>,
    /// The picture's width and height the sizes were measured on (None: not known yet)
    #[serde(default)]
    res: Option<(u32, u32)>,
    #[serde(skip)]
    samples: HashMap<String, Vec<f64>>,
    /// Lines since the last one at the locked size, all off it on the same side
    #[serde(skip)]
    drift: HashMap<String, Vec<f64>>,
    /// where they are kept (None: nowhere)
    #[serde(skip)]
    path: Option<PathBuf>,
}

impl SizeLock {
    /// The sizes kept in `path` (text-sizes.json in the data folder), kept there from now on.
    pub fn load(path: PathBuf) -> Self {
        let mut s: Self = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        s.locked.retain(|_, v| Self::plausible(*v));
        s.path = Some(path);
        s
    }

    /// A line's height as a share of the frame that text can have (a script's mistake, or NaN, isn't).
    fn plausible(v: f64) -> bool {
        v > 0.0 && v <= 0.5
    }

    /// The size to use for this kind of text, given this line's measured height (fractions of the frame).
    fn size(&mut self, kind: &str, measured: f64) -> f64 {
        if !Self::plausible(measured) {
            return self.locked.get(kind).copied().unwrap_or(0.0);
        }
        if let Some(&v) = self.locked.get(kind) {
            let off = measured / v - 1.0;
            let d = self.drift.entry(kind.into()).or_default();
            if off.abs() < SIZE_DRIFT || d.last().is_some_and(|&p| (p / v - 1.0).signum() != off.signum()) {
                d.clear();
            }
            if off.abs() >= SIZE_DRIFT {
                d.push(measured);
            }
            if d.len() < SIZE_DRIFT_LINES {
                return v;
            }
            // the game's subtitle size changed: lock the new one
            let mut sorted = std::mem::take(d);
            sorted.sort_by(|a, b| a.total_cmp(b));
            let median = sorted[sorted.len() / 2];
            self.locked.insert(kind.into(), median);
            self.store();
            return median;
        }
        let v = self.samples.entry(kind.into()).or_default();
        v.push(measured);
        let mut sorted = v.clone();
        sorted.sort_by(|a, b| a.total_cmp(b));
        let median = sorted[sorted.len() / 2];
        if sorted.len() >= SIZE_SAMPLES {
            self.locked.insert(kind.into(), median);
            self.store();
        }
        median
    }

    /// The picture now being read is `w`×`h`: sizes measured on a picture of another resolution are
    /// forgotten, and measured again on the next lines. True when they were.
    fn at_resolution(&mut self, w: u32, h: u32) -> bool {
        if w == 0 || h == 0 || self.res == Some((w, h)) {
            return false;
        }
        // (sizes saved before the resolution was kept: taken as measured on this picture)
        let other = self.res.is_some();
        if other {
            self.locked.clear();
            self.samples.clear();
            self.drift.clear();
        }
        self.res = Some((w, h));
        self.store();
        other
    }

    fn store(&self) {
        // (tests never touch the player's own file)
        if cfg!(test) {
            return;
        }
        let Some(path) = &self.path else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, serde_json::to_string_pretty(self).unwrap_or_default());
    }
}

struct Job {
    area: String,
    seq: u64,
    /// English text; may contain button-icon placeholders "⟦n⟧"
    en: String,
    phrases: Vec<(String, String)>,
    /// Button icons cut from the frame (PNG data URLs), in placeholder order
    icons: Vec<String>,
    /// English line height as a fraction of the frame height (0 = unknown)
    line_h: f64,
    /// When this English line first appeared on screen
    en_start: Instant,
    /// The English text block as [x, y, w, h] fractions of the frame
    text_box: Option<[f64; 4]>,
    /// The game's box around it (see TranslationEvent::game_box)
    game_box: Option<[f64; 4]>,
    /// Two speakers' lines (see ocr::speaker_lines): each part with its brightness; empty = one line
    parts: Vec<(String, f64)>,
}

#[derive(Default)]
struct AreaState {
    candidate: String,
    /// An area whose lines stack up: the candidate's and the current text's lines, top to bottom (a
    /// block is the same block when its lines are, one by one: as one text, a line added to or gone
    /// from three or more still read as "the same")
    candidate_lines: Vec<String>,
    current_lines: Vec<String>,
    candidate_since: Option<Instant>,
    current: String,
    last_seen: Option<Instant>,
    /// translation-job seq of the English line now on screen (0 = none)
    current_seq: u64,
    /// when that line appeared
    current_start: Option<Instant>,
    /// The line on screen was sent without the game's coloured words: read on its first frame, while
    /// GTA VI's instructions still fade in with a white glow. Its colours are looked for again until then.
    colors_until: Option<Instant>,
    /// Where the game's own box was last found in this area (crop pixels), and when: its edges get
    /// lost over a dark scene, its place doesn't change (see ocr::in_game_box).
    game_box: Option<((u32, u32, u32, u32), Instant)>,
    /// The text read in it then.
    game_box_text: String,
    /// The line on screen's English box and game box, as first sent: the same line sent again (its
    /// colours seen) keeps them (a reading with a stray mark made its black box wider, then narrower)
    line_boxes: Option<(Option<[f64; 4]>, Option<[f64; 4]>)>,
    /// When the line on screen was last read (other text read since: it's gone, see the reading), and
    /// whether its end was told already.
    current_seen: Option<Instant>,
    current_ended: bool,
    /// The anchored box's text last read (its first line's left and top, crop pixels): the box's text
    /// stays still from one read to the next, a sign's moves with the camera.
    anchored_at: Option<(u32, u32)>,
}

/// Stacked lines in parts to translate (ocr::speaker_lines), one speaker's lines together: with the
/// speakers' names on, a line with no name of its own is the named one's above it going on (a sentence
/// ending a line split "I don't wanna go out like this, man, come on!" from "Just chill for a second,
/// please! Let me go!", translated alone and wrong, the whole line reviewed). Fewer than two: none.
fn stack_parts(cr: &Crop, words: &[ocr::Word]) -> Vec<(String, f64)> {
    let parts = ocr::speaker_lines(cr, words);
    // (a name of its own: "Name:", or one of the game's speakers with its colon misread — "Tourist.")
    let named = |t: &str| {
        ocr::split_name(t).is_some()
            || t.split_whitespace().next().is_some_and(|w| {
                let n = w.trim_end_matches(|c: char| matches!(c, '.' | ',' | ';' | '•' | '\'' | '?'));
                n.len() < w.len() && n.chars().next().is_some_and(|c| c.is_uppercase()) && speaker::is_known(n)
            })
    };
    if !parts.iter().any(|(t, _)| named(t)) {
        return parts;
    }
    let mut out: Vec<(String, f64)> = Vec::new();
    for (t, b) in parts {
        match out.last_mut() {
            Some((pt, pb)) if !named(&t) && (b - *pb).abs() < 0.1 => {
                pt.push(' ');
                pt.push_str(&t);
            }
            _ => out.push((t, b)),
        }
    }
    if out.len() < 2 { Vec::new() } else { out }
}

/// Levenshtein distance between two texts (characters).
fn edit_distance(a: &str, b: &str) -> usize {
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

/// Each area's highlight colours as last seen this session (for a line never seen in colour yet).
static AREA_COLORS: std::sync::LazyLock<Mutex<HashMap<String, Vec<String>>>> = std::sync::LazyLock::new(Default::default);

/// GTA VI's highlight pink (its instructions' "prison", measured on captured frames).
const GTA6_HIGHLIGHT: &str = "#ff7da7";

/// Each line's highlighted words and colours as last seen this session (its text in lower-case letters
/// and digits -> its phrases): a line that comes back is coloured from its first frame.
static LINE_COLORS: std::sync::LazyLock<Mutex<HashMap<String, Vec<(String, String)>>>> = std::sync::LazyLock::new(Default::default);

/// The same text (OCR's reading differences aside): for stacked lines (`la`, `lb`: their lines), the
/// same lines one by one; else the whole text alike.
fn same_text(a: &str, la: &[String], b: &str, lb: &[String]) -> bool {
    if !la.is_empty() && !lb.is_empty() {
        la.len() == lb.len() && la.iter().zip(lb).all(|(x, y)| ocr::similar(x, y))
    } else {
        ocr::similar(a, b)
    }
}

/// How long the game's box is remembered where it was found last.
const GAME_BOX_KEEP: Duration = Duration::from_secs(15);

/// Instant -> Unix milliseconds.
fn unix_ms(t: Instant) -> u64 {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    now.saturating_sub(t.elapsed()).as_millis() as u64
}

// ------------------------------------------------------------------------------------- presenter
//
// Decides WHEN each area's Arabic is shown and hidden:
//  - an Arabic line stays up as long as its English line was on screen (so it ends as much after the
//    English as it started after it — the translation delay);
//  - when the next line's translation is ready and the current one has had its full time, it
//    switches straight away; if not, the next line waits only until it has;
//  - but a line never outstays its English by more than MAX_LAG: a translation that came very late
//    (e.g. menu text read minutes before it was translated) mustn't hold up the lines after it.

/// The most an Arabic line may stay up after its English left the screen.
const MAX_LAG: Duration = Duration::from_secs(3);

enum PMsg {
    /// A translation is ready to show.
    /// `now`: it replaces the area's translation at once (stacked lines: see Presenter::ready_now).
    Ready { seq: u64, en_start: Instant, ev: TranslationEvent, now: bool },
    /// The English line with this seq left the screen at `at` (gone, or replaced by the next line).
    Ended { area: String, seq: u64, at: Instant },
    /// A full-screen page took over (or the language changed): hide every area now, and drop the
    /// translations of every line read so far (seq up to `upto`) that are still on their way.
    ClearAll { upto: u64 },
}

struct Shown {
    seq: u64,
    shown_at: Instant,
    en_start: Instant,
}

/// What the presenter wants on screen.
#[derive(Debug, PartialEq)]
enum Act {
    Show { area: String, seq: u64 },
    Hide { area: String },
}

/// The timing rules, kept free of the app so they can be tested.
#[derive(Default)]
pub struct Presenter {
    showing: HashMap<String, Shown>,
    pending: HashMap<String, (u64, Instant)>,
    ended: HashMap<(String, u64), Instant>,
    /// translations of lines up to this seq come too late (a page took over since): dropped
    cleared_upto: u64,
    /// areas whose waiting translation goes up at once (see ready_now)
    now: std::collections::HashSet<String>,
    /// A translation leaves with its English (the app's): not held up for as long as it took to come
    /// (a line the translator took 600 ms for stayed 600 ms after its English left)
    follow: bool,
}

/// How long the end of an English line is remembered (a translation can come minutes late: the
/// first one, while the offline model downloads).
const ENDED_KEEP: Duration = Duration::from_secs(3600);

impl Presenter {
    /// A translation is ready; false (dropped) when its line was cleared by a page meanwhile.
    fn ready(&mut self, area: &str, seq: u64, en_start: Instant) -> bool {
        if seq <= self.cleared_upto {
            return false;
        }
        self.pending.insert(area.into(), (seq, en_start)); // only the newest ready line waits
        true
    }
    /// A translation that replaces the area's at once, not after the one up has had its time: lines
    /// that stack up (GTA VI's subtitles) come and go as one block, the lines up already being part
    /// of the new block (a line added under them, or the top one gone): holding the old block up meant
    /// English on screen without its translation for a moment each time.
    fn ready_now(&mut self, area: &str, seq: u64, en_start: Instant) -> bool {
        let ok = self.ready(area, seq, en_start);
        if ok {
            self.now.insert(area.into());
        }
        ok
    }
    fn ended(&mut self, area: &str, seq: u64, at: Instant) {
        self.ended.insert((area.into(), seq), at);
        // (a line after the one up ended, the one up's end never told — stacked lines don't end the
        // block up when a line joins it: see the reading): it ended then too
        if let Some(s) = self.showing.get(area).filter(|s| s.seq < seq) {
            self.ended.entry((area.into(), s.seq)).or_insert(at);
        }
    }
    fn clear_all(&mut self, upto: u64) -> Vec<Act> {
        self.cleared_upto = self.cleared_upto.max(upto);
        self.pending.clear();
        self.showing.drain().map(|(area, _)| Act::Hide { area }).collect()
    }
    fn tick(&mut self, now: Instant) -> Vec<Act> {
        let mut acts = Vec::new();
        let areas: Vec<String> = self.showing.keys().chain(self.pending.keys()).cloned().collect();
        for area in areas {
            // Has the line on screen been up as long as its English was? Its time ends as long after the
            // English ended as the Arabic came after the English started (at most MAX_LAG).
            let done = match self.showing.get(&area) {
                None => true,
                Some(_) if self.now.contains(&area) && self.pending.contains_key(&area) => true,
                Some(s) => {
                    // (its end was never told — the record lost, or a path that forgot it: a newer line
                    // of this area waiting means its English had gone by the time that one started)
                    let end = self.ended.get(&(area.clone(), s.seq)).copied().or_else(|| self.pending.get(&area).filter(|p| p.0 > s.seq).map(|p| p.1));
                    end.is_some_and(|end| {
                        let lag = if self.follow { Duration::ZERO } else { s.shown_at.saturating_duration_since(s.en_start).min(MAX_LAG) };
                        now >= end.max(s.shown_at) + lag.min(end.saturating_duration_since(s.en_start))
                    })
                }
            };
            if !done {
                continue;
            }
            if let Some((seq, en_start)) = self.pending.remove(&area) {
                self.now.remove(&area);
                self.showing.insert(area.clone(), Shown { seq, shown_at: now, en_start });
                acts.push(Act::Show { area, seq });
            } else if self.showing.remove(&area).is_some() {
                acts.push(Act::Hide { area });
            }
        }
        self.ended.retain(|_, at| now.saturating_duration_since(*at) < ENDED_KEEP);
        acts
    }
}

/// The shortest time between two readings (ms): about a frame of a 60 fps capture.
pub const MIN_INTERVAL_MS: u64 = 16;

/// An area whose lines stack up, read in parts from the bottom (`grown`: its whole height,
/// `stack_grown`): the area itself first, where line 1 and the next two are, then a band of about
/// three lines above it only when the lines read go up to the top of what's been read (another line
/// could be there), and so on up. Most of the time there's no text, or one or two lines: one small
/// picture for the OCR (about 10 ms at 1080p, against 55 for the whole height); each band above
/// costs about as much again, and only while the lines go on.
/// Parts overlap by OV (more than a line's height): a line across a border is taken from the part its
/// middle is in, where it's whole. Words come back in the whole height's pixels, with its crop (the
/// colours, icons and boxes are looked for there).
pub fn read_stacked(ocr: &ocr::Ocr, rgba: &[u8], fw: u32, fh: u32, base: &crate::config::Area, grown: &crate::config::Area) -> (Option<Crop>, Vec<ocr::Word>, bool) {
    const OV: f64 = 0.04;
    const BAND: f64 = 0.125; // (three of GTA VI's lines: 44 px apart at 1080p)
    let crop = Crop::from_frame(rgba, fw, fh, grown.x, grown.y, grown.w, grown.h);
    let Some(gh) = crop.as_ref().map(|c| c.h as f64) else { return (crop, Vec::new(), true) };
    let row = |f: f64| (f.clamp(0.0, 1.0) * fh as f64) as u32;
    let ov = row(OV).max(8);
    let mid = |w: &ocr::Word| w.rect.1 + w.rect.3 / 2;
    let shift = |w: ocr::Word, dy: u32, line: u32| ocr::Word { rect: (w.rect.0, w.rect.1 + dy, w.rect.2, w.rect.3), line: w.line + line, ..w };
    let (mut words, mut sure) = (Vec::new(), true);
    // (the part read last: its top, as a fraction of the picture and in the whole height's pixels)
    let mut top = base.y.max(grown.y);
    for n in 0u32.. {
        let (y, h) = if n == 0 { (top, grown.y + grown.h - top) } else { ((top - BAND).max(grown.y), (top - (top - BAND).max(grown.y)) + OV) };
        let Some(part) = Crop::from_frame(rgba, fw, fh, grown.x, y, grown.w, h) else { break };
        let (got, ok) = ocr.recognize_checked(&part).unwrap_or((Vec::new(), true));
        sure &= ok;
        let dy = row(y).saturating_sub(row(grown.y));
        let border = row(top).saturating_sub(row(grown.y)); // (the top of the part below, in the whole height)
        if n > 0 {
            // A line across the border: from this part when its middle is above the border's middle.
            // A word the border cut in the part below (its middle there is lower than the whole word's)
            // goes too when this part has it whole: read twice, "Tourists: Tourists: Okay now…".
            let above: Vec<ocr::Word> = got.into_iter().map(|w| shift(w, dy, 10_000 * n)).filter(|w| mid(w) < border + ov / 2).collect();
            let same = |a: &ocr::Word, b: &ocr::Word| {
                let x = (a.rect.0 + a.rect.2).min(b.rect.0 + b.rect.2).saturating_sub(a.rect.0.max(b.rect.0));
                let ys = a.rect.1 < b.rect.1 + b.rect.3 && b.rect.1 < a.rect.1 + a.rect.3;
                ys && x * 2 >= a.rect.2.min(b.rect.2)
            };
            words.retain(|w: &ocr::Word| mid(w) >= border + ov / 2 && !above.iter().any(|u| same(u, w)));
            words.extend(above);
        } else {
            words.extend(got.into_iter().map(|w| shift(w, dy, 0)));
        }
        top = y;
        if top <= grown.y + 1e-9 {
            break;
        }
        // Up again only when the lines read so far go up to near this part's top: a line above
        // joins one at most 2.4 line heights over it, and one lower than that was read whole here.
        let stack = ocr::bottom_stack(&words, gh);
        let mut hs: Vec<u32> = stack.iter().map(|w| w.rect.3).collect();
        hs.sort_unstable();
        let lh = hs.get(hs.len() * 9 / 10).or(hs.last()).copied().unwrap_or(0) as f64;
        let highest = stack.iter().map(|w| mid(w) as f64).fold(f64::MAX, f64::min);
        let part_top = row(top).saturating_sub(row(grown.y)) as f64;
        if stack.is_empty() || highest - part_top >= 3.0 * lh {
            break;
        }
    }
    (crop, words, sure)
}

/// The game's text seen in an area, not translated on screen yet (`Live::covers`): set (Some) or gone
/// (None), told to the overlays ("cover") when it changes.
fn set_cover(sink: &dyn Sink, live: &Live, area: &str, cover: Option<serde_json::Value>) {
    let changed = {
        let mut m = live.covers.lock().unwrap();
        match cover {
            Some(v) => m.insert(area.to_string(), v.clone()).as_ref() != Some(&v),
            None => m.remove(area).is_some(),
        }
    };
    if changed {
        let v = live.covers.lock().unwrap().get(area).cloned();
        sink.emit("cover", serde_json::json!({ "area": area, "cover": v }));
    }
}

fn present_loop(sink: Arc<dyn Sink>, rx: mpsc::Receiver<PMsg>, live: Arc<Live>) {
    let mut p = Presenter { follow: true, ..Presenter::default() };
    let mut events: HashMap<(String, u64), TranslationEvent> = HashMap::new();
    loop {
        let mut msgs = Vec::new();
        match rx.recv_timeout(Duration::from_millis(40)) {
            Ok(m) => msgs.push(m),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }
        msgs.extend(rx.try_iter());
        let mut acts = Vec::new();
        for m in msgs {
            match m {
                PMsg::Ready { seq, en_start, ev, now } => {
                    if if now { p.ready_now(&ev.area, seq, en_start) } else { p.ready(&ev.area, seq, en_start) } {
                        events.retain(|(a, _), _| *a != ev.area); // keep only the newest waiting line
                        events.insert((ev.area.clone(), seq), ev);
                    }
                }
                PMsg::Ended { area, seq, at } => p.ended(&area, seq, at),
                PMsg::ClearAll { upto } => {
                    events.clear();
                    let areas: Vec<String> = live.covers.lock().unwrap().keys().cloned().collect();
                    for a in areas {
                        set_cover(&*sink, &live, &a, None);
                    }
                    acts.extend(p.clear_all(upto));
                }
            }
        }
        acts.extend(p.tick(Instant::now()));
        for act in acts {
            match act {
                Act::Show { area, seq } => {
                    if let Some(ev) = events.remove(&(area.clone(), seq)) {
                        // (the text seen there is this line, or one before it: its translation is up now)
                        let cover_seq = live.covers.lock().unwrap().get(&area).map(|c| c["seq"].as_u64().unwrap_or(0));
                        if cover_seq.is_some_and(|s| s != 0 && s <= seq) {
                            set_cover(&*sink, &live, &area, None);
                        }
                        let ar: String = ev
                            .segments
                            .iter()
                            .map(|(t, c)| if c.as_deref().is_some_and(|c| c.starts_with("icon:")) { "🎮" } else { t.as_str() })
                            .collect();
                        live.on_screen.lock().unwrap().insert(area.clone(), serde_json::json!({ "en": ev.en, "ar": ar.trim() }));
                        live.shown.lock().unwrap().insert(area.clone(), serde_json::to_value(&ev).unwrap_or_default());
                        crate::live::push(&live, &area, &ev.en, &ev.en_segments, &ev.segments, ev.at);
                        sink.emit("translation", serde_json::to_value(&ev).unwrap_or_default());
                    }
                }
                Act::Hide { area } => {
                    live.on_screen.lock().unwrap().remove(&area);
                    live.shown.lock().unwrap().remove(&area);
                    let ev = TranslationEvent { area, en: String::new(), en_segments: Vec::new(), segments: Vec::new(), stats: String::new(), error: None, line_h: 0.0, text_box: None, game_box: None, cached: false, at: 0 };
                    sink.emit("translation", serde_json::to_value(&ev).unwrap_or_default());
                }
            }
        }
    }
}

#[cfg(test)]
mod presenter_tests {
    use super::*;
    const A: &str = "subtitles";
    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn arabic_stays_as_long_as_the_english_did() {
        let t = Instant::now();
        let mut p = Presenter::default();
        p.ready(A, 1, t); // English appeared at 0
        assert_eq!(p.tick(t + ms(600)), vec![Act::Show { area: A.into(), seq: 1 }]); // translated after 0.6 s
        p.ended(A, 1, t + ms(3000)); // English was up 3 s
        assert_eq!(p.tick(t + ms(3100)), vec![]); // Arabic only up 2.5 s: stays
        assert_eq!(p.tick(t + ms(3590)), vec![]);
        assert_eq!(p.tick(t + ms(3600)), vec![Act::Hide { area: A.into() }]); // 3 s after it appeared
    }

    #[test]
    fn next_line_waits_until_the_current_one_had_its_time() {
        let t = Instant::now();
        let mut p = Presenter::default();
        p.ready(A, 1, t);
        p.tick(t + ms(800)); // line 1 shown at 0.8 s
        p.ended(A, 1, t + ms(2000)); // replaced by line 2 at 2 s (line 1 was up 2 s)
        p.ready(A, 2, t + ms(2000));
        assert_eq!(p.tick(t + ms(2500)), vec![]); // line 2 ready, but line 1 only shown 1.7 s
        assert_eq!(p.tick(t + ms(2800)), vec![Act::Show { area: A.into(), seq: 2 }]); // switches, no gap
    }

    #[test]
    fn next_line_switches_immediately_when_the_current_one_had_its_time() {
        let t = Instant::now();
        let mut p = Presenter::default();
        p.ready(A, 1, t);
        p.tick(t + ms(500)); // shown at 0.5 s
        p.ended(A, 1, t + ms(1000)); // English up 1 s
        // translation of the next line arrives late (at 3 s): line 1 has had its 1 s long ago
        p.ready(A, 2, t + ms(1000));
        assert_eq!(p.tick(t + ms(3000)), vec![Act::Show { area: A.into(), seq: 2 }]);
    }

    #[test]
    fn a_very_late_translation_does_not_block_the_next_lines() {
        let t = Instant::now();
        let mut p = Presenter::default();
        p.ready(A, 1, t); // menu text seen at 0 ...
        p.tick(t + ms(320_000)); // ... but only translated and shown 320 s later
        p.ended(A, 1, t + ms(321_000)); // its English goes 1 s after that
        p.ready(A, 2, t + ms(321_000));
        assert_eq!(p.tick(t + ms(323_900)), vec![]); // line 1 stays at most MAX_LAG after its English left
        assert_eq!(p.tick(t + ms(324_000)), vec![Act::Show { area: A.into(), seq: 2 }]); // not 320 s later
    }

    #[test]
    fn page_takeover_hides_at_once() {
        let t = Instant::now();
        let mut p = Presenter::default();
        p.ready(A, 1, t);
        p.tick(t + ms(500));
        p.ready(A, 2, t + ms(900));
        assert_eq!(p.clear_all(2), vec![Act::Hide { area: A.into() }]);
        assert_eq!(p.tick(t + ms(5000)), vec![]); // nothing left waiting
    }

    #[test]
    fn a_translation_of_a_line_cleared_by_a_page_is_dropped() {
        let t = Instant::now();
        let mut p = Presenter::default();
        p.clear_all(2); // a page took over while line 2 was being translated
        assert!(!p.ready(A, 2, t));
        assert_eq!(p.tick(t + ms(100)), vec![]);
        // a line read after the page
        assert!(p.ready(A, 3, t + ms(200)));
        assert_eq!(p.tick(t + ms(300)), vec![Act::Show { area: A.into(), seq: 3 }]);
    }

    #[test]
    fn a_line_whose_end_was_never_told_gives_way_to_the_next() {
        let t = Instant::now();
        let mut p = Presenter::default();
        p.ready(A, 1, t);
        assert_eq!(p.tick(t + ms(500)), vec![Act::Show { area: A.into(), seq: 1 }]);
        // no Ended for 1; line 2's English started at 60 s
        p.ready(A, 2, t + ms(60_000));
        assert_eq!(p.tick(t + ms(60_000)), vec![]); // 1 still has its own time (its lag)
        assert_eq!(p.tick(t + ms(60_600)), vec![Act::Show { area: A.into(), seq: 2 }]);
    }

    #[test]
    fn a_very_late_translation_still_knows_when_its_english_ended() {
        let t = Instant::now();
        let mut p = Presenter::default();
        p.ended(A, 1, t + ms(4_000));
        p.ready(A, 1, t);
        assert_eq!(p.tick(t + ms(200_000)), vec![Act::Show { area: A.into(), seq: 1 }]); // 200 s late: the end is still known
        assert_eq!(p.tick(t + ms(203_100)), vec![Act::Hide { area: A.into() }]); // gone at most MAX_LAG later
    }

    #[test]
    fn a_misreading_is_nearer_the_line_up_than_another_line() {
        assert_eq!(edit_distance("getinvehicle", "getinavehicle"), 1);
        assert_eq!(edit_distance("getinvehicle", "getinthevehicle"), 3);
        assert_eq!(edit_distance("getifvehicle", "getinavehicle"), 2);
        assert_eq!(edit_distance("", "abc"), 3);
    }

    #[test]
    fn a_block_of_stacked_lines_is_the_same_when_its_lines_are() {
        let l = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let three = l(&["Tourist: Eyes on the road, you're gonna get someone killed!", "Tourist: Oh! God... c'mon, now, sweetie... think of your momma...", "Tourist: Hey trust me, you don't wanna do this..."]);
        let moved = l(&["Tourist: Oh! God... c'mon, now, sweetie... think of your momma...", "Tourist: Hey trust me, you don't wanna do this...", "Cops: Stop right fucking now!"]);
        let noisy = l(&["Tourist: Eyes on the road, you're gonna get someone killed!", "Tourist. Ohl God... c'mon, now, sweetie... think of your momma...", "Tourist: Hey trust me, you don't wanna do this..."]);
        assert!(same_text(&three.join(" "), &three, &noisy.join(" "), &noisy)); // (a misread letter or two)
        assert!(!same_text(&three.join(" "), &three, &moved.join(" "), &moved)); // a line gone, one added
        assert!(!same_text(&three.join(" "), &three, &three[1..].join(" "), &three[1..])); // the top one gone
    }

    #[test]
    fn a_translation_that_follows_leaves_with_its_english() {
        let t = Instant::now();
        let mut p = Presenter { follow: true, ..Presenter::default() };
        p.ready(A, 1, t);
        assert_eq!(p.tick(t + ms(600)), vec![Act::Show { area: A.into(), seq: 1 }]); // 600 ms to translate
        p.ended(A, 1, t + ms(3000));
        assert_eq!(p.tick(t + ms(3001)), vec![Act::Hide { area: A.into() }]); // not 600 ms later
        // stacked: a line joined at 5 s (no end told for the block up), the new block's translation
        // replaces it when ready; the new block's English gone ends the old one too if it never came
        p.ready(A, 2, t + ms(4000));
        assert_eq!(p.tick(t + ms(4100)), vec![Act::Show { area: A.into(), seq: 2 }]);
        assert!(p.tick(t + ms(5050)).is_empty()); // (seq 3 seen at 5 s: no end for 2)
        p.ended(A, 3, t + ms(5500)); // seq 3's English gone before its translation came
        assert_eq!(p.tick(t + ms(5501)), vec![Act::Hide { area: A.into() }]);
    }

    #[test]
    fn stacked_lines_replace_the_block_at_once() {
        let t = Instant::now();
        let mut p = Presenter::default();
        p.ready(A, 1, t);
        assert_eq!(p.tick(t + ms(300)), vec![Act::Show { area: A.into(), seq: 1 }]); // 300 ms after its English
        // a line added under it at 2 s: the new block's translation goes up as soon as it's ready, not
        // 300 ms later (the time the first block took)
        p.ended(A, 1, t + ms(2000));
        p.ready_now(A, 2, t + ms(2000));
        assert_eq!(p.tick(t + ms(2050)), vec![Act::Show { area: A.into(), seq: 2 }]);
        // (and a plain one waits as before)
        p.ended(A, 2, t + ms(4000));
        p.ready(A, 3, t + ms(4000));
        assert!(p.tick(t + ms(4010)).is_empty());
    }
}

// ------------------------------------------------------------------------------------- reading

/// What one pass of the reading looks at.
pub enum Input<'a> {
    /// No picture now (nothing captured yet, the source closed): what was up goes after a while.
    Nothing,
    /// The whole frame, read with the host's OCR: the areas, and the game's pages.
    Frame(&'a Arc<FrameData>, &'a Ocr),
    /// The frame's size, and each area's crop (by area id) with the words the host's own OCR read in
    /// it. No game pages (they need the whole frame and OCR of their own regions). An area missing
    /// here reads as empty.
    Areas { w: u32, h: u32, reads: HashMap<String, AreaRead> },
}

/// One area read by the host: its crop (as `Crop::from_frame` cuts it) and the words in it, in crop
/// pixels. `sure`: the reading is settled (false: text still animating in; it waits longer).
pub struct AreaRead {
    pub crop: Crop,
    pub words: Vec<Word>,
    pub sure: bool,
}

/// The reading's state from one pass to the next.
struct ReadState {
    sizes: SizeLock,
    states: HashMap<String, AreaState>,
    seq_counter: u64,
    screen: Option<screens::ScreenLayout>,
    screen_checked: Instant,
    // when the red title was last seen missing (None = it is on screen)
    title_missing_since: Option<Instant>,
    // pause-menu text already sent to the translator (once per session)
    menu_asked: std::collections::HashSet<String>,
    // Every state of a pause-menu tab seen so far (tab, picture of the menu area, its lines with
    // Arabic). Some pages alternate by themselves — the Settings controller diagram switches
    // between its "press" and "hold" labels every ~3 s — and a state seen before comes back at
    // once, complete, without reading it again.
    menu_states: Vec<(String, Vec<u8>, Vec<screens::MenuText>)>,
    // target language of what is on screen and in the caches
    last_lang: String,
    // a profile page still has pieces waiting for the translator (other languages than Arabic)
    screen_missing: bool,
    // the game's pages script (pagescript.rs), loaded again when the game or the script changes
    pages: Option<pagescript::Pages>,
    pages_for: (String, u64),
    // a load that left the game without pages (a script that failed to load) is tried again a few
    // times, spaced out: how many so far, and when the last was
    pages_tries: (u32, Instant),
    // when a page followed like a menu was last read with OCR (see `settling` below)
    menu_read_at: Instant,
    // no picture since (the window closed, the card unplugged, OBS failing)
    no_frame_since: Option<Instant>,
    // pause-menu text the translator couldn't translate is asked again now and then
    menu_retry_at: Instant,
    // the page on screen last time round (to tell its script when it goes)
    ended_check: Option<String>,
}

impl ReadState {
    fn new(paths: &Paths) -> Self {
        ReadState {
            sizes: SizeLock::load(paths.data.join("text-sizes.json")),
            states: HashMap::new(),
            seq_counter: 0,
            screen: None,
            screen_checked: Instant::now() - Duration::from_secs(10),
            title_missing_since: None,
            menu_asked: std::collections::HashSet::new(),
            menu_states: Vec::new(),
            last_lang: String::new(),
            screen_missing: false,
            pages: None,
            pages_for: (String::new(), u64::MAX),
            pages_tries: (0, Instant::now()),
            menu_read_at: Instant::now() - Duration::from_secs(10),
            no_frame_since: None,
            menu_retry_at: Instant::now(),
            ended_check: None,
        }
    }
}

/// The reading, driven by the host: `step` a few times a second (it says how long to wait before
/// the next). Its translator and presenter threads end when it's dropped. Not `Send` (a game's pages
/// script lives in it): make it on the thread that steps it.
pub struct Reader {
    ctx: Context,
    latest: Arc<Mutex<HashMap<String, u64>>>,
    jobs: mpsc::Sender<Job>,
    present: mpsc::Sender<PMsg>,
    state: Option<ReadState>,
}

/// Start the translator and presenter threads, and the reading that feeds them.
pub fn start(ctx: Context) -> Reader {
    let latest = Arc::new(Mutex::new(HashMap::new()));
    let (tx, rx) = mpsc::channel::<Job>();
    let (ptx, prx) = mpsc::channel::<PMsg>();
    {
        let (s, l) = (ctx.sink.clone(), ctx.live.clone());
        std::thread::spawn(move || present_loop(s, prx, l));
    }
    {
        let (c, l, p) = (ctx.clone(), latest.clone(), ptx.clone());
        std::thread::spawn(move || translate_loop(c, l, rx, p));
    }
    Reader { state: Some(ReadState::new(&ctx.paths)), ctx, latest, jobs: tx, present: ptx }
}

impl Reader {
    pub fn context(&self) -> &Context {
        &self.ctx
    }

    /// One pass: every enabled area of the input read; text that stays the same long enough goes
    /// to the translator; a game page found takes over. How long to wait before the next pass.
    pub fn step(&mut self, input: Input) -> Duration {
        let ReadState {
            mut sizes,
            mut states,
            mut seq_counter,
            mut screen,
            mut screen_checked,
            mut title_missing_since,
            mut menu_asked,
            mut menu_states,
            mut last_lang,
            mut screen_missing,
            mut pages,
            mut pages_for,
            mut pages_tries,
            mut menu_read_at,
            mut no_frame_since,
            mut menu_retry_at,
            mut ended_check,
        } = self.state.take().expect("the reading's state");
        let (ctx, latest, jobs, present) = (&self.ctx, &self.latest, &self.jobs, &self.present);
        macro_rules! done {
            ($wait:expr) => {{
                let wait = $wait;
                self.state = Some(ReadState {
                    sizes,
                    states,
                    seq_counter,
                    screen,
                    screen_checked,
                    title_missing_since,
                    menu_asked,
                    menu_states,
                    last_lang,
                    screen_missing,
                    pages,
                    pages_for,
                    pages_tries,
                    menu_read_at,
                    no_frame_since,
                    menu_retry_at,
                    ended_check,
                });
                return wait;
            }};
        }
        let c = ctx.cfg.lock().unwrap().clone();
        let t0 = Instant::now();
        // (the player's, or the game's own: see Config::timing)
        let (interval_ms, stable_ms, clear_after_ms) = c.timing();
        // (VI Translator's ready translations used or not: the memory takes them in or leaves them out)
        ctx.memory.lock().unwrap().use_shared(c.shared_translations);
        if c.target_lang != last_lang {
            // another language: everything shown and cached is in the old one
            last_lang = c.target_lang.clone();
            let l = langs::get(&c.target_lang);
            *ctx.live.live_lang.lock().unwrap() = serde_json::json!({ "code": l.code, "rtl": l.rtl });
            ctx.memory.lock().unwrap().set_lang(&c.target_lang);
            menu_asked.clear();
            menu_states.clear();
            {
                let mut mc = ctx.live.menu_cache.lock().unwrap();
                mc.lang = c.target_lang.clone();
                mc.map.clear();
            }
            ctx.live.menu_failed.lock().unwrap().clear();
            if screen.is_some() {
                hide_screen(&*ctx.sink, &ctx.live, &latest, &mut screen);
            }
            for st in states.values_mut() {
                st.current.clear();
                st.current_lines.clear();
                st.candidate.clear();
                st.candidate_lines.clear();
            }
            let _ = present.send(PMsg::ClearAll { upto: seq_counter });
        }
        // pause-menu text the translator failed on: asked again when it's read again (every 30 s at most)
        if menu_retry_at.elapsed() >= Duration::from_secs(30) {
            menu_retry_at = Instant::now();
            for en in std::mem::take(&mut *ctx.live.menu_failed.lock().unwrap()) {
                menu_asked.remove(&en);
            }
        }
        screens::use_words_for(&c.game(), &c.target_lang);
        let (frame, ocr_now, mut reads, size) = match input {
            Input::Nothing => (None, None, HashMap::new(), None),
            Input::Frame(f, o) => (Some(f), Some(o), HashMap::new(), Some((f.w, f.h))),
            Input::Areas { w, h, reads } => (None, None, reads, Some((w, h))),
        };
        if size.is_none() {
            // No picture: once that has lasted as long as an English line may blink out, what was up
            // goes, as if the game took it away (else a page and the last line stay on the OBS page).
            let since = *no_frame_since.get_or_insert_with(Instant::now);
            // (a stream from a console drops frames now and then: a gap shorter than the floor isn't the
            // game taking its text away)
            if since.elapsed() >= Duration::from_millis(clear_after_ms.max(DROPOUT_FLOOR_MS)) {
                if screen.is_some() {
                    hide_screen(&*ctx.sink, &ctx.live, &latest, &mut screen);
                }
                for (id, st) in states.iter_mut() {
                    if st.current_seq != 0 {
                        let at = st.last_seen.unwrap_or(since);
                        let _ = present.send(PMsg::Ended { area: id.clone(), seq: st.current_seq, at });
                    }
                    *st = AreaState::default();
                }
            }
        } else {
            no_frame_since = None;
        }
        if let Some((fw, fh)) = size {
            // another resolution: the text sizes are measured again
            sizes.at_resolution(fw, fh);
            if let (Some(frame), Some(ocr)) = (frame, ocr_now) {
                // ---- game pages (the game's pages script finds and reads them; pagescript.rs) ----
                // Appear: each page's `present` (cheap) every frame, then its `read`.
                // Stay: `same` every frame — the translation stays exactly as long as the page does — with
                //       the page's hold rules, or followed like a menu (`hold.follow`).
                let now_for = (c.game(), pagescript::GENERATION.load(std::sync::atomic::Ordering::Relaxed));
                let changed = now_for != pages_for;
                let retry = pages.is_none()
                    && pages_tries.0 < 3
                    && pages_tries.1.elapsed() >= Duration::from_secs(30)
                    && pagescript::files(&ctx.paths.data, &now_for.0).0.exists();
                if changed || retry {
                    pages_tries = (if changed { 0 } else { pages_tries.0 + 1 }, Instant::now());
                    // (the states a menu's tabs were seen in belong to the script that read them)
                    menu_states.clear();
                    pages = pagescript::Pages::for_game(&now_for.0, &ctx.paths.data);
                    if let (Some(p), true) = (&pages, std::env::var_os("VI_TRANSLATOR_DEBUG").is_some()) {
                        eprintln!("[pages] {} script ({}): {:?}", now_for.0, p.origin, p.list.iter().map(|i| &i.id).collect::<Vec<_>>());
                    }
                    pages_for = now_for;
                }
                // the script's pages whose switch is on in the profile
                let script_pages: Vec<String> = pages
                    .as_ref()
                    .map(|p| p.list.iter().filter(|i| p.page_on(&c.profile_opts, i)).map(|i| i.id.clone()).collect())
                    .unwrap_or_default();
                let screens_on = !script_pages.is_empty();
                // (the parts of a page the player switches one by one: what its script reads, `vi.on`)
                if let Some(p) = &pages {
                    p.set_switches(&c.profile_opts);
                }
                let mut found: Option<screens::ScreenLayout> = None;
                // A page on screen whose switch was just turned off (Game pages) goes now, not when the game
                // closes it.
                if let Some(old) = &screen {
                    let on = pages.as_ref().and_then(|p| p.info(&old.id).map(|i| p.page_on(&c.profile_opts, i)));
                    // (a page its script no longer has goes too)
                    if on != Some(true) {
                        hide_screen(&*ctx.sink, &ctx.live, &latest, &mut screen);
                    }
                }
                if screens_on {
                    match &screen {
                        None => {
                            if let Some(p) = &pages {
                                for id in &script_pages {
                                    if let Some(k) = p.present(id, &frame, &ocr) {
                                        found = p.read(id, &frame, &ocr, &k);
                                        if found.is_some() {
                                            break;
                                        }
                                    }
                                }
                            }
                        }
                        // A script's page is up. Without hold rules (captions): still the same? Else read it
                        // again, or it's gone. With them (Mission Failed): kept while its title blinks out
                        // (up to hold_ms), ended once the screen is dark, read again every reread_ms.
                        Some(old) if screens::menu_rects(old).is_none() && pages.as_ref().is_some_and(|p| p.has(&old.id)) => {
                            let p = pages.as_ref().expect("checked");
                            let id = old.id.clone();
                            let info = p.info(&id).cloned().unwrap_or_default();
                            let still = p.same(&id, &frame, &ocr, old) == Some(true);
                            if info.hold_ms == 0 && info.reread_ms == 0 {
                                if !still {
                                    match p.present(&id, &frame, &ocr).and_then(|k| p.read(&id, &frame, &ocr, &k)) {
                                        Some(l) => found = Some(l),
                                        None => hide_screen(&*ctx.sink, &ctx.live, &latest, &mut screen),
                                    }
                                }
                            } else {
                                if still {
                                    title_missing_since = None;
                                }
                                let missing = title_missing_since.get_or_insert_with(Instant::now).elapsed();
                                if !still && ((info.end_when_dark && screens::is_dark(&frame)) || missing > Duration::from_millis(info.hold_ms)) {
                                    hide_screen(&*ctx.sink, &ctx.live, &latest, &mut screen);
                                    title_missing_since = None;
                                } else if !still {
                                    // still fading: keep showing, no re-read
                                } else if info.reread_ms > 0 && screen_checked.elapsed() >= Duration::from_millis(info.reread_ms) {
                                    screen_checked = Instant::now();
                                    if let Some(mut l) = p.present(&id, &frame, &ocr).and_then(|k| p.read(&id, &frame, &ocr, &k)) {
                                        screens::keep_measured(&mut l, old);
                                        found = Some(l);
                                    }
                                }
                            }
                        }
                        // A page followed like a menu (`hold.follow`): gone the moment the script's `same`
                        // says so; its boxes watched every frame, read again when they change.
                        Some(old) if screens::menu_rects(old).is_some() && pages.as_ref().is_some_and(|p| p.has(&old.id)) => {
                            let id = old.id.clone();
                            let (content_rect, buttons_rect) = screens::menu_rects(old).expect("checked");
                            let p = pages.as_ref().expect("checked");
                            let pause_present = |f: &FrameData| p.same(&id, f, &ocr, old) == Some(true);
                            let reread = |f: &FrameData| match p.can(&id, "reread") {
                                true => p.reread(&id, f, &ocr, old),
                                false => p.present(&id, f, &ocr).and_then(|k| p.read(&id, f, &ocr, &k)),
                            };
                            let full_read = |f: &FrameData| p.present(&id, f, &ocr).and_then(|k| p.read(&id, f, &ocr, &k));
                            // Every frame: a menu line whose English changed or went away loses its Arabic
                            // at once, and the menu is read again straight away.
                            let gone = old.menu.iter().any(|m| !screens::region_same(&frame, &m.rect, &m.sig))
                                || old.labels.iter().any(|l| !screens::region_same(&frame, &l.rect, &l.sig))
                                || !screens::region_same(&frame, &buttons_rect, &old.buttons_sig);
                            // Read again only when something changed (or every 3 s, to be safe): re-reading a
                            // still menu only makes OCR's boxes shift a few pixels.
                            let changed = screens::content_changed_in(&frame, &content_rect, &old.content_sig);
                            let due = screen_checked.elapsed() >= Duration::from_secs(3);
                            // Only the content area's picture moved, not a translated box, tab or button row (an
                            // animated picture, a live map, the game behind the menu): read again at most once
                            // a second, not every frame.
                            let settling = changed && !gone && !due && menu_read_at.elapsed() < Duration::from_secs(1);
                            // a state of this tab seen before, matching what's on screen now
                            let tab = format!("{id}/{}", screens::selected_tab_name(old).unwrap_or(""));
                            let known = (gone || changed)
                                .then(|| menu_states.iter().find(|(t, sig, _)| *t == tab && !screens::content_changed_in(&frame, &content_rect, sig)))
                                .flatten()
                                .filter(|(_, sig, _)| *sig != old.content_sig);
                            if !pause_present(&frame) {
                                hide_screen(&*ctx.sink, &ctx.live, &latest, &mut screen);
                            } else if let Some((_, sig, menu)) = known {
                                let mut l = old.clone();
                                (l.menu, l.content_sig) = (menu.clone(), sig.clone());
                                screen_checked = Instant::now();
                                found = Some(l);
                            } else if gone && !due && old.labels.iter().all(|l| screens::region_same(&frame, &l.rect, &l.sig)) {
                                // Same tab, its content changed (the controller diagram swapped labels): read
                                // the menu again right now (~150 ms), so the old lines go and the new ones come
                                // together, with no half-empty moment in between.
                                screen_checked = Instant::now();
                                menu_read_at = Instant::now();
                                match reread(&frame) {
                                    Some(l) => found = Some(l),
                                    None => {
                                        // The page wasn't ready (a list's selection still sliding): keep its list rows
                                        // (the same words, a highlight moving past them: not dropped to English for a
                                        // moment), but not other text that changed (a panel showing the next topic);
                                        // and try again shortly, not in 3 s.
                                        let mut l = old.clone();
                                        l.menu.retain(|m| m.item || screens::region_same(&frame, &m.rect, &m.sig));
                                        found = Some(l);
                                        screen_checked = Instant::now() - Duration::from_millis(2800);
                                    }
                                }
                            } else if gone && !due {
                                // drop what changed now; read the menu again on the next pass (`due`)
                                let mut l = old.clone();
                                l.menu.retain(|m| screens::region_same(&frame, &m.rect, &m.sig));
                                screen_checked = Instant::now() - Duration::from_secs(10);
                                found = Some(l);
                            } else if settling || (!changed && !due && !gone) {
                                // nothing new on screen (or only the picture behind it moving): just add
                                // translations that arrived meanwhile
                                if old.menu.iter().any(|m| m.ar.is_empty()) || screen_missing {
                                    found = Some(old.clone());
                                }
                            } else {
                                screen_checked = Instant::now();
                                menu_read_at = Instant::now();
                                // same tabs as before: a quick re-read of just the menu; else a full read
                                let tabs_same = old.labels.iter().all(|l| screens::region_same(&frame, &l.rect, &l.sig));
                                let read = if tabs_same { reread(&frame) } else { full_read(&frame) };
                                match read {
                                    Some(l) => found = Some(l),
                                    // mid-transition (tabs sliding): try again in half a second
                                    None => screen_checked = Instant::now() - Duration::from_millis(2500),
                                }
                            }
                        }
                        // a page no script has any more (the script changed)
                        Some(_) => hide_screen(&*ctx.sink, &ctx.live, &latest, &mut screen),
                    }
                } else if screen.is_some() {
                    hide_screen(&*ctx.sink, &ctx.live, &latest, &mut screen);
                }
                // A script's page went: tell it (`ended`), so it can forget what it kept about it.
                {
                    let now_id = screen.as_ref().map(|s| s.id.clone()).or_else(|| found.as_ref().map(|l| l.id.clone()));
                    if ended_check != now_id {
                        if let (Some(p), Some(gone)) = (&pages, &ended_check) {
                            p.ended(gone, &frame);
                        }
                        ended_check = now_id;
                    }
                }
                if let Some(layout) = found.as_mut().filter(|l| !l.menu.is_empty()) {
                    // The page's text boxes: the layout's words, then this session's translations, then the
                    // translation memory; anything else is sent to the translator and shows on a later read.
                    // Other players' names (a friends list): the script marks them private — never
                    // translated, sent or kept.
                    *ctx.live.screen_private.lock().unwrap() = layout.menu.iter().filter(|m| m.private).map(|m| [m.rect.x, m.rect.y, m.rect.w, m.rect.h]).collect();
                    layout.menu.retain(|m| !m.private);
                    for m in layout.menu.iter_mut() {
                        if m.fixed {
                            if m.ar.is_empty() {
                                m.ar = m.en.clone();
                            }
                            continue;
                        }
                        // (OCR may spell a line slightly differently from one read to the next; looked at only
                        // when the layout's words don't have it)
                        let cached = || {
                            let cache = ctx.live.menu_cache.lock().unwrap();
                            cache.map.get(&m.en).cloned().or_else(|| cache.map.iter().find(|(k, _)| ocr::similar(k, &m.en)).map(|(_, v)| v.clone()))
                        };
                        let remembered = || {
                            ctx.memory
                                .lock()
                                .unwrap()
                                .get(&m.en, &[], &[])
                                .map(|(segs, _)| segs.iter().map(|s| s.0.as_str()).collect::<String>())
                                .filter(|ar| translate::keeps_terms(&m.en, ar))
                        };
                        // a list item is one of the menu's known words, even when OCR garbled it (faint, greyed-out items)
                        // (also headings and short panel lines: the faint "Join Friends" title)
                        if (m.item || m.heading || m.en.split_whitespace().count() <= 3) && !m.en.trim().starts_with('<') {
                            if let Some((en, ar)) = screens::menu_item_word(&m.en) {
                                m.en = en.to_string();
                                m.ar = ar.to_string();
                                continue;
                            }
                        }
                        let built_in = screens::menu_word(&m.en).map(String::from);
                        match built_in.or_else(cached).or_else(remembered) {
                            // a switcher "< Michael >" keeps its arrows
                            Some(ar) if m.en.trim().starts_with('<') && m.en.trim().ends_with('>') => m.ar = format!("< {ar} >"),
                            Some(ar) => m.ar = ar,
                            None => {
                                if !menu_asked.contains(&m.en) && !menu_asked.iter().any(|a| ocr::similar(a, &m.en)) && menu_asked.insert(m.en.clone()) {
                                    seq_counter += 1;
                                    let _ = jobs.send(Job { area: MENU_TEXT.into(), seq: seq_counter, en: m.en.clone(), phrases: Vec::new(), icons: Vec::new(), line_h: 0.0, en_start: Instant::now(), text_box: None, game_box: None, parts: Vec::new() });
                                }
                            }
                        }
                    }
                    // Stability from one read to the next, on the same tab: a line read a bit differently
                    // keeps its Arabic.
                    if let Some(old) = screen.as_ref().filter(|o| o.id == layout.id && screens::menu_rects(o).is_some() && screens::selected_tab_name(o) == screens::selected_tab_name(layout)) {
                        // (only OCR's jitter of a few pixels: a line that moved — a list scrolled, read once
                        // mid-scroll — takes its new place, or its Arabic would stay off by part of a row)
                        let near = |a: &screens::Rect, b: &screens::Rect| (a.y - b.y).abs() < a.h.min(b.h).max(0.001) * 0.25 && (a.x - b.x).abs() < 0.01;
                        // The same line found again (same place, similar words): keep its box, colours and
                        // Arabic exactly, so nothing shifts or changes wording between reads.
                        for m in layout.menu.iter_mut() {
                            // (the selected row's box is the game's white bar, measured exactly: it takes this read's
                            // place, or a bar that moved a few pixels — the selection arriving, a list settling after a
                            // scroll — would show the old box's edge beside it)
                            let selected = m.item && m.bg.eq_ignore_ascii_case("#ffffff");
                            // the picture there hasn't changed: it's the same text, whatever OCR made of it this time
                            // (unless this read found more of the game's icons for it: a first read while the page was
                            // still appearing can miss them, and would keep the line without them)
                            // (and the words largely the same: two paragraphs in the same place on the same background
                            // can look alike to that small picture, and the last topic's would stay)
                            if let Some(o) = old.menu.iter().find(|o| near(&o.rect, &m.rect) && !o.ar.is_empty() && o.icons.len() >= m.icons.len() && screens::same_words(&o.en, &m.en) && screens::region_same(&frame, &o.rect, &o.sig)) {
                                let rect = m.rect.clone();
                                *m = o.clone();
                                if selected {
                                    m.rect = rect;
                                }
                                continue;
                            }
                            if let Some(o) = old.menu.iter().find(|o| near(&o.rect, &m.rect) && ocr::similar(&o.en, &m.en)) {
                                // same place and words as before; colours and picture from this read (the
                                // selection's white highlight may have moved onto it or off it)
                                // (its Arabic and the icons it places go together; a read with more icons wins)
                                let (ar, icons) = if o.ar.is_empty() || m.icons.len() > o.icons.len() { (m.ar.clone(), m.icons.clone()) } else { (o.ar.clone(), o.icons.clone()) };
                                let rect = if selected || o.bg != m.bg { m.rect.clone() } else { o.rect.clone() };
                                *m = screens::MenuText { ar, icons, rect, right: o.right, ..m.clone() };
                            }
                        }
                    }
                    // Remember this state of the tab (and add lines an earlier read of it found but
                    // this one missed, if they're still on screen). Only a layout that matches the
                    // picture on screen — not one just trimmed while the page changes under it.
                    if let Some((content_rect, _)) = screens::menu_rects(layout).filter(|(r, _)| !screens::content_changed_in(&frame, r, &layout.content_sig)) {
                        let tab = format!("{}/{}", layout.id, screens::selected_tab_name(layout).unwrap_or(""));
                        let near = |a: &screens::Rect, b: &screens::Rect| (a.y - b.y).abs() < a.h.min(b.h).max(0.001) * 0.6 && (a.x - b.x).abs() < 0.03;
                        if let Some(e) = menu_states.iter_mut().find(|(t, sig, _)| *t == tab && !screens::content_changed_in(&frame, &content_rect, sig)) {
                            for o in e.2.iter() {
                                if !o.ar.is_empty() && !layout.menu.iter().any(|m| near(&o.rect, &m.rect)) && screens::region_same(&frame, &o.rect, &o.sig) {
                                    layout.menu.push(o.clone());
                                }
                            }
                            (e.1, e.2) = (layout.content_sig.clone(), layout.menu.clone());
                        } else {
                            menu_states.push((tab, layout.content_sig.clone(), layout.menu.clone()));
                            if menu_states.len() > 16 {
                                menu_states.remove(0);
                            }
                        }
                    }
                }
                // The page's fixed words (title, tabs, buttons, the name block): the ones the script or the
                // layout's words didn't translate come from the translator, like the text boxes, and show
                // once translated.
                if let Some(layout) = found.as_mut() {
                    // (the lock, not a copy of the cache: the translator only waits a moment to add to it)
                    let cache = ctx.live.menu_cache.lock().unwrap();
                    let mut ask = |en: &str| {
                        if menu_asked.insert(en.to_string()) {
                            seq_counter += 1;
                            let _ = jobs.send(Job { area: MENU_TEXT.into(), seq: seq_counter, en: en.to_string(), phrases: Vec::new(), icons: Vec::new(), line_h: 0.0, en_start: Instant::now(), text_box: None, game_box: None, parts: Vec::new() });
                        }
                    };
                    // (the translation memory, with VI Translator's ready translations, before the translator: a
                    // line known there shows on this read)
                    let memory = ctx.memory.clone();
                    let known = move |en: &str| -> Option<String> {
                        memory
                            .lock()
                            .unwrap()
                            .get(en, &[], &[])
                            .map(|(segs, _)| segs.iter().map(|s| s.0.as_str()).collect::<String>())
                            .filter(|ar| !ar.trim().is_empty() && translate::keeps_terms(en, ar))
                    };
                    screen_missing = localize_screen(layout, &cache.map, &known, &mut ask);
                    drop(cache);
                } else if found.is_some() {
                    screen_missing = false;
                }
                if let Some(mut layout) = found {
                    // the reason is always drawn in the same size
                    // (measured only on a new page / new reason: refreshes reuse the frozen geometry)
                    let new_reason = screen.as_ref().is_none_or(|s| s.reason_en != layout.reason_en);
                    if new_reason && screens::menu_rects(&layout).is_none() {
                        layout.reason_line_h = sizes.size(&format!("__reason_{}@{}", layout.id, pages.as_ref().map_or(0, |p| p.version)), layout.reason_line_h);
                    }
                    // ...and so is the title (kept centred where the game's title is)
                    let th = if screen.is_none() && screens::menu_rects(&layout).is_none() && layout.title.h > 0.0 {
                        sizes.size(&format!("__title_{}@{}", layout.id, pages.as_ref().map_or(0, |p| p.version)), layout.title.h)
                    } else {
                        0.0
                    };
                    if th > 0.0 {
                        layout.title.y += (layout.title.h - th) / 2.0;
                        layout.title.h = th;
                    }
                    if screen.is_none() {
                        screen_checked = Instant::now();
                        title_missing_since = None;
                        // The page replaces normal subtitles: clear what the areas were showing, right away
                        // (each line on screen ends now: logged, and its translation dropped if it's still
                        // on its way — see PMsg::ClearAll).
                        let now = Instant::now();
                        for (id, st) in states.iter_mut() {
                            if st.current_seq != 0 {
                                let _ = present.send(PMsg::Ended { area: id.clone(), seq: st.current_seq, at: now });
                            }
                            st.current_start = None;
                            st.current.clear();
                            st.current_lines.clear();
                            st.candidate.clear();
                            st.candidate_lines.clear();
                            st.current_seq = 0;
                            seq_counter += 1;
                            latest.lock().unwrap().insert(id.clone(), seq_counter);
                        }
                        let _ = present.send(PMsg::ClearAll { upto: seq_counter });
                    }
                    if screen.as_ref() != Some(&layout) {
                        let reason_changed = screen.as_ref().map(|s| s.reason_en != layout.reason_en).unwrap_or(true);
                        if reason_changed {
                            *ctx.live.screen_reason.lock().unwrap() = serde_json::json!([]);
                            if !layout.reason_en.is_empty() {
                                seq_counter += 1;
                                latest.lock().unwrap().insert(SCREEN_REASON.into(), seq_counter);
                                let _ = jobs.send(Job { area: SCREEN_REASON.into(), seq: seq_counter, en: layout.reason_en.clone(), phrases: Vec::new(), icons: Vec::new(), line_h: 0.0, en_start: Instant::now(), text_box: None, game_box: None, parts: Vec::new() });
                            }
                        }
                        // (dev builds: which page, read off what — a page taken for one wrongly is found by this)
                        #[cfg(debug_assertions)]
                        {
                            let v = serde_json::to_value(&layout).unwrap_or_default();
                            let was = screen.as_ref().and_then(|s| serde_json::to_value(s).ok()).and_then(|s| s.get("id").cloned());
                            if was.as_ref() != v.get("id") {
                                let ens = |k: &str| v.get(k).and_then(|a| a.as_array()).map(|a| a.iter().filter_map(|x| x.get("en").and_then(|e| e.as_str())).take(6).collect::<Vec<_>>().join(" | ")).unwrap_or_default();
                                eprintln!(
                                    "[page] detected {} | title {:?} | labels {} | menu {} | buttons {}",
                                    v.get("id").and_then(|x| x.as_str()).unwrap_or("?"),
                                    v.get("title_en").and_then(|x| x.as_str()).unwrap_or(""),
                                    ens("labels"),
                                    ens("menu"),
                                    ens("buttons")
                                );
                            }
                        }
                        *ctx.live.screen.lock().unwrap() = serde_json::to_value(&layout).ok();
                        ctx.sink.emit("screen", serde_json::to_value(Some(&layout)).unwrap_or_default());
                        ctx.live.screen_active.store(true, std::sync::atomic::Ordering::Relaxed);
                        screen = Some(layout);
                    }
                }
                if screen.is_some() {
                    // The pause menu is checked on every new frame (its checks are tiny), so its Arabic
                    // goes the moment the English does: pace by the capture rate, not the read interval.
                    let pause = screen.is_some();
                    let pace = if pause { (1000 / c.capture_fps.clamp(1, 60) as u64).min(interval_ms) } else { interval_ms };
                    let spent = t0.elapsed();
                    // (normal areas pause while the page is up)
                    done!(Duration::from_millis(pace.max(30)).saturating_sub(spent));
                }
            } else if screen.is_some() {
                // (no whole frame to look for pages in)
                hide_screen(&*ctx.sink, &ctx.live, &latest, &mut screen);
            }

            // GTA VI's phone in the hand (WhatUp, Snapmatic): its text beside it (see phone.rs). Each piece
            // from the memory, or the translator (like a page's menu text: asked once, in the background,
            // shown when it comes); the contacts' names written as the speakers' are.
            if let (Some(f), Some(ocr)) = (frame, ocr_now) {
                let found = if c.phone_text() {
                    crate::phone::find_screen(&f.rgba, f.w, f.h)
                        // (a short light part, a Snapmatic post's caption: a phone only with its buttons up)
                        .or_else(|| crate::phone::find_screen_unsure(&f.rgba, f.w, f.h).filter(|_| crate::phone::phone_buttons(&crate::phone::read_buttons(ocr, &f.rgba, f.w, f.h))))
                        // (where the screen is, steady: a frame finding less of it is the last place — the reading
                        // turned upright round the same middle, the rows in the same places)
                        .map(|scr| {
                            let mut last = PHONE_SCREEN.lock().unwrap();
                            let prev = last.map(|(r, _)| r);
                            let scr = crate::phone::steady_screen(&mut last, scr, Instant::now());
                            // (the phone somewhere else now — turned to, walked with: its rows as read there, not
                            // the ones remembered from the last place, which came back as doubles)
                            if prev.is_some_and(|p| p.0.abs_diff(scr.0) > scr.2 / 5 || p.1.abs_diff(scr.1) > scr.3 / 5) {
                                ctx.live.phone_pieces.lock().unwrap().clear();
                            }
                            scr
                        })
                        .map(|scr| (scr, crate::phone::read_screen_any(ocr, &f.rgba, f.w, f.h, scr)))
                        .filter(|(_, r)| !r.pieces.is_empty())
                        // (a chat only — its header, the contact's name at the screen's top, read now or a moment ago
                        // with no list's rows since: the chat list, the phone's other apps and a light doorway taken
                        // for a screen get no second phone)
                        .filter(|(_, r)| {
                            let mut at = PHONE_CHAT_AT.lock().unwrap();
                            let now = Instant::now();
                            if r.pieces.iter().any(|p| p.avatar.is_some() && !p.title) {
                                *at = None;
                                return false;
                            }
                            if r.pieces.iter().any(|p| p.title) {
                                *at = Some(now);
                                return true;
                            }
                            at.is_some_and(|t| now.saturating_duration_since(t) < Duration::from_millis(3000))
                        })
                } else {
                    None
                };
                let now = Instant::now();
                match found {
                    Some((scr, read)) => {
                        *ctx.live.phone_seen.lock().unwrap() = Some(now);
                        // (a phone held tilted: read upright — its pieces' places and pictures are in that frame,
                        // `fr`; its turn, and the pictures' column's slant in it, go to the overlay, drawn as turned)
                        let fr: &[u8] = read.upright.as_deref().unwrap_or(&f.rgba[..]);
                        let (turn, slant) = (read.angle, crate::phone::tilt_of(&read.pieces).unwrap_or(0.0));
                        let screen_own = read.screen;
                        // (the screen's corners as it is in the frame, and where each piece is on it: the overlay's
                        // second phone drawn through the same corners, each piece where it is on the screen)
                        // (the screen's corners not found this frame — the picture moving, a scroll's blur: the last ones
                        // found, a moment old at most; without them the overlay fell back to another layout and jumped)
                        let (quad, quad_in) = {
                            let mut last = PHONE_QUAD.lock().unwrap();
                            match (read.quad, read.quad_in_pieces) {
                                (Some(q), Some(qi)) => {
                                    *last = Some((q, qi, now));
                                    (Some(q), Some(qi))
                                }
                                _ => match *last {
                                    Some((q, qi, at)) if now.saturating_duration_since(at) < Duration::from_millis(1500) => (Some(q), Some(qi)),
                                    _ => (read.quad, read.quad_in_pieces),
                                },
                            }
                        };
                        let photos: Vec<[f64; 4]> = read.photos.iter().filter_map(|r| quad_in.and_then(|q| crate::phone::on_screen(q, *r))).collect();
                        // (an emoji sent on its own: where it is on the screen, and its picture as it is)
                        let stickers: Vec<serde_json::Value> = read
                            .stickers
                            .iter()
                            .filter_map(|r| {
                                let uv = quad_in.and_then(|q| crate::phone::on_screen(q, *r))?;
                                let png = crate::phone::part_png(read.upright.as_deref().unwrap_or(&f.rgba[..]), f.w, f.h, *r, (0, 0, r.2, r.3), 48)?;
                                use base64::Engine as _;
                                Some(serde_json::json!({ "uv": uv, "png": format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(png)) }))
                            })
                            .collect();
                        let uv = |r: (u32, u32, u32, u32)| quad_in.and_then(|q| crate::phone::on_screen(q, r));
                        // (steady from one read to the next: a misreading keeps the first reading, a missed piece stays a moment)
                        let pieces = crate::phone::steady(&mut ctx.live.phone_pieces.lock().unwrap(), read.pieces, scr, now);
                        let (w, h) = (f.w as f64, f.h as f64);
                        let frac = |r: (u32, u32, u32, u32)| [r.0 as f64 / w, r.1 as f64 / h, r.2 as f64 / w, r.3 as f64 / h];
                        let lang = c.target_lang.clone();
                        let mut out = Vec::new();
                        let read_btns = crate::phone::read_buttons(ocr, &f.rgba, f.w, f.h);
                        let mut btns = Vec::new();
                        {
                            let m = ctx.memory.lock().unwrap();
                            let cache = ctx.live.menu_cache.lock().unwrap();
                            // the buttons' words at the bottom right ("SELECT", "BACK"): the layout's words for
                            // them, else the memory's, else the translator's (asked once)
                            for (r, en) in &read_btns {
                                let title: String = en.split(' ').map(|w| { let mut c = w.chars(); c.next().map(|f| f.to_string() + &c.as_str().to_lowercase()).unwrap_or_default() }).collect::<Vec<_>>().join(" ");
                                let ar = screens::button_word(&title)
                                    .map(str::to_string)
                                    .or_else(|| m.get(&title, &[], &[]).map(|(segs, _)| segs.iter().map(|s| s.0.as_str()).collect::<String>().trim().to_string()))
                                    .or_else(|| cache.map.get(&title).cloned());
                                match ar {
                                    // (a button's word plain: "تراجع!" is "تراجع")
                                    Some(ar) => {
                                        // (its icon after it, as the game's: "SEND ✕")
                                        let icon = crate::phone::button_icon(&f.rgba, f.w, f.h, *r).and_then(|ic| crate::phone::part_png(&f.rgba, f.w, f.h, ic, (0, 0, ic.2, ic.3), 40).map(|png| (ic, png)));
                                        btns.push(serde_json::json!({
                                            "rect": frac(*r), "en": en, "ar": ar.trim_end_matches(['!', '.', '!']).trim(),
                                            "icon": icon.as_ref().map(|(_, png)| {
                                                use base64::Engine as _;
                                                format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(png))
                                            }),
                                            "icon_rect": icon.map(|(ic, _)| frac(ic)),
                                        }))
                                    }
                                    None => {
                                        if menu_asked.insert(title.clone()) {
                                            seq_counter += 1;
                                            let _ = jobs.send(Job { area: MENU_TEXT.into(), seq: seq_counter, en: title, phrases: Vec::new(), icons: Vec::new(), line_h: 0.0, en_start: now, text_box: None, game_box: None, parts: Vec::new() });
                                        }
                                    }
                                }
                            }
                            for p in &pieces {
                                if p.title {
                                    let fg = p.look.fg;
                                    let bg = p.look.bg;
                                    // (the chat's header: its contact's picture at its end)
                                    let pic = p.avatar.and_then(|a| crate::phone::part_png(fr, f.w, f.h, p.rect, (a.0, a.1, a.2, a.2), 56)).map(|png| {
                                        use base64::Engine as _;
                                        format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(png))
                                    });
                                    out.push(serde_json::json!({ "rect": frac(p.rect), "uv": uv(p.rect), "box_uv": p.bubble.and_then(uv), "title": true, "ar": speaker::name_in(&lang, &p.en, None), "fg": format!("#{:02x}{:02x}{:02x}", fg[0], fg[1], fg[2]), "bg": format!("#{:02x}{:02x}{:02x}", bg[0], bg[1], bg[2]), "avatar": pic }));
                                    continue;
                                }
                                // (its emojis as slots after its words, as the game's lines have them: "… babe ⟦1⟧")
                                let with_emoji = if p.emoji.is_empty() { p.en.clone() } else { format!("{} {}", p.en, (1..=p.emoji.len()).map(ocr::icon_token).collect::<Vec<_>>().join(" ")) };
                                let exact = m.get(&with_emoji, &crate::memory::slots(p.emoji.len()), &[]).map(|(segs, src)| (crate::memory::slot_text(&segs), src));
                                // (a chat list's row shows only its message's start, cut to fit: "Hey Jason. You
                                // good? I saw": the whole line it starts, the row cutting it to one line again; a
                                // reviewed whole line before a machine translation of the cut start)
                                let whole = if p.avatar.is_some() && exact.as_ref().is_none_or(|(_, src)| src != "reviewed") { m.starting_with(&p.en) } else { None };
                                let reviewed_now = matches!(&whole, Some((_, _, src)) if src == "reviewed") || matches!(&exact, Some((_, src)) if src == "reviewed");
                                let ar = match (exact, whole) {
                                    (_, Some((_, ar, src))) if src == "reviewed" => Some(ar),
                                    (Some((ar, _)), _) => Some(ar),
                                    (None, Some((_, ar, _))) => Some(ar),
                                    (None, None) => None,
                                };
                                // (a contact's row keeps the reviewed translation it was given while its message reads much
                                // the same: a reading a letter off missed the reviewed line, and the row flipped to the
                                // translator's own words and back several times a second)
                                let row_key = p.name.clone().filter(|_| p.avatar.is_some());
                                let ar = {
                                    let mut guard = PHONE_ROW_AR.lock().unwrap();
                                    let rows = guard.get_or_insert_with(HashMap::new);
                                    match (&row_key, &ar) {
                                        (Some(k), Some(a)) if reviewed_now => {
                                            rows.insert(k.clone(), (p.en.clone(), a.clone()));
                                            Some(a.clone())
                                        }
                                        // (the first translation it got, whatever its source, held too: a machine translation of
                                        // a reading a word different came out differently, and the row flipped between the two)
                                        (Some(k), Some(a)) if !rows.contains_key(k) => {
                                            rows.insert(k.clone(), (p.en.clone(), a.clone()));
                                            Some(a.clone())
                                        }
                                        // (a clearly cleaner reading of it — more of its words real words: "Hey this is Joey from
                                        // Coa..." for "Hev this is Jov from Co" — its translation in place of a misreading's)
                                        (Some(k), Some(a)) if rows.get(k).is_some_and(|(en, _)| {
                                            let real = |t: &str| t.split_whitespace().filter(|w| w.chars().filter(|c| c.is_alphabetic()).count() >= 3).count();
                                            real(&p.en) > real(en) + 1
                                        }) => {
                                            rows.insert(k.clone(), (p.en.clone(), a.clone()));
                                            Some(a.clone())
                                        }
                                        (Some(k), _) => match rows.get(k) {
                                            Some((en, a)) if ocr::similar(en, &p.en) || {
                                                // (most of its words the same: "hon my rum is be..." for "thnx hon my rum is be...")
                                                let ws = |t: &str| t.to_lowercase().split_whitespace().map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_string()).filter(|w| w.len() > 1).collect::<std::collections::HashSet<_>>();
                                                let (a1, b1) = (ws(en), ws(&p.en));
                                                !b1.is_empty() && a1.intersection(&b1).count() * 3 >= b1.len().max(a1.len()) * 2
                                            } => Some(a.clone()),
                                            _ => ar,
                                        },
                                        _ => ar,
                                    }
                                };
                                // (a translation with no words in it for words: the translator gave up on a cut
                                // line, "thnx hon 😘 my rum is be..." came back as a row of dots: none, not that;
                                // asked again without the cut word at its end, "thnx hon my rum is", shown with "…")
                                let has_words = |a: &str| !p.en.chars().any(char::is_alphabetic) || a.replace(|c: char| c == '{' || c == '}' || c.is_ascii_digit(), "").chars().any(char::is_alphabetic);
                                let cut_en = {
                                    let t = p.en.trim_end_matches(['.', '…', ' ']);
                                    if t.len() < p.en.len() { t.rsplit_once(' ').map(|(a, _)| a.to_string()).unwrap_or_else(|| t.to_string()) } else { t.to_string() }
                                };
                                let gave_up = cache.map.get(&p.en).is_some_and(|a| !has_words(a));
                                let ar = ar
                                    .or_else(|| cache.map.get(&p.en).cloned())
                                    .or_else(|| cache.map.iter().find(|(k, _)| ocr::similar(k, &p.en)).map(|(_, v)| v.clone()))
                                    .filter(|a| has_words(a))
                                    .or_else(|| cache.map.get(&cut_en).filter(|a| has_words(a)).map(|a| format!("{}…", a.trim_end_matches(['.', '…', ' ']))));
                                let hex = |c: [u8; 3]| format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2]);
                                // (a translation without its slots: the emojis after it)
                                let ar = ar.map(|a| if p.emoji.is_empty() || a.contains('{') { a } else { format!("{a} {}", (1..=p.emoji.len()).map(|i| format!("{{{i}}}")).collect::<String>()) });
                                // (a chat list's row shows while its message's translation comes: its contact, its picture)
                                let ar = if p.avatar.is_some() && p.en.is_empty() { Some(String::new()) } else { ar };
                                let png = |o: (i32, i32, u32, u32), h: u32| {
                                    crate::phone::part_png(fr, f.w, f.h, p.rect, o, h).map(|png| {
                                        use base64::Engine as _;
                                        format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(png))
                                    })
                                };
                                if ar.is_none() && p.avatar.is_some() {
                                    out.push(serde_json::json!({
                                        "rect": frac(p.rect), "uv": uv(p.rect), "box_uv": p.bubble.and_then(uv), "ar": "", "name": p.name.as_ref().map(|n| speaker::name_in(&lang, n, None)),
                                        "bg": hex(p.look.bg), "fg": hex(p.look.fg), "edge": p.look.edge.map(hex),
                                        "avatar": p.avatar.and_then(|a| png((a.0, a.1, a.2, a.2), 56)),
                                        "badge": p.badge.and_then(|b| png(b, 28)),
                                    }));
                                }
                                // (a chat's message not translated yet: its English meanwhile, marked pending — it's in its place
                                // and size from the first reading, and only its words change when its translation comes; before,
                                // it appeared only then, and the chat's layout moved round it)
                                let ask_needed = ar.is_none();
                                let (ar, pending) = match ar {
                                    Some(a) => (Some(a), false),
                                    None if p.avatar.is_none() && !p.title && !p.en.trim().is_empty() => (Some(p.en.clone()), true),
                                    None => (None, false),
                                };
                                if ask_needed {
                                    let ask = if gave_up { cut_en.clone() } else { p.en.clone() };
                                    if !ask.is_empty() && !menu_asked.contains(&ask) && !menu_asked.iter().any(|a| ocr::similar(a, &ask)) && menu_asked.insert(ask.clone()) {
                                        seq_counter += 1;
                                        let _ = jobs.send(Job { area: MENU_TEXT.into(), seq: seq_counter, en: ask, phrases: Vec::new(), icons: Vec::new(), line_h: 0.0, en_start: now, text_box: None, game_box: None, parts: Vec::new() });
                                    }
                                }
                                match ar {
                                    Some(ar) => out.push(serde_json::json!({
                                        "rect": frac(p.rect), "uv": uv(p.rect), "box_uv": p.bubble.and_then(uv),
                                        "pending": pending,
                                        "ar": ar,
                                        "name": p.name.as_ref().map(|n| speaker::name_in(&lang, n, None)),
                                        "bg": hex(p.look.bg),
                                        "fg": hex(p.look.fg),
                                        "label": p.look.label,
                                        "end": p.look.end,
                                        "edge": p.look.edge.map(hex),
                                        "boxed": p.look.boxed,
                                        // (a chat list's row: its contact's picture, as on the phone)
                                        "avatar": p.avatar.and_then(|a| png((a.0, a.1, a.2, a.2), 56)),
                                        "preview": p.preview.and_then(|b| png(b, 28)),
                                        "badge": p.badge.and_then(|b| png(b, 28)),
                                        "emoji": p.emoji.iter().filter_map(|e| crate::phone::part_png(fr, f.w, f.h, *e, (0, 0, e.2, e.3), 40)).map(|png| {
                                            use base64::Engine as _;
                                            format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(png))
                                        }).collect::<Vec<_>>(),
                                    })),
                                    None => {}
                                }
                            }
                        }
                        let sbg = crate::phone::screen_bg(fr, f.w, f.h, scr, scr.1, scr.1 + scr.3);
                        let v = serde_json::json!({ "screen": frac(scr), "screen_bg": format!("#{:02x}{:02x}{:02x}", sbg[0], sbg[1], sbg[2]), "pieces": out, "list": pieces.iter().any(|p| p.avatar.is_some() && !p.title), "buttons": btns, "turn": turn, "slant": slant, "screen_own": screen_own.map(frac), "quad": quad.map(|q| q.map(|(x, y)| [x / w, y / h])), "photos": photos, "stickers": stickers });
                        let mut up = ctx.live.phone.lock().unwrap();
                        if up.as_ref() != Some(&v) {
                            *up = Some(v.clone());
                            ctx.live.phone_active.store(true, std::sync::atomic::Ordering::Relaxed);
                            ctx.sink.emit("phone", v);
                        }
                    }
                    None => {
                        // GTA VI's message bubble at the bottom right (see chat.rs): its translation in a
                        // bubble like it, above it. Read again only when its box changes.
                        let chat = if c.phone_bubble() { crate::chat::find_bubble(&f.rgba, f.w, f.h) } else { None };
                        let bubble = chat.and_then(|r| {
                            let frac_r = screens::Rect { x: r.0 as f64 / f.w as f64, y: r.1 as f64 / f.h as f64, w: r.2 as f64 / f.w as f64, h: r.3 as f64 / f.h as f64 };
                            let sig = screens::region_sig(f, &frac_r);
                            let mut memo = ctx.live.chat_read.lock().unwrap();
                            let same = memo.as_ref().is_some_and(|(s, mr, _)| {
                                let near = mr.0.abs_diff(r.0) <= 12 && mr.1.abs_diff(r.1) <= 12 && mr.2.abs_diff(r.2) <= 12 && mr.3.abs_diff(r.3) <= 12;
                                near && s.iter().zip(&sig).map(|(a, b)| a.abs_diff(*b) as u32).sum::<u32>() / (sig.len().max(1) as u32) < 6
                            });
                            if !same {
                                *memo = Some((sig, r, crate::chat::read_bubble(ocr, &f.rgba, f.w, f.h, r)));
                            }
                            memo.as_ref().and_then(|m| m.2.clone()).map(|b| crate::chat::Bubble { rect: r, ..b })
                        });
                        let chat_v = bubble.and_then(|b| {
                            let lang = c.target_lang.clone();
                            let hex = |c: [u8; 3]| format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2]);
                            let m = ctx.memory.lock().unwrap();
                            let cache = ctx.live.menu_cache.lock().unwrap();
                            // (its emojis as slots "{1}" … in the Arabic, their pictures drawn there)
                            let exact = m.get(&b.en, &crate::memory::slots(b.emoji.len()), &[]).map(|(segs, src)| (crate::memory::slot_text(&segs), src));
                            // (the words alone, for the translator)
                            let plain = b.en.split_whitespace().filter(|w| !w.starts_with('⟦')).collect::<Vec<_>>().join(" ");
                            // (a message the game cut off: the whole line it's the start of, cut as the game cut
                            // it; a reviewed whole line before a machine translation of the cut start)
                            let whole = if b.cut() && exact.as_ref().is_none_or(|(_, src)| src != "reviewed") {
                                m.starting_with(&b.en).map(|(en, ar, src)| (crate::chat::cut_like(&ar, &b.en, &en), src))
                            } else {
                                None
                            };
                            let ar = match (exact, whole) {
                                (_, Some((ar, src))) if src == "reviewed" => Some(ar),
                                (Some((ar, _)), _) => Some(ar),
                                (None, Some((ar, _))) => Some(ar),
                                (None, None) => None,
                            }
                            .or_else(|| cache.map.get(&plain).cloned());
                            if ar.is_none() && menu_asked.insert(plain.clone()) {
                                seq_counter += 1;
                                let _ = jobs.send(Job { area: MENU_TEXT.into(), seq: seq_counter, en: plain.clone(), phrases: Vec::new(), icons: Vec::new(), line_h: 0.0, en_start: now, text_box: None, game_box: None, parts: Vec::new() });
                            }
                            // (a translation without its slots: the emojis after it, as they come)
                            let ar = ar.map(|a| {
                                if b.emoji.is_empty() || a.contains('{') {
                                    a
                                } else {
                                    format!("{a} {}", (1..=b.emoji.len()).map(|i| format!("{{{i}}}")).collect::<Vec<_>>().join(""))
                                }
                            });
                            let emoji: Vec<String> = b
                                .emoji
                                .iter()
                                .filter_map(|e| crate::phone::part_png(&f.rgba, f.w, f.h, *e, (0, 0, e.2, e.3), 40))
                                .map(|png| {
                                    use base64::Engine as _;
                                    format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(png))
                                })
                                .collect();
                            let avatar = b.avatar.and_then(|a| crate::phone::part_png(&f.rgba, f.w, f.h, a, (0, 0, a.2, a.3), 56)).map(|png| {
                                use base64::Engine as _;
                                format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(png))
                            });
                            let badge = b.badge.and_then(|a| crate::phone::part_png(&f.rgba, f.w, f.h, a, (0, 0, a.2, a.3), 28)).map(|png| {
                                use base64::Engine as _;
                                format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(png))
                            });
                            let (w, h) = (f.w as f64, f.h as f64);
                            ar.map(|ar| {
                                serde_json::json!({
                                    "rect": [b.rect.0 as f64 / w, b.rect.1 as f64 / h, b.rect.2 as f64 / w, b.rect.3 as f64 / h],
                                    "name": b.name.as_ref().map(|n| speaker::name_in(&lang, n, None)),
                                    "ar": ar,
                                    "bg": hex(b.bg), "fg": hex(b.fg), "name_fg": hex(b.name_fg),
                                    "avatar": avatar,
                                    "badge": badge,
                                    "emoji": emoji,
                                })
                            })
                        });
                        let mut up = ctx.live.phone.lock().unwrap();
                        match chat_v {
                            Some(cv) => {
                                *ctx.live.phone_seen.lock().unwrap() = Some(now);
                                let v = serde_json::json!({ "chat": cv });
                                if up.as_ref() != Some(&v) {
                                    *up = Some(v.clone());
                                    ctx.live.phone_active.store(true, std::sync::atomic::Ordering::Relaxed);
                                    ctx.sink.emit("phone", v);
                                }
                            }
                            None => {
                                // (put away: gone a moment after it was last seen, not on a frame it was missed)
                                let gone = ctx.live.phone_seen.lock().unwrap().is_none_or(|t| now.saturating_duration_since(t) > Duration::from_millis(300));
                                if gone && up.is_some() {
                                    *up = None;
                                    *ctx.live.chat_read.lock().unwrap() = None;
                                    ctx.live.phone_active.store(false, std::sync::atomic::Ordering::Relaxed);
                                    ctx.sink.emit("phone", serde_json::Value::Null);
                                }
                            }
                        }
                    }
                }
            }

            // an area turned off (or gone): its line ends now (else the presenter waits for it forever)
            states.retain(|id, st| {
                let keep = c.areas.iter().any(|a| a.enabled && &a.id == id);
                if !keep && st.current_seq != 0 {
                    let _ = present.send(PMsg::Ended { area: id.clone(), seq: st.current_seq, at: Instant::now() });
                }
                keep
            });
            for area_cfg in c.areas.iter().filter(|a| a.enabled) {
                // (an area whose lines stack up is read up to STACK_H above it too: see bottom_stack)
                let stacks = c.fx(&area_cfg.id).stacks(&area_cfg.id);
                // (when the English counts as gone: lines that stack up come and go quickly; an
                // instruction stays up and is read over every kind of ground, sunlit gravel missing it
                // for a moment now and then: gone after 350 ms at the least, not blinking)
                let clear_ms = clear_ms_for(stacks, clear_after_ms);
                // (read here, from the frame: grown; the host reads the area as it is)
                let own_read = matches!((&frame, &ocr_now), (Some(_), Some(_)));
                let grown;
                let area = if stacks && own_read {
                    grown = crate::config::stack_grown(area_cfg);
                    &grown
                } else {
                    area_cfg
                };
                let st = states.entry(area.id.clone()).or_default();
                let now = Instant::now();
                // (the host read the area itself, or its OCR reads it here)
                let (crop, words, sure) = match (frame, ocr_now) {
                    (Some(frame), Some(ocr)) if stacks => read_stacked(ocr, &frame.rgba, fw, fh, area_cfg, area),
                    (Some(frame), Some(ocr)) => {
                        let crop = Crop::from_frame(&frame.rgba, fw, fh, area.x, area.y, area.w, area.h);
                        let (words, sure) = match &crop {
                            Some(cr) => ocr.recognize_checked(cr).unwrap_or((Vec::new(), true)),
                            None => (Vec::new(), true),
                        };
                        (crop, words, sure)
                    }
                    _ => match reads.remove(&area.id) {
                        Some(r) => (Some(r.crop), r.words, r.sure),
                        None => (None, Vec::new(), true),
                    },
                };
                let (mut text, mut words) = (ocr::clean(&words), words);
                // Subtitles are centred in the picture: text off-centre (a menu's "Browse" at the bottom
                // right) is not a subtitle, and a panel at the side of the area (a garage's car stats) is
                // left out of the line instead of pushing it off-centre.
                if area.centered_only && !words.is_empty() {
                    let kept = ocr::centered_words(&words, area.x.clamp(0.0, 1.0) * fw as f64, fw as f64);
                    if kept.len() != words.len() {
                        words = kept;
                        text = ocr::clean(&words);
                    }
                }
                // Lines stacked up from the area's own line 1: only with a line there, and what's joined to it.
                // (always its words in reading order, top line first: the band read above line 1's came after
                // them, and the block's English came out of order — "…Oh! God… Tourist: Eyes on the road")
                let mut lines_now: Vec<String> = Vec::new();
                if let Some(cr) = crop.as_ref().filter(|_| stacks && !words.is_empty()) {
                    // (lines sliding up, line 1's place empty for a moment: what's up stays, for 400 ms at most)
                    let moving = ocr::stack_moving(&words, cr.h as f64);
                    let all = std::mem::take(&mut words);
                    words = ocr::bottom_stack(&all, cr.h as f64);
                    if words.is_empty() && moving && !st.current.is_empty() && st.last_seen.is_some_and(|t| now.saturating_duration_since(t) < Duration::from_millis(400)) {
                        continue;
                    }
                    text = ocr::clean(&words);
                    let n = words.iter().map(|w| w.line + 1).max().unwrap_or(0);
                    lines_now = (0..n).map(|l| ocr::clean(&words.iter().filter(|w| w.line == l).cloned().collect::<Vec<_>>())).collect();
                }
                // An area of the game's own box (GTA VI's help): no box around the text, no text here.
                // (only its words: not a shop's name above the box or a sign under it, read along with them)
                // (the game's box this frame, found or remembered: the translation goes right under it)
                let mut help_box = None;
                let anchor = crate::config::box_anchor(area).map(|[ax, ay, aw]| {
                    let (cx0, cy0) = ((area.x.clamp(0.0, 1.0) * fw as f64) as u32, (area.y.clamp(0.0, 1.0) * fh as f64) as u32);
                    (((ax * fw as f64).round() as u32).saturating_sub(cx0), ((ay * fh as f64).round() as u32).saturating_sub(cy0), (aw * fw as f64) as u32)
                });
                if let (true, Some(anchor)) = (area.needs_box && !text.is_empty(), anchor) {
                    // where the box always is: its text by its place and its lines, still from one read
                    // to the next (or with the box's edges seen around it)
                    match ocr::in_anchored_box(&words, anchor) {
                        Some((b, kept)) => {
                            let first = kept.first().map(|w| (w.rect.0, w.rect.1)).unwrap_or_default();
                            let still = st.anchored_at.is_some_and(|(x, y)| x.abs_diff(first.0) <= 3 && y.abs_diff(first.1) <= 3);
                            // the box's own edges, when they show: the translation takes exactly them
                            let edges = crop.as_ref().and_then(|cr| ocr::in_game_box(cr, &kept, None)).map(|(e, _, _)| e).filter(|e| e.0.abs_diff(b.0) <= 12 && e.1.abs_diff(b.1) <= 12);
                            st.anchored_at = Some(first);
                            if still || edges.is_some() {
                                words = kept;
                                text = ocr::clean(&words);
                                // (edges not seen this time: the ones seen for this instruction, if they were:
                                // the same text has the same box; else from its lines)
                                if let Some(e) = edges {
                                    st.game_box = Some((e, now));
                                    st.game_box_text = text.clone();
                                }
                                let same = st.game_box.filter(|_| ocr::similar(&text, &st.game_box_text)).map(|(e, _)| e);
                                help_box = Some(edges.or(same).unwrap_or(b));
                            } else {
                                text.clear();
                            }
                        }
                        None => {
                            st.anchored_at = None;
                            text.clear();
                        }
                    }
                    // (a sign in capitals, or a shop menu's item where the box would be: not sentences)
                    if ocr::mostly_capitals(&text) || !ocr::reads_as_sentences(&text) {
                        text.clear();
                    }
                } else if area.needs_box && !text.is_empty() {
                    let known = st.game_box.filter(|(_, at)| at.elapsed() < GAME_BOX_KEEP).map(|(b, _)| b);
                    match crop.as_ref().and_then(|cr| ocr::in_game_box(cr, &words, known)) {
                        // (its edges not found: only the same instruction going on, or one of two lines or
                        // more; not a word of the scene, or of a menu, where the box was a moment ago)
                        Some((_, kept, false))
                            if !ocr::similar(&ocr::clean(&kept), &st.game_box_text)
                                && kept.iter().map(|w| w.line).collect::<std::collections::HashSet<_>>().len() < 2 =>
                        {
                            text.clear()
                        }
                        Some((b, kept, seen)) => {
                            if seen {
                                st.game_box = Some((b, now));
                                st.game_box_text = ocr::clean(&kept);
                            }
                            help_box = Some(b);
                            if kept.len() != words.len() {
                                words = kept;
                                text = ocr::clean(&words);
                            }
                        }
                        None => text.clear(),
                    }
                    // (a sign or billboard with a frame around it: in capitals, not the game's sentences)
                    if ocr::mostly_capitals(&text) {
                        text.clear();
                    }
                }
                // Not English at all (another script, icons, a map read as letters): no text here.
                if !ocr::looks_like_words(&text) {
                    text.clear();
                }
                if !text.is_empty() {
                    st.last_seen = Some(now);
                    // The line on screen read again; or other text read in its place for as long as it takes
                    // the English to count as gone: it's gone (an instruction replaced by the next, which
                    // isn't translated yet: the old translation went on showing over it).
                    if same_text(&text, &lines_now, &st.current, &st.current_lines) {
                        st.current_seen = Some(now);
                    } else if !stacks && st.current_seq != 0 && !st.current_ended && st.current_seen.is_some_and(|t| now.saturating_duration_since(t) > Duration::from_millis(clear_ms)) {
                        let _ = present.send(PMsg::Ended { area: area.id.clone(), seq: st.current_seq, at: st.current_seen.unwrap_or(now) });
                        st.current_ended = true;
                    }
                    // (the same line, its pale coloured name faded out first or not read this time: the line
                    // on screen goes on, not a new one translated and shown again without its name)
                    if speaker::without_name(&st.candidate).is_some_and(|rest| ocr::similar(rest, &text)) {
                        text = st.candidate.clone();
                        lines_now = st.candidate_lines.clone();
                    }
                    let mut is_new = false;
                    if !same_text(&text, &lines_now, &st.candidate, &st.candidate_lines) {
                        // A new line: with the "over the game's text" look its black box goes up now,
                        // before the line has settled or been translated (Read Areas).
                        // (and the lines that stack up, for the translations that ride above them, at once)
                        let fx = c.fx(&area.id);
                        // (an area placed under the game's box keeps that place with this look: no black box)
                        let tell = if c.cover_look() { fx.cover_early && area.placement != "under_text" } else { stacks && fx.rides(&area.id) };
                        // (with the black box over the English: only when its translation is at hand, the
                        // memory knowing it, each of its lines: a line the translator takes half a second for
                        // left an empty black box up meanwhile; its English shows until its translation comes)
                        // (in an area with reviewed translations only — the instructions — only a line with a
                        // reviewed one, as read or with its button icons: a line the memory knew from the translator
                        // left an empty black box up, nothing ever coming to fill it)
                        let reviewed_only = c.reviewed_only(&area.id);
                        let iconed = if reviewed_only { crop.as_ref().map(|cr| ocr::text_with_icons(cr, &words).0).unwrap_or_default() } else { String::new() };
                        let at_hand = || {
                            let m = ctx.memory.lock().unwrap();
                            if reviewed_only {
                                let reviewed = |l: &str| m.get(l, &[], &[]).is_some_and(|(_, src)| src == "reviewed");
                                return reviewed(&text) || (!iconed.is_empty() && reviewed(&iconed)) || speaker::without_name(&text).is_some_and(|r| reviewed(r));
                            }
                            let knows = |l: &str| m.knows(l) || speaker::without_name(l).is_some_and(|rest| m.knows(rest)) || m.knows(&speaker::sentence_of(l));
                            knows(&text) || crop.as_ref().is_some_and(|cr| {
                                let parts = stack_parts(cr, &words);
                                parts.len() > 1 && parts.iter().all(|(p, _)| knows(p))
                            })
                        };
                        if tell && !same_text(&text, &lines_now, &st.current, &st.current_lines) && (!c.cover_look() || at_hand()) {
                            if let Some((cr, b)) = crop.as_ref().zip(ocr::text_with_icons_block(crop.as_ref(), &words)) {
                                let (cx0, cy0) = ((area.x.clamp(0.0, 1.0) * fw as f64) as u32, (area.y.clamp(0.0, 1.0) * fh as f64) as u32);
                                let to_frame = |(x, y, w, h): (u32, u32, u32, u32)| {
                                    let (fw, fh) = (fw as f64, fh as f64);
                                    [(cx0 + x) as f64 / fw, (cy0 + y) as f64 / fh, w as f64 / fw, h as f64 / fh]
                                };
                                let mut hs: Vec<u32> = words.iter().map(|w| w.rect.3).collect();
                                hs.sort_unstable();
                                let mh = hs.get(hs.len() / 2).copied().unwrap_or(20);
                                let game_box = if fx.cover_min { ocr::box_around(cr, b, mh).map(to_frame) } else { None };
                                let v = serde_json::json!({ "seq": 0, "text_box": to_frame(b), "game_box": game_box, "line_h": mh as f64 / fh as f64 });
                                set_cover(&*ctx.sink, &ctx.live, &area.id, Some(v));
                            }
                        }
                        st.candidate = text;
                        st.candidate_lines = lines_now.clone();
                        st.candidate_since = Some(now);
                        is_new = true;
                    }
                    // A line the translation memory already knows needn't wait to settle: show it on
                    // first sight. Other lines wait `stable_ms` so a half-faded frame isn't translated.
                    // (on any reading of it, not only a clean first one: over bright ground the junk readings
                    // in between kept restarting the wait, and "Drive to the prison" came a second late)
                    let known = (is_new || !same_text(&st.candidate, &st.candidate_lines, &st.current, &st.current_lines))
                        && crop.as_ref().is_some_and(|cr| {
                            let (t, icons, _) = ocr::text_with_icons(cr, &words);
                            let line = if icons.is_empty() || stacks { &st.candidate } else { &t };
                            let m = ctx.memory.lock().unwrap();
                            // (a speaker name in front is not part of what's remembered)
                            let knows = |l: &str| m.knows(l) || speaker::without_name(l).is_some_and(|rest| m.knows(rest)) || m.knows(&speaker::sentence_of(l));
                            // (stacked lines: each line known, the block new — a line came or went)
                            // (and as read without the icons: a small word the reader took for a button icon —
                            // "Get in 🎮 vehicle" for "Get in a vehicle" — missed the line the memory has)
                            knows(line) || knows(&st.candidate) || (stacks && {
                                let parts = stack_parts(cr, &words);
                                parts.len() > 1 && parts.iter().all(|(p, _)| knows(p))
                            })
                        });
                    // A reading the app isn't sure of (text still animating in: GTA VI's instructions
                    // fade in with a glow) waits three times as long: by then the real text has
                    // replaced it, or it really is what's on screen.
                    let wait = if sure { stable_ms } else { stable_ms * 3 };
                    // (an instruction on screen is replaced by a reading that holds for a quarter of a second
                    // at least, unless the memory knows it: a junk reading over bright ground — "Drive tofthe
                    // pr.iSoi7" — replaced "Drive to the prison" for a moment)
                    let wait = if !stacks && !st.current.is_empty() { wait.max(250) } else { wait };
                    let settled = !is_new && st.candidate_since.is_some_and(|s| now - s >= Duration::from_millis(wait));
                    // The same line, sent without colours, now shows them: sent again with them (the same
                    // line on screen, not a new one: its start and the lines log stay as they are).
                    let recolor = st.colors_until.is_some_and(|t| now < t)
                        && !st.current.is_empty()
                        && same_text(&st.candidate, &st.candidate_lines, &st.current, &st.current_lines)
                        && crop.as_ref().is_some_and(|cr| !ocr::colored_phrases(cr, &words).is_empty());
                    if st.colors_until.is_some_and(|t| now >= t) {
                        st.colors_until = None;
                    }
                    // (a reading of the line on screen the OCR got wrong — "en, Get in a vehicle", "Get ima
                    // vehicle:" — that the memory takes for the same line, the same translation: the same line,
                    // not sent again, its box moving each time)
                    if !stacks && !recolor && !st.current.is_empty() && (known || settled) && !same_text(&st.candidate, &st.candidate_lines, &st.current, &st.current_lines) {
                        let m = ctx.memory.lock().unwrap();
                        let ar = |l: &str| m.get(l, &[], &[]).map(|(segs, _)| segs.iter().map(|s| s.0.as_str()).collect::<String>().trim().to_string());
                        // (or a misreading the memory takes for another line, near the line up too: "Get ih vehicle"
                        // with "Get in a vehicle" up is a letter from "Get in vehicle", two from the line up — "اركب
                        // المركبة" for a moment over "اركب مركبة". Another line needs a clean reading of it to replace
                        // the line up; a misreading within two letters more of the line up stays on it. And once the
                        // line up hasn't been read for a while, the next goes up whatever its reading.)
                        let norm = |l: &str| l.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect::<String>();
                        let closer_to_up = !st.current_ended
                            && m.line_of(&st.candidate).is_some_and(|other| {
                                let (c, o) = (norm(&st.candidate), norm(&other));
                                c != o && edit_distance(&c, &norm(&st.current)) <= edit_distance(&c, &o) + 2
                            });
                        if closer_to_up || ar(&st.candidate).is_some_and(|a| Some(a) == ar(&st.current)) {
                            drop(m);
                            st.candidate = st.current.clone();
                            st.candidate_lines = st.current_lines.clone();
                            st.current_seen = Some(now);
                        }
                    }
                    // (only reviewed translations here: a reading matching no reviewed line — "Getin a vehigle",
                    // "Drive to the" — went to the translator, "أدخل في فيهيغل", "قُدْ إلى"; it isn't translated,
                    // what's up stays until its English counts as gone)
                    // (and as read with its button icons: "Hold L2 to interact with Andrew" read plain is not the
                    // reviewed "Hold ⟦1⟧ to interact with Andrew"; no instruction with a button was translated)
                    let reviewed_only = c.reviewed_only(&area.id);
                    let allowed = !reviewed_only || {
                        let iconed = crop.as_ref().map(|cr| ocr::text_with_icons(cr, &words).0).unwrap_or_default();
                        let m = ctx.memory.lock().unwrap();
                        let reviewed = |l: &str| m.get(l, &[], &[]).is_some_and(|(_, src)| src == "reviewed");
                        reviewed(&st.candidate) || (!iconed.is_empty() && reviewed(&iconed)) || speaker::without_name(&st.candidate).is_some_and(|r| reviewed(r))
                    };
                    // (a line not let through with its black box up — it went up for a reading that was: no empty box stays)
                    if !allowed && is_new && c.cover_look() {
                        set_cover(&*ctx.sink, &ctx.live, &area.id, None);
                    }
                    if allowed && (((known || settled) && !same_text(&st.candidate, &st.candidate_lines, &st.current, &st.current_lines)) || recolor) {
                        // Stable new text: translate it once. Colours are measured now (and again for a
                        // moment if there were none: see `colors_until`).
                        let prev_lines = std::mem::take(&mut st.current_lines);
                        st.current = st.candidate.clone();
                        st.current_seen = Some(now);
                        st.current_ended = false;
                        st.current_lines = st.candidate_lines.clone();
                        let mut phrases = crop.as_ref().map(|cr| ocr::colored_phrases(cr, &words)).unwrap_or_default();
                        // (a line's colours as seen the last time it was up: GTA VI fades its instructions in
                        // white, their pink word coming a moment later, and the line went up white, then pink)
                        {
                            // (by the line the memory takes it for: a misreading of it finds its colours too)
                            let key: String = ctx.memory.lock().unwrap().line_of(&st.current).unwrap_or_else(|| st.current.clone()).to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect();
                            let mut seen = LINE_COLORS.lock().unwrap();
                            if !phrases.is_empty() {
                                if seen.len() > 2000 {
                                    seen.clear();
                                }
                                seen.insert(key, phrases.clone());
                                AREA_COLORS.lock().unwrap().insert(area.id.clone(), phrases.iter().map(|p| p.1.clone()).collect());
                            } else if let Some(p) = seen.get(&key) {
                                phrases = p.clone();
                            }
                        }
                        st.colors_until = if phrases.is_empty() && !recolor { Some(now + Duration::from_millis(2000)) } else { None };
                        // (a line never seen in colour yet, its reviewed translation marking the highlighted words —
                        // from the game's own highlight tags: those words in the colours this area's were last
                        // seen in, else GTA VI's pink, from its first frame; its own colours, when they show, follow.
                        // Only where the translation is the reviewed one: the colours carry no English words)
                        if phrases.is_empty() && c.reviewed_only(&area.id) {
                            let colors = AREA_COLORS.lock().unwrap().get(&area.id).cloned().unwrap_or_else(|| vec![GTA6_HIGHLIGHT.to_string()]);
                            phrases = (0..4).map(|i| (String::new(), colors.get(i).or(colors.last()).cloned().unwrap_or_default())).collect();
                        }
                        // Controller button icons inside the text ("Press ✕ to ...") come along as placeholders.
                        let (en_icons, icons_found, block) =
                            crop.as_ref().map(|cr| ocr::text_with_icons(cr, &words)).unwrap_or((String::new(), Vec::new(), None));
                        // (stacked subtitle lines have no button icons: one "found" beside them, a dark mark
                        // or the start of a pale name, made the lines one instruction, translated whole)
                        // (and none when the line as read without them is one the memory has: the "icon" was a word)
                        // (where only reviewed lines go, only a reviewed one as read plain: the translator's
                        // "Hold L2 to …" took the place of the reviewed "Hold ⟦1⟧ to …")
                        let plain_known = !icons_found.is_empty() && {
                            let m = ctx.memory.lock().unwrap();
                            let has = |l: &str| if reviewed_only { m.get(l, &[], &[]).is_some_and(|(_, src)| src == "reviewed") } else { m.knows(l) };
                            has(&st.current) || speaker::without_name(&st.current).is_some_and(|r| has(r))
                        };
                        let (en, icons) = if icons_found.is_empty() || stacks || plain_known { (st.current.clone(), Vec::new()) } else { (en_icons, icons_found) };
                        // stacked subtitle lines (not with button icons: those are instructions)
                        let parts = match &crop {
                            // (subtitles only: an instructions box's sentences read as one text)
                            Some(cr) if icons.is_empty() && area.centered_only => stack_parts(cr, &words),
                            _ => Vec::new(),
                        };
                        // the block in frame fractions (the crop starts at the area's top-left)
                        let (cx0, cy0) = ((area.x.clamp(0.0, 1.0) * fw as f64) as u32, (area.y.clamp(0.0, 1.0) * fh as f64) as u32);
                        let to_frame = |(x, y, w, h): (u32, u32, u32, u32)| {
                            let (fw, fh) = (fw as f64, fh as f64);
                            [(cx0 + x) as f64 / fw, (cy0 + y) as f64 / fh, w as f64 / fw, h as f64 / fh]
                        };
                        let text_box = block.map(to_frame);
                        // the game's own box (only placed below it: "under_text")
                        let game_box = match (&crop, block) {
                            (Some(cr), Some(b)) if area.placement == "under_text" || (c.cover_look() && c.fx(&area.id).cover_min) => {
                                let mut hs: Vec<u32> = words.iter().map(|w| w.rect.3).collect();
                                hs.sort_unstable();
                                ocr::box_around(cr, b, hs.get(hs.len() / 2).copied().unwrap_or(20)).map(to_frame)
                            }
                            _ => None,
                        };
                        // (an area of the game's own box: the box it was read in, also when its edges
                        // weren't found this time but where it was found last)
                        let game_box = if area.needs_box { help_box.map(to_frame).or(game_box) } else { game_box };
                        seq_counter += 1;
                        latest.lock().unwrap().insert(area.id.clone(), seq_counter);
                        // the previous English line ended when this one appeared
                        let en_start = if recolor { st.current_start.unwrap_or(now) } else { st.candidate_since.unwrap_or(now) };
                        // (stacked lines: the block up isn't over, a line joined or left it; the new block's
                        // translation replaces it as soon as it's ready, not a blank meanwhile)
                        // (lines all new, none of the ones up still there: those end now, not stay until
                        // the new ones' translation comes)
                        let goes_on = stacks && st.candidate_lines.iter().any(|l| prev_lines.iter().any(|p| ocr::similar(l, p)));
                        if st.current_seq != 0 && !goes_on {
                            let _ = present.send(PMsg::Ended { area: area.id.clone(), seq: st.current_seq, at: if recolor { now } else { en_start } });
                        }
                        st.current_seq = seq_counter;
                        // typical English word height (median) -> the Arabic is sized to match
                        let mut hs: Vec<u32> = words.iter().map(|w| w.rect.3).collect();
                        hs.sort_unstable();
                        let measured = hs.get(hs.len() / 2).map(|&h| h as f64 / fh as f64).unwrap_or(0.0);
                        // same size for every line of this area
                        let line_h = sizes.size(&area.id, measured);
                        st.current_start = Some(en_start);
                        // (the text seen there now has its seq: its translation takes its black box down)
                        {
                            let mut covers = ctx.live.covers.lock().unwrap();
                            if let Some(v) = covers.get_mut(&area.id).filter(|v| v["seq"].as_u64() == Some(0)) {
                                v["seq"] = seq_counter.into();
                            }
                        }
                        // (the same line sent again, its colours seen: its boxes as first sent)
                        let (text_box, game_box) = match (recolor, st.line_boxes) {
                            (true, Some(b)) => b,
                            _ => (text_box, game_box),
                        };
                        st.line_boxes = Some((text_box, game_box));
                        let _ = jobs.send(Job { area: area.id.clone(), seq: seq_counter, en, phrases, icons, line_h, en_start, text_box, game_box, parts });
                    }
                } else if !st.current.is_empty()
                    && st.last_seen.is_some_and(|s| now - s > Duration::from_millis(clear_ms))
                {
                    // The English is gone (not just an OCR blink). It really ended when last seen; the
                    // presenter keeps the Arabic up for as long as the English was on screen.
                    if let Some(last) = st.last_seen {
                        let _ = present.send(PMsg::Ended { area: area.id.clone(), seq: st.current_seq, at: last });
                    }
                    st.current_start = None;
                    st.current.clear();
                    st.current_lines.clear();
                    st.candidate.clear();
                    st.candidate_lines.clear();
                    st.candidate_since = None;
                    st.current_seq = 0;
                    set_cover(&*ctx.sink, &ctx.live, &area.id, None);
                } else if st.current.is_empty()
                    && !st.candidate.is_empty()
                    && st.last_seen.is_some_and(|s| now - s > Duration::from_millis(clear_ms))
                {
                    // Text that came and went before it was ever translated: forget it, or its old start
                    // time would be reused when similar text shows up minutes later.
                    st.candidate.clear();
                    st.candidate_lines.clear();
                    st.candidate_since = None;
                    set_cover(&*ctx.sink, &ctx.live, &area.id, None);
                }
            }
        }
        let spent = t0.elapsed();
        perf_note(PerfKind::Read, spent.as_secs_f64() * 1000.0);
        done!(Duration::from_millis(interval_ms.max(MIN_INTERVAL_MS)).saturating_sub(spent));
    }
}

/// The reviewed translations the phone's contact rows were given, by contact: their message as read, its
/// translation (a row keeps it while its message reads much the same).
static PHONE_ROW_AR: Mutex<Option<HashMap<String, (String, String)>>> = Mutex::new(None);

/// The phone screen's corners as last found (in the frame, and in the frame its pieces are in), and when.
static PHONE_QUAD: Mutex<Option<([(f64, f64); 4], [(f64, f64); 4], Instant)>> = Mutex::new(None);

/// The phone screen's place as last read, and when (phone::steady_screen).
/// When a chat's header was last read on the phone (a chat's overlay only: see the phone block).
static PHONE_CHAT_AT: Mutex<Option<Instant>> = Mutex::new(None);
static PHONE_SCREEN: Mutex<Option<((u32, u32, u32, u32), Instant)>> = Mutex::new(None);

fn translate_loop(ctx: Context, latest: Arc<Mutex<HashMap<String, u64>>>, jobs: mpsc::Receiver<Job>, present: mpsc::Sender<PMsg>) {
    let Context { cfg, live: shared, memory, sink, paths } = ctx;
    let mut tr = Translator::new(paths.model_dir());
    // Subtitle lines (and a page's reason) first; a page's menu text only when nothing else waits,
    // newest first (the tab on screen now before an old backlog): a menu full of text never holds a
    // subtitle up for more than the one menu line being translated.
    let (mut lines_q, mut menu_q) = (std::collections::VecDeque::new(), std::collections::VecDeque::<Job>::new());
    loop {
        if lines_q.is_empty() && menu_q.is_empty() {
            match jobs.recv() {
                Ok(j) if j.area == MENU_TEXT => menu_q.push_back(j),
                Ok(j) => lines_q.push_back(j),
                Err(_) => return,
            }
        }
        for j in jobs.try_iter() {
            if j.area == MENU_TEXT {
                menu_q.push_back(j);
            } else {
                lines_q.push_back(j);
            }
        }
        let Some(job) = lines_q.pop_front().or_else(|| menu_q.pop_back()) else { continue };
        // Skip text that was already replaced on screen while we were busy. (Pause-menu lines
        // aren't an area: each is translated once and kept, so never skip those.)
        if job.area != MENU_TEXT && latest.lock().unwrap().get(&job.area).copied() != Some(job.seq) {
            continue;
        }
        let c = cfg.lock().unwrap().clone();
        let status_sink = sink.clone();
        let status = move |s: String| {
            status_sink.emit("status-msg", serde_json::Value::String(s));
        };
        // Translation memory first: a known line needs no translator at all.
        // (the Mission Failed reason too: same game text, same reuse)
        // (from the memory of the job's language only: just after a language switch the memory may
        // not have followed yet, and its lines would be in the other language)
        // Two speakers' lines (white and grey) are translated one by one: as one sentence the
        // translator merges or drops one ("… so... What?" lost the "What?"). Each comes back in its own
        // line, the other speaker's in grey like the game's.
        let t_translate = Instant::now();
        let (result, hit) = if job.parts.len() > 1 {
            let mut all: translate::Segments = Vec::new();
            let mut hits = true;
            let mut err = None;
            for (i, (en, rel)) in job.parts.iter().enumerate() {
                let (r, h) = translate_spoken(&mut tr, &c, &memory, en, &[], &job.phrases, &status);
                hits &= h;
                match r {
                    Ok(segs) => {
                        if i > 0 {
                            all.push(("\n".into(), None));
                        }
                        // the game's grey, on the plain parts (a highlighted word keeps its colour); the
                        // pages draw "dim" grey whatever the colour setting
                        let grey = (*rel < 0.95).then(|| "dim".to_string());
                        all.extend(segs.into_iter().map(|(t, col)| (t, col.or_else(|| grey.clone()))));
                    }
                    Err(e) => err = Some(e),
                }
            }
            (match err { Some(e) => Err(e), None => Ok(all) }, hits)
        } else {
            translate_spoken(&mut tr, &c, &memory, &job.en, &job.icons, &job.phrases, &status)
        };
        perf_note(PerfKind::Translate, t_translate.elapsed().as_secs_f64() * 1000.0);
        let hit_from = if hit { "memory" } else { "" };
        // time from the English line appearing to its Arabic being ready
        let ready_ms = job.en_start.elapsed().as_secs_f64() * 1000.0;
        if job.area != MENU_TEXT {
            perf_note(PerfKind::Ready, ready_ms);
        }
        let mem_note = {
            let mut m = memory.lock().unwrap();
            if m.lang() == c.target_lang {
                m.record(hit, ready_ms);
            }
            Some(if hit { format!("{hit_from} hit · ready {ready_ms:.0} ms after the English") } else { format!("ready {ready_ms:.0} ms after the English · saved to memory") })
        };
        // pause-menu text: keep it for the menu page (it appears on the next read)
        if job.area == MENU_TEXT {
            let ar: String = result.as_ref().map(|segs| segs.iter().map(|s| s.0.as_str()).collect()).unwrap_or_default();
            let mut mc = shared.menu_cache.lock().unwrap();
            // (asked in another language than the one in use now: the menu asks again in this one)
            if mc.lang != c.target_lang {
                continue;
            }
            if ar.trim().is_empty() {
                // (an error, or nothing: asked again later, when it's read again — see ocr_loop)
                shared.menu_failed.lock().unwrap().push(job.en.clone());
            } else {
                mc.map.insert(job.en.clone(), ar.trim().to_string());
            }
            continue;
        }
        // the English with its icons, for the live translations; and as text with a small marker
        let en_segments = if job.parts.len() > 1 {
            // (two speakers: the English in two lines too)
            let mut s: Segments = Vec::new();
            for (i, p) in job.parts.iter().enumerate() {
                if i > 0 {
                    s.push(("\n".into(), None));
                }
                s.push((p.0.clone(), None));
            }
            with_phrase_colors(s, &job.phrases)
        } else {
            with_phrase_colors(english_segments(&job.en, &job.icons), &job.phrases)
        };
        let job = Job { en: strip_icon_tokens(&job.en), ..job };
        // The Mission Failed reason is only useful while it is current. (Area lines go to the
        // presenter even if a newer line exists: it shows them for their time, then the next.)
        if job.area == SCREEN_REASON && latest.lock().unwrap().get(&job.area).copied() != Some(job.seq) {
            continue;
        }
        let (seq, en_start) = (job.seq, job.en_start);
        let ev = match result {
            Ok(segments) => TranslationEvent {
                area: job.area,
                en: job.en,
                en_segments,
                segments,
                stats: match &mem_note {
                    Some(n) if hit => n.clone(),
                    Some(n) => format!("{} · {n}", tr.stats(&c)),
                    None => tr.stats(&c),
                },
                error: None,
                line_h: job.line_h,
                text_box: job.text_box,
                game_box: job.game_box,
                cached: hit,
                at: unix_ms(en_start),
            },
            Err(e) => TranslationEvent {
                area: job.area,
                en: job.en,
                en_segments,
                segments: Vec::new(),
                stats: tr.stats(&c),
                error: Some(format!("{e:#}")),
                line_h: job.line_h,
                text_box: job.text_box,
                game_box: job.game_box,
                cached: false,
                at: unix_ms(en_start),
            },
        };
        if ev.area == SCREEN_REASON && ev.error.is_none() {
            *shared.screen_reason.lock().unwrap() = serde_json::to_value(&ev.segments).unwrap_or_default();
            crate::live::push(&shared, SCREEN_REASON, &ev.en, &ev.en_segments, &ev.segments, ev.at);
        }
        if ev.area == SCREEN_REASON || ev.error.is_some() {
            sink.emit("translation", serde_json::to_value(&ev).unwrap_or_default()); // errors go straight to the log
        } else {
            // (stacked lines: the new block replaces the one up at once)
            let now = c.fx(&ev.area).stacks(&ev.area);
            let _ = present.send(PMsg::Ready { seq, en_start, ev, now });
        }
    }
}

/// One English line to the target language: from the translation memory when it knows it, else
/// the translator (and then into the memory). True when it came from the memory.
/// `translate_line`, with a speaker name in front ("Dobbs: …") kept out of the translation: only the
/// sentence is translated and remembered, and the name comes back in front in the game's colour, in
/// the target language's letters (see `speaker`).
fn translate_spoken(
    tr: &mut Translator,
    c: &Config,
    memory: &Arc<Mutex<memory::Memory>>,
    en: &str,
    icons: &[String],
    phrases: &[(String, String)],
    status: &dyn Fn(String),
) -> (anyhow::Result<translate::Segments>, bool) {
    // "[laughs]": the sound cues off first (each translated on its own), back where they were at the end
    if let Some((cues, clean)) = speaker::take_cues(en) {
        let shown: Vec<String> = cues
            .before
            .iter()
            .chain(&cues.after_name)
            .chain(&cues.after)
            .map(|cue| {
                speaker::cue_in(&c.target_lang, cue).unwrap_or_else(|| {
                    let (r, _) = translate_line(tr, c, memory, cue, &[], &[], status);
                    r.ok().map(|segs| segs.iter().map(|s| s.0.as_str()).collect::<String>()).filter(|t| !t.trim().is_empty()).unwrap_or_else(|| cue.clone())
                })
            })
            .collect();
        // a cue-only line ("Donnie: [laughs]", "[rattling]"): the name (if any) and the cues
        let words = clean.chars().filter(|ch| ch.is_alphabetic()).count();
        let probe = format!("{clean} xx");
        let only_name = speaker::plain_name(&probe).is_some_and(|(_, rest)| rest == "xx");
        if words < 2 || only_name {
            let named = if only_name { speaker::plain_name(&probe).map(|(n, _)| n) } else { None };
            let segs = match &named {
                Some(name) => {
                    let color = phrases.iter().find(|(p, _)| p.starts_with(name.as_str())).map(|(_, col)| col.clone()).unwrap_or_else(|| speaker::PLAIN_NAME_COLOR.to_string());
                    let shown_name = speaker::name_in(&c.target_lang, name, reviewed_name(memory, c, name));
                    let mut s = speaker::with_name(&shown_name, &color, Vec::new());
                    s.truncate(2);
                    s
                }
                None => Vec::new(),
            };
            return (Ok(speaker::with_cues(&cues, &shown, named.is_some(), segs)), true);
        }
        let (result, hit) = translate_spoken(tr, c, memory, &clean, icons, phrases, status);
        let named = speaker::split(&clean, phrases).is_some() || speaker::plain_name(&clean).is_some();
        return (result.map(|segs| speaker::with_cues(&cues, &shown, named, segs)), hit);
    }
    let named = speaker::split(en, phrases).or_else(|| plain_speaker(memory, c, en));
    let (line, phrases) = match &named {
        Some((name, _, rest)) => (*rest, speaker::phrases_without(phrases, name)),
        None => (en, phrases.to_vec()),
    };
    // "In Spanish: …": the sentence alone, the tag in front (as the app knows it, or translated too)
    let (result, hit) = match speaker::language_tag(line) {
        Some((language, sentence)) => {
            let (result, hit) = translate_line(tr, c, memory, sentence, icons, &phrases, status);
            let tag = speaker::language_in(&c.target_lang, &language).or_else(|| {
                let (r, _) = translate_line(tr, c, memory, &format!("In {language}:"), &[], &[], status);
                r.ok().map(|segs| segs.iter().map(|s| s.0.as_str()).collect::<String>())
            });
            match tag {
                Some(tag) => (result.map(|segs| speaker::with_language(&tag, segs)), hit),
                None => (result, hit),
            }
        }
        None => translate_line(tr, c, memory, line, icons, &phrases, status),
    };
    let Some((name, color, _)) = named else { return (result, hit) };
    let mut reviewed = reviewed_name(memory, c, &name);
    // a role ("Cop:") is translated; outside Arabic, by the translator when no reviewed line has it
    if reviewed.is_none() && c.target_lang != "ar" && speaker::is_role(&name) {
        let (r, _) = translate_line(tr, c, memory, &name, &[], &[], status);
        reviewed = r.ok().map(|segs| segs.iter().map(|s| s.0.as_str()).collect());
    }
    let shown = if speaker::is_role(&name) && c.target_lang != "ar" {
        reviewed.map(|r| r.trim().trim_end_matches(|ch: char| matches!(ch, ':' | '.' | '!' | '?' | '،' | '؟' | '…')).trim().to_string()).filter(|r| !r.is_empty()).unwrap_or(name)
    } else {
        speaker::name_in(&c.target_lang, &name, reviewed)
    };
    (result.map(|segs| speaker::with_name(&shown, &color, segs)), hit)
}

/// A "Name:" the colour check missed, taken when the app knows it: one of the game's people, a name
/// with a reviewed translation, or a sentence it knows without the name. (name, colour, rest)
fn plain_speaker<'a>(memory: &Arc<Mutex<memory::Memory>>, c: &Config, en: &'a str) -> Option<(String, String, &'a str)> {
    let (name, rest) = speaker::plain_name(en)?;
    let known = speaker::is_known(&name) || speaker::is_role(&name) || reviewed_name(memory, c, &name).is_some() || memory.lock().unwrap().knows(rest);
    known.then(|| (name, speaker::PLAIN_NAME_COLOR.to_string(), rest))
}

/// A reviewed translation of a name alone ("Dobbs" -> "دوبس") from the downloaded translations.
fn reviewed_name(memory: &Arc<Mutex<memory::Memory>>, c: &Config, name: &str) -> Option<String> {
    let m = memory.lock().unwrap();
    if m.lang() != c.target_lang {
        return None;
    }
    m.get(name, &[], &[]).filter(|(_, src)| src == "reviewed").map(|(segs, _)| segs.iter().map(|s| s.0.as_str()).collect())
}

fn translate_line(
    tr: &mut Translator,
    c: &Config,
    memory: &Arc<Mutex<memory::Memory>>,
    en: &str,
    icons: &[String],
    phrases: &[(String, String)],
    status: &dyn Fn(String),
) -> (anyhow::Result<translate::Segments>, bool) {
    // (from the memory of the job's language only: just after a language switch the memory may
    // not have followed yet, and its lines would be in the other language)
    let mut remembered = {
        let m = memory.lock().unwrap();
        if m.lang() == c.target_lang { m.get(en, icons, phrases) } else { None }
    };
    // (not one that mangled a term that must stay, like GTA+ — made before terms were protected)
    if remembered.as_ref().is_some_and(|(segs, _)| !translate::keeps_terms(en, &segs.iter().map(|s| s.0.as_str()).collect::<String>())) {
        remembered = None;
    }
    // A hand-written phrasebook line beats a machine translation remembered earlier (only an
    // reviewed line ranks above it).
    if remembered.as_ref().is_some_and(|(_, src)| src != "reviewed" && src != "phrasebook") {
        if phrasebook::find(&c.game(), &c.target_lang, en, icons).is_some() {
            remembered = None; // the translator below answers from the phrasebook, and memory is updated
        }
    }
    let hit = remembered.is_some();
    let result = match remembered {
        // (coloured here when the remembered line has no colours but the game highlights a word)
        Some((segs, _)) => Ok(translate::polite(&c.target_lang, tr.recolor(c, segs, phrases, status))),
        None => tr.translate(c, en, phrases, icons, status),
    };
    if let (false, Ok(segs)) = (hit, &result) {
        let mut m = memory.lock().unwrap();
        // (translated for the language in use when the job started: if the player switched since,
        // it belongs to neither memory)
        // (not one that lost a term that must stay, like GTA+: it would be turned down when reused)
        if m.lang() == c.target_lang && translate::keeps_terms(en, &segs.iter().map(|s| s.0.as_str()).collect::<String>()) {
            let source = if phrasebook::find(&c.game(), &c.target_lang, en, icons).is_some() { "phrasebook" } else { c.engine.as_str() };
            m.put(en, segs, source);
        }
    }
    (result, hit)
}

/// "Press ⟦1⟧ to jump" with its icons -> ["Press ", icon 1, " to jump"]: the English line as segments,
/// each "⟦n⟧" the game's own button icon (as in the translation). A mark with no icon becomes 🎮.
pub fn english_segments(en: &str, icons: &[String]) -> Segments {
    let mut segs: Segments = Vec::new();
    let mut text = String::new();
    let mut rest = en;
    while let Some(open) = rest.find('⟦') {
        let after = &rest[open + '⟦'.len_utf8()..];
        let Some(close) = after.find('⟧') else { break };
        text.push_str(&rest[..open]);
        let icon = after[..close].parse::<usize>().ok().and_then(|n| icons.get(n.wrapping_sub(1)));
        match icon {
            Some(png) => {
                if !text.is_empty() {
                    segs.push((std::mem::take(&mut text), None));
                }
                segs.push((String::new(), Some(format!("icon:{png}"))));
            }
            None => text.push('🎮'),
        }
        rest = &after[close + '⟧'.len_utf8()..];
    }
    text.push_str(rest);
    if !text.is_empty() {
        segs.push((text, None));
    }
    segs
}

/// The game's coloured words (`phrases`: text and colour, in reading order) as coloured segments of the
/// English, as the translation has them: "Restore the engine" with "engine" in pink. Icon segments and
/// text already coloured stay as they are; a phrase not found in the text is skipped.
pub fn with_phrase_colors(segs: Segments, phrases: &[(String, String)]) -> Segments {
    let mut out = segs;
    for (phrase, color) in phrases {
        let phrase = phrase.trim();
        if phrase.is_empty() {
            continue;
        }
        let mut next: Segments = Vec::with_capacity(out.len() + 2);
        let mut done = false;
        for (text, c) in out {
            let at = if done || c.is_some() { None } else { text.find(phrase).or_else(|| text.to_lowercase().find(&phrase.to_lowercase()).filter(|&i| text.is_char_boundary(i) && text.is_char_boundary(i + phrase.len()))) };
            match at {
                Some(i) => {
                    let (before, rest) = text.split_at(i);
                    let (word, after) = rest.split_at(phrase.len());
                    if !before.is_empty() {
                        next.push((before.to_string(), None));
                    }
                    next.push((word.to_string(), Some(color.clone())));
                    if !after.is_empty() {
                        next.push((after.to_string(), None));
                    }
                    done = true;
                }
                None => next.push((text, c)),
            }
        }
        out = next;
    }
    out
}

/// "Press ⟦1⟧ to jump" -> "Press 🎮 to jump" (for showing English lines in the log).
pub fn strip_icon_tokens(s: &str) -> String {
    s.split_whitespace()
        .map(|t| if t.starts_with('⟦') && t.ends_with('⟧') { "🎮" } else { t })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod size_lock_tests {
    use super::*;

    fn measured(l: &mut SizeLock, kind: &str, v: f64) -> f64 {
        (0..SIZE_SAMPLES).map(|_| l.size(kind, v)).last().unwrap()
    }

    #[test]
    fn another_resolution_measures_again() {
        let mut l = SizeLock::default();
        assert!(!l.at_resolution(1920, 1080), "the first picture: nothing to forget");
        assert_eq!(measured(&mut l, "subtitles", 0.03), 0.03);
        assert_eq!(l.size("subtitles", 0.05), 0.03, "locked");
        // same resolution: kept
        assert!(!l.at_resolution(1920, 1080));
        assert_eq!(l.size("subtitles", 0.05), 0.03);
        // another one: measured again on the next lines
        assert!(l.at_resolution(1280, 720));
        assert!(l.locked.is_empty());
        assert_eq!(measured(&mut l, "subtitles", 0.04), 0.04);
        // no picture size (no frame yet): nothing changes
        assert!(!l.at_resolution(0, 0));
        assert_eq!(l.size("subtitles", 0.05), 0.04);
    }

    #[test]
    fn another_subtitle_size_in_the_game_measures_again() {
        let mut l = SizeLock::default();
        assert_eq!(measured(&mut l, "subtitles", 0.030), 0.030);
        // a line or two with short letters, or a big one: the locked size stays
        for v in [0.022, 0.023, 0.030, 0.040, 0.022, 0.031, 0.022, 0.023, 0.024] {
            assert_eq!(l.size("subtitles", v), 0.030, "{v}");
        }
        // off on both sides in turn: not a new size
        for v in [0.022, 0.040, 0.022, 0.040, 0.022, 0.040] {
            assert_eq!(l.size("subtitles", v), 0.030, "{v}");
        }
        assert_eq!(l.size("subtitles", 0.030), 0.030);
        // Large in the game's settings: five bigger lines in a row lock the new size
        for v in [0.037, 0.036, 0.038, 0.036] {
            assert_eq!(l.size("subtitles", v), 0.030);
        }
        assert_eq!(l.size("subtitles", 0.037), 0.037);
        assert_eq!(l.size("subtitles", 0.030), 0.037, "locked again");
        // and back to Small
        let small: Vec<f64> = [0.025, 0.026, 0.024, 0.025, 0.025].iter().map(|&v| l.size("subtitles", v)).collect();
        assert_eq!(small.last(), Some(&0.025));
        // other kinds of text keep their own size
        assert_eq!(measured(&mut l, "help", 0.02), 0.02);
        assert_eq!(l.size("subtitles", 0.024), 0.025);
    }

    #[test]
    fn sizes_saved_before_the_resolution_was_kept_stay() {
        let mut l: SizeLock = serde_json::from_str(r#"{"locked":{"subtitles":0.03}}"#).unwrap();
        assert!(!l.at_resolution(1920, 1080));
        assert_eq!(l.size("subtitles", 0.05), 0.03);
        assert_eq!(l.res, Some((1920, 1080)));
    }
}

#[cfg(test)]
mod english_icon_tests {
    use super::*;

    fn icon(n: &str) -> (String, Option<String>) {
        (String::new(), Some(format!("icon:{n}")))
    }

    #[test]
    fn the_english_keeps_its_icons() {
        let icons = vec!["1.1|data:image/png;base64,AAA".to_string(), "data:image/png;base64,BBB".to_string()];
        assert_eq!(
            english_segments("Press ⟦1⟧ to jump", &icons),
            vec![("Press ".to_string(), None), icon("1.1|data:image/png;base64,AAA"), (" to jump".to_string(), None)]
        );
        // at the start, at the end, side by side
        assert_eq!(english_segments("⟦2⟧ ⟦1⟧", &icons), vec![icon("data:image/png;base64,BBB"), (" ".to_string(), None), icon("1.1|data:image/png;base64,AAA")]);
        // a mark with no icon cut for it: the game pad, as in the text
        assert_eq!(english_segments("Hold ⟦3⟧", &icons), vec![("Hold 🎮".to_string(), None)]);
        // no marks: the line as it is
        assert_eq!(english_segments("Meet me at the docks.", &icons), vec![("Meet me at the docks.".to_string(), None)]);
        assert!(english_segments("", &icons).is_empty());
    }
}

/// The fixed words of a game page that the script or the layout's words didn't translate: each from
/// the session's translations (`cache`, filled by MENU_TEXT jobs), else the translation memory and the ready
/// translations (`known`), else the translator (`ask` queues one). Until translated, tabs,
/// buttons and the name block keep the English, and no title is drawn. Returns true while any is
/// missing.
fn localize_screen(l: &mut screens::ScreenLayout, cache: &HashMap<String, String>, known: &dyn Fn(&str) -> Option<String>, ask: &mut dyn FnMut(&str)) -> bool {
    // "MAP", "MONDAY 05:00": translated as ordinary words ("Map"), not shouted letters
    let words = |en: &str| -> String {
        if en.chars().any(|c| c.is_lowercase()) {
            return en.to_string();
        }
        en.split(' ')
            .map(|w| {
                let mut cs = w.chars();
                cs.next().map(|f| f.to_uppercase().chain(cs.flat_map(char::to_lowercase)).collect()).unwrap_or_default()
            })
            .collect::<Vec<String>>()
            .join(" ")
    };
    let mut missing = false;
    let mut get = |en: &str, missing: &mut bool| -> Option<String> {
        let key = words(en);
        // (the memory keeps the game's lines as read: the English as it is first, then as words)
        let hit = cache.get(&key).cloned().or_else(|| known(en)).or_else(|| if key != en { known(&key) } else { None });
        if hit.is_none() {
            ask(&key);
            *missing = true;
        }
        hit
    };
    // (what the script, or the layout's words, already translated stays; the English until then)
    let untranslated = |ar: &str, en: &str| ar.is_empty() || ar == en;
    if !l.title_en.is_empty() && untranslated(&l.title_ar, &l.title_en) {
        l.title_ar = get(&l.title_en, &mut missing).unwrap_or_default();
    }
    for c in l.cards.iter_mut() {
        if !c.title_en.is_empty() && untranslated(&c.title_ar, &c.title_en) {
            c.title_ar = get(&c.title_en, &mut missing).unwrap_or_default();
        }
        if !c.text_en.is_empty() && untranslated(&c.text_ar, &c.text_en) {
            c.text_ar = get(&c.text_en, &mut missing).unwrap_or_default();
        }
        if !c.head_en.is_empty() && untranslated(&c.head_ar, &c.head_en) {
            c.head_ar = get(&c.head_en, &mut missing).unwrap_or_default();
        }
        for x in c.sub.iter_mut() {
            if !x.en.is_empty() && untranslated(&x.ar, &x.en) {
                x.ar = get(&x.en, &mut missing).unwrap_or_default();
            }
        }
        // (a value of numbers or money stays as it is: the script gives it back unchanged)
        for r in c.rows.iter_mut() {
            if !r.en.is_empty() && untranslated(&r.ar, &r.en) {
                r.ar = get(&r.en, &mut missing).unwrap_or_default();
            }
            if r.value_en.chars().filter(|ch| ch.is_alphabetic()).count() >= 2 && untranslated(&r.value_ar, &r.value_en) {
                r.value_ar = get(&r.value_en, &mut missing).unwrap_or_default();
            }
        }
    }
    for lb in l.labels.iter_mut() {
        if untranslated(&lb.ar, &lb.en) {
            lb.ar = get(&lb.en, &mut missing).unwrap_or_else(|| lb.en.clone());
        }
    }
    for b in l.buttons.iter_mut() {
        if untranslated(&b.ar, &b.en) {
            b.ar = get(&b.en, &mut missing).unwrap_or_else(|| b.en.clone());
        }
    }
    if let Some(p) = l.player.as_mut() {
        // line by line: what the script translated stays (a time or a number reads the same anyway)
        p.ar.resize(p.en.len(), String::new());
        for (a, e) in p.ar.iter_mut().zip(&p.en) {
            if untranslated(a, e) && e.chars().any(char::is_alphabetic) {
                *a = get(e, &mut missing).unwrap_or_else(|| e.clone());
            } else if a.is_empty() {
                *a = e.clone();
            }
        }
    }
    missing
}

/// Take the profile page down right away (the game's own page is gone).
fn hide_screen(sink: &dyn Sink, shared: &Live, latest: &Mutex<HashMap<String, u64>>, screen: &mut Option<screens::ScreenLayout>) {
    *screen = None;
    // its reason may still be with the translator: translate_loop drops it when it comes
    latest.lock().unwrap().remove(SCREEN_REASON);
    shared.screen_active.store(false, std::sync::atomic::Ordering::Relaxed);
    *shared.screen.lock().unwrap() = None;
    shared.screen_private.lock().unwrap().clear();
    *shared.screen_reason.lock().unwrap() = serde_json::json!([]);
    sink.emit("screen", serde_json::Value::Null);
}

/// Shortest time a line may be missed before it counts as gone (ms). A subtitle drops out of a reading
/// now and then (bright ground behind it, the text fading, a frame the stream dropped); ended after
/// 100 ms, it came back as a new line that had to settle again, and a short subtitle could be gone
/// before its translation, which the memory had, was shown.
pub(crate) const DROPOUT_FLOOR_MS: u64 = 250;

/// When an area's English counts as gone: lines that stack up after the dropout floor at least; an
/// instruction stays up and is read over every kind of ground: 350 ms at least.
pub(crate) fn clear_ms_for(stacks: bool, clear_after_ms: u64) -> u64 {
    if stacks { clear_after_ms.max(DROPOUT_FLOOR_MS) } else { clear_after_ms.max(350) }
}

#[cfg(test)]
mod english_color_tests {
    #[test]
    fn a_short_dropout_never_ends_a_line() {
        // the old default, 100 ms: a subtitle missed for a moment stays up
        assert_eq!(super::clear_ms_for(true, 100), super::DROPOUT_FLOOR_MS);
        assert_eq!(super::clear_ms_for(true, 300), 300);
        assert_eq!(super::clear_ms_for(false, 100), 350);
        assert_eq!(super::clear_ms_for(false, 900), 900);
    }

    use super::*;

    #[test]
    fn the_games_coloured_words_are_coloured_in_the_english() {
        let segs = with_phrase_colors(english_segments("Restore the engine", &[]), &[("engine".into(), "#ff86bf".into())]);
        assert_eq!(segs, vec![("Restore the ".to_string(), None), ("engine".to_string(), Some("#ff86bf".to_string()))]);
    }

    #[test]
    fn icons_stay_and_a_phrase_not_found_is_skipped() {
        let icons = vec!["data:x".to_string()];
        let segs = with_phrase_colors(english_segments("Press ⟦1⟧ to follow Nate", &icons), &[("Nate".into(), "#ff86bf".into()), ("Lucia".into(), "#ff86bf".into())]);
        assert_eq!(segs, vec![("Press ".to_string(), None), (String::new(), Some("icon:data:x".to_string())), (" to follow ".to_string(), None), ("Nate".to_string(), Some("#ff86bf".to_string()))]);
    }
}

#[cfg(test)]
mod localize_screen_tests {
    use super::*;

    #[test]
    fn the_maps_box_takes_known_translations_before_the_translator() {
        let mut l = screens::ScreenLayout {
            cards: vec![screens::Card {
                twin: true,
                head_en: "DUNK'S ROBBERY TIPS".into(),
                title_en: "LOST VISTA".into(),
                text_en: "There's a stash of pills somewhere.".into(),
                sub: vec![screens::CardLine { en: "Come back at night".into(), ..Default::default() }],
                rows: vec![
                    screens::CardRow { en: "SECURITY".into(), ar: "الحراسة".into(), value_en: "LOW".into(), ..Default::default() },
                    screens::CardRow { en: "TAKE VALUE".into(), value_en: "$1,000 - $10,000".into(), value_ar: "$1,000 - $10,000".into(), ..Default::default() },
                ],
                ..Default::default()
            }],
            ..Default::default()
        };
        // this session's translations first, then the memory (the ready translations), the rest asked for
        let cache: HashMap<String, String> = [("Lost Vista".to_string(), "لوست فيستا".to_string())].into();
        let known = |en: &str| -> Option<String> {
            match en {
                "There's a stash of pills somewhere." => Some("هناك مخبأ حبوب في مكان ما.".into()),
                "Low" => Some("منخفض".into()),
                _ => None,
            }
        };
        let mut asked = Vec::new();
        let missing = localize_screen(&mut l, &cache, &known, &mut |en: &str| asked.push(en.to_string()));
        let c = &l.cards[0];
        assert_eq!(c.title_ar, "لوست فيستا");
        assert_eq!(c.text_ar, "هناك مخبأ حبوب في مكان ما.");
        assert_eq!(c.rows[0].ar, "الحراسة", "the script's own word stays");
        assert_eq!(c.rows[0].value_ar, "منخفض");
        assert_eq!(c.rows[1].value_ar, "$1,000 - $10,000", "money isn't sent");
        assert!(missing);
        assert_eq!(asked, ["Dunk's Robbery Tips", "Come back at night", "Take Value"]);
    }
}
