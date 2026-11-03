//! The watch loop: look, decide, maybe run the command, sleep, repeat.
//!
//! This is what `nodonuts watch` runs in the background.

use crate::camera::{FrameSource, Webcam};
use crate::config::{self, Config};
use crate::control::{Reply, STOP_REQUEST};
use crate::enroll;
use crate::face::{self, Embedding, FaceFinder, OnnxFaceFinder};
use crate::files::{self, Paths};
use crate::password;
use crate::presence::{self, Rules, Sighting, State, Tracker};
use crate::runner;
use anyhow::{Context, Result};
use signal_hook::consts::{
    SIGABRT, SIGALRM, SIGHUP, SIGINT, SIGIO, SIGPROF, SIGQUIT, SIGTERM, SIGTSTP, SIGTTIN, SIGTTOU,
    SIGUSR1, SIGUSR2, SIGVTALRM, SIGXCPU, SIGXFSZ,
};
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::panic::{self, AssertUnwindSafe};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

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
                config.aggressive_checks,
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
        if fire && self.rules.intrusion_confirmed(seen, next.stranger_streak) {
            log::info!(
                "someone else is at the laptop for {} checks in a row (aggressive mode)",
                next.stranger_streak
            );
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

    /// Closes the camera so the owner's command can use it. The next check
    /// opens it again.
    pub fn release_camera(&mut self) {
        self.camera.release();
    }

    /// `face_checkout`: decides how a stop request ends. Looks at the camera
    /// up to `attempts` times, `pause` apart, and stops looking as soon as
    /// the owner is seen.
    pub fn check_before_stop(&mut self, attempts: u32, pause: Duration) -> StopOutcome {
        for attempt in 0..attempts {
            if attempt > 0 {
                thread::sleep(pause);
            }
            if self.look().owner {
                return StopOutcome::OwnerConfirmed;
            }
        }
        StopOutcome::OwnerNotSeen
    }

    /// Handles one connection on the control socket (see the `control`
    /// module): a `nodonuts stop` asking to stop.
    ///
    /// Runs up to `face_checks` face checkouts, then, if a fallback
    /// password is set, gives up to `password_tries` tries to type it.
    /// Returns `None` if the connection wasn't a proper stop request; the
    /// watcher then simply carries on.
    pub fn handle_stop_request(
        &mut self,
        connection: UnixStream,
        rules: &StopRules,
        password_hash: Option<&str>,
    ) -> Option<StopOutcome> {
        // Two handles on one connection: one to read lines, one to write.
        let mut reader = BufReader::new(connection.try_clone().ok()?);
        let mut writer = connection;
        // Writing can only fail if `stop` went away (e.g. Ctrl+C). Then
        // nobody can confirm anything, which ends in the command running.
        let mut send = |reply: Reply| {
            let _ = writer.write_all(reply.to_line().as_bytes());
        };

        // Anything other than a prompt `STOP` is ignored, so a stray
        // connection can't hold the watcher up.
        let _ = reader
            .get_ref()
            .set_read_timeout(Some(rules.request_timeout));
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(read) if read > 0 && line.trim() == STOP_REQUEST => {}
            _ => return None,
        }
        log::info!("stop requested by `nodonuts stop`");

        if !rules.face_checkout {
            send(Reply::Done(StopOutcome::Unchecked.as_str().to_string()));
            return Some(StopOutcome::Unchecked);
        }

        for attempt in 1..=rules.face_checks {
            send(Reply::Looking {
                attempt,
                of: rules.face_checks,
            });
            if self.check_before_stop(rules.looks, rules.pause) == StopOutcome::OwnerConfirmed {
                send(Reply::Done(
                    StopOutcome::OwnerConfirmed.as_str().to_string(),
                ));
                return Some(StopOutcome::OwnerConfirmed);
            }
            log::info!(
                "face checkout {attempt}/{}: owner not seen",
                rules.face_checks
            );
            send(Reply::NotSeen {
                attempt,
                of: rules.face_checks,
            });
        }

        if let Some(hash) = password_hash {
            // One deadline for all tries, so someone sitting at the prompt
            // can't keep the watcher (and the command) waiting for long.
            let deadline = Instant::now() + rules.password_window;
            for attempt in 1..=rules.password_tries {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    log::warn!("no fallback password typed in time");
                    break;
                }
                send(Reply::Password {
                    attempt,
                    of: rules.password_tries,
                });
                let _ = reader.get_ref().set_read_timeout(Some(remaining));
                line.clear();
                match reader.read_line(&mut line) {
                    Ok(read) if read > 0 => {}
                    // Timed out, or `stop` went away: no more tries.
                    _ => break,
                }
                let typed = line.trim_end_matches(['\n', '\r']);
                if password::verify(typed, hash) {
                    log::info!("fallback password accepted; stopping");
                    send(Reply::Done(
                        StopOutcome::PasswordConfirmed.as_str().to_string(),
                    ));
                    return Some(StopOutcome::PasswordConfirmed);
                }
                log::warn!(
                    "wrong fallback password ({attempt}/{})",
                    rules.password_tries
                );
                send(Reply::Wrong);
            }
        }

        // Not the owner, as far as we can tell: refuse to stop. The caller
        // runs the command and carries on watching.
        send(Reply::Done(StopOutcome::Denied.as_str().to_string()));
        Some(StopOutcome::Denied)
    }

    /// After a refused stop: the command has run for this absence, so treat
    /// it like a normal fire (no second fire until the owner is seen again).
    pub fn mark_fired(&mut self) {
        self.tracker.state = State::Fired;
        self.tracker.stranger_streak = 0;
    }

    fn log_error(&mut self, message: String) {
        if self.last_error.as_ref() != Some(&message) {
            log::warn!("{message} (counts as owner not seen)");
            self.last_error = Some(message);
        }
    }
}

