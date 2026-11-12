//! User settings, stored as JSON in %APPDATA%\VKadre\settings.json.
//! Same file and field names as the Python version, so both read each other's settings.

use std::path::PathBuf;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

pub const APP_NAME: &str = "В кадре";
pub const APP_ID: &str = "VKadre";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn app_data_dir() -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| dirs_home().join(".config"));
    let d = base.join(APP_ID);
    let _ = std::fs::create_dir_all(&d);
    d
}

fn dirs_home() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Folders where bundled resources (models/) may live: next to the exe, then the repo root in dev.
pub fn resource_dirs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            v.push(dir.to_path_buf());
        }
    }
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    v.push(manifest.clone());
    if let Some(parent) = manifest.parent() {
        v.push(parent.to_path_buf());
    }
    v
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    // OBS connection
    pub obs_host: String,
    pub obs_port: u16,
    pub obs_password: String,
    pub auto_connect: bool,

    // App
    /// "ru" | "en"
    pub language: String,
    /// turn tracking on as soon as OBS is reached
    pub auto_start: bool,
    pub start_minimized: bool,

    // What to control
    /// legacy: main scene (used when scene_modes is empty)
    pub scene: String,
    /// camera source (the same source can sit in several scenes)
    pub source: String,
    /// the source was picked by the app, not by the person: a better camera may replace it
    pub source_auto: bool,
    /// legacy, migrated into scene_modes
    pub active_scenes: Vec<String>,
    /// What to do in each OBS scene: track | duo | wide. Scenes not listed are not touched.
    pub scene_modes: IndexMap<String, String>,

    // Detection
    pub model: String,
    pub use_gpu: bool,
    pub confidence: f64,
    pub kp_conf: f64,
    pub detect_fps: u32,
    pub render_fps: u32,

    // Shot
    /// auto | fixed
    pub zoom_mode: String,
    /// chest | waist | hips | knee
    pub shot: String,
    pub fixed_zoom: f64,
    pub min_zoom: f64,
    pub max_zoom: f64,
    pub wide_zoom: f64,
    pub headroom: f64,
    pub torso_smoothing: f64,

    // Motion
    pub deadzone_x: f64,
    pub deadzone_y: f64,
    pub deadzone_zoom: f64,
    pub smooth_time: f64,
    pub zoom_smooth_time: f64,
    pub max_speed: f64,

    // Behaviour
    pub lost_timeout: f64,
    pub wide_on_lost: bool,
    pub zone: [f64; 4],
    pub restore_on_exit: bool,

    // Auto-cut: two shots from one camera
    pub auto_cut: bool,
    /// hips | knee | full
    pub cut_wide_shot: String,
    /// seconds a shot stays before the next cut
    pub cut_min_hold: f64,
    /// speaker speed (frame widths / s) that counts as walking
    pub cut_move_speed: f64,

    // Remember the speaker by clothing color
    pub reid: bool,
    pub reid_threshold: f64,

    // Camera watchdog
    pub watchdog: bool,
    pub watchdog_sound: bool,
    pub watchdog_switch: bool,
    pub backup_scene: String,

    // Name plate (lower third)
    pub lt_name: String,
    pub lt_title: String,
    pub lt_auto: bool,
    pub lt_duration: f64,
    /// after the speaker is found in a newly active scene
    pub lt_delay: f64,
    pub lt_color: String,
    /// dark | light | color
    pub lt_plate: String,
    /// wipe | slide | rise | fade
    pub lt_anim: String,
    pub lt_anim_speed: f64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            obs_host: "localhost".into(),
            obs_port: 4455,
            obs_password: String::new(),
            auto_connect: true,
            language: "ru".into(),
            auto_start: false,
            start_minimized: false,
            scene: String::new(),
            source: String::new(),
            source_auto: true,
            active_scenes: Vec::new(),
            scene_modes: IndexMap::new(),
            model: "yolo11n-pose.onnx".into(),
            use_gpu: true,
            confidence: 0.4,
            kp_conf: 0.5,
            detect_fps: 10,
            render_fps: 30,
            zoom_mode: "auto".into(),
            shot: "waist".into(),
            fixed_zoom: 2.0,
            min_zoom: 1.3,
            max_zoom: 3.0,
            wide_zoom: 1.0,
            headroom: 0.08,
            torso_smoothing: 0.8,
            deadzone_x: 0.12,
            deadzone_y: 0.15,
            deadzone_zoom: 0.2,
            smooth_time: 0.7,
            zoom_smooth_time: 2.0,
            max_speed: 0.35,
            lost_timeout: 4.0,
            wide_on_lost: true,
            zone: [0.0, 0.0, 1.0, 1.0],
            restore_on_exit: true,
            auto_cut: false,
            cut_wide_shot: "knee".into(),
            cut_min_hold: 6.0,
            cut_move_speed: 0.06,
            reid: true,
            reid_threshold: 0.72,
            watchdog: true,
            watchdog_sound: true,
            watchdog_switch: false,
            backup_scene: String::new(),
            lt_name: String::new(),
            lt_title: String::new(),
            // the plate is shown by hand only (button / N)
            lt_auto: false,
            lt_duration: 7.0,
            lt_delay: 4.0,
            lt_color: "#ff9a8b".into(),
            lt_plate: "dark".into(),
            lt_anim: "wipe".into(),
            lt_anim_speed: 1.0,
        }
    }
}

