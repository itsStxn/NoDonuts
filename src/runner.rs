//! Runs the owner's bash command.

use anyhow::{Context, Result};
use std::process::Command;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

/// Runs `command` through bash, off the watch loop, and logs the result.
///
/// We spawn a thread so a slow command cannot stall the watcher. The thread
/// waits for the command and logs its exit code and output.
pub fn run(command: &str) {
    let command = command.to_string();
    // `move` hands ownership of `command` to the thread, so it stays alive
    // as long as the thread needs it.
    thread::spawn(move || {
        if let Err(error) = run_and_log(&command) {
            log::error!("failed to run command: {error:#}");
        }
    });
}

/// Runs `command` like [`run`], but waits up to `timeout` for it to finish.
///
/// Used just before the watcher exits: without waiting, the exit could cut
/// the command off. The wait is capped so a slow command cannot hold up a
/// logout or shutdown. If it is still running then, we exit anyway; the
/// command keeps running, but its output is no longer logged.
pub fn run_and_wait(command: &str, timeout: Duration) {
    let command = command.to_string();
    // A channel: the thread sends one message when the command is done.
    let (done, finished) = mpsc::channel();
    thread::spawn(move || {
        if let Err(error) = run_and_log(&command) {
            log::error!("failed to run command: {error:#}");
        }
        // Ignore send errors: they only mean we stopped waiting.
        let _ = done.send(());
    });
    if finished.recv_timeout(timeout).is_err() {
        log::warn!(
            "command still running after {} s; not waiting any longer",
            timeout.as_secs()
        );
    }
}

fn run_and_log(command: &str) -> Result<()> {
    log::info!("running command: {command}");
    // `bash -c "<command>"` runs the whole string as the owner wrote it.
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
