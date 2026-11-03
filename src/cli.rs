//! Command-line definitions.
//!
//! `clap`'s derive API turns these plain structs and enums into a parser:
//! each enum variant becomes a subcommand and each `///` comment becomes
//! its `--help` text.

use clap::{Parser, Subcommand};

/// Watches the camera and runs your command when you walk away.
#[derive(Debug, Parser)]
#[command(name = "nodonuts", version, about)]
pub struct Cli {
    /// What to do.
    #[command(subcommand)]
    pub command: Command,
}

/// The top-level subcommands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Learn the owner's face. Sit in front of the camera and look at it.
    Enroll,
    /// Start watching in the background.
    Start,
    /// Stop watching.
    Stop,
    /// Tell whether the watcher is running.
    Status,
    /// Update the login entry to point at this copy of nodonuts. Run it from
    /// the copy you want started at login, e.g. after moving the program.
    Refresh,
    /// Set or change the fallback password, used by `stop` when your face
    /// isn't recognized.
    Password {
        /// Remove the fallback password instead.
        #[arg(long)]
        remove: bool,
    },
    /// Show or change settings.
    Config {
        /// What to do with the settings.
        #[command(subcommand)]
        action: ConfigAction,
    },
    /// The background loop that `start` launches. Not meant to be run directly.
    #[command(hide = true)]
    Watch,
}

/// The `config` subcommands.
#[derive(Debug, Subcommand)]
pub enum ConfigAction {
    /// Print all settings.
    Show,
    /// Print one setting.
    Get {
        /// Setting name, e.g. `grace_seconds`.
        key: String,
    },
    /// Validate and save one setting.
    Set {
        /// Setting name, e.g. `grace_seconds`.
        key: String,
        /// New value. Quote it if it contains spaces.
        #[arg(allow_hyphen_values = true)]
        value: String,
    },
    /// Restore the default settings.
    Reset,
}
