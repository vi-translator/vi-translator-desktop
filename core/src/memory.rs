//! Translation memory: remember every translated line, reuse it instantly next time.
//!
//! Keys are normalised English (lower-case, no punctuation, button icons as `{1}`, `{2}`), so the same
//! game line matches whatever icon images or punctuation OCR produced. The Arabic is stored as a
//! template: `{n}` for the n-th icon and `<cK>…</cK>` for the K-th highlighted (coloured) phrase, so
//! icons and colours from the current frame are put back on reuse.
//!
//! Everything lives in one folder ("translation DB"), encrypted (see `secure`):
//!  - `memory-<lang>.dat` — lines translated live by the chosen translator (appended, one sealed
//!                      record per line). Lines that come from a downloaded shared pack replace them;
//!  - `reviewed.jsonl` — only on the maintainer's PC: reviewed translations being prepared (plain
//!                      text, read if present; the app never writes it).
//! Files from before encryption (`memory.jsonl`, `memory-<lang>.jsonl`) are converted once, then removed.

use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::secure;
use crate::translate::Segments;

#[derive(Serialize, Deserialize, Clone)]
pub struct Entry {
    /// normalised English key
    pub en: String,
    /// Arabic template with `{n}` icons and `<cK>…</cK>` colours
    pub ar: String,
    /// which translator made it ("claude", "deepl", "local", "phrasebook", "reviewed" …)
    pub source: String,
}

#[derive(Serialize, Clone, Default)]
pub struct Stats {
    pub entries: usize,
    /// how many of them are reviewed translations
    pub reviewed: usize,
    pub hits: u64,
    pub misses: u64,
    /// average time from the English line appearing to its Arabic being ready
    pub avg_hit_ms: f64,
    pub avg_miss_ms: f64,
}

pub struct Memory {
    dir: PathBuf,
    /// target language: each has its own memory file
    lang: String,
    path: PathBuf,
    map: HashMap<String, Entry>,
    /// word -> keys containing it, so near matches only compare lines that share most words
    index: HashMap<String, Vec<String>>,
    hits: u64,
    misses: u64,
    hit_ms: f64,
    miss_ms: f64,
    /// translations downloaded in the background, by language (the one in use is applied; the others
    /// wait for a switch back): kept through reload, clear and language switches
    packs: HashMap<String, Vec<Entry>>,
    /// the near match found for recent texts (None: none close enough), until the memory changes
    near_cache: std::cell::RefCell<HashMap<String, Option<String>>>,
}

/// How many texts' near matches are kept (then all are forgotten and found again).
const NEAR_CACHE: usize = 2048;
/// How many of a text's rarest words find the lines it's compared with.
const RARE_WORDS: usize = 3;
/// How many of those lines (most words in common first) are compared letter by letter (many: a line
/// misread twice, "Get ih your vehiéle", shares only its common words with its own, among hundreds alike).
const NEAR_TRIES: usize = 600;

/// How much we trust each source: a better translation is never replaced by a worse one.
fn rank(source: &str) -> u8 {
    match source {
        "reviewed" => 6,
        "phrasebook" => 5,
        "claude" | "openai" => 4,
        "deepl" | "azure" | "google" => 3,
        "libre" => 2,
        _ => 1, // local model
    }
}

/// An icon "⟦n⟧" or "{n}" stuck to text ("⟦1⟧m", "(⟦1⟧/⟦2⟧)", "⟦1⟧⟦2⟧") as a word of its own: spaces
/// round each one.
fn icons_apart(line: &str) -> String {
    let cs: Vec<char> = line.chars().collect();
    let mut out = String::with_capacity(line.len() + 8);
    let mut i = 0;
    while i < cs.len() {
        let close = match cs[i] {
            '⟦' => Some('⟧'),
            '{' => Some('}'),
            _ => None,
        };
        if let Some(close) = close {
            let mut j = i + 1;
            while j < cs.len() && cs[j].is_ascii_digit() {
                j += 1;
            }
            if j > i + 1 && j < cs.len() && cs[j] == close {
                out.push(' ');
                out.extend(&cs[i..=j]);
                out.push(' ');
                i = j + 1;
                continue;
            }
        }
        out.push(cs[i]);
        i += 1;
    }
    out
}

/// Lower-case words; icons "⟦n⟧" -> "{n}" (renumbered in order, each a word of its own even stuck to
/// text); punctuation dropped.
pub fn key_of(line: &str) -> String {
    let line = &icons_apart(line);
    let mut out = Vec::new();
    let mut icon = 0;
    for tok in line.split_whitespace() {
        let core = tok.trim_matches(|c: char| !c.is_alphanumeric() && c != '⟦' && c != '⟧'); // "⟦1⟧." is still an icon
        // "{n}" too, so a key that is already normalised (reviewed files) stays the same
        let braced = tok.len() > 2 && tok.starts_with('{') && tok.ends_with('}') && tok[1..tok.len() - 1].chars().all(|c| c.is_ascii_digit());
        if braced || (core.starts_with('⟦') && core.ends_with('⟧')) {
            icon += 1;
            out.push(format!("{{{icon}}}"));
            continue;
        }
        // (lower case first, then only letters, numbers and apostrophes: so a key is its own key)
        let w: String = tok.to_lowercase().chars().filter(|c| c.is_alphanumeric() || *c == '\'').collect();
        if !w.is_empty() {
            out.push(w);
        }
    }
    out.join(" ")
}

