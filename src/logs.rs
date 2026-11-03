//! Keeping the watcher's log from growing forever (`logs_maxdays`).
//!
//! The log lines only show the time of day, so the age of the log is kept
//! in a tiny side file: the moment it was last emptied, as seconds since
//! 1970. Once the log is older than `logs_maxdays`, it is emptied and the
//! clock starts again.

use crate::files::{self, Paths};
use anyhow::{Context, Result};
use std::fs::OpenOptions;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const SECONDS_PER_DAY: u64 = 24 * 60 * 60;

/// True if a log started at `since` is due to be emptied at `now`.
/// `max_days` of 0 means never. A `since` in the future (the clock went
/// backwards) is not due; the caller restarts the clock instead.
pub fn is_due(since: SystemTime, now: SystemTime, max_days: u64) -> bool {
    if max_days == 0 {
        return false;
    }
    let max_age = Duration::from_secs(max_days.saturating_mul(SECONDS_PER_DAY));
    match now.duration_since(since) {
        Ok(age) => age >= max_age,
        Err(_) => false,
    }
}

/// Empties the log if it's older than `max_days`. Returns true if it did.
///
/// Safe while the logger is writing: the logger appends, so after the log is
/// cut to zero length its next line simply lands at the new end.
pub fn empty_if_due(paths: &Paths, max_days: u64, now: SystemTime) -> Result<bool> {
    let since = match read_since(paths) {
        // In the future: the clock went backwards. Start counting from now.
        Some(since) if since <= now => since,
        _ => {
            // First run, or an unreadable/odd value: start counting now.
            write_since(paths, now)?;
            return Ok(false);
        }
    };
    if !is_due(since, now, max_days) {
        return Ok(false);
    }
    if paths.log_file.exists() {
        let file = OpenOptions::new()
            .write(true)
            .open(&paths.log_file)
            .with_context(|| format!("could not open {}", paths.log_file.display()))?;
        file.set_len(0)
            .with_context(|| format!("could not empty {}", paths.log_file.display()))?;
    }
    write_since(paths, now)?;
    Ok(true)
}

fn read_since(paths: &Paths) -> Option<SystemTime> {
    let text = std::fs::read_to_string(&paths.log_started_file).ok()?;
    let seconds: u64 = text.trim().parse().ok()?;
    UNIX_EPOCH.checked_add(Duration::from_secs(seconds))
}

fn write_since(paths: &Paths, now: SystemTime) -> Result<()> {
    let seconds = now
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs();
    files::write_private(&paths.log_started_file, seconds.to_string().as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const DAY: Duration = Duration::from_secs(SECONDS_PER_DAY);

    fn at(days: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(1_000_000_000) + DAY * days as u32
    }

    fn temp_paths(name: &str) -> (PathBuf, Paths) {
        let dir = std::env::temp_dir().join(format!("nodonuts-logs-{name}-{}", std::process::id()));
        let paths = Paths {
            config_file: dir.join("config.toml"),
            face_file: dir.join("face.toml"),
            pid_file: dir.join("pid"),
            log_file: dir.join("nodonuts.log"),
            log_started_file: dir.join("log_started"),
            autostart_file: dir.join("autostart.desktop"),
            stop_result_file: dir.join("last_stop"),
            password_file: dir.join("fallback_checkout.toml"),
            control_socket: dir.join("control.sock"),
        };
        (dir, paths)
    }

    #[test]
    fn due_after_max_days() {
        assert!(!is_due(at(0), at(6), 7));
        assert!(is_due(at(0), at(7), 7));
        assert!(is_due(at(0), at(30), 7));
    }

    #[test]
    fn zero_means_never() {
        assert!(!is_due(at(0), at(10_000), 0));
    }

    #[test]
    fn clock_going_backwards_is_not_due() {
        assert!(!is_due(at(10), at(3), 7));
    }

    #[test]
    fn empties_the_log_only_when_due() {
        let (dir, paths) = temp_paths("empty");
        files::write_private(&paths.log_file, b"old line\n").unwrap();

        // First check: no record yet, so it starts counting and keeps the log.
        assert!(!empty_if_due(&paths, 7, at(0)).unwrap());
        assert!(!empty_if_due(&paths, 7, at(6)).unwrap());
        assert_eq!(
            std::fs::read_to_string(&paths.log_file).unwrap(),
            "old line\n"
        );

        // Day 7: emptied, and the clock restarts.
        assert!(empty_if_due(&paths, 7, at(7)).unwrap());
        assert_eq!(std::fs::read_to_string(&paths.log_file).unwrap(), "");
        files::check_private(&paths.log_file).unwrap();
        assert!(!empty_if_due(&paths, 7, at(13)).unwrap());
        assert!(empty_if_due(&paths, 7, at(14)).unwrap());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_clock_in_the_past_restarts_the_count() {
        let (dir, paths) = temp_paths("backwards");
        files::write_private(&paths.log_file, b"line\n").unwrap();
        empty_if_due(&paths, 7, at(10)).unwrap(); // starts counting at day 10
        // The clock jumps back to day 2: restart from there, keep the log.
        assert!(!empty_if_due(&paths, 7, at(2)).unwrap());
        assert!(
            empty_if_due(&paths, 7, at(9)).unwrap(),
            "7 days after day 2"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn appending_continues_after_emptying() {
        use std::io::Write;
        let (dir, paths) = temp_paths("append");
        files::write_private(&paths.log_file, b"").unwrap();
        // Like the logger: one handle, opened once, in append mode.
        let mut logger = OpenOptions::new()
            .append(true)
            .open(&paths.log_file)
            .unwrap();
        logger.write_all(b"before\n").unwrap();
        empty_if_due(&paths, 1, at(0)).unwrap();
        assert!(empty_if_due(&paths, 1, at(1)).unwrap());
        logger.write_all(b"after\n").unwrap();
        // No gap of zeros where the old text was: just the new line.
        assert_eq!(std::fs::read_to_string(&paths.log_file).unwrap(), "after\n");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