/// How a stop request ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopOutcome {
    /// `face_checkout` is off: stopped without looking.
    Unchecked,
    /// `face_checkout` is on and the owner was seen: stopped quietly.
    OwnerConfirmed,
    /// The face wasn't recognized, but the fallback password was right:
    /// stopped quietly.
    PasswordConfirmed,
    /// `face_checkout` is on and the owner was not seen: the command ran, then
    /// the watcher stopped. (Stop signals only: nobody can type a password.)
    OwnerNotSeen,
    /// `nodonuts stop` failed the checkout (face and password): the command
    /// ran, and the watcher did NOT stop. The next `nodonuts stop` starts
    /// over.
    Denied,
}

impl StopOutcome {
    /// The word sent to (or left for) `nodonuts stop` to report.
    pub fn as_str(self) -> &'static str {
        match self {
            StopOutcome::Unchecked => "unchecked",
            StopOutcome::OwnerConfirmed => "owner",
            StopOutcome::PasswordConfirmed => "password",
            StopOutcome::OwnerNotSeen => "command",
            StopOutcome::Denied => "denied",
        }
    }
}

/// How a stop request through `nodonuts stop` is checked.
#[derive(Debug, Clone)]
pub struct StopRules {
    /// Whether to check at all (`face_checkout`).
    pub face_checkout: bool,
    /// Face checkout attempts before asking for the password (`face_checks`).
    pub face_checks: u32,
    /// Camera looks per attempt.
    pub looks: u32,
    /// Pause between looks.
    pub pause: Duration,
    /// How many password tries.
    pub password_tries: u32,
    /// Total time allowed for all password tries.
    pub password_window: Duration,
    /// How long to wait for `STOP` after a connection opens.
    pub request_timeout: Duration,
}

impl StopRules {
    /// The rules for the real watcher.
    pub fn from_config(config: &Config) -> StopRules {
        StopRules {
            face_checkout: config.face_checkout,
            // `try_from` can't fail for the validated 1..=10, but stays
            // safe if it ever could.
            face_checks: u32::try_from(config.face_checks).unwrap_or(10),
            looks: FACE_CHECKOUT_LOOKS,
            pause: FACE_CHECKOUT_PAUSE,
            // One try: a wrong password runs the command (the next
            // `nodonuts stop` can try again).
            password_tries: 1,
            password_window: Duration::from_secs(30),
            request_timeout: Duration::from_secs(5),
        }
    }
}

/// One face checkout looks at the camera up to 8 times, 250 ms apart. Each
/// look also takes time (frames + face models), so this is about 3 s in
/// total on a typical laptop. It stops early as soon as the owner is seen.
const FACE_CHECKOUT_LOOKS: u32 = 8;
const FACE_CHECKOUT_PAUSE: Duration = Duration::from_millis(250);
/// How long to wait for the command before exiting. Kept short so a slow
/// command cannot hold up a logout or shutdown.
const COMMAND_WAIT_BEFORE_EXIT: Duration = Duration::from_secs(5);

