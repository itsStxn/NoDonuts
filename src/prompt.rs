//! Asking for a password in the terminal without showing what is typed.

use anyhow::{Context, Result, bail};
use nix::errno::Errno;
use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
use nix::sys::termios::{self, FlushArg, LocalFlags, SetArg};
use signal_hook::consts::{SIGHUP, SIGINT, SIGQUIT, SIGTERM};
use signal_hook::iterator::Signals;
use std::io::{self, BufRead, IsTerminal, Write};
use std::os::fd::AsFd;
use std::thread;
use std::time::{Duration, Instant};

/// Prints `prompt` and reads one line with the typed characters hidden.
///
/// Hiding works by turning off the terminal's "echo". If the user presses
/// Ctrl+C meanwhile, echo is turned back on before exiting; otherwise the
/// terminal would stay silent afterwards.
pub fn read_hidden(prompt: &str) -> Result<String> {
    match read_hidden_within(prompt, None)? {
        Some(line) => Ok(line),
        // Can't happen without a time limit, but no need to panic over it.
        None => bail!("no password typed"),
    }
}

/// Like [`read_hidden`], but gives up after `limit` (if given): returns
/// `Ok(None)` when nothing was entered in time. Anything half-typed is
/// thrown away, so it doesn't end up on the shell's command line.
pub fn read_hidden_within(prompt: &str, limit: Option<Duration>) -> Result<Option<String>> {
    let stdin = io::stdin();
    if !stdin.is_terminal() {
        bail!("can't ask for a password: not running in a terminal");
    }
    print!("{prompt}");
    io::stdout()
        .flush()
        .context("could not write to the terminal")?;

    let original = termios::tcgetattr(&stdin).context("could not read the terminal settings")?;
    let mut hidden = original.clone();
    hidden.local_flags.remove(LocalFlags::ECHO);
    termios::tcsetattr(&stdin, SetArg::TCSANOW, &hidden)
        .context("could not hide the typed password")?;

    // A helper thread waits for Ctrl+C (and similar) while we read. It runs
    // outside the signal handler itself, so restoring the terminal is safe.
    let mut signals =
        Signals::new([SIGINT, SIGQUIT, SIGTERM, SIGHUP]).context("could not watch for Ctrl+C")?;
    let signal_handle = signals.handle();
    let saved = original.clone();
    let watcher = thread::spawn(move || {
        // `forever()` ends once `signal_handle.close()` is called below.
        if signals.forever().next().is_some() {
            let _ = termios::tcsetattr(io::stdin(), SetArg::TCSANOW, &saved);
            println!();
            std::process::exit(130); // the usual exit code for Ctrl+C
        }
    });

    // The terminal hands over input a whole line at a time, so "ready to
    // read" means Enter was pressed, and the read below won't block.
    let in_time = match limit {
        Some(limit) => wait_for_line(&stdin, limit),
        None => true,
    };
    let mut line = String::new();
    let read = if in_time {
        stdin.lock().read_line(&mut line).map(|_| ())
    } else {
        // Too late: drop whatever was half-typed (it would otherwise be
        // read by the shell once we exit).
        let _ = termios::tcflush(&stdin, FlushArg::TCIFLUSH);
        Ok(())
    };

    // Put everything back, whatever happened.
    let _ = termios::tcsetattr(&stdin, SetArg::TCSANOW, &original);
    signal_handle.close();
    let _ = watcher.join();
    println!(); // the Enter key wasn't echoed either

    if !in_time {
        return Ok(None);
    }
    read.context("could not read the password")?;
    // Keep spaces: the password can be anything. Only the line ending goes.
    Ok(Some(line.trim_end_matches(['\n', '\r']).to_string()))
}

/// Waits up to `limit` for a full line on `stdin`. True if one arrived.
fn wait_for_line(stdin: &io::Stdin, limit: Duration) -> bool {
    let deadline = Instant::now() + limit;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        // `poll` takes whole milliseconds, at most about 24 days.
        let timeout = PollTimeout::try_from(remaining).unwrap_or(PollTimeout::MAX);
        let mut fds = [PollFd::new(stdin.as_fd(), PollFlags::POLLIN)];
        match poll(&mut fds, timeout) {
            Ok(0) => return false, // timed out
            Ok(_) => return true,
            // Interrupted by a signal (e.g. the terminal being resized):
            // keep waiting for what's left.
            Err(Errno::EINTR) if remaining.is_zero() => return false,
            Err(Errno::EINTR) => continue,
            // Any other problem: let the read report it.
            Err(_) => return true,
        }
    }
}

/// Asks for a new password twice, to catch typos.
pub fn ask_new_password() -> Result<String> {
    new_password_with(read_hidden)
}

/// The logic of [`ask_new_password`], with the reading passed in so tests
/// can feed answers without a terminal.
///
/// `impl FnMut(&str) -> Result<String>` means "any function (or closure)
/// that takes the prompt text and returns what was typed".
pub fn new_password_with(mut read: impl FnMut(&str) -> Result<String>) -> Result<String> {
    for _ in 0..3 {
        let first = read("New fallback password: ")?;
        if first.is_empty() {
            println!("The password can't be empty.");
            continue;
        }
        let second = read("Type it again: ")?;
        if first == second {
            return Ok(first);
        }
        println!("The passwords didn't match.");
    }
    bail!("no password set (3 tries)")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Plays back answers in order.
    fn answers(list: &[&str]) -> impl FnMut(&str) -> Result<String> {
        let mut remaining: Vec<String> = list.iter().rev().map(|s| s.to_string()).collect();
        move |_prompt: &str| match remaining.pop() {
            Some(answer) => Ok(answer),
            None => bail!("no more answers"),
        }
    }

    #[test]
    fn matching_passwords_are_accepted() {
        let password = new_password_with(answers(&["a b c", "a b c"])).unwrap();
        assert_eq!(password, "a b c");
    }

    #[test]
    fn a_mismatch_or_empty_answer_asks_again() {
        let password = new_password_with(answers(&["one", "two", "", "ok", "ok"])).unwrap();
        assert_eq!(password, "ok");
    }

    #[test]
    fn three_failed_tries_give_up() {
        assert!(new_password_with(answers(&["a", "b", "c", "d", "e", "f"])).is_err());
    }
}
