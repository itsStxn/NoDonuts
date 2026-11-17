//! Settings: load, save, validate, get, set and reset `config.toml`.

use crate::alert;
use crate::files;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// The names accepted by `config get` and `config set`, alphabetically.
pub const KEYS: [&str; 16] = [
    "alert_location",
    "autostart",
    "camera",
    "check_every_ms",
    "command",
    "command_once",
    "face_checkout",
    "face_checks",
    "grace_period",
    "grace_seconds",
    "logs_maxdays",
    "pwd_ms",
    "stranger_checks",
    "stranger_mode",
    "threshold_owner",
    "threshold_stranger",
];

/// The grace period used when `grace_seconds` is `os` but the OS timeout
/// can't be read.
pub const DEFAULT_GRACE_SECONDS: u64 = 30;

/// The `grace_seconds` setting: a number of seconds, or `os` to use the
/// operating system's own screen-lock timeout (read when the watcher
/// starts).
///
/// In the TOML file it is either a number (`grace_seconds = 30`) or the text
/// `"os"`. `serde` goes through [`StoredGrace`] to allow both.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "StoredGrace", into = "StoredGrace")]
pub enum GraceSeconds {
    /// A fixed number of seconds, greater than 0.
    Seconds(u64),
    /// Whatever the OS waits before locking the screen.
    Os,
}

/// What `grace_seconds` looks like in the file: a number or some text.
#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum StoredGrace {
    Number(u64),
    Word(String),
}

impl From<GraceSeconds> for StoredGrace {
    fn from(grace: GraceSeconds) -> StoredGrace {
        match grace {
            GraceSeconds::Seconds(seconds) => StoredGrace::Number(seconds),
            GraceSeconds::Os => StoredGrace::Word("os".to_string()),
        }
    }
}

impl TryFrom<StoredGrace> for GraceSeconds {
    type Error = String;

    fn try_from(stored: StoredGrace) -> std::result::Result<GraceSeconds, String> {
        match stored {
            StoredGrace::Number(seconds) => Ok(GraceSeconds::Seconds(seconds)),
            StoredGrace::Word(word) if word == "os" => Ok(GraceSeconds::Os),
            StoredGrace::Word(_) => Err(GRACE_HELP.to_string()),
        }
    }
}

const GRACE_HELP: &str = "grace_seconds must be a whole number greater than 0, or `os`";

impl From<u64> for GraceSeconds {
    fn from(seconds: u64) -> GraceSeconds {
        GraceSeconds::Seconds(seconds)
    }
}

impl GraceSeconds {
    /// Parses a `config set` value: a whole number above 0, or `os`.
    pub fn parse(value: &str) -> Result<GraceSeconds> {
        let value = value.trim();
        if value == "os" {
            return Ok(GraceSeconds::Os);
        }
        match value.parse::<u64>() {
            Ok(seconds) if seconds > 0 => Ok(GraceSeconds::Seconds(seconds)),
            _ => bail!(GRACE_HELP),
        }
    }

    /// The number of seconds to use. `Os` has to be turned into a number
    /// first (see `Config::resolve_grace`); until then it counts as the
    /// default, so a missed step can't mean a grace period of 0.
    pub fn seconds(self) -> u64 {
        match self {
            GraceSeconds::Seconds(seconds) => seconds,
            GraceSeconds::Os => DEFAULT_GRACE_SECONDS,
        }
    }

    /// How it's written: `30` or `os`.
    fn text(self) -> String {
        match self {
            GraceSeconds::Seconds(seconds) => seconds.to_string(),
            GraceSeconds::Os => "os".to_string(),
        }
    }
}

