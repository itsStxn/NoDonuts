//! `autostart`: start the watcher automatically when the owner logs in.
//!
//! "At login" rather than "at boot": the watcher needs the owner's desktop
//! session. Linux only gives camera access to the logged-in user, and
//! commands like `loginctl lock-session` act on that session.
//!
//! - Linux: a desktop autostart entry, `~/.config/autostart/nodonuts.desktop`,
//!   which GNOME, KDE and most desktops run at login.
//! - macOS: a LaunchAgent, `~/Library/LaunchAgents/com.amazon.nodonuts.plist`,
//!   which launchd runs at login.
//!
//! Either way the entry just runs `nodonuts start`.

use crate::files::{self, Paths};
use anyhow::{Context, Result, bail};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

/// Makes the login entry match `enabled`: writes it, or removes it.
///
/// Returns the program the entry runs (`None` when disabled), so callers
/// can show it.
pub fn sync(paths: &Paths, enabled: bool) -> Result<Option<PathBuf>> {
    if !enabled {
        remove(&paths.autostart_file)?;
        return Ok(None);
    }
    // The full path of the program running right now. The entry points at
    // it, so run `config set` from the copy you want started at login.
    let program = std::env::current_exe().context("could not find the nodonuts program")?;
    let entry = login_entry(&program)?;
    // Private like the config: whatever this file names runs at login.
    files::write_private(&paths.autostart_file, entry.as_bytes())?;
    Ok(Some(program))
}

/// True if the login entry exists.
pub fn is_enabled(paths: &Paths) -> bool {
    paths.autostart_file.exists()
}

/// The program the login entry will run, read back from the entry.
/// `Ok(None)` means there is no entry (autostart is off).
///
/// The entry stores a full path, so it breaks silently if that program is
/// moved or deleted. `nodonuts status` uses this to warn about it.
pub fn stored_program(paths: &Paths) -> Result<Option<PathBuf>> {
    let path = &paths.autostart_file;
    if !path.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("could not read {}", path.display()))?;
    match program_in_entry(&text) {
        Some(program) => Ok(Some(program)),
        None => bail!(
            "{} is not a login entry NoDonuts recognizes",
            path.display()
        ),
    }
}

/// Checks the login entry against the `autostart` setting, without
/// changing anything. Returns a description of what is wrong, or `None` if
/// all is well. `nodonuts refresh` fixes any of these.
pub fn problem(paths: &Paths, enabled: bool) -> Option<String> {
    let entry = paths.autostart_file.display();
    match (enabled, stored_program(paths)) {
        (_, Err(error)) => Some(format!("{error:#}")),
        (true, Ok(None)) => Some(format!(
            "autostart is on, but there is no login entry ({entry})"
        )),
        (true, Ok(Some(program))) if !program.is_file() => Some(format!(
            "the login entry runs {}, which no longer exists (moved or deleted)",
            program.display()
        )),
        (false, Ok(Some(_))) => Some(format!(
            "autostart is off, but a login entry still exists ({entry})"
        )),
        _ => None,
    }
}

#[cfg(target_os = "macos")]
fn program_in_entry(text: &str) -> Option<PathBuf> {
    program_in_launch_agent(text)
}

#[cfg(not(target_os = "macos"))]
fn program_in_entry(text: &str) -> Option<PathBuf> {
    program_in_desktop_entry(text)
}

/// Reads the program path back out of a Linux autostart file: undoes the
/// escaping that [`desktop_entry`] applies to the `Exec` line.
pub fn program_in_desktop_entry(text: &str) -> Option<PathBuf> {
    let exec = text.lines().find_map(|line| line.strip_prefix("Exec="))?;
    // Step 1: undo the desktop-file string escaping (`\\` -> `\`).
    let mut value = String::new();
    let mut chars = exec.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            // `?` here: a lone trailing backslash means a broken entry.
            value.push(chars.next()?);
        } else {
            value.push(c);
        }
    }
    // Step 2: the program is the first argument, wrapped in double quotes,
    // with `\` escaping the next character inside them.
    let mut chars = value.strip_prefix('"')?.chars();
    let mut program = String::new();
    loop {
        match chars.next()? {
            '"' => break,
            '\\' => program.push(chars.next()?),
            c => program.push(c),
        }
    }
    // Step 3: `%%` is a literal `%`.
    Some(PathBuf::from(program.replace("%%", "%")))
}

