//! Faces: find them (YuNet), turn each into an embedding (SFace), and
//! compare embeddings (cosine similarity).
//!
//! An *embedding* is a list of 128 numbers that describes a face. Photos of
//! the same person give similar lists; different people give different
//! ones. We only ever store this list, never an image.

use anyhow::{Context, Result};
use image::RgbImage;
use image::imageops::{self, FilterType};
use tract_onnx::prelude::*;

/// A face description: 128 numbers, scaled to length 1.
pub type Embedding = Vec<f32>;

/// Number of values in an SFace embedding.
pub const EMBEDDING_LEN: usize = 128;

/// One face found in a frame.
#[derive(Debug, Clone)]
pub struct Face {
    /// Size of the face box in pixels (width x height). Bigger = closer.
    pub area: f32,
    /// What the face looks like.
    pub embedding: Embedding,
}

/// Anything that can find faces in an image and describe them.
/// Tests use a fake one; the real one is [`OnnxFaceFinder`].
pub trait FaceFinder {
    /// Returns every face found, best detections first.
    fn find_faces(&self, image: &RgbImage) -> Result<Vec<Face>>;
}

/// How much `face` looks like the owner: its similarity to the best-matching
/// enrolled pose (straight, turned left, ...). 0 if nothing is enrolled.
pub fn best_similarity(face: &[f32], owner: &[Embedding]) -> f32 {
    owner
        .iter()
        .map(|pose| cosine_similarity(face, pose))
        .fold(0.0, f32::max)
}

/// Is any of these faces someone other than the owner?
pub fn has_stranger(faces: &[Face], owner: &[Embedding], threshold: f64) -> bool {
    faces
        .iter()
        .any(|face| f64::from(best_similarity(&face.embedding, owner)) < threshold)
}

/// Is any of these faces the owner?
pub fn is_owner(faces: &[Face], owner: &[Embedding], threshold: f64) -> bool {
    faces
        .iter()
        .any(|face| f64::from(best_similarity(&face.embedding, owner)) >= threshold)
}

/// How alike two embeddings are: 1.0 = same direction, 0 = unrelated.
///
/// Returns 0 for mismatched or empty inputs, so bad data never matches.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let norm_a = length(a);
    let norm_b = length(b);
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    dot / (norm_a * norm_b)
}

/// Scales a vector to length 1 (leaves an all-zero vector alone).
pub fn normalize(mut values: Vec<f32>) -> Vec<f32> {
    let len = length(&values);
    if len > 0.0 {
        for v in &mut values {
            *v /= len;
        }
    }
    values
}

/// Averages several embeddings into one, then normalizes it.
pub fn average(embeddings: &[Embedding]) -> Embedding {
    let mut sum = vec![0.0_f32; EMBEDDING_LEN];
    for embedding in embeddings {
        for (total, value) in sum.iter_mut().zip(embedding) {
            *total += value;
        }
    }
    normalize(sum)
}

fn length(values: &[f32]) -> f32 {
    values.iter().map(|v| v * v).sum::<f32>().sqrt()
}

// ---------------------------------------------------------------------------
// The real face finder, built on two ONNX models.
// ---------------------------------------------------------------------------

// The models are compiled into the binary, so NoDonuts ships as one file.
static YUNET_MODEL: &[u8] = include_bytes!("../models/yunet.onnx");
static SFACE_MODEL: &[u8] = include_bytes!("../models/sface.onnx");

/// YuNet takes a fixed 640x640 image.
const DETECT_SIZE: u32 = 640;
/// SFace takes a 112x112 image of an aligned face.
const ALIGNED_SIZE: u32 = 112;
/// Minimum detection confidence (OpenCV's default for YuNet).
const MIN_DETECTION_SCORE: f32 = 0.9;
/// Overlapping boxes above this overlap are the same face.
const MAX_OVERLAP: f32 = 0.3;
/// Only describe the best few faces, to bound the work per frame.
const MAX_FACES: usize = 5;

/// Where the eyes, nose and mouth corners sit in the 112x112 image SFace
/// was trained on. We rotate and scale each face to match.
const ALIGNED_LANDMARKS: [(f32, f32); 5] = [
    (38.2946, 51.6963), // eye on the image's left
    (73.5318, 51.5014), // eye on the image's right
    (56.0252, 71.7366), // nose tip
    (41.5493, 92.3655), // mouth corner, image's left
    (70.7299, 92.2041), // mouth corner, image's right
];

/// A loaded, ready-to-run model.
type Model = TypedRunnableModel<TypedModel>;

/// Face finder using YuNet (detection) and SFace (recognition).
#[derive(Debug)]
pub struct OnnxFaceFinder {
    detector: Model,
    recognizer: Model,
}

