//! The alert: tells the owner that the command ran.
//!
//! With a lock command, all the owner sees afterwards is a locked screen,
//! which looks the same as locking it themselves. So whenever the command
//! runs, NoDonuts also:
//!
//! - appends a short entry to `NODONUTS ALERT.txt` in `alert_location` (the
//!   desktop by default), which is hard to miss after unlocking;
//! - shows a desktop notification (`notify-send` on Linux, `osascript` on
//!   macOS), which GNOME keeps in its list until dismissed;
//! - remembers where the file went, so `nodonuts status` can point at it.
//!
//! Deleting the file means "seen": `status` stops mentioning it.

use crate::files::{self, Paths};
use anyhow::{Context, Result, bail};
use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;

/// The alert file's name. Capitals to catch the eye; `.txt` so a
/// double-click opens it in a text editor.
pub const FILE_NAME: &str = "NODONUTS ALERT.txt";
/// The `alert_location` value meaning "the desktop folder".
pub const DESKTOP: &str = "desktop";
/// The `alert_location` value meaning "no alert at all".
pub const OFF: &str = "off";

/// Why the command ran.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// The owner was away for longer than the grace period.
    GracePeriod {
        /// `grace_seconds`.
        seconds: u64,
    },
    /// Aggressive mode: someone else was in view for enough checks.
    Aggressive {
        /// How many checks in a row.
        checks: u64,
    },
    /// `nodonuts stop` failed the face checkout and the password.
    StopRefused,
    /// A stop signal (kill, logout, shutdown) while the owner wasn't seen.
    StopSignal,
}

impl Reason {
    /// A full sentence, for the alert file and the notification.
    pub fn describe(self) -> String {
        match self {
            Reason::GracePeriod { seconds } => {
                format!("you were away for longer than the grace period ({seconds} s)")
            }
            Reason::Aggressive { checks } => format!(
                "someone else was at the laptop for {checks} checks in a row (aggressive mode)"
            ),
            Reason::StopRefused => "someone ran `nodonuts stop`, but your face wasn't recognized \
                                    and the fallback password wasn't given"
                .to_string(),
            Reason::StopSignal => "NoDonuts was asked to stop (kill, logout or shutdown) while \
                                   you weren't in front of the camera"
                .to_string(),
        }
    }

    /// A few words, for `nodonuts status`.
    pub fn short(self) -> &'static str {
        match self {
            Reason::GracePeriod { .. } => "grace period over",
            Reason::Aggressive { .. } => "aggressive mode",
            Reason::StopRefused => "stop refused",
            Reason::StopSignal => "stop signal while away",
        }
    }
}

/// What the watcher does after the command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Afterwards {
    /// It carries on watching (`command_once = false`).
    KeepsWatching,
    /// It stops (a stop signal, `command_once = false`).
    Stops,
    /// It stops and has turned autostart off (`command_once`).
    StopsOnce,
}

impl Afterwards {
    fn describe(self) -> &'static str {
        match self {
            Afterwards::KeepsWatching => "NoDonuts kept watching.",
            Afterwards::Stops => "NoDonuts stopped. Start it again with `nodonuts start`.",
            Afterwards::StopsOnce => {
                "NoDonuts stopped and turned autostart off (command_once). Start it again \
                 with `nodonuts start`, and `nodonuts config set autostart true` if you use it."
            }
        }
    }
}

/// Checks an `alert_location` value without touching the disk: `desktop`,
/// `off`, a full path, or a path starting with `~/`. Relative paths are
/// refused: the watcher doesn't run from your terminal's folder (and not
/// at all from one with autostart), so they'd point somewhere unexpected.
pub fn check_setting(value: &str) -> Result<()> {
    if value == DESKTOP || value == OFF || value.starts_with('/') || value.starts_with("~/") {
        return Ok(());
    }
    bail!(
        "alert_location must be `{DESKTOP}`, `{OFF}`, or a full path to a folder \
         (starting with `/` or `~/`)"
    )
}

/// For `config set`: the folder must already exist and be yours.
pub fn check_folder(value: &str) -> Result<()> {
    check_setting(value)?;
    let Some(folder) = folder(value) else {
        return Ok(()); // `off`
    };
    if value == DESKTOP {
        return Ok(()); // found by the system; falls back if missing
    }
    let meta = std::fs::metadata(&folder)
        .with_context(|| format!("alert_location: {} doesn't exist", folder.display()))?;
    if !meta.is_dir() {
        bail!("alert_location: {} is not a folder", folder.display());
    }
    if meta.uid() != nix::unistd::getuid().as_raw() {
        bail!("alert_location: {} is not your folder", folder.display());
    }
    Ok(())
}