fn similarity(a: &str, b: &str) -> f64 {
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

/// Segments -> template (icons -> {n}, coloured runs -> <cK>…</cK>).
fn to_template(segs: &Segments) -> String {
    let (mut out, mut icon, mut col) = (String::new(), 0, 0);
    for (t, c) in segs {
        match c.as_deref() {
            Some(c) if c.starts_with("icon:") => {
                icon += 1;
                out.push_str(&format!("{{{icon}}}"));
            }
            Some(_) => {
                col += 1;
                out.push_str(&format!("<c{col}>{t}</c{col}>"));
            }
            None => out.push_str(t),
        }
    }
    out
}

/// The icons as their slots ("1", "2" …: see `slot_text`), for a translation shown with its own pictures.
pub fn slots(n: usize) -> Vec<String> {
    (1..=n.max(1)).map(|i| i.to_string()).collect()
}

/// Segments as text, each icon as its slot "{n}" (filled with `slots`): the drawing puts the pictures in.
pub fn slot_text(segs: &Segments) -> String {
    segs.iter()
        .map(|(t, c)| match c.as_deref().and_then(|c| c.strip_prefix("icon:")) {
            Some(n) => format!("{{{n}}}"),
            None => t.clone(),
        })
        .collect::<String>()
        .trim()
        .to_string()
}

/// Template -> segments with this frame's icons and highlight colours.
pub(crate) fn fill(tpl: &str, icons: &[String], phrases: &[(String, String)]) -> Segments {
    let mut segs: Segments = Vec::new();
    let mut rest = tpl;
    loop {
        let next_icon = rest.find('{');
        let next_col = rest.find("<c");
        let at = match (next_icon, next_col) {
            (None, None) => break,
            (Some(a), None) | (None, Some(a)) => a,
            (Some(a), Some(b)) => a.min(b),
        };
        if at > 0 {
            segs.push((rest[..at].to_string(), None));
        }
        rest = &rest[at..];
        if rest.starts_with('{') {
            let Some(close) = rest.find('}') else { break };
            if let Some(png) = rest[1..close].parse::<usize>().ok().and_then(|n| icons.get(n.wrapping_sub(1))) {
                segs.push((String::new(), Some(format!("icon:{png}"))));
            }
            rest = &rest[close + 1..];
        } else {
            let Some(gt) = rest.find('>') else { break };
            let k = rest[2..gt].to_string();
            let close_tag = format!("</c{k}>");
            let Some(end) = rest.find(&close_tag) else { break };
            let text = rest[gt + 1..end].to_string();
            let color = k.parse::<usize>().ok().and_then(|k| phrases.get(k.wrapping_sub(1))).map(|p| p.1.clone());
            segs.push((text, color));
            rest = &rest[end + close_tag.len()..];
        }
    }
    if !rest.is_empty() {
        segs.push((rest.to_string(), None));
    }
    segs
}

impl Memory {
    /// Load the memory for a target language from the translation-DB folder (`memory-<lang>.dat`, plus
    /// `reviewed.jsonl` for Arabic when it's there). A memory file from before encryption is converted.
    pub fn load(dir: PathBuf, lang: &str) -> Self {
        convert_other_languages(&dir, lang);
        let path = dir.join(format!("memory-{lang}.dat"));
        let kind = format!("memory/{lang}");
        let mut map: HashMap<String, Entry> = HashMap::new();
        let add = |e: Entry, map: &mut HashMap<String, Entry>| {
            // later lines win unless a better source is already there
            let keep_old = map.get(&e.en).is_some_and(|old: &Entry| rank(&old.source) > rank(&e.source));
            if !keep_old {
                map.insert(e.en.clone(), e);
            }
        };
        if let Ok(text) = std::fs::read_to_string(&path) {
            for line in text.lines() {
                if let Some(e) = secure::open(&kind, line).and_then(|j| serde_json::from_str::<Entry>(&j).ok()) {
                    add(e, &mut map);
                }
            }
        }
        // before encryption: memory.jsonl (Arabic) or memory-<lang>.jsonl, in plain text
        let legacy = if lang == "ar" { dir.join("memory.jsonl") } else { dir.join(format!("memory-{lang}.jsonl")) };
        if let Ok(text) = std::fs::read_to_string(&legacy) {
            let moved: Vec<Entry> = text.lines().filter_map(|l| serde_json::from_str::<Entry>(l).ok()).collect();
            if append_sealed(&path, &kind, moved.iter().map(|e| serde_json::to_string(e).unwrap_or_default())) {
                let _ = std::fs::remove_file(&legacy);
            }
            for e in moved {
                add(e, &mut map);
            }
        }
        if lang == "ar" {
            if let Ok(text) = std::fs::read_to_string(dir.join("reviewed.jsonl")) {
                for line in text.lines() {
                    if let Ok(mut e) = serde_json::from_str::<Entry>(line) {
                        e.source = "reviewed".into();
                        e.en = key_of(&e.en); // a hand-edited file may keep raw English
                        add(e, &mut map);
                    }
                }
            }
        }
        let mut m = Self { dir, lang: lang.to_string(), path, map: HashMap::new(), index: HashMap::new(), hits: 0, misses: 0, hit_ms: 0.0, miss_ms: 0.0, packs: HashMap::new(), near_cache: Default::default() };
        for (k, e) in map {
            m.insert(k, e);
        }
        m
    }

    fn remove(&mut self, key: &str) {
        self.near_cache.get_mut().clear();
        if self.map.remove(key).is_some() {
            for w in key.split(' ') {
                if let Some(keys) = self.index.get_mut(w) {
                    keys.retain(|k| k != key);
                }
            }
        }
    }

    fn insert(&mut self, key: String, e: Entry) {
        self.near_cache.get_mut().clear();
        if !self.map.contains_key(&key) {
            let mut words: Vec<&str> = key.split(' ').collect();
            words.sort_unstable();
            words.dedup();
            for w in words {
                self.index.entry(w.to_string()).or_default().push(key.clone());
            }
        }
        self.map.insert(key, e);
    }

    /// Best entry for this English: exact key, else a close one (≥ 86–92 %, same number of icons).
    /// A near match is looked for once per text: the answer is kept (`near_cache`) until the
    /// memory changes, as the same subtitle is read again on every pass while it's on screen.
    fn find(&self, key: &str) -> Option<&Entry> {
        if let Some(e) = self.map.get(key) {
            return Some(e);
        }
        let known = self.near_cache.borrow().get(key).cloned();
        let found = match known {
            Some(found) => found,
            None => {
                let found = self.near(key);
                let mut cache = self.near_cache.borrow_mut();
                if cache.len() >= NEAR_CACHE {
                    cache.clear();
                }
                cache.insert(key.to_string(), found.clone());
                found
            }
        };
        found.and_then(|k| self.map.get(&k))
    }

    /// The key of the line closest to `key`, if one is close enough. Only lines that share one of
    /// `key`'s rarest words are compared (via the word index): a line that shares none of its few
    /// rarest words isn't that line. Counting every line with "the" or "you" in it, as before, took
    /// 24 ms for a near hit and 140 ms for a miss in a memory of 400,000 lines.
    fn near(&self, key: &str) -> Option<String> {
        let mut words: Vec<&str> = key.split(' ').collect();
        words.sort_unstable();
        words.dedup();
        // (words the memory doesn't have at all are left out: OCR noise, "Yepr", "cometo")
        let mut known: Vec<&Vec<String>> = words.iter().filter_map(|w| self.index.get(*w)).collect();
        known.sort_unstable_by_key(|keys| keys.len());
        let icons = key.matches('{').count();
        let len = key.chars().count() as f64;
        // (lines of about the same length only: can't be that alike otherwise)
        let fits = |k: &str| {
            let kl = k.chars().count() as f64;
            (kl - len).abs() <= len * 0.15 + 2.0 && k.matches('{').count() == icons
        };
        let mut shared: HashMap<&str, usize> = HashMap::new();
        for keys in known.iter().take(RARE_WORDS) {
            for k in keys.iter() {
                if let Some(n) = shared.get_mut(k.as_str()) {
                    *n += 1;
                } else if fits(k) {
                    shared.insert(k.as_str(), 1);
                }
            }
        }
        let mut cands: Vec<(&str, usize)> = shared.into_iter().collect();
        cands.sort_unstable_by(|a, b| b.1.cmp(&a.1));
        let mut best: Option<(f64, &str)> = None;
        // compared without spaces (a lost or extra space is no difference); long lines may differ a
        // little more than short ones, where one letter can change the meaning
        let bare = |t: &str| t.replace(' ', "");
        let key_bare = bare(key);
        let need_sim = if key_bare.chars().count() >= 12 { 0.86 } else { 0.92 };
        for (k, _) in cands.into_iter().take(NEAR_TRIES) {
            let s = similarity(&bare(k), &key_bare);
            if s >= need_sim && best.is_none_or(|(b, _)| s > b) {
                best = Some((s, k));
            }
        }
        best.map(|(_, k)| k.to_string())
    }

    pub fn knows(&self, line: &str) -> bool {
        let key = key_of(line);
        !key.is_empty() && self.find(&key).is_some()
    }

    /// The remembered Arabic for this line (with this frame's icons and colours) and its source.
    pub fn get(&self, line: &str, icons: &[String], phrases: &[(String, String)]) -> Option<(Segments, String)> {
        let key = key_of(line);
        if key.is_empty() {
            return None;
        }
        self.find(&key).map(|e| (fill(&e.ar, icons, phrases), e.source.clone()))
    }

    /// A remembered line that starts with `start`, a line the game cut off ("Me again. Can't wait
    /// to ge..."): its whole English key, its Arabic (filled with no icons) and its source, the most trusted
    /// first, then the shortest. The last word may be cut too ("ge" of "get"). Lines with at least
    /// three whole words only: a shorter start fits too many lines.
    pub fn starting_with(&self, start: &str) -> Option<(String, String, String)> {
        let start = start.trim_end().trim_end_matches(['.', '…']).trim_end();
        let key = key_of(start);
        let mut words: Vec<&str> = key.split(' ').filter(|w| !w.is_empty()).collect();
        // (a cut word: "ge"; a whole one would match too, as its own start)
        let part = words.pop()?;
        if words.len() < 3 {
            return None;
        }
        let rarest = words.iter().filter_map(|w| self.index.get(*w)).min_by_key(|keys| keys.len())?;
        let head = words.join(" ");
        let mut best: Option<&Entry> = None;
        for k in rarest {
            // (its icons' slots aside: a chat row's start is read without its emoji, "thnx hon my rum is be...",
            // the game's line has it between the words, "thnx hon {1} my rum is behind counter")
            let bare: String;
            let k_cmp: &str = if k.contains('{') {
                bare = k.split(' ').filter(|w| !(w.starts_with('{') && w.ends_with('}'))).collect::<Vec<_>>().join(" ");
                &bare
            } else {
                k
            };
            let Some(rest) = k_cmp.strip_prefix(head.as_str()) else { continue };
            let Some(next) = rest.strip_prefix(' ').and_then(|r| r.split(' ').next()) else { continue };
            if !next.starts_with(part) {
                continue;
            }
            let Some(e) = self.map.get(k) else { continue };
            let better = |b: &Entry| (rank(&e.source), std::cmp::Reverse(e.en.len())) > (rank(&b.source), std::cmp::Reverse(b.en.len()));
            if best.is_none_or(better) {
                best = Some(e);
            }
        }
        best.map(|e| (e.en.clone(), slot_text(&fill(&e.ar, &slots(9), &[])), e.source.clone()))
    }

    /// A remembered line that `part` is a piece of: GTA VI's phone after a scroll shows a message cut off at the
    /// screen's top or bottom — its end, its start, or its middle — and the memory has the whole message. Its
    /// whole English key, its Arabic (filled with no icons) and its source: the closest (its words alike, a
    /// misread letter or two allowed, as `near`), then the most trusted, then the shortest. Pieces of four
    /// words and 16 letters at the least: a shorter one is in too many lines.
    pub fn containing(&self, part: &str) -> Option<(String, String, String)> {
        let key = key_of(part);
        let words: Vec<&str> = key.split(' ').filter(|w| !w.is_empty() && !(w.starts_with('{') && w.ends_with('}'))).collect();
        let bare = |ws: &[&str]| ws.concat();
        let want = bare(&words);
        if words.len() < 4 || want.chars().count() < 16 {
            return None;
        }
        // (the lines with the piece's rarest words, most of them in common first: as `near`)
        let mut known: Vec<&Vec<String>> = {
            let mut ws = words.clone();
            ws.sort_unstable();
            ws.dedup();
            ws.iter().filter_map(|w| self.index.get(*w)).collect()
        };
        known.sort_unstable_by_key(|keys| keys.len());
        let mut shared: HashMap<&str, usize> = HashMap::new();
        for keys in known.iter().take(RARE_WORDS) {
            for k in keys.iter() {
                *shared.entry(k.as_str()).or_default() += 1;
            }
        }
        let mut cands: Vec<(&str, usize)> = shared.into_iter().filter(|(k, _)| k.len() > key.len()).collect();
        cands.sort_unstable_by(|a, b| b.1.cmp(&a.1));
        let need = if want.chars().count() >= 30 { 0.86 } else { 0.9 };
        let mut best: Option<(f64, &Entry)> = None;
        // (every line the piece fits about as well as the best: when they're different messages the piece is in
        // more than one — "Visit the app for additional perks with our business partners." ends several of an
        // app's — and which one it's a piece of can't be told: none. Taken one reading and another the next,
        // the second phone went from one message to the other)
        let mut fits: Vec<(f64, &Entry)> = Vec::new();
        for (k, _) in cands.into_iter().take(NEAR_TRIES) {
            let kw: Vec<&str> = k.split(' ').filter(|w| !(w.starts_with('{') && w.ends_with('}'))).collect();
            if kw.len() <= words.len() {
                continue;
            }
            // (the piece against each run of as many of the line's words: a word misread or split stays in place)
            let s = (0..=kw.len() - words.len()).map(|i| similarity(&bare(&kw[i..i + words.len()]), &want)).fold(0.0, f64::max);
            if s < need {
                continue;
            }
            let Some(e) = self.map.get(k) else { continue };
            fits.push((s, e));
            let better = |b: &(f64, &Entry)| (s, rank(&e.source), std::cmp::Reverse(e.en.len())) > (b.0, rank(&b.1.source), std::cmp::Reverse(b.1.en.len()));
            if best.as_ref().is_none_or(better) {
                best = Some((s, e));
            }
        }
        let (top, e) = best?;
        // (as well: a letter's difference tells them apart — "$100,000" against "$500,000")
        if fits.iter().any(|(s, o)| *s >= top - 0.005 && o.en != e.en) {
            return None;
        }
        Some((e.en.clone(), slot_text(&fill(&e.ar, &slots(9), &[])), e.source.clone()))
    }

    /// The line the memory takes `line` for (its own text, as kept: a misreading finds the line it's
    /// a misreading of), or None.
    pub fn line_of(&self, line: &str) -> Option<String> {
        let key = key_of(line);
        if key.is_empty() {
            return None;
        }
        self.find(&key).map(|e| e.en.clone())
    }

    pub fn put(&mut self, line: &str, segs: &Segments, source: &str) {
        let key = key_of(line);
        if key.is_empty() || segs.iter().all(|(t, c)| t.trim().is_empty() && c.is_none()) {
            return;
        }
        if self.map.get(&key).is_some_and(|old| rank(&old.source) > rank(source)) {
            return;
        }
        let e = Entry { en: key.clone(), ar: to_template(segs), source: source.into() };
        // (the same again adds nothing: the file only grows with what's new)
        if self.map.get(&key).is_some_and(|old| old.ar == e.ar && old.source == e.source) {
            return;
        }
        append_sealed(&self.path, &format!("memory/{}", self.lang), [serde_json::to_string(&e).unwrap_or_default()]);
        self.insert(key, e);
    }

    pub fn record(&mut self, hit: bool, ms: f64) {
        if hit {
            self.hits += 1;
            self.hit_ms += ms;
        } else {
            self.misses += 1;
            self.miss_ms += ms;
        }
    }

    pub fn stats(&self) -> Stats {
        let avg = |sum: f64, n: u64| if n == 0 { 0.0 } else { sum / n as f64 };
        Stats {
            entries: self.map.len(),
            reviewed: self.map.values().filter(|e| e.source == "reviewed").count(),
            hits: self.hits,
            misses: self.misses,
            avg_hit_ms: avg(self.hit_ms, self.hits),
            avg_miss_ms: avg(self.miss_ms, self.misses),
        }
    }

    /// Forget the live translations (the lines log stays).
    pub fn clear(&mut self) {
        let _ = std::fs::remove_file(&self.path);
        let packs = std::mem::take(&mut self.packs);
        *self = Self::load(self.dir.clone(), &self.lang);
        self.take_packs(packs);
    }

    /// Switch to another target language (its own memory, and what was downloaded for it).
    pub fn set_lang(&mut self, lang: &str) {
        if self.lang != lang {
            let (h, m, hm, mm) = (self.hits, self.misses, self.hit_ms, self.miss_ms);
            let packs = std::mem::take(&mut self.packs);
            *self = Self::load(self.dir.clone(), lang);
            (self.hits, self.misses, self.hit_ms, self.miss_ms) = (h, m, hm, mm);
            self.take_packs(packs);
        }
    }

    /// The target language this memory is for.
    pub fn lang(&self) -> &str {
        &self.lang
    }

    /// The downloads kept from before a reload: the language in use's applied again.
    fn take_packs(&mut self, mut packs: HashMap<String, Vec<Entry>>) {
        let mine = packs.remove(&self.lang);
        self.packs = packs;
        if let Some(entries) = mine {
            let lang = self.lang.clone();
            self.set_shared(&lang, entries);
        }
    }

    /// Add translations downloaded in the background: they replace the player's own lines for the
    /// same English, here and in the memory file (which then keeps only what the pack doesn't have).
    pub fn set_shared(&mut self, lang: &str, entries: Vec<Entry>) {
        // (downloaded for another language: the player switched meanwhile; kept for a switch back)
        if lang != self.lang {
            self.packs.insert(lang.to_string(), entries);
            return;
        }
        // lines the last pack had and this one doesn't (taken out on the server): not used any more
        let gone: Vec<String> = {
            let now: std::collections::HashSet<&str> = entries.iter().map(|e| e.en.as_str()).collect();
            self.packs
                .get(lang)
                .into_iter()
                .flatten()
                .filter(|e| !now.contains(e.en.as_str()))
                .filter(|e| self.map.get(&e.en).is_some_and(|m| m.source == e.source && m.ar == e.ar))
                .map(|e| e.en.clone())
                .collect()
        };
        for k in gone {
            self.remove(&k);
        }
        for e in &entries {
            if self.map.get(&e.en).is_none_or(|old| rank(&old.source) <= rank(&e.source)) {
                self.insert(e.en.clone(), e.clone());
            }
        }
        let covered: std::collections::HashSet<&str> = entries.iter().map(|e| e.en.as_str()).collect();
        self.drop_from_file(&covered);
        self.packs.insert(lang.to_string(), entries);
    }

    /// Rewrite the memory file without the lines for these English keys (if it has any).
    fn drop_from_file(&self, keys: &std::collections::HashSet<&str>) {
        let kind = format!("memory/{}", self.lang);
        let Ok(text) = std::fs::read_to_string(&self.path) else { return };
        let mut removed = 0;
        let kept: Vec<&str> = text
            .lines()
            .filter(|l| {
                let gone = secure::open(&kind, l)
                    .and_then(|j| serde_json::from_str::<Entry>(&j).ok())
                    .is_some_and(|e| keys.contains(e.en.as_str()));
                removed += gone as usize;
                !gone
            })
            .collect();
        if removed == 0 {
            return;
        }
        // written next to it, then swapped in: a crash can't leave half a file
        let tmp = self.path.with_extension("dat.tmp");
        let body: String = kept.iter().map(|l| format!("{l}\n")).collect();
        if std::fs::write(&tmp, body).is_ok() {
            let _ = std::fs::rename(&tmp, &self.path);
        }
    }
}

/// Memory files from before encryption for the other languages (memory-<lang>.jsonl): converted too,
/// so nothing stays readable on disk (the one for `lang` is converted by `load` itself).
fn convert_other_languages(dir: &std::path::Path, lang: &str) {
    let Ok(files) = std::fs::read_dir(dir) else { return };
    for f in files.flatten() {
        let name = f.file_name().to_string_lossy().to_string();
        let Some(other) = name.strip_prefix("memory-").and_then(|n| n.strip_suffix(".jsonl")) else { continue };
        if other == lang || other.is_empty() || !other.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(f.path()) else { continue };
        let lines = text.lines().filter(|l| serde_json::from_str::<Entry>(l).is_ok()).map(String::from);
        if append_sealed(&dir.join(format!("memory-{other}.dat")), &format!("memory/{other}"), lines) {
            let _ = std::fs::remove_file(f.path());
        }
    }
}

/// Append records, each sealed on its own line. False if the file couldn't be written.
fn append_sealed(path: &std::path::Path, kind: &str, records: impl IntoIterator<Item = String>) -> bool {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) else { return false };
    let body: String = records.into_iter().map(|r| secure::seal(kind, &r) + "\n").collect();
    f.write_all(body.as_bytes()).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        std::env::temp_dir().join(format!("vi-translator-mem-{}", std::process::id()))
    }
    fn text(segs: &Segments) -> String {
        segs.iter().map(|(t, c)| match c.as_deref() {
            Some(c) if c.starts_with("icon:") => format!("[{}]", &c[5..]),
            Some(c) => format!("<{t}|{c}>"),
            None => t.clone(),
        }).collect()
    }

    #[test]
    fn files_are_encrypted_old_ones_converted_and_shared_lines_replace_local_ones() {
        let path = std::env::temp_dir().join(format!("vi-translator-enc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        // a memory file from before encryption
        std::fs::write(
            path.join("memory.jsonl"),
            "{\"en\":\"take cover\",\"ar\":\"احتمِ\",\"source\":\"local\"}\n{\"en\":\"follow me\",\"ar\":\"اتبعني\",\"source\":\"local\"}\n",
        )
        .unwrap();
        let mut m = Memory::load(path.clone(), "ar");
        assert!(!path.join("memory.jsonl").exists(), "the plain file is converted, then removed");
        let dat = std::fs::read_to_string(path.join("memory-ar.dat")).unwrap();
        assert!(!dat.contains("take cover") && !dat.contains("احتمِ"), "nothing readable on disk");
        assert!(m.get("Take cover!", &[], &[]).is_some(), "still remembered");
        m.put("Get in the car.", &vec![("اركب السيارة.".into(), None)], "local");
        // after a restart: the same lines, read back from the encrypted file
        let mut m = Memory::load(path.clone(), "ar");
        assert_eq!(m.get("get in the car", &[], &[]).unwrap().0[0].0, "اركب السيارة.");
        // a shared pack with "take cover": it wins, and the local line leaves the file
        m.set_shared("ar", vec![Entry { en: "take cover".into(), ar: "احتمِ بساتر!".into(), source: "reviewed".into() }]);
        assert_eq!(m.get("take cover", &[], &[]).unwrap().1, "reviewed");
        let m2 = Memory::load(path.clone(), "ar");
        assert!(m2.get("take cover", &[], &[]).is_none(), "the player's own line for it is gone");
        assert!(m2.get("follow me", &[], &[]).is_some(), "lines the pack doesn't have stay");
        // a pack for another language (the player switched languages meanwhile): not taken
        let mut m3 = Memory::load(path.clone(), "ar");
        m3.set_shared("ar", vec![Entry { en: "take cover".into(), ar: "احتمِ بساتر!".into(), source: "reviewed".into() }]);
        m3.set_shared("hi", vec![Entry { en: "follow me".into(), ar: "मेरे पीछे आओ".into(), source: "reviewed".into() }]);
        assert_ne!(m3.get("follow me", &[], &[]).unwrap().1, "reviewed");
        // a line taken out of the pack on the server: not used any more
        m3.set_shared("ar", vec![]);
        assert!(m3.get("take cover", &[], &[]).is_none());
        // switching languages and back (before the next download): the download is used again
        m3.set_shared("ar", vec![Entry { en: "take cover".into(), ar: "احتمِ بساتر!".into(), source: "reviewed".into() }]);
        m3.set_lang("hi");
        assert_eq!(m3.lang(), "hi");
        assert_eq!(m3.get("follow me", &[], &[]).unwrap().1, "reviewed", "hi's own download, kept from before");
        assert!(m3.get("take cover", &[], &[]).is_none());
        m3.set_lang("ar");
        assert_eq!(m3.get("take cover", &[], &[]).unwrap().1, "reviewed");
        let _ = std::fs::remove_dir_all(&path);
    }

    #[test]
    fn remembers_with_new_icons_and_colours_and_survives_restart() {
        let path = tmp();
        let _ = std::fs::remove_dir_all(&path);
        let mut m = Memory::load(path.clone(), "ar");
        let segs: Segments = vec![
            ("اذهب إلى ".into(), None),
            ("البنك".into(), Some("#67caff".into())),
            (" واضغط ".into(), None),
            (String::new(), Some("icon:0.8|OLD".into())),
        ];
        m.put("Go to the bank and press ⟦1⟧.", &segs, "local");
        // same line later: different punctuation/case, a new icon image, a new highlight colour
        let got = m.get("go to the bank and press ⟦1⟧", &["0.9|NEW".into()], &[("bank".into(), "#ffd455".into())]).unwrap().0;
        assert_eq!(text(&got), "اذهب إلى <البنك|#ffd455> واضغط [0.9|NEW]");
        // reloaded from disk
        let m2 = Memory::load(path.clone(), "ar");
        assert!(m2.knows("Go to the bank and press ⟦1⟧!"));
        assert_eq!(key_of(&key_of("Press ⟦1⟧ ⟦2⟧ to zoom.")), "press {1} {2} to zoom");
        // (the server's KeyNormalizerTest has the same cases)
        assert_eq!(key_of("Go\u{a0}to the car."), "go to the car");
        assert_eq!(key_of("Press\u{a0}\u{27e6}1\u{27e7} to jump"), "press {1} to jump");
        assert_eq!(key_of("1\u{bd} miles"), "1\u{bd} miles");
        assert_eq!(key_of("E=mc\u{b2}"), "emc\u{b2}");
        assert_eq!(key_of("Chapter \u{2165}"), "chapter \u{2175}");
        assert_eq!(key_of("\u{2003}Lucia\u{2003}"), "lucia");
        assert_eq!(key_of("\u{130}stanbul"), "istanbul");
        assert_eq!(key_of(&key_of("\u{130}stanbul")), key_of("\u{130}stanbul"));
        // icons stuck to text are words of their own
        assert_eq!(key_of("Distance: \u{27e6}1\u{27e7}m"), "distance {1} m");
        assert_eq!(key_of("Trackers (\u{27e6}1\u{27e7}/\u{27e6}2\u{27e7}) installed."), "trackers {1} {2} installed");
        assert_eq!(key_of("Love ya. {1}{2}{3}"), "love ya {1} {2} {3}");
        assert_eq!(key_of("{1}% done"), "{1} done");
        assert_eq!(key_of(&key_of("Love ya. {1}{2}{3}")), key_of("Love ya. {1}{2}{3}"));
        assert_eq!(key_of("a {b} {12x} c"), "a b 12x c");
        let _ = std::fs::remove_dir_all(&path);
    }

    #[test]
    fn a_piece_of_a_line_finds_the_whole_line() {
        let path = tmp().with_extension("piece");
        let _ = std::fs::remove_dir_all(&path);
        let mut m = Memory::load(path.clone(), "ar");
        let whole = "Hey Mo, it's Lucia and Jason here, had to get out of PG. Do you know anyone who can handle cars in Vice?";
        m.put(whole, &vec![("مرحبًا مو، معك لوسيا وجيسون.".into(), None)], "reviewed");
        m.put("Aside from that, there's a group that races down at Buckskin Swamp.", &vec![("غير ذلك".into(), None)], "reviewed");
        let found = |m: &Memory, part: &str| m.containing(part).map(|(en, ar, _)| (en, ar));
        // (its end, cut at the screen's top; its start, cut at the bottom; its middle, both)
        for part in ["who can handle cars in Vice?", "Hey Mo, it's Lucia and Jason here,", "had to get out of PG. Do you know"] {
            assert_eq!(found(&m, part).map(|f| f.1), Some("مرحبًا مو، معك لوسيا وجيسون.".to_string()), "{part}");
        }
        assert_eq!(found(&m, "who can handel cars in Vice?").map(|f| f.0), Some(key_of(whole)), "a misread letter");
        // (too short to be sure of; words the line doesn't have; the whole line itself is `get`'s)
        assert!(found(&m, "cars in Vice?").is_none());
        assert!(found(&m, "who can fix bikes in Vice?").is_none());
        assert!(found(&m, whole).is_none());
        // (a piece two messages end with: which one it's a piece of can't be told)
        m.put("Your account limit is $100,000. Visit the app for additional perks with our business partners.", &vec![("حدّك 100,000".into(), None)], "reviewed");
        m.put("You're gold now: your limit is $500,000. Visit the app for additional perks with our business partners.", &vec![("حدّك 500,000".into(), None)], "reviewed");
        assert!(found(&m, "Visit the app for additional perks with our business partners.").is_none());
        assert_eq!(found(&m, "limit is $100,000. Visit the app for additional perks").map(|f| f.1), Some("حدّك 100,000".to_string()), "told apart by what's read of it");
        let _ = std::fs::remove_dir_all(&path);
    }

    #[test]
    fn small_ocr_slips_hit_but_different_lines_dont() {
        let path = tmp().with_extension("2");
        let _ = std::fs::remove_dir_all(&path);
        let mut m = Memory::load(path.clone(), "ar");
        m.put("Blame the pricks who called you out here!", &vec![("ألقِ باللوم على الأوغاد الذين استدعوك إلى هنا!".into(), None)], "local");
        assert!(m.knows("Blame the pricks who caled you out here"));
        assert!(!m.knows("Blame the cops who called you out here"));
        let _ = std::fs::remove_dir_all(&path);
    }

    #[test]
    fn better_sources_are_not_overwritten() {
        let path = tmp().with_extension("3");
        let _ = std::fs::remove_dir_all(&path);
        let mut m = Memory::load(path.clone(), "ar");
        m.put("Take cover.", &vec![("احتمِ.".into(), None)], "claude");
        m.put("Take cover.", &vec![("خذ الغطاء".into(), None)], "local");
        assert_eq!(text(&m.get("Take cover.", &[], &[]).unwrap().0), "احتمِ.");
        // a reviewed line wins over everything and survives clearing the memory
        std::fs::write(path.join("reviewed.jsonl"), "{\"en\":\"Take cover!\",\"ar\":\"احتمِ بساتر!\",\"source\":\"x\"}\n").unwrap();
        let mut m = Memory::load(path.clone(), "ar");
        let (segs, src) = m.get("take cover", &[], &[]).unwrap();
        assert_eq!((text(&segs).as_str(), src.as_str()), ("احتمِ بساتر!", "reviewed"));
        m.clear();
        assert!(m.knows("Take cover."));
        let _ = std::fs::remove_dir_all(&path);
    }

    #[test]
    fn a_cut_off_line_finds_the_whole_one() {
        let path = tmp().with_extension("4");
        let _ = std::fs::remove_dir_all(&path);
        let mut m = Memory::load(path.clone(), "ar");
        m.put("Me again. Can't wait to get out of here.", &vec![("أنا مجددًا. لا أطيق صبرًا للخروج من هنا.".into(), None)], "claude");
        m.put("Me again. Can't wait to see you.", &vec![("أنا مجددًا. لا أطيق صبرًا لرؤيتك.".into(), None)], "local");
        let (en, ar, _) = m.starting_with("Me again. Can't wait to ge...").unwrap();
        assert_eq!((en.as_str(), ar.as_str()), ("me again can't wait to get out of here", "أنا مجددًا. لا أطيق صبرًا للخروج من هنا."));
        // (too short a start to tell)
        assert!(m.starting_with("Me ag...").is_none());
        let _ = std::fs::remove_dir_all(&path);
    }
}

#[cfg(test)]
mod perf {
    use super::*;

    #[test]
    fn lookup_stays_fast_with_thousands_of_lines() {
        let path = std::env::temp_dir().join(format!("vi-translator-mem-perf-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        let mut m = Memory::load(path.clone(), "ar");
        for i in 0..5000 {
            let en = format!("This is game line number {i} about the heist and the cops near the bank");
            m.insert(key_of(&en), Entry { en: key_of(&en), ar: "نص".into(), source: "local".into() });
        }
        let t = std::time::Instant::now();
        let hit = m.knows("This is game line number 4321 about the heist and the cops near the bank");
        let exact_ms = t.elapsed().as_secs_f64() * 1000.0;
        let t = std::time::Instant::now();
        let miss = m.knows("A completely different sentence that is not stored anywhere at all here");
        let miss_ms = t.elapsed().as_secs_f64() * 1000.0;
        let t = std::time::Instant::now();
        let near = m.knows("This is game line number 4321 about the heist and the cops near the bnk");
        let near_ms = t.elapsed().as_secs_f64() * 1000.0;
        println!("5000 entries: exact hit {exact_ms:.2} ms, miss {miss_ms:.2} ms, near hit {near_ms:.2} ms");
        assert!(hit && !miss && near);
        let _ = std::fs::remove_dir_all(&path);
    }
}
