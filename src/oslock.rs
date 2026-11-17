//! The operating system's own screen-lock timer, for `grace_seconds = os`.
//!
//! The OS locks the screen after some idle time (no keyboard or mouse). With
//! `grace_seconds = os`, NoDonuts uses that same number as its grace period.
//! Reading it means asking the desktop's settings, which differs per system:
//!
//! - Linux with GNOME (Ubuntu's default): `gsettings`. The screen blanks after
//!   `idle-delay`, then locks `lock-delay` later (if locking is enabled).
//! - macOS: `pmset` for the display-sleep time, plus the "require password
//!   after" delay. This has not been tried on a real Mac, so it is a best
//!   guess; if it can't be read, `None` makes NoDonuts use its default.
//! - Other desktops (KDE, ...): not supported, so `None`.
//!
//! The parsing is kept apart from running the commands, so it can be unit
//! tested with sample output.

use std::process::Command;

/// How long the OS waits, idle, before locking the screen, in seconds.
/// `None` if it can't be read (unsupported desktop, no such setting), or if
/// the OS never locks the screen on its own.
pub fn lock_after_seconds() -> Option<u64> {
    read_system()
}

/// Parses what `gsettings get` prints for a number: `uint32 900` (or `900`).
pub fn parse_gsettings_number(text: &str) -> Option<u64> {
    text.split_whitespace().last()?.parse().ok()
}

/// Parses what `gsettings get` prints for a yes/no setting.
pub fn parse_gsettings_bool(text: &str) -> Option<bool> {
    match text.trim() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

/// GNOME: the screen blanks after `idle_seconds`, then locks `lock_delay`
/// seconds later. `None` if locking is off, or the screen never blanks
/// (`idle_seconds` of 0 means "never").
pub fn gnome_lock_seconds(lock_enabled: bool, idle_seconds: u64, lock_delay: u64) -> Option<u64> {
    if !lock_enabled || idle_seconds == 0 {
        return None;
    }
    Some(idle_seconds.saturating_add(lock_delay))
}

/// Finds the display-sleep time, in minutes, in `pmset -g` output (a line
/// like ` displaysleep         10`).
pub fn parse_pmset_display_sleep(text: &str) -> Option<u64> {
    text.lines().find_map(|line| {
        let mut words = line.split_whitespace();
        (words.next()? == "displaysleep")
            .then(|| words.next()?.parse().ok())
            .flatten()
    })
}

/// macOS: the display sleeps after `display_sleep_minutes`, and a password is
/// asked `password_delay` seconds after that (if one is required). `None` if
/// the display never sleeps (0 means "never") or no password is required.
pub fn mac_lock_seconds(
    display_sleep_minutes: u64,
    password_required: bool,
    password_delay: u64,
) -> Option<u64> {
    if !password_required || display_sleep_minutes == 0 {
        return None;
    }
    Some(
        display_sleep_minutes
            .saturating_mul(60)
            .saturating_add(password_delay),
    )
}

/// Runs a program and returns what it printed, or `None` if it isn't there
/// or it failed.
fn output_of(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program).args(args).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(not(target_os = "macos"))]
fn read_system() -> Option<u64> {
    let get = |schema: &str, key: &str| output_of("gsettings", &["get", schema, key]);
    let enabled = parse_gsettings_bool(&get("org.gnome.desktop.screensaver", "lock-enabled")?)?;
    let idle = parse_gsettings_number(&get("org.gnome.desktop.session", "idle-delay")?)?;
    let delay = parse_gsettings_number(&get("org.gnome.desktop.screensaver", "lock-delay")?)?;
    gnome_lock_seconds(enabled, idle, delay)
}

#[cfg(target_os = "macos")]
fn read_system() -> Option<u64> {
    let minutes = parse_pmset_display_sleep(&output_of("pmset", &["-g"])?)?;
    let setting = |key: &str| {
        output_of("defaults", &["read", "com.apple.screensaver", key])
            .and_then(|text| text.parse::<u64>().ok())
    };
    // Modern macOS asks for the password by default, so a setting we can't
    // read counts as "required", with no extra delay.
    let required = setting("askForPassword").is_none_or(|value| value != 0);
    let delay = setting("askForPasswordDelay").unwrap_or(0);
    mac_lock_seconds(minutes, required, delay)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gsettings_numbers_and_bools_parse() {
        assert_eq!(parse_gsettings_number("uint32 900\n"), Some(900));
        assert_eq!(parse_gsettings_number("0"), Some(0));
        assert_eq!(parse_gsettings_number(""), None);
        assert_eq!(parse_gsettings_number("uint32 soon"), None);
        assert_eq!(parse_gsettings_bool("true\n"), Some(true));
        assert_eq!(parse_gsettings_bool("false"), Some(false));
        assert_eq!(parse_gsettings_bool("maybe"), None);
    }

    #[test]
    fn gnome_adds_the_lock_delay_to_the_idle_time() {
        // This laptop: blank after 900 s, lock at once.
        assert_eq!(gnome_lock_seconds(true, 900, 0), Some(900));
        assert_eq!(gnome_lock_seconds(true, 300, 60), Some(360));
    }

    #[test]
    fn gnome_without_a_lock_has_no_timeout() {
        assert_eq!(gnome_lock_seconds(false, 900, 0), None, "locking off");
        assert_eq!(gnome_lock_seconds(true, 0, 0), None, "never blanks");
    }

    #[test]
    fn pmset_display_sleep_is_found() {
        let sample = "System-wide power settings:\n\
                      Currently in use:\n \
                      standby              1\n \
                      displaysleep         10\n \
                      sleep                1 (sleep prevented by powerd)\n";
        assert_eq!(parse_pmset_display_sleep(sample), Some(10));
        assert_eq!(parse_pmset_display_sleep("sleep 1\n"), None);
        assert_eq!(parse_pmset_display_sleep(" displaysleep  never\n"), None);
    }

    #[test]
    fn mac_adds_the_password_delay_to_the_display_sleep() {
        assert_eq!(mac_lock_seconds(10, true, 0), Some(600));
        assert_eq!(mac_lock_seconds(2, true, 5), Some(125));
        assert_eq!(mac_lock_seconds(10, false, 0), None, "no password asked");
        assert_eq!(mac_lock_seconds(0, true, 0), None, "display never sleeps");
    }
}
