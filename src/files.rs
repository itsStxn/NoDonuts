//! Where NoDonuts keeps its files, and how it keeps them private.
//!
//! The config holds a command that bash will run, so anyone who can edit it
//! can run code as you. Every file we write is therefore readable and
//! writable by its owner only (`0600`), and we refuse to use files that
//! others could have tampered with.

use anyhow::{Context, Result, bail};
use nix::errno::Errno;
use nix::fcntl::{Flock, FlockArg};
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// Owner read/write only.
const PRIVATE_FILE: u32 = 0o600;
/// Owner only (directories need the "x" bit to be entered).
const PRIVATE_DIR: u32 = 0o700;

/// The locations of every file NoDonuts uses.
#[derive(Debug, Clone)]
pub struct Paths {
    /// Settings, e.g. `~/.config/nodonuts/config.toml` on Linux.
    pub config_file: PathBuf,
    /// The owner's face embedding (never a photo).
    pub face_file: PathBuf,
    /// Holds the process ID of the running watcher (written by the watcher).
    pub pid_file: PathBuf,
    /// Locked by the running watcher, so only one can run at a time.
    pub lock_file: PathBuf,
    /// The watcher's log.
    pub log_file: PathBuf,
    /// When the log was last emptied (for `logs_maxdays`).
    pub log_started_file: PathBuf,
    /// The login entry that starts the watcher when `autostart` is on.
    pub autostart_file: PathBuf,
    /// Written by the watcher as it exits: how the stop went (see
    /// `watcher::StopOutcome`), so `nodonuts stop` can report it.
    pub stop_result_file: PathBuf,
    /// The fallback checkout password's hash (never the password).
    pub password_file: PathBuf,
    /// The local socket `nodonuts stop` uses to talk to the watcher.
    pub control_socket: PathBuf,
}

impl Paths {
    /// Finds the standard per-user directories for this OS.
    ///
    /// Linux: `~/.config/nodonuts` and `~/.local/share/nodonuts`.
    /// macOS: `~/Library/Application Support/nodonuts` for both.
    pub fn locate() -> Result<Paths> {
        // `ProjectDirs::from` returns an `Option`: `None` if no home
        // directory is known. `.context(...)` turns that into an error.
        let dirs = directories::ProjectDirs::from("", "", "nodonuts")
            .context("could not find your home directory")?;
        let config_dir = dirs.config_dir();
        let data_dir = dirs.data_local_dir();
        let base = directories::BaseDirs::new().context("could not find your home directory")?;
        // Where the OS looks for things to run at login (see `autostart`).
        #[cfg(target_os = "macos")]
        let autostart_file = base
            .home_dir()
            .join("Library/LaunchAgents/com.amazon.nodonuts.plist");
        #[cfg(not(target_os = "macos"))]
        let autostart_file = base.config_dir().join("autostart/nodonuts.desktop");
        Ok(Paths {
            config_file: config_dir.join("config.toml"),
            face_file: data_dir.join("owner_face.toml"),
            pid_file: data_dir.join("nodonuts.pid"),
            lock_file: data_dir.join("nodonuts.lock"),
            log_file: data_dir.join("nodonuts.log"),
            log_started_file: data_dir.join("log_started"),
            autostart_file,
            stop_result_file: data_dir.join("last_stop"),
            password_file: data_dir.join("fallback_checkout.toml"),
            control_socket: data_dir.join("control.sock"),
        })
    }
}

/// Creates the parent directory of `path` (owner-only) if it is missing.
pub fn create_parent_dir(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        DirBuilder::new()
            .recursive(true)
            .mode(PRIVATE_DIR)
            .create(parent)
            .with_context(|| format!("could not create directory {}", parent.display()))?;
    }
    Ok(())
}

/// Writes `contents` to `path`, replacing it, with `0600` permissions.
pub fn write_private(path: &Path, contents: &[u8]) -> Result<()> {
    create_parent_dir(path)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(PRIVATE_FILE) // only applies when the file is created...
        .open(path)
        .with_context(|| format!("could not open {} for writing", path.display()))?;
    // ...so also tighten an existing file's permissions.
    fs::set_permissions(path, fs::Permissions::from_mode(PRIVATE_FILE))
        .with_context(|| format!("could not set permissions on {}", path.display()))?;
    file.write_all(contents)
        .with_context(|| format!("could not write {}", path.display()))?;
    Ok(())
}

/// Tries to take an exclusive lock on `path` (created owner-only if
/// missing). `Ok(None)` means another process holds it.
///
/// The lock lasts as long as the returned value is kept, and the OS drops
/// it when the process ends, even after a crash or `kill -9`, so it can
/// never go stale. The lock file itself is never deleted: deleting it would
/// let two processes lock two different files.
pub fn try_lock(path: &Path) -> Result<Option<Flock<File>>> {
    create_parent_dir(path)?;
    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .mode(PRIVATE_FILE)
        .open(path)
        .with_context(|| format!("could not open {}", path.display()))?;
    match Flock::lock(file, FlockArg::LockExclusiveNonblock) {
        Ok(lock) => Ok(Some(lock)),
        Err((_, Errno::EWOULDBLOCK)) => Ok(None),
        Err((_, error)) => Err(error).with_context(|| format!("could not lock {}", path.display())),
    }
}

/// Fails unless `path` is owned by the current user and not writable by
/// group or others.
pub fn check_private(path: &Path) -> Result<()> {
    let meta = fs::metadata(path).with_context(|| format!("could not read {}", path.display()))?;
    let me = nix::unistd::getuid().as_raw();
    if meta.uid() != me {
        bail!(
            "{} is not owned by you, refusing to use it. Fix with: chown $USER '{}'",
            path.display(),
            path.display()
        );
    }
    // 0o022 = the "write" bits for group and others.
    if meta.mode() & 0o022 != 0 {
        bail!(
            "{} can be modified by other users, refusing to use it. Fix with: chmod 600 '{}'",
            path.display(),
            path.display()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir()
            .join(format!("nodonuts-test-{}", std::process::id()))
            .join(name)
    }

    #[test]
    fn written_files_are_private() {
        let path = temp_path("private.txt");
        write_private(&path, b"hello").unwrap();
        let mode = fs::metadata(&path).unwrap().mode() & 0o777;
        assert_eq!(mode, 0o600);
        check_private(&path).unwrap();
        fs::remove_file(&path).unwrap();
    }

    #[test]
    fn only_one_holder_of_the_lock_at_a_time() {
        let path = temp_path("nodonuts.lock");
        let first = try_lock(&path).unwrap();
        assert!(first.is_some());
        // A second try (like a second watcher) is refused while it's held...
        assert!(try_lock(&path).unwrap().is_none());
        // ...and succeeds once the first holder lets go (or exits).
        drop(first);
        assert!(try_lock(&path).unwrap().is_some());
        let mode = fs::metadata(&path).unwrap().mode() & 0o777;
        assert_eq!(mode, 0o600);
        fs::remove_file(&path).unwrap();
    }

    #[test]
    fn group_writable_files_are_refused() {
        let path = temp_path("shared.txt");
        write_private(&path, b"hello").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o620)).unwrap();
        assert!(check_private(&path).is_err());
        fs::remove_file(&path).unwrap();
    }
}
