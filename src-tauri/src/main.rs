#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod capture;
mod card_sound;
mod debug_frames;
// capture cards: Media Foundation on Windows, nokhwa (V4L2 / AVFoundation) elsewhere
#[cfg_attr(not(windows), path = "device_nokhwa.rs")]
mod device;
mod obs;
mod obs_page;
// installed fonts: DirectWrite on Windows, fontdb elsewhere
#[cfg_attr(not(windows), path = "fonts_fontdb.rs")]
mod fonts;
mod hotkey;
// the platform's OCR engine, for the core's text reading
mod ocr_engine;
mod overlay;
mod picture_find;
mod preview_shot;
#[cfg(debug_assertions)]
mod dev_debug;
mod lan;
mod updater;
mod vcam;
// Xbox remote play in a window of the app (Microsoft's web player)
mod xbox;
// PS5 remote play in the app (chiaki-ng)
mod ps5;

// The reading and translating are the core's (shared with the phone apps); its modules read as this
// crate's own (`crate::config`, `crate::ocr` …).
use vi_translator_core::{config, engine, langs, memory, ocr, pagescript, screens, snapshot, speaker, sync, translate, tv};


use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::capture::Shared;
use crate::config::Config;
use crate::ocr::{Crop, Ocr};
use crate::ocr_engine::PlatformOcr;
use crate::translate::Translator;

pub const APP_TITLE: &str = "VI Translator";
const IDENTIFIER: &str = "com.vitranslator.app";
/// The folder name in Documents and Pictures.
const FOLDER: &str = "VI Translator";
/// Where earlier versions kept their files (newest first): moved over on first start.
const OLD_NAMES: [(&str, &str); 2] = [("com.gta6translator.app", "GTA 6 Translator"), ("com.arabicsubs.app", "Arabic Subs")];

fn copy_dir(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let dest = to.join(e.file_name());
        if e.file_type()?.is_dir() {
            copy_dir(&e.path(), &dest)?;
        } else {
            std::fs::copy(e.path(), dest)?;
        }
    }
    Ok(())
}

/// Move the settings, downloaded model, translation memory and recordings of the app's earlier
/// names ("GTA 6 Translator", "Arabic Subs") to the new folders, once. The newest old folder that
/// exists wins; nothing is moved if the new folder exists.
fn move_old_folders() {
    let appdata = app_data_root();
    let mut pairs = Vec::new();
    for (id, folder) in OLD_NAMES {
        pairs.push((appdata.join(id), data_dir()));
        for dir in [documents_dir(), pictures_dir()].into_iter().flatten() {
            pairs.push((dir.join(folder), dir.join(FOLDER)));
        }
    }
    for (old, new) in pairs {
        if old.is_dir() && !new.exists() {
            // (a folder open in Explorer can't be moved: copy it instead, and leave the old one)
            if std::fs::rename(&old, &new).is_err() {
                if let Err(e) = copy_dir(&old, &new) {
                    eprintln!("could not copy {} to {}: {e}", old.display(), new.display());
                }
            }
        }
    }
}

/// Where apps keep their settings: %APPDATA% on Windows, ~/Library/Application Support on macOS,
/// ~/.local/share on Linux.
fn app_data_root() -> PathBuf {
    #[cfg(windows)]
    return PathBuf::from(std::env::var("APPDATA").unwrap_or_else(|_| ".".into()));
    #[cfg(not(windows))]
    return dirs::data_dir().unwrap_or_else(|| PathBuf::from("."));
}
fn data_dir() -> PathBuf {
    app_data_root().join(IDENTIFIER)
}
fn config_path() -> PathBuf {
    data_dir().join("config.json")
}
/// Debugging: where "Save frames" recordings go (one sub-folder per recording).
fn frames_dir() -> PathBuf {
    // Pictures\VI Translator\debug-frames: easy to find, and (unlike AppData) never redirected by Windows
    // into a private copy when the app is started from a packaged app.
    pictures_dir().map(|p| p.join(FOLDER).join("debug-frames")).unwrap_or_else(|| data_dir().join("debug-frames"))
}

/// The user's Pictures folder (follows OneDrive / moved folders).
fn pictures_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    return known_folder(&windows::Win32::UI::Shell::FOLDERID_Pictures);
    #[cfg(not(windows))]
    return dirs::picture_dir();
}

/// The user's Documents folder.
fn documents_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    return known_folder(&windows::Win32::UI::Shell::FOLDERID_Documents);
    #[cfg(not(windows))]
    return dirs::document_dir();
}

/// Translation memory and lines log: Documents\VI Translator\translation-db (not AppData, which
/// Windows may redirect into a private copy).
fn db_dir() -> PathBuf {
    documents_dir()
        .map(|p| p.join(FOLDER).join("translation-db"))
        .unwrap_or_else(|| data_dir().join("translation-db"))
}

#[cfg(windows)]
fn known_folder(id: &windows::core::GUID) -> Option<PathBuf> {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{KF_FLAG_DEFAULT, SHGetKnownFolderPath};
    unsafe {
        let p = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None).ok()?;
        let path = p.to_string().ok().map(PathBuf::from);
        CoTaskMemFree(Some(p.0 as *const _));
        path
    }
}
fn model_dir() -> PathBuf {
    paths().model_dir()
}

/// The folders and settings file, for the core.
fn paths() -> sync::Paths {
    sync::Paths { data: data_dir(), db: db_dir(), config: config_path() }
}

struct AppState {
    cfg: Arc<Mutex<Config>>,
    shared: Arc<Shared>,
    saver: Arc<debug_frames::SaverState>,
    memory: Arc<Mutex<memory::Memory>>,
    /// a newer version found by the updater
    update: Arc<Mutex<Option<updater::Update>>>,
}


/// Commands that change things or reach outside the app answer the main window only: an overlay
/// window shows text the game showed, and has no business asking for more.
fn main_only(w: &tauri::Webview) -> Result<(), String> {
    if w.label() == "main" { Ok(()) } else { Err("not allowed in this window".into()) }
}

/// Translation memory: on/off, size, hits/misses and the time saved.
#[tauri::command]
fn memory_status(state: tauri::State<AppState>) -> memory::Stats {
    state.memory.lock().unwrap().stats()
}

/// A line's translation from the memory, in the language in use, as plain text (the Game profile's
/// look previews show a real game line): None when the memory doesn't have it.
/// GTA VI's phone's translations now up (see the core's engine), for its overlay window when it opens.
#[tauri::command]
fn get_phone(state: tauri::State<AppState>) -> Option<serde_json::Value> {
    state.shared.phone.lock().unwrap().clone()
}

/// What the memory takes a line for, and its translation and where it came from (for checking a
/// reading the app showed: dev builds' console).
#[tauri::command]
fn memory_probe(state: tauri::State<AppState>, line: String) -> serde_json::Value {
    let m = state.memory.lock().unwrap();
    let got = m.get(&line, &[], &[]);
    serde_json::json!({
        "line_of": m.line_of(&line),
        "ar": got.as_ref().map(|(segs, _)| segs.iter().map(|s| s.0.as_str()).collect::<String>()),
        "source": got.map(|(_, src)| src),
    })
}

#[tauri::command]
fn memory_line(state: tauri::State<AppState>, line: String) -> Option<String> {
    let m = state.memory.lock().unwrap();
    m.get(&line, &[], &[]).map(|(segs, _)| segs.iter().map(|s| s.0.as_str()).collect::<String>().trim().to_string())
}

#[tauri::command]
fn memory_clear(webview: tauri::Webview, state: tauri::State<AppState>) -> Result<(), String> {
    main_only(&webview)?;
    state.memory.lock().unwrap().clear();
    Ok(())
}

#[derive(Serialize, Clone)]
struct FrameEvent {
    /// a picture is coming in (the preview itself comes from /preview)
    live: bool,
    /// full frame size (what areas are fractions of)
    w: u32,
    h: u32,
    status: String,
    /// where the live preview streams from
    preview_url: String,
}

// ------------------------------------------------------------------------------------- commands

/// The languages the app can translate into.
#[tauri::command]
fn list_langs() -> Vec<langs::Lang> {
    langs::LANGS.to_vec()
}

/// This build's version (from the release tag).
#[tauri::command]
fn app_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// A development build (`cargo tauri dev`, debug): the Show on tab shows the tools for developing
/// the app. Releases never do.
#[tauri::command]
fn dev_build() -> bool {
    cfg!(debug_assertions)
}

/// Dev builds' debug server: a page's answer to an /eval (dev_debug.rs). Nothing in a release build.
#[tauri::command]
fn dev_report(id: String, value: serde_json::Value) -> Result<(), String> {
    #[cfg(debug_assertions)]
    {
        dev_debug::report(id, value);
        Ok(())
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = (id, value);
        Err("dev builds only".into())
    }
}

/// Whether this build has the free offline translator (not on Intel Macs: see Cargo.toml).
#[tauri::command]
fn has_offline() -> bool {
    vi_translator_core::translate::HAS_OFFLINE
}

/// A newer version of the app, if one was found.
#[tauri::command]
fn update_status(state: tauri::State<AppState>) -> Option<updater::Update> {
    state.update.lock().unwrap().clone()
}

/// Download and check the newer version, put it in place and restart into it.
#[tauri::command]
async fn install_update(app: AppHandle, webview: tauri::Webview, state: tauri::State<'_, AppState>) -> Result<(), String> {
    main_only(&webview)?;
    let u = state.update.lock().unwrap().clone().ok_or("no update available")?;
    // VI Translator.app (the .dmg's): its update is the new .dmg, on the release's page
    if u.app_bundle {
        return std::process::Command::new("open").arg(&u.page).spawn().map(|_| ()).map_err(|e| format!("could not open {}: {e}", u.page));
    }
    tauri::async_runtime::spawn_blocking(move || updater::install(&u)).await.map_err(|e| e.to_string())?.map_err(|e| format!("{e:#}"))?;
    app.restart()
}

/// The home-network page: on/off, its address and a QR code of it.
#[tauri::command]
fn lan_info(webview: tauri::Webview, state: tauri::State<AppState>) -> Result<lan::LanInfo, String> {
    main_only(&webview)?;
    let c = state.cfg.lock().unwrap().clone();
    Ok(lan::info(&c, &state.shared))
}

/// TVs on the home network waiting to be connected (the VI Translator app on the TV), and whether each is
/// connected already.
#[tauri::command]
async fn find_tvs(webview: tauri::Webview, state: tauri::State<'_, AppState>) -> Result<Vec<tv::FoundTv>, String> {
    main_only(&webview)?;
    let shared = state.shared.clone();
    tauri::async_runtime::spawn_blocking(move || tv::find(&shared.tvs)).await.map_err(|e| e.to_string())
}

/// Connect the TV at `address` with the code it shows. Its name, or why not ("wrong_code",
/// "too_many", "no_answer", "not_sharing").
#[tauri::command]
async fn connect_tv(webview: tauri::Webview, state: tauri::State<'_, AppState>, address: String, code: String) -> Result<String, String> {
    main_only(&webview)?;
    let (c, shared) = (state.cfg.lock().unwrap().clone(), state.shared.clone());
    // the TV loads its page from the home-network sharing, which has to be on (the window turns it on first)
    let running = shared.lan_running.load(std::sync::atomic::Ordering::Relaxed);
    let ip = lan::lan_ip();
    let (true, true, Some(ip)) = (c.lan_page, running, ip) else { return Err(tv::PairError::NotSharing.code().into()) };
    let pc = format!("{ip}:{}", c.lan_port);
    tauri::async_runtime::spawn_blocking(move || tv::pair(&shared.tvs, &address, &code, &pc, &tv::pc_name()))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.code().to_string())
}

