//! The camera: open it, grab frames, and reopen it after errors.

use anyhow::{Context, Result, bail};
use image::RgbImage;
use nokhwa::Camera;
use nokhwa::pixel_format::RgbFormat;
use nokhwa::utils::{
    ApiBackend, CameraIndex, FrameFormat, RequestedFormat, RequestedFormatType, Resolution,
};
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

/// The camera chosen in the `camera` setting (the built-in one by default).
///
/// It opens the camera lazily on the first `grab`, and after any error it
/// closes the camera so the next `grab` reopens it. A busy or unplugged
/// camera is therefore just an error for that one check.
pub struct Webcam {
    // `Option` means "maybe a value": `None` while the camera is closed.
    camera: Option<Camera>,
    /// The `camera` setting: `built-in`, or a camera's name.
    setting: String,
    /// The name of the camera last opened, to log changes only.
    last_used: Option<String>,
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
    /// Prepares the camera without opening it. `setting` is the `camera`
    /// setting: `built-in`, or a camera's name.
    pub fn new(setting: &str) -> Webcam {
        Webcam {
            camera: None,
            setting: setting.to_string(),
            last_used: None,
        }
    }
}

impl FrameSource for Webcam {
    fn grab(&mut self) -> Result<RgbImage> {
        if self.camera.is_none() {
            // `?` returns early with the error if opening fails.
            self.camera = Some(open_camera(&self.setting, &mut self.last_used)?);
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

/// The `camera` setting value that means "the laptop's own camera".
pub const BUILT_IN: &str = "built-in";

/// A camera NoDonuts can use (it delivers color frames).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CameraEntry {
    /// The device number (on Linux, N in `/dev/videoN`).
    pub index: u32,
    /// The name the system gives it, e.g. "HP 5MP Camera: HP 5MP Camera".
    pub name: String,
    /// `Some(true)` built in, `Some(false)` external, `None` unknown.
    pub built_in: Option<bool>,
}

/// Lists the cameras NoDonuts can use. Devices that can't take color
/// pictures are left out: on Linux, metadata-only devices and infrared
/// cameras (black and white only, for face login) appear next to the real
/// camera, and the face models need color.
pub fn list_usable() -> Vec<CameraEntry> {
    let Ok(found) = nokhwa::query(ApiBackend::Auto) else {
        return Vec::new();
    };
    let mut usable: Vec<CameraEntry> = found
        .into_iter()
        .filter_map(|info| {
            let CameraIndex::Index(index) = *info.index() else {
                return None; // only numbered devices are supported
            };
            if !delivers_color(index) {
                return None;
            }
            let name = info.human_name();
            Some(CameraEntry {
                index,
                built_in: is_built_in(index, &name),
                name,
            })
        })
        .collect();
    usable.sort_by_key(|camera| camera.index);
    usable
}

/// The built-in camera from `cameras`: the first one known to be built
/// in, else the first one not known to be external, else the first one.
pub fn pick_built_in(cameras: &[CameraEntry]) -> Option<&CameraEntry> {
    cameras
        .iter()
        .find(|c| c.built_in == Some(true))
        .or_else(|| cameras.iter().find(|c| c.built_in != Some(false)))
        .or_else(|| cameras.first())
}

/// What the `camera` setting points at among `cameras`, and whether it had
/// to fall back to the built-in camera because the chosen one is missing
/// (e.g. an unplugged webcam: staying protected beats not watching).
pub fn resolve<'a>(setting: &str, cameras: &'a [CameraEntry]) -> Option<(&'a CameraEntry, bool)> {
    if setting != BUILT_IN
        && let Some(found) = cameras.iter().find(|c| c.name == setting)
    {
        return Some((found, false));
    }
    let fell_back = setting != BUILT_IN;
    pick_built_in(cameras).map(|camera| (camera, fell_back))
}

/// True if the device can deliver color frames (rules out metadata-only
/// devices, which can't be opened as cameras, and infrared cameras).
/// Opening a device without starting it is quick and doesn't light it up.
fn delivers_color(index: u32) -> bool {
    let any = RequestedFormat::new::<RgbFormat>(RequestedFormatType::None);
    // nokhwa can panic on odd devices; treat that as "not usable".
    panic::catch_unwind(AssertUnwindSafe(|| {
        let Ok(mut camera) = Camera::new(CameraIndex::Index(index), any) else {
            return false;
        };
        camera.compatible_fourcc().is_ok_and(|formats| {
            formats
                .iter()
                .any(|format| !matches!(format, FrameFormat::GRAY))
        })
    }))
    .unwrap_or(false)
}

/// Is this the laptop's own camera?
///
/// Linux reports, for each USB port, whether it's built in ("fixed") or a
/// plug-in port ("removable"); laptop cameras sit on a fixed internal port.
#[cfg(target_os = "linux")]
fn is_built_in(index: u32, _name: &str) -> Option<bool> {
    // videoN/device is the camera's USB interface; its parent is the device.
    let path = format!("/sys/class/video4linux/video{index}/device/../removable");
    match std::fs::read_to_string(path).ok()?.trim() {
        "fixed" => Some(true),
        "removable" => Some(false),
        _ => None,
    }
}

/// Is this the Mac's own camera? macOS doesn't tell us directly through the
/// camera library, so go by the usual names. An iPhone used as a webcam
/// (Continuity Camera) is never the built-in one.
#[cfg(not(target_os = "linux"))]
fn is_built_in(_index: u32, name: &str) -> Option<bool> {
    let name = name.to_lowercase();
    if name.contains("iphone") || name.contains("continuity") {
        Some(false)
    } else if name.contains("facetime") || name.contains("macbook") || name.contains("built-in") {
        Some(true)
    } else {
        None
    }
}

/// Opens the camera the `camera` setting points at.
fn open_camera(setting: &str, last_used: &mut Option<String>) -> Result<Camera> {
    let cameras = list_usable();
    let Some((chosen, fell_back)) = resolve(setting, &cameras) else {
        bail!("no usable camera found");
    };
    // Log only when the camera in use changes, not on every reopen.
    if last_used.as_deref() != Some(chosen.name.as_str()) {
        if fell_back {
            log::warn!(
                "camera '{setting}' not found; using the built-in camera '{}'",
                chosen.name
            );
        } else {
            log::info!("using camera '{}'", chosen.name);
        }
        *last_used = Some(chosen.name.clone());
    }
    let index = CameraIndex::Index(chosen.index);
    let preferred = RequestedFormat::new::<RgbFormat>(RequestedFormatType::HighestResolution(
        Resolution::new(PREFERRED_WIDTH, PREFERRED_HEIGHT),
    ));
    // If 640x480 is not offered, let the camera choose.
    let mut camera = Camera::new(index.clone(), preferred)
        .or_else(|_| {
            let any = RequestedFormat::new::<RgbFormat>(RequestedFormatType::None);
            Camera::new(index, any)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn cam(index: u32, name: &str, built_in: Option<bool>) -> CameraEntry {
        CameraEntry {
            index,
            name: name.to_string(),
            built_in,
        }
    }

    #[test]
    fn built_in_is_the_one_marked_built_in() {
        // External plugged in first: still pick the built-in one.
        let cameras = [
            cam(0, "Logitech C920", Some(false)),
            cam(2, "HP 5MP Camera", Some(true)),
        ];
        assert_eq!(pick_built_in(&cameras).unwrap().name, "HP 5MP Camera");
    }

    #[test]
    fn built_in_falls_back_sensibly() {
        // Unknown beats known-external (e.g. an iPhone on a Mac).
        let cameras = [cam(0, "iPhone", Some(false)), cam(1, "Some Camera", None)];
        assert_eq!(pick_built_in(&cameras).unwrap().name, "Some Camera");
        // Only externals: use the first rather than nothing.
        let cameras = [cam(3, "Logitech C920", Some(false))];
        assert_eq!(pick_built_in(&cameras).unwrap().index, 3);
        assert!(pick_built_in(&[]).is_none());
    }

    #[test]
    fn resolve_finds_the_chosen_camera_by_name() {
        let cameras = [
            cam(0, "HP 5MP Camera", Some(true)),
            cam(4, "Logitech C920", Some(false)),
        ];
        let (chosen, fell_back) = resolve("Logitech C920", &cameras).unwrap();
        assert_eq!((chosen.index, fell_back), (4, false));
        let (chosen, fell_back) = resolve(BUILT_IN, &cameras).unwrap();
        assert_eq!((chosen.index, fell_back), (0, false));
    }

    #[test]
    fn a_missing_chosen_camera_falls_back_to_the_built_in_one() {
        let cameras = [cam(0, "HP 5MP Camera", Some(true))];
        let (chosen, fell_back) = resolve("Logitech C920", &cameras).unwrap();
        assert_eq!(chosen.name, "HP 5MP Camera");
        assert!(fell_back, "so the watcher can log it");
        assert!(resolve("Logitech C920", &[]).is_none());
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn mac_names_are_recognized() {
        assert_eq!(is_built_in(0, "FaceTime HD Camera"), Some(true));
        assert_eq!(is_built_in(0, "MacBook Pro Camera"), Some(true));
        assert_eq!(is_built_in(0, "Sam's iPhone Camera"), Some(false));
        assert_eq!(is_built_in(0, "Logitech C920"), None);
    }
}