/// A face box from YuNet, in original image pixels.
#[derive(Debug, Clone, Copy)]
struct Detection {
    score: f32,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    /// Eyes, nose tip, mouth corners (same order as `ALIGNED_LANDMARKS`).
    landmarks: [(f32, f32); 5],
}

impl OnnxFaceFinder {
    /// Loads and optimizes both models. Takes a moment, so do it once.
    pub fn load() -> Result<OnnxFaceFinder> {
        Ok(OnnxFaceFinder {
            detector: load_model(YUNET_MODEL, DETECT_SIZE).context("could not load YuNet")?,
            recognizer: load_model(SFACE_MODEL, ALIGNED_SIZE).context("could not load SFace")?,
        })
    }

    fn detect(&self, image: &RgbImage) -> Result<Vec<Detection>> {
        // Shrink the frame to fit 640x640 without stretching it; the rest
        // of the 640x640 input stays black.
        let scale = DETECT_SIZE as f32 / image.width().max(image.height()) as f32;
        let new_width = ((image.width() as f32 * scale).round() as u32).clamp(1, DETECT_SIZE);
        let new_height = ((image.height() as f32 * scale).round() as u32).clamp(1, DETECT_SIZE);
        let resized = imageops::resize(image, new_width, new_height, FilterType::Triangle);

        // YuNet wants blue-green-red order and raw 0-255 values.
        let size = DETECT_SIZE as usize;
        let mut input = tract_ndarray::Array4::<f32>::zeros((1, 3, size, size));
        for (x, y, pixel) in resized.enumerate_pixels() {
            let [red, green, blue] = pixel.0;
            let (x, y) = (x as usize, y as usize);
            input[[0, 0, y, x]] = f32::from(blue);
            input[[0, 1, y, x]] = f32::from(green);
            input[[0, 2, y, x]] = f32::from(red);
        }
        let outputs = self.detector.run(tvec!(Tensor::from(input).into()))?;

        // YuNet looks at the image on three grids (cells of 8, 16 and 32
        // pixels). Its 12 outputs are, in order: class score, objectness,
        // box and landmarks, each for strides 8, 16, 32.
        let mut detections = Vec::new();
        for (i, stride) in [8_u32, 16, 32].into_iter().enumerate() {
            let class_score = outputs[i].to_array_view::<f32>()?;
            let objectness = outputs[3 + i].to_array_view::<f32>()?;
            let boxes = outputs[6 + i].to_array_view::<f32>()?;
            let points = outputs[9 + i].to_array_view::<f32>()?;
            let columns = (DETECT_SIZE / stride) as usize;
            let s = stride as f32;

            for cell in 0..class_score.shape()[1] {
                let score = (class_score[[0, cell, 0]].clamp(0.0, 1.0)
                    * objectness[[0, cell, 0]].clamp(0.0, 1.0))
                .sqrt();
                if score < MIN_DETECTION_SCORE {
                    continue;
                }
                let column = (cell % columns) as f32;
                let row = (cell / columns) as f32;
                // Box centre is an offset from the cell; size is log-scaled.
                let center_x = (column + boxes[[0, cell, 0]]) * s;
                let center_y = (row + boxes[[0, cell, 1]]) * s;
                let width = boxes[[0, cell, 2]].exp() * s;
                let height = boxes[[0, cell, 3]].exp() * s;
                let mut landmarks = [(0.0, 0.0); 5];
                for (n, landmark) in landmarks.iter_mut().enumerate() {
                    let lx = (column + points[[0, cell, 2 * n]]) * s;
                    let ly = (row + points[[0, cell, 2 * n + 1]]) * s;
                    *landmark = (lx / scale, ly / scale);
                }
                // Divide by `scale` to go back to original image pixels.
                detections.push(Detection {
                    score,
                    x: (center_x - width / 2.0) / scale,
                    y: (center_y - height / 2.0) / scale,
                    width: width / scale,
                    height: height / scale,
                    landmarks,
                });
            }
        }
        Ok(remove_overlaps(detections))
    }

    fn embed(&self, image: &RgbImage, detection: &Detection) -> Result<Embedding> {
        let aligned = align_face(image, &detection.landmarks);
        // SFace wants red-green-blue order and raw 0-255 values.
        let size = ALIGNED_SIZE as usize;
        let mut input = tract_ndarray::Array4::<f32>::zeros((1, 3, size, size));
        for (x, y, pixel) in aligned.enumerate_pixels() {
            let (x, y) = (x as usize, y as usize);
            for channel in 0..3 {
                input[[0, channel, y, x]] = f32::from(pixel.0[channel]);
            }
        }
        let outputs = self.recognizer.run(tvec!(Tensor::from(input).into()))?;
        let values: Vec<f32> = outputs[0].to_array_view::<f32>()?.iter().copied().collect();
        Ok(normalize(values))
    }
}

