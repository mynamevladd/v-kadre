//! Person + pose detector on ONNX Runtime (YOLO11-pose exported to ONNX).
//!
//! Runs on the GPU through DirectML (any DX12 GPU: NVIDIA / AMD / Intel), falls back to CPU.
//! Model output: (1, 56, N) — 4 box (cx, cy, w, h in input pixels), 1 person score,
//! 17 keypoints * (x, y, conf). Scores are already sigmoid-ed by the export.

use std::path::Path;

use anyhow::{anyhow, Context, Result};
use ndarray::{Array4, ArrayViewD};

use crate::framing::{Keypoints, Person};
use crate::rgb::RgbImage;

/// Anything that turns a frame into people. The engine only needs this; tests use a fake.
pub trait Detect: Send + Sync {
    fn input_size(&self) -> usize;
    fn device_name(&self) -> String;
    fn detect(&self, img: &RgbImage, conf_thr: f64) -> Result<Vec<Person>>;
}

pub fn nms(boxes: &[[f32; 4]], scores: &[f32], iou_thr: f32) -> Vec<usize> {
    let mut order: Vec<usize> = (0..scores.len()).collect();
    order.sort_by(|&a, &b| scores[b].partial_cmp(&scores[a]).unwrap_or(std::cmp::Ordering::Equal));
    let mut keep = Vec::new();
    while let Some(&i) = order.first() {
        keep.push(i);
        let bi = boxes[i];
        let area_i = (bi[2] - bi[0]) * (bi[3] - bi[1]);
        order = order[1..]
            .iter()
            .copied()
            .filter(|&r| {
                let br = boxes[r];
                let w = (bi[2].min(br[2]) - bi[0].max(br[0])).max(0.0);
                let h = (bi[3].min(br[3]) - bi[1].max(br[1])).max(0.0);
                let inter = w * h;
                let area_r = (br[2] - br[0]) * (br[3] - br[1]);
                inter / (area_i + area_r - inter).max(1e-9) <= iou_thr
            })
            .collect();
    }
    keep
}

/// Small bilinear resize (the engine asks OBS for the right size, so this is rarely hit).
pub fn resize_bilinear(img: &RgbImage, nw: usize, nh: usize) -> RgbImage {
    let (w, h) = (img.width, img.height);
    if (w, h) == (nw, nh) {
        return img.clone();
    }
    let mut out = RgbImage::new(nw, nh, 0);
    let xs: Vec<(usize, usize, f32)> = (0..nw)
        .map(|x| {
            let sx = (x as f32 + 0.5) * w as f32 / nw as f32 - 0.5;
            let x0 = (sx.floor() as i64).clamp(0, w as i64 - 1) as usize;
            let x1 = (x0 + 1).min(w - 1);
            (x0, x1, (sx - x0 as f32).clamp(0.0, 1.0))
        })
        .collect();
    for y in 0..nh {
        let sy = (y as f32 + 0.5) * h as f32 / nh as f32 - 0.5;
        let y0 = (sy.floor() as i64).clamp(0, h as i64 - 1) as usize;
        let y1 = (y0 + 1).min(h - 1);
        let fy = (sy - y0 as f32).clamp(0.0, 1.0);
        for (x, &(x0, x1, fx)) in xs.iter().enumerate() {
            let (a, b, c, d) = (img.pixel(x0, y0), img.pixel(x1, y0), img.pixel(x0, y1), img.pixel(x1, y1));
            let mut px = [0u8; 3];
            for k in 0..3 {
                let top = a[k] as f32 * (1.0 - fx) + b[k] as f32 * fx;
                let bot = c[k] as f32 * (1.0 - fx) + d[k] as f32 * fx;
                px[k] = (top * (1.0 - fy) + bot * fy) as u8;
            }
            out.set_pixel(x, y, px);
        }
    }
    out
}

/// Letterbox geometry: resize keeping aspect, pad to size x size.
#[derive(Debug, Clone, Copy)]
pub struct Letterbox {
    pub scale: f32,
    pub px: usize,
    pub py: usize,
    pub nw: usize,
    pub nh: usize,
}

