//! Glue between the Slint window and the engine: settings in and out, engine events, OBS lists.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use anyhow::Result;
use slint::{Color, ComponentHandle, Image, Model, ModelRc, Rgb8Pixel, SharedPixelBuffer, SharedString, Timer, TimerMode, VecModel};

use crate::engine::{Emit, Engine, Event, PreviewData};
use crate::framing::{HEAD_KP, HIP_KP, SHOULDER_KP};
use crate::i18n::{self, tr};
use crate::lower_third::{anim_seconds, write_page, LowerThird};
use crate::obs::{ObsConnection, EVENTS_NONE};
use crate::settings::{Settings, VERSION};
use crate::{AppWindow, Cfg, Dot, Fmt, PersonBox, SceneRow, I18n};

const SHOT_CHOICES: [(&str, &str); 5] =
    [("auto", "chest"), ("auto", "waist"), ("auto", "hips"), ("auto", "knee"), ("fixed", "")];
const WIDE_SHOTS: [&str; 3] = ["hips", "knee", "full"];
const SCENE_MODES: [&str; 4] = ["", "track", "duo", "wide"];
const PLATE_COLORS: [&str; 6] = ["#ff9a8b", "#c6f36b", "#ffd84d", "#7cc8ff", "#c9a7ff", "#f4f1ef"];
const PLATE_STYLES: [&str; 3] = ["dark", "light", "color"];
const PLATE_ANIMS: [&str; 4] = ["wipe", "slide", "rise", "fade"];

const IDLE_TITLE: &str = "Слежение не запущено";
const IDLE_HINT: &str = "Нажмите «Включить слежение» или F5";
const NO_OBS_TITLE: &str = "Нет связи с OBS";
const NO_OBS_HINT: &str =
    "Запустите OBS и включите WebSocket: Инструменты → Настройки сервера WebSocket. Затем нажмите «Включить слежение» или F5.";

fn hex(c: &str) -> Color {
    parse_hex(c).unwrap_or(Color::from_rgb_u8(255, 255, 255))
}