/// Reads the program path back out of a macOS LaunchAgent: the first
/// `<string>` after `ProgramArguments`.
pub fn program_in_launch_agent(text: &str) -> Option<PathBuf> {
    let after = text.split_once("<key>ProgramArguments</key>")?.1;
    let start = after.split_once("<string>")?.1;
    let escaped = start.split_once("</string>")?.0;
    Some(PathBuf::from(xml_unescape(escaped)))
}

fn remove(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        // Already gone: nothing to do.
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("could not remove {}", path.display())),
    }
}

// `#[cfg(...)]` compiles a line only on the matching OS, so each platform
// builds just its own kind of entry.
#[cfg(target_os = "macos")]
fn login_entry(program: &Path) -> Result<String> {
    launch_agent(program)
}

#[cfg(not(target_os = "macos"))]
fn login_entry(program: &Path) -> Result<String> {
    desktop_entry(program)
}

/// The Linux autostart file for `program`.
pub fn desktop_entry(program: &Path) -> Result<String> {
    let program = path_text(program)?;
    // The `Exec` line has its own quoting rules (from the freedesktop.org
    // Desktop Entry spec): wrap the path in double quotes and backslash-
    // escape `"`, `` ` ``, `$` and `\` inside them. Then the whole value is
    // escaped once more as a desktop-file string, which doubles every
    // backslash. A literal `%` is written `%%`.
    let mut quoted = String::new();
    for c in program.chars() {
        match c {
            '"' | '`' | '$' => {
                quoted.push_str("\\\\");
                quoted.push(c);
            }
            '\\' => quoted.push_str("\\\\\\\\"),
            '%' => quoted.push_str("%%"),
            _ => quoted.push(c),
        }
    }
    Ok(format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=NoDonuts\n\
         Comment=Start the NoDonuts watcher at login (autostart)\n\
         Exec=\"{quoted}\" start\n\
         Terminal=false\n\
         NoDisplay=true\n\
         X-GNOME-Autostart-enabled=true\n"
    ))
}

/// The macOS LaunchAgent file for `program`.
pub fn launch_agent(program: &Path) -> Result<String> {
    let program = xml_escape(path_text(program)?);
    // `AbandonProcessGroup`: `nodonuts start` exits once the watcher is up;
    // this stops launchd from cleaning up the watcher along with it.
    Ok(format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.amazon.nodonuts</string>
    <key>ProgramArguments</key>
    <array>
        <string>{program}</string>
        <string>start</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>AbandonProcessGroup</key>
    <true/>
</dict>
</plist>
"#
    ))
}

/// The path as text. Refuses paths that cannot be written safely into a
/// one-line entry.
fn path_text(program: &Path) -> Result<&str> {
    let Some(text) = program.to_str() else {
        bail!(
            "the nodonuts path is not valid UTF-8: {}",
            program.display()
        );
    };
    if text.chars().any(char::is_control) {
        bail!("the nodonuts path contains control characters");
    }
    Ok(text)
}

fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// The reverse of [`xml_escape`]. `&amp;` goes last, so `&amp;lt;` becomes
/// `&lt;` and not `<`.
fn xml_unescape(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_entry_runs_start() {
        let entry = desktop_entry(Path::new("/home/me/.local/bin/nodonuts")).unwrap();
        assert!(entry.starts_with("[Desktop Entry]\n"));
        assert!(entry.contains("\nExec=\"/home/me/.local/bin/nodonuts\" start\n"));
        assert!(entry.contains("\nType=Application\n"));
    }

    #[test]
    fn desktop_entry_escapes_special_characters() {
        let entry = desktop_entry(Path::new("/a b/100%/$x/nodonuts")).unwrap();
        // Spaces are fine inside the quotes, `%` doubles, `$` is escaped.
        assert!(entry.contains("Exec=\"/a b/100%%/\\\\$x/nodonuts\" start"));
    }

    #[test]
    fn launch_agent_runs_start_and_escapes_xml() {
        let agent = launch_agent(Path::new("/Users/me/R&D/nodonuts")).unwrap();
        assert!(agent.contains("<string>/Users/me/R&amp;D/nodonuts</string>"));
        assert!(agent.contains("<string>start</string>"));
        assert!(agent.contains("<key>RunAtLoad</key>\n    <true/>"));
    }

    /// Paths with every character the escaping has to handle.
    const TRICKY_PATHS: [&str; 5] = [
        "/home/me/.local/bin/nodonuts",
        "/a b/100%/$x/nodonuts",
        "/q\"uote/back\\slash/`tick`/nodonuts",
        "/Users/me/R&D/<dir>/it's/nodonuts",
        "/odd/&amp;lt;/nodonuts",
    ];

    #[test]
    fn desktop_entry_path_round_trips() {
        for path in TRICKY_PATHS {
            let entry = desktop_entry(Path::new(path)).unwrap();
            assert_eq!(
                program_in_desktop_entry(&entry),
                Some(PathBuf::from(path)),
                "{path}"
            );
        }
    }

    #[test]
    fn launch_agent_path_round_trips() {
        for path in TRICKY_PATHS {
            let agent = launch_agent(Path::new(path)).unwrap();
            assert_eq!(
                program_in_launch_agent(&agent),
                Some(PathBuf::from(path)),
                "{path}"
            );
        }
    }

    #[test]
    fn unrecognized_entries_give_no_path() {
        assert_eq!(program_in_desktop_entry("[Desktop Entry]\nName=x\n"), None);
        assert_eq!(program_in_desktop_entry("Exec=nodonuts start\n"), None);
        assert_eq!(
            program_in_desktop_entry("Exec=\"/unterminated start\n"),
            None
        );
        assert_eq!(program_in_launch_agent("<plist></plist>"), None);
    }

    #[test]
    fn unsafe_paths_are_refused() {
        assert!(desktop_entry(Path::new("/tmp/evil\nExec=rm")).is_err());
        assert!(launch_agent(Path::new("/tmp/evil\n")).is_err());
    }

    fn temp_paths(name: &str) -> (PathBuf, Paths) {
        let dir = std::env::temp_dir().join(format!("nodonuts-{name}-{}", std::process::id()));
        let paths = Paths {
            config_file: dir.join("config.toml"),
            face_file: dir.join("face.toml"),
            pid_file: dir.join("pid"),
            lock_file: dir.join("nodonuts.lock"),
            log_file: dir.join("log"),
            log_started_file: dir.join("log_started"),
            autostart_file: dir.join("autostart").join("nodonuts.desktop"),
            stop_result_file: dir.join("last_stop"),
            password_file: dir.join("fallback_checkout.toml"),
            control_socket: dir.join("control.sock"),
        };
        (dir, paths)
    }

    #[test]
    fn problem_spots_every_mismatch() {
        let (dir, paths) = temp_paths("problem");
        // Off and no entry: fine. On and no entry: a problem.
        assert_eq!(problem(&paths, false), None);
        assert!(problem(&paths, true).unwrap().contains("no login entry"));

        // On, entry pointing at an existing program: fine.
        sync(&paths, true).unwrap();
        assert_eq!(problem(&paths, true), None);
        // Off but an entry remains: a problem.
        assert!(problem(&paths, false).unwrap().contains("still exists"));

        // On, entry pointing at a program that is gone: a problem.
        let entry = desktop_entry(&dir.join("gone/nodonuts")).unwrap();
        std::fs::write(&paths.autostart_file, entry).unwrap();
        assert!(problem(&paths, true).unwrap().contains("no longer exists"));

        // A garbled entry: a problem either way.
        std::fs::write(&paths.autostart_file, "garbage").unwrap();
        assert!(problem(&paths, true).is_some());
        assert!(problem(&paths, false).is_some());

        // Checking never changes anything.
        assert_eq!(
            std::fs::read_to_string(&paths.autostart_file).unwrap(),
            "garbage"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn sync_writes_then_removes_the_entry() {
        let (dir, paths) = temp_paths("autostart");
        assert!(!is_enabled(&paths));
        // Disabling when nothing exists is fine.
        assert_eq!(sync(&paths, false).unwrap(), None);

        assert_eq!(stored_program(&paths).unwrap(), None);
        let program = sync(&paths, true).unwrap();
        assert!(program.is_some());
        assert!(is_enabled(&paths));
        files::check_private(&paths.autostart_file).unwrap();
        // The entry points at the program running the test.
        assert_eq!(stored_program(&paths).unwrap(), program);

        // A garbled entry is an error, not a silent "no".
        std::fs::write(&paths.autostart_file, "garbage").unwrap();
        assert!(stored_program(&paths).is_err());

        sync(&paths, false).unwrap();
        assert!(!is_enabled(&paths));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
