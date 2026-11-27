//! OCR engine on macOS: Apple Vision (VNRecognizeTextRequest, English, accurate).
//!
//! Vision reports lines; each line's box is shared out between its words by character position
//! (close enough for placing words; button icons between words may be found less reliably).
//!
//! Not tested on a Mac yet: built only by the release workflow on macOS runners.

use anyhow::{Result, anyhow};
use objc2::AllocAnyThread;
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::AnyObject;
use objc2_core_foundation::CFData;
use objc2_core_graphics::{CGBitmapInfo, CGColorRenderingIntent, CGColorSpace, CGDataProvider, CGImage, CGImageAlphaInfo};
use objc2_foundation::{NSArray, NSDictionary, NSString};
use objc2_vision::{
    VNImageOption, VNImageRequestHandler, VNRecognizeTextRequest, VNRecognizedTextObservation, VNRequest, VNRequestTextRecognitionLevel,
};

use vi_translator_core::ocr::{RawWord, TextLine};

pub struct Engine;

impl Engine {
    pub fn new() -> Result<Self> {
        Ok(Self)
    }

    pub fn init_thread() {}

    pub fn max_dim() -> u32 {
        4000
    }

    /// Words in a dark-on-light image (4 bytes per pixel, grey), with line numbers and boxes in
    /// image pixels.
    pub fn read(&self, bgra: &[u8], w: u32, h: u32) -> Result<Vec<RawWord>> {
        // the prepared image is grey: one channel is enough
        let grey: Vec<u8> = bgra.chunks_exact(4).map(|p| p[0]).collect();
        autoreleasepool(|_| unsafe { recognize(&grey, w, h) })
    }

    /// The image's lines of text, each as it lies: Vision gives a line's four corners (text on a phone held
    /// tilted; the box round such a line is twice as tall as its letters).
    pub fn read_lines(&self, bgra: &[u8], w: u32, h: u32) -> Result<Vec<TextLine>> {
        let grey: Vec<u8> = bgra.chunks_exact(4).map(|p| p[0]).collect();
        autoreleasepool(|_| unsafe {
            let Some(results) = observe(&grey, w, h)? else { return Ok(Vec::new()) };
            // (normalised points, origin at the bottom left)
            let px = |p: objc2_core_foundation::CGPoint| [p.x * w as f64, (1.0 - p.y) * h as f64];
            let mid = |a: [f64; 2], b: [f64; 2]| [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
            Ok(results
                .iter()
                .filter_map(|obs| {
                    let t = obs.topCandidates(1).firstObject()?.string().to_string();
                    let (tl, tr, bl, br) = (px(obs.topLeft()), px(obs.topRight()), px(obs.bottomLeft()), px(obs.bottomRight()));
                    Some(TextLine { t, l: mid(tl, bl), r: mid(tr, br), h: (tl[0] - bl[0]).hypot(tl[1] - bl[1]) })
                })
                .collect())
        })
    }
}

unsafe fn grey_image(grey: &[u8], w: u32, h: u32) -> Result<objc2_core_foundation::CFRetained<CGImage>> {
    let data = CFData::from_bytes(grey);
    let provider = CGDataProvider::with_cf_data(Some(&data)).ok_or_else(|| anyhow!("OCR: no data provider"))?;
    let space = CGColorSpace::new_device_gray().ok_or_else(|| anyhow!("OCR: no colour space"))?;
    unsafe {
        CGImage::new(
            w as usize,
            h as usize,
            8,
            8,
            w as usize,
            Some(&space),
            CGBitmapInfo(CGImageAlphaInfo::None.0),
            Some(&provider),
            std::ptr::null(),
            false,
            CGColorRenderingIntent::RenderingIntentDefault,
        )
    }
    .ok_or_else(|| anyhow!("OCR: could not make an image"))
}

/// The image's lines of text, as Vision found them.
unsafe fn observe(grey: &[u8], w: u32, h: u32) -> Result<Option<Retained<NSArray<VNRecognizedTextObservation>>>> {
    let image = unsafe { grey_image(grey, w, h)? };
    let request = VNRecognizeTextRequest::new();
    request.setRecognitionLevel(VNRequestTextRecognitionLevel::Accurate);
    request.setRecognitionLanguages(&NSArray::from_retained_slice(&[NSString::from_str("en-US")]));
    request.setUsesLanguageCorrection(true);
    let options: Retained<NSDictionary<VNImageOption, AnyObject>> = NSDictionary::new();
    let handler = unsafe { VNImageRequestHandler::initWithCGImage_options(VNImageRequestHandler::alloc(), &image, &options) };
    let as_request: &VNRequest = request.as_ref();
    handler.performRequests_error(&NSArray::from_slice(&[as_request])).map_err(|e| anyhow!("OCR failed: {e}"))?;
    Ok(request.results())
}

unsafe fn recognize(grey: &[u8], w: u32, h: u32) -> Result<Vec<RawWord>> {
    let mut words = Vec::new();
    let Some(results) = (unsafe { observe(grey, w, h)? }) else { return Ok(words) };
    for (line_no, obs) in results.iter().enumerate() {
        let Some(best) = obs.topCandidates(1).firstObject() else { continue };
        let text = best.string().to_string();
        // normalised box, origin at the bottom left
        let b = unsafe { obs.boundingBox() };
        let (lx, ly, lw, lh) = (
            b.origin.x * w as f64,
            (1.0 - b.origin.y - b.size.height) * h as f64,
            b.size.width * w as f64,
            b.size.height * h as f64,
        );
        let total = text.chars().count().max(1) as f64;
        let mut pos = 0usize;
        // share the line's width out by characters
        for part in text.split(' ') {
            let len = part.chars().count();
            if !part.is_empty() {
                words.push(RawWord {
                    line: line_no as u32,
                    text: part.to_string(),
                    x: lx + lw * pos as f64 / total,
                    y: ly,
                    w: lw * len as f64 / total,
                    h: lh,
                });
            }
            pos += len + 1;
        }
    }
    Ok(words)
}
