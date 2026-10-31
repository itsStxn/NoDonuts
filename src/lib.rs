//! NoDonuts watches the laptop camera for its owner's face, and runs an
//! owner-defined bash command when the owner has been away for a while.
//!
//! Lint levels for this crate live in the `[lints]` table of `Cargo.toml`.
//!
//! How the pieces fit together:
//!
//! ```text
//! start -> spawn self as hidden `watch` -> write PID file
//! watch loop (every check_every_ms):
//!   camera frame -> YuNet finds faces -> SFace embeds each face
//!   -> cosine similarity vs saved owner embedding -> owner_seen: bool
//!   -> presence state machine -> maybe run the command
//! stop -> read PID file -> SIGTERM -> watcher exits cleanly
//! ```

// Each `pub mod` line pulls in the file `src/<name>.rs`.
pub mod autostart;
pub mod camera;
pub mod cli;
pub mod config;
pub mod daemon;
pub mod enroll;
pub mod face;
pub mod files;
pub mod presence;
pub mod runner;
pub mod watcher;
