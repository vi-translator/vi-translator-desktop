//! The platform's OCR engine, for the core's text reading (vi_translator_core::ocr): Windows OCR
//! (ocr_windows.rs), Apple Vision on macOS (ocr_vision.rs), Tesseract on Linux (ocr_tesseract.rs).

use anyhow::Result;
use vi_translator_core::ocr::{Ocr, RawWord, TextEngine};

#[cfg_attr(windows, path = "ocr_windows.rs")]
#[cfg_attr(target_os = "macos", path = "ocr_vision.rs")]
#[cfg_attr(all(not(windows), not(target_os = "macos")), path = "ocr_tesseract.rs")]
mod engine;

impl TextEngine for engine::Engine {
    fn read(&self, bgra: &[u8], w: u32, h: u32) -> Result<Vec<RawWord>> {
        engine::Engine::read(self, bgra, w, h)
    }
    fn max_dim(&self) -> u32 {
        engine::Engine::max_dim()
    }
    // (Apple Vision knows each line's corners: lines as they lie, for text that isn't level)
    #[cfg(target_os = "macos")]
    fn read_lines(&self, bgra: &[u8], w: u32, h: u32) -> Result<Vec<vi_translator_core::ocr::TextLine>> {
        engine::Engine::read_lines(self, bgra, w, h)
    }
}

/// `Ocr::new()` and `Ocr::init_thread()` with this platform's engine.
pub trait PlatformOcr: Sized {
    /// (on Windows, on a thread that has WinRT initialised: see `init_thread`)
    fn new() -> Result<Self>;
    fn init_thread();
}

impl PlatformOcr for Ocr {
    fn new() -> Result<Self> {
        Ok(Ocr::with_engine(Box::new(engine::Engine::new()?)))
    }
    fn init_thread() {
        engine::Engine::init_thread();
    }
}
