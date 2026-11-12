//! Name plate (lower third) in OBS.
//!
//! The page lives in %APPDATA%\VKadre\overlay\lower_third.html (written from the copy built into the exe,
//! together with the fonts). OBS shows it through a Browser Source named «В кадре · Плашка» that we add
//! to each scene where we track the speaker. To show the plate we write the name into the page, turn
//! the source on (the page reloads and plays in), and turn it off after it has played out.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use serde_json::json;

use crate::obs::{ObsApi, ObsConnection, EVENTS_NONE};
use crate::settings::{app_data_dir, Settings};

pub const INPUT_NAME: &str = "В кадре · Плашка";
/// play-out animation + a margin
const OUT_SECONDS: f64 = 2.0;

const TEMPLATE: &str = include_str!("../assets/overlay/lower_third.html");
const FONTS: [(&str, &[u8]); 3] = [
    ("Unbounded-Black.ttf", include_bytes!("../assets/fonts/Unbounded-Black.ttf")),
    ("Manrope-Bold.ttf", include_bytes!("../assets/fonts/Manrope-Bold.ttf")),
    ("Manrope-ExtraBold.ttf", include_bytes!("../assets/fonts/Manrope-ExtraBold.ttf")),
];

pub fn overlay_dir() -> PathBuf {
    let d = app_data_dir().join("overlay");
    let _ = std::fs::create_dir_all(d.join("fonts"));
    d
}

/// Whole on-screen time of one play: in + hold + out, scaled by the animation speed.
pub fn anim_seconds(s: &Settings) -> f64 {
    s.lt_duration + (1.2 + OUT_SECONDS) / s.lt_anim_speed.max(0.25)
}

/// Writes the page with the text and look baked in.
/// preview=true writes a separate looping page with a backdrop, for opening in a browser.
pub fn write_page(s: &Settings, preview: bool) -> Result<PathBuf> {
    write_page_in(&overlay_dir(), s, preview)
}

pub fn write_page_in(dir: &Path, s: &Settings, preview: bool) -> Result<PathBuf> {
    std::fs::create_dir_all(dir.join("fonts"))?;
    for (name, bytes) in FONTS {
        let target = dir.join("fonts").join(name);
        if !target.exists() {
            std::fs::write(target, bytes)?;
        }
    }
    let has_name = !s.lt_name.trim().is_empty();
    let name = if has_name { s.lt_name.trim() } else if preview { "Имя Фамилия" } else { "" };
    let title = if has_name || !preview { s.lt_title.trim() } else { "Подпись" };
    let data = json!({
        "name": name, "title": title, "duration": s.lt_duration, "accent": s.lt_color,
        "plate": s.lt_plate, "anim": s.lt_anim, "speed": s.lt_anim_speed, "loop": preview,
    });
    let block = format!("/*DATA*/var DATA = {data};/*END*/");
    let html = match (TEMPLATE.find("/*DATA*/"), TEMPLATE.find("/*END*/")) {
        (Some(a), Some(b)) if b > a => format!("{}{}{}", &TEMPLATE[..a], block, &TEMPLATE[b + "/*END*/".len()..]),
        _ => TEMPLATE.to_string(),
    };
    let page = dir.join(if preview { "lower_third_preview.html" } else { "lower_third.html" });
    std::fs::write(&page, html)?;
    Ok(page)
}

/// Shows the plate in OBS from a background thread, so the UI never waits for OBS.
#[derive(Default, Clone)]
pub struct LowerThird {
    busy: Arc<AtomicBool>,
}

impl LowerThird {
    /// Show the plate in `scene` (None = the scene that is on air now).
    /// Returns false when there is no name or the plate is already on screen.
    pub fn show(&self, s: Settings, scene: Option<String>, on_error: impl FnOnce(String) + Send + 'static) -> bool {
        if s.lt_name.trim().is_empty() {
            return false;
        }
        if self.busy.swap(true, Ordering::SeqCst) {
            return false; // already on screen
        }
        let busy = self.busy.clone();
        std::thread::Builder::new()
            .name("lower-third".into())
            .spawn(move || {
                if let Err(e) = play(&s, scene) {
                    log::warn!("Плашка: {e:#}");
                    on_error(format!("Не удалось показать плашку: {e}"));
                }
                busy.store(false, Ordering::SeqCst);
            })
            .is_ok()
    }
}