/// All settings. `Serialize`/`Deserialize` let `serde` convert this struct
/// to and from TOML text for us.
///
/// Keep the fields in alphabetical order: serde writes them in the order
/// they're declared, so this is the order `config show` prints them in (a
/// test checks it).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
// `default`: keys missing from the file get their default value.
// `deny_unknown_fields`: a typo like `grace_second` is an error, not ignored.
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Where `NODONUTS ALERT.txt` goes when the command runs: `desktop`,
    /// `off` (no alert, no notification), or a folder (`/...` or `~/...`).
    pub alert_location: String,
    /// If true, the watcher starts by itself when the owner logs in.
    pub autostart: bool,
    /// Which camera to use: `built-in` (the laptop's own), or a camera's
    /// name as chosen with `nodonuts camera`.
    pub camera: String,
    /// How often to look at the camera, in milliseconds.
    pub check_every_ms: u64,
    /// The bash command to run.
    pub command: String,
    /// If true, once the command has run the watcher stops and turns
    /// autostart off, so a command that reboots or logs out can't loop
    /// (e.g. after a haircut, at every login). Best with a command that
    /// locks the owner out.
    pub command_once: bool,
    /// If true, stopping the watcher (`nodonuts stop`, `kill`, `pkill`)
    /// needs the owner's face. If the owner isn't seen, the command runs
    /// before the watcher exits.
    pub face_checkout: bool,
    /// `nodonuts stop` asks for the fallback password if the owner isn't
    /// recognized within this many camera frames. One frame every
    /// `check_every_ms`, like `stranger_checks`, so the wait is up to
    /// `face_checks x check_every_ms`. 1 to 10.
    pub face_checks: u64,
    /// If true, the command runs once the owner has been away for
    /// `grace_seconds`. If false, being away never runs it and
    /// `grace_seconds` does nothing: only stranger mode (a stranger at the
    /// laptop while the owner isn't) can run the command.
    pub grace_period: bool,
    /// How long the owner may be away before the command runs (when
    /// `grace_period` is on): a number of seconds, or `os` for the operating
    /// system's own screen-lock timeout.
    pub grace_seconds: GraceSeconds,
    /// Empty the watcher's log once it's this many days old. 0 = never.
    pub logs_maxdays: u64,
    /// How long `nodonuts stop` gives to type the fallback password, in
    /// milliseconds. Kept short: while the watcher waits for the password
    /// it isn't checking the camera, so a long wait is free time for
    /// someone at the laptop. 1 to 60000.
    pub pwd_ms: u64,
    /// Stranger mode fires only after this many checks in a row see a
    /// stranger and not the owner. One check every `check_every_ms`, so the
    /// wait is `stranger_checks x check_every_ms`. At least 1.
    /// `alias`: config files from before the rename used `aggressive_checks`.
    #[serde(alias = "aggressive_checks")]
    pub stranger_checks: u64,
    /// If true, run the command immediately (no grace period) when someone
    /// other than the owner is in front of the camera and the owner is not.
    /// This is the tool's main job, so it is on by default.
    /// `alias`: config files from before the rename used `aggressive_mode`.
    #[serde(alias = "aggressive_mode")]
    pub stranger_mode: bool,
    /// Minimum similarity (0 to 1) for a face to count as the owner.
    /// `alias`: config files saved before the rename used `match_threshold`;
    /// they still load, and are saved with the new name.
    #[serde(alias = "match_threshold")]
    pub threshold_owner: f64,
    /// A face below this similarity (0 to 1) counts as a stranger, for
    /// stranger mode. Between this and `threshold_owner` a face is
    /// "unsure": not the owner, but not a stranger either. At most
    /// `threshold_owner`; setting them equal removes the unsure zone.
    pub threshold_stranger: f64,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            alert_location: alert::DESKTOP.to_string(),
            autostart: false,
            camera: "built-in".to_string(),
            // Each check costs ~100 ms of CPU (the face models): every
            // 1000 ms is ~10% of one core, half of what 500 ms costs, and
            // nothing a 30 s grace period would notice.
            check_every_ms: 1000,
            command: "echo 'The owner is gone!'".to_string(),
            command_once: true,
            // On by default: without it, anyone at the laptop can simply
            // run `nodonuts stop` during the grace period.
            face_checkout: true,
            // 2 frames, about 2 s at the default check_every_ms.
            face_checks: 2,
            // On: the tool has always run the command after an absence.
            grace_period: true,
            // Follow the OS's own screen-lock timeout (read when the
            // watcher starts); 30 s if it can't be read.
            grace_seconds: GraceSeconds::Os,
            logs_maxdays: 7,
            // Enough for a short password typed from memory.
            pwd_ms: 3000,
            // 2 = one misread frame of the owner is not enough to fire.
            stranger_checks: 2,
            // On: protecting the laptop from someone else using it is the
            // tool's main job.
            stranger_mode: true,
            // SFace's recommended threshold for cosine similarity.
            threshold_owner: 0.36,
            // Well below the owner's: a turned or badly lit owner usually
            // scores in between, which is "unsure", not a stranger.
            threshold_stranger: 0.2,
        }
    }
}

