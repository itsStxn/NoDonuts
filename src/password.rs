//! The fallback checkout password: used to stop the watcher when
//! `face_checkout` can't recognize the owner.
//!
//! Only a hash is stored, never the password. Argon2 is deliberately slow
//! (tens of milliseconds per try), so someone who copies the file can't
//! quickly try millions of guesses, which matters because the password can
//! be anything, including something short.

use crate::files;
use anyhow::{Context, Result, anyhow, bail};
use argon2::Argon2;
use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use serde::{Deserialize, Serialize};
use std::io::ErrorKind;
use std::path::Path;

/// What the file holds: one Argon2 hash string, which includes its own
/// random salt and settings.
#[derive(Debug, Serialize, Deserialize)]
struct StoredPassword {
    hash: String,
}

/// Hashes `password` with a fresh random salt.
pub fn hash(password: &str) -> Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    let hashed = Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        // The library's error type doesn't plug into `?` directly.
        .map_err(|error| anyhow!("could not hash the password: {error}"))?;
    Ok(hashed.to_string())
}

/// True if `password` matches `hash`. A malformed hash never matches.
pub fn verify(password: &str, hash: &str) -> bool {
    match PasswordHash::new(hash) {
        Ok(parsed) => Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok(),
        Err(_) => false,
    }
}

/// Hashes `password` and saves it, readable by the owner only.
pub fn save(path: &Path, password: &str) -> Result<()> {
    let stored = StoredPassword {
        hash: hash(password)?,
    };
    files::write_private(path, toml::to_string(&stored)?.as_bytes())
}

/// Loads the stored hash. `Ok(None)` means no password is set.
pub fn load(path: &Path) -> Result<Option<String>> {
    if !path.exists() {
        return Ok(None);
    }
    files::check_private(path)?;
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("could not read {}", path.display()))?;
    let stored: StoredPassword = toml::from_str(&text)
        .with_context(|| format!("invalid fallback password file {}", path.display()))?;
    if PasswordHash::new(&stored.hash).is_err() {
        bail!(
            "the fallback password file {} is damaged. Set a new password with `nodonuts password`",
            path.display()
        );
    }
    Ok(Some(stored.hash))
}

/// Deletes the stored password. Returns false if none was set.
pub fn remove(path: &Path) -> Result<bool> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).with_context(|| format!("could not remove {}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_and_verify() {
        let hashed = hash("donuts").unwrap();
        assert!(hashed.starts_with("$argon2"), "{hashed}");
        assert!(!hashed.contains("donuts"), "the password is never stored");
        assert!(verify("donuts", &hashed));
        assert!(!verify("Donuts", &hashed));
        assert!(!verify("", &hashed));
    }

    #[test]
    fn same_password_gives_different_hashes() {
        // Random salts: two people (or two settings) with the same password
        // don't get the same hash.
        assert_ne!(hash("x").unwrap(), hash("x").unwrap());
    }

    #[test]
    fn malformed_hashes_never_match() {
        assert!(!verify("anything", "not a hash"));
        assert!(!verify("", ""));
    }

    #[test]
    fn save_load_remove() {
        let path = std::env::temp_dir()
            .join(format!("nodonuts-password-test-{}", std::process::id()))
            .join("fallback_checkout.toml");
        assert_eq!(load(&path).unwrap(), None);
        assert!(!remove(&path).unwrap());

        save(&path, "s3cret with spaces").unwrap();
        files::check_private(&path).unwrap();
        let stored = load(&path).unwrap().unwrap();
        assert!(verify("s3cret with spaces", &stored));

        std::fs::write(&path, "hash = \"garbage\"\n").unwrap();
        assert!(load(&path).is_err(), "a damaged file is an error");

        assert!(remove(&path).unwrap());
        assert_eq!(load(&path).unwrap(), None);
    }
}
