//! Enrollment: learn the owner's face, and save/load what was learned.
//!
//! The owner is guided through a few poses (straight, turned left, turned
//! right, looking down), with a small variation for each shot. Each pose's
//! shots are averaged into one embedding, and one embedding per pose is
//! stored: a face then counts as the owner if it matches any pose. Averaging
//! everything into one would blur the turned poses into the straight one,
//! and they'd barely help.
//!
//! The result is stored as TOML holding a few arrays of 128 numbers. Never
//! an image.

use crate::camera::{self, FrameSource, Webcam};
use crate::config;
use crate::daemon;
use crate::face::{self, Embedding, Face, FaceFinder, OnnxFaceFinder};
use crate::files::{self, Paths};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, IsTerminal, Write};
use std::path::Path;
use std::thread;
use std::time::Duration;

/// One pose to enroll.
#[derive(Debug)]
pub struct Pose {
    /// What to do, shown before the pose.
    pub instruction: &'static str,
    /// A small variation for each shot of this pose (one shot each).
    pub shots: &'static [&'static str],
    /// What to say when no face is found in this pose.
    pub if_no_face: &'static str,
}

/// The poses, in order: 10 shots in total.
pub const POSES: [Pose; 4] = [
    Pose {
        instruction: "Look straight at the camera, face centered, at your normal \
                      working distance.",
        shots: &[
            "Neutral face.",
            "Smile.",
            "Lean a little closer.",
            "Lean back a little.",
        ],
        if_no_face: "No face found: face the camera, and avoid strong light behind you.",
    },
    Pose {
        instruction: "Turn your head a little to YOUR LEFT, like glancing at a second \
                      screen. Keep both eyes visible to the camera.",
        shots: &["About 20 degrees.", "A bit further, about 30 degrees."],
        if_no_face: "No face found: that's too far. Turn back a little toward the camera.",
    },
    Pose {
        instruction: "Now turn your head a little to YOUR RIGHT. Keep both eyes visible \
                      to the camera.",
        shots: &["About 20 degrees.", "A bit further, about 30 degrees."],
        if_no_face: "No face found: that's too far. Turn back a little toward the camera.",
    },
    Pose {
        instruction: "Tilt your head down as if reading something on your keyboard.",
        shots: &[
            "Look at the keyboard.",
            "Look at the bottom edge of your screen.",
        ],
        if_no_face: "No face found: lift your chin a little.",
    },
];

/// Tries per shot before giving up on the whole enrollment.
const TRIES_PER_SHOT: usize = 10;
/// A face smaller than this share of the frame is too far for a good shot.
const MIN_FACE_SHARE: f32 = 0.02;
/// Time to settle into the next shot's variation.
const SETTLE_TIME: Duration = Duration::from_millis(1500);
/// Pause before retrying a shot.
const RETRY_PAUSE: Duration = Duration::from_millis(700);

/// The total number of shots across all poses.
pub fn total_shots() -> usize {
    POSES.iter().map(|pose| pose.shots.len()).sum()
}

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
    println!(
        "You'll be guided through {} poses ({} shots). Sit as you normally do at your desk.",
        POSES.len(),
        total_shots()
    );
    let interactive = std::io::stdin().is_terminal();
    let mut wait_until_ready = || -> Result<()> {
        if interactive {
            print!("  Press Enter when ready.");
            std::io::stdout().flush()?;
            let mut line = String::new();
            std::io::stdin().lock().read_line(&mut line)?;
        } else {
            thread::sleep(Duration::from_secs(3));
        }
        Ok(())
    };
    let timing = Timing {
        settle: SETTLE_TIME,
        retry: RETRY_PAUSE,
    };
    let poses = capture_poses(
        &mut webcam,
        &finder,
        &mut wait_until_ready,
        &timing,
        config.threshold_stranger,
    )?;
    save(&paths.face_file, &poses)?;
    println!("Done. Your face is saved in {}", paths.face_file.display());
    println!("Run `nodonuts start` to start watching.");
    Ok(())
}

/// Pauses during enrollment (zero in tests).
#[derive(Debug, Clone, Copy)]
pub struct Timing {
    /// Before each shot, to settle into its variation.
    pub settle: Duration,
    /// Before retrying a shot that didn't work.
    pub retry: Duration,
}