fn parse_hex(c: &str) -> Option<Color> {
    let c = c.trim().trim_start_matches('#');
    if c.len() != 6 || !c.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return None;
    }
    let v = u32::from_str_radix(c, 16).ok()?;
    Some(Color::from_rgb_u8((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

mod col {
    use super::hex;
    use slint::Color;
    pub fn accent() -> Color { hex("#ff9a8b") }
    pub fn faint() -> Color { hex("#6e6e6e") }
    pub fn muted() -> Color { hex("#8c8c8c") }
    pub fn warn() -> Color { hex("#ffb547") }
    pub fn danger() -> Color { hex("#ff5a5a") }
}

/// OBS version, scenes (as the user sees them), scene -> sources, input -> kind.
type ObsLists = (String, Vec<String>, HashMap<String, Vec<String>>, HashMap<String, String>);

/// How likely a source is the camera to follow: 0 = a capture device, 5 = surely not.
fn camera_rank(kind: Option<&str>) -> u8 {
    match kind {
        // capture cards, USB cameras, DeckLink (Windows / macOS / Linux)
        Some("dshow_input" | "av_capture_input" | "macos-avcapture" | "v4l2_input" | "decklink-input") => 0,
        // network cameras and NDI feeds
        Some(k) if k.contains("ndi") => 1,
        // video files and streams
        Some("ffmpeg_source" | "vlc_source") => 2,
        // a screen or a window may show a camera app, but rarely
        Some("window_capture" | "game_capture" | "monitor_capture" | "screen_capture") => 3,
        Some("image_source" | "slideshow" | "browser_source" | "color_source" | "color_source_v3" | "text_gdiplus" | "text_ft2_source") => 5,
        // unknown inputs and nested scenes
        _ => 4,
    }
}

/// The source to switch to, if any: the best-ranked one when nothing usable is chosen, or when the app
/// chose the current one itself and a real camera (or a video) has appeared since. A person's choice stays.
fn pick_camera(current: &str, auto: bool, sources: &[String], kinds: &HashMap<String, String>) -> Option<String> {
    let rank = |s: &str| camera_rank(kinds.get(s).map(String::as_str));
    let best = sources.iter().min_by_key(|s| rank(s))?;
    let present = sources.iter().any(|s| s == current);
    if !present || (auto && rank(best) <= 2 && rank(best) < rank(current)) {
        (best != current).then(|| best.clone())
    } else {
        None
    }
}

fn strings(v: &[String]) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(v.iter().map(SharedString::from).collect::<Vec<_>>()))
}

fn beep() {
    #[cfg(windows)]
    {
        #[link(name = "user32")]
        extern "system" {
            fn MessageBeep(kind: u32) -> i32;
        }
        // MB_ICONHAND
        unsafe {
            MessageBeep(0x10);
        }
    }
}

fn format_num(v: f32, fmt: &str, en: bool) -> String {
    let v = v as f64;
    let sec = if en { "s" } else { "с" };
    match fmt {
        "x1" => format!("{v:.1}×"),
        "pct0" => format!("{:.0}%", v * 100.0),
        "s0" => format!("{v:.0} {sec}"),
        "s1" => format!("{v:.1} {sec}"),
        "s2" => format!("{v:.2} {sec}"),
        _ => format!("{v:.2}"),
    }
}

/// UI-thread state. Lives in a thread-local so engine events (posted to the event loop) can reach it.
struct App {
    ui: slint::Weak<AppWindow>,
    settings: Arc<RwLock<Settings>>,
    engine: Arc<Engine>,
    plate: LowerThird,
    save_timer: Timer,
    status_timer: Timer,
    beep_timer: Timer,
    idle_status: RefCell<String>,
    scenes: RefCell<Vec<String>>,
    scene_sources: RefCell<HashMap<String, Vec<String>>>,
    /// rows currently shown in «Режим в сценах»
    shown_scenes: RefCell<Vec<String>>,
    fault: RefCell<String>,
    running: RefCell<bool>,
    /// untranslated texts, redrawn on a language switch
    message: RefCell<(String, String, i32)>,
    obs_text: RefCell<String>,
}

thread_local! {
    static APP: RefCell<Option<Rc<App>>> = const { RefCell::new(None) };
}

fn with_app(f: impl FnOnce(&Rc<App>)) {
    let app = APP.with(|a| a.borrow().clone());
    if let Some(app) = app {
        f(&app);
    }
}

pub fn run(mut settings: Settings) -> Result<()> {
    // the name plate is shown by hand only
    settings.lt_auto = false;
    let ui = AppWindow::new()?;
    ui.global::<Fmt>().on_num(|v, f, en| format_num(v, f.as_str(), en).into());
    ui.global::<I18n>().on_lookup(|s| i18n::to_english(s.as_str()).into());
    // `--lang en` / `--lang ru` overrides the saved language for this run only
    let args: Vec<String> = std::env::args().collect();
    let lang_arg = args.iter().position(|a| a == "--lang").and_then(|i| args.get(i + 1)).cloned();
    let en = lang_arg.as_deref().map(|l| l == "en").unwrap_or(settings.language == "en");
    i18n::set_english(en);
    ui.global::<I18n>().set_en(en);
    ui.set_version(VERSION.into());
    ui.set_message_title(tr(NO_OBS_TITLE).into());
    ui.set_message_hint(tr(NO_OBS_HINT).into());
    ui.set_empty_kind(1);
    ui.set_obs_text(tr("OBS не отвечает").into());

    let settings = Arc::new(RwLock::new(settings));
    let emit: Emit = Arc::new(|ev: Event| {
        let _ = slint::invoke_from_event_loop(move || with_app(|app| app.on_event(ev)));
    });
    let engine = Arc::new(Engine::new(settings.clone(), emit));

    let app = Rc::new(App {
        ui: ui.as_weak(),
        settings,
        engine,
        plate: LowerThird::default(),
        save_timer: Timer::default(),
        status_timer: Timer::default(),
        beep_timer: Timer::default(),
        idle_status: RefCell::new("Готово".into()),
        scenes: RefCell::new(Vec::new()),
        scene_sources: RefCell::new(HashMap::new()),
        shown_scenes: RefCell::new(Vec::new()),
        fault: RefCell::new(String::new()),
        running: RefCell::new(false),
        message: RefCell::new((NO_OBS_TITLE.into(), NO_OBS_HINT.into(), 1)),
        obs_text: RefCell::new("OBS не отвечает".into()),
    });
    APP.with(|a| *a.borrow_mut() = Some(app.clone()));

    app.load_into_ui();
    app.wire();
    app.status("Готово", 0, None);
    app.update_tally();
    // debug / screenshots: `--tab N` opens a settings tab, `--open-settings` the settings dialog
    if let Some(tab) = args.iter().position(|a| a == "--tab").and_then(|i| args.get(i + 1)).and_then(|t| t.parse().ok()) {
        ui.set_tab(tab);
    }
    if args.iter().any(|a| a == "--open-settings") {
        ui.invoke_open_settings();
    }
    if args.iter().any(|a| a == "--plate-play") {
        ui.set_tab(3);
        ui.set_plate_playing(true);
    }

    let (auto_connect, auto_start, minimized) = {
        let s = app.settings.read().unwrap();
        (s.auto_connect, s.auto_start, s.start_minimized)
    };
    if auto_connect || auto_start {
        let a = app.clone();
        Timer::single_shot(Duration::from_millis(300), move || a.refresh_obs());
    }
    if auto_start {
        // the engine connects by itself; start once the window is up
        let a = app.clone();
        Timer::single_shot(Duration::from_millis(800), move || {
            if !a.engine.running() {
                a.toggle_running();
            }
        });
    }
    if minimized {
        let w = ui.as_weak();
        Timer::single_shot(Duration::from_millis(50), move || {
            if let Some(ui) = w.upgrade() {
                ui.window().set_minimized(true);
            }
        });
    }

    let a = app.clone();
    ui.window().on_close_requested(move || {
        a.engine.stop_and_wait();
        a.save();
        slint::CloseRequestResponse::HideWindow
    });

    ui.run()?;
    app.engine.stop_and_wait();
    app.save();
    APP.with(|a| *a.borrow_mut() = None);
    Ok(())
}

impl App {
    fn ui(&self) -> AppWindow {
        self.ui.upgrade().expect("window alive")
    }

    fn s(&self) -> Settings {
        self.settings.read().unwrap().clone()
    }

    // ------------------------------------------------------------------ settings <-> UI

    fn load_into_ui(&self) {
        let ui = self.ui();
        let cfg = ui.global::<Cfg>();
        let s = self.s();
        let shot = if s.zoom_mode == "fixed" { 4 } else { SHOT_CHOICES.iter().position(|c| c.1 == s.shot).unwrap_or(1) };
        cfg.set_shot_index(shot as i32);
        cfg.set_fixed_zoom(s.fixed_zoom as f32);
        cfg.set_max_zoom(s.max_zoom as f32);
        cfg.set_headroom(s.headroom as f32);
        cfg.set_auto_cut(s.auto_cut);
        cfg.set_cut_index(WIDE_SHOTS.iter().position(|w| *w == s.cut_wide_shot).unwrap_or(1) as i32);
        cfg.set_cut_hold(s.cut_min_hold as f32);
        cfg.set_smooth(s.smooth_time as f32);
        cfg.set_deadzone(s.deadzone_x as f32);
        cfg.set_speed(s.max_speed as f32);
        cfg.set_zoom_smooth(s.zoom_smooth_time as f32);
        cfg.set_reid(s.reid);
        cfg.set_wide_on_lost(s.wide_on_lost);
        cfg.set_lost_timeout(s.lost_timeout as f32);
        cfg.set_lt_name(s.lt_name.as_str().into());
        cfg.set_lt_name_upper(s.lt_name.trim().to_uppercase().into());
        cfg.set_lt_title(s.lt_title.as_str().into());
        cfg.set_lt_duration(s.lt_duration as f32);
        cfg.set_lt_swatches(ModelRc::new(VecModel::from(PLATE_COLORS.iter().map(|c| hex(c)).collect::<Vec<_>>())));
        let color = s.lt_color.to_lowercase();
        match PLATE_COLORS.iter().position(|c| *c == color) {
            Some(i) => cfg.set_lt_color_index(i as i32),
            None => {
                cfg.set_lt_custom_hex(color.as_str().into());
                cfg.set_lt_custom_color(hex(&color));
                cfg.set_lt_color_index(PLATE_COLORS.len() as i32);
            }
        }
        cfg.set_lt_plate_index(PLATE_STYLES.iter().position(|p| *p == s.lt_plate).unwrap_or(0) as i32);
        cfg.set_lt_anim_index(PLATE_ANIMS.iter().position(|p| *p == s.lt_anim).unwrap_or(0) as i32);
        cfg.set_lt_speed(s.lt_anim_speed as f32);
        cfg.set_watchdog(s.watchdog);
        cfg.set_watchdog_sound(s.watchdog_sound);
        cfg.set_watchdog_switch(s.watchdog_switch);
        if !s.backup_scene.is_empty() {
            cfg.set_backup_scenes(strings(std::slice::from_ref(&s.backup_scene)));
            cfg.set_backup_index(0);
        }
        cfg.set_use_gpu(s.use_gpu);
        cfg.set_detect_fps(s.detect_fps as i32);
        if !s.source.is_empty() {
            cfg.set_sources(strings(std::slice::from_ref(&s.source)));
            cfg.set_source_index(0);
        }
        self.fill_modes();
        self.update_source_meta();
        let [x0, y0, x1, y1] = s.zone;
        ui.set_zone_x(x0 as f32);
        ui.set_zone_y(y0 as f32);
        ui.set_zone_w((x1 - x0) as f32);
        ui.set_zone_h((y1 - y0) as f32);
    }

    /// Reads every control back into the settings (the panel has no per-field wiring).
    fn read_from_ui(&self) {
        let ui = self.ui();
        let cfg = ui.global::<Cfg>();
        let mut s = self.settings.write().unwrap();
        let (mode, shot) = SHOT_CHOICES[(cfg.get_shot_index().clamp(0, 4)) as usize];
        s.zoom_mode = mode.into();
        if !shot.is_empty() {
            s.shot = shot.into();
        }
        s.fixed_zoom = cfg.get_fixed_zoom() as f64;
        s.max_zoom = cfg.get_max_zoom() as f64;
        s.headroom = cfg.get_headroom() as f64;
        s.auto_cut = cfg.get_auto_cut();
        s.cut_wide_shot = WIDE_SHOTS[(cfg.get_cut_index().clamp(0, 2)) as usize].into();
        s.cut_min_hold = cfg.get_cut_hold() as f64;
        s.smooth_time = cfg.get_smooth() as f64;
        s.deadzone_x = cfg.get_deadzone() as f64;
        s.max_speed = cfg.get_speed() as f64;
        s.zoom_smooth_time = cfg.get_zoom_smooth() as f64;
        s.reid = cfg.get_reid();
        s.wide_on_lost = cfg.get_wide_on_lost();
        s.lost_timeout = cfg.get_lost_timeout() as f64;
        s.lt_name = cfg.get_lt_name().to_string();
        cfg.set_lt_name_upper(s.lt_name.trim().to_uppercase().into());
        s.lt_title = cfg.get_lt_title().to_string();
        s.lt_duration = cfg.get_lt_duration() as f64;
        let ci = cfg.get_lt_color_index();
        s.lt_color = if (0..PLATE_COLORS.len() as i32).contains(&ci) {
            PLATE_COLORS[ci as usize].into()
        } else {
            let h = cfg.get_lt_custom_hex().to_string();
            if parse_hex(&h).is_some() { h } else { s.lt_color.clone() }
        };
        s.lt_plate = PLATE_STYLES[(cfg.get_lt_plate_index().clamp(0, 2)) as usize].into();
        s.lt_anim = PLATE_ANIMS[(cfg.get_lt_anim_index().clamp(0, 3)) as usize].into();
        s.lt_anim_speed = cfg.get_lt_speed() as f64;
        s.watchdog = cfg.get_watchdog();
        s.watchdog_sound = cfg.get_watchdog_sound();
        s.watchdog_switch = cfg.get_watchdog_switch();
        let bi = cfg.get_backup_index();
        if bi >= 0 {
            if let Some(name) = cfg.get_backup_scenes().row_data(bi as usize) {
                s.backup_scene = name.to_string();
            }
        }
        s.use_gpu = cfg.get_use_gpu();
        s.detect_fps = cfg.get_detect_fps().clamp(2, 30) as u32;
    }

    fn changed(&self) {
        self.read_from_ui();
        let s = self.settings.clone();
        // save a moment after the last change, not on every slider step
        self.save_timer.start(TimerMode::SingleShot, Duration::from_millis(600), move || {
            if let Err(e) = s.read().unwrap().save() {
                log::warn!("Не удалось сохранить настройки: {e}");
            }
        });
    }

    fn save(&self) {
        if let Err(e) = self.settings.read().unwrap().save() {
            log::warn!("Не удалось сохранить настройки: {e}");
        }
    }

    // ------------------------------------------------------------------ wiring

    fn wire(self: &Rc<Self>) {
        let ui = self.ui();
        let cfg = ui.global::<Cfg>();

        let a = self.clone();
        cfg.on_changed(move || a.changed());
        let a = self.clone();
        cfg.on_source_selected(move |i| a.on_source(i));
        let a = self.clone();
        cfg.on_scene_mode_changed(move |row, mode| a.on_scene_mode(row, mode));
        let a = self.clone();
        cfg.on_custom_hex_edited(move |text| {
            let ui = a.ui();
            let cfg = ui.global::<Cfg>();
            if let Some(c) = parse_hex(&text) {
                let h = format!("#{}", text.trim().trim_start_matches('#').to_lowercase());
                cfg.set_lt_custom_hex(h.into());
                cfg.set_lt_custom_color(c);
                cfg.set_lt_color_index(PLATE_COLORS.len() as i32);
                a.changed();
            }
        });
        let a = self.clone();
        cfg.on_refresh(move || a.refresh_obs());
        let a = self.clone();
        cfg.on_show_plate(move || a.show_plate(None));
        let a = self.clone();
        cfg.on_preview_plate(move || a.preview_plate());

        let a = self.clone();
        ui.on_toggle_run(move || a.toggle_running());
        let a = self.clone();
        ui.on_wide_toggled(move |on| a.engine.set_paused(on));
        let a = self.clone();
        ui.on_retarget(move || a.engine.retarget());
        let a = self.clone();
        ui.on_zone_reset(move || a.set_zone([0.0, 0.0, 1.0, 1.0]));
        let a = self.clone();
        ui.on_show_plate(move || a.show_plate(None));
        let a = self.clone();
        ui.on_preview_clicked(move |x, y| a.on_preview_click(x as f64, y as f64));
        let a = self.clone();
        ui.on_zone_drawn(move |x0, y0, x1, y1| {
            let r = |v: f32| (v as f64 * 1000.0).round() / 1000.0;
            a.set_zone([r(x0), r(y0), r(x1), r(y1)]);
            a.engine.retarget();
        });
        let a = self.clone();
        ui.on_open_settings(move || {
            let ui = a.ui();
            let s = a.s();
            ui.set_set_lang(if s.language == "en" { 1 } else { 0 });
            ui.set_conn_host(s.obs_host.as_str().into());
            ui.set_conn_port(s.obs_port as i32);
            ui.set_conn_password(s.obs_password.as_str().into());
            ui.set_conn_auto(s.auto_connect);
            ui.set_set_auto_start(s.auto_start);
            ui.set_set_minimized(s.start_minimized);
            ui.set_conn_result("".into());
            ui.set_show_settings(true);
        });
        // the language switches at once, so the dialog itself shows it
        let a = self.clone();
        ui.on_lang_changed(move |i| a.set_language(i == 1));
        let a = self.clone();
        ui.on_conn_test(move || a.test_connection());
        let a = self.clone();
        ui.on_settings_cancel(move || {
            // back to the saved language if it was only tried
            let en = a.settings.read().unwrap().language == "en";
            a.set_language(en);
            a.ui().set_show_settings(false);
        });
        let a = self.clone();
        ui.on_settings_save(move || {
            let ui = a.ui();
            let reconnect = {
                let mut s = a.settings.write().unwrap();
                let host = ui.get_conn_host().trim().to_string();
                let host = if host.is_empty() { "localhost".to_string() } else { host };
                let port = ui.get_conn_port().clamp(1, 65535) as u16;
                let password = ui.get_conn_password().to_string();
                let reconnect = host != s.obs_host || port != s.obs_port || password != s.obs_password;
                s.obs_host = host;
                s.obs_port = port;
                s.obs_password = password;
                s.auto_connect = ui.get_conn_auto();
                s.auto_start = ui.get_set_auto_start();
                s.start_minimized = ui.get_set_minimized();
                s.language = if ui.get_set_lang() == 1 { "en".into() } else { "ru".into() };
                reconnect
            };
            ui.set_show_settings(false);
            a.save();
            if reconnect || !ui.get_obs_ok() {
                a.refresh_obs();
            }
        });
    }

    /// Switches the interface language now; texts set from Rust are redrawn too.
    fn set_language(&self, en: bool) {
        i18n::set_english(en);
        let ui = self.ui();
        ui.global::<I18n>().set_en(en);
        let (title, hint, _) = self.message.borrow().clone();
        ui.set_message_title(tr(&title).into());
        ui.set_message_hint(tr(&hint).into());
        let idle = self.idle_status.borrow().clone();
        self.status(&idle, 0, None);
        self.update_tally();
        self.fill_modes();
        let obs = self.obs_text.borrow().clone();
        ui.set_obs_text(tr(&obs).into());
    }

    /// Big text over the camera monitor (kept untranslated so a language switch can redo it).
    /// kind: 0 plain, 1 no OBS (reconnect / how to), 2 ready to start.
    fn set_message(&self, title: &str, hint: &str, kind: i32) {
        *self.message.borrow_mut() = (title.to_string(), hint.to_string(), kind);
        let ui = self.ui();
        ui.set_message_title(tr(title).into());
        ui.set_message_hint(tr(hint).into());
        ui.set_empty_kind(kind);
    }

    /// What the stopped monitor says: nothing works without OBS, otherwise it is ready to start.
    fn idle_message(&self) {
        if *self.running.borrow() {
            return;
        }
        if self.ui().get_obs_ok() {
            self.set_message(IDLE_TITLE, IDLE_HINT, 2);
        } else {
            self.set_message(NO_OBS_TITLE, NO_OBS_HINT, 1);
        }
    }

    fn set_obs_text(&self, text: &str) {
        *self.obs_text.borrow_mut() = text.to_string();
        self.ui().set_obs_text(tr(text).into());
    }

    // ------------------------------------------------------------------ helpers

    /// Footer status: ● TEXT. With a timeout, falls back to the idle text afterwards.
    fn status(&self, text: &str, timeout_ms: u64, color: Option<Color>) {
        let ui = self.ui();
        let dot = color.unwrap_or_else(|| if *self.running.borrow() { col::accent() } else { col::faint() });
        ui.set_status_text(tr(text).into());
        ui.set_status_color(dot);
        if timeout_ms > 0 {
            let w = self.ui.clone();
            self.status_timer.start(TimerMode::SingleShot, Duration::from_millis(timeout_ms), move || {
                with_app(|a| {
                    let idle = a.idle_status.borrow().clone();
                    a.status(&idle, 0, None);
                });
                let _ = w;
            });
        } else {
            self.status_timer.stop();
            *self.idle_status.borrow_mut() = text.to_string();
        }
    }

    fn update_source_meta(&self) {
        let s = self.s();
        self.ui().set_source_name(s.source.as_str().into());
    }

    /// Top-bar tally: stopped / waiting for a scene / following a scene.
    fn update_tally(&self) {
        let ui = self.ui();
        if !*self.running.borrow() {
            ui.set_tally_state(0);
            ui.set_tally_text(tr("Слежение остановлено").into());
        } else if let Some(scene) = self.engine.active_scene() {
            ui.set_tally_state(2);
            ui.set_tally_text(format!("{} · {scene}", tr("Ведёт")).into());
        } else {
            ui.set_tally_state(1);
            ui.set_tally_text(tr("Ждёт сцену").into());
        }
    }

    fn set_zone(&self, z: [f64; 4]) {
        self.settings.write().unwrap().zone = z;
        let ui = self.ui();
        ui.set_zone_x(z[0] as f32);
        ui.set_zone_y(z[1] as f32);
        ui.set_zone_w((z[2] - z[0]) as f32);
        ui.set_zone_h((z[3] - z[1]) as f32);
        self.save();
    }

    fn preview_plate(&self) {
        match write_page(&self.s(), true) {
            Ok(page) => {
                let _ = std::process::Command::new("explorer").arg(page).spawn();
            }
            Err(e) => self.status(&format!("Не удалось открыть превью: {e}"), 6000, Some(col::danger())),
        }
    }

    // ------------------------------------------------------------------ OBS lists

    /// Talks to OBS on a background thread; the window never waits for it.
    fn refresh_obs(&self) {
        let s = self.s();
        std::thread::spawn(move || {
            let r = (|| -> Result<ObsLists> {
                if crate::demo::active() {
                    return Ok(crate::demo::lists());
                }
                let mut c = ObsConnection::connect(&s.obs_host, s.obs_port, &s.obs_password, Duration::from_secs(3), EVENTS_NONE)?;
                let version = c.version()?;
                let scenes = c.scenes()?;
                let mut map = HashMap::new();
                for sc in &scenes {
                    map.insert(sc.clone(), c.scene_sources(sc)?);
                }
                let kinds = c.input_kinds().unwrap_or_default();
                c.close();
                Ok((version, scenes, map, kinds))
            })();
            let r = r.map_err(|e| format!("{e:#}"));
            let _ = slint::invoke_from_event_loop(move || with_app(|a| a.apply_obs_lists(r)));
        });
    }

    fn apply_obs_lists(&self, r: std::result::Result<ObsLists, String>) {
        let ui = self.ui();
        let (version, scenes, map, kinds) = match r {
            Ok(v) => v,
            Err(e) => {
                log::warn!("OBS: {e}");
                ui.set_obs_ok(false);
                // say what is actually wrong: a password problem is not "OBS is not running"
                let (short, long) = if e.contains("пароль") {
                    ("OBS: нужен пароль", format!("{e}. Кнопка подключения — справа вверху."))
                } else if e.contains("не отвечает") {
                    ("OBS не отвечает", "Нет связи с OBS — запущен ли OBS и включён ли WebSocket?".to_string())
                } else {
                    ("OBS: ошибка связи", format!("OBS: {e}"))
                };
                self.set_obs_text(short);
                self.status(&long, 0, Some(col::danger()));
                self.idle_message();
                return;
            }
        };
        let s = self.s();
        let _ = &s;
        self.set_obs_text(&version);
        ui.set_obs_ok(true);
        self.idle_message();
        // clear a stale connection error
        if !*self.running.borrow() {
            self.status(&format!("Подключено к {version}"), 0, None);
        }

        // every source that sits in some scene, cameras first, without the name plate
        let mut sources: Vec<String> = Vec::new();
        for sc in &scenes {
            for src in map.get(sc).into_iter().flatten() {
                if !sources.contains(src) && !src.starts_with("В кадре ·") {
                    sources.push(src.clone());
                }
            }
        }
        sources.sort_by_key(|src| camera_rank(kinds.get(src).map(String::as_str)));
        let cfg = ui.global::<Cfg>();
        let mut picked = None;
        {
            let mut s = self.settings.write().unwrap();
            if let Some(best) = pick_camera(&s.source, s.source_auto, &sources, &kinds) {
                s.source = best.clone();
                s.source_auto = true;
                picked = Some(best);
            }
            let idx = sources.iter().position(|x| *x == s.source).map(|i| i as i32).unwrap_or(-1);
            cfg.set_sources(strings(&sources));
            cfg.set_source_index(idx);

            cfg.set_backup_scenes(strings(&scenes));
            cfg.set_backup_index(scenes.iter().position(|x| *x == s.backup_scene).map(|i| i as i32).unwrap_or(-1));
        }
        *self.scenes.borrow_mut() = scenes;
        *self.scene_sources.borrow_mut() = map;
        if let Some(name) = picked {
            self.save();
            if !*self.running.borrow() {
                self.status(&format!("Подключено к {version} · камера: {name}"), 0, None);
            }
        }
        self.fill_modes();
        self.update_source_meta();
    }

    /// Every OBS scene on the bus; scenes without the chosen camera are shown dimmed
    /// (before OBS is reached: the saved ones).
    fn fill_modes(&self) {
        let s = self.s();
        let modes = s.modes();
        let map = self.scene_sources.borrow();
        let scenes: Vec<String> =
            if map.is_empty() { modes.keys().cloned().collect() } else { self.scenes.borrow().clone() };
        let on_air = if *self.running.borrow() { self.engine.active_scene() } else { None };
        let rows: Vec<SceneRow> = scenes
            .iter()
            .enumerate()
            .map(|(i, sc)| {
                let m = modes.get(sc).map(String::as_str).unwrap_or("");
                SceneRow {
                    index: i as i32 + 1,
                    name: sc.as_str().into(),
                    mode: SCENE_MODES.iter().position(|x| *x == m).unwrap_or(0) as i32,
                    on_air: on_air.as_deref() == Some(sc.as_str()),
                    has_camera: map.is_empty() || map.get(sc).is_some_and(|v| v.contains(&s.source)),
                }
            })
            .collect();
        let ui = self.ui();
        let cfg = ui.global::<Cfg>();
        cfg.set_scenes_empty_text(
            tr(if map.is_empty() { "Подключитесь к OBS, чтобы увидеть сцены" } else { "В OBS нет сцен" }).into(),
        );
        cfg.set_scene_rows(ModelRc::new(VecModel::from(rows)));
        *self.shown_scenes.borrow_mut() = scenes;
    }

    fn on_source(&self, i: i32) {
        let ui = self.ui();
        let Some(name) = ui.global::<Cfg>().get_sources().row_data(i.max(0) as usize) else { return };
        {
            // the person chose it: never replace it automatically
            let mut s = self.settings.write().unwrap();
            s.source = name.to_string();
            s.source_auto = false;
        }
        self.changed();
        self.fill_modes();
        self.update_source_meta();
    }

    fn on_scene_mode(&self, row: i32, mode: i32) {
        let Some(scene) = self.shown_scenes.borrow().get(row as usize).cloned() else { return };
        {
            let mut s = self.settings.write().unwrap();
            let mut modes = s.modes();
            let m = SCENE_MODES[mode.clamp(0, 3) as usize];
            if m.is_empty() {
                modes.shift_remove(&scene);
            } else {
                modes.insert(scene.clone(), m.to_string());
            }
            s.scene = modes
                .iter()
                .find(|(_, m)| m.as_str() != "wide")
                .or_else(|| modes.iter().next())
                .map(|(k, _)| k.clone())
                .unwrap_or_default();
            s.scene_modes = modes;
            s.active_scenes.clear();
        }
        self.save();
        self.fill_modes();
        self.update_source_meta();
    }

    // ------------------------------------------------------------------ actions

    fn toggle_running(&self) {
        if self.engine.running() {
            self.engine.stop();
        } else {
            self.read_from_ui();
            self.save();
            self.set_message("Запуск", "Загружаю нейросеть и подключаюсь к OBS…", 0);
            self.engine.start();
        }
    }

    fn test_connection(&self) {
        let ui = self.ui();
        ui.set_conn_result(tr("Проверяю…").into());
        ui.set_conn_result_color(col::muted());
        let host = ui.get_conn_host().trim().to_string();
        let host = if host.is_empty() { "localhost".to_string() } else { host };
        let port = ui.get_conn_port().clamp(1, 65535) as u16;
        let pw = ui.get_conn_password().to_string();
        std::thread::spawn(move || {
            let r = ObsConnection::connect(&host, port, &pw, Duration::from_secs(3), EVENTS_NONE).and_then(|mut c| {
                let v = c.version();
                c.close();
                v
            });
            let _ = slint::invoke_from_event_loop(move || {
                with_app(|a| {
                    let ui = a.ui();
                    match r {
                        Ok(v) => {
                            ui.set_conn_result(tr(&format!("Подключено: {v}")).into());
                            ui.set_conn_result_color(col::accent());
                        }
                        Err(e) => {
                            ui.set_conn_result(tr(&format!("Не удалось подключиться: {e}")).into());
                            ui.set_conn_result_color(col::danger());
                        }
                    }
                })
            });
        });
    }

    fn show_plate(&self, scene: Option<String>) {
        let s = self.s();
        if s.lt_name.trim().is_empty() {
            self.status("Впишите имя для плашки (вкладка «Плашка»)", 5000, Some(col::warn()));
            return;
        }
        let scene = scene.or_else(|| self.engine.active_scene());
        let on_error = |text: String| {
            let _ = slint::invoke_from_event_loop(move || with_app(|a| a.status(&text, 6000, Some(col::danger()))));
        };
        if self.plate.show(s.clone(), scene, on_error) {
            self.status(&format!("Плашка: {}", s.lt_name.trim()), (anim_seconds(&s) * 1000.0) as u64, None);
        }
    }

    fn on_preview_click(&self, x: f64, y: f64) {
        if self.engine.running() && self.engine.lock_at(x, y) {
            let ui = self.ui();
            if ui.get_wide_checked() {
                ui.set_wide_checked(false);
            }
            let second = self.engine.mode() == "duo" && self.engine.partner_id().is_some();
            self.status(if second { "Второй человек выбран" } else { "Цель выбрана" }, 2000, None);
        }
    }

    // ------------------------------------------------------------------ engine events

    fn on_event(&self, ev: Event) {
        match ev {
            Event::Preview(d) => self.on_preview(*d),
            Event::Status(t) => self.status(&t, 0, None),
            Event::Running(r) => self.on_running(r),
            Event::Active(active) => {
                if active {
                    self.set_message("", "", 0);
                } else {
                    self.set_message("Ни одна из отмеченных сцен не в эфире — слежение ждёт", "", 0);
                }
                self.update_tally();
                self.fill_modes();
            }
            Event::Error(t) => {
                self.status(&format!("Ошибка: {t}"), 0, Some(col::danger()));
                self.ui().set_error_text(tr(&t).into());
            }
            Event::Device(d) => {
                let ui = self.ui();
                ui.global::<Cfg>().set_device(d.as_str().into());
                ui.set_footer_device(d.as_str().into());
            }
            Event::CameraFault(t) => self.on_fault(t),
            Event::SpeakerOnAir(_) => {}
        }
    }

    fn on_running(&self, running: bool) {
        *self.running.borrow_mut() = running;
        let ui = self.ui();
        ui.set_running(running);
        ui.global::<Cfg>().set_running(running);
        self.update_tally();
        self.fill_modes();
        if running {
            return;
        }
        ui.set_has_frame(false);
        ui.set_frame(Image::default());
        ui.set_people(ModelRc::default());
        ui.set_dots(ModelRc::default());
        ui.set_air_live(false);
        ui.set_air_meta("".into());
        ui.set_air_scene("".into());
        ui.set_state_text("".into());
        ui.set_hud("".into());
        ui.set_ro_zoom("—".into());
        ui.set_ro_shot("—".into());
        ui.set_ro_match("—".into());
        ui.set_ro_camera("—".into());
        ui.set_ro_camera_bad(false);
        self.on_fault(String::new());
        self.status("Остановлено", 0, None);
        self.idle_message();
    }

    fn on_fault(&self, text: String) {
        if *self.fault.borrow() == text {
            return;
        }
        *self.fault.borrow_mut() = text.clone();
        let ui = self.ui();
        ui.set_fault(tr(&text).into());
        ui.set_ro_camera_bad(!text.is_empty());
        ui.set_ro_camera(tr(if !text.is_empty() { "Сбой" } else if *self.running.borrow() { "Норма" } else { "—" }).into());
        if !text.is_empty() {
            self.status(&text, 0, Some(col::danger()));
            if self.settings.read().unwrap().watchdog_sound {
                beep();
                self.beep_timer.start(TimerMode::Repeated, Duration::from_secs(4), beep);
            }
        } else {
            self.beep_timer.stop();
            if self.engine.running() {
                self.status("Камера снова в порядке", 5000, None);
            }
        }
    }

    fn on_preview(&self, d: PreviewData) {
        let ui = self.ui();
        let live = d.active && !d.paused && d.fault.is_empty();
        ui.set_air_live(live);
        ui.set_air_meta(format!("{:.2}×", d.zoom).into());
        ui.set_air_scene(d.scene.as_str().into());
        ui.set_ro_zoom(format!("{:.2}×", d.zoom).into());
        ui.set_ro_match(if d.match_score > 0.0 { format!("{:.0}%", d.match_score * 100.0) } else { "—".to_string() }.into());
        let shot = match (d.mode.as_str(), d.cut.as_str()) {
            ("wide", _) => "Общий",
            ("duo", _) => "Дуэт",
            _ if d.paused => "Общий",
            (_, "wide") => "Средний",
            _ => "Крупный",
        };
        ui.set_ro_shot(tr(shot).into());
        if d.fault.is_empty() {
            ui.set_ro_camera(tr("Норма").into());
            ui.set_ro_camera_bad(false);
        }
        // nobody looks at a minimized window: skip the picture work
        if ui.window().is_minimized() {
            return;
        }

        let (w, h) = (d.img.width as u32, d.img.height as u32);
        let buf = SharedPixelBuffer::<Rgb8Pixel>::clone_from_slice(&d.img.data, w, h);
        ui.set_frame(Image::from_rgb8(buf));
        ui.set_frame_aspect(w as f32 / h.max(1) as f32);
        ui.set_has_frame(true);

        let mut boxes = Vec::with_capacity(d.people.len());
        let mut dots = Vec::new();
        for p in &d.people {
            let kind = if Some(p.tid) == d.target_id {
                2
            } else if Some(p.tid) == d.partner_id {
                1
            } else {
                0
            };
            let tag = match kind {
                2 if d.match_score > 0.0 => format!("{} {:.0}%", tr("Спикер"), d.match_score * 100.0),
                2 => tr("Спикер"),
                1 => tr("Второй"),
                _ => String::new(),
            };
            let [x0, y0, x1, y1] = p.bbox;
            boxes.push(PersonBox {
                x: x0 as f32,
                y: y0 as f32,
                w: (x1 - x0) as f32,
                h: (y1 - y0) as f32,
                kind,
                tag: tag.into(),
            });
            if kind == 2 {
                if let Some(kp) = &p.kp {
                    for i in HEAD_KP.iter().chain(SHOULDER_KP.iter()).chain(HIP_KP.iter()) {
                        if kp.conf[*i] >= 0.5 {
                            dots.push(Dot { x: kp.xy[*i][0] as f32, y: kp.xy[*i][1] as f32 });
                        }
                    }
                }
            }
        }
        // draw the speaker last so it sits on top
        boxes.sort_by_key(|b| b.kind);
        ui.set_people(ModelRc::new(VecModel::from(boxes)));
        ui.set_dots(ModelRc::new(VecModel::from(dots)));

        let [cx0, cy0, cx1, cy1] = d.crop;
        ui.set_crop_x(cx0 as f32);
        ui.set_crop_y(cy0 as f32);
        ui.set_crop_w((cx1 - cx0) as f32);
        ui.set_crop_h((cy1 - cy0) as f32);
        let [zx0, zy0, zx1, zy1] = d.zone;
        ui.set_zone_x(zx0 as f32);
        ui.set_zone_y(zy0 as f32);
        ui.set_zone_w((zx1 - zx0) as f32);
        ui.set_zone_h((zy1 - zy0) as f32);
        ui.set_active(d.active);
        ui.set_paused(d.paused);

        let cut_word = match d.cut.as_str() {
            "close" => " · крупный план",
            "wide" => " · средний план",
            _ => "",
        };
        let text = if !d.active {
            "Ждёт сцену".to_string()
        } else if d.paused {
            "Общий план".to_string()
        } else if d.mode == "wide" {
            "Общий план сцены".to_string()
        } else if d.target_id.is_none() {
            "Ищу спикера".to_string()
        } else if d.mode == "duo" {
            if d.partner_id.is_some() { "Дуэт" } else { "Дуэт · ищу второго" }.to_string()
        } else {
            format!("Слежение{cut_word}")
        };
        ui.set_state_text(tr(&text).into());
        ui.set_hud(tr(&format!("{:.2}×   {:.0} мс   {:.0} к/с", d.zoom, d.infer_ms, d.fps)).into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn number_formats_match_python() {
        assert_eq!(format_num(2.0, "x1", false), "2.0×");
        assert_eq!(format_num(0.08, "pct0", false), "8%");
        assert_eq!(format_num(0.7, "s2", false), "0.70 с");
        assert_eq!(format_num(6.0, "s0", false), "6 с");
        assert_eq!(format_num(6.0, "s0", true), "6 s");
    }

    #[test]
    fn camera_is_picked_by_kind() {
        let kinds: HashMap<String, String> = [
            ("Захват окна", "window_capture"),
            ("Мультимедиа", "ffmpeg_source"),
            ("Камера", "dshow_input"),
            ("Браузер", "browser_source"),
        ]
        .into_iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect();
        let all: Vec<String> = ["Захват окна", "Браузер", "Мультимедиа", "Камера"].map(String::from).to_vec();
        // nothing chosen yet: the capture device
        assert_eq!(pick_camera("", true, &all, &kinds).as_deref(), Some("Камера"));
        // the app picked a window capture earlier, a video appeared: switch
        let no_cam: Vec<String> = ["Захват окна", "Браузер", "Мультимедиа"].map(String::from).to_vec();
        assert_eq!(pick_camera("Захват окна", true, &no_cam, &kinds).as_deref(), Some("Мультимедиа"));
        // the person chose the window capture: keep it
        assert_eq!(pick_camera("Захват окна", false, &all, &kinds), None);
        // the person's source disappeared from OBS: pick the best one
        assert_eq!(pick_camera("Старая камера", false, &all, &kinds).as_deref(), Some("Камера"));
        // only screens and pictures: don't hop between them
        let poor: Vec<String> = ["Захват окна", "Браузер"].map(String::from).to_vec();
        assert_eq!(pick_camera("Захват окна", true, &poor, &kinds), None);
    }

    #[test]
    fn hex_colors() {
        assert!(parse_hex("#c6f36b").is_some());
        assert!(parse_hex("c6f36b").is_some());
        assert!(parse_hex("#c6f3").is_none());
        assert!(parse_hex("#zzzzzz").is_none());
    }
}
