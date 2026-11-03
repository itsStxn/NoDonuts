//! Enrollment: learn the owner's face, and save/load what was learned.
//!
//! We capture several frames, take the biggest face in each (the person
//! sitting closest), and average their embeddings. Averaging smooths out
//! one-off differences like a blink or a slight head turn.
//!
//! The result is stored as TOML holding one array of 128 numbers. Never an
//! image.

use crate::camera::{self, FrameSource, Webcam};
use crate::config;
use crate::daemon;
use crate::face::{self, Embedding, FaceFinder, OnnxFaceFinder};
use crate::files::{self, Paths};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::thread;
use std::time::Duration;

/// How many good frames make up the owner's face.
pub const FRAMES_NEEDED: usize = 10;
/// Give up after this many frames (about 30 seconds).
const MAX_ATTEMPTS: usize = 100;
/// Pause between frames, so the frames are not all identical.
const PAUSE_BETWEEN_FRAMES: Duration = Duration::from_millis(300);

/// `nodonuts enroll`: capture the owner's face and save it.
pub fn run(paths: &Paths) -> Result<()> {
    if daemon::running_pid(paths)?.is_some() {
        bail!("the watcher is using the camera. Run `nodonuts stop` first");
    }
    camera::request_permission()?;
    println!("Loading face models...");
    let finder = OnnxFaceFinder::load()?;
    // Enroll with the camera the watcher will use.
    let config = config::load(&paths.config_file)?;
    let mut webcam = Webcam::new(&config.camera);
    println!("Look at the camera. Move your head slightly between shots.");
    let embedding = capture_owner(&mut webcam, &finder, PAUSE_BETWEEN_FRAMES)?;
    save(&paths.face_file, &embedding)?;
    println!("Done. Your face is saved in {}", paths.face_file.display());
    println!("Run `nodonuts start` to start watching.");
    Ok(())
}

/// Captures `FRAMES_NEEDED` faces and returns their average embedding.
///
/// `&mut dyn FrameSource` means "any type that implements `FrameSource`",
/// so tests can pass a fake camera here.
pub fn capture_owner(
    camera: &mut dyn FrameSource,
    finder: &dyn FaceFinder,
    pause: Duration,
) -> Result<Embedding> {
    let mut embeddings: Vec<Embedding> = Vec::new();
    for _ in 0..MAX_ATTEMPTS {
        if embeddings.len() == FRAMES_NEEDED {
            break;
        }
        thread::sleep(pause);
        let frame = match camera.grab() {
            Ok(frame) => frame,
            Err(error) => {
                println!("  camera problem, retrying: {error:#}");
                continue;
            }
        };
        let faces = finder.find_faces(&frame)?;
        // `max_by` picks the largest face; `total_cmp` compares floats.
        match faces.into_iter().max_by(|a, b| a.area.total_cmp(&b.area)) {
            Some(biggest) => {
                embeddings.push(biggest.embedding);
                println!("  captured {}/{FRAMES_NEEDED}", embeddings.len());
            }
            None => println!("  no face found, keep looking at the camera"),
        }
    }
    if embeddings.len() < FRAMES_NEEDED {
        bail!(
            "only found your face in {}/{FRAMES_NEEDED} frames. \
             Check the lighting and try again",
            embeddings.len()
        );
    }
    Ok(face::average(&embeddings))
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredFace {
    embedding: Embedding,
}

/// Saves the owner's embedding with owner-only permissions.
pub fn save(path: &Path, embedding: &Embedding) -> Result<()> {
    if embedding.len() != face::EMBEDDING_LEN {
        bail!("refusing to save a malformed face embedding");
    }
    let stored = StoredFace {
        embedding: embedding.clone(),
    };
    files::write_private(path, toml::to_string(&stored)?.as_bytes())
}

/// Loads the owner's embedding, with a clear error if none is enrolled.
pub fn load(path: &Path) -> Result<Embedding> {
    if !path.exists() {
        bail!("no face enrolled yet. Run `nodonuts enroll` first");
    }
    files::check_private(path)?;
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("could not read {}", path.display()))?;
    let stored: StoredFace =
        toml::from_str(&text).with_context(|| format!("invalid face file {}", path.display()))?;
    if stored.embedding.len() != face::EMBEDDING_LEN {
        bail!("the enrolled face is malformed. Run `nodonuts enroll` again");
    }
    Ok(stored.embedding)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::face::Face;
    use image::RgbImage;

    /// A camera that fails every other frame.
    struct FlakyCamera {
        calls: usize,
    }

    impl FrameSource for FlakyCamera {
        fn grab(&mut self) -> Result<RgbImage> {
            self.calls += 1;
            if self.calls.is_multiple_of(2) {
                bail!("camera busy");
            }
            Ok(RgbImage::new(4, 4))
        }
    }

    /// Always sees two faces: a small stranger and a big owner.
    struct TwoFaces;

    impl FaceFinder for TwoFaces {
        fn find_faces(&self, _image: &RgbImage) -> Result<Vec<Face>> {
            let mut stranger = vec![0.0; face::EMBEDDING_LEN];
            stranger[1] = 1.0;
            let mut owner = vec![0.0; face::EMBEDDING_LEN];
            owner[0] = 1.0;
            Ok(vec![
                Face {
                    area: 10.0,
                    embedding: stranger,
                },
                Face {
                    area: 500.0,
                    embedding: owner,
                },
            ])
        }
    }

    /// Never sees anyone.
    struct NoFaces;

    impl FaceFinder for NoFaces {
        fn find_faces(&self, _image: &RgbImage) -> Result<Vec<Face>> {
            Ok(Vec::new())
        }
    }

    #[test]
    fn enrolls_the_biggest_face_despite_camera_errors() {
        let mut camera = FlakyCamera { calls: 0 };
        let embedding = capture_owner(&mut camera, &TwoFaces, Duration::ZERO).unwrap();
        assert!((embedding[0] - 1.0).abs() < 1e-6);
        assert_eq!(camera.calls, FRAMES_NEEDED * 2 - 1);
    }

    #[test]
    fn fails_when_no_face_is_found() {
        let mut camera = FlakyCamera { calls: 0 };
        assert!(capture_owner(&mut camera, &NoFaces, Duration::ZERO).is_err());
    }

    #[test]
    fn save_then_load_round_trips() {
        let path = std::env::temp_dir()
            .join(format!("nodonuts-owner-test-{}", std::process::id()))
            .join("owner_face.toml");
        assert!(load(&path).is_err()); // nothing enrolled yet
        let embedding = face::normalize(vec![0.5; face::EMBEDDING_LEN]);
        save(&path, &embedding).unwrap();
        let loaded = load(&path).unwrap();
        assert!((face::cosine_similarity(&loaded, &embedding) - 1.0).abs() < 1e-6);
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn malformed_embeddings_are_rejected() {
        let path = std::env::temp_dir().join("nodonuts-never-written.toml");
        assert!(save(&path, &vec![0.1, 0.2]).is_err());
    }
}
