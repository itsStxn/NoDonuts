//! `start`, `stop` and `status`: run the watcher in the background.
//!
//! `start` launches this same program again as `nodonuts watch`, detached
//! from the terminal, and writes the new process ID (PID) to a file.
//! `stop` reads that file and asks the process to exit with `SIGTERM`.

use crate::autostart;
use crate::config;
use crate::enroll;
use crate::files::{self, Paths};
use anyhow::{Context, Result, bail};
use nix::sys::signal::{self, Signal};
use nix::unistd::Pid;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

/// How long `start` watches the new process for an early crash.
const STARTUP_CHECK: Duration = Duration::from_secs(3);
/// How long `stop` waits for a clean exit before forcing it.
const STOP_TIMEOUT: Duration = Duration::from_secs(10);

/// `nodonuts start`.
pub fn start(paths: &Paths) -> Result<()> {
    if let Some(pid) = running_pid(paths)? {
        bail!("NoDonuts is already watching (PID {pid})");
    }
    // Check everything the watcher needs now, so mistakes show up here
    // instead of only in the log.
    enroll::load(&paths.face_file)?;
    let config = config::load(&paths.config_file)?;
    files::create_parent_dir(&paths.log_file)?;
    // Keep the login entry in line with the config, in case the config
    // file was edited by hand. A failure here must not stop the watcher.
    if let Err(error) = autostart::sync(paths, config.autostart) {
        eprintln!("warning: could not update the login entry: {error:#}");
    }

    let program = std::env::current_exe().context("could not find the nodonuts program")?;
    let mut child = Command::new(program)
        .arg("watch")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        // A new process group: Ctrl+C in this terminal will not reach it.
        .process_group(0)
        .spawn()
        .context("could not start the watcher")?;
    let pid = child.id();
    files::write_private(&paths.pid_file, pid.to_string().as_bytes())?;

    // If the watcher dies right away (e.g. no camera access), say so.
    let deadline = Instant::now() + STARTUP_CHECK;
    while Instant::now() < deadline {
        if let Some(status) = child.try_wait()? {
            remove_pid_file(paths);
            bail!(
                "the watcher stopped right away ({status}). See the log: {}",
                paths.log_file.display()
            );
        }
        thread::sleep(Duration::from_millis(100));
    }
    println!("NoDonuts is watching (PID {pid}).");
    println!("Log: {}", paths.log_file.display());
    Ok(())
}

/// `nodonuts stop`.
pub fn stop(paths: &Paths) -> Result<()> {
    let Some(pid) = running_pid(paths)? else {
        println!("NoDonuts is not running.");
        return Ok(());
    };
    let target = Pid::from_raw(pid);
    signal::kill(target, Signal::SIGTERM).context("could not signal the watcher")?;

    let deadline = Instant::now() + STOP_TIMEOUT;
    while is_alive(pid) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(100));
    }
    if is_alive(pid) {
        // It did not exit in time (e.g. stuck on the camera). Force it.
        signal::kill(target, Signal::SIGKILL).context("could not stop the watcher")?;
        println!("The watcher did not exit in time and was killed.");
    }
    remove_pid_file(paths);
    println!("NoDonuts stopped (PID {pid}).");
    Ok(())
}

/// `nodonuts status`.
pub fn status(paths: &Paths) -> Result<()> {
    match running_pid(paths)? {
        Some(pid) => println!("NoDonuts is watching (PID {pid})."),
        None => println!("NoDonuts is not running."),
    }
    let at_login = if autostart::is_enabled(paths) {
        "yes"
    } else {
        "no"
    };
    println!("Starts at login: {at_login}");
    Ok(())
}

/// Returns the watcher's PID if it is running. Cleans up a stale PID file
/// (the process is gone, e.g. after a crash or a reboot).
pub fn running_pid(paths: &Paths) -> Result<Option<i32>> {
    if !paths.pid_file.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&paths.pid_file)
        .with_context(|| format!("could not read {}", paths.pid_file.display()))?;
    // `.ok()` turns a parse error into `None`: a garbled file is stale.
    let pid: Option<i32> = text.trim().parse().ok();
    match pid {
        Some(pid) if is_our_watcher(pid) => Ok(Some(pid)),
        _ => {
            remove_pid_file(paths);
            Ok(None)
        }
    }
}

/// True if a process with this PID exists.
fn is_alive(pid: i32) -> bool {
    // Signal "None" sends nothing; it only checks the process exists.
    signal::kill(Pid::from_raw(pid), None).is_ok()
}

/// True if `pid` is a running `nodonuts watch`. PIDs get reused after a
/// process exits, so we check the command line before trusting a PID file;
/// otherwise `stop` could signal an unrelated program.
fn is_our_watcher(pid: i32) -> bool {
    if pid <= 0 || !is_alive(pid) {
        return false;
    }
    // `ps -o command=` prints the full command line, on Linux and macOS.
    let output = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "command="])
        .output();
    match output {
        Ok(output) => {
            let command_line = String::from_utf8_lossy(&output.stdout);
            let command_line = command_line.trim();
            command_line.contains("nodonuts") && command_line.ends_with(" watch")
        }
        Err(_) => false,
    }
}

fn remove_pid_file(paths: &Paths) {
    // Ignore errors: the file may already be gone.
    let _ = std::fs::remove_file(&paths.pid_file);
}
