//! The `nodonuts` command: parses the command line and hands off to the
//! module that does the work.

use amzn_no_donuts::cli::{Cli, Command, ConfigAction};
use amzn_no_donuts::config::{self, Config};
use amzn_no_donuts::files::Paths;
use amzn_no_donuts::{daemon, enroll, watcher};
use anyhow::Result;
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
            let mut config = config::load(&paths.config_file)?;
            config.set(&key, &value)?;
            config::save(&paths.config_file, &config)?;
            println!("{key} = {}", config.get(&key)?);
            remind_if_running(paths)?;
        }
        ConfigAction::Reset => {
            config::save(&paths.config_file, &Config::default())?;
            println!("Settings reset to defaults.");
            remind_if_running(paths)?;
        }
    }
    Ok(())
}

/// A running watcher keeps the settings it started with.
fn remind_if_running(paths: &Paths) -> Result<()> {
    if daemon::running_pid(paths)?.is_some() {
        println!("The watcher is running with the old settings.");
        println!("Apply the change with: nodonuts stop && nodonuts start");
    }
    Ok(())
}