impl Config {
    /// Checks every setting is usable.
    pub fn validate(&self) -> Result<()> {
        if self.grace_seconds == GraceSeconds::Seconds(0) {
            bail!(GRACE_HELP);
        }
        if self.command.trim().is_empty() {
            bail!("command must not be empty");
        }
        if self.camera.trim().is_empty() {
            bail!("camera must not be empty (use `built-in` for the laptop's own camera)");
        }
        // `contains` on a range is false for NaN too.
        if !(0.0..=1.0).contains(&self.threshold_owner) {
            bail!("threshold_owner must be between 0 and 1");
        }
        if !(0.0..=1.0).contains(&self.threshold_stranger) {
            bail!("threshold_stranger must be between 0 and 1");
        }
        if self.threshold_stranger > self.threshold_owner {
            bail!(
                "threshold_stranger ({}) can't be higher than threshold_owner ({}). \
                 Lower threshold_stranger first, or raise threshold_owner",
                self.threshold_stranger,
                self.threshold_owner
            );
        }
        if self.check_every_ms == 0 {
            bail!("check_every_ms must be a whole number greater than 0");
        }
        if self.stranger_checks == 0 {
            bail!("stranger_checks must be a whole number of 1 or more");
        }
        // Capped so `nodonuts stop` stays quick (one frame per check_every_ms).
        if !(1..=10).contains(&self.face_checks) {
            bail!("face_checks must be a whole number from 1 to 10");
        }
        if self.logs_maxdays > 3650 {
            bail!("logs_maxdays must be a whole number from 0 (never empty) to 3650");
        }
        // Only the form here; whether the folder exists is checked by
        // `config set` (a folder that disappears later must not stop the
        // watcher from starting: the alert falls back instead).
        alert::check_setting(&self.alert_location)?;
        // Capped at a minute: `nodonuts stop` waits at most that long for
        // each message from the watcher.
        if !(1..=60_000).contains(&self.pwd_ms) {
            bail!("pwd_ms must be a whole number from 1 to 60000");
        }
        Ok(())
    }

    /// Returns one setting as text.
    pub fn get(&self, key: &str) -> Result<String> {
        let value = match key {
            "grace_period" => self.grace_period.to_string(),
            "grace_seconds" => self.grace_seconds.text(),
            "command" => self.command.clone(),
            "command_once" => self.command_once.to_string(),
            "threshold_owner" => self.threshold_owner.to_string(),
            "threshold_stranger" => self.threshold_stranger.to_string(),
            "check_every_ms" => self.check_every_ms.to_string(),
            "stranger_mode" => self.stranger_mode.to_string(),
            "stranger_checks" => self.stranger_checks.to_string(),
            "alert_location" => self.alert_location.clone(),
            "autostart" => self.autostart.to_string(),
            "camera" => self.camera.clone(),
            "face_checkout" => self.face_checkout.to_string(),
            "face_checks" => self.face_checks.to_string(),
            "logs_maxdays" => self.logs_maxdays.to_string(),
            "pwd_ms" => self.pwd_ms.to_string(),
            _ => bail!(unknown_key(key)),
        };
        Ok(value)
    }

