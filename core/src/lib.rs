//! VI Translator's reading and translating, without the desktop app around it: the same code the
//! desktop app (src-tauri) runs, for the phone apps (Android, iOS, through the UniFFI layer in
//! `ffi/`) and the TV apps' "PC" side. See README.md.
//!
//! The host supplies the picture and the text recognition (`ocr::TextEngine`); the core does
//! everything from there: steadied lines, translations (the translation memory, the downloaded
//! reviewed translations, the chosen translation service), colours and icons, the game's pages,
//! the timing, and what the overlays show (`live`), which TVs on the home network draw (`tv`,
//! `tv_server`).

pub mod config;
pub mod engine;
pub mod frame;
pub mod game_icons;
pub mod lan_page;
pub mod langs;
pub mod live;
pub mod memory;
pub mod net;
pub mod ocr;
pub mod pagescript;
pub mod perf;
pub mod phone;
pub mod phone_read;
pub mod chat;
pub mod phrasebook;
pub mod screens;
pub mod secure;
pub mod sign;
pub mod snapshot;
pub mod speaker;
pub mod sync;
pub mod translate;
pub mod tv;
pub mod tv_server;

pub use sync::Paths;

/// This build's version (the app's: the workspace's version).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Where the host hears what happened: `event` is one of
/// - "translation": a line to show or hide (`engine::TranslationEvent`; empty `segments`: hide the area);
/// - "screen": a game page to show (`screens::ScreenLayout`), or null when it goes;
/// - "status-msg": a status line (a string: the translator downloading its model …);
/// - "config": the settings changed (a layout update; the settings without secrets);
/// - "pages": the game's pages script changed (null).
///
/// The payload is what the desktop app's window gets for the event of that name.
pub trait Sink: Send + Sync {
    fn emit(&self, event: &str, payload: serde_json::Value);
}

/// A sink that drops everything.
pub struct NoSink;

impl Sink for NoSink {
    fn emit(&self, _event: &str, _payload: serde_json::Value) {}
}

impl Paths {
    /// The folder of the offline translator's model (Arabic's; the other languages get a folder in it).
    pub fn model_dir(&self) -> std::path::PathBuf {
        self.data.join("models").join("opus-mt-en-ar")
    }
}
