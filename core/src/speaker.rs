//! Speaker names in front of subtitles ("Dobbs: Not to be annoying, can you fix my leg?"), as GTA VI
//! shows them when its "speaker name" option is on: the name in a colour of its own, then a colon.
//!
//! Only the sentence is translated (and remembered), so the same line finds the same translation
//! with or without the name. The name is written in the target language's letters where it uses the
//! Arabic script (a reviewed name from the downloaded translations first, then the game's known
//! names, then letter by letter), and drawn back in front in the game's colour.

use crate::translate::Segments;

/// The segment colour marking a speaker name: "name:#rrggbb" (always drawn in that colour, also with
/// highlight colours turned off, like the grey of the other speaker's line).
pub const NAME_PREFIX: &str = "name:";

/// A line starting with a speaker name: (name, its colour, the rest). The name must be coloured in the
/// game (a highlighted phrase starting with it): a white "Tip:" or "Note:" is part of the text.
pub fn split<'a>(en: &'a str, phrases: &[(String, String)]) -> Option<(String, String, &'a str)> {
    let Some((tagged, rest)) = crate::ocr::split_name(en) else { return split_lost_colon(en, phrases) };
    // the phrase is the name, or a highlighted run that starts with it. Compared without the colon
    // (and the dots OCR reads next to it): the game colours the name, and its colour may stop before
    // the colon ("Andrew" for "Andrew:"), which as a whole line went to the translator, the colon
    // coming back as a bracket ("أندرو)") and the name uncoloured.
    let name = bare_name(&tagged);
    let word = |w: &str| w.trim_end_matches(|c: char| matches!(c, ':' | '.' | '…' | ',' | ';')).to_string();
    let first = word(name.split_whitespace().next()?);
    let color = phrases
        .iter()
        .find(|(p, _)| {
            let p = p.trim();
            p.starts_with(tagged.as_str()) || p.starts_with(name.as_str()) || (p.split_whitespace().next().map(word).as_deref() == Some(first.as_str()) && name.starts_with(word(p).as_str()))
        })?
        .1
        .clone();
    if rest.chars().filter(|c| c.is_alphabetic()).count() < 2 {
        return None;
    }
    Some((bare_name(&tagged), color, rest))
}

/// The OCR lost the colon after a coloured name ("Local. What is wrong with you?", "Local What is…":
/// the name, read on its own, is paler than the line): a highlighted phrase at the start that is one
/// of the game's speakers is the name, when the line goes on after it.
fn split_lost_colon<'a>(en: &'a str, phrases: &[(String, String)]) -> Option<(String, String, &'a str)> {
    let (p, color) = phrases.iter().find(|(p, _)| {
        let name = bare_name(p.trim());
        !name.is_empty() && en.starts_with(name.as_str()) && is_known(&name)
    })?;
    let name = bare_name(p.trim());
    let after = &en[name.len()..];
    // (the name's end: a dot, a colon or a space, not a longer word "Locally")
    if !after.starts_with(|c: char| c.is_whitespace() || matches!(c, '.' | ':' | '…' | ',')) {
        return None;
    }
    let rest = after.trim_start_matches(|c: char| matches!(c, '.' | ':' | '…' | ',')).trim_start();
    if rest.chars().filter(|c| c.is_alphabetic()).count() < 2 {
        return None;
    }
    Some((name, color.clone(), rest))
}

/// "Lucia.:" / "Jason…:" -> "Lucia" / "Jason": the name without its colon or the dots the OCR reads
/// next to it.
fn bare_name(tagged: &str) -> String {
    tagged.trim_end_matches(':').trim_end_matches(|c: char| matches!(c, '.' | '…' | ',' | ';')).trim().to_string()
}

/// A "Name:" in front that the colour check missed (a pale name, or one over a bright sky): (name,
/// the rest). The caller takes it only for a name it knows (the game's people, a reviewed name, or a
/// sentence it knows without it), so a white "Tip:" stays part of the text. Never a language tag.
pub fn plain_name(en: &str) -> Option<(String, &str)> {
    if language_tag(en).is_some() {
        return None;
    }
    let (tagged, rest) = crate::ocr::split_name(en)?;
    if rest.chars().filter(|c| c.is_alphabetic()).count() < 2 {
        return None;
    }
    Some((bare_name(&tagged), rest))
}

/// One of the game's people (as KNOWN lists them, or the list of the game's speakers).
pub fn is_known(name: &str) -> bool {
    let low = name.to_lowercase();
    KNOWN.iter().any(|(en, _)| *en == low) || listed_ar(&low).is_some()
}

/// The game's speakers as an Arabic subtitle names them, reviewed: speaker_names_ar.tsv (English,
/// a tab, the Arabic; the names in the game's text, with the roles translated: "The Man" is
/// "الرجل"). Ahead of the downloaded translations, whose line of the same text is often a line of
/// dialogue ("Mother..."), not the name.
static NAMES_AR: std::sync::LazyLock<std::collections::HashMap<String, &'static str>> = std::sync::LazyLock::new(|| {
    include_str!("speaker_names_ar.tsv")
        .lines()
        .filter_map(|l| l.split_once('\t'))
        .map(|(en, ar)| (en.trim().to_lowercase(), ar.trim()))
        .filter(|(en, ar)| !en.is_empty() && !ar.is_empty())
        .collect()
});

