//! User settings, saved as JSON in the app's config folder.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Write a file whole or not at all: to a temporary file next to it, then renamed over it. A crash
/// or a full disk midway leaves the old file as it was (never a half-written one).
pub fn write_atomic(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = path.with_file_name(format!("{name}.{}-{}.tmp", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
    let result = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Text to show (a name from the server, a settings file or the player): no control characters and
/// no bidi embeddings, overrides or isolates (one left open in an Arabic name turns the text after
/// it around wherever the name is shown), at most `max` characters. (The window escapes it too;
/// this keeps it short and printable.)
pub fn clean_text(s: &str, max: usize) -> String {
    s.chars().filter(|c| !c.is_control() && !bidi_control(*c)).take(max).collect::<String>().trim().to_string()
}

/// U+202A–U+202E (embeddings, overrides) and U+2066–U+2069 (isolates).
fn bidi_control(c: char) -> bool {
    matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}

/// A colour as the style form and the overlays use it: "#rrggbb".
pub fn is_hex_color(s: &str) -> bool {
    s.len() == 7 && s.starts_with('#') && s[1..].bytes().all(|b| b.is_ascii_hexdigit())
}

/// A font family name as installed fonts have them, in any script ("(A) Arslan Wessam A", an Arabic
/// font known only by its Arabic name, with its marks): anything printable but what could end the
/// quoted name in the windows' CSS (" \ ; { } < >). Anything else: none, the default.
pub fn clean_font(s: &str) -> String {
    let s = s.trim();
    let ok = |c: char| !c.is_control() && !bidi_control(c) && !"\"\\;{}<>".contains(c);
    if s.chars().count() <= 64 && s.chars().all(ok) { s.to_string() } else { String::new() }
}

/// A setting's keyword ("under_text", "bottom-center" …): a-z, 0-9, _ and - only, else none.
fn clean_word(s: &str) -> String {
    if s.len() <= 32 && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-') { s.to_string() } else { String::new() }
}

/// An area id the server takes in line uploads: [A-Za-z0-9_-]{1,32}.
pub fn valid_area_id(s: &str) -> bool {
    !s.is_empty() && s.len() <= 32 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// A number within a range (and a real number: NaN or infinity becomes the default).
fn within(v: f64, lo: f64, hi: f64, default: f64) -> f64 {
    if v.is_finite() { v.clamp(lo, hi) } else { default }
}

/// A rectangle of the captured window that is read with OCR. Coordinates are fractions (0..1)
/// of the window's client area, so they survive window resizes.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Area {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    /// Where the Arabic box goes relative to the area: above | below | left | right | over | custom
    pub placement: String,
    /// With match_size: fine-tune on top of the matched size. Without: multiplies the global font size.
    pub font_scale: f64,
    /// Size the Arabic from the English text's measured height, so both look the same size.
    pub match_size: bool,
    /// Font for this area's Arabic; empty = use the main font from Style.
    pub font_family: String,
    /// Where the Arabic box sits in its region for the preset placements: "" (automatic) or
    /// "<top|middle|bottom>-<left|center|right>", e.g. "top-left". Custom placement uses out_*align.
    pub text_pos: String,
    /// Box behind this area's Arabic: "" = main style, "none" = no box, outlined text like game
    /// subtitles, "game" = GTA-style dark see-through box with square corners.
    pub background: String,
    /// placement = "custom": the region the Arabic is drawn in, as fractions of the window.
    pub out_x: f64,
    pub out_y: f64,
    pub out_w: f64,
    pub out_h: f64,
    /// placement = "custom": where the text box sits inside that region.
    pub out_valign: String, // top | middle | bottom
    pub out_halign: String, // right | center | left
    /// Only read text centred in the picture (game subtitles are): a word elsewhere in the area — a
    /// menu's "Browse" at the bottom right — is not a subtitle.
    pub centered_only: bool,
    /// Only read text inside the game's own box (GTA VI's help box): text with no box around it —
    /// the credits' names, a sign — is not this area's. The translation also takes the box's edges.
    #[serde(default)]
    pub needs_box: bool,
    /// With needs_box: where the game's box always is, as fractions of the picture: its left edge, its
    /// top and its widest (it grows to the right and down with its text). None: GTA VI's help box for
    /// the "help" area (see `box_anchor`), else found by its edges alone.
    #[serde(default)]
    pub box_anchor: Option<[f64; 3]>,
    /// This area's own text style (colours, box, outline, size …). None: the profile's style, which
    /// is what an area starts from (settings and layouts from before areas had their own).
    pub style: Option<Style>,
}

impl Area {
    /// Keep its settings within what the app uses (they can come from the server's layouts and
    /// games list, which aren't signed): places inside the picture, sizes in range, keywords and
    /// fonts plain words, a short printable name.
    pub fn sanitize(&mut self) {
        let d = Area::default();
        self.name = clean_text(&self.name, 60);
        if self.name.is_empty() {
            self.name = self.id.clone();
        }
        for v in [&mut self.x, &mut self.y, &mut self.out_x, &mut self.out_y] {
            *v = within(*v, 0.0, 1.0, 0.0);
        }
        for v in [&mut self.w, &mut self.h, &mut self.out_w, &mut self.out_h] {
            *v = within(*v, 0.005, 1.0, 0.1);
        }
        self.font_scale = within(self.font_scale, 0.3, 3.0, d.font_scale);
        self.font_family = clean_font(&self.font_family);
        for v in [&mut self.placement, &mut self.text_pos, &mut self.background, &mut self.out_valign, &mut self.out_halign] {
            *v = clean_word(v);
        }
        if self.placement.is_empty() {
            self.placement = d.placement;
        }
        if let Some(s) = self.style.as_mut() {
            s.sanitize();
        }
    }
}

impl Default for Area {
    fn default() -> Self {
        Self {
            id: "area".into(),
            name: "Area".into(),
            enabled: true,
            x: 0.1,
            y: 0.1,
            w: 0.3,
            h: 0.1,
            placement: "above".into(),
            font_scale: 1.0,
            match_size: true,
            centered_only: false,
            needs_box: false,
            box_anchor: None,
            font_family: String::new(),
            text_pos: String::new(),
            background: String::new(),
            out_x: 0.1,
            out_y: 0.6,
            out_w: 0.8,
            out_h: 0.15,
            out_valign: "bottom".into(),
            out_halign: "center".into(),
            style: None,
        }
    }
}

/// How the Arabic text looks. Sizes are in pixels for a 1080p window and scale with the window.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Style {
    pub font_family: String,
    pub font_size: f64,
    pub bold: bool,
    pub text_color: String,
    pub bg_color: String,
    pub bg_opacity: f64,
    pub outline_color: String,
    pub outline_width: f64,
    pub shadow: bool,
    pub padding: f64,
    pub radius: f64,
    pub line_height: f64,
    /// Colour Arabic words the same as highlighted English words.
    pub keep_colors: bool,
}

impl Style {
    /// Keep it within what the style form offers (it can come from the server: see Area::sanitize).
    pub fn sanitize(&mut self) {
        let d = Style::default();
        self.font_family = clean_font(&self.font_family);
        if self.font_family.is_empty() {
            self.font_family = d.font_family.clone();
        }
        for (v, dv) in [(&mut self.text_color, &d.text_color), (&mut self.bg_color, &d.bg_color), (&mut self.outline_color, &d.outline_color)] {
            if !is_hex_color(v) {
                *v = dv.clone();
            }
        }
        self.font_size = within(self.font_size, 10.0, 96.0, d.font_size);
        self.line_height = within(self.line_height, 1.0, 2.2, d.line_height);
        self.bg_opacity = within(self.bg_opacity, 0.0, 1.0, d.bg_opacity);
        self.padding = within(self.padding, 0.0, 30.0, d.padding);
        self.radius = within(self.radius, 0.0, 30.0, d.radius);
        self.outline_width = within(self.outline_width, 0.0, 6.0, d.outline_width);
    }
}

impl Default for Style {
    fn default() -> Self {
        Self {
            font_family: "Segoe UI".into(),
            font_size: 26.0,
            bold: true,
            text_color: "#ffffff".into(),
            bg_color: "#000000".into(),
            bg_opacity: 0.65,
            outline_color: "#000000".into(),
            outline_width: 0.0,
            shadow: true,
            padding: 10.0,
            radius: 10.0,
            line_height: 1.5,
            keep_colors: true,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Exact title of the window to capture.
    pub window_title: String,
    /// Only part of that window is the game picture (another program showing the game with its own
    /// panels around it, like a streaming or upscaling app): that part, as x, y, width, height
    /// fractions of the window. None: the whole window. (Window sources only.)
    pub source_crop: Option<[f64; 4]>,
    /// The window the Arabic is drawn over on this PC: "" = the source window. Another window when the
    /// picture is read elsewhere (a capture card, OBS, another app's virtual camera) but the game is
    /// watched in this one.
    pub overlay_window: String,
    /// The part of `overlay_window` that shows the game (x, y, width, height fractions); None: all of it.
    pub overlay_crop: Option<[f64; 4]>,
    /// Send the game with its translations as a webcam ("OBS Virtual Camera") for other apps.
    pub virtual_camera: bool,
    /// With a capture card as the source: play its sound on this PC's speakers (the app reads only
    /// the picture), from this sound input ("" = the one named after the card), at this volume (0–100).
    pub card_sound: bool,
    pub card_sound_input: String,
    pub card_sound_volume: u32,
    /// The snapshot's keyboard shortcut, pressed anywhere ("Control+Shift+KeyS", "F8"; "" = none).
    pub snap_hotkey: String,
    /// local | deepl | azure | google | libre | claude | openai
    pub engine: String,
    /// VI Translator's ready translations first (the shared pack downloaded for the game: reviewed
    /// lines, shown at once); a line it hasn't goes to `engine`. Off: the pack isn't used, and every
    /// line the player's own memory doesn't know goes to the translator (much slower). On in a
    /// settings file from before this setting.
    pub shared_translations: bool,
    /// Language to translate into (see `langs::LANGS`): "ar", "hi", "tr" …
    pub target_lang: String,
    /// Look for a newer version of the app (on start and daily) and offer it.
    pub update_check: bool,
    /// Live translations page for other devices on the home network (off by default).
    pub lan_page: bool,
    pub lan_port: u16,
    /// Language of the app's own interface: "en" or "ar" (right to left).
    pub ui_lang: String,
    pub deepl_key: String,
    pub azure_key: String,
    pub azure_region: String,
    pub google_key: String,
    pub libre_url: String,
    pub libre_key: String,
    pub claude_key: String,
    pub claude_model: String,
    /// ChatGPT (OpenAI API)
    pub openai_key: String,
    pub openai_model: String,
    /// Claude and ChatGPT only: msa | egyptian | gulf | levantine | saudi
    pub dialect: String,
    /// Claude and ChatGPT only: the player's own notes added to the prompt (names, terms, tone).
    /// Empty: the prompt is unchanged.
    pub llm_notes: String,
    pub interval_ms: u64,
    pub stable_ms: u64,
    pub clear_after_ms: u64,
    /// The player chose their own timing (the three values above) instead of the game profile's.
    /// None only in a settings file from before this setting: worked out once when it is loaded.
    /// (missing in a file: None, not the default's value, so an older file is recognised)
    #[serde(default)]
    pub timing_custom: Option<bool>,
    pub overlays_enabled: bool,
    /// Each output's own switch, under `overlays_enabled` (each TV's is kept with the TV, tv.rs):
    /// this device's own overlays: the overlay windows on a PC, the translations drawn over the
    /// picture in a phone app (its viewer of the TV page, `Tvs::local_key`) ...
    pub device_overlays: bool,
    /// ... and the OBS browser source.
    pub obs_overlays: bool,
    /// GTA VI's phone in the player's hand (WhatUp, Snapmatic): its text translated beside it.
    pub phone_text: bool,
    /// GTA VI's message bubble at the bottom right: its translation in a bubble above it.
    /// None only in a settings file from before this setting (the bubble went with `phone_text` then):
    /// set to `phone_text` once when it is loaded.
    #[serde(default)]
    pub phone_bubble: Option<bool>,
    /// The list of live translations (in the app and on the home-network page) keeps filling,
    /// whether or not the overlays are on.
    pub live_translations: bool,
    /// The preview's "NN fps" badge: shown or not, and in which corner of the picture
    /// (top-left | top-right | bottom-left | bottom-right).
    pub fps_badge: bool,
    pub fps_corner: String,
    /// App window theme: light | dark
    pub theme: String,
    /// Random id of this install (no account), made on first start.
    pub install_id: String,
    /// Project server; empty = the built-in one.
    pub server_url: String,
    /// The profile last used for each game (where switching to a game goes).
    pub game_profiles: std::collections::BTreeMap<String, String>,
    /// Version of the last layout update applied (from before per-game versions; see `layout_versions`).
    pub layout_version: u32,
    /// Version of the last layout update applied, by game.
    pub layout_versions: std::collections::BTreeMap<String, u32>,
    /// Games the server added (built-in profiles beyond the app's own `BUILTINS`), and the version of
    /// the list they came in.
    pub server_games: Vec<String>,
    pub games_version: u32,
    /// OBS on this PC: its WebSocket server (Tools → WebSocket Server Settings), used to read an OBS
    /// source ("obs:<name>" as the source) while OBS owns the capture card.
    pub obs_port: u16,
    pub obs_password: String,
    /// Port of the Arabic-only page for an OBS Browser Source (http://127.0.0.1:<port>/obs).
    pub obs_page_port: u16,
    /// Full frames per second read for text (OCR, profiles, saved frames).
    pub capture_fps: u32,
    /// Frames per second of the live preview in the app (small frames, only while it's visible).
    pub preview_fps: u32,
    /// 4K picture: a capture card runs at its largest size up to 3840×2160 (else 1920×1080 is
    /// preferred), and the app's picture is shown up to 3840 wide (else 1920). For players with a
    /// 4K card or a 4K Remote Play window who play on the app's picture; it costs more CPU and GPU.
    pub picture_4k: bool,
    /// Snapshots (the whole picture translated at once): the service ("" = the live translations'
    /// one; an engine id; "azure-image" = Azure's image translation, with its own endpoint and key).
    pub snap_service: String,
    pub snap_azure_endpoint: String,
    pub snap_azure_key: String,
    /// "gemini-image": Google's image model and its API key
    pub snap_gemini_model: String,
    pub snap_gemini_key: String,
    /// Keep every snapshot (the picture and its translation) in Pictures\VI Translator\snapshots.
    pub snap_save: bool,
    /// The player's own showing and hiding of a profile's read areas and game pages, by profile id: kept
    /// outside the profile, so a built-in one's can be switched too (its other settings stay locked).
    pub shown: std::collections::BTreeMap<String, Shown>,
    /// The home-network page may switch the overlays, the live translations and what is shown.
    pub lan_control: bool,
    /// The home-network page can take snapshots too.
    pub snap_lan: bool,
    /// What covers the English under a translation: "blur" (the picture, blurred) or "solid".
    pub snap_style: String,
    /// Debugging: save captured frames as PNG files while on.
    pub debug_save_frames: bool,
    /// Debugging: minimum time between saved frames.
    pub debug_frame_interval_ms: u64,
    /// The game profile in use: a built-in one ("gta6", or a game the server added) or one of
    /// `profiles` (a copy).
    pub profile: String,
    /// Goes up each time the app switches games by itself: a window's save sent before it heard of
    /// the switch can't undo it (see save_config).
    pub profile_rev: u64,
    /// The settings file was there but couldn't be read (nor kept aside) at start: it isn't saved over.
    #[serde(skip)]
    pub unreadable: bool,
    /// Running on a phone (set by the phone library, never saved): the phone's timing defaults.
    #[serde(skip)]
    pub phone: bool,
    /// Which changes of the app's defaults this settings file has had (see `DEFAULTS_VERSION`).
    /// A file from before it reads 0.
    #[serde(default)]
    pub defaults_version: u32,
    /// Set by `load` when it moved a setting to a new default: the caller saves the file.
    #[serde(skip)]
    pub defaults_moved: bool,
    /// The player's own profiles (copies they can change); the built-in ones are in `builtins`.
    pub profiles: Vec<Profile>,
    /// The built-in profiles as they are now (the app's defaults, tuned by layout updates from the
    /// server). Never changed by the player: copied to make one of theirs.
    pub builtins: Vec<Profile>,
    /// 1 once the setup from before profiles has been moved into them.
    pub profiles_version: u32,
    /// The profile in use, as the rest of the app reads it (kept equal to that profile by `normalize`):
    /// its game pages, read areas and translation text style.
    pub profile_opts: ProfileOpts,
    pub areas: Vec<Area>,
    pub style: Style,
    /// How each read area's translation comes on screen, by area id (Read Areas): the player's own,
    /// kept for every profile, built-in ones too (their areas can't be changed). Missing: the defaults.
    pub area_fx: std::collections::BTreeMap<String, AreaFx>,
    /// The game's own defaults for its areas' effects where the player set none (its built-in profile's
    /// `fx`, which layout updates tune): kept equal to it by `normalize`.
    pub fx_defaults: std::collections::BTreeMap<String, FxDefaults>,
    /// Each area's effects as they apply (the player's own, else the game's default, else the app's):
    /// worked out by `normalize`, for the windows and pages that draw; what they send back is ignored.
    #[serde(skip_deserializing)]
    pub fx_view: std::collections::BTreeMap<String, AreaFx>,
}

/// A game's defaults for a read area's effects (see `Profile::fx`): only what it names.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct FxDefaults {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fade_ms: Option<u32>,
}

/// How a read area's translation comes on screen (see `Config::area_fx`).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AreaFx {
    /// With the "over the game's text" look: the black box is at least as big as the game's text
    /// (its own box around it, when the game draws one), so none of the English shows past it.
    pub cover_min: bool,
    /// With that look: the black box is up as soon as the text is seen, before its translation
    /// is ready (the translation then comes into it).
    pub cover_early: bool,
    /// The translation fades in over this many ms (0 = it just appears); not set: the game's default
    /// (`Config::fx_defaults`, from layout updates), or off.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fade_ms: Option<u32>,
    /// The game's lines stack up from a bottom line (GTA VI's subtitles: up to six lines and more):
    /// the area's text is read only when a line sits in the area, and the lines above it only when
    /// they're joined to it, up to STACK_H above the area. None: the area's default (`stack_default`).
    pub stack_up: Option<bool>,
    /// With the "above the game's text" look: right above the stacked lines' top (the area that
    /// stacks up), moving up as more lines come; the stacking area's translation goes above the
    /// others'. None: the area's default (`ride_default`).
    pub ride: Option<bool>,
    /// A border in Vice City colours around this area's translation, to tell it apart from the others
    /// (GTA VI's bottom instructions from the subtitles). None: the area's default (`accent_default`).
    pub accent: Option<bool>,
    /// Only reviewed translations: a reading that matches no reviewed line (an OCR misreading of the
    /// line over bright ground) isn't translated; what's up stays, then goes. For text the reviewed
    /// translations cover whole (GTA VI's bottom instructions). None: the area's default
    /// (`reviewed_only_default`).
    pub reviewed_only: Option<bool>,
}

impl Default for AreaFx {
    fn default() -> Self {
        Self { cover_min: true, cover_early: true, fade_ms: None, stack_up: None, ride: None, accent: None, reviewed_only: None }
    }
}

/// How far above its area a stacking area reads (fraction of the picture's height): ten lines of
/// GTA VI's subtitles.
pub const STACK_H: f64 = 0.36;

/// GTA VI's help box (top left), measured on 2,700 frames of a 1080p stream (124 with its edges
/// clear, to the pixel): its left edge at 32 px, its top at 288, at most 445 px wide (its width and
/// height follow its text: its right edge a line height past the text's, its bottom 11 px under it; a
/// line is 33 px).
pub const GTA6_HELP_BOX: [f64; 3] = [32.0 / 1920.0, 288.0 / 1080.0, 0.232];

/// Where an area's game box always is (Area::box_anchor), GTA VI's help box for "help".
pub fn box_anchor(a: &Area) -> Option<[f64; 3]> {
    a.box_anchor.or_else(|| (a.needs_box && a.id == "help").then_some(GTA6_HELP_BOX))
}

/// The part of the picture an area whose lines stack up is read in: up to STACK_H above its bottom.
pub fn stack_grown(a: &Area) -> Area {
    let bottom = (a.y + a.h).clamp(0.0, 1.0);
    let top = (bottom - STACK_H).max(0.0).min(a.y);
    Area { y: top, h: bottom - top, ..a.clone() }
}

/// GTA VI's subtitles stack up from their bottom line (the built-in areas' ids).
pub fn stack_default(area: &str) -> bool {
    area == "subtitles"
}
/// …and their translation and the bottom instructions' move together above them.
pub fn ride_default(area: &str) -> bool {
    area == "subtitles" || area == "instructions"
}

impl AreaFx {
    /// GTA VI's bottom instructions: all of them are in the reviewed translations.
    pub fn reviewed_only(&self, area: &str) -> bool {
        self.reviewed_only.unwrap_or(area == "instructions")
    }
    pub fn stacks(&self, area: &str) -> bool {
        self.stack_up.unwrap_or_else(|| stack_default(area))
    }
    pub fn rides(&self, area: &str) -> bool {
        self.ride.unwrap_or_else(|| ride_default(area))
    }
}

impl Config {
    /// An area's effects (the defaults when it has none of its own).
    pub fn fx(&self, area: &str) -> AreaFx {
        let mut f = self.area_fx.get(area).cloned().unwrap_or_default();
        f.fade_ms = Some(f.fade_ms.or_else(|| self.fx_defaults.get(area).and_then(|d| d.fade_ms)).unwrap_or(0).min(2000));
        f
    }
    /// Every read area's effects as they apply now (`fx`): what the overlays and pages draw by.
    pub fn fx_all(&self) -> std::collections::BTreeMap<String, AreaFx> {
        let ids = self.areas.iter().map(|a| a.id.clone()).chain(self.area_fx.keys().cloned()).chain(self.fx_defaults.keys().cloned());
        ids.map(|id| {
            let f = self.fx(&id);
            (id, f)
        })
        .collect()
    }
    /// Only reviewed translations in this area (`AreaFx::reviewed_only`), while they are used: with
    /// the ready translations off there are none, and the area's lines go to the translator like any.
    pub fn reviewed_only(&self, area: &str) -> bool {
        self.shared_translations && self.fx(area).reviewed_only(area)
    }
    /// The phone's text translated beside it: GTA VI's (its built-in profile, or a copy of it).
    pub fn phone_text(&self) -> bool {
        self.phone_text && self.game() == "gta6"
    }
    /// GTA VI's message bubble translated above it (as the phone, in settings from before its own switch).
    pub fn phone_bubble(&self) -> bool {
        self.phone_bubble.unwrap_or(self.phone_text) && self.game() == "gta6"
    }
    /// The game profile's look puts each translation over the game's lines, on a black box.
    pub fn cover_look(&self) -> bool {
        self.profile_opts.more.get("translation_cover") == Some(&true)
    }
}

/// What's translated, as the player chooses it (Game pages): one checkbox for each part of the game
/// translated apart, none covering another, shown under its group's heading (`group`). Each stands for
/// one thing: a read area of the profile (`areas`), a switch of the game's pages script (`pages`, saved in
/// the profile's `more`), or a setting of the app's own (`setting`: "phone_text", "phone_bubble"). The
/// dialogue (the subtitles) is always translated, so no switch covers it. Nothing of a switch's own is
/// saved: the defaults stay the areas', pages' and settings' own (all on, as before there were switches).
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct ContentSwitch {
    pub id: &'static str,
    pub group: &'static str,
    pub areas: Vec<&'static str>,
    pub pages: Vec<&'static str>,
    pub setting: &'static str,
}

/// A game's What's-translated switches (none for a game without them).
/// GTA VI's side conversations (people in the street talking) aren't told apart from the story's
/// dialogue: they come in the same subtitles, so they have no switch of their own.
pub fn content_switches(game: &str) -> Vec<ContentSwitch> {
    let setting = |group, id| ContentSwitch { id, group, areas: Vec::new(), pages: Vec::new(), setting: id };
    let page = |group, id| ContentSwitch { id, group, areas: Vec::new(), pages: vec![id], setting: "" };
    let area = |group, id| ContentSwitch { id, group, areas: vec![id], pages: Vec::new(), setting: "" };
    match game {
        "gta6" => vec![
            // the phone: its WhatUp chats on a second phone beside it, the message bubble at the bottom right
            setting("phone", "phone_text"),
            setting("phone", "phone_bubble"),
            // the pause menu, part by part (the script's `pause_*` switches, each from the old `pause_menu`)
            page("menus", "pause_home"),
            page("menus", "pause_map"),
            page("menus", "pause_map_index"),
            page("menus", "pause_map_box"),
            page("menus", "pause_map_buttons"),
            page("menus", "pause_brief"),
            page("menus", "pause_jason_lucia"),
            page("menus", "pause_progress"),
            page("menus", "pause_help"),
            page("menus", "pause_settings"),
            page("menus", "pause_tabs"),
            // the help box (top left), the instructions under the subtitles, the game's notices
            area("notices", "help"),
            area("notices", "instructions"),
            page("notices", "game_notices"),
            // the game's other screens
            page("screens", "wasted"),
            page("screens", "mission_failed"),
            page("screens", "weapon_wheel"),
        ],
        _ => Vec::new(),
    }
}

/// The player's own show/hide of one profile's read areas and game pages (see `Config::shown`).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Shown {
    pub areas: std::collections::BTreeMap<String, bool>,
    pub pages: std::collections::BTreeMap<String, bool>,
}

/// A game profile: where the text is read and where its translation goes (read areas), how the
/// translation looks (text style), and the game's own pages it translates (menus …).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Profile {
    pub id: String,
    pub name: String,
    /// The game it is for ("gta6" | one the server added): which game pages it can recognise, and which shared
    /// translations and layout updates it gets. A copy keeps the game of the profile it came from.
    pub game: String,
    pub areas: Vec<Area>,
    pub style: Style,
    pub pages: ProfileOpts,
    /// How the game's text comes and goes, if it needs other timing than the app's (0 = the app's).
    pub timing: Timing,
    /// The game's defaults for its areas' effects, by area id (a layout update's `fx`): what applies
    /// where the player chose nothing (`Config::fx`).
    #[serde(skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub fx: std::collections::BTreeMap<String, FxDefaults>,
}