/// The folder an `alert_location` value points at. `None` for `off`.
pub fn folder(setting: &str) -> Option<PathBuf> {
    let home = || directories::BaseDirs::new().map(|dirs| dirs.home_dir().to_path_buf());
    match setting {
        OFF => None,
        // The system's desktop folder, which isn't always `~/Desktop`
        // (e.g. `~/Bureau` on a French Ubuntu).
        DESKTOP => directories::UserDirs::new()
            .and_then(|dirs| dirs.desktop_dir().map(Path::to_path_buf))
            .or_else(|| home().map(|home| home.join("Desktop"))),
        _ => match setting.strip_prefix("~/") {
            Some(rest) => home().map(|home| home.join(rest)),
            None => Some(PathBuf::from(setting)),
        },
    }
}

/// One alert, as written to the file.
pub fn entry_text(
    time: &str,
    reason: Reason,
    command: &str,
    afterwards: Afterwards,
    log_file: &Path,
) -> String {
    format!(
        "=== NoDonuts ran your command ===\n\
         When:       {time}\n\
         Why:        {}\n\
         Command:    {command}\n\
         Afterwards: {}\n\
         Full log:   {}\n\
         (Delete this file once you've read it.)\n\n",
        reason.describe(),
        afterwards.describe(),
        log_file.display()
    )
}

/// Raises the alert: the file, the record for `status`, the notification.
/// Never fails: problems are logged, and the command must run regardless.
pub fn raise(paths: &Paths, setting: &str, reason: Reason, command: &str, afterwards: Afterwards) {
    if setting == OFF {
        return;
    }
    let time = local_time();
    let entry = entry_text(&time, reason, command, afterwards, &paths.log_file);
    match write_file(paths, setting, &entry) {
        Ok(file) => {
            log::info!("alert written to {}", file.display());
            remember(paths, &file, &time, reason);
            notify(&format!(
                "Your command ran at {time}: {}. See {}",
                reason.describe(),
                file.display()
            ));
        }
        Err(error) => log::warn!("could not write the alert file: {error:#}"),
    }
}

/// Appends `entry` to the alert file in the chosen folder, or in the
/// NoDonuts data folder if that one is missing. Returns the file written.
pub fn write_file(paths: &Paths, setting: &str, entry: &str) -> Result<PathBuf> {
    let chosen = folder(setting).context("alerts are off")?;
    let folder = if chosen.is_dir() {
        chosen
    } else {
        // E.g. an unplugged drive: an alert in the data folder beats none.
        let fallback = data_folder(paths);
        log::warn!(
            "alert_location {} is missing; writing the alert to {} instead",
            chosen.display(),
            fallback.display()
        );
        files::create_parent_dir(&fallback.join(FILE_NAME))?;
        fallback
    };
    let file = folder.join(FILE_NAME);
    let mut handle = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        // Never follow a symlink planted under that name.
        .custom_flags(nix::libc::O_NOFOLLOW)
        .open(&file)
        .with_context(|| format!("could not open {}", file.display()))?;
    handle
        .write_all(entry.as_bytes())
        .with_context(|| format!("could not write {}", file.display()))?;
    Ok(file)
}

/// The last alert, for `nodonuts status`: when, why, and where the file
/// is. `None` if there was none, or its file has been deleted (read).
pub fn last(paths: &Paths) -> Option<(String, String, PathBuf)> {
    let text = std::fs::read_to_string(&paths.last_alert_file).ok()?;
    let mut lines = text.lines();
    let file = PathBuf::from(lines.next()?);
    let time = lines.next()?.to_string();
    let reason = lines.next()?.to_string();
    file.is_file().then_some((time, reason, file))
}

/// Notes the alert for `status`: file path, time, reason, one per line.
fn remember(paths: &Paths, file: &Path, time: &str, reason: Reason) {
    let record = format!("{}\n{time}\n{}\n", file.display(), reason.short());
    if let Err(error) = files::write_private(&paths.last_alert_file, record.as_bytes()) {
        log::warn!("could not record the alert for `nodonuts status`: {error:#}");
    }
}