/// A speaker of the game in Arabic, from the list (`low`: the name in lower case).
fn listed_ar(low: &str) -> Option<&'static str> {
    NAMES_AR.get(low).copied()
}

/// The colour a name the colour check missed is drawn in: the subtitles' own near-white.
pub const PLAIN_NAME_COLOR: &str = "#f2f2f2";

/// The line without a "Name:" in front (colour not known yet: for the memory check only).
pub fn without_name(en: &str) -> Option<&str> {
    let rest = crate::ocr::split_name(en).map(|(_, rest)| rest).unwrap_or(en);
    let rest = language_tag(rest).map(|(_, s)| s).unwrap_or(rest);
    (rest.len() < en.len()).then_some(rest)
}

/// The sentence alone, for the memory check and the log: no cues, no name, no "In Spanish:".
pub fn sentence_of(en: &str) -> String {
    let clean = take_cues(en).map(|(_, t)| t).unwrap_or_else(|| en.to_string());
    without_name(&clean).unwrap_or(&clean).to_string()
}

/// The highlighted phrases of the sentence without the name (the name is not a highlighted word in it).
pub fn phrases_without(phrases: &[(String, String)], name: &str) -> Vec<(String, String)> {
    let tag = format!("{name}:");
    phrases
        .iter()
        .filter_map(|(p, c)| {
            let rest = p.strip_prefix(&tag).map(str::trim).unwrap_or(p);
            (!rest.is_empty()).then(|| (rest.to_string(), c.clone()))
        })
        .collect()
}

/// Languages written in the Arabic script: the name is written in it. Others keep the game's name.
fn arabic_script(lang: &str) -> bool {
    matches!(lang, "ar" | "fa" | "ur")
}

/// The game's people, as their names are usually written in Arabic.
const KNOWN: &[(&str, &str)] = &[
    ("lucia", "لوسيا"),
    ("jason", "جيسون"),
    ("cal", "كال"),
    ("boobie", "بوبي"),
    ("dre'quan", "دريكوان"),
    ("drequan", "دريكوان"),
    ("brian", "براين"),
    ("raul", "راؤول"),
    ("bae-luxe", "بي-لوكس"),
    ("roxy", "روكسي"),
    ("real dimez", "ريال دايمز"),
    ("mike", "مايك"),
    ("michael", "مايكل"),
    ("trevor", "تريفور"),
    ("franklin", "فرانكلين"),
    ("lamar", "لامار"),
];

/// Roles the game puts where a name goes ("Cop:", "Prison Guard:"): translated, never spelt out in
/// Arabic letters ("كوب"). Arabic, with the article, as the subtitle names a person.
const ROLES: &[(&str, &str)] = &[
    ("cop", "الشرطي"),
    ("cops", "الشرطة"),
    ("police", "الشرطة"),
    ("police officer", "ضابط الشرطة"),
    ("officer", "الضابط"),
    ("deputy", "نائب الشريف"),
    ("sheriff", "الشريف"),
    ("trooper", "شرطي الولاية"),
    ("detective", "المحقق"),
    ("sergeant", "الرقيب"),
    ("captain", "النقيب"),
    ("agent", "العميل"),
    ("soldier", "الجندي"),
    ("guard", "الحارس"),
    ("prison guard", "حارس السجن"),
    ("security guard", "حارس الأمن"),
    ("security", "الأمن"),
    ("bouncer", "حارس الملهى"),
    ("dispatch", "مركز البلاغات"),
    ("dispatcher", "مركز البلاغات"),
    ("operator", "عامل الهاتف"),
    ("pilot", "الطيار"),
    ("driver", "السائق"),
    ("cashier", "أمين الصندوق"),
    ("clerk", "الموظف"),
    ("teller", "الصرّاف"),
    ("bank teller", "صرّاف البنك"),
    ("bartender", "النادل"),
    ("waiter", "النادل"),
    ("waitress", "النادلة"),
    ("customer", "الزبون"),
    ("shopkeeper", "صاحب المتجر"),
    ("owner", "المالك"),
    ("manager", "المدير"),
    ("boss", "الزعيم"),
    ("worker", "العامل"),
    ("mechanic", "الميكانيكي"),
    ("attendant", "العامل"),
    ("salesman", "البائع"),
    ("dealer", "التاجر"),
    ("doctor", "الطبيب"),
    ("nurse", "الممرضة"),
    ("paramedic", "المسعف"),
    ("reporter", "المراسل"),
    ("news anchor", "مذيع الأخبار"),
    ("anchor", "المذيع"),
    ("announcer", "المذيع"),
    ("host", "المقدّم"),
    ("radio host", "مذيع الراديو"),
    ("dj", "منسّق الأغاني"),
    ("caller", "المتصل"),
    ("voice", "الصوت"),
    ("man", "الرجل"),
    ("woman", "المرأة"),
    ("guy", "الرجل"),
    ("girl", "الفتاة"),
    ("boy", "الفتى"),
    ("kid", "الطفل"),
    ("old man", "العجوز"),
    ("old woman", "العجوز"),
    ("homeless man", "المشرّد"),
    ("pedestrian", "أحد المارة"),
    ("passerby", "أحد المارة"),
    ("local", "أحد السكان"),
    ("neighbor", "الجار"),
    ("neighbour", "الجار"),
    ("tourist", "السائح"),
    ("stranger", "الغريب"),
    ("hostage", "الرهينة"),
    ("inmate", "السجين"),
    ("prisoner", "السجين"),
    ("thug", "البلطجي"),
    ("goon", "البلطجي"),
    ("gangster", "رجل العصابة"),
    ("gang member", "عضو العصابة"),
    ("biker", "سائق الدراجة"),
    ("crowd", "الحشد"),
    ("everyone", "الجميع"),
    ("all", "الجميع"),
];