/// Reading timing a game can bring (see `Config::timing`).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Timing {
    pub interval_ms: u64,
    pub stable_ms: u64,
    pub clear_after_ms: u64,
}

impl Timing {
    /// Within the Timing sliders' ranges (0 = the app's own value).
    pub fn sanitize(&mut self) {
        let fit = |v: u64, lo: u64, hi: u64| if v == 0 { 0 } else { v.clamp(lo, hi) };
        self.interval_ms = fit(self.interval_ms, 16, 1000);
        self.stable_ms = self.stable_ms.min(1500);
        self.clear_after_ms = fit(self.clear_after_ms, 50, 3000);
    }
}

impl Profile {
    /// Keep a profile within what the app uses (see Area::sanitize): its areas (ids the server takes,
    /// each once), style, page fonts and switches, timing, and a short printable name.
    pub fn sanitize(&mut self) {
        self.name = clean_text(&self.name, 60);
        let mut seen = std::collections::HashSet::new();
        self.areas.retain(|a| valid_area_id(&a.id) && seen.insert(a.id.clone()));
        for a in self.areas.iter_mut() {
            a.sanitize();
        }
        self.style.sanitize();
        let d = ProfileOpts::default();
        self.pages.title_font = clean_font(&self.pages.title_font);
        if self.pages.title_font.is_empty() {
            self.pages.title_font = d.title_font;
        }
        self.pages.text_font = clean_font(&self.pages.text_font);
        if self.pages.text_font.is_empty() {
            self.pages.text_font = d.text_font;
        }
        self.pages.more.retain(|k, _| !k.is_empty() && k.len() <= 32 && k.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'));
        self.timing.sanitize();
    }
}

impl Default for Profile {
    fn default() -> Self {
        builtin_profile("gta6").expect("built-in")
    }
}

/// The app's own built-in profiles, in the order they are listed (the server can add more games:
/// `Config::server_games`).
pub const BUILTINS: [&str; 1] = ["gta6"];

/// GTA VI's own text look: white, medium weight (the font sets it: not bold), a soft shadow, no
/// outline — measured against the game's subtitles on gameplay frames.
fn gta6_text() -> Style {
    Style { font_family: "Noto Sans Arabic Medium".into(), bold: false, outline_width: 0.0, shadow: true, ..Style::default() }
}

/// GTA VI's subtitles and objectives: white with a thin dark border, no shadow (like the game's own).
fn gta6_bordered() -> Style {
    Style { outline_width: 1.2, outline_color: "#000000".into(), shadow: false, ..gta6_text() }
}

/// A built-in profile as the app ships it.
pub fn builtin_profile(id: &str) -> Option<Profile> {
    let gta_areas = || {
        vec![
            Area {
                id: "subtitles".into(),
                name: "Subtitles".into(),
                centered_only: true,
                // up to two lines (two speakers), at ~0.84–0.92 of the height; it ends above the step
                // dots and the instructions GTA VI shows under the subtitles
                x: 0.10,
                y: 0.80,
                w: 0.80,
                h: 0.118,
                // A fixed place, whatever the English does: above the bottom instructions' Arabic
                // line, which sits right above the highest the game's lines go (two lines with an
                // instruction under them, ~0.83); grows upward, so it never covers anything and
                // doesn't jump when a line or an instruction comes or goes.
                placement: "custom".into(),
                out_x: 0.10,
                out_y: 0.585,
                out_w: 0.80,
                out_h: 0.20,
                out_valign: "bottom".into(),
                out_halign: "center".into(),
                text_pos: "bottom-center".into(),
                background: "none".into(),
                font_family: "Noto Sans Arabic Medium".into(),
                style: Some(gta6_bordered()),
                // matched size × 104% looks the same as GTA's subtitles (calibrated on real frames)
                font_scale: 1.04,
                ..Area::default()
            },
            Area {
                id: "help".into(),
                name: "Instructions (top left)".into(),
                // GTA VI's instructions box starts at ~0.27 of the height and grows downward: four lines
                // reach ~0.41 (seen on gameplay frames), so the area goes down to 0.48
                x: 0.01,
                y: 0.18,
                w: 0.30,
                h: HELP_H,
                // right under GTA's own box: same left edge, at most its width, growing downward
                placement: "under_text".into(),
                text_pos: "top-left".into(),
                font_family: "Tajawal".into(),
                style: Some(gta6_text()),
                // square, dark box like GTA's own instructions box
                background: "game".into(),
                // matched size × 116% looks the same as GTA's instruction text: the size measured is the
                // median word's (19 px at 1080p: most of its words are short, lower-case), the text's
                // capitals are 24 px (92 % had the Arabic a fifth smaller than the English)
                font_scale: 1.16,
                // the opening credits' names in this corner have no box: not help text
                needs_box: true,
                ..Area::default()
            },
            Area {
                id: "instructions".into(),
                name: "Instructions (bottom)".into(),
                // GTA VI's instructions at the very bottom, under the subtitles and the step dots
                // ("Use Ⓡ to aim at the teller"): centred, one line, with button icons
                centered_only: true,
                x: 0.20,
                y: 0.935,
                w: 0.60,
                h: 0.06,
                // A fixed line right above the highest the game's subtitles go (~0.83), the subtitles'
                // Arabic above it: nothing covers the English, and nothing jumps.
                placement: "custom".into(),
                out_x: 0.10,
                out_y: 0.765,
                out_w: 0.80,
                out_h: 0.05,
                out_valign: "bottom".into(),
                out_halign: "center".into(),
                background: "none".into(), // like the game: no box, white with pink words
                // set apart from the subtitles above it: Tajawal, bold
                font_family: "Tajawal".into(),
                style: Some(Style { bold: true, ..gta6_bordered() }),
                font_scale: 0.92,
                ..Area::default()
            },
        ]
    };
    match id {
        // A starting guess (subtitles at the bottom, instructions top left): layout updates from the
        // server set GTA VI's real places, and its pages come with its pages script.
        "gta6" => Some(Profile {
            id: "gta6".into(),
            name: "Grand Theft Auto VI".into(),
            game: "gta6".into(),
            areas: gta_areas(),
            style: Style::default(),
            pages: ProfileOpts::default(),
            timing: Timing::default(),
            fx: Default::default(),
        }),
        _ => None,
    }
}

/// The game profile's pages: which are translated, and in what fonts.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ProfileOpts {
    /// The switches the game's pages script declares (`pages.option`), by name; one not set here
    /// takes the script's default.
    pub more: std::collections::BTreeMap<String, bool>,
    /// Font for big titles such as "فشلت المهمة".
    pub title_font: String,
    /// Font for the smaller screen text (reason, buttons).
    pub text_font: String,
}

