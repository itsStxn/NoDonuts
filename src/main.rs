//! The `nodonuts` command: parses the command line and hands off to the
//! module that does the work.

use amzn_no_donuts::cli::{Cli, Command, ConfigAction};
use amzn_no_donuts::config::{self, Config};
use amzn_no_donuts::files::Paths;
use amzn_no_donuts::{autostart, daemon, enroll, password, prompt, watcher};
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
        Command::Password { remove } => set_password(&paths, remove),
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
            refuse_while_running(paths, "settings")?;
            let mut config = config::load(&paths.config_file)?;
            config.set(&key, &value)?;
            config::save(&paths.config_file, &config)?;
            println!("{key} = {}", config.get(&key)?);
            if key == "autostart" {
                apply_autostart(paths, config.autostart)?;
            }
        }
        ConfigAction::Reset => {
            refuse_while_running(paths, "settings")?;
            config::save(&paths.config_file, &Config::default())?;
            println!("Settings reset to defaults.");
            apply_autostart(paths, false)?;
        }
    }
    Ok(())
}

/// `nodonuts password`: set, change or remove the fallback password.
fn set_password(paths: &Paths, remove: bool) -> Result<()> {
    refuse_while_running(paths, "the fallback password")?;
    if remove {
        if password::remove(&paths.password_file)? {
            println!("Fallback password removed.");
        } else {
            println!("No fallback password was set.");
        }
        return Ok(());
    }
    let typed = prompt::ask_new_password()?;
    password::save(&paths.password_file, &typed)?;
    println!("Fallback password saved.");
    Ok(())
}

/// Settings and the login entry can only change while the watcher is
/// stopped. Otherwise someone at the laptop could quietly change them (e.g.
/// swap the command for a harmless one) for the next start, and the running
/// watcher would be using different settings from the ones `config show`
/// prints. `what` names the thing being changed, for the message.
fn refuse_while_running(paths: &Paths, what: &str) -> Result<()> {
    if let Some(pid) = daemon::running_pid(paths)? {
        bail!(
            "the watcher is running (PID {pid}), so {what} can't be changed. \
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
/// settings. Today that is only the login entry. Refused while the watcher
/// runs, like changing settings: otherwise someone at the laptop could point
/// the login entry at another program for the next login.
fn refresh(paths: &Paths) -> Result<()> {
    refuse_while_running(paths, "the login entry")?;
    let config = config::load(&paths.config_file)?;
    apply_autostart(paths, config.autostart)
}
