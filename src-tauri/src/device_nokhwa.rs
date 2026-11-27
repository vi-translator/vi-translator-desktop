//! Capture cards and other video devices on Linux (V4L2) and macOS (AVFoundation), with nokhwa.
//!
//! Same contract as the Windows version (device.rs): the card's picture is the whole game frame,
//! read at the capture rate into the shared frame slot. MJPEG frames (what most capture cards
//! send) are decoded with the image crate; other formats with nokhwa's own converters.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use nokhwa::Camera;
use nokhwa::pixel_format::RgbAFormat;
use nokhwa::utils::{ApiBackend, CameraIndex, FrameFormat, RequestedFormat, RequestedFormatType};

use crate::capture::{FrameData, Shared, shrink};

/// Prefix that marks a device (not a window) in the config's source field.
pub const PREFIX: &str = "device:";

fn init() {
    #[cfg(target_os = "macos")]
    {
        static ONCE: std::sync::Once = std::sync::Once::new();
        // asks for camera access the first time (capture cards are cameras to macOS)
        ONCE.call_once(|| nokhwa::nokhwa_initialize(|_| {}));
    }
}

fn devices() -> Vec<(String, CameraIndex)> {
    init();
    nokhwa::query(ApiBackend::Auto)
        .unwrap_or_default()
        .into_iter()
        .map(|c| (c.human_name(), c.index().clone()))
        .collect()
}

/// Names of the video devices (capture cards, webcams).
pub fn list() -> Vec<String> {
    devices().into_iter().map(|(n, _)| n).collect()
}

fn open(name: &str) -> Result<Camera> {
    let (_, index) = devices().into_iter().find(|(n, _)| n == name).ok_or_else(|| anyhow!("capture device \"{name}\" not found"))?;
    let mut cam = Camera::new(index, RequestedFormat::new::<RgbAFormat>(RequestedFormatType::AbsoluteHighestResolution))
        .map_err(|e| anyhow!("open \"{name}\": {e}"))?;
    cam.open_stream().map_err(|e| anyhow!("start \"{name}\": {e}"))?;
    Ok(cam)
}

/// One frame as RGBA.
fn read(cam: &mut Camera) -> Result<(u32, u32, Vec<u8>)> {
    let buf = cam.frame().map_err(|e| anyhow!("{e}"))?;
    if buf.source_frame_format() == FrameFormat::MJPEG {
        let img = image::load_from_memory_with_format(buf.buffer(), image::ImageFormat::Jpeg)?.into_rgba8();
        let (w, h) = img.dimensions();
        return Ok((w, h, img.into_raw()));
    }
    let res = buf.resolution();
    if buf.source_frame_format() == FrameFormat::YUYV
        && let Some(rgba) = yuyv_rgba(buf.buffer(), res.width(), res.height())
    {
        return Ok((res.width(), res.height(), rgba));
    }
    let img = buf.decode_image::<RgbAFormat>().map_err(|e| anyhow!("{e}"))?;
    let (w, h) = img.dimensions();
    Ok((w, h, img.into_raw()))
}

/// A YUYV frame as RGBA, in the colour standard of a picture its size: BT.709 for HD, BT.601 below (as the
/// Windows reader takes a card that doesn't say). nokhwa's own converter is BT.601 whatever the size: an HD
/// game's colours came out with their green too low (the pause menu's purple (149, 90, 136) as (144, 81, 134)),
/// and a box drawn in that colour over the game on a TV showed pinker than the game around it.
/// `None`: not a YUYV frame's bytes (another layout under the name), left to nokhwa. (A frame's bytes, and a few
/// more: macOS hands a 1280×720 one over with a row's worth after it, 1 845 760 bytes for 1 843 200. Not half as
/// many again: that is a 4:2:0 layout's size, or a 10-bit one's.)
fn yuyv_rgba(data: &[u8], w: u32, h: u32) -> Option<Vec<u8>> {
    let color = crate::capture::YuvColor { bt709: h >= 720, full_range: false };
    let need = (w * h * 2) as usize;
    (w % 2 == 0 && need > 0 && (need..=need + need / 8).contains(&data.len())).then(|| crate::capture::yuy2_to_rgba(data, w, h, color))
}