/// The connected TVs (names only).
#[tauri::command]
fn list_tvs(webview: tauri::Webview, state: tauri::State<AppState>) -> Result<Vec<tv::TvInfo>, String> {
    main_only(&webview)?;
    Ok(state.shared.tvs.list())
}

/// Disconnect a TV: its key stops working, and the TV shows a new code.
#[tauri::command]
fn remove_tv(webview: tauri::Webview, state: tauri::State<AppState>, id: String) -> Result<(), String> {
    main_only(&webview)?;
    state.shared.tvs.remove_id(&id);
    Ok(())
}

/// Opens the home-network page in the default browser (only that address: the page passes none).
#[tauri::command]
fn open_lan_page(webview: tauri::Webview, state: tauri::State<AppState>) -> Result<(), String> {
    main_only(&webview)?;
    let c = state.cfg.lock().unwrap().clone();
    let url = lan::info(&c, &state.shared).url.ok_or("The page is not running")?;
    open_url(&url)
}

/// Opens an address in the default browser.
pub fn open_url(url: &str) -> Result<(), String> {
    // Windows: the shell's own "open" (the default browser), as Explorer does for a link
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::UI::Shell::ShellExecuteW;
        use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
        use windows::core::{HSTRING, w};
        let r = unsafe { ShellExecuteW(None, w!("open"), &HSTRING::from(url), None, None, SW_SHOWNORMAL) };
        // (a value above 32 means it worked)
        if r.0 as isize > 32 { Ok(()) } else { Err(format!("could not open {url}")) }
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(url).spawn().map(|_| ()).map_err(|e| e.to_string())
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        std::process::Command::new("xdg-open").arg(url).spawn().map(|_| ()).map_err(|e| e.to_string())
    }
}

/// PS5 in the app: signed in, linked PS5, whether the stream wants the login passcode.
#[tauri::command]
fn ps5_status(webview: tauri::Webview) -> Result<serde_json::Value, String> {
    main_only(&webview)?;
    Ok(ps5::status())
}

/// Sony's sign-in page in a window of the app (for the account ID linking needs). (async: on Windows
/// a window made from a synchronous command stays blank)
#[tauri::command]
async fn ps5_sign_in(webview: tauri::Webview) -> Result<(), String> {
    main_only(&webview)?;
    ps5::sign_in(&data_dir())
}

/// Signing in with a browser instead: Sony's page opens there, and the address it ends on comes back.
#[tauri::command]
fn ps5_browser_sign_in(webview: tauri::Webview) -> Result<String, String> {
    main_only(&webview)?;
    let url = ps5::login_url_for_browser();
    open_url(&url)?;
    Ok(url)
}

/// Sony's sign-in address (to copy into a browser).
#[tauri::command]
fn ps5_login_url(webview: tauri::Webview) -> Result<String, String> {
    main_only(&webview)?;
    Ok(ps5::login_url_for_browser())
}

#[tauri::command]
async fn ps5_sign_in_address(webview: tauri::Webview, address: String) -> Result<(), String> {
    main_only(&webview)?;
    tauri::async_runtime::spawn_blocking(move || ps5::sign_in_with_address(&address)).await.map_err(|e| e.to_string())?
}

#[tauri::command]
fn ps5_set_account(webview: tauri::Webview, id: String) -> Result<(), String> {
    main_only(&webview)?;
    ps5::set_account_id(&id)
}

#[tauri::command]
fn ps5_wake(webview: tauri::Webview) -> Result<(), String> {
    main_only(&webview)?;
    ps5::wake()
}

/// The PS5's sound on this computer: on or off, and its volume (0–100).
#[tauri::command]
fn ps5_set_sound(webview: tauri::Webview, on: bool, volume: u32) -> Result<(), String> {
    main_only(&webview)?;
    ps5::set_sound(on, volume);
    Ok(())
}

#[tauri::command]
fn ps5_set_resolution(webview: tauri::Webview, resolution: u32) -> Result<(), String> {
    main_only(&webview)?;
    ps5::set_resolution(resolution);
    Ok(())
}

#[tauri::command]
fn ps5_sign_out(webview: tauri::Webview) -> Result<(), String> {
    main_only(&webview)?;
    ps5::sign_out();
    Ok(())
}

/// The PS5s on the home network.
#[tauri::command]
async fn ps5_find(webview: tauri::Webview) -> Result<Vec<ps5::Found>, String> {
    main_only(&webview)?;
    tauri::async_runtime::spawn_blocking(ps5::find).await.map_err(|e| e.to_string())?
}

/// Links the PS5 at `host` with the PIN it shows (Settings > System > Remote Play > Link Device).
#[tauri::command]
async fn ps5_link(webview: tauri::Webview, host: String, pin: String) -> Result<String, String> {
    main_only(&webview)?;
    tauri::async_runtime::spawn_blocking(move || ps5::link(&host, &pin)).await.map_err(|e| e.to_string())?
}

#[tauri::command]
fn ps5_forget(webview: tauri::Webview) -> Result<(), String> {
    main_only(&webview)?;
    ps5::forget();
    Ok(())
}

/// The PlayStation login passcode the PS5 asked for.
#[tauri::command]
fn ps5_login_pin(webview: tauri::Webview, pin: String) -> Result<(), String> {
    main_only(&webview)?;
    ps5::give_pin(&pin);
    Ok(())
}

/// Opens Xbox remote play in its window of the app (or brings it to the front). (async: on Windows a
/// window made from a synchronous command stays blank)
#[tauri::command]
async fn open_xbox(app: AppHandle, webview: tauri::Webview) -> Result<(), String> {
    main_only(&webview)?;
    xbox::open(&app, &data_dir())
}

/// The settings: whole for the main window; without secrets (translator keys, the OBS password, the
/// install id) for an overlay, which needs only areas, style and language.
#[tauri::command]
fn get_config(webview: tauri::Webview, state: tauri::State<AppState>) -> Config {
    let c = state.cfg.lock().unwrap();
    if webview.label() == "main" { c.clone() } else { c.redacted() }
}

/// The game pages a game's script has, and the switches it declares (for Game pages).
#[tauri::command]
fn page_options(game: String) -> serde_json::Value {
    pagescript::page_options(&game, &data_dir())
}

#[tauri::command]
fn save_config(app: AppHandle, webview: tauri::Webview, state: tauri::State<AppState>, config: Config) -> Result<(), String> {
    main_only(&webview)?;
    // (the profile in use decides the areas, style and pages; a built-in one can't be changed)
    let mut config = config;
    // One lock for the whole merge, write and event: a layout update or a game switch landing
    // between them can't be lost, nor the file and the event come out older than the settings.
    let mut now = state.cfg.lock().unwrap();
    // the built-in profiles are the app's (layout updates may have changed them meanwhile)
    config.builtins = now.builtins.clone();
    config.server_games = now.server_games.clone();
    config.games_version = now.games_version;
    config.layout_version = now.layout_version;
    config.layout_versions = now.layout_versions.clone();
    // (the defaults already moved stay moved: a window that left the field out can't undo it)
    config.defaults_version = config.defaults_version.max(now.defaults_version);
    // sent before the window heard of a game switch the app made by itself: the switch stays
    if config.profile_rev != now.profile_rev {
        config.profile = now.profile.clone();
        config.profile_rev = now.profile_rev;
    }
    // (a settings file that's there but couldn't be read stays as it is: see Config::load)
    config.unreadable = now.unreadable;
    config.normalize();
    *now = config.clone();
    // (the change is in use whether or not the file could be written: the overlays hear of it either way)
    let saved = config.save(&config_path()).map_err(|e| e.to_string());
    app.emit("config", &config.redacted()).map_err(|e| e.to_string())?;
    saved
}

#[tauri::command]
fn list_windows(webview: tauri::Webview) -> Result<Vec<capture::WindowInfo>, String> {
    main_only(&webview)?;
    Ok(other_windows())
}

/// Other apps' windows (the Xbox player's window is a source of its own).
fn other_windows() -> Vec<capture::WindowInfo> {
    let mut w = capture::list_windows(APP_TITLE);
    w.retain(|w| w.title != xbox::TITLE);
    w
}

/// The screens (for a whole screen as the source, or to draw the Arabic over).
#[tauri::command]
fn list_screens(webview: tauri::Webview) -> Result<Vec<capture::ScreenInfo>, String> {
    main_only(&webview)?;
    Ok(capture::list_screens())
}

/// Everything that can be a source: windows, capture devices, and OBS's sources (if OBS answers).
#[tauri::command]
async fn list_sources(webview: tauri::Webview, state: tauri::State<'_, AppState>) -> Result<serde_json::Value, String> {
    main_only(&webview)?;
    let (port, password) = {
        let c = state.cfg.lock().unwrap();
        (c.obs_port, c.obs_password.clone())
    };
    let (windows, devices, obs) = tauri::async_runtime::spawn_blocking(move || {
        let obs = match obs::list(port, &password) {
            Ok((list, canvas)) => serde_json::json!({
                "ok": true,
                "sources": list.into_iter().map(|(n, k)| serde_json::json!({ "name": n, "kind": k })).collect::<Vec<_>>(),
                "canvas": canvas.map(|(w, h)| serde_json::json!({ "w": w, "h": h })),
            }),
            Err(e) => serde_json::json!({ "ok": false, "error": format!("{e:#}") }),
        };
        (other_windows(), device::list(), obs)
    })
    .await
    .map_err(|e| e.to_string())?;
    // (the Xbox player: its own entry, whether its window is open or not)
    let xbox = xbox::available().then_some(xbox::TITLE);
    Ok(serde_json::json!({ "windows": windows, "devices": devices, "obs": obs, "screens": capture::list_screens(), "xbox": xbox, "ps5": ps5::PREFIX }))
}

