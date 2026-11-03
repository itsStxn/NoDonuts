//! `start`, `stop` and `status`: run the watcher in the background.
//!
//! `start` launches this same program again as `nodonuts watch`, detached
//! from the terminal. The watcher takes a lock (so only one runs) and
//! writes its process ID (PID) to a file.
//! `stop` asks the watcher to stop over its control socket (see `control`),
//! falling back to `SIGTERM` if that socket isn't there.

use crate::autostart;
use crate::config;
use crate::control::{Reply, STOP_REQUEST};
use crate::enroll;
use crate::files::{self, Paths};
use crate::password;
use crate::prompt;
use crate::watcher;
use anyhow::{Context, Result, bail};
use nix::sys::signal::{self, Signal};
use nix::unistd::Pid;
use std::io::{BufRead, BufReader, IsTerminal, Write};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

/// How long `start` watches the new process for an early crash.
const STARTUP_CHECK: Duration = Duration::from_secs(3);
/// How long `stop` waits for a clean exit before forcing it, once the
/// checkout is over. The watcher may still run the command (up to 5 s).
const STOP_TIMEOUT: Duration = Duration::from_secs(20);
/// How long `stop` waits for each message from the watcher.
const REPLY_TIMEOUT: Duration = Duration::from_secs(60);

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
    // Check the login entry, but never change it here: `start` is also run
    // from test copies (e.g. `build/bin/`), and rewriting the entry would
    // quietly point autostart at them. Only `config set autostart` and
    // `refresh` change it.
    if let Some(problem) = autostart::problem(paths, config.autostart) {
        eprintln!(
            "warning: {problem}. Fix: `nodonuts stop`, then `nodonuts refresh`, \
             then `nodonuts start`."
        );
    }
    // A damaged or unsafe password file stops us here, like the face file.
    let has_password = password::load(&paths.password_file)?.is_some();
    if config.face_checkout && !has_password {
        if std::io::stdin().is_terminal() {
            println!("face_checkout is on, but no fallback password is set.");
            println!("You'll need it to stop NoDonuts if your face isn't recognized.");
            let typed = prompt::ask_new_password()?;
            password::save(&paths.password_file, &typed)?;
            println!("Fallback password saved. Change it with `nodonuts password`.");
        } else {
            // E.g. autostart at login: no terminal to ask in. Start anyway;
            // being watched without the fallback beats not being watched.
            eprintln!(
                "warning: no fallback password set, and no terminal to ask for one. \
                 Set one with `nodonuts password`."
            );
        }
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
    // The watcher writes its own PID file once it holds the single-watcher
    // lock, so `start` never writes or removes it.
    let pid = child.id();

    // If the watcher dies right away (e.g. no camera access), say so.
    let deadline = Instant::now() + STARTUP_CHECK;
    while Instant::now() < deadline {
        if let Some(status) = child.try_wait()? {
            if status.code() == Some(watcher::ALREADY_RUNNING_EXIT_CODE) {
                // Another `start` (or autostart) won a race with this one.
                match running_pid(paths)? {
                    Some(other) => bail!("NoDonuts is already watching (PID {other})"),
                    None => bail!("NoDonuts is already watching"),
                }
            }
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
    // Cleared first, so an old result is never reported for this stop.
    remove_stop_result(paths);
    let target = Pid::from_raw(pid);

    // Talk to the watcher over its socket, so it can ask for the fallback
    // password if needed. If the socket isn't there (an older watcher, or
    // setting it up failed), fall back to a plain signal.
    let outcome = match UnixStream::connect(&paths.control_socket) {
        Ok(connection) => talk_to_watcher(connection),
        Err(_) => {
            signal::kill(target, Signal::SIGTERM).context("could not signal the watcher")?;
            None
        }
    };
    // The checkout failed: the watcher ran the command and keeps running.
    // Don't wait for it to exit (and certainly don't force it).
    if outcome.as_deref() == Some("denied") {
        println!("Owner not recognized: your command ran, and NoDonuts is still watching.");
        println!("Run `nodonuts stop` again to retry.");
        bail!("not stopped");
    }

    let deadline = Instant::now() + STOP_TIMEOUT;
    while is_alive(pid) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(100));
    }
    if is_alive(pid) {
        if !may_force_stop(outcome.as_deref()) {
            // No confirmed answer (the conversation broke, the watcher went
            // quiet, or only a signal could be sent). Killing it now would
            // end it without the face checkout and without the command, so
            // leave it running.
            println!(
                "NoDonuts didn't confirm the stop and is still running (PID {pid}). \
                 It wasn't force-stopped, since that would skip the face checkout."
            );
            println!("See the log: {}", paths.log_file.display());
            bail!("not stopped");
        }
        // The watcher agreed to stop but is stuck on its way out (e.g. on
        // the camera). Finishing the job skips nothing.
        signal::kill(target, Signal::SIGKILL).context("could not stop the watcher")?;
        println!("The watcher did not exit in time and was killed.");
    }
    remove_pid_file(paths);

    // The socket tells us how it went; with a signal, the watcher leaves a
    // result file instead.
    let outcome = outcome
        .or_else(|| std::fs::read_to_string(&paths.stop_result_file).ok())
        .unwrap_or_default();
    remove_stop_result(paths);
    match outcome.trim() {
        "owner" => println!("Owner recognized. NoDonuts stopped (PID {pid})."),
        "password" => println!("Fallback password accepted. NoDonuts stopped (PID {pid})."),
        "once" => {
            println!(
                "Owner not recognized: your command ran, then NoDonuts stopped (PID {pid}) \
                 and turned autostart off (command_once)."
            );
        }
        "command" => {
            println!("Owner not recognized: your command ran, then NoDonuts stopped (PID {pid}).");
        }
        _ => println!("NoDonuts stopped (PID {pid})."),
    }
    Ok(())
}

