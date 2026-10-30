//! The watch loop: look, decide, maybe run the command, sleep, repeat.
//!
//! This is what `nodonuts watch` runs in the background.

use crate::camera::{FrameSource, Webcam};
use crate::config::{self, Config};
use crate::enroll;
use crate::face::{self, Embedding, FaceFinder, OnnxFaceFinder};
use crate::files::{self, Paths};
use crate::presence::{self, Rules, State, Tracker};
use crate::runner;
use anyhow::{Context, Result};
use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};
use std::fs::OpenOptions;
use std::os::unix::fs::OpenOptionsExt;
use std::panic::{self, AssertUnwindSafe};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, SystemTime};

/// Looks at the camera and tracks the owner's presence.
///
/// It owns its camera and face finder as `Box<dyn Trait>`: "some type that
/// implements this trait, stored on the heap". Tests pass in fakes.
pub struct Watcher {
    camera: Box<dyn FrameSource>,
    finder: Box<dyn FaceFinder>,
    owner: Embedding,
    threshold: f64,
    rules: Rules,
    tracker: Tracker,
    /// The last error we logged, so a busy camera does not flood the log.
    last_error: Option<String>,
}

impl std::fmt::Debug for Watcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Watcher")
            .field("tracker", &self.tracker)
            .finish_non_exhaustive()
    }
}

impl Watcher {
    /// Builds a watcher from its parts and the config.
    pub fn new(
        camera: Box<dyn FrameSource>,
        finder: Box<dyn FaceFinder>,
        owner: Embedding,
        config: &Config,
    ) -> Watcher {
        Watcher {
            camera,
            finder,
            owner,
            threshold: config.match_threshold,
            rules: Rules::new(config.grace_seconds, config.check_every_ms),
            tracker: Tracker::new(),
            last_error: None,
        }
    }

    /// The current presence state.
    pub fn state(&self) -> State {
        self.tracker.state
    }

    /// One check at time `now`. Returns true if the command should run.
    pub fn tick(&mut self, now: SystemTime) -> bool {
        let seen = self.owner_seen();
        let (next, fire) = presence::next(self.tracker, seen, now, &self.rules);
        if next.state != self.tracker.state {
            log::info!("{}", describe(next.state));
        }
        self.tracker = next;
        fire
    }

    /// Records that the laptop is awake at `now`. Called right before
    /// sleeping, so a slow check is not mistaken for the laptop sleeping:
    /// only the time spent idle counts toward the wake gap.
    pub fn mark_awake(&mut self, now: SystemTime) {
        self.tracker.last_awake = Some(now);
    }

    /// Grabs a frame and checks it for the owner. Any problem (camera
    /// error, no faces, even a panic inside a library) counts as "not
    /// seen": failing toward running the command is the safe default.
    fn owner_seen(&mut self) -> bool {
        // `catch_unwind` turns a panic (a crash inside a library) into an
        // `Err` so the watcher keeps running.
        let result = panic::catch_unwind(AssertUnwindSafe(|| -> Result<bool> {
            let frame = self.camera.grab()?;
            let faces = self.finder.find_faces(&frame)?;
            Ok(face::is_owner(&faces, &self.owner, self.threshold))
        }));
        match result {
            Ok(Ok(seen)) => {
                if self.last_error.take().is_some() {
                    log::info!("camera is working again");
                }
                seen
            }
            Ok(Err(error)) => {
                self.log_error(format!("{error:#}"));
                false
            }
            Err(_) => {
                self.log_error("internal error while checking the camera".to_string());
                false
            }
        }
    }

    fn log_error(&mut self, message: String) {
        if self.last_error.as_ref() != Some(&message) {
            log::warn!("{message} (counts as owner not seen)");
            self.last_error = Some(message);
        }
    }
}

fn describe(state: State) -> &'static str {
    match state {
        State::Present => "owner is here",
        State::Away { .. } => "owner not seen; grace period started",
        State::Fired => "grace period over",
    }
}

/// `nodonuts watch`: the background loop. Runs until SIGTERM (from
/// `nodonuts stop`) or SIGINT.
pub fn run(paths: &Paths) -> Result<()> {
    start_logging(paths)?;
    // Log startup errors too, since nobody sees this process's stderr.
    let result = watch(paths);
    if let Err(error) = &result {
        log::error!("watcher stopped: {error:#}");
    }
    result
}

fn watch(paths: &Paths) -> Result<()> {
    // Settings are read once: a running watcher keeps the settings it
    // started with.
    let config = config::load(&paths.config_file)?;
    let owner = enroll::load(&paths.face_file)?;

    // An `AtomicBool` is a true/false that is safe to set from a signal
    // handler and read from the loop. `Arc` lets both share it.
    let stop = Arc::new(AtomicBool::new(false));
    signal_hook::flag::register(SIGTERM, Arc::clone(&stop))?;
    signal_hook::flag::register(SIGINT, Arc::clone(&stop))?;
    // Closing the terminal sends SIGHUP; catching it (and ignoring the
    // flag) keeps the watcher alive.
    signal_hook::flag::register(SIGHUP, Arc::new(AtomicBool::new(false)))?;

    let finder = OnnxFaceFinder::load()?;
    let mut watcher = Watcher::new(Box::new(Webcam::new()), Box::new(finder), owner, &config);
    let interval = Duration::from_millis(config.check_every_ms);
    log::info!(
        "watching (grace {} s, check every {} ms)",
        config.grace_seconds,
        config.check_every_ms
    );

    while !stop.load(Ordering::Relaxed) {
        if watcher.tick(SystemTime::now()) {
            runner::run(&config.command);
        }
        watcher.mark_awake(SystemTime::now());
        sleep_unless_stopped(interval, &stop);
    }
    log::info!("stopped");
    Ok(())
}