/// A still of a whole window (all of it, whatever part of it is the source), to choose the part
/// showing the game: {"w", "h", "img": JPEG data URL}.
#[tauri::command]
async fn window_still(webview: tauri::Webview, title: String) -> Result<serde_json::Value, String> {
    main_only(&webview)?;
    tauri::async_runtime::spawn_blocking(move || {
        let f = capture::grab_once(&title).map_err(|e| format!("{e:#}"))?;
        let img = snapshot::jpeg_data_url(&f, 1600, 80).map_err(|e| format!("{e:#}"))?;
        Ok(serde_json::json!({ "w": f.w, "h": f.h, "img": img }))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// The part of a window showing the game (x, y, w, h fractions): what moves in it over a second or
/// so. None when too little moved (the game paused, a still menu).
#[tauri::command]
async fn find_picture(webview: tauri::Webview, title: String) -> Result<Option<[f64; 4]>, String> {
    main_only(&webview)?;
    tauri::async_runtime::spawn_blocking(move || {
        let mut frames = Vec::new();
        for i in 0..3 {
            if i > 0 {
                std::thread::sleep(Duration::from_millis(600));
            }
            frames.push(capture::grab_once(&title).map_err(|e| format!("{e:#}"))?);
        }
        let refs: Vec<&capture::FrameData> = frames.iter().map(|f| f.as_ref()).collect();
        Ok(picture_find::moving_part(&refs).and_then(config::clean_crop))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// The sound inputs (for the capture card's sound) and what that sound is doing now.
#[tauri::command]
async fn sound_inputs(state: tauri::State<'_, AppState>) -> Result<serde_json::Value, String> {
    let status = state.shared.sound_status.lock().unwrap().clone();
    let inputs = tauri::async_runtime::spawn_blocking(card_sound::list_inputs).await.map_err(|e| e.to_string())?;
    Ok(serde_json::json!({ "inputs": inputs, "status": status }))
}

#[tauri::command]
fn sound_status(state: tauri::State<AppState>) -> String {
    state.shared.sound_status.lock().unwrap().clone()
}

/// The snapshot's keyboard shortcut: in use now and kept in the settings, or why these keys can't be.
#[tauri::command]
fn set_snap_hotkey(app: AppHandle, webview: tauri::Webview, state: tauri::State<AppState>, keys: String) -> Result<(), String> {
    main_only(&webview)?;
    let keys = keys.trim().to_string();
    if let Err(e) = hotkey::set(&app, &keys) {
        // (the one in use before stays)
        let old = state.cfg.lock().unwrap().snap_hotkey.clone();
        let _ = hotkey::set(&app, &old);
        return Err(e);
    }
    let mut c = state.cfg.lock().unwrap();
    c.snap_hotkey = keys;
    c.save(&config_path()).map_err(|e| e.to_string())
}

/// The snapshot (opened by its shortcut) was closed: back to the game's window.
#[tauri::command]
fn back_to_game() {
    hotkey::back();
}

/// What the virtual camera is doing ("" while it's off).
#[tauri::command]
fn vcam_status(state: tauri::State<AppState>) -> String {
    vcam::status(&state.shared)
}

/// The profile screen on show right now (layout + Arabic reason), for windows opened after it appeared.
#[tauri::command]
fn get_screen(state: tauri::State<AppState>) -> serde_json::Value {
    serde_json::json!({
        "layout": state.shared.screen.lock().unwrap().clone(),
        "reason": state.shared.screen_reason.lock().unwrap().clone(),
    })
}

/// Debugging: state of the frame saver (on/off, frames saved, folder).
#[tauri::command]
fn frame_saver_status(state: tauri::State<AppState>) -> debug_frames::SaverEvent {
    let on = state.cfg.lock().unwrap().debug_save_frames;
    state.saver.event(on)
}

/// Debugging: open the last recording's folder (or the parent folder) in File Explorer.
#[tauri::command]
fn open_frames_folder(webview: tauri::Webview, state: tauri::State<AppState>) -> Result<String, String> {
    main_only(&webview)?;
    let dir = state.saver.folder.lock().unwrap().clone().unwrap_or_else(frames_dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    debug_frames::open_folder(&dir).map_err(|e| e.to_string())?;
    Ok(dir.display().to_string())
}

/// Debugging: save a picture of the preview (`area`: where it is in the page). The first one of a run makes a
/// new folder in Pictures\VI Translator\previews, the next ones go to the same folder. The file's path.
/// Next to it, "frame_NNN.png": the game's frame at the size the app captures and reads it (the preview's
/// picture is only as big as the preview on screen).
#[tauri::command]
async fn save_preview(webview: tauri::Webview, area: preview_shot::Area, state: tauri::State<'_, AppState>) -> Result<String, String> {
    main_only(&webview)?;
    let base = pictures_dir().map(|p| p.join(FOLDER).join("previews")).unwrap_or_else(|| data_dir().join("previews"));
    // (the frame first: the one the preview is showing now)
    let frame = state.shared.latest();
    let path = preview_shot::save(&webview, area, &base).await?;
    if let (Some(f), Some(name)) = (frame, path.file_name().and_then(|n| n.to_str())) {
        let full = path.with_file_name(name.replacen("preview_", "frame_", 1));
        debug_frames::save_png_fast(&f, &full).map_err(|e| format!("the full-size frame: {e:#}"))?;
    }
    Ok(path.display().to_string())
}

/// Where snapshots are kept: Pictures\VI Translator\snapshots.
fn snapshots_dir() -> PathBuf {
    pictures_dir().map(|p| p.join(FOLDER).join("snapshots")).unwrap_or_else(|| data_dir().join("snapshots"))
}

/// A snapshot of the picture on screen now: translated with the snapshot service (the core's
/// `snapshot::now`); with "Keep every snapshot", its picture is saved (and Azure's translated
/// picture; the app saves its own drawing of the others, see `save_snapshot_render`).
fn snapshot_now(cfg: &Mutex<Config>, shared: &Shared, memory: &Mutex<memory::Memory>) -> anyhow::Result<snapshot::Snapshot> {
    let Some(frame) = shared.latest() else { anyhow::bail!("no picture yet: pick a source first") };
    let private = shared.screen_private.lock().unwrap().clone();
    let c = cfg.lock().unwrap().clone();
    let id = debug_frames::timestamp();
    let ocr = || {
        Ocr::init_thread();
        Ocr::new()
    };
    let snap = snapshot::now(&frame, &private, &c, memory, &paths(), &ocr, id.clone())?;
    if c.snap_save {
        let dir = snapshots_dir();
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join(format!("{id}_original.png")), snapshot::png_bytes(&snapshot::covered(&frame, &private))?)?;
        if let Some((bytes, ext)) = snap.translated_image.as_deref().and_then(snapshot::data_url_bytes) {
            std::fs::write(dir.join(format!("{id}_{}.{ext}", snap.lang)), bytes)?;
        }
    }
    Ok(snap)
}

/// Translate the whole picture now (the Snapshot tab's button and Play's).
#[tauri::command]
async fn take_snapshot(webview: tauri::Webview, state: tauri::State<'_, AppState>) -> Result<snapshot::Snapshot, String> {
    main_only(&webview)?;
    let (cfg, shared, memory) = (state.cfg.clone(), state.shared.clone(), state.memory.clone());
    tauri::async_runtime::spawn_blocking(move || snapshot_now(&cfg, &shared, &memory).map_err(|e| format!("{e:#}")))
        .await
        .map_err(|e| e.to_string())?
}

/// The app's drawing of a snapshot (its picture with the translation over it), saved next to its picture.
#[tauri::command]
fn save_snapshot_render(webview: tauri::Webview, id: String, lang: String, data: String) -> Result<String, String> {
    main_only(&webview)?;
    // (names the app made: a timestamp and a language code)
    let ok = |t: &str, max: usize| !t.is_empty() && t.len() <= max && t.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if !ok(&id, 40) || !ok(&lang, 10) {
        return Err("bad name".into());
    }
    let (bytes, ext) = snapshot::data_url_bytes(&data).ok_or("not a picture")?;
    let dir = snapshots_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!("{id}_{lang}.{ext}"));
    std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
    Ok(path.display().to_string())
}

#[tauri::command]
fn open_snapshots_folder(webview: tauri::Webview) -> Result<String, String> {
    main_only(&webview)?;
    let dir = snapshots_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    debug_frames::open_folder(&dir).map_err(|e| e.to_string())?;
    Ok(dir.display().to_string())
}

/// Debugging: open the folder the preview pictures go to (this run's, or the one for all of them).
#[tauri::command]
fn open_previews_folder(webview: tauri::Webview) -> Result<String, String> {
    main_only(&webview)?;
    let dir = preview_shot::FOLDER.lock().unwrap().clone().unwrap_or_else(|| pictures_dir().map(|p| p.join(FOLDER).join("previews")).unwrap_or_else(|| data_dir().join("previews")));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    debug_frames::open_folder(&dir).map_err(|e| e.to_string())?;
    Ok(dir.display().to_string())
}

#[tauri::command]
fn list_fonts() -> Vec<fonts::FontInfo> {
    fonts::list_fonts()
}

// ------------------------------------------------------------------------------------- workers

/// The live preview: turns small preview frames into JPEG (only while the preview is watched; the
/// app's picture asks for them one by one at http://127.0.0.1:<port>/preview), and tells the main window
/// twice a second whether a picture is coming in, its size and the capture status.
fn preview_loop(app: AppHandle, cfg: Arc<Mutex<Config>>, shared: Arc<Shared>) {
    let mut last_raw: Option<Arc<capture::FrameData>> = None;
    let mut last_status = Instant::now() - Duration::from_secs(5);
    loop {
        std::thread::sleep(Duration::from_millis(5));
        let raw = shared.preview_raw.lock().unwrap().clone();
        // someone started watching, but the picture hasn't changed since: start from the last full frame
        let watching = shared.watching();
        let raw = match raw {
            None if watching && shared.preview_jpeg.lock().unwrap().is_none() && shared.preview_yuv.lock().unwrap().is_none() => shared.latest().map(|f| {
                let small = Arc::new(capture::shrink(&f.rgba, f.w, f.h, (f.w * 4) as usize, false));
                *shared.preview_raw.lock().unwrap() = Some(small.clone());
                small
            }),
            r => r,
        };
        if let Some(f) = raw.filter(|f| last_raw.as_ref().is_none_or(|l| !Arc::ptr_eq(l, f))) {
            last_raw = Some(f.clone());
            let t = Instant::now();
            // a fast (SIMD) encoder straight from RGBA: a full 1080p picture in a few ms, so the
            // preview can be full size without falling behind
            let mut jpeg = Vec::with_capacity(f.rgba.len() / 10);
            let enc = jpeg_encoder::Encoder::new(&mut jpeg, 85);
            if enc.encode(&f.rgba, f.w as u16, f.h as u16, jpeg_encoder::ColorType::Rgba).is_ok() {
                shared.put_preview_jpeg(jpeg);
            }
            if std::env::var_os("VI_TRANSLATOR_DEBUG").is_some() {
                eprintln!("[preview] {}x{} JPEG in {:.1} ms", f.w, f.h, t.elapsed().as_secs_f64() * 1000.0);
            }
        }
        if last_status.elapsed() >= Duration::from_millis(500) {
            last_status = Instant::now();
            let visible = app
                .get_webview_window("main")
                .is_some_and(|w| w.is_visible().unwrap_or(false) && !w.is_minimized().unwrap_or(false));
            shared.preview_paused.store(!visible, std::sync::atomic::Ordering::Relaxed);
            let frame = shared.latest();
            let status = shared.status.lock().unwrap().clone();
            let port = cfg.lock().unwrap().obs_page_port;
            let (w, h) = frame.as_ref().map(|f| (f.w, f.h)).unwrap_or((0, 0));
            let _ = app.emit_to(
                "main",
                "frame",
                FrameEvent { live: frame.is_some(), w, h, status, preview_url: format!("http://127.0.0.1:{port}/preview") },
            );
            if frame.is_none() {
                *shared.preview_raw.lock().unwrap() = None;
                *shared.preview_jpeg.lock().unwrap() = None;
                *shared.preview_yuv.lock().unwrap() = None;
            }
        }
    }
}


/// The window and overlays hear the core's events as Tauri events of the same name.
struct TauriSink(AppHandle);

impl vi_translator_core::Sink for TauriSink {
    fn emit(&self, event: &str, payload: serde_json::Value) {
        // (what the home-network page changed, and its snapshots: for the main window only)
        let _ = match event {
            "lan-control" | "snapshot" => self.0.emit_to("main", event, payload),
            _ => self.0.emit(event, payload),
        };
    }
}

/// Reads every enabled area of the latest frame, waits for text to be stable, then queues it for
/// translation (the core's reader: vi_translator_core::engine).
fn ocr_loop(app: AppHandle, ctx: engine::Context, shared: Arc<Shared>) {
    Ocr::init_thread();
    let ocr = match Ocr::new() {
        Ok(o) => o,
        Err(e) => {
            let _ = app.emit("status-msg", format!("OCR unavailable: {e}"));
            return;
        }
    };
    let mut reader = engine::start(ctx);
    let mut last_seq = 0u64;
    loop {
        // The next frame as soon as it's captured: after the wait, a frame read already is waited out
        // (2 ms at a time, up to a capture's worth), so a reading starts on a new frame, not on one
        // up to a frame old, and not twice on the same one. (No new frame for that long: the same
        // one again, for what's timed: a line that's gone, a page's checks.)
        let mut frame = shared.latest();
        let give_up = Instant::now() + Duration::from_millis(120);
        while frame.as_ref().is_some_and(|f| f.seq == last_seq) && Instant::now() < give_up {
            std::thread::sleep(Duration::from_millis(2));
            frame = shared.latest();
        }
        if let Some(f) = &frame {
            last_seq = f.seq;
        }
        let wait = match &frame {
            Some(f) => reader.step(engine::Input::Frame(f, &ocr)),
            None => reader.step(engine::Input::Nothing),
        };
        std::thread::sleep(wait);
    }
}

/// How the app keeps up, over the last minute (see vi_translator_core::perf).
#[tauri::command]
fn perf_status() -> serde_json::Value {
    vi_translator_core::perf::perf_status()
}

// ------------------------------------------------------------------------------------- test mode

/// `vi-translator --test-image shot.png [--area help]`: OCR + translate one screenshot and print the result.
/// See `--read-video`: frames come from ffmpeg (raw RGBA on its stdout, never saved).
/// `--phone-crops` (dev builds): the phone read in a saved frame, its pieces and their crops.
#[cfg(debug_assertions)]
fn phone_crops(file: &str, out: &str) -> anyhow::Result<()> {
    use vi_translator_core::phone;
    let img = image::open(file)?.to_rgba8();
    let (w, h) = img.dimensions();
    let rgba = img.as_raw();
    std::fs::create_dir_all(out)?;
    Ocr::init_thread();
    let ocr = Ocr::new()?;
    let scr = phone::find_screen(rgba, w, h)
        .or_else(|| phone::find_screen_unsure(rgba, w, h))
        .ok_or_else(|| anyhow::anyhow!("no phone screen found"))?;
    println!("screen {scr:?}");
    let read = phone::read_screen_any(&ocr, rgba, w, h, scr);
    println!("turn {:.2} upright {} slant {:?}", read.angle, read.upright.is_some(), phone::tilt_of(&read.pieces));
    println!("quad {:?}", read.quad.map(|q| q.map(|(x, y)| (x.round(), y.round()))));
    let corners = read.quad_in_pieces;
    println!("photos {:?}", read.photos);
    let photo_boxes = read.photos.clone();
    let pieces = read.pieces;
    let up_img = read.upright.map(|u| image::RgbaImage::from_raw(w, h, u).expect("frame size"));
    let img = up_img.unwrap_or(img);
    let rgba = img.as_raw();
    let mut marked = img.clone();
    let mut boxed = |r: (i32, i32, u32, u32), c: [u8; 4]| {
        for t in 0..2i32 {
            for xx in r.0..r.0 + r.2 as i32 {
                for yy in [r.1 + t, r.1 + r.3 as i32 - 1 - t] {
                    if xx >= 0 && yy >= 0 && (xx as u32) < w && (yy as u32) < h {
                        marked.put_pixel(xx as u32, yy as u32, image::Rgba(c));
                    }
                }
            }
            for yy in r.1..r.1 + r.3 as i32 {
                for xx in [r.0 + t, r.0 + r.2 as i32 - 1 - t] {
                    if xx >= 0 && yy >= 0 && (xx as u32) < w && (yy as u32) < h {
                        marked.put_pixel(xx as u32, yy as u32, image::Rgba(c));
                    }
                }
            }
        }
    };
    let save = |name: String, r: (u32, u32, u32, u32), o: (i32, i32, u32, u32), hh: u32| {
        if let Some(png) = phone::part_png(rgba, w, h, r, o, hh) {
            let _ = std::fs::write(std::path::Path::new(out).join(name), png);
        }
    };
    for (i, p) in pieces.iter().enumerate() {
        println!(
            "{i}: {}rect {:?} name {:?} en {:?} label {} avatar {:?} badge {:?} preview {:?} emoji {}",
            if p.title { "TITLE " } else { "" }, p.rect, p.name, p.en, p.look.label, p.avatar, p.badge, p.preview, p.emoji.len()
        );
        boxed((p.rect.0 as i32, p.rect.1 as i32, p.rect.2, p.rect.3), [255, 0, 0, 255]);
        let at = |o: (i32, i32, u32, u32)| (p.rect.0 as i32 + o.0, p.rect.1 as i32 + o.1, o.2, o.3);
        if let Some(a) = p.avatar {
            boxed(at((a.0, a.1, a.2, a.2)), [0, 200, 255, 255]);
            save(format!("{i:02}_avatar.png"), p.rect, (a.0, a.1, a.2, a.2), 56);
        }
        if let Some(b) = p.badge {
            boxed(at(b), [0, 220, 0, 255]);
            save(format!("{i:02}_badge.png"), p.rect, b, 28);
        }
        if let Some(b) = p.preview {
            boxed(at(b), [255, 160, 0, 255]);
            save(format!("{i:02}_preview.png"), p.rect, b, 28);
        }
        for (j, e) in p.emoji.iter().enumerate() {
            boxed((e.0 as i32, e.1 as i32, e.2, e.3), [255, 0, 255, 255]);
            save(format!("{i:02}_emoji{j}.png"), *e, (0, 0, e.2, e.3), 40);
        }
    }
    let (sx, sy, sw, sh) = scr;
    boxed((sx as i32, sy as i32, sw, sh), [255, 255, 0, 255]);
    for r in &photo_boxes {
        boxed((r.0 as i32, r.1 as i32, r.2, r.3), [0, 255, 0, 255]);
    }
    for (x, y) in corners.into_iter().flatten() {
        boxed((x as i32 - 6, y as i32 - 6, 12, 12), [255, 0, 255, 255]);
    }
    let cut = image::imageops::crop_imm(&marked, sx.saturating_sub(60), sy.saturating_sub(40), (sw + 120).min(w), (sh + 80).min(h)).to_image();
    cut.save(std::path::Path::new(out).join("marked.png"))?;
    Ok(())
}

fn read_video(file: &str, fps: f64, from: f64, to: f64) -> anyhow::Result<()> {
    use std::io::{Read, Write};
    let cfg = Config::load(&config_path());
    let areas: Vec<_> = cfg.areas.iter().filter(|a| a.enabled).cloned().collect();
    // the picture's size
    let probe = std::process::Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "v:0", "-show_entries", "stream=width,height", "-of", "csv=p=0", file])
        .output()?;
    let dims = String::from_utf8_lossy(&probe.stdout).trim().to_string();
    let (w, h) = dims.split_once(',').and_then(|(a, b)| Some((a.trim().parse::<u32>().ok()?, b.trim().parse::<u32>().ok()?))).ok_or_else(|| anyhow::anyhow!("no video size in {dims:?}"))?;
    let mut cmd = std::process::Command::new("ffmpeg");
    cmd.args(["-v", "error"]);
    if from > 0.0 {
        cmd.args(["-ss", &format!("{from}")]);
    }
    cmd.args(["-i", file]);
    if to > from {
        cmd.args(["-t", &format!("{}", to - from)]);
    }
    cmd.args(["-an", "-vf", &format!("fps={fps}"), "-f", "rawvideo", "-pix_fmt", "rgba", "-"]).stdout(std::process::Stdio::piped());
    let mut child = cmd.spawn()?;
    let mut pipe = child.stdout.take().ok_or_else(|| anyhow::anyhow!("no ffmpeg output"))?;
    Ocr::init_thread();
    let ocr = Ocr::new()?;
    let mut frame = vec![0u8; (w * h * 4) as usize];
    let out = std::io::stdout();
    let mut out = out.lock();
    let mut n = 0u64;
    while pipe.read_exact(&mut frame).is_ok() {
        let t = from + n as f64 / fps;
        n += 1;
        for area in &areas {
            let Some(crop) = Crop::from_frame(&frame, w, h, area.x, area.y, area.w, area.h) else { continue };
            let (mut words, sure) = ocr.recognize_checked(&crop)?;
            if area.centered_only {
                words = ocr::centered_words(&words, area.x.clamp(0.0, 1.0) * w as f64, w as f64);
            }
            let en = ocr::clean(&words);
            if en.is_empty() {
                continue;
            }
            let (en_icons, icons, block) = ocr::text_with_icons(&crop, &words);
            let phrases = ocr::colored_phrases(&crop, &words);
            let parts = if icons.is_empty() && area.centered_only { ocr::speaker_lines(&crop, &words) } else { Vec::new() };
            // where the text sits across the frame (areas that read only centred text skip the rest)
            let cx = block.map(|(x, _, bw, _)| (area.x * w as f64 + x as f64 + bw as f64 / 2.0) / w as f64);
            let line = serde_json::json!({
                "t": (t * 100.0).round() / 100.0, "area": area.id, "en": if icons.is_empty() { en } else { en_icons },
                "sure": sure, "icons": icons.len(), "cx": cx, "phrases": phrases,
                "parts": parts.iter().map(|(p, rel)| serde_json::json!([p, (rel * 100.0).round() / 100.0])).collect::<Vec<_>>(),
            });
            writeln!(out, "{line}")?;
        }
        if n % 600 == 0 {
            eprintln!("read-video: {:.0} min", t / 60.0);
            out.flush()?;
        }
    }
    let _ = child.wait();
    eprintln!("read-video: {n} frames");
    Ok(())
}

/// One English line translated outside the running app (--test-image, --translate-frames) the way
/// the app does it: a speaker name or "In Spanish:" kept apart, the memory and the downloaded reviewed
/// lines first, then the translator; harsh words softened.
fn offline_line(
    tr: &mut Translator,
    cfg: &Config,
    mem: &memory::Memory,
    en: &str,
    icons: &[String],
    phrases: &[(String, String)],
    log: &dyn Fn(String),
) -> anyhow::Result<translate::Segments> {
    let one = |tr: &mut Translator, line: &str, phrases: &[(String, String)]| -> anyhow::Result<translate::Segments> {
        Ok(match mem.get(line, icons, phrases) {
            Some((segs, src)) => {
                log(format!("From memory ({src})"));
                translate::polite(&cfg.target_lang, tr.recolor(cfg, segs, phrases, log))
            }
            None => tr.translate(cfg, line, phrases, icons, log)?,
        })
    };
    let named = speaker::split(en, phrases).or_else(|| {
        let (name, rest) = speaker::plain_name(en)?;
        let reviewed = mem.get(&name, &[], &[]).is_some_and(|(_, src)| src == "reviewed");
        (speaker::is_known(&name) || speaker::is_role(&name) || reviewed || mem.knows(rest)).then(|| (name, speaker::PLAIN_NAME_COLOR.to_string(), rest))
    });
    let (line, phrases) = match &named {
        Some((name, color, rest)) => {
            log(format!("Speaker: {name} ({color})"));
            (*rest, speaker::phrases_without(phrases, name))
        }
        None => (en, phrases.to_vec()),
    };
    let segs = match speaker::language_tag(line) {
        Some((language, sentence)) => {
            log(format!("Language: {language}"));
            let segs = one(tr, sentence, &phrases)?;
            let tag = speaker::language_in(&cfg.target_lang, &language).unwrap_or_else(|| format!("In {language}:"));
            speaker::with_language(&tag, segs)
        }
        None => one(tr, line, &phrases)?,
    };
    Ok(match named {
        Some((name, color, _)) => {
            let mut reviewed: Option<String> = mem.get(&name, &[], &[]).filter(|(_, src)| src == "reviewed").map(|(s, _)| s.iter().map(|x| x.0.as_str()).collect());
            let role = speaker::is_role(&name) && cfg.target_lang != "ar";
            if role && reviewed.is_none() {
                reviewed = one(tr, &name, &[]).ok().map(|segs| segs.iter().map(|s| s.0.as_str()).collect());
            }
            let shown = if role {
                reviewed.map(|r| r.trim().trim_end_matches(|ch: char| matches!(ch, ':' | '.' | '!' | '?' | '،' | '؟' | '…')).trim().to_string()).filter(|r| !r.is_empty()).unwrap_or(name)
            } else {
                speaker::name_in(&cfg.target_lang, &name, reviewed)
            };
            speaker::with_name(&shown, &color, segs)
        }
        None => segs,
    })
}

/// `--translate-frames <video> <times file> <out dir>`: the frames at those times (seconds, one per
/// line) read and translated as the app would, each saved as <out dir>/<time>.jpg with a JSON line on
/// stdout: {"t", "file", "w", "h", "areas": [{"id", "segs", "line_h", "text_box", "game_box"}]}.
/// The frames come from ffmpeg in order (2 per second), so a whole stream is decoded once.
fn translate_frames(file: &str, times_file: &str, out_dir: &str) -> anyhow::Result<()> {
    use std::io::{Read, Write};
    let cfg = Config::load(&config_path());
    let areas: Vec<_> = cfg.areas.iter().filter(|a| a.enabled).cloned().collect();
    let mut wanted: Vec<f64> = std::fs::read_to_string(times_file)?.lines().filter_map(|l| l.trim().parse().ok()).collect();
    wanted.sort_by(|a, b| a.total_cmp(b));
    std::fs::create_dir_all(out_dir)?;
    let probe = std::process::Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "v:0", "-show_entries", "stream=width,height", "-of", "csv=p=0", file])
        .output()?;
    let dims = String::from_utf8_lossy(&probe.stdout).trim().to_string();
    let (w, h) = dims.split_once(',').and_then(|(a, b)| Some((a.trim().parse::<u32>().ok()?, b.trim().parse::<u32>().ok()?))).ok_or_else(|| anyhow::anyhow!("no video size in {dims:?}"))?;
    let fps = 2.0;
    // (several of these can share a video: each starts at its own first frame)
    let start = wanted.first().map(|t| (t * fps).floor() / fps).unwrap_or(0.0);
    let mut child = std::process::Command::new("ffmpeg")
        .args(["-v", "error", "-ss", &format!("{start}"), "-i", file, "-an", "-vf", "fps=2", "-f", "rawvideo", "-pix_fmt", "rgba", "-"])
        .stdout(std::process::Stdio::piped())
        .spawn()?;
    let mut pipe = child.stdout.take().ok_or_else(|| anyhow::anyhow!("no ffmpeg output"))?;
    Ocr::init_thread();
    let ocr = Ocr::new()?;
    let mut tr = Translator::new(model_dir());
    let mut mem = memory::Memory::load(db_dir(), &cfg.target_lang);
    if let Some(entries) = sync::kept_pack_in(&data_dir(), &cfg.game(), &cfg.target_lang) {
        mem.set_shared(&cfg.target_lang, entries);
    }
    let quiet = |_: String| {};
    let mut frame = vec![0u8; (w * h * 4) as usize];
    let out = std::io::stdout();
    let mut out = out.lock();
    let (mut n, mut next) = (0u64, 0usize);
    while next < wanted.len() && pipe.read_exact(&mut frame).is_ok() {
        let t = start + n as f64 / fps;
        n += 1;
        if (t - wanted[next]).abs() > 0.01 {
            continue;
        }
        next += 1;
        let mut shown = Vec::new();
        for area in &areas {
            let Some(crop) = Crop::from_frame(&frame, w, h, area.x, area.y, area.w, area.h) else { continue };
            let (mut words, sure) = ocr.recognize_checked(&crop)?;
            if area.centered_only {
                words = ocr::centered_words(&words, area.x.clamp(0.0, 1.0) * w as f64, w as f64);
            }
            let en = ocr::clean(&words);
            // (a reading the app isn't sure of, text still fading in, is one it would wait out)
            if en.is_empty() || (!sure && area.id != "subtitles") {
                continue;
            }
            let (en_icons, icons, block) = ocr::text_with_icons(&crop, &words);
            // instructions and help: only lines the reviewed translations know (a half-faded
            // "Follow the p" isn't a line; the app would show the whole one a moment later)
            if area.id != "subtitles" && !mem.knows(if icons.is_empty() { &en } else { &en_icons }) {
                continue;
            }
            let (cx0, cy0) = ((area.x.clamp(0.0, 1.0) * w as f64) as u32, (area.y.clamp(0.0, 1.0) * h as f64) as u32);
            let to_frame = |(x, y, bw, bh): (u32, u32, u32, u32)| [(cx0 + x) as f64 / w as f64, (cy0 + y) as f64 / h as f64, bw as f64 / w as f64, bh as f64 / h as f64];
            // (areas that read only centred text skip a menu's hint at the side, as the app does)
            if area.centered_only {
                if let Some((x, _, bw, _)) = block {
                    if ((cx0 + x + bw / 2) as f64 / w as f64 - 0.5).abs() > 0.04 {
                        continue;
                    }
                }
            }
            let phrases = ocr::colored_phrases(&crop, &words);
            let mut hs: Vec<u32> = words.iter().map(|wd| wd.rect.3).collect();
            hs.sort_unstable();
            let lh = hs.get(hs.len() / 2).copied().unwrap_or(20);
            if area.needs_box && block.and_then(|b| ocr::box_around(&crop, b, lh)).is_none() {
                continue;
            }
            let parts = if icons.is_empty() && area.centered_only { ocr::speaker_lines(&crop, &words) } else { Vec::new() };
            let segs = if parts.len() > 1 {
                let mut all: translate::Segments = Vec::new();
                for (i, (p, rel)) in parts.iter().enumerate() {
                    if i > 0 {
                        all.push(("\n".into(), None));
                    }
                    let grey = (*rel < 0.95).then(|| "dim".to_string());
                    let segs = offline_line(&mut tr, &cfg, &mem, p, &[], &phrases, &quiet)?;
                    all.extend(segs.into_iter().map(|(t, col)| (t, col.or_else(|| grey.clone()))));
                }
                all
            } else {
                let line = if icons.is_empty() { en.clone() } else { en_icons.clone() };
                offline_line(&mut tr, &cfg, &mem, &line, &icons, &phrases, &quiet)?
            };
            let game_box = match block {
                Some(b) if area.placement == "under_text" => ocr::box_around(&crop, b, lh).map(to_frame),
                _ => None,
            };
            shown.push(serde_json::json!({
                "id": area.id, "en": en, "segs": segs, "line_h": lh as f64 / h as f64,
                "text_box": block.map(to_frame), "game_box": game_box,
            }));
        }
        let name = format!("{:07.1}.jpg", t);
        let img = image::RgbaImage::from_raw(w, h, frame.clone()).ok_or_else(|| anyhow::anyhow!("frame"))?;
        image::DynamicImage::ImageRgba8(img).to_rgb8().save_with_format(std::path::Path::new(out_dir).join(&name), image::ImageFormat::Jpeg)?;
        writeln!(out, "{}", serde_json::json!({ "t": t, "file": name, "w": w, "h": h, "areas": shown }))?;
        out.flush()?;
        if next % 100 == 0 {
            eprintln!("translate-frames: {next}/{}", wanted.len());
        }
    }
    let _ = child.kill();
    eprintln!("translate-frames: {next} frames");
    Ok(())
}

/// `--read-lines <dir> [--area <id>]`: each saved frame's lines in a read area as the reading sees
/// them (an area whose lines stack up read as far above it as the app reads): each OCR line with its
/// middle and height (frame pixels), then what's kept as the area's text.
fn read_lines(dir: &str, area_id: &str) -> anyhow::Result<()> {
    let cfg = Config::load(&config_path());
    let area_cfg = cfg.areas.iter().find(|a| a.id == area_id).cloned().ok_or_else(|| anyhow::anyhow!("no area {area_id}"))?;
    let stacks = cfg.fx(area_id).stacks(area_id);
    let area = if stacks { config::stack_grown(&area_cfg) } else { area_cfg.clone() };
    Ocr::init_thread();
    let ocr = Ocr::new()?;
    let mut files: Vec<_> = std::fs::read_dir(dir)?.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "png" || x == "jpg") && !p.to_string_lossy().ends_with("_overlay.png")).collect();
    files.sort();
    let mut known_box: Option<((u32, u32, u32, u32), String, u64)> = None;
    let mut anchored_at: Option<(u32, u32)> = None;
    for path in files {
        let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
        let img = image::open(&path)?.to_rgba8();
        let (w, h) = img.dimensions();
        // (as the reading does: an area that stacks up in two parts, the second only when needed)
        let t = Instant::now();
        let (crop, mut words, _) = if stacks {
            engine::read_stacked(&ocr, img.as_raw(), w, h, &area_cfg, &area)
        } else {
            let c = Crop::from_frame(img.as_raw(), w, h, area.x, area.y, area.w, area.h);
            let r = c.as_ref().map(|c| ocr.recognize_checked(c)).transpose()?.unwrap_or_default();
            (c, r.0, r.1)
        };
        let ms = t.elapsed().as_millis();
        let Some(crop) = crop else { continue };
        if area.centered_only {
            words = ocr::centered_words(&words, area.x.clamp(0.0, 1.0) * w as f64, w as f64);
        }
        let y0 = (area.y.clamp(0.0, 1.0) * h as f64) as u32;
        let mut lines: std::collections::BTreeMap<u32, Vec<&ocr::Word>> = Default::default();
        for wd in &words {
            lines.entry(wd.line).or_default().push(wd);
        }
        println!("{name} ({ms} ms)");
        for (_, ws) in lines {
            let mid = ws.iter().map(|w| w.rect.1 + w.rect.3 / 2).sum::<u32>() / ws.len() as u32 + y0;
            let hh = ws.iter().map(|w| w.rect.3).max().unwrap_or(0);
            println!("   y {mid:4} h {hh:3}  {}", ws.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" "));
        }
        if stacks && !words.is_empty() {
            words = ocr::bottom_stack(&words, crop.h as f64);
        }
        // (as the reading does: the box where it was found last, for 15 s (the frames' times), and then
        // only for the same instruction or one of two lines or more)
        let at_ms: u64 = name.rsplit('_').next().and_then(|t| t.trim_end_matches(".png").trim_end_matches("ms").parse().ok()).unwrap_or(0);
        let anchor = config::box_anchor(&area).map(|[ax, ay, aw]| {
            let (cx0, cy0) = ((area.x.clamp(0.0, 1.0) * w as f64) as u32, (area.y.clamp(0.0, 1.0) * h as f64) as u32);
            (((ax * w as f64).round() as u32).saturating_sub(cx0), ((ay * h as f64).round() as u32).saturating_sub(cy0), (aw * w as f64) as u32)
        });
        if let (true, Some(anchor)) = (area.needs_box && !words.is_empty(), anchor) {
            match ocr::in_anchored_box(&words, anchor) {
                Some((b, kept)) => {
                    let first = kept.first().map(|w| (w.rect.0, w.rect.1)).unwrap_or_default();
                    let still = anchored_at.is_some_and(|(x, y): (u32, u32)| x.abs_diff(first.0) <= 3 && y.abs_diff(first.1) <= 3);
                    let e = ocr::in_game_box(&crop, &kept, None).map(|(e, _, _)| e).filter(|e| e.0.abs_diff(b.0) <= 12 && e.1.abs_diff(b.1) <= 12);
                    anchored_at = Some(first);
                    let right = kept.iter().map(|w| w.rect.0 + w.rect.2).max().unwrap_or(0);
                    let bottom = kept.iter().map(|w| w.rect.1 + w.rect.3).max().unwrap_or(0);
                    println!("   box {b:?} edges {e:?} text right {right} bottom {bottom}{}", if still { " still" } else { "" });
                    words = if still || e.is_some() { kept } else { Vec::new() };
                }
                None => {
                    anchored_at = None;
                    words.clear();
                }
            }
        } else if area.needs_box && !words.is_empty() {
            let known = known_box.as_ref().filter(|(_, _, t)| at_ms.saturating_sub(*t) < 15_000).map(|(b, _, _)| *b);
            match ocr::in_game_box(&crop, &words, known) {
                Some((_, kept, false))
                    if !ocr::similar(&ocr::clean(&kept), known_box.as_ref().map_or("", |k| k.1.as_str()))
                        && kept.iter().map(|w| w.line).collect::<std::collections::HashSet<_>>().len() < 2 =>
                {
                    words.clear()
                }
                Some((b, kept, seen)) => {
                    println!("   game box {b:?}{}", if seen { "" } else { " (where it was found last)" });
                    if seen {
                        known_box = Some((b, ocr::clean(&kept), at_ms));
                    }
                    words = kept;
                }
                None => words.clear(),
            }
        }
        let mut text = ocr::clean(&words);
        if area.needs_box && (ocr::mostly_capitals(&text) || (config::box_anchor(&area).is_some() && !ocr::reads_as_sentences(&text))) {
            text.clear();
        }
        println!("   => {text:?}");
    }
    Ok(())
}