/// Sends `STOP` and follows the watcher's checkout (see the `control`
/// module): shows progress and asks for the fallback password when the
/// watcher wants it. Returns how it ended, or `None` if the watcher went
/// quiet.
fn talk_to_watcher(connection: UnixStream) -> Option<String> {
    let mut writer = connection.try_clone().ok()?;
    // Generous: the watcher may be finishing a command first.
    let _ = connection.set_read_timeout(Some(REPLY_TIMEOUT));
    writer
        .write_all(format!("{STOP_REQUEST}\n").as_bytes())
        .ok()?;

    for line in BufReader::new(connection).lines() {
        // A read error (timeout, closed) ends the conversation.
        let line = line.ok()?;
        match Reply::parse(&line) {
            // One line per frame would scroll fast; say it once.
            Some(Reply::Looking { attempt: 1, .. }) => println!("Look at the camera..."),
            Some(Reply::Looking { .. }) => {}
            Some(Reply::NotSeen { attempt, of }) => println!("Not recognized ({attempt}/{of})."),
            Some(Reply::Password { attempt, of }) => {
                let label = if of > 1 {
                    format!("Fallback password ({attempt}/{of}): ")
                } else {
                    "Fallback password: ".to_string()
                };
                // If we can't ask (no terminal), send an empty answer: it
                // counts as wrong, which ends in the command running.
                let typed = prompt::read_hidden(&label).unwrap_or_else(|error| {
                    eprintln!("{error:#}");
                    String::new()
                });
                writer.write_all(format!("{typed}\n").as_bytes()).ok()?;
            }
            Some(Reply::Wrong) => println!("Wrong password."),
            Some(Reply::Done(outcome)) => return Some(outcome),
            None => {} // unknown line: ignore
        }
    }
    None
}

/// May `stop` force-kill a watcher that hasn't exited in time? Only if the
/// watcher itself confirmed a quiet stop over the socket: `owner` or
/// `password` (the checkout passed), or `unchecked` (`face_checkout` off).
/// With no confirmed answer, a kill would skip the checkout.
fn may_force_stop(outcome: Option<&str>) -> bool {
    matches!(outcome, Some("owner" | "password" | "unchecked"))
}

fn remove_stop_result(paths: &Paths) {
    // Ignore errors: the file may not exist.
    let _ = std::fs::remove_file(&paths.stop_result_file);
}

/// `nodonuts status`.
pub fn status(paths: &Paths) -> Result<()> {
    match running_pid(paths)? {
        Some(pid) => println!("NoDonuts is watching (PID {pid})."),
        None => {
            println!("NoDonuts is not running.");
            let last = std::fs::read_to_string(&paths.stop_result_file).unwrap_or_default();
            if last.trim() == "once" {
                println!(
                    "  It stopped after running your command (command_once), and turned \
                     autostart off."
                );
                println!(
                    "  Start again with `nodonuts start`, and `nodonuts config set autostart true` \
                     if you use autostart."
                );
            }
        }
    }
    print_autostart_status(paths);
    match password::load(&paths.password_file) {
        Ok(Some(_)) => println!("Fallback password: set"),
        Ok(None) => println!("Fallback password: not set (set one with `nodonuts password`)"),
        Err(error) => println!("Fallback password: unusable ({error:#})"),
    }
    Ok(())
}

/// The "Starts at login" line of `status`, with a warning when the login
/// entry points at a program that is no longer there.
fn print_autostart_status(paths: &Paths) {
    let fix = "Fix: run `nodonuts refresh` from the copy you want started at login \
               (stop the watcher first if it's running).";
    match autostart::stored_program(paths) {
        Ok(None) => println!("Starts at login: no"),
        Ok(Some(program)) if program.is_file() => {
            println!("Starts at login: yes ({})", program.display());
        }
        Ok(Some(program)) => {
            println!("Starts at login: yes, but it will FAIL");
            println!(
                "  warning: the login entry runs {}, which no longer exists",
                program.display()
            );
            println!("  (the program was moved or deleted). {fix}");
        }
        Err(error) => {
            println!("Starts at login: unknown");
            println!("  warning: {error:#}. {fix}");
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn force_stop_only_after_a_confirmed_quiet_stop() {
        // The watcher confirmed it may stop quietly.
        assert!(may_force_stop(Some("owner")));
        assert!(may_force_stop(Some("password")));
        assert!(may_force_stop(Some("unchecked")));
        // No confirmed answer, or the checkout didn't pass: never.
        assert!(!may_force_stop(None));
        assert!(!may_force_stop(Some("command")));
        assert!(!may_force_stop(Some("denied")));
        assert!(!may_force_stop(Some("")));
        assert!(!may_force_stop(Some("garbage")));
    }
}
