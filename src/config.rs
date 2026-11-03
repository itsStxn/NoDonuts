//! Settings: load, save, validate, get, set and reset `config.toml`.

use crate::files;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// The names accepted by `config get` and `config set`, alphabetically.
pub const KEYS: [&str; 11] = [
    "aggressive_checks",
    "aggressive_mode",
    "autostart",
    "check_every_ms",
    "command",
    "face_checkout",
    "face_checks",
    "grace_seconds",
    "logs_maxdays",
    "threshold_owner",
    "threshold_stranger",
];

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
    /// Aggressive mode fires only after this many checks in a row see a
    /// stranger and not the owner. One check every `check_every_ms`, so the
    /// wait is `aggressive_checks x check_every_ms`. At least 1.
    pub aggressive_checks: u64,
    /// If true, run the command immediately (no grace period) when someone
    /// other than the owner is in front of the camera and the owner is not.
    pub aggressive_mode: bool,
    /// If true, the watcher starts by itself when the owner logs in.
    pub autostart: bool,
    /// How often to look at the camera, in milliseconds.
    pub check_every_ms: u64,
    /// The bash command to run.
    pub command: String,
    /// If true, stopping the watcher (`nodonuts stop`, `kill`, `pkill`)
    /// needs the owner's face. If the owner isn't seen, the command runs
    /// before the watcher exits.
    pub face_checkout: bool,
    /// How many face checkout attempts in a row (about 3 s each) must fail
    /// during `nodonuts stop` before it asks for the fallback password.
    /// 1 to 10.
    pub face_checks: u64,
    /// How long the owner may be away before the command runs.
    pub grace_seconds: u64,
    /// Empty the watcher's log once it's this many days old. 0 = never.
    pub logs_maxdays: u64,
    /// Minimum similarity (0 to 1) for a face to count as the owner.
    /// `alias`: config files saved before the rename used `match_threshold`;
    /// they still load, and are saved with the new name.
    #[serde(alias = "match_threshold")]
    pub threshold_owner: f64,
    /// A face below this similarity (0 to 1) counts as a stranger, for
    /// aggressive mode. Between this and `threshold_owner` a face is
    /// "unsure": not the owner, but not a stranger either. At most
    /// `threshold_owner`; setting them equal removes the unsure zone.
    pub threshold_stranger: f64,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            // 2 = one misread frame of the owner is not enough to fire.
            aggressive_checks: 2,
            aggressive_mode: false,
            autostart: false,
            check_every_ms: 500,
            command: "echo 'The owner is gone!'".to_string(),
            // On by default: without it, anyone at the laptop can simply
            // run `nodonuts stop` during the grace period.
            face_checkout: true,
            // 3 attempts, about 9 s: room for a bad angle or bad light.
            face_checks: 3,
            grace_seconds: 30,
            logs_maxdays: 7,
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
        if self.grace_seconds == 0 {
            bail!("grace_seconds must be a whole number greater than 0");
        }
        if self.command.trim().is_empty() {
            bail!("command must not be empty");
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
        if self.aggressive_checks == 0 {
            bail!("aggressive_checks must be a whole number of 1 or more");
        }
        // Capped so `nodonuts stop` can't take minutes (each check is ~3 s).
        if !(1..=10).contains(&self.face_checks) {
            bail!("face_checks must be a whole number from 1 to 10");
        }
        if self.logs_maxdays > 3650 {
            bail!("logs_maxdays must be a whole number from 0 (never empty) to 3650");
        }
        Ok(())
    }

    /// Returns one setting as text.
    pub fn get(&self, key: &str) -> Result<String> {
        let value = match key {
            "grace_seconds" => self.grace_seconds.to_string(),
            "command" => self.command.clone(),
            "threshold_owner" => self.threshold_owner.to_string(),
            "threshold_stranger" => self.threshold_stranger.to_string(),
            "check_every_ms" => self.check_every_ms.to_string(),
            "aggressive_mode" => self.aggressive_mode.to_string(),
            "aggressive_checks" => self.aggressive_checks.to_string(),
            "autostart" => self.autostart.to_string(),
            "face_checkout" => self.face_checkout.to_string(),
            "face_checks" => self.face_checks.to_string(),
            "logs_maxdays" => self.logs_maxdays.to_string(),
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
            "grace_seconds" => updated.grace_seconds = parse_whole_number(key, value)?,
            "command" => updated.command = value.to_string(),
            "threshold_owner" => updated.threshold_owner = parse_fraction(key, value)?,
            "threshold_stranger" => updated.threshold_stranger = parse_fraction(key, value)?,
            "check_every_ms" => updated.check_every_ms = parse_whole_number(key, value)?,
            "aggressive_mode" => updated.aggressive_mode = parse_on_off(key, value)?,
            "aggressive_checks" => {
                updated.aggressive_checks = value
                    .trim()
                    .parse()
                    .with_context(|| format!("{key} must be a whole number of 1 or more"))?
            }
            "autostart" => updated.autostart = parse_on_off(key, value)?,
            "face_checkout" => updated.face_checkout = parse_on_off(key, value)?,
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
        config.set("command", "reboot").unwrap();
        config.set("threshold_owner", "0.5").unwrap();
        config.set("threshold_stranger", "0.3").unwrap();
        config.set("check_every_ms", "250").unwrap();
        config.set("aggressive_mode", "true").unwrap();
        assert_eq!(config.get("aggressive_mode").unwrap(), "true");
        config.set("aggressive_mode", "false").unwrap();
        assert_eq!(config.get("aggressive_mode").unwrap(), "false");
        config.set("aggressive_checks", "1").unwrap();
        assert_eq!(config.get("aggressive_checks").unwrap(), "1");
        config.set("aggressive_checks", "10").unwrap();
        assert_eq!(config.get("aggressive_checks").unwrap(), "10");
        config.set("logs_maxdays", "0").unwrap();
        config.set("logs_maxdays", "30").unwrap();
        assert_eq!(config.get("logs_maxdays").unwrap(), "30");
        config.set("face_checks", "1").unwrap();
        config.set("face_checks", "10").unwrap();
        assert_eq!(config.get("face_checks").unwrap(), "10");
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
            ("aggressive_mode", "yes"),
            ("aggressive_mode", "1"),
            ("aggressive_mode", ""),
            ("autostart", "on"),
            ("face_checkout", "yes"),
            ("face_checks", "0"),
            ("face_checks", "11"),
            ("face_checks", "2.5"),
            ("logs_maxdays", "-1"),
            ("logs_maxdays", "1.5"),
            ("logs_maxdays", "3651"),
            ("aggressive_checks", "0"),
            ("aggressive_checks", "-1"),
            ("aggressive_checks", "1.5"),
            ("aggressive_checks", "two"),
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
        assert_eq!(config.grace_seconds, 10);
        assert_eq!(config.command, Config::default().command);
        // Configs written before aggressive_mode existed keep it off.
        assert!(!config.aggressive_mode);
        assert!(!config.autostart);
        // ...but face_checkout is on, including for older configs.
        assert!(config.face_checkout);
        assert_eq!(config.aggressive_checks, 2);
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
        assert_eq!(load(&path).unwrap().grace_seconds, 99);

        // Reset = save the defaults.
        save(&path, &Config::default()).unwrap();
        assert_eq!(load(&path).unwrap(), Config::default());
        std::fs::remove_file(&path).unwrap();
    }
}
