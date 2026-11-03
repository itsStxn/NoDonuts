//! NoDonuts watches the laptop camera for its owner's face, and runs an
//! owner-defined bash command when the owner has been away for a while.
//!
//! Lint levels for this crate live in the `[lints]` table of `Cargo.toml`.
//!
//! How the pieces fit together:
//!
//! ```text
//! start (or autostart at login) -> spawn self as hidden `watch`
//!
//! watch:
//!   take nodonuts.lock (only one watcher) -> write PID file
//!   -> listen on control.sock (for `stop`) -> catch stop signals
//!   loop, every check_every_ms:
//!     camera frame -> YuNet finds faces -> SFace embeds each face
//!     -> similarity vs the owner's embedding
//!          >= threshold_owner   : owner
//!          <  threshold_stranger: stranger   (in between: unsure)
//!     -> presence state machine (grace period, aggressive mode, wake)
//!     -> fire? release the camera, run the command, wait for it
//!     every ~60 s: empty the log if older than logs_maxdays
//!
//! stop (`nodonuts stop`, over control.sock):
//!   face checkout: up to face_checks frames, one per check_every_ms
//!   -> still not seen: ask the fallback password once
//!   -> ok: watcher exits quietly
//!   -> not ok: command runs, watcher keeps watching
//!
//! stop signal (kill, pkill, logout, shutdown):
//!   one face checkout -> not seen: command runs -> watcher exits
//! ```

// Each `pub mod` line pulls in the file `src/<name>.rs`.
pub mod autostart;
pub mod camera;
pub mod cli;
pub mod config;
pub mod control;
pub mod daemon;
pub mod enroll;
pub mod face;
pub mod files;
pub mod logs;
pub mod password;
pub mod presence;
pub mod prompt;
pub mod runner;
pub mod watcher;