fn play(s: &Settings, scene: Option<String>) -> Result<()> {
    let page = write_page(s, false)?;
    let mut conn = ObsConnection::connect(&s.obs_host, s.obs_port, &s.obs_password, Duration::from_secs(5), EVENTS_NONE)?;
    let scene = match scene {
        Some(sc) => sc,
        None => conn.program_scene()?.unwrap_or_default(),
    };
    let item_id = ensure(&mut conn, &scene, &page)?;
    conn.refresh_browser(INPUT_NAME);
    std::thread::sleep(Duration::from_millis(400));
    conn.set_item_enabled(&scene, item_id, true)?;
    std::thread::sleep(Duration::from_secs_f64(anim_seconds(s)));
    conn.set_item_enabled(&scene, item_id, false)?;
    conn.close();
    Ok(())
}

/// Browser source exists, points at our page and sits on top of `scene`.
fn ensure(conn: &mut ObsConnection, scene: &str, page: &Path) -> Result<i64> {
    let settings = json!({
        "is_local_file": true, "local_file": page.display().to_string(), "width": 1920, "height": 1080,
        "shutdown": true, "restart_when_active": true, "reroute_audio": false, "css": "",
    });
    let item_id = if conn.input_names()?.iter().any(|n| n == INPUT_NAME) {
        conn.set_input_settings(INPUT_NAME, settings)?;
        if conn.scene_sources(scene)?.iter().any(|n| n == INPUT_NAME) {
            conn.item_id(scene, INPUT_NAME)?
        } else {
            conn.create_scene_item(scene, INPUT_NAME, false)?
        }
    } else {
        conn.create_input(scene, INPUT_NAME, "browser_source", settings, false)?
    };
    let (cw, ch) = conn.canvas_size()?;
    conn.set_item_transform(
        scene,
        item_id,
        json!({
            "positionX": 0.0, "positionY": 0.0, "alignment": 5, "rotation": 0.0,
            "boundsType": "OBS_BOUNDS_STRETCH", "boundsAlignment": 0,
            "boundsWidth": cw as f64, "boundsHeight": ch as f64,
        }),
    )?;
    conn.move_to_top(scene, item_id)?;
    Ok(item_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_has_data_and_fonts() {
        let dir = std::env::temp_dir().join(format!("vkadre-test-{}", std::process::id()));
        let s = Settings {
            lt_name: "Иван «Ваня» Петров".into(),
            lt_title: "Пастор".into(),
            lt_duration: 5.0,
            lt_color: "#c6f36b".into(),
            lt_plate: "light".into(),
            lt_anim: "slide".into(),
            lt_anim_speed: 1.5,
            ..Default::default()
        };
        let page = write_page_in(&dir, &s, false).unwrap();
        let html = std::fs::read_to_string(&page).unwrap();
        assert!(html.contains(r#""name":"Иван «Ваня» Петров""#), "{}", &html[..200.min(html.len())]);
        assert!(html.contains(r##""accent":"#c6f36b""##) && html.contains(r#""plate":"light""#));
        assert!(html.contains(r#""anim":"slide""#) && html.contains(r#""loop":false"#));
        assert_eq!(html.matches("/*DATA*/").count(), 1);
        assert!(dir.join("fonts").join("Unbounded-Black.ttf").exists());
        let preview = write_page_in(&dir, &s, true).unwrap();
        assert_eq!(preview.file_name().unwrap(), "lower_third_preview.html");
        assert!(std::fs::read_to_string(preview).unwrap().contains(r#""loop":true"#));
        let _ = std::fs::remove_dir_all(dir);
    }
}