pub fn letterbox_geometry(w: usize, h: usize, size: usize) -> Letterbox {
    let scale = (size as f32 / w as f32).min(size as f32 / h as f32);
    let nw = (w as f32 * scale).round() as usize;
    let nh = (h as f32 * scale).round() as usize;
    Letterbox { scale, px: (size - nw) / 2, py: (size - nh) / 2, nw, nh }
}

/// NCHW float tensor 0..1, grey (114) padding.
pub fn letterbox_tensor(img: &RgbImage, size: usize) -> (Array4<f32>, Letterbox) {
    let lb = letterbox_geometry(img.width, img.height, size);
    let resized = resize_bilinear(img, lb.nw, lb.nh);
    let mut t = Array4::<f32>::from_elem((1, 3, size, size), 114.0 / 255.0);
    for y in 0..lb.nh {
        for x in 0..lb.nw {
            let p = resized.pixel(x, y);
            for c in 0..3 {
                t[[0, c, y + lb.py, x + lb.px]] = p[c] as f32 / 255.0;
            }
        }
    }
    (t, lb)
}

/// Turns raw (1, 56, N) model output into normalized people.
pub fn decode(output: &ArrayViewD<f32>, img_w: usize, img_h: usize, lb: Letterbox, conf_thr: f32, iou_thr: f32) -> Vec<Person> {
    let shape = output.shape();
    if shape.len() != 3 || shape[1] < 56 {
        return Vec::new();
    }
    let n = shape[2];
    let at = |k: usize, j: usize| output[[0, k, j]];

    let mut idx = Vec::new();
    let mut boxes = Vec::new();
    let mut scores = Vec::new();
    for j in 0..n {
        let sc = at(4, j);
        if sc < conf_thr {
            continue;
        }
        let (cx, cy, bw, bh) = (at(0, j), at(1, j), at(2, j), at(3, j));
        idx.push(j);
        boxes.push([cx - bw / 2.0, cy - bh / 2.0, cx + bw / 2.0, cy + bh / 2.0]);
        scores.push(sc);
    }
    if idx.is_empty() {
        return Vec::new();
    }
    let (s, px, py) = (lb.scale, lb.px as f32, lb.py as f32);
    let (iw, ih) = (img_w as f32, img_h as f32);
    let nx = |v: f32| ((v - px) / s / iw) as f64;
    let ny = |v: f32| ((v - py) / s / ih) as f64;

    nms(&boxes, &scores, iou_thr)
        .into_iter()
        .map(|k| {
            let j = idx[k];
            let b = boxes[k];
            let mut xy = [[0.0; 2]; 17];
            let mut conf = [0.0; 17];
            for p in 0..17 {
                xy[p] = [nx(at(5 + 3 * p, j)), ny(at(6 + 3 * p, j))];
                conf[p] = at(7 + 3 * p, j) as f64;
            }
            Person {
                bbox: [nx(b[0]), ny(b[1]), nx(b[2]), ny(b[3])],
                score: scores[k] as f64,
                kp: Some(Keypoints { xy, conf }),
                tid: -1,
            }
        })
        .collect()
}

pub struct PoseDetector {
    session: ort::session::Session,
    input_name: String,
    size: usize,
    gpu: bool,
}

impl PoseDetector {
    pub fn new(model_path: &Path, use_gpu: bool) -> Result<Self> {
        if !model_path.exists() {
            return Err(anyhow!("Модель не найдена: {}", model_path.display()));
        }
        if use_gpu {
            match Self::build(model_path, true) {
                Ok(d) => return Ok(d),
                Err(e) => log::warn!("DirectML недоступен, работаю на процессоре: {e:#}"),
            }
        }
        Self::build(model_path, false)
    }