    /// Parses and validates `value`, then stores it. On error nothing changes.
    // `&mut self` means this method may modify the config it is called on.
    pub fn set(&mut self, key: &str, value: &str) -> Result<()> {
        // Change a copy, so a bad value leaves `self` untouched.
        let mut updated = self.clone();
        match key {
            "grace_period" => updated.grace_period = parse_on_off(key, value)?,
            "grace_seconds" => updated.grace_seconds = GraceSeconds::parse(value)?,
            "command" => updated.command = value.to_string(),
            "command_once" => updated.command_once = parse_on_off(key, value)?,
            "threshold_owner" => updated.threshold_owner = parse_fraction(key, value)?,
            "threshold_stranger" => updated.threshold_stranger = parse_fraction(key, value)?,
            "check_every_ms" => updated.check_every_ms = parse_whole_number(key, value)?,
            "stranger_mode" => updated.stranger_mode = parse_on_off(key, value)?,
            "stranger_checks" => {
                updated.stranger_checks = value
                    .trim()
                    .parse()
                    .with_context(|| format!("{key} must be a whole number of 1 or more"))?
            }
            "alert_location" => {
                let value = value.trim();
                alert::check_folder(value)?;
                updated.alert_location = value.to_string();
            }
            "autostart" => updated.autostart = parse_on_off(key, value)?,
            // Chosen from a list of the cameras actually connected, so
            // a typo'd name can't slip in.
            "camera" => bail!("choose the camera with `nodonuts camera`"),
            "face_checkout" => updated.face_checkout = parse_on_off(key, value)?,
            "pwd_ms" => {
                updated.pwd_ms = value
                    .trim()
                    .parse()
                    .with_context(|| format!("{key} must be a whole number from 1 to 60000"))?
            }
            "logs_maxdays" => {
                updated.logs_maxdays = value.trim().parse().with_context(|| {
                    format!("{key} must be a whole number from 0 (never empty) to 3650")
                })?
            }
            "face_checks" => {
                updated.face_checks = value
                    .trim()
                    .parse()
                    .with_context(|| format!("{key} must be a whole number from 1 to 10"))?
            }
            _ => bail!(unknown_key(key)),
        }
        updated.validate()?;
        *self = updated;
        Ok(())
    }

    /// A warning when these settings mean the command can never run on its
    /// own: with both the grace period and stranger mode off, nothing
    /// ever fires (only a refused `nodonuts stop` still does). Allowed,
    /// since setting the two one at a time passes through it, but worth a
    /// loud note.
    pub fn warning(&self) -> Option<&'static str> {
        if !self.grace_period && !self.stranger_mode {
            Some(
                "grace_period and stranger_mode are both off: your command will never \
                 run when you leave or when someone else sits down (only a refused \
                 `nodonuts stop` runs it). Turn one of them on.",
            )
        } else {
            None
        }
    }

    /// Turns `grace_seconds = os` into a number, using `os_lock`, which says
    /// what the OS waits before locking (`None` if it can't tell). If the OS
    /// timeout can't be read, the default is used. Returns a sentence about
    /// what was decided, or `None` if `grace_seconds` was already a number.
    pub fn resolve_grace(&mut self, os_lock: impl FnOnce() -> Option<u64>) -> Option<String> {
        if self.grace_seconds != GraceSeconds::Os {
            return None;
        }
        Some(match os_lock() {
            Some(seconds) if seconds > 0 => {
                self.grace_seconds = GraceSeconds::Seconds(seconds);
                format!("grace_seconds = os: the OS locks the screen after {seconds} s, using that")
            }
            _ => {
                self.grace_seconds = GraceSeconds::Seconds(DEFAULT_GRACE_SECONDS);
                format!(
                    "grace_seconds = os, but the OS lock timeout can't be read (or the OS \
                     never locks), using {DEFAULT_GRACE_SECONDS} s"
                )
            }
        })
    }

    /// Parses TOML text and validates it.
    pub fn from_toml(text: &str) -> Result<Config> {
        let config: Config = toml::from_str(text)?;
        config.validate()?;
        Ok(config)
    }

    /// Renders the settings as TOML text.
    pub fn to_toml(&self) -> Result<String> {
        Ok(toml::to_string(self)?)
    }
}