impl Settings {
    /// Scene -> mode, with old settings migrated.
    pub fn modes(&self) -> IndexMap<String, String> {
        if !self.scene_modes.is_empty() {
            return self.scene_modes.clone();
        }
        if !self.active_scenes.is_empty() {
            return self.active_scenes.iter().map(|s| (s.clone(), "track".to_string())).collect();
        }
        let mut m = IndexMap::new();
        if !self.scene.is_empty() {
            m.insert(self.scene.clone(), "track".to_string());
        }
        m
    }

    pub fn path() -> PathBuf {
        app_data_dir().join("settings.json")
    }

    /// Loads settings field by field: a field with a wrong type keeps its default
    /// instead of throwing the whole file away.
    pub fn load() -> Self {
        let p = Self::path();
        let Ok(text) = std::fs::read_to_string(&p) else {
            return Self::default();
        };
        // Notepad and PowerShell save UTF-8 with a BOM; JSON parsers reject it
        let text = text.trim_start_matches('\u{feff}');
        match serde_json::from_str::<serde_json::Value>(text) {
            Ok(v) => Self::from_value(v),
            Err(e) => {
                log::warn!("Не удалось прочитать настройки: {e}");
                Self::default()
            }
        }
    }

    pub fn from_value(v: serde_json::Value) -> Self {
        let serde_json::Value::Object(data) = v else {
            return Self::default();
        };
        let mut merged = match serde_json::to_value(Self::default()) {
            Ok(serde_json::Value::Object(m)) => m,
            _ => return Self::default(),
        };
        for (k, val) in data {
            if let Some(slot) = merged.get_mut(&k) {
                let old = std::mem::replace(slot, val);
                // check this one field parses; restore the default otherwise
                let probe = serde_json::Value::Object(merged.clone());
                if serde_json::from_value::<Settings>(probe).is_err() {
                    log::warn!("Настройка «{k}» повреждена — оставлено значение по умолчанию");
                    *merged.get_mut(&k).unwrap() = old;
                }
            }
        }
        serde_json::from_value(serde_json::Value::Object(merged)).unwrap_or_default()
    }

    pub fn save(&self) -> std::io::Result<()> {
        // the demo runs on made-up scenes: never overwrite the real settings with them
        if crate::demo::active() {
            return Ok(());
        }
        let p = Self::path();
        let tmp = p.with_extension("tmp");
        let text = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, &p)
    }

    pub fn model_path(&self) -> PathBuf {
        let user = app_data_dir().join("models").join(&self.model);
        if user.exists() {
            return user;
        }
        for dir in resource_dirs() {
            let p = dir.join("models").join(&self.model);
            if p.exists() {
                return p;
            }
        }
        resource_dirs()[0].join("models").join(&self.model)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scene_modes_migrate_old_settings() {
        let s = Settings { scene: "A".into(), ..Default::default() };
        assert_eq!(s.modes().into_iter().collect::<Vec<_>>(), vec![("A".into(), "track".into())]);
        let s = Settings { scene: "A".into(), active_scenes: vec!["B".into(), "C".into()], ..Default::default() };
        assert_eq!(s.modes().keys().cloned().collect::<Vec<_>>(), vec!["B".to_string(), "C".to_string()]);
        let mut modes = IndexMap::new();
        modes.insert("B".to_string(), "duo".to_string());
        let s = Settings { scene_modes: modes, ..Default::default() };
        assert_eq!(s.modes().get("B").map(String::as_str), Some("duo"));
    }

    #[test]
    fn broken_field_keeps_default_and_unknown_is_ignored() {
        let v = serde_json::json!({"obs_port": "oops", "detect_fps": 15, "something_new": 1});
        let s = Settings::from_value(v);
        assert_eq!(s.obs_port, 4455);
        assert_eq!(s.detect_fps, 15);
    }

    #[test]
    fn reads_python_settings_file() {
        let v = serde_json::json!({
            "obs_host": "localhost", "scene_modes": {"Проповедь": "track", "Интервью": "duo"},
            "zone": [0.1, 0.0, 0.9, 1.0], "lt_duration": 5, "fixed_zoom": 2
        });
        let s = Settings::from_value(v);
        assert_eq!(s.modes().keys().next().map(String::as_str), Some("Проповедь"));
        assert_eq!(s.zone, [0.1, 0.0, 0.9, 1.0]);
        assert_eq!(s.lt_duration, 5.0);
    }
}