    fn build(model_path: &Path, gpu: bool) -> Result<Self> {
        use ort::session::builder::GraphOptimizationLevel;
        let mut b = ort::session::Session::builder()?.with_optimization_level(GraphOptimizationLevel::Level3)?;
        if gpu {
            // DirectML requirements: no memory pattern, sequential execution
            b = b
                .with_execution_providers([ort::execution_providers::DirectMLExecutionProvider::default()
                    .build()
                    .error_on_failure()])?
                .with_memory_pattern(false)?
                .with_parallel_execution(false)?;
        } else {
            // a few threads are enough for the nano model; leave the rest of the CPU to OBS
            let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
            b = b.with_intra_threads((cores / 2).clamp(1, 4))?;
        }
        let session = b.commit_from_file(model_path).context("не удалось загрузить модель")?;
        let input = session.inputs.first().ok_or_else(|| anyhow!("у модели нет входов"))?;
        let input_name = input.name.clone();
        let size = match &input.input_type {
            ort::value::ValueType::Tensor { dimensions, .. } if dimensions.len() == 4 && dimensions[2] > 0 => {
                dimensions[2] as usize
            }
            _ => 640,
        };
        log::info!(
            "Detector: {}, input {size}, {}",
            model_path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default(),
            if gpu { "DirectML" } else { "CPU" }
        );
        Ok(PoseDetector { session, input_name, size, gpu })
    }
}

impl Detect for PoseDetector {
    fn input_size(&self) -> usize {
        self.size
    }

    fn device_name(&self) -> String {
        if self.gpu { "GPU (DirectML)".into() } else { "CPU".into() }
    }

    fn detect(&self, img: &RgbImage, conf_thr: f64) -> Result<Vec<Person>> {
        let (x, lb) = letterbox_tensor(img, self.size);
        let outputs = self.session.run(ort::inputs![self.input_name.as_str() => x]?)?;
        let out = outputs[0].try_extract_tensor::<f32>()?;
        Ok(decode(&out, img.width, img.height, lb, conf_thr as f32, 0.5))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ndarray::Array3;

    #[test]
    fn nms_basic() {
        let boxes = [[0.0, 0.0, 10.0, 10.0], [1.0, 1.0, 11.0, 11.0], [50.0, 50.0, 60.0, 60.0]];
        assert_eq!(nms(&boxes, &[0.9, 0.8, 0.7], 0.5), vec![0, 2]);
    }

    #[test]
    fn decode_roundtrip() {
        let lb = letterbox_geometry(960, 540, 640);
        let n = 10;
        let mut out = Array3::<f32>::zeros((1, 56, n));
        // one person at image center, 2 duplicates + low-score noise
        for j in 0..3 {
            out[[0, 0, j]] = 320.0 + j as f32;
            out[[0, 1, j]] = 320.0;
            out[[0, 2, j]] = 60.0;
            out[[0, 3, j]] = 200.0;
            out[[0, 4, j]] = 0.9 - j as f32 * 0.1;
            for p in 0..17 {
                out[[0, 5 + 3 * p, j]] = 320.0;
                out[[0, 6 + 3 * p, j]] = 300.0;
                out[[0, 7 + 3 * p, j]] = 0.95;
            }
        }
        for j in 3..n {
            out[[0, 4, j]] = 0.05;
        }
        let view = out.into_dyn();
        let people = decode(&view.view(), 960, 540, lb, 0.4, 0.5);
        assert_eq!(people.len(), 1);
        let (cx, _) = people[0].center();
        assert!((cx - 0.5).abs() < 0.01);
        assert!((people[0].kp.as_ref().unwrap().conf[0] - 0.95).abs() < 1e-6);
    }

    #[test]
    fn letterbox_pads_and_scales() {
        let img = RgbImage::new(640, 360, 200);
        let (t, lb) = letterbox_tensor(&img, 640);
        assert_eq!((lb.nw, lb.nh, lb.px, lb.py), (640, 360, 0, 140));
        assert!((t[[0, 0, 0, 0]] - 114.0 / 255.0).abs() < 1e-6);
        assert!((t[[0, 0, 300, 10]] - 200.0 / 255.0).abs() < 1e-6);
    }
}