/// The signals that ask the watcher to stop. `nodonuts stop` and plain
/// `kill`/`pkill` send SIGTERM, but `pkill` can send any signal, and most of
/// them end a program that doesn't catch them. Catching them all here means
/// they all go through `face_checkout`.
///
/// What can't be caught: SIGKILL (`kill -9`) and SIGSTOP (`kill -STOP`), by
/// design of the OS; and crash signals like SIGSEGV or SIGBUS, which are
/// unsafe to catch.
fn stop_signals() -> Vec<i32> {
    let mut signals = vec![
        SIGTERM, SIGINT, SIGQUIT, SIGUSR1, SIGUSR2, SIGALRM, SIGVTALRM, SIGPROF, SIGXCPU, SIGXFSZ,
        SIGIO, SIGABRT,
        // These pause a program instead of ending it, so `pkill -TSTP` would
        // freeze the watcher like `kill -STOP`.
        SIGTSTP, SIGTTIN, SIGTTOU,
    ];
    // Linux has a few more signals that end a program, including the
    // "real-time" range (`pkill -RTMIN ...`).
    #[cfg(target_os = "linux")]
    {
        use nix::sys::signal::Signal;
        signals.push(Signal::SIGPWR as i32);
        signals.push(Signal::SIGSTKFLT as i32);
        signals.extend(nix::libc::SIGRTMIN()..=nix::libc::SIGRTMAX());
    }
    signals
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
    // Kept in memory: changing the file while we run has no effect.
    let password_hash = password::load(&paths.password_file)?;

    // An `AtomicBool` is a true/false that is safe to set from a signal
    // handler and read from the loop. `Arc` lets both share it.
    let stop = Arc::new(AtomicBool::new(false));
    for signal in stop_signals() {
        signal_hook::flag::register(signal, Arc::clone(&stop))
            .with_context(|| format!("could not catch signal {signal}"))?;
    }
    // Closing the terminal sends SIGHUP; catching it (and ignoring the
    // flag) keeps the watcher alive.
    signal_hook::flag::register(SIGHUP, Arc::new(AtomicBool::new(false)))?;

    let finder = OnnxFaceFinder::load()?;
    let mut watcher = Watcher::new(Box::new(Webcam::new()), Box::new(finder), owner, &config);
    let interval = Duration::from_millis(config.check_every_ms);
    let aggressive = if config.aggressive_mode {
        format!("on, {} checks", config.aggressive_checks)
    } else {
        "off".to_string()
    };
    log::info!(
        "watching (grace {} s, check every {} ms, aggressive mode {aggressive}, face checkout {})",
        config.grace_seconds,
        config.check_every_ms,
        if config.face_checkout { "on" } else { "off" },
    );

    // `nodonuts stop` talks to us over this socket. Without it, `stop` falls
    // back to a plain signal, so failing to set it up isn't fatal.
    let requested = Arc::new(AtomicBool::new(false));
    let requests = match listen_for_stop_requests(&paths.control_socket, Arc::clone(&requested)) {
        Ok(requests) => Some(requests),
        Err(error) => {
            log::warn!("{error:#}; `nodonuts stop` will use a plain signal");
            None
        }
    };
    let rules = StopRules::from_config(&config);
    if config.face_checkout && password_hash.is_none() {
        log::warn!("no fallback password set: a failed face checkout can't be overridden");
    }

    // Leave the loop when a signal arrives or `nodonuts stop` connects.
    let interrupted = || stop.load(Ordering::Relaxed) || requested.load(Ordering::Relaxed);
    // The owner's command, once it has run (it may still be running if we
    // stopped waiting for it).
    let mut command: Option<runner::RunningCommand> = None;

    let outcome = 'watching: loop {
        while !interrupted() {
            if watcher.tick(SystemTime::now()) {
                // Hand the camera over to the command (e.g. a script that
                // takes a photo) and wait for it. No checks happen meanwhile;
                // the command only runs once per absence, so nothing is
                // missed.
                watcher.release_camera();
                log::info!("camera released until the command finishes");
                let running = runner::start(&config.command);
                running.wait_unless(&interrupted);
                command = Some(running);
            }
            // After the command: its run time is not mistaken for a sleep gap.
            watcher.mark_awake(SystemTime::now());
            sleep_unless(interval, &interrupted);
        }

        // A signal: one face checkout, no password (nobody can type one).
        if stop.load(Ordering::Relaxed) {
            break 'watching if config.face_checkout {
                log::info!("stop signal received; checking for the owner's face");
                watcher.check_before_stop(rules.looks, rules.pause)
            } else {
                StopOutcome::Unchecked
            };
        }

        // `nodonuts stop` connected. Clear the flag *before* taking the
        // connections, so one arriving meanwhile sets it again.
        requested.store(false, Ordering::Relaxed);
        if let Some(requests) = &requests {
            while let Ok(connection) = requests.try_recv() {
                match watcher.handle_stop_request(connection, &rules, password_hash.as_deref()) {
                    // Not a real stop request: ignore it.
                    None => {}
                    // Checkout failed: run the command, keep watching.
                    Some(StopOutcome::Denied) => {
                        let still_running = command.as_ref().is_some_and(|c| !c.is_done());
                        if still_running {
                            log::warn!(
                                "stop refused: owner not recognized; the command is already running"
                            );
                        } else {
                            log::warn!("stop refused: owner not recognized; running the command");
                            watcher.release_camera();
                            let running = runner::start(&config.command);
                            running.wait_unless(&interrupted);
                            command = Some(running);
                        }
                        watcher.mark_fired();
                        // The checkout took a while: not a sleep gap.
                        watcher.mark_awake(SystemTime::now());
                    }
                    Some(outcome) => break 'watching outcome,
                }
            }
        }
        // Carry on watching.
    };

    // With `face_checkout`, only the owner's face (or the fallback
    // password) ends the watcher quietly; otherwise the command runs first.
    let command_still_running = command.as_ref().is_some_and(|running| !running.is_done());
    match outcome {
        StopOutcome::Unchecked => {}
        StopOutcome::OwnerConfirmed => log::info!("owner recognized; stopping"),
        // `Denied` never gets here: it doesn't end the loop.
        StopOutcome::PasswordConfirmed | StopOutcome::Denied => {}
        StopOutcome::OwnerNotSeen if command_still_running => {
            // Running a second copy wouldn't add anything.
            log::warn!("stop requested but the owner was not seen; the command is already running");
        }
        StopOutcome::OwnerNotSeen => {
            log::warn!("stop requested but the owner was not seen; running the command first");
            watcher.release_camera();
            runner::run_and_wait(&config.command, COMMAND_WAIT_BEFORE_EXIT);
        }
    }
    // For a signal-based `nodonuts stop` to report. Not being able to write
    // it must not keep the watcher running.
    if let Err(error) = files::write_private(&paths.stop_result_file, outcome.as_str().as_bytes()) {
        log::warn!("could not record how the stop went: {error:#}");
    }
    let _ = std::fs::remove_file(&paths.control_socket);
    log::info!("stopped");
    Ok(())
}