impl Default for ProfileOpts {
    fn default() -> Self {
        Self { more: Default::default(), title_font: "Lalezar".into(), text_font: "Tajawal".into() }
    }
}

/// A part of a window (x, y, width, height fractions) as the app can use it: inside the window and at
/// least 5% of it each way; None for something unusable, or for (about) the whole window.
pub fn clean_crop(c: [f64; 4]) -> Option<[f64; 4]> {
    if c.iter().any(|v| !v.is_finite()) {
        return None;
    }
    let x = c[0].clamp(0.0, 0.95);
    let y = c[1].clamp(0.0, 0.95);
    let w = c[2].min(1.0 - x);
    let h = c[3].min(1.0 - y);
    if w < 0.05 || h < 0.05 {
        return None;
    }
    let whole = x < 0.002 && y < 0.002 && w > 0.996 && h > 0.996;
    (!whole).then_some([x, y, w, h])
}

impl Default for Config {
    fn default() -> Self {
        Self {
            window_title: "PS Remote Play".into(),
            source_crop: None,
            overlay_window: String::new(),
            overlay_crop: None,
            virtual_camera: false,
            card_sound: true,
            card_sound_input: String::new(),
            card_sound_volume: 100,
            snap_hotkey: String::new(),
            engine: "local".into(),
            shared_translations: true,
            deepl_key: String::new(),
            azure_key: String::new(),
            azure_region: String::new(),
            google_key: String::new(),
            libre_url: "http://localhost:5000".into(),
            libre_key: String::new(),
            claude_key: String::new(),
            claude_model: "claude-haiku-5-5".into(),
            openai_key: String::new(),
            openai_model: "gpt-6-luna".into(),
            dialect: "msa".into(),
            llm_notes: String::new(),
            interval_ms: DEFAULT_TIMING.interval_ms,
            stable_ms: DEFAULT_TIMING.stable_ms,
            clear_after_ms: DEFAULT_TIMING.clear_after_ms,
            timing_custom: Some(false),
            overlays_enabled: true,
            device_overlays: true,
            obs_overlays: true,
            phone_text: true,
            phone_bubble: None,
            live_translations: true,
            fps_badge: false,
            fps_corner: "top-right".into(),
            theme: "dark".into(),
            target_lang: "ar".into(),
            ui_lang: "en".into(),
            update_check: true,
            lan_page: false,
            lan_port: 8791,
            install_id: String::new(),
            server_url: String::new(),
            layout_version: 0,
            layout_versions: Default::default(),
            game_profiles: Default::default(),
            server_games: Vec::new(),
            games_version: 0,
            obs_port: 4455,
            obs_password: String::new(),
            obs_page_port: 8790,
            capture_fps: DEFAULT_TIMING.capture_fps,
            preview_fps: DEFAULT_TIMING.preview_fps,
            picture_4k: false,
            snap_service: String::new(),
            snap_azure_endpoint: String::new(),
            snap_azure_key: String::new(),
            snap_gemini_model: "gemini-nano-banana-2.1".into(),
            snap_gemini_key: String::new(),
            shown: cover_shown(),
            lan_control: true,
            snap_save: true,
            snap_lan: true,
            snap_style: "blur".into(),
            debug_save_frames: false,
            debug_frame_interval_ms: 1000,
            profile: "gta6".into(),
            profile_rev: 0,
            unreadable: false,
            phone: false,
            defaults_version: DEFAULTS_VERSION,
            defaults_moved: false,
            profiles: Vec::new(),
            builtins: BUILTINS.iter().filter_map(|id| builtin_profile(id)).collect(),
            // 0 here too: a settings file from before profiles has no such field and takes this
            // value, which is what tells `normalize` to move its setup into a profile
            profiles_version: 0,
            profile_opts: builtin_profile("gta6").expect("built-in").pages,
            areas: builtin_profile("gta6").expect("built-in").areas,
            style: Style::default(),
            area_fx: Default::default(),
            fx_defaults: Default::default(),
            fx_view: Default::default(),
        }
    }
}

