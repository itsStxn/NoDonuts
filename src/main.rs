//! The `nodonuts` command: parses the command line and hands off to the
//! module that does the work.

use amzn_no_donuts::cli::{Cli, Command, ConfigAction};
use amzn_no_donuts::config::{self, Config};
use amzn_no_donuts::files::Paths;
use amzn_no_donuts::{autostart, daemon, enroll, watcher};
use anyhow::{Result, bail};
use clap::Parser;

// Returning `Result` from `main` means any error that reaches here is
// printed (with its context chain) and the program exits with status 1.
fn main() -> Result<()> {
    let cli = Cli::parse();
    let paths = Paths::locate()?;
    match cli.command {
        Command::Enroll => enroll::run(&paths),
        Command::Start => daemon::start(&paths),
        Command::Stop => daemon::stop(&paths),
        Command::Status => daemon::status(&paths),
        Command::Refresh => refresh(&paths),
        Command::Watch => watcher::run(&paths),
        Command::Config { action } => run_config(&paths, action),
    }
}

fn run_config(paths: &Paths, action: ConfigAction) -> Result<()> {
    match action {
        ConfigAction::Show => {
            let config = config::load(&paths.config_file)?;
            println!("# {}", paths.config_file.display());
            print!("{}", config.to_toml()?);
        }
        ConfigAction::Get { key } => {
            let config = config::load(&paths.config_file)?;
            println!("{}", config.get(&key)?);
        }
        ConfigAction::Set { key, value } => {
            refuse_while_running(paths)?;
            let mut config = config::load(&paths.config_file)?;
            config.set(&key, &value)?;
            config::save(&paths.config_file, &config)?;
            println!("{key} = {}", config.get(&key)?);
            if key == "autostart" {
                apply_autostart(paths, config.autostart)?;
            }
        }
        ConfigAction::Reset => {
            refuse_while_running(paths)?;
            config::save(&paths.config_file, &Config::default())?;
            println!("Settings reset to defaults.");
            apply_autostart(paths, false)?;
        }
    }
    Ok(())
}

/// Settings can only change while the watcher is stopped. Otherwise someone
/// at the laptop could quietly change them (e.g. swap the command for a
/// harmless one) for the next start, and the running watcher would be using
/// different settings from the ones `config show` prints.
fn refuse_while_running(paths: &Paths) -> Result<()> {
    if let Some(pid) = daemon::running_pid(paths)? {
        bail!(
            "the watcher is running (PID {pid}), so settings can't be changed. \
             Run `nodonuts stop` first"
        );
    }
    Ok(())
}

/// Creates or removes the login entry, and says what happened.
fn apply_autostart(paths: &Paths, enabled: bool) -> Result<()> {
    match autostart::sync(paths, enabled)? {
        Some(program) => {
            println!("NoDonuts will start when you log in.");
            println!("  runs:  {} start", program.display());
            println!("  entry: {}", paths.autostart_file.display());
        }
        None => println!("NoDonuts will not start at login."),
    }
    Ok(())
}

/// `nodonuts refresh`: rebuilds everything NoDonuts generates from the
/// settings. Today that is only the login entry. Safe to run any time; a
/// running watcher is not touched.
fn refresh(paths: &Paths) -> Result<()> {
    let config = config::load(&paths.config_file)?;
    apply_autostart(paths, config.autostart)
}