/// Loads the config file. A missing file means default settings.
///
/// The file must be private (see [`files::check_private`]) because bash runs
/// the command it contains.
pub fn load(path: &Path) -> Result<Config> {
    if !path.exists() {
        return Ok(Config::default());
    }
    files::check_private(path)?;
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("could not read {}", path.display()))?;
    Config::from_toml(&text).with_context(|| format!("invalid settings in {}", path.display()))
}

/// Saves the config file with owner-only permissions.
pub fn save(path: &Path, config: &Config) -> Result<()> {
    config.validate()?;
    files::write_private(path, config.to_toml()?.as_bytes())
}

fn parse_fraction(key: &str, value: &str) -> Result<f64> {
    value
        .trim()
        .parse()
        .with_context(|| format!("{key} must be a number between 0 and 1"))
}

fn parse_whole_number(key: &str, value: &str) -> Result<u64> {
    value
        .trim()
        .parse()
        .with_context(|| format!("{key} must be a whole number greater than 0"))
}

fn parse_on_off(key: &str, value: &str) -> Result<bool> {
    match value.trim() {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => bail!("{key} must be true or false"),
    }
}

fn unknown_key(key: &str) -> String {
    format!(
        "unknown setting '{key}'. Known settings: {}",
        KEYS.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid() {
        Config::default().validate().unwrap();
    }

    #[test]
    fn set_then_get_round_trips() {
        let mut config = Config::default();
        config.set("grace_seconds", "45").unwrap();
        config.set("grace_period", "false").unwrap();
        assert_eq!(config.get("grace_period").unwrap(), "false");
        config.set("grace_period", "true").unwrap();
        assert_eq!(config.get("grace_period").unwrap(), "true");
        config.set("command", "reboot").unwrap();
        config.set("threshold_owner", "0.5").unwrap();
        config.set("threshold_stranger", "0.3").unwrap();
        config.set("check_every_ms", "250").unwrap();
        config.set("stranger_mode", "true").unwrap();
        assert_eq!(config.get("stranger_mode").unwrap(), "true");
        config.set("stranger_mode", "false").unwrap();
        assert_eq!(config.get("stranger_mode").unwrap(), "false");
        config.set("stranger_checks", "1").unwrap();
        assert_eq!(config.get("stranger_checks").unwrap(), "1");
        config.set("stranger_checks", "10").unwrap();
        assert_eq!(config.get("stranger_checks").unwrap(), "10");
        config.set("logs_maxdays", "0").unwrap();
        config.set("logs_maxdays", "30").unwrap();
        assert_eq!(config.get("logs_maxdays").unwrap(), "30");
        config.set("alert_location", "off").unwrap();
        assert_eq!(config.get("alert_location").unwrap(), "off");
        let mine = std::env::temp_dir().join(format!("nodonuts-alerts-{}", std::process::id()));
        std::fs::create_dir_all(&mine).unwrap();
        config
            .set("alert_location", mine.to_str().unwrap())
            .unwrap();
        std::fs::remove_dir_all(&mine).unwrap();
        config.set("alert_location", "desktop").unwrap();
        config.set("pwd_ms", "1").unwrap();
        config.set("pwd_ms", "60000").unwrap();
        config.set("pwd_ms", "2500").unwrap();
        assert_eq!(config.get("pwd_ms").unwrap(), "2500");
        config.set("face_checks", "1").unwrap();
        config.set("face_checks", "10").unwrap();
        assert_eq!(config.get("face_checks").unwrap(), "10");
        config.set("command_once", "false").unwrap();
        assert_eq!(config.get("command_once").unwrap(), "false");
        config.set("face_checkout", "false").unwrap();
        assert_eq!(config.get("face_checkout").unwrap(), "false");
        config.set("autostart", "true").unwrap();
        assert_eq!(config.get("autostart").unwrap(), "true");
        assert_eq!(config.get("grace_seconds").unwrap(), "45");
        assert_eq!(config.get("command").unwrap(), "reboot");
        assert_eq!(config.get("threshold_owner").unwrap(), "0.5");
        assert_eq!(config.get("threshold_stranger").unwrap(), "0.3");
        assert_eq!(config.get("check_every_ms").unwrap(), "250");
    }

    #[test]
    fn camera_defaults_to_built_in_and_is_not_set_by_hand() {
        let mut config = Config::default();
        assert_eq!(config.get("camera").unwrap(), "built-in");
        // Only `nodonuts camera` chooses it.
        assert!(config.set("camera", "Logitech C920").is_err());
        assert_eq!(config.camera, "built-in");
        // A chosen camera round-trips through the file.
        config.camera = "Logitech C920".to_string();
        let text = config.to_toml().unwrap();
        assert_eq!(Config::from_toml(&text).unwrap().camera, "Logitech C920");
        // Older files without the key get the built-in camera.
        assert_eq!(Config::from_toml("").unwrap().camera, "built-in");
        assert!(Config::from_toml("camera = ''").is_err());
    }

    #[test]
    fn threshold_stranger_can_never_exceed_threshold_owner() {
        let mut config = Config::default(); // owner 0.36, stranger 0.2
        // Raising the stranger threshold above the owner's is refused...
        assert!(config.set("threshold_stranger", "0.5").is_err());
        // ...and so is lowering the owner's below the stranger's.
        assert!(config.set("threshold_owner", "0.1").is_err());
        assert_eq!(config, Config::default(), "nothing changed");
        // Equal is allowed: no unsure zone, like before the split.
        config.set("threshold_stranger", "0.36").unwrap();
        // A hand-edited file breaking the rule doesn't load.
        assert!(Config::from_toml("threshold_owner = 0.2\nthreshold_stranger = 0.3").is_err());
    }

    #[test]
    fn old_match_threshold_files_still_load() {
        let config = Config::from_toml("match_threshold = 0.4\ngrace_seconds = 10").unwrap();
        assert_eq!(config.threshold_owner, 0.4);
        assert_eq!(
            config.threshold_stranger, 0.2,
            "default for the new setting"
        );
        // Saved with the new name from now on.
        let text = config.to_toml().unwrap();
        assert!(text.contains("threshold_owner = 0.4"));
        assert!(!text.contains("match_threshold"));
        // But the old name isn't a `config set` key any more.
        assert!(Config::default().set("match_threshold", "0.4").is_err());
    }

    #[test]
    fn invalid_values_are_rejected_and_change_nothing() {
        let mut config = Config::default();
        for (key, bad) in [
            ("grace_period", "yes"),
            ("grace_period", "1"),
            ("grace_period", ""),
            ("grace_seconds", "0"),
            ("grace_seconds", "-5"),
            ("grace_seconds", "1.5"),
            ("grace_seconds", "soon"),
            ("threshold_owner", "1.1"),
            ("threshold_owner", "-0.1"),
            ("threshold_owner", "NaN"),
            ("threshold_stranger", "-0.1"),
            ("threshold_stranger", "NaN"),
            ("threshold_stranger", "0.9"),
            ("check_every_ms", "0"),
            ("command", "   "),
            ("stranger_mode", "yes"),
            ("stranger_mode", "1"),
            ("stranger_mode", ""),
            ("autostart", "on"),
            ("face_checkout", "yes"),
            ("command_once", "1"),
            ("face_checks", "0"),
            ("face_checks", "11"),
            ("face_checks", "2.5"),
            ("logs_maxdays", "-1"),
            ("logs_maxdays", "1.5"),
            ("logs_maxdays", "3651"),
            ("alert_location", ""),
            ("alert_location", "alerts"),
            ("alert_location", "/no/such/folder/for/nodonuts"),
            ("pwd_ms", "0"),
            ("pwd_ms", "60001"),
            ("pwd_ms", "1.5"),
            ("pwd_ms", "-1"),
            ("stranger_checks", "0"),
            ("stranger_checks", "-1"),
            ("stranger_checks", "1.5"),
            ("stranger_checks", "two"),
        ] {
            assert!(config.set(key, bad).is_err(), "{key}={bad} should fail");
        }
        assert_eq!(config, Config::default());
    }

    #[test]
    fn unknown_keys_are_rejected() {
        let mut config = Config::default();
        assert!(config.get("colour").is_err());
        assert!(config.set("colour", "blue").is_err());
        assert!(Config::from_toml("colour = 'blue'").is_err());
    }

    #[test]
    fn missing_keys_use_defaults() {
        let config = Config::from_toml("grace_seconds = 10").unwrap();
        assert_eq!(config.grace_seconds, GraceSeconds::Seconds(10));
        assert_eq!(config.command, Config::default().command);
        // A file without `grace_seconds` follows the OS timeout.
        let config_without = Config::from_toml("command = 'true'").unwrap();
        assert_eq!(config_without.grace_seconds, GraceSeconds::Os);
        // Stranger mode is the tool's main job: on unless switched off.
        assert!(config.stranger_mode);
        assert!(!config.autostart);
        // ...but face_checkout is on, including for older configs.
        assert!(config.face_checkout);
        assert!(config.command_once);
        assert_eq!(config.stranger_checks, 2);
        assert_eq!(config.pwd_ms, 3000);
        // Older files keep behaving as before: the grace period is on.
        assert!(config.grace_period);
        assert_eq!(config.alert_location, "desktop");
        // A hand-edited file with a relative path doesn't load.
        assert!(Config::from_toml("alert_location = 'alerts'").is_err());
        // A full path that doesn't exist (yet) does: the alert falls back.
        let config = Config::from_toml("alert_location = '/no/such/folder'").unwrap();
        assert_eq!(config.alert_location, "/no/such/folder");
    }

    #[test]
    fn both_detectors_off_is_allowed_but_warned_about() {
        let mut config = Config::default();
        assert!(config.warning().is_none(), "defaults: both on");
        // Strangers only: grace off, stranger mode on (the default) is fine.
        config.set("grace_period", "false").unwrap();
        assert!(config.warning().is_none());
        // Stranger mode off as well leaves nothing that can fire.
        config.set("stranger_mode", "false").unwrap();
        assert!(config.warning().is_some());
        // Grace period back on: valid again ("away only", the old default).
        config.set("grace_period", "true").unwrap();
        assert!(config.warning().is_none());
    }

    #[test]
    fn grace_seconds_can_be_a_number_or_os() {
        let mut config = Config::default();
        // `os` is the default.
        assert_eq!(config.grace_seconds, GraceSeconds::Os);
        assert_eq!(config.get("grace_seconds").unwrap(), "os");
        config.set("grace_seconds", "30").unwrap();
        assert_eq!(config.get("grace_seconds").unwrap(), "30");
        config.set("grace_seconds", "os").unwrap();
        assert_eq!(config.grace_seconds, GraceSeconds::Os);
        config.set("grace_seconds", " 45 ").unwrap();
        assert_eq!(config.grace_seconds, GraceSeconds::Seconds(45));
        // Only the exact word, and a failed set changes nothing.
        for bad in ["OS", "Os", "operating-system", "0", "-5", "1.5", ""] {
            assert!(config.set("grace_seconds", bad).is_err(), "{bad:?}");
        }
        assert_eq!(config.grace_seconds, GraceSeconds::Seconds(45));
    }

    #[test]
    fn grace_seconds_reads_and_writes_both_forms_in_the_file() {
        // Old files have a plain number and keep loading.
        let config = Config::from_toml("grace_seconds = 12").unwrap();
        assert_eq!(config.grace_seconds, GraceSeconds::Seconds(12));
        assert!(config.to_toml().unwrap().contains("grace_seconds = 12\n"));
        // `os` is written as text, and reads back.
        let config = Config::from_toml("grace_seconds = \"os\"").unwrap();
        assert_eq!(config.grace_seconds, GraceSeconds::Os);
        let text = config.to_toml().unwrap();
        assert!(text.contains("grace_seconds = \"os\""), "{text}");
        assert_eq!(Config::from_toml(&text).unwrap(), config);
        // Anything else doesn't load.
        for bad in ["\"soon\"", "\"OS\"", "0", "-3", "1.5", "true"] {
            let text = format!("grace_seconds = {bad}");
            assert!(Config::from_toml(&text).is_err(), "{text}");
        }
    }

    #[test]
    fn os_grace_becomes_the_os_timeout() {
        let mut config = Config {
            grace_seconds: GraceSeconds::Os,
            ..Config::default()
        };
        let decision = config.resolve_grace(|| Some(900)).unwrap();
        assert_eq!(config.grace_seconds, GraceSeconds::Seconds(900));
        assert!(decision.contains("900 s"), "{decision}");
    }

    #[test]
    fn os_grace_falls_back_when_the_os_timeout_is_unknown() {
        for unreadable in [None, Some(0)] {
            let mut config = Config {
                grace_seconds: GraceSeconds::Os,
                ..Config::default()
            };
            let decision = config.resolve_grace(|| unreadable).unwrap();
            assert_eq!(config.grace_seconds, GraceSeconds::Seconds(30));
            assert!(decision.contains("30 s"), "{decision}");
        }
    }

    #[test]
    fn a_number_is_never_replaced_by_the_os_timeout() {
        let mut config = Config {
            grace_seconds: GraceSeconds::Seconds(30),
            ..Config::default()
        };
        let decision = config.resolve_grace(|| panic!("must not ask the OS"));
        assert_eq!(decision, None);
        assert_eq!(config.grace_seconds, GraceSeconds::Seconds(30));
        // Until it's resolved, `os` counts as the default, never as 0.
        assert_eq!(GraceSeconds::Os.seconds(), 30);
    }

    #[test]
    fn old_aggressive_names_still_load() {
        let config =
            Config::from_toml("aggressive_mode = false\naggressive_checks = 4\ngrace_seconds = 10")
                .unwrap();
        assert!(!config.stranger_mode);
        assert_eq!(config.stranger_checks, 4);
        // Saved with the new names from now on.
        let text = config.to_toml().unwrap();
        assert!(text.contains("stranger_mode = false"));
        assert!(text.contains("stranger_checks = 4"));
        assert!(!text.contains("aggressive"));
        // But the old names aren't `config set` keys any more.
        let mut config = Config::default();
        assert!(config.set("aggressive_mode", "true").is_err());
        assert!(config.set("aggressive_checks", "3").is_err());
        assert!(config.get("aggressive_mode").is_err());
    }

    #[test]
    fn settings_are_shown_in_alphabetical_order() {
        let text = Config::default().to_toml().unwrap();
        // The key at the start of each `key = value` line, in print order.
        let shown: Vec<&str> = text
            .lines()
            .filter_map(|line| line.split_once(" = "))
            .map(|(key, _value)| key)
            .collect();
        let mut sorted = shown.clone();
        sorted.sort();
        assert_eq!(shown, sorted);
        // Every setting is shown, and `KEYS` lists the same ones in order.
        assert_eq!(shown, KEYS);
    }

    #[test]
    fn toml_round_trips() {
        let mut config = Config::default();
        config.set("command", "echo \"bye\"").unwrap();
        let text = config.to_toml().unwrap();
        assert_eq!(Config::from_toml(&text).unwrap(), config);
    }

    #[test]
    fn save_load_and_reset() {
        let path = std::env::temp_dir()
            .join(format!("nodonuts-config-test-{}", std::process::id()))
            .join("config.toml");
        // Missing file -> defaults.
        assert_eq!(load(&path).unwrap(), Config::default());

        let mut config = Config::default();
        config.set("grace_seconds", "99").unwrap();
        save(&path, &config).unwrap();
        assert_eq!(
            load(&path).unwrap().grace_seconds,
            GraceSeconds::Seconds(99)
        );

        // Reset = save the defaults.
        save(&path, &Config::default()).unwrap();
        assert_eq!(load(&path).unwrap(), Config::default());
        std::fs::remove_file(&path).unwrap();
    }
}