/// Guides the owner through [`POSES`] and returns one embedding per pose.
///
/// `wait_until_ready` is called before each pose (Enter in a terminal).
/// `threshold_stranger` guards against enrolling someone else: after the
/// first shot, a face that scores below it against the shots so far is
/// rejected (someone walked into the picture).
///
/// `&mut dyn FrameSource` means "any type that implements `FrameSource`",
/// so tests can pass a fake camera here.
pub fn capture_poses(
    camera: &mut dyn FrameSource,
    finder: &dyn FaceFinder,
    wait_until_ready: &mut dyn FnMut() -> Result<()>,
    timing: &Timing,
    threshold_stranger: f64,
) -> Result<Vec<Embedding>> {
    let total = total_shots();
    let mut shot_number = 0;
    // Every shot so far, to check new shots are the same person.
    let mut all_shots: Vec<Embedding> = Vec::new();
    let mut poses: Vec<Embedding> = Vec::new();

    for (pose_number, pose) in POSES.iter().enumerate() {
        println!();
        println!(
            "Pose {}/{}: {}",
            pose_number + 1,
            POSES.len(),
            pose.instruction
        );
        wait_until_ready()?;
        let mut pose_shots: Vec<Embedding> = Vec::new();
        for variation in pose.shots {
            shot_number += 1;
            println!("  Shot {shot_number}/{total}: {variation}");
            thread::sleep(timing.settle);
            let embedding =
                take_shot(camera, finder, pose, &all_shots, timing, threshold_stranger)?;
            println!("    captured");
            all_shots.push(embedding.clone());
            pose_shots.push(embedding);
        }
        poses.push(face::average(&pose_shots));
    }
    Ok(poses)
}

/// One shot: retries until a usable face of the same person is captured.
fn take_shot(
    camera: &mut dyn FrameSource,
    finder: &dyn FaceFinder,
    pose: &Pose,
    shots_so_far: &[Embedding],
    timing: &Timing,
    threshold_stranger: f64,
) -> Result<Embedding> {
    for attempt in 0..TRIES_PER_SHOT {
        if attempt > 0 {
            thread::sleep(timing.retry);
        }
        let frame = match camera.grab() {
            Ok(frame) => frame,
            Err(error) => {
                println!("    camera problem, retrying: {error:#}");
                continue;
            }
        };
        let faces = finder.find_faces(&frame)?;
        // `max_by` picks the largest face (the person sitting closest);
        // `total_cmp` compares floats.
        let Some(biggest) = faces.into_iter().max_by(|a, b| a.area.total_cmp(&b.area)) else {
            println!("    {}", pose.if_no_face);
            continue;
        };
        let frame_area = (frame.width() * frame.height()) as f32;
        if biggest.area < frame_area * MIN_FACE_SHARE {
            println!("    Your face is quite small: move a little closer to the camera.");
            continue;
        }
        if !same_person(&biggest, shots_so_far, threshold_stranger) {
            println!(
                "    That doesn't look like the same person as the first shots. \
                 Make sure only you are in front of the camera."
            );
            continue;
        }
        return Ok(biggest.embedding);
    }
    bail!(
        "couldn't get a usable shot after {TRIES_PER_SHOT} tries. \
         Check the lighting and the camera (`nodonuts camera`), then try again"
    )
}

/// True if `face` looks like the person in `shots_so_far` (always true for
/// the first shot).
fn same_person(face: &Face, shots_so_far: &[Embedding], threshold_stranger: f64) -> bool {
    if shots_so_far.is_empty() {
        return true;
    }
    f64::from(face::best_similarity(&face.embedding, shots_so_far)) >= threshold_stranger
}

/// What the face file holds. Files from before guided poses have a single
/// `embedding`; newer ones a list of `embeddings` (one per pose).
#[derive(Debug, Default, Serialize, Deserialize)]
struct StoredFace {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    embeddings: Vec<Embedding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    embedding: Option<Embedding>,
}

/// More poses than this in a file means it's not one we wrote.
const MAX_POSES: usize = 16;

/// Saves the owner's poses with owner-only permissions.
pub fn save(path: &Path, poses: &[Embedding]) -> Result<()> {
    check_poses(poses).context("refusing to save a malformed face")?;
    let stored = StoredFace {
        embeddings: poses.to_vec(),
        embedding: None,
    };
    files::write_private(path, toml::to_string(&stored)?.as_bytes())
}

/// Loads the owner's poses, with a clear error if none is enrolled.
pub fn load(path: &Path) -> Result<Vec<Embedding>> {
    if !path.exists() {
        bail!("no face enrolled yet. Run `nodonuts enroll` first");
    }
    files::check_private(path)?;
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("could not read {}", path.display()))?;
    let stored: StoredFace =
        toml::from_str(&text).with_context(|| format!("invalid face file {}", path.display()))?;
    let mut poses = stored.embeddings;
    if let Some(old) = stored.embedding {
        poses.push(old);
    }
    if check_poses(&poses).is_err() {
        bail!("the enrolled face is malformed. Run `nodonuts enroll` again");
    }
    Ok(poses)
}