pub struct DeviceCapture {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl DeviceCapture {
    pub fn is_finished(&self) -> bool {
        self.thread.as_ref().is_none_or(|t| t.is_finished())
    }
    pub fn stop(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// One frame as RGBA: width, height, pixels.
type Frame = (u32, u32, Vec<u8>);
/// What the card's reader hands over: `None` once the card is open, then its frames.
type FromReader = Result<Option<Frame>>;

/// A card that was sending frames and sends nothing for this long has stopped (they send 25 frames
/// a second or more). Before its first frame a card is waited for as long as it takes: one that
/// sends nothing while the console is off would otherwise be opened again every few seconds.
const STALL: Duration = Duration::from_secs(5);
/// How often a wait for the card looks whether the capture was stopped (the player chose another
/// source): a card that sends nothing (console off) must not keep the switch waiting.
const STOP_CHECK: Duration = Duration::from_millis(250);

/// Read the device in the background until stopped (or it fails: the manager then restarts it).
/// (`_big`, the 4K picture: here the card always runs at its highest resolution; the picture's size
/// is the preview's limit, `capture::preview_w`)
pub fn start(name: String, shared: Arc<Shared>, _big: bool) -> DeviceCapture {
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    let thread = std::thread::spawn(move || {
        // The card is read on a thread of its own, which hands its frames over here. nokhwa waits
        // for the next frame with no time limit, and on macOS a card can stop sending with no error
        // (seen with a USB card: other apps still got its picture). Waited for here, that frame
        // would never come: this thread would never end, the manager would never open the card
        // again, and the last frame would stay as the picture, under "Capturing".
        let (tx, rx) = std::sync::mpsc::sync_channel::<FromReader>(1);
        let card = name.clone();
        let reader = std::thread::spawn(move || {
            let mut cam = match open(&card) {
                Ok(cam) => cam,
                Err(e) => {
                    let _ = tx.send(Err(e));
                    return;
                }
            };
            let _ = tx.send(Ok(None));
            // (ends with the capture: nobody takes the frames any more)
            loop {
                let frame = read(&mut cam).map(Some);
                let failed = frame.is_err();
                if tx.send(frame).is_err() || failed {
                    break;
                }
            }
            let _ = cam.stop_stream();
        });
        take_frames(&rx, &name, &shared, &flag, STALL);
        drop(rx);
        // The card is closed before this capture counts as finished (the manager may open it again
        // at once), but a reader that is stuck isn't waited for.
        // (A known limit: a stuck reader keeps its thread and its hold on the card until a frame
        // comes again. Doing better needs a way to close nokhwa's camera from another thread.)
        let until = Instant::now() + Duration::from_secs(2);
        while !reader.is_finished() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(10));
        }
    });
    DeviceCapture { stop, thread: Some(thread) }
}

/// Put the reader's frames in the shared slots until stopped, the card fails, or it stops sending:
/// nothing for `stall` after its first frame.
fn take_frames(rx: &std::sync::mpsc::Receiver<FromReader>, name: &str, shared: &Shared, stop: &AtomicBool, stall: Duration) {
    use std::sync::mpsc::RecvTimeoutError;
    // The next thing from the reader, waited for at most `limit` (none: as long as it takes), in short
    // steps that notice a stop (then Disconnected: the capture ends; a reader stuck on the card
    // doesn't hold up the switch to another source).
    let wait = |limit: Option<Duration>| -> Result<FromReader, RecvTimeoutError> {
        let start = Instant::now();
        loop {
            let step = limit.map_or(STOP_CHECK, |l| l.saturating_sub(start.elapsed()).min(STOP_CHECK));
            match rx.recv_timeout(step) {
                Err(RecvTimeoutError::Timeout) => {
                    if stop.load(Ordering::Relaxed) {
                        return Err(RecvTimeoutError::Disconnected);
                    }
                    if limit.is_some_and(|l| start.elapsed() >= l) {
                        return Err(RecvTimeoutError::Timeout);
                    }
                }
                r => return r,
            }
        }
    };
    // (opening has no time limit: the first time, macOS asks for camera access and waits for the answer)
    let mut next = wait(None);
    let (mut open, mut sending) = (false, false);
    let ago = Instant::now() - Duration::from_secs(1);
    let (mut last, mut last_preview) = (ago, ago);
    while !stop.load(Ordering::Relaxed) {
        match next {
            Ok(Ok(None)) => {
                open = true;
                shared.set_status(format!("Capturing \"{name}\""));
            }
            // devices push frames at their own rate: every one is read, what's due is kept
            Ok(Ok(Some((w, h, rgba)))) => {
                sending = true;
                if shared.preview_due(last_preview) {
                    last_preview = Instant::now();
                    shared.put_preview(shrink(&rgba, w, h, (w * 4) as usize, false));
                }
                if shared.full_due(last) {
                    last = Instant::now();
                    shared.put_full(w, h, rgba);
                }
            }
            Ok(Err(e)) if open => {
                shared.set_status(format!("Capture device \"{name}\" stopped: {e}"));
                return;
            }
            Ok(Err(e)) => {
                shared.set_status(format!("{e}"));
                std::thread::sleep(Duration::from_secs(3));
                return;
            }
            Err(RecvTimeoutError::Timeout) => {
                shared.set_status(format!("Capture device \"{name}\" stopped sending frames (none for {} s)", stall.as_secs()));
                return;
            }
            Err(RecvTimeoutError::Disconnected) => return,
        }
        next = wait(sending.then_some(stall));
    }
}

/// Capture a single frame from a device (used by `--grab-device`).
pub fn grab_once(name: &str) -> Result<Arc<FrameData>> {
    let mut cam = open(name)?;
    // the first frames of a capture card are often black: skip a few
    let mut frame = read(&mut cam)?;
    for _ in 0..5 {
        frame = read(&mut cam)?;
    }
    let _ = cam.stop_stream();
    Ok(Arc::new(FrameData { w: frame.0, h: frame.1, rgba: frame.2, seq: 1 }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_card_that_goes_quiet_ends_the_capture() {
        // the card opens, sends a frame, then nothing more (and no error): unplugged on macOS
        let (tx, rx) = std::sync::mpsc::sync_channel::<FromReader>(2);
        tx.send(Ok(None)).unwrap();
        tx.send(Ok(Some((2, 1, vec![7; 8])))).unwrap();
        let shared = Shared::default();
        let t = Instant::now();
        take_frames(&rx, "Card", &shared, &AtomicBool::new(false), Duration::from_millis(100));
        assert!(t.elapsed() < Duration::from_secs(2), "it waited for a frame that never comes");
        assert_eq!(shared.latest().map(|f| (f.w, f.h)), Some((2, 1)), "the frame it did send is kept");
        assert_eq!(*shared.status.lock().unwrap(), "Capture device \"Card\" stopped sending frames (none for 0 s)");
        drop(tx);
    }

    #[test]
    fn a_card_is_waited_for_until_its_first_frame() {
        // open, but its first frame comes later than `stall` (no signal yet): not a stopped card
        let (tx, rx) = std::sync::mpsc::sync_channel::<FromReader>(2);
        tx.send(Ok(None)).unwrap();
        let late = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(400));
            tx.send(Ok(Some((2, 1, vec![7; 8])))).unwrap();
            std::thread::sleep(Duration::from_secs(1)); // (kept open: what ends the capture is the silence)
        });
        let shared = Shared::default();
        take_frames(&rx, "Card", &shared, &AtomicBool::new(false), Duration::from_millis(100));
        assert!(shared.latest().is_some(), "it gave up before the first frame");
        late.join().unwrap();
    }