impl FaceFinder for OnnxFaceFinder {
    fn find_faces(&self, image: &RgbImage) -> Result<Vec<Face>> {
        let mut faces = Vec::new();
        for detection in self.detect(image)?.iter().take(MAX_FACES) {
            faces.push(Face {
                area: detection.width * detection.height,
                embedding: self.embed(image, detection)?,
            });
        }
        Ok(faces)
    }
}

/// Parses an ONNX model and fixes its input to a 1x3xSIZExSIZE image.
fn load_model(bytes: &[u8], size: u32) -> Result<Model> {
    // `&[u8]` implements `Read`, so tract can read the embedded bytes.
    let mut reader = bytes;
    let size = size as usize;
    let model = tract_onnx::onnx()
        .model_for_read(&mut reader)?
        .with_input_fact(0, f32::fact([1, 3, size, size]).into())?
        .into_optimized()?
        .into_runnable()?;
    Ok(model)
}

/// Non-maximum suppression: when boxes overlap, keep only the best one.
fn remove_overlaps(mut detections: Vec<Detection>) -> Vec<Detection> {
    detections.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut kept: Vec<Detection> = Vec::new();
    for candidate in detections {
        if kept.iter().all(|k| overlap(k, &candidate) <= MAX_OVERLAP) {
            kept.push(candidate);
        }
    }
    kept
}

/// Intersection over union of two boxes: 0 = apart, 1 = identical.
fn overlap(a: &Detection, b: &Detection) -> f32 {
    let left = a.x.max(b.x);
    let top = a.y.max(b.y);
    let right = (a.x + a.width).min(b.x + b.width);
    let bottom = (a.y + a.height).min(b.y + b.height);
    let intersection = (right - left).max(0.0) * (bottom - top).max(0.0);
    let union = a.width * a.height + b.width * b.height - intersection;
    if union <= 0.0 {
        0.0
    } else {
        intersection / union
    }
}

/// Cuts out the face as a 112x112 image, rotated and scaled so the eyes,
/// nose and mouth land where SFace expects them.
fn align_face(image: &RgbImage, landmarks: &[(f32, f32); 5]) -> RgbImage {
    // Find the rotation+scale (a, b) and shift (tx, ty) that best map the
    // detected landmarks onto the template:
    //   template_x = a*x - b*y + tx
    //   template_y = b*x + a*y + ty
    // This is the standard least-squares "similarity transform".
    let n = landmarks.len() as f32;
    let (mut sx, mut sy, mut dx, mut dy) = (0.0, 0.0, 0.0, 0.0);
    for (&(x, y), &(u, v)) in landmarks.iter().zip(&ALIGNED_LANDMARKS) {
        sx += x;
        sy += y;
        dx += u;
        dy += v;
    }
    let (src_cx, src_cy, dst_cx, dst_cy) = (sx / n, sy / n, dx / n, dy / n);
    let (mut num_a, mut num_b, mut denom) = (0.0, 0.0, 0.0);
    for (&(x, y), &(u, v)) in landmarks.iter().zip(&ALIGNED_LANDMARKS) {
        let (x, y, u, v) = (x - src_cx, y - src_cy, u - dst_cx, v - dst_cy);
        num_a += x * u + y * v;
        num_b += x * v - y * u;
        denom += x * x + y * y;
    }
    let mut aligned = RgbImage::new(ALIGNED_SIZE, ALIGNED_SIZE);
    if denom <= f32::EPSILON {
        return aligned; // degenerate landmarks: a black image never matches.
    }
    let a = num_a / denom;
    let b = num_b / denom;
    let tx = dst_cx - (a * src_cx - b * src_cy);
    let ty = dst_cy - (b * src_cx + a * src_cy);

    // For each output pixel, run the transform backwards to find where it
    // comes from in the camera frame.
    let det = a * a + b * b;
    for (u, v, pixel) in aligned.enumerate_pixels_mut() {
        let (u, v) = (u as f32 - tx, v as f32 - ty);
        let x = (a * u + b * v) / det;
        let y = (-b * u + a * v) / det;
        *pixel = image::Rgb(sample(image, x, y));
    }
    aligned
}