/// A role in place of a name ("Cop", "Prison Guard"), or a title before one ("Officer Smith").
pub fn is_role(name: &str) -> bool {
    let low = name.to_lowercase();
    let first = low.split_whitespace().next().unwrap_or("");
    ROLES.iter().any(|(en, _)| *en == low || *en == first)
}

/// A role (or a title and a name) in Arabic: "Cop" -> "الشرطي", "Officer Smith" -> "الضابط سميث".
fn role_ar(name: &str) -> Option<String> {
    let low = name.to_lowercase();
    // "The Man": the role ("ذ مان" was "The" and "Man" spelt out)
    let low = low.strip_prefix("the ").unwrap_or(&low).to_string();
    if let Some((_, ar)) = ROLES.iter().find(|(en, _)| *en == low) {
        return Some(ar.to_string());
    }
    // "Guard 2": the role, then its number
    if let Some((role, n)) = low.rsplit_once(' ').filter(|(_, n)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit())) {
        if let Some(ar) = role_ar(role) {
            return Some(format!("{ar} {n}"));
        }
    }
    let (first, rest) = low.split_once(' ')?;
    let (_, title) = ROLES.iter().find(|(en, _)| *en == first)?;
    let person = KNOWN.iter().find(|(en, _)| *en == rest).map(|(_, ar)| ar.to_string()).unwrap_or_else(|| rest.split('-').map(transliterate).collect::<Vec<_>>().join("-"));
    Some(format!("{title} {person}"))
}