    #[test]
    fn a_card_that_sends_nothing_lets_the_capture_stop() {
        // open, then nothing (console off): stopping (another source chosen) ends the wait at once
        let (tx, rx) = std::sync::mpsc::sync_channel::<FromReader>(2);
        tx.send(Ok(None)).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let s2 = stop.clone();
        let stopper = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            s2.store(true, Ordering::Relaxed);
        });
        let t = Instant::now();
        take_frames(&rx, "Card", &Shared::default(), &stop, Duration::from_secs(5));
        assert!(t.elapsed() < Duration::from_secs(2), "the stop wasn't noticed");
        stopper.join().unwrap();
        drop(tx);
    }

    #[test]
    fn an_hd_card_s_colours_are_bt709() {
        // the pause menu's purple as a 720p card sends it (Y, U, Y, V)
        let px = [107u8, 142, 107, 152];
        let frame: Vec<u8> = px.iter().copied().cycle().take(1280 * 720 * 2).collect();
        assert_eq!(yuyv_rgba(&frame, 1280, 720).unwrap()[..4], [149, 90, 136, 255], "read as BT.601 its green is 81");
        // a picture smaller than HD is BT.601
        assert_eq!(yuyv_rgba(&px, 2, 1).unwrap()[..4], [144, 81, 134, 255]);
        // as macOS hands the frame over: a row's worth more after it (seen with a USB card: 1 845 760 bytes)
        let padded: Vec<u8> = px.iter().copied().cycle().take(1_845_760).collect();
        let rgba = yuyv_rgba(&padded, 1280, 720).expect("the padded frame is still a YUYV frame");
        assert_eq!((rgba.len(), &rgba[rgba.len() - 4..]), (1280 * 720 * 4, &[149, 90, 136, 255][..]));
        // not a YUYV frame's size (4:2:0 under the name, 8 or 10 bits): nokhwa's to decode
        assert!(yuyv_rgba(&frame[..1280 * 720 * 3 / 2], 1280, 720).is_none());
        assert!(yuyv_rgba(&[frame.as_slice(), &frame[..1280 * 720]].concat(), 1280, 720).is_none());
    }

    #[test]
    fn a_card_that_fails_says_why() {
        let (tx, rx) = std::sync::mpsc::sync_channel::<FromReader>(2);
        tx.send(Ok(None)).unwrap();
        tx.send(Err(anyhow!("gone"))).unwrap();
        let shared = Shared::default();
        take_frames(&rx, "Card", &shared, &AtomicBool::new(false), Duration::from_secs(5));
        assert_eq!(*shared.status.lock().unwrap(), "Capture device \"Card\" stopped: gone");
    }
}
