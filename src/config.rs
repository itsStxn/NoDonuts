//! Settings: load, save, validate, get, set and reset `config.toml`.

use crate::files;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// The names accepted by `config get` and `config set`.
pub const KEYS: [&str; 7] = [
    "grace_seconds",
    "command",
    "match_threshold",
    "check_every_ms",
    "aggressive_mode",
    "aggressive_checks",
    "autostart",
];

/// All settings. `Serialize`/`Deserialize` let `serde` convert this struct
/// to and from TOML text for us.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
// `default`: keys missing from the file get their default value.
// `deny_unknown_fields`: a typo like `grace_second` is an error, not ignored.
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// How long the owner may be away before the command runs.
    pub grace_seconds: u64,
    /// The bash command to run.
    pub command: String,
    /// Minimum cosine similarity (0 to 1) for a face to count as the owner.
    pub match_threshold: f64,
    /// How often to look at the camera, in milliseconds.
    pub check_every_ms: u64,
    /// If true, run the command immediately (no grace period) when someone
    /// other than the owner is in front of the camera and the owner is not.
    pub aggressive_mode: bool,
    /// Aggressive mode fires only after this many checks in a row see a
    /// stranger and not the owner. One check every `check_every_ms`, so the
    /// wait is `aggressive_checks x check_every_ms`. At least 1.
    pub aggressive_checks: u64,
    /// If true, the watcher starts by itself when the owner logs in.
    pub autostart: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            grace_seconds: 30,
            command: "echo 'The owner is gone!'".to_string(),
            // SFace's recommended threshold for cosine similarity.
            match_threshold: 0.36,
            check_every_ms: 500,
            aggressive_mode: false,
            // 2 = one misread frame of the owner is not enough to fire.
            aggressive_checks: 2,
            autostart: false,
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
        if !(0.0..=1.0).contains(&self.match_threshold) {
            bail!("match_threshold must be between 0 and 1");
        }
        if self.check_every_ms == 0 {
            bail!("check_every_ms must be a whole number greater than 0");
        }
        if self.aggressive_checks == 0 {
            bail!("aggressive_checks must be a whole number of 1 or more");
        }
        Ok(())
    }

    /// Returns one setting as text.
    pub fn get(&self, key: &str) -> Result<String> {
        let value = match key {
            "grace_seconds" => self.grace_seconds.to_string(),
            "command" => self.command.clone(),
            "match_threshold" => self.match_threshold.to_string(),
            "check_every_ms" => self.check_every_ms.to_string(),
            "aggressive_mode" => self.aggressive_mode.to_string(),
            "aggressive_checks" => self.aggressive_checks.to_string(),
            "autostart" => self.autostart.to_string(),
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
            "match_threshold" => {
                updated.match_threshold = value
                    .trim()
                    .parse()
                    .with_context(|| format!("{key} must be a number between 0 and 1"))?
            }
            "check_every_ms" => updated.check_every_ms = parse_whole_number(key, value)?,
            "aggressive_mode" => updated.aggressive_mode = parse_on_off(key, value)?,
            "aggressive_checks" => {
                updated.aggressive_checks = value
                    .trim()
                    .parse()
                    .with_context(|| format!("{key} must be a whole number of 1 or more"))?
            }
            "autostart" => updated.autostart = parse_on_off(key, value)?,
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
        config.set("match_threshold", "0.5").unwrap();
        config.set("check_every_ms", "250").unwrap();
        config.set("aggressive_mode", "true").unwrap();
        assert_eq!(config.get("aggressive_mode").unwrap(), "true");
        config.set("aggressive_mode", "false").unwrap();
        assert_eq!(config.get("aggressive_mode").unwrap(), "false");
        config.set("aggressive_checks", "1").unwrap();
        assert_eq!(config.get("aggressive_checks").unwrap(), "1");
        config.set("aggressive_checks", "10").unwrap();
        assert_eq!(config.get("aggressive_checks").unwrap(), "10");
        config.set("autostart", "true").unwrap();
        assert_eq!(config.get("autostart").unwrap(), "true");
        assert_eq!(config.get("grace_seconds").unwrap(), "45");
        assert_eq!(config.get("command").unwrap(), "reboot");
        assert_eq!(config.get("match_threshold").unwrap(), "0.5");
        assert_eq!(config.get("check_every_ms").unwrap(), "250");
    }

    #[test]
    fn invalid_values_are_rejected_and_change_nothing() {
        let mut config = Config::default();
        for (key, bad) in [
            ("grace_seconds", "0"),
            ("grace_seconds", "-5"),
            ("grace_seconds", "1.5"),
            ("grace_seconds", "soon"),
            ("match_threshold", "1.1"),
            ("match_threshold", "-0.1"),
            ("match_threshold", "NaN"),
            ("check_every_ms", "0"),
            ("command", "   "),
            ("aggressive_mode", "yes"),
            ("aggressive_mode", "1"),
            ("aggressive_mode", ""),
            ("autostart", "on"),
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
        assert_eq!(config.aggressive_checks, 2);
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