fn check_poses(poses: &[Embedding]) -> Result<()> {
    if poses.is_empty() || poses.len() > MAX_POSES {
        bail!("expected 1 to {MAX_POSES} poses, got {}", poses.len());
    }
    if poses.iter().any(|pose| pose.len() != face::EMBEDDING_LEN) {
        bail!("an embedding doesn't have {} values", face::EMBEDDING_LEN);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::RgbImage;
    use std::cell::Cell;

    fn unit_vector(axis: usize) -> Embedding {
        let mut e = vec![0.0; face::EMBEDDING_LEN];
        e[axis] = 1.0;
        e
    }

    const NO_WAIT: Timing = Timing {
        settle: Duration::ZERO,
        retry: Duration::ZERO,
    };

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
            Ok(RgbImage::new(100, 100))
        }
    }

    /// Plays back a script of what each frame shows (repeating the last).
    struct Scripted {
        script: Vec<Vec<Face>>,
        position: Cell<usize>,
    }

    impl FaceFinder for Scripted {
        fn find_faces(&self, _image: &RgbImage) -> Result<Vec<Face>> {
            let i = self.position.get().min(self.script.len() - 1);
            self.position.set(self.position.get() + 1);
            Ok(self.script[i].clone())
        }
    }

    fn face_on(axis: usize, area: f32) -> Face {
        Face {
            area,
            embedding: unit_vector(axis),
        }
    }

    fn enroll(finder: &dyn FaceFinder) -> Result<Vec<Embedding>> {
        let mut camera = FlakyCamera { calls: 0 };
        let mut ready_calls = 0;
        let result = capture_poses(
            &mut camera,
            finder,
            &mut || {
                ready_calls += 1;
                Ok(())
            },
            &NO_WAIT,
            0.2,
        );
        if result.is_ok() {
            assert_eq!(ready_calls, POSES.len(), "asked to get ready once per pose");
        }
        result
    }

    #[test]
    fn ten_shots_in_four_poses() {
        assert_eq!(total_shots(), 10);
        assert_eq!(POSES.len(), 4);
    }

    #[test]
    fn one_embedding_per_pose_from_the_biggest_face() {
        // A small stranger and a big owner in every frame.
        let finder = Scripted {
            script: vec![vec![face_on(1, 300.0), face_on(0, 2000.0)]],
            position: Cell::new(0),
        };
        let poses = enroll(&finder).unwrap();
        assert_eq!(poses.len(), POSES.len());
        for pose in &poses {
            assert!((pose[0] - 1.0).abs() < 1e-6, "the owner, not the stranger");
        }
    }

    #[test]
    fn retries_until_a_face_is_found() {
        // No face for a few frames, then the owner.
        let finder = Scripted {
            script: vec![vec![], vec![], vec![face_on(0, 2000.0)]],
            position: Cell::new(0),
        };
        assert_eq!(enroll(&finder).unwrap().len(), POSES.len());
    }

    #[test]
    fn a_tiny_face_is_not_accepted() {
        // 100 of 10 000 pixels = 1%, under the 2% minimum: never accepted.
        let finder = Scripted {
            script: vec![vec![face_on(0, 100.0)]],
            position: Cell::new(0),
        };
        assert!(enroll(&finder).is_err());
    }

    #[test]
    fn someone_else_is_not_enrolled() {
        // The owner (axis 0) for the first shot, then a different person.
        let finder = Scripted {
            script: vec![vec![face_on(0, 2000.0)], vec![face_on(1, 2000.0)]],
            position: Cell::new(0),
        };
        assert!(enroll(&finder).is_err());
    }

    #[test]
    fn fails_when_no_face_is_ever_found() {
        let finder = Scripted {
            script: vec![vec![]],
            position: Cell::new(0),
        };
        assert!(enroll(&finder).is_err());
    }

    fn temp_file(name: &str) -> std::path::PathBuf {
        std::env::temp_dir()
            .join(format!("nodonuts-owner-test-{}", std::process::id()))
            .join(name)
    }

    #[test]
    fn save_then_load_round_trips() {
        let path = temp_file("owner_face.toml");
        assert!(load(&path).is_err()); // nothing enrolled yet
        let poses = vec![unit_vector(0), unit_vector(1), unit_vector(2)];
        save(&path, &poses).unwrap();
        assert_eq!(load(&path).unwrap(), poses);
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn old_single_embedding_files_still_load() {
        let path = temp_file("old_face.toml");
        let old = StoredFace {
            embeddings: Vec::new(),
            embedding: Some(unit_vector(5)),
        };
        files::write_private(&path, toml::to_string(&old).unwrap().as_bytes()).unwrap();
        assert_eq!(load(&path).unwrap(), vec![unit_vector(5)]);
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn malformed_faces_are_rejected() {
        let path = temp_file("never-written.toml");
        assert!(save(&path, &[vec![0.1, 0.2]]).is_err());
        assert!(save(&path, &[]).is_err());
        assert!(save(&path, &vec![unit_vector(0); MAX_POSES + 1]).is_err());
    }
}