/// `--read-chat <dir>`: each saved frame's message bubble at the bottom right (see the core's chat.rs):
/// its box, the name and the message, its picture and colours, and how long finding and reading took.
fn read_chat(dir: &str) -> anyhow::Result<()> {
    Ocr::init_thread();
    let ocr = Ocr::new()?;
    let mut files: Vec<_> = std::fs::read_dir(dir)?.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "png" || x == "jpg") && !p.to_string_lossy().ends_with("_overlay.png")).collect();
    files.sort();
    for path in files {
        let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
        let img = image::open(&path)?.to_rgba8();
        let (w, h) = img.dimensions();
        let t = Instant::now();
        let Some(rect) = vi_translator_core::chat::find_bubble(img.as_raw(), w, h) else {
            println!("{name}: no bubble ({} ms)", t.elapsed().as_millis());
            continue;
        };
        let found_ms = t.elapsed().as_millis();
        match vi_translator_core::chat::read_bubble(&ocr, img.as_raw(), w, h, rect) {
            Some(b) => println!(
                "{name}: bubble {rect:?} ({found_ms} + {} ms)\n   name {:?} message {:?}{}\n   avatar {:?} badge {:?} bg {:?} fg {:?} name fg {:?}\n   emoji {:?}",
                t.elapsed().as_millis() - found_ms,
                b.name,
                b.en,
                if b.cut() { " CUT" } else { "" },
                b.avatar,
                b.badge,
                b.bg,
                b.fg,
                b.name_fg,
                b.emoji
            ),
            None => println!("{name}: bubble {rect:?}, no text read ({} ms)", t.elapsed().as_millis()),
        }
    }
    Ok(())
}

