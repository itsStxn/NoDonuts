//! Runs the owner's bash command when they are away.

use anyhow::{Context, Result};
use std::process::Command;
use std::thread;

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

fn run_and_log(command: &str) -> Result<()> {
    log::info!("owner is away; running command: {command}");
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