/// Sleeps for `duration`, waking early if `stop` is set, so `nodonuts stop`
/// is quick even with a long check interval.
fn sleep_unless_stopped(duration: Duration, stop: &AtomicBool) {
    let step = Duration::from_millis(100);
    let mut slept = Duration::ZERO;
    while slept < duration && !stop.load(Ordering::Relaxed) {
        let nap = step.min(duration - slept);
        thread::sleep(nap);
        slept += nap;
    }
}

fn start_logging(paths: &Paths) -> Result<()> {
    files::create_parent_dir(&paths.log_file)?;
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(&paths.log_file)
        .with_context(|| format!("could not open log {}", paths.log_file.display()))?;
    // The model runtime (tract) logs its CPU features at startup; skip that.
    let log_config = simplelog::ConfigBuilder::new()
        .add_filter_ignore_str("tract")
        .build();
    simplelog::WriteLogger::init(log::LevelFilter::Info, log_config, file)
        .context("could not start logging")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::face::Face;
    use anyhow::bail;
    use image::RgbImage;
    use std::cell::Cell;
    use std::rc::Rc;

    /// Plays back a script: `Ok(true)` = frame with the owner,
    /// `Ok(false)` = empty frame, `Err` = camera error.
    struct ScriptedCamera {
        script: Vec<Result<bool, ()>>,
        position: usize,
        /// Shared with the fake finder, to tell it what is in the frame.
        owner_in_frame: Rc<Cell<bool>>,
    }

    impl FrameSource for ScriptedCamera {
        fn grab(&mut self) -> Result<RgbImage> {
            let step = self.script[self.position.min(self.script.len() - 1)];
            self.position += 1;
            match step {
                Ok(owner) => {
                    self.owner_in_frame.set(owner);
                    Ok(RgbImage::new(4, 4))
                }
                Err(()) => bail!("camera unplugged"),
            }
        }
    }

    struct FakeFinder {
        owner_in_frame: Rc<Cell<bool>>,
    }

    impl FaceFinder for FakeFinder {
        fn find_faces(&self, _image: &RgbImage) -> Result<Vec<Face>> {
            if self.owner_in_frame.get() {
                Ok(vec![Face {
                    area: 100.0,
                    embedding: owner_embedding(),
                }])
            } else {
                Ok(Vec::new())
            }
        }
    }

    fn owner_embedding() -> Embedding {
        let mut e = vec![0.0; face::EMBEDDING_LEN];
        e[0] = 1.0;
        e
    }

    fn watcher(script: Vec<Result<bool, ()>>, grace_seconds: u64) -> Watcher {
        let shared = Rc::new(Cell::new(false));
        let camera = ScriptedCamera {
            script,
            position: 0,
            owner_in_frame: Rc::clone(&shared),
        };
        let finder = FakeFinder {
            owner_in_frame: shared,
        };
        Watcher::new(
            Box::new(camera),
            Box::new(finder),
            owner_embedding(),
            &test_config(grace_seconds),
        )
    }

    fn test_config(grace_seconds: u64) -> Config {
        Config {
            grace_seconds,
            check_every_ms: 1000,
            // `..` fills the remaining fields from the defaults.
            ..Config::default()
        }
    }

    /// Runs one tick per second of fake time; returns the seconds at which
    /// the command fired.
    fn run_seconds(watcher: &mut Watcher, seconds: u64) -> Vec<u64> {
        let start = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let mut fired_at = Vec::new();
        for second in 0..seconds {
            let now = start + Duration::from_secs(second);
            if watcher.tick(now) {
                fired_at.push(second);
            }
            watcher.mark_awake(now);
        }
        fired_at
    }

    #[test]
    fn owner_present_never_fires() {
        let mut w = watcher(vec![Ok(true)], 3);
        assert!(run_seconds(&mut w, 20).is_empty());
        assert_eq!(w.state(), State::Present);
    }

    #[test]
    fn camera_errors_count_as_not_seen() {
        let mut w = watcher(vec![Ok(true), Err(())], 3);
        // Seen at 0 s, errors from 1 s: away at 1 s, fires at 4 s.
        assert_eq!(run_seconds(&mut w, 20), vec![4]);
        assert_eq!(w.state(), State::Fired);
    }

    #[test]
    fn camera_that_fails_then_recovers() {
        // Two failing checks (shorter than the grace period), then the
        // camera recovers and the owner is there again.
        let mut w = watcher(vec![Ok(true), Err(()), Err(()), Ok(true)], 3);
        assert!(run_seconds(&mut w, 20).is_empty());
        assert_eq!(w.state(), State::Present);
    }

    #[test]
    fn long_camera_outage_fires_then_re_arms_on_recovery() {
        let mut script = vec![Ok(true)];
        script.extend(std::iter::repeat_n(Err(()), 10));
        script.push(Ok(true));
        let mut w = watcher(script, 3);
        assert_eq!(run_seconds(&mut w, 20), vec![4]);
        assert_eq!(w.state(), State::Present);
    }

    #[test]
    fn panics_in_the_camera_count_as_not_seen() {
        struct PanickingCamera;
        impl FrameSource for PanickingCamera {
            fn grab(&mut self) -> Result<RgbImage> {
                panic!("driver bug");
            }
        }
        let finder = FakeFinder {
            owner_in_frame: Rc::new(Cell::new(false)),
        };
        let mut w = Watcher::new(
            Box::new(PanickingCamera),
            Box::new(finder),
            owner_embedding(),
            &test_config(1),
        );
        assert_eq!(run_seconds(&mut w, 5), vec![1]);
    }
}