/// `--read-phone <dir>`: each saved frame's phone screen (where it was found) and its text in pieces,
/// as the reading finds them.
fn read_phone(dir: &str) -> anyhow::Result<()> {
    Ocr::init_thread();
    let ocr = Ocr::new()?;
    let mut files: Vec<_> = std::fs::read_dir(dir)?.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "png" || x == "jpg") && !p.to_string_lossy().ends_with("_overlay.png")).collect();
    files.sort();
    // (as the reading does: steady from one frame to the next, by the frames' times)
    let mut seen = Vec::new();
    let start = Instant::now();
    for path in files {
        let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
        let at_ms: u64 = name.rsplit('_').next().and_then(|t| t.trim_end_matches(".png").trim_end_matches(".jpg").trim_end_matches("ms").parse().ok()).unwrap_or(0);
        let when = start + Duration::from_millis(at_ms);
        let img = image::open(&path)?.to_rgba8();
        let (w, h) = img.dimensions();
        let t = Instant::now();
        // (`--whole`: the phone read whole, as on a Mac — phone_read.rs: its title and its pieces)
        if std::env::args().any(|a| a == "--whole") {
            use vi_translator_core::phone_read as pr;
            let frame = capture::FrameData { w, h, rgba: img.into_raw(), seq: 0 };
            let Some(shot) = vi_translator_core::phone::find_phone(&frame.rgba, w, h).and_then(|scr| pr::shot(&frame, scr)) else {
                println!("{name}: no phone ({} ms)", t.elapsed().as_millis());
                continue;
            };
            let found = t.elapsed().as_millis();
            let lines = ocr.lines_with(&shot.crop, vi_translator_core::ocr::Prep::Brightest)?;
            let (title, items) = pr::page(lines, shot.col, shot.bottom, &|l| shot.tint(l), &|l| shot.ink(l));
            println!("{name}: found in {found} ms, read in {} ms | title {title:?}", t.elapsed().as_millis() - found);
            for it in items {
                println!("   {:5} {:8} {:7} | {}", it.k, it.from, it.color, it.en);
            }
            continue;
        }
        let screen = vi_translator_core::phone::find_screen(img.as_raw(), w, h)
            .or_else(|| vi_translator_core::phone::find_screen_unsure(img.as_raw(), w, h).filter(|_| vi_translator_core::phone::phone_buttons(&vi_translator_core::phone::read_buttons(&ocr, img.as_raw(), w, h))));
        let Some(screen) = screen else {
            println!("{name}: no phone ({} ms)", t.elapsed().as_millis());
            continue;
        };
        let pieces = vi_translator_core::phone::read_screen(&ocr, img.as_raw(), w, h, screen);
        let pieces = vi_translator_core::phone::steady(&mut seen, pieces, screen, when);
        println!("{name}: phone {screen:?} ({} ms)", t.elapsed().as_millis());
        let sbg = vi_translator_core::phone::screen_bg(img.as_raw(), w, h, screen, screen.1, screen.1 + screen.3);
        println!("   screen bg {sbg:?}");
        for (r, en) in vi_translator_core::phone::read_buttons(&ocr, img.as_raw(), w, h) {
            println!("   BUTTON {r:?} {en:?} icon {:?}", vi_translator_core::phone::button_icon(img.as_raw(), w, h, r));
        }
        for p in pieces {
            let l = &p.look;
            println!("   {}{:?} {:?}{} bg {:?} fg {:?}{}{}{}", if p.title { "TITLE " } else { "" }, p.rect, p.en, p.name.as_ref().map(|n| format!(" (name {n:?})")).unwrap_or_default(), l.bg, l.fg, if l.label { " LABEL" } else { "" }, if l.end { " END" } else { "" }, l.edge.map(|e| format!(" EDGE {e:?}")).unwrap_or_default() + &p.avatar.map(|a| format!(" AVATAR {a:?}")).unwrap_or_default() + &p.preview.map(|a| format!(" PREVIEW {a:?}")).unwrap_or_default() + &p.badge.map(|a| format!(" BADGE {a:?}")).unwrap_or_default() + &(if p.emoji.is_empty() { String::new() } else { format!(" EMOJI {:?}", p.emoji) }));
        }
    }
    Ok(())
}