/// The name as the target language writes it. `reviewed`: a reviewed translation of the name alone
/// (the downloaded translations can carry the names the game's people go by).
pub fn name_in(lang: &str, name: &str, reviewed: Option<String>) -> String {
    // (no English letter in it: written already. A page read again kept its line, its name with it, and
    // "بي-لوكس" was spelled again letter by letter: the Arabic ones dropped, "-" left)
    if !arabic_script(lang) || !name.chars().any(|c| c.is_ascii_alphabetic()) {
        return name.to_string();
    }
    let low = name.to_lowercase();
    if let Some((_, ar)) = KNOWN.iter().find(|(en, _)| *en == low) {
        return ar.to_string();
    }
    if lang == "ar" {
        if let Some(ar) = listed_ar(&low) {
            return ar.to_string();
        }
        if let Some(r) = role_ar(name) {
            return r;
        }
    }
    // a reviewed translation of the name alone: the downloaded line "Lucia." comes with its full stop
    // ("لوسيا."), and only a name-sized one is a name (not a sentence that happens to match)
    let words = name.split_whitespace().count();
    let reviewed = reviewed.map(|r| r.trim().trim_end_matches(|c: char| matches!(c, ':' | '.' | '…' | '!' | '?' | '،' | '؟' | ',' | '؛')).trim().to_string());
    if let Some(r) = reviewed.filter(|r| !r.is_empty() && r.split_whitespace().count() <= words + 1 && !r.contains(|c: char| matches!(c, '.' | '!' | '?' | '؟' | '،'))) {
        return r;
    }
    // each word on its own ("Officer Smith"); a word that is one of the known names is written as they are,
    // and an English "the" is left out (never "ذ")
    low.split(' ')
        .filter(|word| !word.is_empty() && *word != "the")
        .map(|word| {
            KNOWN
                .iter()
                .find(|(en, _)| *en == word)
                .map(|(_, ar)| ar.to_string())
                .unwrap_or_else(|| word.split('-').map(transliterate).collect::<Vec<_>>().join("-"))
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn is_vowel(c: char) -> bool {
    matches!(c, 'a' | 'e' | 'i' | 'o' | 'u' | 'y')
}

/// English letters -> Arabic letters, by sound ("dobbs" -> "دوبس", "lucia" -> "لوسيا").
pub fn transliterate(word: &str) -> String {
    let w: Vec<char> = word.chars().filter(|c| c.is_ascii_alphabetic()).map(|c| c.to_ascii_lowercase()).collect();
    let n = w.len();
    let soft = |i: usize| i < n && matches!(w[i], 'e' | 'i' | 'y');
    // a "magic e" ("Kyle", "Mike", "Kate", "Jones"): the vowel before the last consonant is long
    let magic = n >= 4 && w[n - 1] == 'e' && !is_vowel(w[n - 2]) && is_vowel(w[n - 3]) && (n < 4 || !is_vowel(w[n - 4]));
    let mut out = String::new();
    let mut i = 0;
    while i < n {
        let c = w[i];
        let next = if i + 1 < n { w[i + 1] } else { ' ' };
        // a doubled consonant is one sound
        if i > 0 && c == w[i - 1] && !matches!(c, 'a' | 'e' | 'i' | 'o' | 'u') {
            i += 1;
            continue;
        }
        let two: String = [c, next].iter().collect();
        let digraph = match two.as_str() {
            "sh" => Some("ش"),
            "ch" => Some("تش"),
            "th" => Some("ث"),
            "ph" => Some("ف"),
            "kh" => Some("خ"),
            "gh" => Some("غ"),
            "ck" => Some("ك"),
            "qu" => Some("كو"),
            "oo" | "ou" => Some("و"),
            "ee" | "ea" | "ie" => Some(if i == 0 { "إي" } else { "ي" }),
            "ai" | "ay" => Some(if i == 0 { "اي" } else { "اي" }),
            _ => None,
        };
        if let Some(d) = digraph {
            out.push_str(d);
            i += 2;
            continue;
        }
        let last = i + 1 == n;
        if magic && i == n - 3 {
            out.push_str(match c {
                'i' | 'y' => "اي",
                'a' => "اي",
                'o' => "و",
                _ => "و",
            });
            i += 1;
            continue;
        }
        let s = match c {
            'a' => if i == 0 { "ا" } else { "ا" },
            'e' => {
                if i == 0 {
                    "إ"
                } else if last {
                    "" // a silent final e ("Mike")
                } else {
                    "ي"
                }
            }
            'i' => if i == 0 { "إي" } else { "ي" },
            'o' => if i == 0 { "أو" } else { "و" },
            'u' => if i == 0 { "أو" } else { "و" },
            'y' => "ي",
            'b' => "ب",
            'c' => if soft(i + 1) { "س" } else { "ك" },
            'd' => "د",
            'f' => "ف",
            'g' => if soft(i + 1) { "ج" } else { "غ" },
            'h' => "ه",
            'j' => "ج",
            'k' => "ك",
            'l' => "ل",
            'm' => "م",
            'n' => "ن",
            'p' => "ب",
            'q' => "ك",
            'r' => "ر",
            's' => "س",
            't' => "ت",
            'v' => "ف",
            'w' => "و",
            'x' => "كس",
            'z' => "ز",
            _ => "",
        };
        out.push_str(s);
        i += 1;
    }
    // a medial short "e" written as ي between two consonants reads oddly long; keep it (it helps the
    // reader say the name) but never two ي in a row
    out.replace("يي", "ي")
}

/// The name in front of the translated sentence: "دوبس:" in the game's colour, then a space.
pub fn with_name(name_shown: &str, color: &str, mut segs: Segments) -> Segments {
    let mut out: Segments = vec![(format!("{name_shown}:"), Some(format!("{NAME_PREFIX}{color}"))), (" ".into(), None)];
    out.append(&mut segs);
    out
}

// --------------------------------------------------------------------------------- sound cues

/// Sound cues in square brackets, as GTA VI shows them with its sound subtitles on: "[rattling]" on a
/// line of its own or in front, "Donnie: [sigh] Fuck.", "Donnie: [laughs]". Kept apart like a speaker
/// name: only the sentence is translated and remembered, so the same line finds the same translation
/// with or without a cue; each cue is translated on its own and put back where it was.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Cues {
    /// In front of everything ("[rattling] Donnie: …").
    pub before: Vec<String>,
    /// Right after the speaker name ("Donnie: [sigh] Fuck.").
    pub after_name: Vec<String>,
    /// Anywhere else: at the end, or inside the sentence.
    pub after: Vec<String>,
}

impl Cues {
    pub fn is_empty(&self) -> bool {
        self.before.is_empty() && self.after_name.is_empty() && self.after.is_empty()
    }
}

/// The colour cues are drawn in: a soft grey, so they read as sounds, not speech.
pub const CUE_COLOR: &str = "#b9b9b9";

/// The line without its cues ("[sigh]" out of "Donnie: [sigh] Fuck." -> "Donnie: Fuck."), and the cues by
/// where they were. None when it has none.
pub fn take_cues(en: &str) -> Option<(Cues, String)> {
    let mut cues = Cues::default();
    let mut text = String::new();
    let mut rest = en;
    while let Some(open) = rest.find('[') {
        let Some(len) = rest[open..].find(']') else { break };
        let inner = rest[open + 1..open + len].trim();
        // (a cue is a few words of letters: anything else in brackets stays text)
        let is_cue = (1..=40).contains(&inner.chars().count())
            && inner.chars().any(|c| c.is_alphabetic())
            && inner.chars().all(|c| c.is_alphabetic() || c == ' ' || c == '-' || c == '\'' || c == ',');
        if !is_cue {
            text.push_str(&rest[..open + len + 1]);
            rest = &rest[open + len + 1..];
            continue;
        }
        text.push_str(&rest[..open]);
        let before = text.trim();
        let tokens: Vec<&str> = before.split_whitespace().collect();
        if before.is_empty() {
            cues.before.push(inner.to_string());
        } else if crate::ocr::name_len(&tokens) == Some(tokens.len()) {
            cues.after_name.push(inner.to_string());
        } else {
            cues.after.push(inner.to_string());
        }
        text.push(' ');
        rest = &rest[open + len + 1..];
    }
    text.push_str(rest);
    if cues.is_empty() {
        return None;
    }
    Some((cues, text.split_whitespace().collect::<Vec<_>>().join(" ")))
}

/// Common cues as Arabic writes them (others go to the translator, which remembers them too).
const CUES_AR: &[(&str, &str)] = &[
    ("laughs", "يضحك"),
    ("laughing", "يضحك"),
    ("laughter", "ضحك"),
    ("chuckles", "يضحك بخفة"),
    ("chuckling", "يضحك بخفة"),
    ("giggles", "يقهقه"),
    ("sigh", "يتنهّد"),
    ("sighs", "يتنهّد"),
    ("gasp", "يشهق"),
    ("gasps", "يشهق"),
    ("groans", "يتأوّه"),
    ("groaning", "يتأوّه"),
    ("grunts", "يهمهم"),
    ("grunting", "يهمهم"),
    ("screams", "يصرخ"),
    ("screaming", "صراخ"),
    ("shouts", "يصيح"),
    ("shouting", "صياح"),
    ("yells", "يصيح"),
    ("coughs", "يسعل"),
    ("coughing", "سعال"),
    ("cries", "يبكي"),
    ("crying", "بكاء"),
    ("sobbing", "نحيب"),
    ("panting", "يلهث"),
    ("whispers", "يهمس"),
    ("whispering", "همس"),
    ("scoffs", "يسخر"),
    ("sniffs", "يستنشق"),
    ("clears throat", "يتنحنح"),
    ("exhales", "يزفر"),
    ("inhales", "يستنشق"),
    ("mumbles", "يتمتم"),
    ("mumbling", "تمتمة"),
    ("whistles", "يصفّر"),
    ("rattling", "قعقعة"),
    ("music playing", "موسيقى"),
    ("music", "موسيقى"),
    ("phone ringing", "رنين الهاتف"),
    ("phone buzzing", "اهتزاز الهاتف"),
    ("phone vibrates", "اهتزاز الهاتف"),
    ("siren wailing", "صفّارة إنذار"),
    ("sirens wailing", "صفّارات إنذار"),
    ("sirens", "صفّارات إنذار"),
    ("alarm blaring", "إنذار يدوّي"),
    ("gunshot", "إطلاق نار"),
    ("gunshots", "إطلاق نار"),
    ("gunfire", "إطلاق نار"),
    ("explosion", "انفجار"),
    ("engine revving", "هدير المحرك"),
    ("tires screeching", "صرير الإطارات"),
    ("tyres screeching", "صرير الإطارات"),
    ("door opens", "يُفتح الباب"),
    ("door closes", "يُغلق الباب"),
    ("knocking", "طرق"),
    ("footsteps", "خطوات"),
    ("crowd cheering", "هتاف الحشد"),
    ("cheering", "هتاف"),
    ("applause", "تصفيق"),
    ("dog barking", "نباح كلب"),
    ("beeping", "صفير"),
    ("static", "تشويش"),
    ("indistinct chatter", "أحاديث غير واضحة"),
    ("indistinct", "غير واضح"),
    ("slap", "صفعة"),
    ("thud", "ارتطام"),
    ("crash", "تحطّم"),
    ("muffled", "مكتوم"),
];

/// A cue in the target language from the built-in list (Arabic), or None (the translator then).
pub fn cue_in(lang: &str, cue: &str) -> Option<String> {
    if lang != "ar" {
        return None;
    }
    let low = cue.trim().trim_end_matches(|c: char| matches!(c, '.' | ',' | '!' | '?')).to_lowercase();
    CUES_AR.iter().find(|(en, _)| *en == low).map(|(_, ar)| ar.to_string())
}

/// One cue as drawn: in brackets, in the cue colour.
fn cue_seg(shown: &str) -> (String, Option<String>) {
    (format!("[{}]", shown.trim()), Some(format!("{NAME_PREFIX}{CUE_COLOR}")))
}

/// The cues put back around a translated line ([`with_name`] gave it its name first, `named` says
/// so): in front, right after the name, and at the end. `shown` = each cue as the target language
/// writes it, in the order of `before`, `after_name`, `after`.
pub fn with_cues(cues: &Cues, shown: &[String], named: bool, segs: Segments) -> Segments {
    let (b, n) = (cues.before.len(), cues.after_name.len());
    let mut out: Segments = Vec::new();
    for s in &shown[..b] {
        out.push(cue_seg(s));
        out.push((" ".into(), None));
    }
    let mut segs = segs.into_iter();
    if named && n > 0 {
        // the name and the space after it, then the cues
        if let Some(first) = segs.next() {
            out.push(first);
        }
        if let Some(space) = segs.next() {
            out.push(space);
        }
        for s in &shown[b..b + n] {
            out.push(cue_seg(s));
            out.push((" ".into(), None));
        }
    } else {
        for s in &shown[b..b + n] {
            out.push(cue_seg(s));
            out.push((" ".into(), None));
        }
    }
    out.extend(segs);
    for s in &shown[b + n..] {
        out.push((" ".into(), None));
        out.push(cue_seg(s));
    }
    // (a cue-only line: no space left dangling)
    while out.last().is_some_and(|(t, c)| c.is_none() && t.trim().is_empty()) {
        out.pop();
    }
    out
}

// ------------------------------------------------------------------------------- language tags

/// Languages the game's people speak in, as GTA VI tags the line ("In Spanish: …"), and how Arabic
/// says "in Spanish".
const LANGUAGES: &[(&str, &str)] = &[
    ("Spanish", "بالإسبانية"),
    ("Italian", "بالإيطالية"),
    ("French", "بالفرنسية"),
    ("Portuguese", "بالبرتغالية"),
    ("Russian", "بالروسية"),
    ("Chinese", "بالصينية"),
    ("Mandarin", "بالماندرين"),
    ("Cantonese", "بالكانتونية"),
    ("Japanese", "باليابانية"),
    ("Korean", "بالكورية"),
    ("German", "بالألمانية"),
    ("Creole", "بالكريولية"),
    ("Haitian Creole", "بالكريولية الهايتية"),
    ("Arabic", "بالعربية"),
    ("Hindi", "بالهندية"),
    ("Vietnamese", "بالفيتنامية"),
    ("Tagalog", "بالتاغالوغية"),
    ("Polish", "بالبولندية"),
    ("Greek", "باليونانية"),
    ("Hebrew", "بالعبرية"),
    ("Dutch", "بالهولندية"),
    ("Turkish", "بالتركية"),
    ("Persian", "بالفارسية"),
    ("Farsi", "بالفارسية"),
    ("Ukrainian", "بالأوكرانية"),
    ("Armenian", "بالأرمنية"),
    ("Patois", "بالباتوا"),
    ("Jamaican Patois", "بالباتوا الجامايكية"),
    ("Swahili", "بالسواحيلية"),
];

/// "In Spanish: Bro, I'm telling you…" -> ("Spanish", "Bro, I'm telling you…"): the language the line
/// is spoken in, kept apart so the sentence is translated (and remembered) on its own. A misread
/// language ("Spahiyh") is matched to the nearest known one.
pub fn language_tag(en: &str) -> Option<(String, &str)> {
    let rest = en.strip_prefix("In ")?;
    let colon = rest.find(':')?;
    let lang = &rest[..colon];
    let words: Vec<&str> = lang.split_whitespace().collect();
    let capitalised = |w: &&str| w.chars().next().is_some_and(|c| c.is_ascii_uppercase()) && w.chars().all(char::is_alphabetic);
    if words.is_empty() || words.len() > 2 || !words.iter().all(capitalised) {
        return None;
    }
    let sentence = rest[colon + 1..].trim_start();
    if sentence.chars().filter(|c| c.is_alphabetic()).count() < 2 {
        return None;
    }
    let names = || LANGUAGES.iter().map(|(en, _)| *en);
    let known = names()
        .find(|k| k.eq_ignore_ascii_case(lang))
        .or_else(|| names().filter(|k| closeness(k, lang) >= 0.7).max_by(|a, b| closeness(a, lang).total_cmp(&closeness(b, lang))));
    Some((known.unwrap_or(lang).to_string(), sentence))
}

/// "In Spanish:" in the target language when the app knows it (Arabic), else None: translate it.
pub fn language_in(lang: &str, language: &str) -> Option<String> {
    if lang != "ar" {
        return None;
    }
    LANGUAGES.iter().find(|(en, _)| *en == language).map(|(_, ar)| format!("{ar}:"))
}

/// The tag in front of the translated sentence (plain text, like the game's).
pub fn with_language(tag_shown: &str, mut segs: Segments) -> Segments {
    let mut out: Segments = vec![(format!("{} ", tag_shown.trim()), None)];
    out.append(&mut segs);
    out
}

/// 0..1 likeness of two words (1 - edit distance / longer length), case-insensitive.
fn closeness(a: &str, b: &str) -> f64 {
    let (a, b): (Vec<char>, Vec<char>) = (a.to_lowercase().chars().collect(), b.to_lowercase().chars().collect());
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut cur = vec![i; b.len() + 1];
        for j in 1..=b.len() {
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + usize::from(a[i - 1] != b[j - 1]));
        }
        prev = cur;
    }
    1.0 - prev[b.len()] as f64 / a.len().max(b.len()).max(1) as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gold() -> Vec<(String, String)> {
        vec![("Dobbs:".into(), "#ffd69f".into())]
    }

    #[test]
    fn a_coloured_name_is_split_off() {
        let (name, color, rest) = split("Dobbs: Not to be annoying, can you fix my leg?", &gold()).unwrap();
        assert_eq!((name.as_str(), color.as_str(), rest), ("Dobbs", "#ffd69f", "Not to be annoying, can you fix my leg?"));
    }

    #[test]
    fn a_coloured_speaker_whose_colon_the_ocr_lost_is_split_off() {
        // "Local: What is wrong with you?" read "Local. What…" or "Local What…" (the pale name read on its
        // own): it went whole to the translator ("محلي، ما خطبك؟")
        let ph = vec![("Local".to_string(), "#d98cff".to_string())];
        for en in ["Local. What is wrong with you?", "Local What is wrong with you?"] {
            let (name, color, rest) = split(en, &ph).unwrap();
            assert_eq!((name.as_str(), color.as_str(), rest), ("Local", "#d98cff", "What is wrong with you?"));
        }
        // a coloured word at the start that isn't one of the game's speakers: no name
        let ph = vec![("Careful".to_string(), "#ff0".to_string())];
        assert!(split("Careful. It's hot.", &ph).is_none());
        // the name's letters starting a longer word: no name
        let ph = vec![("Local".to_string(), "#d98cff".to_string())];
        assert!(split("Locally grown, they said.", &ph).is_none());
    }

    #[test]
    fn a_name_coloured_without_its_colon_is_split_off() {
        // the game's colour stopped before the colon: the line went whole to the translator
        // ("أندرو) طالما…") and the name came out uncoloured
        let ph = vec![("Andrew".to_string(), "#7fc8ff".to_string())];
        let (name, color, rest) = split("Andrew: As long as it's under a thousand.", &ph).unwrap();
        assert_eq!((name.as_str(), color.as_str(), rest), ("Andrew", "#7fc8ff", "As long as it's under a thousand."));
        let ph = vec![("Real Dimez".to_string(), "#c8a0ff".to_string())];
        assert_eq!(split("Real Dimez: Yo.", &ph).unwrap().0, "Real Dimez");
        let ph = vec![("Stefanie".to_string(), "#ffb0d0".to_string())];
        assert_eq!(split("Stefanie: So it wasn't premeditated?", &ph).unwrap().1, "#ffb0d0");
        // another word coloured, not the name: no name
        let ph = vec![("premeditated".to_string(), "#ff0".to_string())];
        assert!(split("Stefanie: So it wasn't premeditated?", &ph).is_none());
    }

    #[test]
    fn a_two_word_name_is_split_off() {
        let ph = vec![("Real Dimez:".to_string(), "#c8a0ff".to_string())];
        let (name, color, rest) = split("Real Dimez: Yo, what's up?", &ph).unwrap();
        assert_eq!((name.as_str(), color.as_str(), rest), ("Real Dimez", "#c8a0ff", "Yo, what's up?"));
        assert_eq!(name_in("ar", "Real Dimez", None), "ريال دايمز");
        assert_eq!(name_in("ar", "Officer Jason", None), "الضابط جيسون");
        assert_eq!(without_name("Officer Smith: Hands up!"), Some("Hands up!"));
        // a colon later in a sentence is not a name
        assert!(without_name("Listen up: we go now.").is_none());
        assert!(without_name("Hey Jason, come: now").is_none());
    }

    #[test]
    fn a_language_tag_is_kept_apart() {
        assert_eq!(language_tag("In Italian: Go, go, go."), Some(("Italian".to_string(), "Go, go, go.")));
        assert_eq!(language_tag("In Spahiyh: One coffee, please."), Some(("Spanish".to_string(), "One coffee, please.")));
        assert_eq!(language_tag("In Haitian Creole: Let's go."), Some(("Haitian Creole".to_string(), "Let's go.")));
        assert_eq!(language_in("ar", "Italian").as_deref(), Some("بالإيطالية:"));
        assert!(language_tag("In time: we go").is_none());
        assert!(language_tag("In the end, it's fine.").is_none());
        assert_eq!(without_name("In Spanish: Hola."), Some("Hola."));
        let segs = with_language("بالإيطالية:", vec![("هيا، هيا، هيا.".into(), None)]);
        assert_eq!(segs[0].0, "بالإيطالية: ");
    }

    #[test]
    fn a_white_label_is_not_a_name() {
        assert!(split("Tip: Hold L2 to aim.", &[]).is_none());
        assert!(split("Not to be annoying: fine.", &gold()).is_none());
    }

    #[test]
    fn dots_by_a_name_are_dropped() {
        let ph = vec![("Lucia.:".to_string(), "#ffd69f".to_string())];
        assert_eq!(split("Lucia.: Let's go.", &ph).unwrap().0, "Lucia");
        assert_eq!(name_in("ar", "Lucia", Some("لوسيا.".into())), "لوسيا");
        assert_eq!(name_in("ar", "Dobbs", Some("دوبس..".into())), "دوبس");
        assert_eq!(name_in("ar", "Dobbs", Some("دوبس، هيا بنا.".into())), "دوبس");
        assert_eq!(plain_name("Jason..: Hold up."), Some(("Jason".to_string(), "Hold up.")));
        assert!(plain_name("In Spanish: Hola, amigo.").is_none());
        assert!(is_known("Jason") && !is_known("Tip"));
    }

    #[test]
    fn sound_cues_are_kept_apart() {
        let (c, rest) = take_cues("[rattling] Donnie: Hell nah, she's good We're good!").unwrap();
        assert_eq!((c.before.clone(), rest.as_str()), (vec!["rattling".to_string()], "Donnie: Hell nah, she's good We're good!"));
        let (c, rest) = take_cues("Donnie: [sigh] Fuck.").unwrap();
        assert_eq!((c.after_name.clone(), rest.as_str()), (vec!["sigh".to_string()], "Donnie: Fuck."));
        let (c, rest) = take_cues("Donnie: [laughs]").unwrap();
        assert_eq!((c.after_name.clone(), rest.as_str()), (vec!["laughs".to_string()], "Donnie:"));
        let (c, rest) = take_cues("We're good! [laughs]").unwrap();
        assert_eq!((c.after.clone(), rest.as_str()), (vec!["laughs".to_string()], "We're good!"));
        assert!(take_cues("Hold [1] to aim").is_none(), "not a cue: no letters");
        assert!(take_cues("No cues here.").is_none());
        assert_eq!(cue_in("ar", "Laughs"), Some("يضحك".to_string()));
        assert_eq!(cue_in("hi", "laughs"), None);
        // put back: in front, after the name, at the end
        let segs = with_name("دوني", "#ffd69f", vec![("تبًا.".into(), None)]);
        let c = Cues { before: vec!["rattling".into()], after_name: vec!["sigh".into()], after: vec![] };
        let out = with_cues(&c, &["قعقعة".into(), "يتنهّد".into()], true, segs);
        let text: String = out.iter().map(|s| s.0.as_str()).collect();
        assert_eq!(text, "[قعقعة] دوني: [يتنهّد] تبًا.");
        assert_eq!(out[0].1.as_deref(), Some("name:#b9b9b9"));
    }

    #[test]
    fn a_role_is_translated() {
        assert_eq!(name_in("ar", "Cop", None), "الشرطي");
        assert_eq!(name_in("ar", "Prison Guard", None), "حارس السجن");
        assert_eq!(name_in("ar", "Officer Smith", None), "الضابط سميث");
        // "The Man" was spelt out as "ذ مان"
        assert_eq!(name_in("ar", "The Man", None), "الرجل");
        assert_eq!(name_in("ar", "The Guard", None), "الحارس");
        assert_eq!(name_in("ar", "Guard 2", None), "الحارس 2");
        assert!(!name_in("ar", "The Zorrusso", None).contains('ذ'));
        // a name the list has wins over a downloaded line of the same text (a line of dialogue)
        assert_eq!(name_in("ar", "Mother", Some("يا ابن...".into())), "الأم");
        assert_eq!(name_in("ar", "Local", Some("محلي!".into())), "أحد السكان");
        // OCR misreadings of the main people
        assert_eq!(name_in("ar", "Luqia", None), "لوسيا");
        assert_eq!(name_in("ar", "Jasen", None), "جيسون");
        // a white "Tip:" is never taken for a speaker
        assert!(!is_known("Tip"));
        assert!(is_known("Stranger"));
        assert!(is_role("Cop") && is_role("Officer Smith") && !is_role("Dobbs"));
    }

    #[test]
    fn names_in_arabic_letters() {
        assert_eq!(name_in("ar", "Dobbs", None), "دوبس");
        assert_eq!(name_in("ar", "Lucia", None), "لوسيا");
        assert_eq!(name_in("ar", "Jason", None), "جيسون");
        // (a name the list doesn't have: the downloaded translation of it)
        assert_eq!(name_in("ar", "Zorvath", Some("زورفاث".into())), "زورفاث");
        assert_eq!(name_in("hi", "Dobbs", None), "Dobbs");
        // (a name written already stays as it is: a page read again kept its line, and "بي-لوكس" came back as "-")
        assert_eq!(name_in("ar", "Bae-Luxe", None), "بي-لوكس");
        for done in ["بي-لوكس", "دريكوان", "جيسون"] {
            assert_eq!(name_in("ar", done, None), done);
        }
        assert_eq!(transliterate("dennis"), "دينيس");
        assert_eq!(transliterate("bobby"), "بوبي");
        assert_eq!(transliterate("kyle"), "كايل");
        assert_eq!(transliterate("kenton"), "كينتون");
        assert_eq!(transliterate("shirley"), "شيرلي");
        assert_eq!(transliterate("peggy"), "بيغي");
    }

    #[test]
    fn the_name_goes_in_front_in_its_colour() {
        let segs = with_name("دوبس", "#ffd69f", vec![("هل يمكنك إصلاح ساقي؟".into(), None)]);
        assert_eq!(segs[0], ("دوبس:".to_string(), Some("name:#ffd69f".to_string())));
        assert_eq!(phrases_without(&gold(), "Dobbs"), Vec::<(String, String)>::new());
    }
}