/// Starts listening on the control socket. A helper thread accepts each
/// connection, hands it over through the returned channel, and sets
/// `requested` so the watch loop notices within 100 ms.
fn listen_for_stop_requests(
    path: &Path,
    requested: Arc<AtomicBool>,
) -> Result<Receiver<UnixStream>> {
    files::create_parent_dir(path)?;
    // A leftover socket file from a watcher that crashed would block `bind`.
    let _ = std::fs::remove_file(path);
    let listener = UnixListener::bind(path)
        .with_context(|| format!("could not listen on {}", path.display()))?;
    // Owner-only, like every NoDonuts file (the folder is owner-only too).
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .with_context(|| format!("could not set permissions on {}", path.display()))?;

    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        // `flatten()` skips failed accepts and keeps the good connections.
        for connection in listener.incoming().flatten() {
            if sender.send(connection).is_err() {
                break; // the watcher is exiting
            }
            requested.store(true, Ordering::Relaxed);
        }
    });
    Ok(receiver)
}

/// Sleeps for `duration`, waking early once `interrupted()` is true, so
/// `nodonuts stop` is quick even with a long check interval.
fn sleep_unless(duration: Duration, interrupted: &dyn Fn() -> bool) {
    let step = Duration::from_millis(100);
    let mut slept = Duration::ZERO;
    while slept < duration && !interrupted() {
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
    fn aggressive_mode_fires_after_the_default_two_stranger_checks() {
        // Stranger from 2 s: 1st check at 2 s, 2nd (confirming) at 3 s.
        let mut w = watcher(vec![Owner, Owner, Stranger], 30, true);
        assert_eq!(run_seconds(&mut w, 10), vec![3]);
    }

    #[test]
    fn aggressive_mode_ignores_a_single_misread_frame() {
        let mut w = watcher(vec![Owner, Stranger, Owner], 30, true);
        assert!(run_seconds(&mut w, 10).is_empty());
        assert_eq!(w.state(), State::Present);
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
    fn face_checkout_stops_quietly_when_the_owner_is_seen() {
        let mut w = watcher(vec![Owner], 30, false);
        assert_eq!(
            w.check_before_stop(12, Duration::ZERO),
            StopOutcome::OwnerConfirmed
        );
    }

    #[test]
    fn face_checkout_runs_the_command_when_the_owner_is_not_seen() {
        for scene in [Empty, Stranger, CameraError] {
            let mut w = watcher(vec![scene], 30, false);
            assert_eq!(
                w.check_before_stop(12, Duration::ZERO),
                StopOutcome::OwnerNotSeen,
                "{scene:?}"
            );
        }
    }

    #[test]
    fn face_checkout_waits_for_the_owner_to_look_at_the_camera() {
        // Seen on the 3rd look, within the 12 allowed.
        let mut w = watcher(vec![Empty, Stranger, Owner], 30, false);
        assert_eq!(
            w.check_before_stop(12, Duration::ZERO),
            StopOutcome::OwnerConfirmed
        );
    }

    #[test]
    fn face_checkout_gives_up_after_the_allowed_looks() {
        // Owner only on the 4th look, but only 3 are allowed.
        let mut w = watcher(vec![Empty, Empty, Empty, Owner], 30, false);
        assert_eq!(
            w.check_before_stop(3, Duration::ZERO),
            StopOutcome::OwnerNotSeen
        );
    }

    #[test]
    fn owner_next_to_a_stranger_counts_as_confirmed() {
        let mut w = watcher(vec![OwnerAndStranger], 30, false);
        assert_eq!(
            w.check_before_stop(12, Duration::ZERO),
            StopOutcome::OwnerConfirmed
        );
    }

    #[test]
    fn release_camera_closes_it_and_checks_carry_on_afterwards() {
        /// A camera that records whether it is open.
        struct TrackedCamera {
            open: Rc<Cell<bool>>,
            releases: Rc<Cell<u32>>,
        }
        impl FrameSource for TrackedCamera {
            fn grab(&mut self) -> Result<RgbImage> {
                self.open.set(true);
                Ok(RgbImage::new(4, 4))
            }
            fn release(&mut self) {
                self.open.set(false);
                self.releases.set(self.releases.get() + 1);
            }
        }
        let open = Rc::new(Cell::new(false));
        let releases = Rc::new(Cell::new(0));
        let camera = TrackedCamera {
            open: Rc::clone(&open),
            releases: Rc::clone(&releases),
        };
        let finder = FakeFinder {
            current: Rc::new(Cell::new(Owner)),
        };
        let mut w = Watcher::new(
            Box::new(camera),
            Box::new(finder),
            unit_vector(0),
            &Config::default(),
        );
        run_seconds(&mut w, 1);
        assert!(open.get(), "a check opens the camera");
        w.release_camera();
        assert!(!open.get(), "released for the command");
        assert_eq!(releases.get(), 1);
        run_seconds(&mut w, 1);
        assert!(open.get(), "the next check reopens it");
        assert_eq!(w.state(), State::Present);
    }

    // --- `nodonuts stop` over the control socket -------------------------

    /// Fast rules for tests: one look per face checkout, no pauses.
    fn fast_rules(face_checks: u32) -> StopRules {
        StopRules {
            face_checkout: true,
            face_checks,
            looks: 1,
            pause: Duration::ZERO,
            password_tries: 3,
            password_window: Duration::from_secs(10),
            request_timeout: Duration::from_secs(2),
        }
    }

    /// Plays `nodonuts stop` on another thread: sends `first_line`, answers
    /// each password prompt with the next of `passwords` (hanging up when
    /// they run out), and returns every reply it got.
    fn scripted_stop(
        first_line: &'static str,
        passwords: Vec<&'static str>,
    ) -> (UnixStream, thread::JoinHandle<Vec<Reply>>) {
        let (watcher_side, mut stop_side) = UnixStream::pair().unwrap();
        let client = thread::spawn(move || {
            stop_side.write_all(first_line.as_bytes()).unwrap();
            let mut passwords = passwords.into_iter();
            let mut replies = Vec::new();
            let reader = BufReader::new(stop_side.try_clone().unwrap());
            for line in reader.lines() {
                let Ok(line) = line else { break };
                let reply = Reply::parse(&line).unwrap();
                replies.push(reply.clone());
                match reply {
                    Reply::Password { .. } => match passwords.next() {
                        Some(typed) => stop_side
                            .write_all(format!("{typed}\n").as_bytes())
                            .unwrap(),
                        None => break, // hang up, like Ctrl+C
                    },
                    Reply::Done(_) => break,
                    _ => {}
                }
            }
            replies
        });
        (watcher_side, client)
    }

    fn done(word: &str) -> Reply {
        Reply::Done(word.to_string())
    }

    #[test]
    fn stop_request_owner_seen_stops_quietly() {
        let mut w = watcher(vec![Owner], 30, false);
        let (connection, client) = scripted_stop("STOP\n", vec![]);
        let outcome = w.handle_stop_request(connection, &fast_rules(3), None);
        assert_eq!(outcome, Some(StopOutcome::OwnerConfirmed));
        assert_eq!(
            client.join().unwrap(),
            vec![Reply::Looking { attempt: 1, of: 3 }, done("owner")]
        );
    }

    #[test]
    fn stop_request_retries_the_face_before_giving_up_on_it() {
        // Not seen on the 1st face checkout, seen on the 2nd.
        let mut w = watcher(vec![Empty, Owner], 30, false);
        let (connection, client) = scripted_stop("STOP\n", vec![]);
        let outcome = w.handle_stop_request(connection, &fast_rules(3), None);
        assert_eq!(outcome, Some(StopOutcome::OwnerConfirmed));
        assert_eq!(
            client.join().unwrap(),
            vec![
                Reply::Looking { attempt: 1, of: 3 },
                Reply::NotSeen { attempt: 1, of: 3 },
                Reply::Looking { attempt: 2, of: 3 },
                done("owner"),
            ]
        );
    }

    #[test]
    fn stop_request_falls_back_to_the_password() {
        let hash = password::hash("right one").unwrap();
        let mut w = watcher(vec![Stranger], 30, false);
        let (connection, client) = scripted_stop("STOP\n", vec!["wrong", "right one"]);
        let outcome = w.handle_stop_request(connection, &fast_rules(2), Some(&hash));
        assert_eq!(outcome, Some(StopOutcome::PasswordConfirmed));
        assert_eq!(
            client.join().unwrap(),
            vec![
                Reply::Looking { attempt: 1, of: 2 },
                Reply::NotSeen { attempt: 1, of: 2 },
                Reply::Looking { attempt: 2, of: 2 },
                Reply::NotSeen { attempt: 2, of: 2 },
                Reply::Password { attempt: 1, of: 3 },
                Reply::Wrong,
                Reply::Password { attempt: 2, of: 3 },
                done("password"),
            ]
        );
    }

    #[test]
    fn three_wrong_passwords_run_the_command() {
        let hash = password::hash("right").unwrap();
        let mut w = watcher(vec![Empty], 30, false);
        let (connection, client) = scripted_stop("STOP\n", vec!["a", "b", "c", "right"]);
        let outcome = w.handle_stop_request(connection, &fast_rules(1), Some(&hash));
        assert_eq!(outcome, Some(StopOutcome::Denied));
        let replies = client.join().unwrap();
        assert_eq!(replies.last(), Some(&done("denied")));
        assert_eq!(
            replies.iter().filter(|r| **r == Reply::Wrong).count(),
            3,
            "a 4th try is never offered"
        );
    }

    #[test]
    fn without_a_password_a_failed_face_checkout_runs_the_command() {
        let mut w = watcher(vec![Empty], 30, false);
        let (connection, client) = scripted_stop("STOP\n", vec![]);
        let outcome = w.handle_stop_request(connection, &fast_rules(2), None);
        assert_eq!(outcome, Some(StopOutcome::Denied));
        let replies = client.join().unwrap();
        assert!(!replies.iter().any(|r| matches!(r, Reply::Password { .. })));
        assert_eq!(replies.last(), Some(&done("denied")));
    }

    #[test]
    fn hanging_up_at_the_password_prompt_runs_the_command() {
        let hash = password::hash("right").unwrap();
        let mut w = watcher(vec![Empty], 30, false);
        let (connection, client) = scripted_stop("STOP\n", vec![]); // no answers
        let outcome = w.handle_stop_request(connection, &fast_rules(1), Some(&hash));
        assert_eq!(outcome, Some(StopOutcome::Denied));
        client.join().unwrap();
    }

    #[test]
    fn the_real_rules_ask_for_the_password_once() {
        let rules = StopRules::from_config(&Config::default());
        assert_eq!(rules.password_tries, 1);
        assert_eq!(rules.face_checks, 3);

        // One wrong password is enough to refuse the stop.
        let hash = password::hash("right").unwrap();
        let mut w = watcher(vec![Empty], 30, false);
        let (connection, client) = scripted_stop("STOP\n", vec!["wrong", "right"]);
        let fast = StopRules {
            looks: 1,
            pause: Duration::ZERO,
            ..rules
        };
        assert_eq!(
            w.handle_stop_request(connection, &fast, Some(&hash)),
            Some(StopOutcome::Denied)
        );
        let replies = client.join().unwrap();
        assert_eq!(
            replies
                .iter()
                .filter(|r| matches!(r, Reply::Password { .. }))
                .count(),
            1,
            "asked once, after all {} face checks",
            fast.face_checks
        );
        assert_eq!(replies.last(), Some(&done("denied")));
    }

    #[test]
    fn a_refused_stop_can_be_retried_from_scratch() {
        let hash = password::hash("right").unwrap();
        let mut w = watcher(vec![Empty], 30, false);
        // 1st `nodonuts stop`: three wrong passwords, refused.
        let (connection, client) = scripted_stop("STOP\n", vec!["a", "b", "c"]);
        assert_eq!(
            w.handle_stop_request(connection, &fast_rules(2), Some(&hash)),
            Some(StopOutcome::Denied)
        );
        client.join().unwrap();
        w.mark_fired();
        assert_eq!(w.state(), State::Fired);
        // 2nd `nodonuts stop`: full face checks and 3 fresh tries again.
        let (connection, client) = scripted_stop("STOP\n", vec!["x", "y", "right"]);
        assert_eq!(
            w.handle_stop_request(connection, &fast_rules(2), Some(&hash)),
            Some(StopOutcome::PasswordConfirmed)
        );
        let replies = client.join().unwrap();
        assert_eq!(replies[0], Reply::Looking { attempt: 1, of: 2 });
        assert!(replies.contains(&Reply::Password { attempt: 3, of: 3 }));
    }

    #[test]
    fn after_a_refused_stop_the_grace_period_does_not_fire_again() {
        // Nobody in view; a refused stop has already run the command.
        let mut w = watcher(vec![Empty], 3, false);
        w.mark_fired();
        assert!(run_seconds(&mut w, 20).is_empty());
    }

    #[test]
    fn stop_request_without_face_checkout_is_unchecked() {
        let mut w = watcher(vec![Empty], 30, false);
        let (connection, client) = scripted_stop("STOP\n", vec![]);
        let rules = StopRules {
            face_checkout: false,
            ..fast_rules(3)
        };
        assert_eq!(
            w.handle_stop_request(connection, &rules, None),
            Some(StopOutcome::Unchecked)
        );
        assert_eq!(client.join().unwrap(), vec![done("unchecked")]);
    }

    #[test]
    fn anything_but_stop_is_ignored() {
        let mut w = watcher(vec![Empty], 30, false);
        let (connection, client) = scripted_stop("HELLO\n", vec![]);
        assert_eq!(
            w.handle_stop_request(connection, &fast_rules(3), None),
            None
        );
        assert!(client.join().unwrap().is_empty());
    }

    #[test]
    fn stop_signals_cover_the_common_ones() {
        let signals = stop_signals();
        for expected in [SIGTERM, SIGINT, SIGQUIT, SIGUSR1, SIGUSR2, SIGTSTP] {
            assert!(signals.contains(&expected), "missing signal {expected}");
        }
        // SIGHUP is handled separately (ignored), and must not stop it.
        assert!(!signals.contains(&SIGHUP));
    }

    #[test]
    fn strangers_wait_for_the_grace_period_when_aggressive_mode_is_off() {
        let mut w = watcher(vec![Owner, Stranger], 3, false);
        assert_eq!(run_seconds(&mut w, 20), vec![4]);
    }
}