fn test_image(path: &str, area_id: &str) -> anyhow::Result<()> {
    let cfg = Config::load(&config_path());
    let mut area = cfg.areas.iter().find(|a| a.id == area_id).cloned().ok_or_else(|| anyhow::anyhow!("no area {area_id}"))?;
    // `--region x,y,w,h` (fractions) overrides the saved area, so tests do not depend on your layout
    let args: Vec<String> = std::env::args().collect();
    if let Some(r) = args.iter().position(|a| a == "--region").and_then(|j| args.get(j + 1)) {
        let v: Vec<f64> = r.split(',').filter_map(|s| s.trim().parse().ok()).collect();
        if let [x, y, w, h] = v[..] {
            (area.x, area.y, area.w, area.h) = (x, y, w, h);
        }
    }
    let img = image::open(path)?.to_rgba8();
    let (w, h) = img.dimensions();
    let crop = Crop::from_frame(img.as_raw(), w, h, area.x, area.y, area.w, area.h).ok_or_else(|| anyhow::anyhow!("empty crop"))?;
    Ocr::init_thread();
    let ocr = Ocr::new()?;
    let t = Instant::now();
    let mut words = ocr.recognize(&crop)?;
    if std::env::var_os("VI_TRANSLATOR_DEBUG").is_some() {
        eprintln!("[ocr] words: {:?}", words.iter().map(|w| (&w.text, w.rect)).collect::<Vec<_>>());
    }
    if area.centered_only {
        words = ocr::centered_words(&words, area.x.clamp(0.0, 1.0) * w as f64, w as f64);
    }
    let en = ocr::clean(&words);
    let phrases = ocr::colored_phrases(&crop, &words);
    println!("OCR ({} ms): {en:?}", t.elapsed().as_millis());
    let t2 = Instant::now();
    let (en_icons, icons, block) = ocr::text_with_icons(&crop, &words);
    println!("Text block (crop px): {block:?}");
    if let Some(b) = block {
        let mut hs: Vec<u32> = words.iter().map(|w| w.rect.3).collect();
        hs.sort_unstable();
        let lh = hs.get(hs.len() / 2).copied().unwrap_or(20);
        // the game's own box around it, in frame pixels (the crop starts at the area's top-left)
        let (cx0, cy0) = ((area.x.clamp(0.0, 1.0) * w as f64) as u32, (area.y.clamp(0.0, 1.0) * h as f64) as u32);
        println!("Game box (frame px): {:?}", ocr::box_around(&crop, b, lh).map(|(x, y, bw, bh)| (cx0 + x, cy0 + y, bw, bh)));
    }
    // with VI_TRANSLATOR_ICON_DIR set, write the cut-out icons there (to check how they look)
    if let Some(dir) = std::env::var_os("VI_TRANSLATOR_ICON_DIR") {
        use base64::Engine as _;
        for (n, url) in icons.iter().enumerate() {
            let (rel, png) = url.split_once('|').unwrap_or(("?", url.as_str()));
            println!("icon {}: {} x the text height", n + 1, rel);
            if let Some(b64) = png.strip_prefix("data:image/png;base64,") {
                let bytes = base64::engine::general_purpose::STANDARD.decode(b64)?;
                let stem = std::path::Path::new(path).file_stem().unwrap_or_default().to_string_lossy().to_string();
                std::fs::write(std::path::Path::new(&dir).join(format!("{stem}_icon{}.png", n + 1)), bytes)?;
            }
        }
    }
    println!("With icons ({} ms, {} icon(s)): {en_icons:?}", t2.elapsed().as_millis(), icons.len());
    if !phrases.is_empty() {
        println!("Highlighted: {phrases:?}");
    }
    let mut tr = Translator::new(model_dir());
    // the memory as the app has it (the downloaded reviewed lines too); read only, nothing is saved
    let mut mem = memory::Memory::load(db_dir(), &cfg.target_lang);
    if let Some(entries) = sync::kept_pack_in(&data_dir(), &cfg.game(), &cfg.target_lang) {
        mem.set_shared(&cfg.target_lang, entries);
    }
    let line = |tr: &mut Translator, en: &str, icons: &[String]| offline_line(tr, &cfg, &mem, en, icons, &phrases, &|s| println!("{s}"));
    let t = Instant::now();
    // two speakers' lines (white and grey): each translated on its own, as the app does
    let parts = if icons.is_empty() && area.centered_only { ocr::speaker_lines(&crop, &words) } else { Vec::new() };
    let segs = if parts.len() > 1 {
        println!("Speakers: {parts:?}");
        let mut all: translate::Segments = Vec::new();
        for (i, (en, rel)) in parts.iter().enumerate() {
            if i > 0 {
                all.push(("\n".into(), None));
            }
            let grey = (*rel < 0.95).then(|| "dim".to_string());
            let segs = line(&mut tr, en, &[])?;
            all.extend(segs.into_iter().map(|(t, col)| (t, col.or_else(|| grey.clone()))));
        }
        all
    } else {
        line(&mut tr, &en_icons, &icons)?
    };
    println!("Segments: {}", serde_json::to_string(&segs).unwrap_or_default());
    let shown: String = segs
        .iter()
        .map(|(t, c)| match c {
            Some(c) if c.starts_with("icon:") => "🎮".to_string(),
            Some(_) => format!("[{t}]"),
            None => t.clone(),
        })
        .collect();
    println!("Translation ({} ms, {}): {shown}", t.elapsed().as_millis(), cfg.engine);
    Ok(())
}