/// Changes of the app's defaults that settings saved before them take too, where the player had kept
/// the old default: 1 = text is read from 12 frames a second (was 8); 2 = GTA VI's top-left
/// instructions are read only inside the game's own box (settings saved before `needs_box` existed had
/// it off: menu text and credits there were translated); 3 = that area reaches lower (h 0.20 → 0.30):
/// a four-line box's last line was cut off, so the translation stopped short and covered it;
/// 4 = the live preview at 60 frames a second (was 30: a capture card sends 60, and the preview looked
/// choppy next to the card's own app).
pub const DEFAULTS_VERSION: u32 = 7;

/// The app's timing (Source → Timing): frames captured and shown per second, how often the areas are
/// read, the wait for text to settle, and when the English counts as gone (ms). Fast enough for GTA
/// VI's stacked subtitles and its black box over the English.
pub struct DefaultTiming {
    pub capture_fps: u32,
    pub preview_fps: u32,
    pub interval_ms: u64,
    pub stable_ms: u64,
    pub clear_after_ms: u64,
}
pub const DEFAULT_TIMING: DefaultTiming = DefaultTiming { capture_fps: 60, preview_fps: 60, interval_ms: 16, stable_ms: 80, clear_after_ms: 300 };
/// A phone's timing: reading every frame as the desktop does kept a phone's processor and text
/// recognizer busy without a break and made it hot. A fifth of a second between readings is still
/// quick for subtitles, which stay up for a second at least (a short dropout doesn't end a line).
pub const PHONE_TIMING: DefaultTiming = DefaultTiming { capture_fps: 12, preview_fps: 30, interval_ms: 200, stable_ms: 80, clear_after_ms: 300 };

/// The built-in GTA VI profile with its look "Over the game's text" (the black box over the English).
fn cover_shown() -> std::collections::BTreeMap<String, Shown> {
    let mut shown = std::collections::BTreeMap::new();
    let mut s = Shown::default();
    s.pages.insert("translation_cover".into(), true);
    shown.insert("gta6".to_string(), s);
    shown
}

/// The height of GTA VI's top-left instructions area (and the one before `DEFAULTS_VERSION` 3).
const HELP_H: f64 = 0.30;
const OLD_HELP_H: f64 = 0.20;

