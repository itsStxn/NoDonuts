//! The `nodonuts` command: parses the command line and hands off to the
//! module that does the work.

use amzn_no_donuts::cli::{Cli, Command, ConfigAction};
use amzn_no_donuts::config::{self, Config};
use amzn_no_donuts::files::Paths;
use amzn_no_donuts::{autostart, camera, daemon, enroll, password, prompt, watcher};
use anyhow::{Result, bail};
use clap::Parser;
use std::io::{BufRead, IsTerminal, Write};

// Returning `Result` from `main` means any error that reaches here is
// printed (with its context chain) and the program exits with status 1.
fn main() -> Result<()> {
    exit_quietly_on_broken_pipe();
    let cli = Cli::parse();
    let paths = Paths::locate()?;
    match cli.command {
        Command::Enroll => enroll::run(&paths),
        Command::Start => daemon::start(&paths),
        Command::Stop => daemon::stop(&paths),
        Command::Status => daemon::status(&paths),
        Command::Refresh => refresh(&paths),
        Command::Password { remove } => set_password(&paths, remove),
        Command::Camera => choose_camera(&paths),
        Command::Watch => watcher::run(&paths),
        Command::Config { action } => run_config(&paths, action),
    }
}

/// Makes `nodonuts config show | head -1` end quietly instead of crashing.
///
/// When the reader of our output goes away (here, `head` after one line),
/// writing to it fails with "broken pipe". Rust's `println!` turns that into
/// a panic with a scary message. Most command-line tools just stop silently
/// instead (they're ended by the SIGPIPE signal, exit status 141). Restoring
/// that signal would need `unsafe` code, so instead this catches exactly
/// that one panic and exits the same way. Any other panic is reported as
/// usual.
fn exit_quietly_on_broken_pipe() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // The panic message is a `String` or a `&str`, depending on how the
        // panic was raised.
        let payload = info.payload();
        let message = payload
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| payload.downcast_ref::<&str>().copied())
            .unwrap_or("");
        if message.starts_with("failed printing to") && message.contains("Broken pipe") {
            std::process::exit(141); // 128 + SIGPIPE (13), like other tools
        }
        default_hook(info);
    }));
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

/// `nodonuts camera`: list the usable cameras and let the owner pick one.
fn choose_camera(paths: &Paths) -> Result<()> {
    // Also: on Linux the watcher holds the camera, and listing opens each.
    refuse_while_running(paths, "the camera")?;
    if !std::io::stdin().is_terminal() {
        bail!("`nodonuts camera` is interactive: run it in a terminal");
    }
    camera::request_permission()?;
    let cameras = camera::list_usable();
    if cameras.is_empty() {
        bail!("no usable camera found (is one connected, and not used by another app?)");
    }
    let mut config = config::load(&paths.config_file)?;
    let built_in = camera::pick_built_in(&cameras).map(|c| c.index);
    let current = camera::resolve(&config.camera, &cameras).map(|(c, _)| c.index);

    println!("Cameras:");
    for (number, entry) in cameras.iter().enumerate() {
        let mut notes = Vec::new();
        if Some(entry.index) == built_in {
            notes.push("built-in");
        }
        if Some(entry.index) == current {
            notes.push("current");
        }
        let notes = if notes.is_empty() {
            String::new()
        } else {
            format!("   ({})", notes.join(", "))
        };
        println!("  {}. {}{notes}", number + 1, entry.name);
    }

    let count = cameras.len();
    let chosen = loop {
        print!("Choose a camera [1-{count}], or press Enter to keep the current one: ");
        std::io::stdout().flush()?;
        let mut line = String::new();
        if std::io::stdin().lock().read_line(&mut line)? == 0 {
            return Ok(()); // end of input: keep the current camera
        }
        let line = line.trim();
        if line.is_empty() {
            println!("Camera unchanged.");
            return Ok(());
        }
        match line.parse::<usize>() {
            Ok(n) if (1..=count).contains(&n) => break &cameras[n - 1],
            _ => println!("Please type a number from 1 to {count}."),
        }
    };

    // The built-in camera is saved as `built-in`, not by name, so it keeps
    // meaning "the laptop's own camera" if names or numbers change.
    let setting = if Some(chosen.index) == built_in {
        camera::BUILT_IN.to_string()
    } else {
        chosen.name.clone()
    };
    if setting == config.camera {
        println!("Camera unchanged: {}", chosen.name);
        return Ok(());
    }
    config.camera = setting;
    config::save(&paths.config_file, &config)?;
    println!("Camera set to: {}", chosen.name);
    if paths.face_file.exists() {
        println!(
            "Tip: re-enroll with this camera (`nodonuts enroll`); a different lens and angle \
             can make your face harder to recognize."
        );
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