/// Reads a colour at a fractional position by blending the 4 nearest
/// pixels. Outside the image is black.
fn sample(image: &RgbImage, x: f32, y: f32) -> [u8; 3] {
    let (width, height) = (image.width() as i64, image.height() as i64);
    let (x0, y0) = (x.floor() as i64, y.floor() as i64);
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let pixel_at = |px: i64, py: i64| -> [f32; 3] {
        if px < 0 || py < 0 || px >= width || py >= height {
            return [0.0; 3];
        }
        let p = image.get_pixel(px as u32, py as u32).0;
        [f32::from(p[0]), f32::from(p[1]), f32::from(p[2])]
    };
    let (p00, p10) = (pixel_at(x0, y0), pixel_at(x0 + 1, y0));
    let (p01, p11) = (pixel_at(x0, y0 + 1), pixel_at(x0 + 1, y0 + 1));
    let mut out = [0_u8; 3];
    for c in 0..3 {
        let top = p00[c] * (1.0 - fx) + p10[c] * fx;
        let bottom = p01[c] * (1.0 - fx) + p11[c] * fx;
        out[c] = (top * (1.0 - fy) + bottom * fy).round().clamp(0.0, 255.0) as u8;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn face(embedding: Vec<f32>) -> Face {
        Face {
            area: 1.0,
            embedding,
        }
    }

    #[test]
    fn identical_vectors_have_similarity_one() {
        let v = vec![0.3, -0.2, 0.9];
        assert!((cosine_similarity(&v, &v) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn opposite_and_orthogonal_vectors() {
        assert!((cosine_similarity(&[1.0, 0.0], &[-1.0, 0.0]) + 1.0).abs() < 1e-6);
        assert!(cosine_similarity(&[1.0, 0.0], &[0.0, 1.0]).abs() < 1e-6);
    }

    #[test]
    fn similarity_ignores_length() {
        let similarity = cosine_similarity(&[1.0, 2.0, 3.0], &[2.0, 4.0, 6.0]);
        assert!((similarity - 1.0).abs() < 1e-6);
    }

    #[test]
    fn bad_inputs_never_match() {
        assert_eq!(cosine_similarity(&[], &[]), 0.0);
        assert_eq!(cosine_similarity(&[1.0], &[1.0, 2.0]), 0.0);
        assert_eq!(cosine_similarity(&[0.0, 0.0], &[1.0, 2.0]), 0.0);
    }

    #[test]
    fn is_owner_uses_the_threshold() {
        let owner = vec![vec![1.0, 0.0]];
        let close = face(vec![0.9, 0.1]);
        let far = face(vec![0.0, 1.0]);
        assert!(is_owner(&[far.clone(), close], &owner, 0.36));
        assert!(!is_owner(&[far], &owner, 0.36));
        assert!(!is_owner(&[], &owner, 0.36));
    }

    #[test]
    fn has_stranger_finds_non_owner_faces() {
        let owner = vec![vec![1.0, 0.0]];
        let close = face(vec![0.9, 0.1]);
        let far = face(vec![0.0, 1.0]);
        assert!(has_stranger(&[close.clone(), far], &owner, 0.36));
        assert!(!has_stranger(&[close], &owner, 0.36));
        assert!(!has_stranger(&[], &owner, 0.36));
    }

    #[test]
    fn a_face_matches_its_best_enrolled_pose() {
        // Two poses: straight (axis 0) and turned (axis 1).
        let owner = vec![vec![1.0, 0.0, 0.0], vec![0.0, 1.0, 0.0]];
        let turned = face(vec![0.1, 0.95, 0.0]);
        assert!(best_similarity(&turned.embedding, &owner) > 0.9);
        assert!(is_owner(std::slice::from_ref(&turned), &owner, 0.36));
        assert!(!has_stranger(&[turned], &owner, 0.2));
        // Unlike every pose: a stranger.
        let other = face(vec![0.0, 0.0, 1.0]);
        assert!(has_stranger(&[other], &owner, 0.2));
        assert_eq!(best_similarity(&[1.0, 0.0, 0.0], &[]), 0.0);
    }

    #[test]
    fn average_is_normalized() {
        let mut a = vec![0.0; EMBEDDING_LEN];
        let mut b = vec![0.0; EMBEDDING_LEN];
        a[0] = 1.0;
        b[1] = 1.0;
        let avg = average(&[a, b]);
        assert!((length(&avg) - 1.0).abs() < 1e-6);
        assert!((avg[0] - avg[1]).abs() < 1e-6);
    }

    #[test]
    fn overlapping_boxes_are_merged() {
        let base = Detection {
            score: 0.95,
            x: 0.0,
            y: 0.0,
            width: 100.0,
            height: 100.0,
            landmarks: [(0.0, 0.0); 5],
        };
        let shifted = Detection {
            score: 0.92,
            x: 5.0,
            ..base
        };
        let apart = Detection {
            score: 0.91,
            x: 500.0,
            ..base
        };
        let kept = remove_overlaps(vec![shifted, apart, base]);
        assert_eq!(kept.len(), 2);
        assert_eq!(kept[0].score, 0.95);
    }

    #[test]
    fn aligning_the_template_is_the_identity() {
        // A gradient image where each pixel encodes its position.
        let image = RgbImage::from_fn(ALIGNED_SIZE, ALIGNED_SIZE, |x, y| {
            image::Rgb([x as u8, y as u8, 0])
        });
        let aligned = align_face(&image, &ALIGNED_LANDMARKS);
        assert_eq!(aligned.get_pixel(50, 60), image.get_pixel(50, 60));
    }
}