// ------------------------------------------------------------------------------------- main

fn main() {
    move_old_folders();
    let args: Vec<String> = std::env::args().collect();
    // `--screen-copy "Window title" out.png`: copy that window's area of the screen (overlays included).
    if let Some(i) = args.iter().position(|a| a == "--screen-copy") {
        let (Some(title), Some(out)) = (args.get(i + 1), args.get(i + 2)) else { std::process::exit(2) };
        let hwnd = capture::find_window(title).unwrap_or(0);
        let r = capture::client_rect(hwnd).and_then(|(x, y, w, h)| capture::grab_screen_region(x, y, w, h).map(|px| (w, h, px)));
        match r {
            Some((w, h, px)) => {
                let f = capture::FrameData { w: w as u32, h: h as u32, rgba: px, seq: 0 };
                capture::save_png(&f, std::path::Path::new(out)).expect("save");
                println!("saved {w}x{h} screen copy to {out}");
            }
            None => eprintln!("screen copy failed"),
        }
        return;
    }
    // `--grab "Window title" out.png`: save one frame of a window (for building/testing profiles).
    // `--grab-device "fifine Video Capture" out.png` / `--list-devices`: test a capture card.
    if args.iter().any(|a| a == "--list-devices") {
        for d in device::list() {
            println!("{d}");
        }
        return;
    }
    if let Some(i) = args.iter().position(|a| a == "--grab-device") {
        let (Some(name), Some(out)) = (args.get(i + 1), args.get(i + 2)) else {
            eprintln!("usage: --grab-device \"Device name\" out.png");
            std::process::exit(2);
        };
        let t = std::time::Instant::now();
        match device::grab_once(name).and_then(|f| capture::save_png(&f, std::path::Path::new(out)).map(|_| f)) {
            Ok(f) => println!("saved {}x{} frame to {out} ({} ms)", f.w, f.h, t.elapsed().as_millis()),
            Err(e) => {
                eprintln!("error: {e:#}");
                std::process::exit(1);
            }
        }
        return;
    }
    // `--grab-obs "Video Capture Device" out.png`: one frame of an OBS source, using the app's saved
    // OBS port and password.
    // `--translate "line" ["line" ...]`: translate text with the configured engine (for testing).
    if let Some(i) = args.iter().position(|a| a == "--translate") {
        let cfg = Config::load(&config_path());
        let mut tr = translate::Translator::new(model_dir());
        for line in &args[i + 1..] {
            match tr.translate(&cfg, line, &[], &[], &|_| {}) {
                Ok(segs) => println!("{line}
  -> {}", segs.iter().map(|s| s.0.as_str()).collect::<String>()),
                Err(e) => println!("{line}
  !! {e:#}"),
            }
        }
        return;
    }
    if let Some(i) = args.iter().position(|a| a == "--grab-obs") {
        let (Some(name), Some(out)) = (args.get(i + 1), args.get(i + 2)) else {
            eprintln!("usage: --grab-obs \"OBS source name\" out.png");
            std::process::exit(2);
        };
        let cfg = Config::load(&config_path());
        let t = std::time::Instant::now();
        let r = obs::Obs::connect(cfg.obs_port, &cfg.obs_password).and_then(|mut o| {
            let list = o.inputs()?;
            println!("OBS sources: {}", list.iter().map(|(n, k)| format!("{n} ({k})")).collect::<Vec<_>>().join(", "));
            let t1 = std::time::Instant::now();
            let f = o.snapshot(name)?;
            let full_ms = t1.elapsed().as_millis();
            let t2 = std::time::Instant::now();
            let j = o.preview_jpeg(name)?;
            println!("full frame {}x{} in {full_ms} ms; preview JPEG {} KB in {} ms", f.w, f.h, j.len() / 1024, t2.elapsed().as_millis());
            capture::save_png(&f, std::path::Path::new(out))?;
            Ok(())
        });
        match r {
            Ok(()) => println!("saved to {out} ({} ms total)", t.elapsed().as_millis()),
            Err(e) => {
                eprintln!("error: {e:#}");
                std::process::exit(1);
            }
        }
        return;
    }
    #[cfg(not(target_os = "linux"))]
    if let Some(i) = args.iter().position(|a| a == "--test-sound") {
        card_sound::test(args.get(i + 1).map(String::as_str));
        return;
    }
    // `--find-picture "Window title"`: the part of that window showing the game (what moves), as fractions.
    if let Some(title) = args.iter().position(|a| a == "--find-picture").and_then(|i| args.get(i + 1)) {
        let mut frames = Vec::new();
        for i in 0..3 {
            if i > 0 {
                std::thread::sleep(Duration::from_millis(600));
            }
            match capture::grab_once(title) {
                Ok(f) => frames.push(f),
                Err(e) => {
                    eprintln!("error: {e:#}");
                    std::process::exit(1);
                }
            }
        }
        let refs: Vec<&capture::FrameData> = frames.iter().map(|f| f.as_ref()).collect();
        let (w, h) = (frames[0].w, frames[0].h);
        match picture_find::moving_part(&refs).and_then(config::clean_crop) {
            Some(c) => println!("{w}x{h}: x {:.4} y {:.4} w {:.4} h {:.4} = {:?} px", c[0], c[1], c[2], c[3], capture::crop_px(w, h, c)),
            None => println!("{w}x{h}: nothing moved enough (or the whole window)"),
        }
        return;
    }
    if let Some(i) = args.iter().position(|a| a == "--grab") {
        let (Some(title), Some(out)) = (args.get(i + 1), args.get(i + 2)) else {
            eprintln!("usage: --grab \"Window title\" out.png");
            std::process::exit(2);
        };
        match capture::grab_once(title).and_then(|f| capture::save_png(&f, std::path::Path::new(out)).map(|_| f)) {
            Ok(f) => println!("saved {}x{} frame to {out}", f.w, f.h),
            Err(e) => {
                eprintln!("error: {e:#}");
                std::process::exit(1);
            }
        }
        return;
    }
    // `--test-recolor "<arabic>" "<highlighted english>"`: colour a remembered (uncoloured) line.
    if let Some(i) = args.iter().position(|a| a == "--test-recolor") {
        let (Some(ar), Some(phrase)) = (args.get(i + 1), args.get(i + 2)) else { return };
        let cfg = Config::load(&config_path());
        let mut tr = Translator::new(model_dir());
        let segs = tr.recolor(&cfg, vec![(ar.clone(), None)], &[(phrase.clone(), "#7ed957".into())], &|_| {});
        println!("{segs:?}");
        return;
    }
    // `--read-pages <folder> [--game <id>] [--script <file.js>] [--full]`: what a pages script reads
    // on every PNG in a folder (saved debug frames), page by page — to try a new page before it's
    // signed and sent to testers. Without --script: the game's script as the app would load it (its
    // own, or a signed download). A --script file needn't be signed (it's yours, on your PC).
    if let Some(dir) = args.iter().position(|a| a == "--read-pages").and_then(|i| args.get(i + 1)) {
        let opt = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
        let full = args.iter().any(|a| a == "--full");
        let run = || -> anyhow::Result<()> {
            Ocr::init_thread();
            let ocr = Ocr::new()?;
            let game = opt("--game").unwrap_or_else(|| Config::load(&config_path()).game());
            // (the language in use, for a script's own words: `--lang xx` to try another)
            screens::use_words_for(&game, &opt("--lang").unwrap_or_else(|| Config::load(&config_path()).target_lang));
            let pages = match opt("--script") {
                Some(file) => {
                    let mut p = pagescript::Pages::load(&std::fs::read_to_string(&file)?)?;
                    p.origin = format!("{file}, version {}", pagescript::version_of(&std::fs::read_to_string(&file)?));
                    p
                }
                None => pagescript::Pages::for_game(&game, &data_dir()).ok_or_else(|| anyhow::anyhow!("no pages script for {game}: give one with --script"))?,
            };
            println!("script: {} · pages: {}", pages.origin, pages.list.iter().map(|i| i.id.as_str()).collect::<Vec<_>>().join(", "));
            for o in &pages.options {
                println!("switch: {} ({}), on by default: {}", o.id, o.name, o.on);
            }
            // (the frames only: not the "_overlay.png" copies saved next to them, with the translation drawn)
            let mut files: Vec<_> = std::fs::read_dir(dir)?
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "png") && !p.file_name().is_some_and(|n| n.to_string_lossy().ends_with("_overlay.png")))
                .collect();
            files.sort();
            let (mut n, mut found, mut recognised) = (0, 0, 0);
            for path in files {
                let Ok(img) = image::open(&path) else { continue };
                let img = img.to_rgba8();
                let (w, h) = img.dimensions();
                let frame = capture::FrameData { w, h, rgba: img.into_raw(), seq: 1 };
                n += 1;
                let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
                let mut any = false;
                for info in &pages.list {
                    let t = Instant::now();
                    let present = pages.present(&info.id, &frame, &ocr);
                    if std::env::var_os("VI_TRANSLATOR_TIMING").is_some() {
                        eprintln!("[timing] {name} {} present {} µs", info.id, t.elapsed().as_micros());
                    }
                    let Some(k) = present else { continue };
                    let read = pages.read(&info.id, &frame, &ocr, &k);
                    let ms = t.elapsed().as_millis();
                    any = true;
                    found += 1;
                    match read {
                        None => println!("{name} {}: on screen, but read() found nothing ({ms} ms)", info.id),
                        Some(l) if full => println!("{name} {} ({ms} ms): {}", info.id, serde_json::to_string_pretty(&l).unwrap_or_default()),
                        Some(l) => {
                            let mut parts = Vec::new();
                            if l.title.w > 0.0 {
                                parts.push(format!("title at {:.3},{:.3}", l.title.x, l.title.y));
                            }
                            if !l.reason_en.is_empty() {
                                parts.push(format!("reason {:?}", l.reason_en));
                            }
                            if !l.buttons.is_empty() {
                                parts.push(format!("buttons {:?}", l.buttons.iter().map(|b| b.en.as_str()).collect::<Vec<_>>()));
                            }
                            if !l.labels.is_empty() {
                                parts.push(format!("labels {:?}", l.labels.iter().map(|b| b.en.as_str()).collect::<Vec<_>>()));
                            }
                            if let Some(p) = &l.player {
                                parts.push(format!("player {:?}", p.en));
                            }
                            if !l.menu.is_empty() {
                                let shown: Vec<String> = l.menu.iter().take(12).map(|m| if m.private { "(private)".to_string() } else { m.en.clone() }).collect();
                                parts.push(format!("{} text box(es) {:?}{}", l.menu.len(), shown, if l.menu.len() > 12 { " …" } else { "" }));
                            }
                            println!("{name} {} ({ms} ms): {}", info.id, if parts.is_empty() { "an empty page".to_string() } else { parts.join(" · ") });
                        }
                    }
                }
                if pages.recognize(&frame, &ocr) {
                    recognised += 1;
                }
                if !any {
                    println!("{name}: no page");
                }
            }
            println!("{n} frames, {found} pages on screen, the game recognised on {recognised}");
            Ok(())
        };
        if let Err(e) = run() {
            eprintln!("read-pages: {e:#}");
        }
        return;
    }
    // `--read-video <file> [--fps N] [--from S] [--to S]`: every enabled area of every frame read the
    // way the app reads them (nothing translated), as JSON lines on stdout — to collect a game's lines.
    if let Some(file) = args.iter().position(|a| a == "--read-video").and_then(|i| args.get(i + 1)) {
        let num = |name: &str, def: f64| args.iter().position(|a| a == name).and_then(|j| args.get(j + 1)).and_then(|v| v.parse().ok()).unwrap_or(def);
        if let Err(e) = read_video(file, num("--fps", 2.0), num("--from", 0.0), num("--to", 0.0)) {
            eprintln!("read-video: {e:#}");
            std::process::exit(1);
        }
        return;
    }
    if let Some(i) = args.iter().position(|a| a == "--translate-frames") {
        let (Some(video), Some(times), Some(out)) = (args.get(i + 1), args.get(i + 2), args.get(i + 3)) else {
            eprintln!("usage: --translate-frames <video> <times file> <out dir>");
            std::process::exit(2);
        };
        if let Err(e) = translate_frames(video, times, out) {
            eprintln!("translate-frames: {e:#}");
            std::process::exit(1);
        }
        return;
    }
    // --test-snapshot <frame.png> [--service <id>]: a snapshot of a saved frame, its blocks printed
    // --phone-crops <frame.png> <out dir> (dev builds): GTA VI's phone read in a saved frame, as the engine
    // reads it: each piece printed, its crops (picture, mark, emoji) saved, and the frame with them drawn on
    #[cfg(debug_assertions)]
    if let Some(i) = args.iter().position(|a| a == "--phone-crops") {
        let (Some(file), Some(out)) = (args.get(i + 1), args.get(i + 2)) else {
            eprintln!("usage: --phone-crops <frame.png> <out dir>");
            std::process::exit(2);
        };
        if let Err(e) = phone_crops(file, out) {
            eprintln!("read-phone: {e:#}");
            std::process::exit(1);
        }
        return;
    }
    if let Some(file) = args.iter().position(|a| a == "--test-snapshot").and_then(|i| args.get(i + 1)) {
        let run = || -> anyhow::Result<()> {
            let img = image::open(file)?.to_rgba8();
            let (w, h) = img.dimensions();
            let frame = capture::FrameData { w, h, rgba: img.into_raw(), seq: 0 };
            let mut c = Config::load(&config_path());
            if let Some(s) = args.iter().position(|a| a == "--service").and_then(|j| args.get(j + 1)) {
                c.snap_service = s.clone();
            }
            let mut mem = memory::Memory::load(db_dir(), &c.target_lang);
            if let Some(entries) = sync::kept_pack_in(&data_dir(), &c.game(), &c.target_lang) {
                mem.set_shared(&c.target_lang, entries);
            }
            let lang = c.target_lang.clone();
            let lookup = |en: &str| mem.get(en, &[], &[]).filter(|(_, src)| src == "reviewed").map(|(segs, _)| translate::polite(&lang, segs));
            let mut tr = Translator::new(model_dir());
            let ocr = || {
                Ocr::init_thread();
                Ocr::new()
            };
            let snap = snapshot::make(&frame, &[], snapshot::Tools { cfg: &c, tr: &mut tr, memory: &lookup, ocr: &ocr }, "test".into())?;
            println!("{} block(s), {} ms, service {}", snap.blocks.len(), snap.ms, snap.service);
            for b in &snap.blocks {
                let t: String = b.segs.iter().map(|s| s.0.as_str()).collect();
                println!("[{:.3} {:.3} {:.3} {:.3}] {} line(s) {} fg {} bg {} | {} -> {}", b.rect[0], b.rect[1], b.rect[2], b.rect[3], b.lines, b.align, b.fg, b.bg, b.en, t);
            }
            if let Some(out) = args.iter().position(|a| a == "--json").and_then(|j| args.get(j + 1)) {
                std::fs::write(out, serde_json::to_string(&snap)?)?;
            }
            Ok(())
        };
        if let Err(e) = run() {
            eprintln!("error: {e:#}");
            std::process::exit(1);
        }
        return;
    }
    if let Some(dir) = args.iter().position(|a| a == "--read-chat").and_then(|i| args.get(i + 1)) {
        if let Err(e) = read_chat(dir) {
            eprintln!("error: {e:#}");
            std::process::exit(1);
        }
        return;
    }
    if let Some(dir) = args.iter().position(|a| a == "--read-phone").and_then(|i| args.get(i + 1)) {
        if let Err(e) = read_phone(dir) {
            eprintln!("error: {e:#}");
            std::process::exit(1);
        }
        return;
    }
    if let Some(dir) = args.iter().position(|a| a == "--read-lines").and_then(|i| args.get(i + 1)) {
        let area = args.iter().position(|a| a == "--area").and_then(|j| args.get(j + 1)).map(String::as_str).unwrap_or("subtitles");
        if let Err(e) = read_lines(dir, area) {
            eprintln!("error: {e:#}");
            std::process::exit(1);
        }
        return;
    }
    if let Some(i) = args.iter().position(|a| a == "--test-image") {
        let area = args.iter().position(|a| a == "--area").and_then(|j| args.get(j + 1)).map(String::as_str).unwrap_or("subtitles");
        let Some(image) = args.get(i + 1) else {
            eprintln!("usage: --test-image <picture.png> [--area <id>]");
            std::process::exit(2);
        };
        if let Err(e) = test_image(image, area) {
            eprintln!("error: {e:#}");
            std::process::exit(1);
        }
        return;
    }

    let mut loaded = Config::load(&config_path());
    loaded.debug_save_frames = false; // recording always starts off
    let mut save_now = std::mem::take(&mut loaded.defaults_moved); // (a setting moved to a new default)
    if loaded.install_id.is_empty() {
        // a random id for this install (no account): lets the server count distinct players
        loaded.install_id = uuid::Uuid::new_v4().to_string();
        save_now = true;
    }
    if save_now {
        let _ = loaded.save(&config_path());
    }
    let cfg = Arc::new(Mutex::new(loaded));
    let saver = Arc::new(debug_frames::SaverState::default());
    let memory = Arc::new(Mutex::new(memory::Memory::load(db_dir(), &cfg.lock().unwrap().target_lang)));
    let shared = Arc::new(Shared::default());
    // the TVs connected to show the overlays, and their keys (their own file, not the settings)
    shared.tvs.load(data_dir().join("tvs.json"));
    let update: Arc<Mutex<Option<updater::Update>>> = Arc::new(Mutex::new(None));

    tauri::Builder::default()
        .plugin(hotkey::plugin())
        .manage(AppState { cfg: cfg.clone(), shared: shared.clone(), saver: saver.clone(), memory: memory.clone(), update: update.clone() })
        .invoke_handler(tauri::generate_handler![app_version, dev_build, dev_report, has_offline, memory_line, memory_probe, get_phone, update_status, install_update, lan_info, open_lan_page, find_tvs, connect_tv, list_tvs, remove_tv, list_langs, list_sources, window_still, find_picture, vcam_status, sound_inputs, sound_status, set_snap_hotkey, back_to_game, get_config, save_config, page_options, list_windows, list_screens, list_fonts, get_screen, frame_saver_status, open_frames_folder, save_preview, open_previews_folder, take_snapshot, save_snapshot_render, open_snapshots_folder, perf_status, memory_status, memory_clear, open_xbox, ps5_status, ps5_sign_in, ps5_browser_sign_in, ps5_login_url, ps5_sign_in_address, ps5_set_account, ps5_wake, ps5_set_sound, ps5_set_resolution, ps5_sign_out, ps5_find, ps5_link, ps5_forget, ps5_login_pin])
        .setup(move |app| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.set_title(&format!("VI Translator {}", env!("CARGO_PKG_VERSION")));
            }
            {
                // the snapshot's keyboard shortcut, if one is set
                let keys = cfg.lock().unwrap().snap_hotkey.clone();
                if let Err(e) = hotkey::set(app.handle(), &keys) {
                    eprintln!("[hotkey] \"{keys}\": {e}");
                }
            }
            ps5::init(app.handle().clone(), &data_dir());
            // Xbox in the app as the source (or `--xbox`): its player opens with the app
            if xbox::available() && (cfg.lock().unwrap().window_title == xbox::TITLE || args.iter().any(|a| a == "--xbox")) {
                if let Err(e) = xbox::open(app.handle(), &data_dir()) {
                    eprintln!("[xbox] {e}");
                }
            }
            let handle = app.handle().clone();
            let sink: Arc<dyn vi_translator_core::Sink> = Arc::new(TauriSink(handle.clone()));
            {
                // the Arabic-only page for an OBS Browser Source
                let (h, c, s) = (handle.clone(), cfg.clone(), shared.clone());
                std::thread::spawn(move || obs_page::run(h, c, s));
            }
            {
                let (h, c, s, m, k) = (app.handle().clone(), cfg.clone(), shared.clone(), memory.clone(), sink.clone());
                std::thread::spawn(move || lan::run(h, c, s, m, k));
            }
            {
                let (h, c, u) = (app.handle().clone(), cfg.clone(), update.clone());
                std::thread::spawn(move || updater::run(h, c, u));
            }

            {
                let (s, c) = (shared.clone(), cfg.clone());
                std::thread::spawn(move || capture::run_manager(s, c));
            }
            // (dev builds: the debug server for the live preview, dev_debug.rs)
            #[cfg(debug_assertions)]
            {
                let (h, s) = (app.handle().clone(), shared.clone());
                let out = pictures_dir().map(|p| p.join(FOLDER).join("dev-shots")).unwrap_or_else(|| data_dir().join("dev-shots"));
                std::thread::spawn(move || dev_debug::run(h, s, data_dir(), out));
            }
            {
                // the capture card's sound on the speakers (with a card as the source)
                let (s, c) = (shared.clone(), cfg.clone());
                std::thread::spawn(move || card_sound::run(c, s));
            }
            {
                // the game with its translations as a webcam (off unless turned on)
                let (s, c) = (shared.clone(), cfg.clone());
                std::thread::spawn(move || vcam::run(c, s));
            }
            {
                let (h, c, s) = (handle.clone(), cfg.clone(), shared.clone());
                std::thread::spawn(move || preview_loop(h, c, s));
            }
            {
                // the reading (and, in the core, its translator and presenter threads)
                let ctx = engine::Context { cfg: cfg.clone(), live: shared.live.clone(), memory: memory.clone(), sink: sink.clone(), paths: paths() };
                let (h, s) = (handle.clone(), shared.clone());
                std::thread::spawn(move || ocr_loop(h, ctx, s));
            }
            {
                let (h, c, s) = (handle.clone(), cfg.clone(), shared.clone());
                std::thread::spawn(move || overlay::position_loop(h, c, s));
            }
            {
                let (h, c, s, sv) = (handle.clone(), cfg.clone(), shared.clone(), saver.clone());
                std::thread::spawn(move || debug_frames::run(h, c, s, sv, frames_dir()));
            }
            {
                let (k, c, m) = (sink.clone(), cfg.clone(), memory.clone());
                std::thread::spawn(move || sync::run(k, c, m, paths()));
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the main window quits the app (overlays included).
            if window.label() == "main" {
                if let tauri::WindowEvent::CloseRequested { .. } = event {
                    window.app_handle().exit(0);
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running VI Translator");

}