impl Config {
    /// Moves settings still on an old default to the new one, once (`defaults_version`); a value the
    /// player chose stays. True when something was moved.
    pub fn move_to_new_defaults(&mut self) -> bool {
        let mut moved = false;
        if self.defaults_version < 1 && self.capture_fps == 8 {
            self.capture_fps = 12;
            moved = true;
        }
        if self.defaults_version < 2 {
            for p in self.builtins.iter_mut().filter(|p| p.id == "gta6") {
                for a in p.areas.iter_mut().filter(|a| a.id == "help" && !a.needs_box) {
                    a.needs_box = true;
                    moved = true;
                }
            }
        }
        if self.defaults_version < 3 {
            // (the areas in use: only while they are the built-in profile's; a player's own copy is theirs)
            let gta6 = self.profile == "gta6";
            let old = |a: &Area| a.id == "help" && (a.h - OLD_HELP_H).abs() < 1e-9;
            for p in self.builtins.iter_mut().filter(|p| p.id == "gta6") {
                for a in p.areas.iter_mut().filter(|a| old(a)) {
                    a.h = HELP_H;
                    moved = true;
                }
            }
            if gta6 {
                for a in self.areas.iter_mut().filter(|a| old(a)) {
                    a.h = HELP_H;
                    moved = true;
                }
            }
        }
        if self.defaults_version < 4 && self.preview_fps == 30 {
            self.preview_fps = 60;
            moved = true;
        }
        if self.defaults_version < 5 {
            // GTA VI's top-left instructions: 116 % of the matched size, not 92 % (the Arabic was a fifth
            // smaller than the English); in the built-in profile, and in use while it's the one in use
            let gta6 = self.profile == "gta6";
            let old = |a: &Area| a.id == "help" && (a.font_scale - 0.92).abs() < 1e-9;
            for p in self.builtins.iter_mut().filter(|p| p.id == "gta6") {
                for a in p.areas.iter_mut().filter(|a| old(a)) {
                    a.font_scale = 1.16;
                    moved = true;
                }
            }
            if gta6 {
                for a in self.areas.iter_mut().filter(|a| old(a)) {
                    a.font_scale = 1.16;
                    moved = true;
                }
            }
        }
        if self.defaults_version < 6 {
            // v1.4.0, for everyone once: the built-in GTA VI profile, its look over the game's text,
            // and the app's timing (the profile's own when it has one)
            self.profile = "gta6".into();
            self.shown.entry("gta6".into()).or_default().pages.insert("translation_cover".into(), true);
            self.capture_fps = DEFAULT_TIMING.capture_fps;
            self.preview_fps = DEFAULT_TIMING.preview_fps;
            self.picture_4k = false;
            self.interval_ms = DEFAULT_TIMING.interval_ms;
            self.stable_ms = DEFAULT_TIMING.stable_ms;
            self.clear_after_ms = DEFAULT_TIMING.clear_after_ms;
            self.timing_custom = Some(false);
            moved = true;
        }
        if self.defaults_version < 7 {
            // v1.4.4, once: the translation's fade-in off (it was 150 ms). Saved settings held 150 for any
            // area whose effects were ever changed — the old default written with them, not the player's
            // choice: not set now (off, or the game's default from the server); another value the player
            // picked stays
            for f in self.area_fx.values_mut() {
                if f.fade_ms == Some(150) {
                    f.fade_ms = None;
                    moved = true;
                }
            }
        }
        let bumped = self.defaults_version < DEFAULTS_VERSION;
        self.defaults_version = self.defaults_version.max(DEFAULTS_VERSION);
        moved || bumped
    }

    pub fn load(path: &PathBuf) -> Self {
        // A settings file that's there but can't be used is never written over by the defaults: kept
        // aside (config.broken-<time>.json), or — when even that fails (another program holds it) —
        // left alone, and nothing is saved over it this time (`unreadable`).
        let unusable = |why: String| -> Self {
            let aside = path.with_extension(format!("broken-{}.json", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)));
            let mut d = Self::default();
            match std::fs::rename(path, &aside) {
                Ok(()) => eprintln!("[config] {} could not be read ({why}); kept as {}", path.display(), aside.display()),
                Err(e) => {
                    eprintln!("[config] {} could not be read ({why}) nor kept aside ({e}): not saved over this time", path.display());
                    d.unreadable = true;
                }
            }
            d
        };
        let mut c: Self = match std::fs::read(path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::default(),
            // (a file saved with a byte-order mark by an editor reads as well)
            Ok(bytes) => match std::str::from_utf8(&bytes).map_err(|e| e.to_string()).and_then(|t| serde_json::from_str::<Self>(t.trim_start_matches('\u{feff}')).map_err(|e| e.to_string())) {
                Ok(c) => c,
                Err(e) => unusable(e),
            },
            Err(e) => unusable(e.to_string()),
        };
        // A settings file from before `timing_custom`: a value moved off the app's default was the
        // player's own timing (that was the rule then).
        if c.timing_custom.is_none() {
            let d = Self::default();
            c.timing_custom = Some(c.interval_ms != d.interval_ms || c.stable_ms != d.stable_ms || c.clear_after_ms != d.clear_after_ms);
        }
        // A settings file from before `phone_bubble`: the bubble as the phone was (one switch for both then).
        if c.phone_bubble.is_none() {
            c.phone_bubble = Some(c.phone_text);
        }
        if !c.unreadable {
            c.defaults_moved = c.move_to_new_defaults();
        }
        c.normalize();
        c
    }

    /// Reading timing in use (interval, settle, clear after; ms): the player's own where they moved
    /// the slider (Source → Timing), else the profile's game timing if it has one, else the app's.
    /// The app's own timing for where it runs: a phone's, or the desktop's.
    pub fn default_timing(&self) -> &'static DefaultTiming {
        if self.phone { &PHONE_TIMING } else { &DEFAULT_TIMING }
    }

    pub fn timing(&self) -> (u64, u64, u64) {
        let t = self.builtins.iter().chain(self.profiles.iter()).find(|p| p.id == self.profile).map(|p| p.timing.clone()).unwrap_or_default();
        // the player's own timing, or the game profile's (each value it has; 0 = it has none)
        let own = self.timing_custom == Some(true);
        let pick = |mine: u64, game: u64| if own || game == 0 { mine } else { game };
        // (not their own: the app's current defaults, not values saved by an older version — a
        // default that changes reaches every player who never chose their own)
        let d = self.default_timing();
        let (interval, stable, clear) = if own { (self.interval_ms, self.stable_ms, self.clear_after_ms) } else { (d.interval_ms, d.stable_ms, d.clear_after_ms) };
        // (within the Timing sliders' ranges, whatever a settings file or a layout says)
        (
            pick(interval, t.interval_ms).clamp(if self.phone { 100 } else { 16 }, 1000),
            pick(stable, t.stable_ms).min(1500),
            pick(clear, t.clear_after_ms).clamp(50, 3000),
        )
    }

    /// A built-in profile (the app's own, or a game the server added)?
    pub fn is_builtin(&self, id: &str) -> bool {
        BUILTINS.contains(&id) || self.server_games.iter().any(|g| g == id)
    }

    /// The game the profile in use is for ("gta6" | one the server added).
    pub fn game(&self) -> String {
        if self.is_builtin(&self.profile) {
            return self.profile.clone();
        }
        self.profiles.iter().find(|p| p.id == self.profile).map(|p| p.game.clone()).unwrap_or_else(|| "gta6".into())
    }

    /// Keep the profiles consistent (after loading, and after the window saves):
    ///  - every built-in profile is there;
    ///  - the profile in use exists, and the settings the app reads (areas, style, game pages) are
    ///    that profile's: a built-in one exactly as it is (it can't be changed), one of the
    ///    player's as they last saved it.
    pub fn normalize(&mut self) {
        // Timing the player never chose is the app's current default (shown so on the Timing page too)
        let d = self.default_timing();
        if self.timing_custom != Some(true) {
            (self.interval_ms, self.stable_ms, self.clear_after_ms) = (d.interval_ms, d.stable_ms, d.clear_after_ms);
        }
        // (a phone: frame rates within its own sliders' ranges, 1–30)
        if self.phone {
            if self.capture_fps > 30 { self.capture_fps = d.capture_fps; }
            if self.preview_fps > 30 { self.preview_fps = d.preview_fps; }
        }
        self.source_crop = self.source_crop.and_then(clean_crop);
        self.overlay_crop = self.overlay_crop.and_then(clean_crop);
        for id in BUILTINS {
            if !self.builtins.iter().any(|p| p.id == id) {
                self.builtins.push(builtin_profile(id).expect("built-in"));
            }
        }
        let server_games = std::mem::take(&mut self.server_games);
        self.server_games = server_games.into_iter().filter(|g| self.builtins.iter().any(|p| &p.id == g)).collect();
        let keep: Vec<String> = self.builtins.iter().map(|p| p.id.clone()).filter(|id| self.is_builtin(id)).collect();
        self.builtins.retain(|p| keep.contains(&p.id));
        // (one per id: the first)
        let mut seen = std::collections::HashSet::new();
        self.builtins.retain(|p| seen.insert(p.id.clone()));
        for p in self.builtins.iter_mut() {
            p.game = p.id.clone();
            if let Some(d) = builtin_profile(&p.id) {
                p.name = d.name; // (built-in names aren't the player's to change)
            }
        }
        // (settings from before profiles: their read areas were GTA V's, which the app no longer has)
        self.profiles_version = 1;
        // The single layout version from before per-game ones was GTA V's, which the app no longer
        // has: it says nothing about GTA VI's layouts (an earlier build credited it to GTA VI, which
        // would skip GTA VI's first layouts: forgotten once, so they apply).
        if self.layout_version > 0 {
            self.layout_version = 0;
            self.layout_versions.clear();
        }
        // what a settings file or the server says about built-in profiles stays within what the app uses
        for p in self.builtins.iter_mut() {
            p.sanitize();
        }
        // ids are unique; names are never empty
        let mut seen = std::collections::HashSet::new();
        let builtin_ids: Vec<String> = self.builtins.iter().map(|p| p.id.clone()).collect();
        self.profiles.retain(|p| !builtin_ids.contains(&p.id) && !p.id.is_empty() && seen.insert(p.id.clone()));
        for p in self.profiles.iter_mut() {
            // (the player's own: names, fonts and numbers within what the app uses, as built-in ones')
            p.sanitize();
            if p.name.trim().is_empty() {
                p.name = "My profile".into();
            }
            // (a game the app no longer has, like GTA V: the copy is GTA VI's now)
            if !builtin_ids.contains(&p.game) {
                p.game = "gta6".into();
            }
        }
        // the profile each game was last played with (where switching to that game goes)
        let ids: Vec<String> = self.builtins.iter().chain(self.profiles.iter()).map(|p| p.id.clone()).collect();
        self.game_profiles.retain(|_, p| ids.contains(p));
        let active = self
            .builtins
            .iter()
            .chain(self.profiles.iter())
            .find(|p| p.id == self.profile)
            .cloned()
            .unwrap_or_else(|| self.builtins.iter().find(|p| p.id == "gta6").cloned().expect("built-in"));
        self.profile = active.id.clone();
        self.game_profiles.insert(active.game.clone(), active.id.clone());
        // (the game's defaults: its built-in profile's, which layout updates tune — a copy follows them too)
        self.fx_defaults = self.builtins.iter().find(|p| p.id == active.game).map(|p| p.fx.clone()).unwrap_or_default();
        for d in self.fx_defaults.values_mut() {
            d.fade_ms = d.fade_ms.map(|ms| ms.min(2000));
        }
        self.areas = active.areas;
        self.style = active.style;
        self.profile_opts = active.pages;
        // what the player showed or hid in this profile (a built-in one too)
        if let Some(s) = self.shown.get(&self.profile) {
            for a in self.areas.iter_mut() {
                if let Some(&on) = s.areas.get(&a.id) {
                    a.enabled = on;
                }
            }
            for (k, &on) in &s.pages {
                self.profile_opts.more.insert(k.clone(), on);
            }
        }
        self.fx_view = self.fx_all();
    }

    /// Show or hide a read area ("area") or a game page's switch ("page") of the profile in use, from
    /// anywhere (the window, the home-network page): kept in `shown`, and in the profile itself when it
    /// is the player's own. False for anything else.
    pub fn set_shown(&mut self, kind: &str, id: &str, on: bool) -> bool {
        let entry = self.shown.entry(self.profile.clone()).or_default();
        match kind {
            "area" => entry.areas.insert(id.to_string(), on),
            "page" => entry.pages.insert(id.to_string(), on),
            _ => return false,
        };
        if let Some(p) = self.profiles.iter_mut().find(|p| p.id == self.profile) {
            match kind {
                "area" => p.areas.iter_mut().filter(|a| a.id == id).for_each(|a| a.enabled = on),
                _ => {
                    p.pages.more.insert(id.to_string(), on);
                }
            }
        }
        self.normalize();
        true
    }

    /// The settings without secrets (translator keys, the OBS password, the install id): what goes
    /// to windows other than the main one, and in events (any window can listen to those).
    pub fn redacted(&self) -> Config {
        let mut c = self.clone();
        for s in [&mut c.deepl_key, &mut c.azure_key, &mut c.google_key, &mut c.libre_key, &mut c.claude_key, &mut c.openai_key, &mut c.snap_azure_key, &mut c.snap_gemini_key, &mut c.obs_password, &mut c.install_id] {
            s.clear();
        }
        c
    }

    /// The server's list of games (the layout update named "games"):
    /// `{"games": [{"id": "rdr2", "name": "Red Dead Redemption 2", "areas": [...], "style": {...}, "pages": {...}}]}`,
    /// each a built-in profile for a game the app doesn't carry itself. A newer list adds those games
    /// and replaces the ones it added before (the app's own games are never replaced; a game the list
    /// no longer has stays, so a profile in use never disappears). The games it changed, for their
    /// layout updates to be applied again.
    pub fn apply_games(&mut self, list: &serde_json::Value, version: u32) -> Vec<String> {
        if version <= self.games_version {
            return Vec::new();
        }
        let mut changed = Vec::new();
        for g in list["games"].as_array().into_iter().flatten() {
            let Ok(mut p) = serde_json::from_value::<Profile>(g.clone()) else { continue };
            let ok_id = !p.id.is_empty() && p.id.len() <= 16 && p.id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit());
            // (from the server, not signed: within what the app uses, a short printable name)
            p.sanitize();
            if !ok_id || BUILTINS.contains(&p.id.as_str()) || p.name.is_empty() {
                continue;
            }
            // a profile of the player's with that id (unlikely: theirs are "custom-…") gives way
            if self.profiles.iter().any(|x| x.id == p.id) {
                continue;
            }
            p.game = p.id.clone();
            self.builtins.retain(|x| x.id != p.id);
            self.builtins.push(p.clone());
            if !self.server_games.contains(&p.id) {
                self.server_games.push(p.id.clone());
            }
            // its layout update applies again over the new profile
            self.layout_versions.remove(&p.id);
            changed.push(p.id);
        }
        self.games_version = version;
        self.normalize();
        changed
    }

    pub fn save(&self, path: &PathBuf) -> anyhow::Result<()> {
        if self.unreadable {
            anyhow::bail!("the settings file couldn't be read when the app started: it isn't written over (restart the app)");
        }
        // (whole or not at all: a crash midway never leaves a half-written settings file)
        write_atomic(path, serde_json::to_string_pretty(self)?.as_bytes())?;
        Ok(())
    }
}

