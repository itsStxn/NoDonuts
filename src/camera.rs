//! The camera: open it, grab frames, and reopen it after errors.

use anyhow::{Context, Result, bail};
use image::RgbImage;
use nokhwa::Camera;
use nokhwa::pixel_format::RgbFormat;
use nokhwa::utils::{CameraIndex, RequestedFormat, RequestedFormatType, Resolution};
use std::panic::{self, AssertUnwindSafe};
use std::sync::mpsc;
use std::time::Duration;

/// Anything that can produce camera frames.
///
/// A *trait* is like an interface: it lists methods a type must provide.
/// The watcher only knows about this trait, so tests can hand it a fake
/// camera instead of a real one.
pub trait FrameSource {
    /// Returns the most recent frame, as 8-bit RGB.
    fn grab(&mut self) -> Result<RgbImage>;

    /// Closes the camera so other programs can use it. The next `grab`
    /// opens it again. Sources with nothing to close (like test fakes) can
    /// keep this default, which does nothing.
    fn release(&mut self) {}
}

/// The built-in (first) camera.
///
/// It opens the camera lazily on the first `grab`, and after any error it
/// closes the camera so the next `grab` reopens it. A busy or unplugged
/// camera is therefore just an error for that one check.
pub struct Webcam {
    // `Option` means "maybe a value": `None` while the camera is closed.
    camera: Option<Camera>,
}

// `nokhwa::Camera` does not implement `Debug`, so we write ours by hand.
impl std::fmt::Debug for Webcam {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Webcam")
            .field("open", &self.camera.is_some())
            .finish()
    }
}

/// The camera keeps a small queue of frames (4 on Linux). Frames are
/// captured continuously but we only read one per check, so the queue holds
/// old frames. Reading through it gives us a current one.
const FRAMES_TO_SKIP: usize = 4;

/// 640x480 is offered by practically every webcam and is plenty for face
/// detection (the detector works at 640x640 anyway).
const PREFERRED_WIDTH: u32 = 640;
const PREFERRED_HEIGHT: u32 = 480;

impl Webcam {
    /// Prepares the camera without opening it.
    pub fn new() -> Webcam {
        Webcam { camera: None }
    }
}

impl Default for Webcam {
    fn default() -> Self {
        Webcam::new()
    }
}

impl FrameSource for Webcam {
    fn grab(&mut self) -> Result<RgbImage> {
        if self.camera.is_none() {
            // `?` returns early with the error if opening fails.
            self.camera = Some(open_camera()?);
        }
        let result = match &mut self.camera {
            Some(camera) => grab_latest(camera),
            None => bail!("camera is not open"),
        };
        if result.is_err() {
            // Close the camera. The next grab reopens it.
            self.release();
        }
        result
    }

    fn release(&mut self) {
        // `take()` leaves `None` behind, so the camera counts as closed
        // even if closing it goes wrong below.
        if let Some(camera) = self.camera.take() {
            // Dropping a `Camera` closes it. nokhwa's drop code can panic if
            // stopping the stream fails; catch that so the watcher survives.
            if panic::catch_unwind(AssertUnwindSafe(move || drop(camera))).is_err() {
                log::warn!("error while closing the camera (ignored)");
            }
        }
    }
}

/// On macOS, asks for camera permission (a no-op elsewhere). Waits up to a
/// minute for the user to answer the system prompt.
pub fn request_permission() -> Result<()> {
    if nokhwa::nokhwa_check() {
        return Ok(());
    }
    // A channel lets the permission callback (run by the OS on another
    // thread) send its answer back to us.
    let (sender, receiver) = mpsc::channel();
    nokhwa::nokhwa_initialize(move |granted| {
        // Ignore send errors: they only mean we stopped waiting.
        let _ = sender.send(granted);
    });
    match receiver.recv_timeout(Duration::from_secs(60)) {
        Ok(true) => Ok(()),
        _ => bail!(
            "no camera permission. Allow your terminal app in \
             System Settings > Privacy & Security > Camera"
        ),
    }
}

fn open_camera() -> Result<Camera> {
    let preferred = RequestedFormat::new::<RgbFormat>(RequestedFormatType::HighestResolution(
        Resolution::new(PREFERRED_WIDTH, PREFERRED_HEIGHT),
    ));
    // If 640x480 is not offered, let the camera choose.
    let mut camera = Camera::new(CameraIndex::Index(0), preferred)
        .or_else(|_| {
            let any = RequestedFormat::new::<RgbFormat>(RequestedFormatType::None);
            Camera::new(CameraIndex::Index(0), any)
        })
        .context("could not open the camera (is it in use, or missing?)")?;
    camera
        .open_stream()
        .context("could not start the camera stream (is it in use?)")?;
    Ok(camera)
}

fn grab_latest(camera: &mut Camera) -> Result<RgbImage> {
    let mut frame = camera.frame().context("could not read a camera frame")?;
    for _ in 1..FRAMES_TO_SKIP {
        frame = camera.frame().context("could not read a camera frame")?;
    }
    // Convert the camera's format (usually MJPEG or YUYV) to RGB.
    let image = frame
        .decode_image::<RgbFormat>()
        .context("could not decode the camera frame")?;
    Ok(image)
}
