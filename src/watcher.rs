//! The watch loop: look, decide, maybe run the command, sleep, repeat.
//!
//! This is what `nodonuts watch` runs in the background.

use crate::camera::{FrameSource, Webcam};
use crate::config::{self, Config};
use crate::enroll;
use crate::face::{self, Embedding, FaceFinder, OnnxFaceFinder};
use crate::files::{self, Paths};
use crate::presence::{self, Rules, Sighting, State, Tracker};
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
            rules: Rules::new(
                config.grace_seconds,
                config.check_every_ms,
                config.aggressive_mode,
            ),
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
        let seen = self.look();
        let (next, fire) = presence::next(self.tracker, seen, now, &self.rules);
        if fire && self.rules.is_intrusion(seen) {
            log::info!("someone else is at the laptop (aggressive mode)");
        } else if next.state != self.tracker.state {
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

    /// Grabs a frame and checks who is in it. Any problem (camera error,
    /// even a panic inside a library) counts as "nobody seen": the grace
    /// period then runs out, failing toward running the command. It never
    /// counts as a stranger, so a broken camera cannot trigger aggressive
    /// mode.
    fn look(&mut self) -> Sighting {
        // `catch_unwind` turns a panic (a crash inside a library) into an
        // `Err` so the watcher keeps running.
        let result = panic::catch_unwind(AssertUnwindSafe(|| -> Result<Sighting> {
            let frame = self.camera.grab()?;
            let faces = self.finder.find_faces(&frame)?;
            Ok(Sighting {
                owner: face::is_owner(&faces, &self.owner, self.threshold),
                stranger: face::has_stranger(&faces, &self.owner, self.threshold),
            })
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
                Sighting::NOBODY
            }
            Err(_) => {
                self.log_error("internal error while checking the camera".to_string());
                Sighting::NOBODY
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
        "watching (grace {} s, check every {} ms, aggressive mode {})",
        config.grace_seconds,
        config.check_every_ms,
        if config.aggressive_mode { "on" } else { "off" }
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

    /// What a scripted frame contains.
    #[derive(Debug, Clone, Copy, PartialEq)]
    enum Scene {
        Owner,
        Empty,
        Stranger,
        OwnerAndStranger,
        CameraError,
    }
    use Scene::*;

    /// Plays back a script of scenes, repeating the last one forever.
    struct ScriptedCamera {
        script: Vec<Scene>,
        position: usize,
        /// Shared with the fake finder, to tell it what is in the frame.
        current: Rc<Cell<Scene>>,
    }

    impl FrameSource for ScriptedCamera {
        fn grab(&mut self) -> Result<RgbImage> {
            let scene = self.script[self.position.min(self.script.len() - 1)];
            self.position += 1;
            if scene == CameraError {
                bail!("camera unplugged");
            }
            self.current.set(scene);
            Ok(RgbImage::new(4, 4))
        }
    }

    struct FakeFinder {
        current: Rc<Cell<Scene>>,
    }

    impl FaceFinder for FakeFinder {
        fn find_faces(&self, _image: &RgbImage) -> Result<Vec<Face>> {
            let owner = Face {
                area: 100.0,
                embedding: unit_vector(0),
            };
            let stranger = Face {
                area: 80.0,
                embedding: unit_vector(1),
            };
            Ok(match self.current.get() {
                Owner => vec![owner],
                Stranger => vec![stranger],
                OwnerAndStranger => vec![owner, stranger],
                Empty | CameraError => Vec::new(),
            })
        }
    }

    /// An embedding pointing along axis `axis`: different axes are
    /// completely different faces.
    fn unit_vector(axis: usize) -> Embedding {
        let mut e = vec![0.0; face::EMBEDDING_LEN];
        e[axis] = 1.0;
        e
    }

    fn watcher(script: Vec<Scene>, grace_seconds: u64, aggressive: bool) -> Watcher {
        let shared = Rc::new(Cell::new(Empty));
        let camera = ScriptedCamera {
            script,
            position: 0,
            current: Rc::clone(&shared),
        };
        let finder = FakeFinder { current: shared };
        let config = Config {
            grace_seconds,
            check_every_ms: 1000,
            aggressive_mode: aggressive,
            // `..` fills the remaining fields from the defaults.
            ..Config::default()
        };
        Watcher::new(Box::new(camera), Box::new(finder), unit_vector(0), &config)
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
        let mut w = watcher(vec![Owner], 3, false);
        assert!(run_seconds(&mut w, 20).is_empty());
        assert_eq!(w.state(), State::Present);
    }

    #[test]
    fn camera_errors_count_as_not_seen() {
        let mut w = watcher(vec![Owner, CameraError], 3, false);
        // Seen at 0 s, errors from 1 s: away at 1 s, fires at 4 s.
        assert_eq!(run_seconds(&mut w, 20), vec![4]);
        assert_eq!(w.state(), State::Fired);
    }

    #[test]
    fn camera_that_fails_then_recovers() {
        // Two failing checks (shorter than the grace period), then the
        // camera recovers and the owner is there again.
        let mut w = watcher(vec![Owner, CameraError, CameraError, Owner], 3, false);
        assert!(run_seconds(&mut w, 20).is_empty());
        assert_eq!(w.state(), State::Present);
    }

    #[test]
    fn long_camera_outage_fires_then_re_arms_on_recovery() {
        let mut script = vec![Owner];
        script.extend(std::iter::repeat_n(CameraError, 10));
        script.push(Owner);
        let mut w = watcher(script, 3, false);
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
            current: Rc::new(Cell::new(Empty)),
        };
        let config = Config {
            grace_seconds: 1,
            check_every_ms: 1000,
            ..Config::default()
        };
        let mut w = Watcher::new(
            Box::new(PanickingCamera),
            Box::new(finder),
            unit_vector(0),
            &config,
        );
        assert_eq!(run_seconds(&mut w, 5), vec![1]);
    }

    #[test]
    fn aggressive_mode_fires_instantly_on_a_stranger() {
        let mut w = watcher(vec![Owner, Owner, Stranger], 30, true);
        assert_eq!(run_seconds(&mut w, 10), vec![2]);
    }

    #[test]
    fn aggressive_mode_ignores_strangers_next_to_the_owner() {
        let mut w = watcher(vec![OwnerAndStranger], 3, true);
        assert!(run_seconds(&mut w, 20).is_empty());
    }

    #[test]
    fn aggressive_mode_never_treats_camera_errors_as_strangers() {
        // Errors still fire, but only after the normal grace period.
        let mut w = watcher(vec![Owner, CameraError], 3, true);
        assert_eq!(run_seconds(&mut w, 20), vec![4]);
    }

    #[test]
    fn strangers_wait_for_the_grace_period_when_aggressive_mode_is_off() {
        let mut w = watcher(vec![Owner, Stranger], 3, false);
        assert_eq!(run_seconds(&mut w, 20), vec![4]);
    }
}
