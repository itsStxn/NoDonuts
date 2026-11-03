//! Runs the owner's bash command.

use anyhow::{Context, Result};
use std::cell::Cell;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, TryRecvError};
use std::thread;
use std::time::Duration;

/// How a command ended, as far as the watcher is concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandEnd {
    /// The command finished.
    Finished,
    /// We stopped waiting (a stop was requested, or the wait timed out).
    /// The command keeps running on its own; its output is still logged
    /// if it finishes before the watcher exits.
    StillRunning,
}

/// A command started with [`start`].
#[derive(Debug)]
pub struct RunningCommand {
    /// Receives one message when the command is done.
    finished: Receiver<()>,
    /// Set once that message was seen. `Cell` lets `&self` methods update
    /// it (the struct is only used from one thread).
    done: Cell<bool>,
}

impl RunningCommand {
    /// Waits for the command to finish, unless `interrupted()` turns true
    /// first (checked every 100 ms), so `nodonuts stop` still works while a
    /// long command runs.
    ///
    /// `&dyn Fn() -> bool` means "some function that answers yes or no".
    pub fn wait_unless(&self, interrupted: &dyn Fn() -> bool) -> CommandEnd {
        loop {
            if self.done.get() {
                return CommandEnd::Finished;
            }
            match self.finished.recv_timeout(Duration::from_millis(100)) {
                // `Disconnected` means the thread is gone (it already sent
                // its message, or it panicked): nothing left to wait for.
                Ok(()) | Err(RecvTimeoutError::Disconnected) => {
                    self.done.set(true);
                    return CommandEnd::Finished;
                }
                Err(RecvTimeoutError::Timeout) => {
                    if interrupted() {
                        log::info!("interrupted while the command is running; not waiting for it");
                        return CommandEnd::StillRunning;
                    }
                }
            }
        }
    }

    /// True once the command has finished. Never waits.
    pub fn is_done(&self) -> bool {
        if self.done.get() {
            return true;
        }
        let done = !matches!(self.finished.try_recv(), Err(TryRecvError::Empty));
        // Remember it: the message can only be received once.
        self.done.set(done);
        done
    }
}

/// Runs `command` and waits for it to finish, unless `stop` is set first.
/// A shorthand for [`start`] followed by [`RunningCommand::wait_unless`].
pub fn run_until_done(command: &str, stop: &AtomicBool) -> CommandEnd {
    start(command).wait_unless(&|| stop.load(Ordering::Relaxed))
}

/// Runs `command` and waits up to `timeout` for it to finish.
///
/// Used just before the watcher exits: without waiting, the exit could cut
/// the command off. The wait is capped so a slow command cannot hold up a
/// logout or shutdown.
pub fn run_and_wait(command: &str, timeout: Duration) -> CommandEnd {
    match start(command).finished.recv_timeout(timeout) {
        Ok(()) | Err(RecvTimeoutError::Disconnected) => CommandEnd::Finished,
        Err(RecvTimeoutError::Timeout) => {
            log::warn!(
                "command still running after {} s; not waiting any longer",
                timeout.as_secs()
            );
            CommandEnd::StillRunning
        }
    }
}

/// Starts `command` on its own thread, which logs its result.
pub fn start(command: &str) -> RunningCommand {
    let command = command.to_string();
    let (done, finished) = mpsc::channel();
    // `move` hands ownership of `command` and `done` to the thread.
    thread::spawn(move || {
        if let Err(error) = run_and_log(&command) {
            log::error!("failed to run command: {error:#}");
        }
        // Ignore send errors: they only mean nobody is waiting any more.
        let _ = done.send(());
    });
    RunningCommand {
        finished,
        done: Cell::new(false),
    }
}

fn run_and_log(command: &str) -> Result<()> {
    log::info!("running command: {command}");
    // `bash -c "<command>"` runs the whole string as the owner wrote it.
    // `output()` waits until the command exits *and* closes its output, so
    // a command that leaves something running in the background (`... &`)
    // counts as running until that background part closes its output too.
    let output = Command::new("bash")
        .arg("-c")
        .arg(command)
        .output()
        .context("could not start bash")?;

    let code = match output.status.code() {
        Some(code) => code.to_string(),
        None => "terminated by signal".to_string(),
    };
    log::info!("command finished (exit {code})");
    log_stream("stdout", &output.stdout);
    log_stream("stderr", &output.stderr);
    Ok(())
}

fn log_stream(name: &str, bytes: &[u8]) {
    let text = String::from_utf8_lossy(bytes);
    let text = text.trim();
    if !text.is_empty() {
        log::info!("command {name}: {text}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn waits_for_the_command_to_finish() {
        let stop = AtomicBool::new(false);
        let started = Instant::now();
        assert_eq!(run_until_done("sleep 0.3", &stop), CommandEnd::Finished);
        assert!(started.elapsed() >= Duration::from_millis(300));
    }

    #[test]
    fn a_stop_ends_the_wait_early() {
        let stop = AtomicBool::new(true);
        let started = Instant::now();
        assert_eq!(run_until_done("sleep 5", &stop), CommandEnd::StillRunning);
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn a_failing_command_still_counts_as_finished() {
        let stop = AtomicBool::new(false);
        assert_eq!(run_until_done("exit 3", &stop), CommandEnd::Finished);
        assert_eq!(
            run_until_done("no-such-command-xyz", &stop),
            CommandEnd::Finished
        );
    }

    #[test]
    fn background_with_redirect_does_not_hold_the_wait() {
        // The documented way to run something without pausing the watcher.
        let stop = AtomicBool::new(false);
        let started = Instant::now();
        assert_eq!(
            run_until_done("sleep 5 >/dev/null 2>&1 &", &stop),
            CommandEnd::Finished
        );
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn is_done_reports_without_waiting() {
        let running = start("sleep 0.3");
        assert!(!running.is_done());
        thread::sleep(Duration::from_millis(800));
        assert!(running.is_done());
        assert!(running.is_done(), "stays done");
    }

    #[test]
    fn run_and_wait_gives_up_after_the_timeout() {
        assert_eq!(
            run_and_wait("sleep 5", Duration::from_millis(200)),
            CommandEnd::StillRunning
        );
        assert_eq!(
            run_and_wait("true", Duration::from_secs(5)),
            CommandEnd::Finished
        );
    }
}
