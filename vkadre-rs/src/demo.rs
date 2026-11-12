//! Demo mode: `VKadre.exe --demo picture.jpg`.
//!
//! A stand-in for OBS: the picture plays the 4K camera, so the whole pipeline (neural net,
//! tracker, framing, previews) runs without OBS. Used for screenshots and to try the program.
//! Settings are not written to disk while the demo runs.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use anyhow::{anyhow, Result};
use serde_json::{json, Map, Value};

use crate::obs::ObsApi;
use crate::rgb::RgbImage;
use crate::settings::Settings;

const SOURCE_RU: &str = "Камера";
const SOURCE_EN: &str = "Camera";
const SCENES_RU: [(&str, &str); 3] = [("Проповедь", "track"), ("Интервью", "duo"), ("Общий план", "wide")];
const SCENES_EN: [(&str, &str); 3] = [("Keynote", "track"), ("Interview", "duo"), ("Wide shot", "wide")];
const CANVAS: (u32, u32) = (1920, 1080);
const SOURCE_W: u32 = 3840;

struct Demo {
    image: image::RgbImage,
    /// screenshots scaled to the requested size, by size
    scaled: Mutex<HashMap<(u32, u32), Arc<image::RgbImage>>>,
}

static DEMO: OnceLock<Demo> = OnceLock::new();
/// demo scenes and names in English (follows the interface language)
static ENGLISH: AtomicBool = AtomicBool::new(false);

fn source() -> &'static str {
    if ENGLISH.load(Ordering::Relaxed) { SOURCE_EN } else { SOURCE_RU }
}

fn scenes() -> &'static [(&'static str, &'static str); 3] {
    if ENGLISH.load(Ordering::Relaxed) { &SCENES_EN } else { &SCENES_RU }
}

/// Reads `--demo <picture>` from the command line; call once at startup.
pub fn init_from_args() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let Some(i) = args.iter().position(|a| a == "--demo") else {
        return Ok(());
    };
    let path = args.get(i + 1).ok_or_else(|| anyhow!("--demo: укажите картинку"))?;
    let img = image::open(path).map_err(|e| anyhow!("--demo {path}: {e}"))?.to_rgb8();
    // upscale to a 4K-wide "camera" so the crop math works like with a real 4K source
    let h = (SOURCE_W as f64 * img.height() as f64 / img.width() as f64).round() as u32;
    let image = image::imageops::resize(&img, SOURCE_W, h, image::imageops::FilterType::CatmullRom);
    log::info!("Demo mode: {path} ({}×{} → {SOURCE_W}×{h})", img.width(), img.height());
    let _ = DEMO.set(Demo { image, scaled: Mutex::new(HashMap::new()) });
    Ok(())
}

pub fn active() -> bool {
    DEMO.get().is_some()
}

/// Settings for the demo run: demo scenes and camera, tracking starts by itself.
pub fn apply(s: &mut Settings) {
    let args: Vec<String> = std::env::args().collect();
    let lang = args.iter().position(|a| a == "--lang").and_then(|i| args.get(i + 1)).cloned();
    let en = lang.as_deref().unwrap_or(&s.language) == "en";
    ENGLISH.store(en, Ordering::Relaxed);
    s.source = source().into();
    s.source_auto = false;
    s.scene_modes = scenes().iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    s.auto_connect = true;
    s.auto_start = true;
    s.start_minimized = false;
    // nothing from the real setup shows up on demo screenshots
    s.obs_host = "localhost".into();
    s.obs_port = 4455;
    s.obs_password = String::new();
    (s.lt_name, s.lt_title) = if en {
        ("Anna Morozova".into(), "Conference speaker".into())
    } else {
        ("Анна Морозова".into(), "Спикер конференции".into())
    };
    // a drawn picture scores lower than a real person: ~0.35 for the speaker, under 0.15 for the hall
    s.confidence = s.confidence.min(0.25);
    s.kp_conf = s.kp_conf.min(0.3);
}

/// What the window asks OBS for: version, scenes, sources per scene, source kinds.
pub fn lists() -> (String, Vec<String>, HashMap<String, Vec<String>>, HashMap<String, String>) {
    let names: Vec<String> = scenes().iter().map(|(k, _)| k.to_string()).collect();
    let map = names.iter().map(|s| (s.clone(), vec![source().to_string()])).collect();
    let kinds = HashMap::from([(source().to_string(), "dshow_input".to_string())]);
    let version = if ENGLISH.load(Ordering::Relaxed) { "OBS 31 · demo" } else { "OBS 31 · демо" };
    (version.into(), names, map, kinds)
}

pub struct DemoObs {
    noise: u32,
}

impl DemoObs {
    pub fn new() -> Self {
        DemoObs { noise: 0x9E37_79B9 }
    }
}

fn demo() -> Result<&'static Demo> {
    DEMO.get().ok_or_else(|| anyhow!("демо-режим не включён"))
}

impl ObsApi for DemoObs {
    fn item_id(&mut self, _scene: &str, source: &str) -> Result<i64> {
        if source == self::source() {
            Ok(1)
        } else {
            Err(anyhow!("нет источника «{source}»"))
        }
    }

    fn item_transform(&mut self, _scene: &str, _item_id: i64) -> Result<Map<String, Value>> {
        let d = demo()?;
        let v = json!({ "sourceWidth": d.image.width(), "sourceHeight": d.image.height(),
                        "positionX": 0.0, "positionY": 0.0, "scaleX": 0.5, "scaleY": 0.5 });
        Ok(v.as_object().cloned().unwrap_or_default())
    }

    fn set_item_transform(&mut self, _scene: &str, _item_id: i64, _transform: Value) -> Result<()> {
        Ok(())
    }

    fn canvas_size(&mut self) -> Result<(u32, u32)> {
        Ok(CANVAS)
    }

    fn program_scene(&mut self) -> Result<Option<String>> {
        Ok(Some(scenes()[0].0.into()))
    }

    fn preview_scene(&mut self) -> Option<String> {
        None
    }

    fn set_program_scene(&mut self, _scene: &str) -> Result<()> {
        Ok(())
    }

    fn screenshot(&mut self, _source: &str, width: u32, height: u32) -> Result<Option<RgbImage>> {
        let d = demo()?;
        let scaled = {
            let mut cache = d.scaled.lock().unwrap();
            cache
                .entry((width, height))
                .or_insert_with(|| {
                    Arc::new(image::imageops::resize(&d.image, width, height, image::imageops::FilterType::Triangle))
                })
                .clone()
        };
        // a little sensor noise: a real camera never gives two identical frames
        let mut data = scaled.as_raw().clone();
        for v in data.iter_mut() {
            self.noise ^= self.noise << 13;
            self.noise ^= self.noise >> 17;
            self.noise ^= self.noise << 5;
            *v = v.saturating_add((self.noise & 3) as u8);
        }
        std::thread::sleep(Duration::from_millis(15));
        Ok(Some(RgbImage { width: width as usize, height: height as usize, data }))
    }

    fn wait_scene_event(&mut self) -> Result<bool> {
        std::thread::sleep(Duration::from_millis(500));
        Ok(false)
    }
}
