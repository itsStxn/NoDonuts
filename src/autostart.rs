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

    #[test]
    fn unsafe_paths_are_refused() {
        assert!(desktop_entry(Path::new("/tmp/evil\nExec=rm")).is_err());
        assert!(launch_agent(Path::new("/tmp/evil\n")).is_err());
    }

    #[test]
    fn sync_writes_then_removes_the_entry() {
        let dir = std::env::temp_dir().join(format!("nodonuts-autostart-{}", std::process::id()));
        let paths = Paths {
            config_file: dir.join("config.toml"),
            face_file: dir.join("face.toml"),
            pid_file: dir.join("pid"),
            log_file: dir.join("log"),
            autostart_file: dir.join("autostart").join("nodonuts.desktop"),
        };
        assert!(!is_enabled(&paths));
        // Disabling when nothing exists is fine.
        assert_eq!(sync(&paths, false).unwrap(), None);

        let program = sync(&paths, true).unwrap();
        assert!(program.is_some());
        assert!(is_enabled(&paths));
        files::check_private(&paths.autostart_file).unwrap();

        sync(&paths, false).unwrap();
        assert!(!is_enabled(&paths));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