/// Where NoDonuts keeps its data (the log's folder).
fn data_folder(paths: &Paths) -> PathBuf {
    paths
        .log_file
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// The local date and time, e.g. `2026-11-03 14:32:07`. Asks `date`, which
/// knows the time zone; avoids pulling in a date library for one line.
fn local_time() -> String {
    let output = Command::new("date").arg("+%Y-%m-%d %H:%M:%S").output();
    match output {
        Ok(output) if output.status.success() => {
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        }
        _ => "(unknown time)".to_string(),
    }
}

/// Shows a desktop notification, on its own thread so it never holds up
/// the command. The message goes in as a separate argument, never pasted
/// into a script, so nothing in it can be run.
fn notify(message: &str) {
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = Command::new("osascript");
        command.args([
            "-e",
            "on run argv",
            "-e",
            "display notification (item 1 of argv) with title \"NODONUTS ALERT\"",
            "-e",
            "end run",
            message,
        ]);
        command
    };
    #[cfg(not(target_os = "macos"))]
    let mut command = {
        let mut command = Command::new("notify-send");
        // `critical`: GNOME keeps it on screen until dismissed.
        command.args([
            "--urgency=critical",
            "--app-name=NoDonuts",
            "NODONUTS ALERT",
            message,
        ]);
        command
    };
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // Started right here, so it's on its way even if the watcher exits a
    // moment later (`command_once` with a quick command). Only the waiting
    // happens on another thread.
    match command.spawn() {
        Ok(mut child) => {
            thread::spawn(move || match child.wait() {
                Ok(status) if status.success() => {}
                Ok(status) => log::warn!("the alert notification failed ({status})"),
                Err(error) => log::warn!("the alert notification failed: {error}"),
            });
        }
        Err(error) => log::warn!("could not show the alert notification: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_paths(name: &str) -> (PathBuf, Paths) {
        let dir =
            std::env::temp_dir().join(format!("nodonuts-alert-{name}-{}", std::process::id()));
        (dir.clone(), Paths::in_dir(&dir))
    }

    #[test]
    fn settings_are_checked() {
        for good in ["desktop", "off", "/tmp/alerts", "~/alerts"] {
            check_setting(good).unwrap();
        }
        for bad in ["", "alerts", "./alerts", "~alerts", "Desktop"] {
            assert!(check_setting(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn folders_resolve() {
        assert_eq!(folder("off"), None);
        assert_eq!(folder("/tmp/x"), Some(PathBuf::from("/tmp/x")));
        let home = directories::BaseDirs::new()
            .unwrap()
            .home_dir()
            .to_path_buf();
        assert_eq!(folder("~/alerts"), Some(home.join("alerts")));
        assert!(folder("desktop").is_some());
    }

    #[test]
    fn config_set_needs_an_existing_folder() {
        let (dir, _) = temp_paths("check");
        std::fs::create_dir_all(&dir).unwrap();
        check_folder(dir.to_str().unwrap()).unwrap();
        assert!(check_folder(dir.join("missing").to_str().unwrap()).is_err());
        let file = dir.join("a-file");
        std::fs::write(&file, "x").unwrap();
        assert!(check_folder(file.to_str().unwrap()).is_err());
        check_folder("off").unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_entry_says_when_why_what_and_where() {
        let text = entry_text(
            "2026-11-03 14:32:07",
            Reason::Aggressive { checks: 2 },
            "loginctl lock-session",
            Afterwards::StopsOnce,
            Path::new("/home/me/nodonuts.log"),
        );
        assert!(text.contains("2026-11-03 14:32:07"));
        assert!(text.contains("2 checks in a row (aggressive mode)"));
        assert!(text.contains("loginctl lock-session"));
        assert!(text.contains("turned autostart off"));
        assert!(text.contains("/home/me/nodonuts.log"));
    }

    #[test]
    fn alerts_are_appended_and_private() {
        let (dir, paths) = temp_paths("append");
        let alerts = dir.join("alerts");
        std::fs::create_dir_all(&alerts).unwrap();
        let setting = alerts.to_str().unwrap();
        let file = write_file(&paths, setting, "first\n").unwrap();
        write_file(&paths, setting, "second\n").unwrap();
        assert_eq!(file, alerts.join(FILE_NAME));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "first\nsecond\n");
        let mode = std::fs::metadata(&file).unwrap().mode() & 0o777;
        assert_eq!(mode, 0o600);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_missing_folder_falls_back_to_the_data_folder() {
        let (dir, paths) = temp_paths("fallback");
        let missing = dir.join("unplugged-drive");
        let file = write_file(&paths, missing.to_str().unwrap(), "entry\n").unwrap();
        assert_eq!(file, dir.join(FILE_NAME));
        assert!(!missing.exists(), "never creates the chosen folder");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_symlink_in_place_of_the_file_is_refused() {
        let (dir, paths) = temp_paths("symlink");
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("elsewhere");
        std::fs::write(&target, "keep me").unwrap();
        std::os::unix::fs::symlink(&target, dir.join(FILE_NAME)).unwrap();
        assert!(write_file(&paths, dir.to_str().unwrap(), "entry\n").is_err());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "keep me");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn status_sees_the_last_alert_until_the_file_is_deleted() {
        let (dir, paths) = temp_paths("last");
        std::fs::create_dir_all(&dir).unwrap();
        assert!(last(&paths).is_none());
        let file = write_file(&paths, dir.to_str().unwrap(), "entry\n").unwrap();
        remember(&paths, &file, "2026-11-03 14:32:07", Reason::StopRefused);
        let (time, reason, shown) = last(&paths).unwrap();
        assert_eq!(time, "2026-11-03 14:32:07");
        assert_eq!(reason, "stop refused");
        assert_eq!(shown, file);
        std::fs::remove_file(&file).unwrap();
        assert!(last(&paths).is_none(), "deleted = read");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