#[cfg(test)]
mod profile_tests {
    use super::*;

    #[test]
    fn the_message_bubble_switch_starts_as_the_phone_was() {
        let dir = std::env::temp_dir().join(format!("vit-bubble-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.json");
        // no settings yet: on, as the phone
        let c = Config::load(&dir.join("none.json"));
        assert_eq!(c.phone_bubble, Some(true));
        assert!(c.phone_text);
        // a settings file from before the switch: the bubble as the phone was
        for (phone, bubble) in [(true, true), (false, false)] {
            std::fs::write(&path, format!(r#"{{"phone_text": {phone}}}"#)).unwrap();
            let c = Config::load(&path);
            assert_eq!((c.phone_text, c.phone_bubble), (phone, Some(bubble)));
        }
        // set apart from the phone: kept
        std::fs::write(&path, r#"{"phone_text": false, "phone_bubble": true}"#).unwrap();
        let c = Config::load(&path);
        assert_eq!((c.phone_text, c.phone_bubble), (false, Some(true)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn whats_translated_switches_are_one_thing_each() {
        let list = content_switches("gta6");
        let mut seen = std::collections::HashSet::new();
        for s in &list {
            assert!(seen.insert(s.id), "{} twice", s.id);
            // exactly one thing: an area, a page switch or a setting (none overlapping another)
            assert_eq!(s.areas.len() + s.pages.len() + usize::from(!s.setting.is_empty()), 1, "{}", s.id);
            assert!(["phone", "menus", "notices", "screens"].contains(&s.group), "{}", s.id);
        }
        // the areas are GTA VI's own, shown by default; the subtitles in none (always translated)
        let gta6 = builtin_profile("gta6").unwrap();
        for id in list.iter().flat_map(|s| s.areas.iter()) {
            assert!(gta6.areas.iter().any(|a| a.id == *id && a.enabled), "{id}");
            assert_ne!(*id, "subtitles");
        }
        // the phone's settings: on by default
        let c = Config::default();
        assert!(c.phone_text && c.phone_bubble.unwrap_or(c.phone_text));
        assert!(content_switches("rdr2").is_empty());
    }

    #[test]
    fn ready_translations_are_on_unless_turned_off() {
        // a settings file from before the switch: on, as the app worked
        let old: Config = serde_json::from_str(r#"{"engine": "deepl"}"#).unwrap();
        assert!(old.shared_translations && Config::default().shared_translations);
        assert!(old.reviewed_only("instructions") && !old.reviewed_only("subtitles"));
        // off: kept, and no area waits for reviewed lines that aren't used (the instructions go to the translator)
        let mut c: Config = serde_json::from_str(r#"{"shared_translations": false}"#).unwrap();
        assert!(!c.shared_translations);
        assert!(!c.reviewed_only("instructions"));
        let back: Config = serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
        assert!(!back.shared_translations);
        c.shared_translations = true;
        assert!(c.reviewed_only("instructions"));
    }

    #[test]
    fn the_fade_in_is_off_unless_chosen_and_150_saved_before_v7_is_let_go() {
        // a settings file from v1.4.3: the old default 150 written with any area's effects, 250 chosen
        let mut c: Config = serde_json::from_str(r#"{"defaults_version": 6, "area_fx": {
            "subtitles": {"cover_min": true, "cover_early": true, "fade_ms": 150, "accent": true},
            "help": {"cover_min": true, "cover_early": true, "fade_ms": 250}}}"#).unwrap();
        assert!(c.move_to_new_defaults());
        c.normalize();
        assert_eq!(c.area_fx["subtitles"].fade_ms, None, "the old default: not the player's");
        assert_eq!(c.area_fx["subtitles"].accent, Some(true), "the rest kept");
        assert_eq!(c.fx("subtitles").fade_ms, Some(0), "off");
        assert_eq!(c.fx("help").fade_ms, Some(250), "chosen: kept");
        assert_eq!(c.fx("instructions").fade_ms, Some(0), "never set: off");
        assert_eq!(c.fx_view["subtitles"].fade_ms, Some(0));
        // only once: 150 chosen after it stays
        c.area_fx.get_mut("subtitles").unwrap().fade_ms = Some(150);
        assert!(!c.move_to_new_defaults());
        assert_eq!(c.fx("subtitles").fade_ms, Some(150));
        // and saved without a fade it isn't given one
        let back: Config = serde_json::from_str(&serde_json::to_string(&Config::default()).unwrap()).unwrap();
        assert!(back.area_fx.is_empty());
    }

    #[test]
    fn the_games_fade_default_applies_where_the_player_chose_none() {
        let mut c = Config::default();
        c.builtins.iter_mut().find(|p| p.id == "gta6").unwrap().fx.insert("subtitles".into(), FxDefaults { fade_ms: Some(300) });
        c.area_fx.insert("help".into(), AreaFx { fade_ms: Some(100), ..AreaFx::default() });
        c.area_fx.insert("subtitles".into(), AreaFx { accent: Some(true), ..AreaFx::default() });
        c.normalize();
        assert_eq!(c.fx("subtitles").fade_ms, Some(300), "the game's default (another option set doesn't fix the fade)");
        assert_eq!(c.fx_view["subtitles"].fade_ms, Some(300));
        assert_eq!(c.fx("help").fade_ms, Some(100), "the player's own");
        c.area_fx.get_mut("subtitles").unwrap().fade_ms = Some(0);
        c.normalize();
        assert_eq!(c.fx("subtitles").fade_ms, Some(0), "the player's off beats the game's default");
        // a copy of the profile follows its game's defaults too
        c.area_fx.clear();
        c.profiles.push(Profile { id: "custom-1".into(), name: "Mine".into(), ..Profile::default() });
        c.profile = "custom-1".into();
        c.normalize();
        assert_eq!(c.fx("subtitles").fade_ms, Some(300));
    }

    #[test]
    fn what_the_server_sends_is_kept_to_what_the_app_uses() {
        let mut c = Config::default();
        c.normalize();
        let list = serde_json::json!({"games": [{
            "id": "rdr2", "name": "Red Dead\u{0007} <b>Redemption</b> 2",
            "style": {"text_color": "#fff\"><img src=x onerror=alert(1)>", "bg_color": "#112233", "font_family": "x';background:url(//e)", "font_size": 9999.0},
            "areas": [{"id": "phone texts", "x": 0.1}, {"id": "subtitles", "x": 5.0, "font_family": "Tajawal", "placement": "<script>"}, {"id": "subtitles", "x": 0.2}],
            "pages": {"title_font": "Lalezar", "more": {"camp": false, "Bad Key": true}},
            "timing": {"interval_ms": 1, "clear_after_ms": 999999}
        }]});
        assert_eq!(c.apply_games(&list, 1), vec!["rdr2".to_string()]);
        let p = c.builtins.iter().find(|p| p.id == "rdr2").unwrap();
        assert_eq!(p.name, "Red Dead <b>Redemption</b> 2", "printable (the window escapes it), no control characters");
        assert_eq!(p.style.text_color, "#ffffff");
        assert_eq!(p.style.bg_color, "#112233");
        assert_eq!(p.style.font_family, "Segoe UI");
        assert_eq!(p.style.font_size, 96.0);
        assert_eq!(p.areas.len(), 1, "an id line uploads can't take, and a repeated one, go");
        assert_eq!((p.areas[0].x, p.areas[0].font_family.as_str(), p.areas[0].placement.as_str()), (1.0, "Tajawal", "above"));
        assert_eq!(p.pages.more.keys().collect::<Vec<_>>(), vec!["camp"]);
        assert_eq!((p.timing.interval_ms, p.timing.clear_after_ms), (16, 3000));
    }

    #[test]
    fn old_defaults_move_once() {
        // a file from before: the old default moves, once
        let mut c: Config = serde_json::from_str(r#"{"capture_fps": 8}"#).unwrap();
        assert_eq!(c.defaults_version, 0);
        assert!(c.move_to_new_defaults());
        assert_eq!((c.capture_fps, c.defaults_version), (60, DEFAULTS_VERSION));
        // set back to 8 by the player afterwards: stays
        c.capture_fps = 8;
        assert!(!c.move_to_new_defaults());
        assert_eq!(c.capture_fps, 8);
        // a new install already has the new defaults
        assert_eq!((Config::default().capture_fps, Config::default().defaults_version), (60, DEFAULTS_VERSION));
    }

    #[test]
    fn version_1_4_0_moves_everyone_to_gta6_over_the_text_with_the_new_timing() {
        // a player on their own profile, the look under the text, slow timing of their own, 4K
        let mut c: Config = serde_json::from_str(
            r#"{"defaults_version": 5, "profile": "mine", "interval_ms": 400, "stable_ms": 500, "clear_after_ms": 900,
                "timing_custom": true, "capture_fps": 12, "preview_fps": 30, "picture_4k": true,
                "shown": {"gta6": {"pages": {"translation_cover": false}}}}"#,
        )
        .unwrap();
        assert!(c.move_to_new_defaults());
        c.normalize();
        assert_eq!(c.profile, "gta6");
        assert!(c.cover_look());
        assert_eq!((c.interval_ms, c.stable_ms, c.clear_after_ms, c.capture_fps, c.preview_fps, c.picture_4k), (16, 80, 300, 60, 60, false));
        assert_eq!(c.timing(), (16, 80, 300));
        // once: what they choose afterwards stays (moving a Timing slider makes the timing their own)
        c.interval_ms = 200;
        c.timing_custom = Some(true);
        c.shown.get_mut("gta6").unwrap().pages.insert("translation_cover".into(), false);
        assert!(!c.move_to_new_defaults());
        c.normalize();
        assert_eq!((c.interval_ms, c.cover_look()), (200, false));
        // a new install has it all already
        let mut d = Config::default();
        d.normalize();
        assert!(d.cover_look());
        assert_eq!(d.timing(), (16, 80, 300));
    }

    #[test]
    fn areas_and_pages_can_be_hidden_in_a_built_in_profile() {
        let mut c = Config::default();
        c.normalize();
        assert!(c.is_builtin(&c.profile));
        assert!(c.areas.iter().find(|a| a.id == "help").unwrap().enabled);
        assert!(c.set_shown("area", "help", false));
        assert!(c.set_shown("page", "weapon_wheel", false));
        // (saving copies the built-in profile in again: the choice stays)
        c.normalize();
        assert!(!c.areas.iter().find(|a| a.id == "help").unwrap().enabled);
        assert_eq!(c.profile_opts.more.get("weapon_wheel"), Some(&false));
        // the built-in profile itself is unchanged
        assert!(c.builtins.iter().find(|p| p.id == "gta6").unwrap().areas.iter().find(|a| a.id == "help").unwrap().enabled);
        assert!(!c.set_shown("colour", "x", true));
    }

    #[test]
    fn gta6_help_needs_its_box_after_the_update() {
        let mut c = Config::default();
        c.normalize();
        for p in c.builtins.iter_mut() {
            for a in p.areas.iter_mut() {
                a.needs_box = false;
            }
        }
        c.defaults_version = 1;
        assert!(c.move_to_new_defaults());
        let gta6 = c.builtins.iter().find(|p| p.id == "gta6").unwrap();
        assert!(gta6.areas.iter().find(|a| a.id == "help").unwrap().needs_box);
        assert!(!gta6.areas.iter().find(|a| a.id == "subtitles").unwrap().needs_box);
    }

    #[test]
    fn a_window_part_is_kept_usable() {
        assert_eq!(clean_crop([0.1, 0.1, 0.5, 0.5]), Some([0.1, 0.1, 0.5, 0.5]));
        assert_eq!(clean_crop([0.0, 0.0, 1.0, 1.0]), None, "the whole window is no part");
        assert_eq!(clean_crop([0.5, 0.5, 0.9, 0.9]), Some([0.5, 0.5, 0.5, 0.5]), "kept inside the window");
        assert_eq!(clean_crop([0.2, 0.2, 0.01, 0.5]), None, "too thin");
        assert_eq!(clean_crop([f64::NAN, 0.0, 0.5, 0.5]), None);
    }

    #[test]
    fn gta6_help_reaches_lower_after_the_update() {
        let mut c = Config::default();
        c.normalize();
        let set = |c: &mut Config, h: f64| {
            for p in c.builtins.iter_mut() {
                p.areas.iter_mut().filter(|a| a.id == "help").for_each(|a| a.h = h);
            }
            c.areas.iter_mut().filter(|a| a.id == "help").for_each(|a| a.h = h);
        };
        set(&mut c, 0.20);
        c.defaults_version = 2;
        assert!(c.move_to_new_defaults());
        let gta6 = c.builtins.iter().find(|p| p.id == "gta6").unwrap();
        assert_eq!(gta6.areas.iter().find(|a| a.id == "help").unwrap().h, 0.30);
        // a height the player chose stays
        set(&mut c, 0.25);
        c.defaults_version = 2;
        c.move_to_new_defaults();
        assert_eq!(c.builtins.iter().find(|p| p.id == "gta6").unwrap().areas.iter().find(|a| a.id == "help").unwrap().h, 0.25);
    }

    #[test]
    fn the_old_layout_number_is_not_gta_vis() {
        // (an earlier build moved GTA V's single layout version to GTA VI)
        let mut c: Config = serde_json::from_str(r#"{"layout_version": 7, "layout_versions": {"gta6": 7}}"#).unwrap();
        c.normalize();
        assert_eq!(c.layout_version, 0);
        assert!(c.layout_versions.is_empty());
        // and the per-game ones after that stay
        c.layout_versions.insert("gta6".into(), 2);
        c.normalize();
        assert_eq!(c.layout_versions.get("gta6"), Some(&2));
    }

    #[test]
    fn settings_are_written_whole() {
        let dir = std::env::temp_dir().join(format!("vit-cfg-test-{}", std::process::id()));
        let path = dir.join("config.json");
        let mut c = Config::default();
        c.deepl_key = "k".into();
        c.save(&path).unwrap();
        assert_eq!(Config::load(&path).deepl_key, "k");
        assert!(std::fs::read_dir(&dir).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().ends_with(".tmp")), "no temporary file left");
        // a file that can't be read is kept aside, not overwritten
        std::fs::write(&path, "{ half a file").unwrap();
        let fresh = Config::load(&path);
        assert_eq!(fresh.deepl_key, "");
        assert!(!fresh.unreadable, "kept aside: a new file may be written");
        let broken = || std::fs::read_dir(&dir).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().contains("broken")).count();
        assert_eq!(broken(), 1);
        // ...and so is one an editor saved in another encoding (not UTF-8)
        std::thread::sleep(std::time::Duration::from_millis(1100)); // (a new name: one per second)
        std::fs::write(&path, b"{\"deepl_key\": \"caf\xe9\"}").unwrap();
        assert_eq!(Config::load(&path).deepl_key, "");
        assert_eq!(broken(), 2);
        assert!(!path.exists(), "moved aside, not left to be written over");
        // a byte-order mark (Notepad's "UTF-8 with BOM") reads fine
        std::fs::write(&path, "\u{feff}{\"deepl_key\": \"k2\"}").unwrap();
        assert_eq!(Config::load(&path).deepl_key, "k2");
        // settings that couldn't be read are never saved over the file
        let stuck = Config { unreadable: true, ..Config::default() };
        assert!(stuck.save(&path).is_err());
        assert_eq!(Config::load(&path).deepl_key, "k2");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_games_timing_unless_the_player_set_their_own() {
        let mut c = Config::default();
        c.normalize();
        let d = Config::default();
        assert_eq!(c.timing(), (d.interval_ms, d.stable_ms, d.clear_after_ms));
        c.builtins.iter_mut().find(|p| p.id == c.profile).unwrap().timing = Timing { interval_ms: 0, stable_ms: 120, clear_after_ms: 1500 };
        assert_eq!(c.timing(), (d.interval_ms, 120, 1500), "the game's, where it has one");
        // the player's own: all three are theirs
        c.timing_custom = Some(true);
        c.clear_after_ms = 900;
        assert_eq!(c.timing(), (d.interval_ms, d.stable_ms, 900));
        // back to the game's (their values are kept for later, but not used)
        c.timing_custom = Some(false);
        assert_eq!(c.timing(), (d.interval_ms, 120, 1500));
    }

    #[test]
    fn a_phone_reads_at_its_own_pace() {
        // a phone's settings saved with the desktop's pace (every 16 ms, 60 fps), not its own
        let mut c: Config = serde_json::from_str(r#"{"defaults_version": 6, "interval_ms": 16, "capture_fps": 60, "preview_fps": 60, "timing_custom": false}"#).unwrap();
        c.phone = true;
        c.normalize();
        assert_eq!(c.timing(), (PHONE_TIMING.interval_ms, PHONE_TIMING.stable_ms, PHONE_TIMING.clear_after_ms));
        assert_eq!((c.capture_fps, c.preview_fps), (12, 30));
        // a phone player's own timing stays theirs, within the phone's range
        let mut c: Config = serde_json::from_str(r#"{"defaults_version": 6, "interval_ms": 16, "timing_custom": true}"#).unwrap();
        c.phone = true;
        c.normalize();
        assert_eq!(c.timing().0, 100);
        // the desktop keeps its own pace
        let mut d = Config::default();
        d.normalize();
        assert_eq!(d.timing().0, DEFAULT_TIMING.interval_ms);
    }

    #[test]
    fn timing_the_player_never_chose_follows_the_new_default() {
        // saved by an older version with the old default, not their own
        let mut c: Config = serde_json::from_str(r#"{"defaults_version": 6, "clear_after_ms": 100, "timing_custom": false}"#).unwrap();
        c.normalize();
        assert_eq!((c.clear_after_ms, c.timing().2), (DEFAULT_TIMING.clear_after_ms, DEFAULT_TIMING.clear_after_ms));
        // their own stays theirs
        let mut c: Config = serde_json::from_str(r#"{"defaults_version": 6, "stable_ms": 0, "clear_after_ms": 100, "timing_custom": true}"#).unwrap();
        c.normalize();
        assert_eq!((c.stable_ms, c.clear_after_ms), (0, 100));
    }

    #[test]
    fn older_settings_keep_the_players_own_timing() {
        let dir = std::env::temp_dir().join(format!("vit-timing-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("config.json");
        // from before v1.4.0: everyone's timing became the app's once (defaults v6), their own or not
        std::fs::write(&path, r#"{"clear_after_ms": 900}"#).unwrap();
        let c = Config::load(&path);
        assert_eq!((c.timing_custom, c.clear_after_ms), (Some(false), DEFAULT_TIMING.clear_after_ms));
        std::fs::write(&path, r#"{}"#).unwrap();
        assert_eq!(Config::load(&path).timing_custom, Some(false));
        // since then, their own stays their own
        std::fs::write(&path, r#"{"defaults_version": 6, "clear_after_ms": 900, "timing_custom": true}"#).unwrap();
        let c = Config::load(&path);
        assert_eq!((c.timing_custom, c.clear_after_ms), (Some(true), 900));
        // chosen since: kept as it is, even with values off the default
        std::fs::write(&path, r#"{"clear_after_ms": 900, "timing_custom": false}"#).unwrap();
        assert_eq!(Config::load(&path).timing_custom, Some(false));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_server_adds_games() {
        let mut c = Config::default();
        c.normalize();
        let list = serde_json::json!({"games": [
            {"id": "rdr2", "name": "Red Dead Redemption 2", "areas": [], "pages": {"more": {"camp": false}}},
            {"id": "gta6", "name": "Not this one"},
            {"id": "Bad Id", "name": "x"}
        ]});
        assert_eq!(c.apply_games(&list, 3), vec!["rdr2".to_string()]);
        assert!(c.is_builtin("rdr2") && !c.is_builtin("custom-1"));
        assert_eq!(c.builtins.iter().find(|p| p.id == "gta6").unwrap().name, "Grand Theft Auto VI");
        assert!(c.apply_games(&list, 3).is_empty(), "same version: nothing new");
        c.profile = "rdr2".into();
        c.normalize();
        assert_eq!((c.game().as_str(), c.areas.len(), c.profile_opts.more.get("camp")), ("rdr2", 0, Some(&false)));
        // kept across a save and a load
        let back: Config = serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
        let mut back = back;
        back.normalize();
        assert_eq!(back.profile, "rdr2");
        // a copy of it is for that game
        back.profiles.push(Profile { id: "custom-9".into(), name: "Mine".into(), game: "rdr2".into(), ..Profile::default() });
        back.normalize();
        assert_eq!(back.profiles[0].game, "rdr2");
        // a newer list that drops it: the game stays
        assert!(back.apply_games(&serde_json::json!({"games": []}), 4).is_empty());
        assert!(back.is_builtin("rdr2"));
    }

    #[test]
    fn a_new_install_starts_on_the_built_in_gta_vi() {
        let mut c = Config::default();
        c.normalize();
        assert_eq!(c.profile, "gta6");
        assert!(c.profiles.is_empty(), "no copy is made for a new install");
        // and loading a file saved by it doesn't make one later either
        let mut again: Config = serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
        again.normalize();
        assert!(again.profiles.is_empty());
    }

    #[test]
    fn gta_v_is_gone_and_its_settings_move_to_gta_vi() {
        // GTA V in use, and a copy of it: GTA VI's built-in profile, and the copy becomes GTA VI's
        let mut c: Config = serde_json::from_str(r#"{"profile":"gta5","profiles_version":1,
            "builtins":[{"id":"gta5","name":"Grand Theft Auto V","game":"gta5"}],
            "profiles":[{"id":"custom-1","name":"My GTA V","game":"gta5"}],
            "profile_opts":{"mission_failed":true,"pause_menu":false}}"#).unwrap();
        c.normalize();
        assert_eq!(c.profile, "gta6");
        assert_eq!(c.builtins.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(), vec!["gta6"]);
        assert_eq!(c.profiles[0].game, "gta6");
        // a file from before profiles
        let mut old: Config = serde_json::from_str(r#"{"profile":"gta5","areas":[{"id":"subtitles","name":"Subtitles","y":0.5}]}"#).unwrap();
        old.normalize();
        assert_eq!((old.profile.as_str(), old.profiles_version), ("gta6", 1));
    }

    #[test]
    fn a_built_in_profile_cant_be_changed() {
        let mut c = Config::default();
        c.profile = "gta6".into();
        c.normalize();
        // the window sends changed areas while a built-in profile is in use: they don't stick
        c.areas[0].x = 0.9;
        c.style.font_size = 99.0;
        c.normalize();
        assert_eq!(c.areas[0].x, builtin_profile("gta6").unwrap().areas[0].x);
        assert_eq!(c.style.font_size, Style::default().font_size);
        // the built-in itself can't be renamed or made another game's either
        c.builtins[0].name = "Mine".into();
        c.builtins[0].game = "x".into();
        c.normalize();
        assert!(c.builtins.iter().all(|p| p.game == p.id && builtin_profile(&p.id).unwrap().name == p.name));
    }

    #[test]
    fn the_players_profile_keeps_their_changes_and_a_missing_one_falls_back() {
        let mut c = Config::default();
        let mut mine = builtin_profile("gta6").unwrap();
        mine.id = "custom-x".into();
        mine.name = "Mine".into();
        mine.areas[0].h = 0.3;
        c.profiles.push(mine);
        c.profile = "custom-x".into();
        c.normalize();
        assert_eq!(c.areas[0].h, 0.3);
        c.profile = "gone".into();
        c.normalize();
        assert_eq!(c.profile, "gta6", "an unknown profile falls back to a built-in one");
    }

    #[test]
    fn a_profile_with_an_arabic_name_keeps_its_name_and_fonts() {
        let mut c = Config::default();
        let mut mine = builtin_profile("gta6").unwrap();
        mine.id = "custom-ar".into();
        // (an override left open after the name, as pasted from a chat app)
        mine.name = "جي تي ايه السادسة (نسخة)\u{202e}".into();
        mine.areas[0].font_family = "(A) Arslan Wessam A".into();
        mine.areas[1].font_family = "خط النسخ العربي".into();
        mine.style.font_family = "Noto Naskh Arabic UI".into();
        mine.pages.title_font = "Bahij Muna".into();
        c.profiles.push(mine);
        c.profile = "custom-ar".into();
        c.normalize();
        let p = c.profiles.iter().find(|p| p.id == "custom-ar").unwrap();
        assert_eq!(p.name, "جي تي ايه السادسة (نسخة)");
        assert_eq!((c.areas[0].font_family.as_str(), c.areas[1].font_family.as_str()), ("(A) Arslan Wessam A", "خط النسخ العربي"));
        assert_eq!((c.style.font_family.as_str(), c.profile_opts.title_font.as_str()), ("Noto Naskh Arabic UI", "Bahij Muna"));
        // saved and read back the same
        let back: Config = serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
        assert_eq!(back.profiles.iter().find(|p| p.id == "custom-ar").unwrap().name, p.name);
    }

    #[test]
    fn a_font_name_cant_end_the_css_string_it_goes_in() {
        for bad in ["Evil\"; color: red", "a\\b", "x{y}", "<b>", "Arial\u{202e}"] {
            assert_eq!(clean_font(bad), "", "{bad:?}");
        }
        for good in ["Segoe UI", "Bahnschrift SemiLight", "(A) Arslan Wessam A", "Font & Co.", "Kufi+", "خط", "Sakkal Majalla"] {
            assert_eq!(clean_font(good), good);
        }
    }
}
