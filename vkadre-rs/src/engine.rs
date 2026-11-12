//! Tracking engine: connects OBS, detector, tracker and framer.
//!
//! Threads:
//!   * worker   — connects, loads the model, then runs the render loop (~30 Hz while a scene is on air):
//!                finds which configured scene is on air, smooths the frame, writes the crop to OBS.
//!   * detector — grabs low-res screenshots of the raw camera source (~10 Hz), finds people,
//!                picks the speaker (and a partner in duo mode), decides cuts, watches the camera.
//!   * events   — waits for OBS scene-change events, so nothing polls OBS while idle.
//!
//! Idle cost: when no configured scene is on air, all three threads sleep on a condition variable
//! and wake only on an OBS event (or once a second to notice a stop). Crops are sent only when they
//! change, and the «Общий» mode does not run the neural net at all.
//!
//! Scene modes (settings.scene_modes): track — follow the speaker; duo — keep two people in the frame;
//! wide — a calm wide shot of the stage zone.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, RwLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use serde_json::{Map, Value};

use crate::appearance::{signature, AppearanceMemory, Signature};
use crate::detector::{Detect, PoseDetector};
use crate::framing::{Framer, Person};
use crate::obs::{ObsApi, ObsConnection, EVENTS_NONE, EVENTS_SCENES, EVENTS_UI, RESTORE_KEYS};
use crate::rgb::RgbImage;
use crate::settings::Settings;
use crate::tracker::{iou, SimpleTracker};
use crate::watchdog::{CameraWatchdog, Fault};

pub const MODES: [&str; 3] = ["track", "duo", "wide"];

/// Monotonic seconds. Starts far from zero so "last seen at 0" means "long ago".
pub fn now() -> f64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f64() + 1.0e6
}

#[derive(Debug, Clone)]
pub struct PreviewData {
    pub img: Arc<RgbImage>,
    pub people: Vec<Person>,
    pub target_id: Option<i64>,
    pub partner_id: Option<i64>,
    pub crop: [f64; 4],
    pub zone: [f64; 4],
    pub active: bool,
    pub paused: bool,
    pub zoom: f64,
    pub infer_ms: f64,
    pub fps: f64,
    pub scene: String,
    pub mode: String,
    /// "close" / "wide" when auto-cut is on
    pub cut: String,
    /// how sure we are it is the remembered speaker (0..1)
    pub match_score: f64,
    pub fault: String,
}

#[derive(Debug, Clone)]
pub enum Event {
    Preview(Box<PreviewData>),
    Status(String),
    Running(bool),
    Active(bool),
    Error(String),
    Device(String),
    /// "" when fine, else a human message
    CameraFault(String),
    /// scene name: the speaker is steadily in frame (name plate)
    SpeakerOnAir(String),
}

pub type Emit = Arc<dyn Fn(Event) + Send + Sync>;
pub type ConnectFn = Arc<dyn Fn(&Settings, u32) -> Result<Box<dyn ObsApi>> + Send + Sync>;
pub type DetectorFn = Arc<dyn Fn(&Settings) -> Result<Arc<dyn Detect>> + Send + Sync>;

/// Tracking state, guarded by one mutex.
struct State {
    framer: Option<Framer>,
    tracker: SimpleTracker,
    memory: AppearanceMemory,
    locked_id: Option<i64>,
    partner_id: Option<i64>,
    last_center: Option<(f64, f64)>,
    last_seen: f64,
    last_people: Vec<Person>,
    last_sigs: HashMap<i64, Option<Signature>>,
    paused: bool,
    active: bool,
    active_scene: Option<String>,
    mode: String,
    snap: bool,
    match_score: f64,
    // auto-cut
    cut: &'static str,
    cut_since: f64,
    speed: f64,
    move_since: Option<f64>,
    still_since: Option<f64>,
    prev_cx: Option<(f64, f64)>,
    // name plate
    plate_pending: bool,
    found_since: Option<f64>,
    // watchdog
    fault: Option<Fault>,
    switched: bool,
}

impl Default for State {
    fn default() -> Self {
        State {
            framer: None,
            tracker: SimpleTracker::default(),
            memory: AppearanceMemory::default(),
            locked_id: None,
            partner_id: None,
            last_center: None,
            last_seen: 0.0,
            last_people: Vec::new(),
            last_sigs: HashMap::new(),
            paused: false,
            active: false,
            active_scene: None,
            mode: "track".into(),
            snap: true,
            match_score: 0.0,
            cut: "close",
            cut_since: 0.0,
            speed: 0.0,
            move_since: None,
            still_since: None,
            prev_cx: None,
            plate_pending: false,
            found_since: None,
            fault: None,
            switched: false,
        }
    }
}

impl State {
    fn reset_target(&mut self) {
        self.locked_id = None;
        self.partner_id = None;
        self.last_center = None;
        if let Some(f) = self.framer.as_mut() {
            f.reset_target();
        }
    }

    fn pick_target(&mut self, s: &Settings, people: &[Person], now: f64) -> Option<usize> {
        if people.is_empty() {
            self.match_score = 0.0;
            return None;
        }
        let mut cur = self.locked_id.and_then(|id| people.iter().position(|p| p.tid == id));

        let use_reid = s.reid && self.memory.model.is_some();
        let scores: Vec<f64> = if use_reid {
            people.iter().map(|p| self.memory.score(self.sig(p.tid))).collect()
        } else {
            Vec::new()
        };
        if use_reid {
            let thr = s.reid_threshold;
            let best = (0..people.len())
                .max_by(|&a, &b| scores[a].partial_cmp(&scores[b]).unwrap_or(std::cmp::Ordering::Equal))
                .unwrap();
            match cur {
                // we followed someone else while the speaker was away — the speaker is back
                Some(c) => {
                    if best != c && scores[c] < thr - 0.1 && scores[best] >= thr + 0.05 {
                        cur = Some(best);
                    }
                }
                None => {
                    if scores[best] >= thr {
                        cur = Some(best);
                    }
                }
            }
        }

        let cur = match cur {
            Some(c) => c,
            None => match self.last_center {
                Some((lx, ly)) if now - self.last_seen < s.lost_timeout => (0..people.len())
                    .min_by(|&a, &b| {
                        let da = { let (x, y) = people[a].center(); (x - lx).hypot(y - ly) };
                        let db = { let (x, y) = people[b].center(); (x - lx).hypot(y - ly) };
                        da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
                    })
                    .unwrap(),
                _ => {
                    if let Some(f) = self.framer.as_mut() {
                        f.reset_target();
                    }
                    (0..people.len())
                        .max_by(|&a, &b| people[a].area().partial_cmp(&people[b].area()).unwrap_or(std::cmp::Ordering::Equal))
                        .unwrap()
                }
            },
        };
        let target = &people[cur];
        self.locked_id = Some(target.tid);

        // learn the speaker's colors only while nobody overlaps them
        if s.reid && people.iter().enumerate().all(|(i, o)| i == cur || iou(&target.bbox, &o.bbox) < 0.05) {
            let sig = self.sig(target.tid).copied();
            self.memory.update(sig.as_ref());
        }
        self.match_score = if use_reid { scores[cur] } else { self.memory.score(self.sig(target.tid)) };
        Some(cur)
    }

    fn sig(&self, tid: i64) -> Option<&Signature> {
        self.last_sigs.get(&tid).and_then(|s| s.as_ref())
    }

    fn pick_partner(&mut self, people: &[Person], target: usize) -> Option<usize> {
        let others: Vec<usize> = (0..people.len()).filter(|&i| i != target).collect();
        if others.is_empty() {
            return None;
        }
        let tx = people[target].center().0;
        let p = others.iter().copied().find(|&i| Some(people[i].tid) == self.partner_id).unwrap_or_else(|| {
            // the biggest person, preferring someone near the speaker
            let score = |i: usize| people[i].area() / (1.0 + 2.0 * (people[i].center().0 - tx).abs());
            others.iter().copied().max_by(|&a, &b| score(a).partial_cmp(&score(b)).unwrap_or(std::cmp::Ordering::Equal)).unwrap()
        });
        self.partner_id = Some(people[p].tid);
        Some(p)
    }

    /// (shot override, cut now?). Close shot while the speaker stands, wider while they walk.
    fn cut_shot(&mut self, s: &Settings, target: &Person, now: f64) -> (Option<String>, bool) {
        let cx = target.center().0;
        if let Some((pcx, pt)) = self.prev_cx {
            let v = (cx - pcx).abs() / (now - pt).max(1e-3);
            self.speed = 0.7 * self.speed + 0.3 * v;
        }
        self.prev_cx = Some((cx, now));
        if !s.auto_cut {
            return (None, false);
        }
        let thr = s.cut_move_speed;
        let can_cut = now - self.cut_since >= s.cut_min_hold;
        let mut cut = false;
        if self.speed > thr {
            self.move_since.get_or_insert(now);
            self.still_since = None;
        } else if self.speed < thr * 0.5 {
            self.still_since.get_or_insert(now);
            self.move_since = None;
        }
        if self.cut == "close" && self.move_since.is_some_and(|t| now - t > 0.8) && can_cut {
            (self.cut, self.cut_since, cut) = ("wide", now, true);
        } else if self.cut == "wide" && self.still_since.is_some_and(|t| now - t > 2.5) && can_cut {
            (self.cut, self.cut_since, cut) = ("close", now, true);
        }
        ((self.cut == "wide").then(|| s.cut_wide_shot.clone()), cut)
    }
}

struct Shared {
    settings: Arc<RwLock<Settings>>,
    state: Mutex<State>,
    stop: AtomicBool,
    running: AtomicBool,
    /// set by the events thread (or by the UI): re-check which scene is on air
    scene_dirty: AtomicBool,
    /// false when the event connection is down: fall back to polling
    events_ok: AtomicBool,
    /// bumped by every notify(), so a sleeper can tell it was woken on purpose
    generation: AtomicU64,
    wake_lock: Mutex<()>,
    wake: Condvar,
    emit: Emit,
}

impl Shared {
    fn settings(&self) -> Settings {
        self.settings.read().unwrap().clone()
    }

    fn notify(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        let _g = self.wake_lock.lock().unwrap();
        self.wake.notify_all();
    }

    fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    /// Sleeps up to `dur`; returns early on stop, on a scene event or on any notify() since `since`.
    fn sleep(&self, since: u64, dur: Duration) {
        let g = self.wake_lock.lock().unwrap();
        let _ = self.wake.wait_timeout_while(g, dur, |_| {
            !self.stop.load(Ordering::Relaxed)
                && !self.scene_dirty.load(Ordering::Relaxed)
                && self.generation.load(Ordering::SeqCst) == since
        });
    }

    /// Sleeps up to `dur`; returns early on stop or on any notify() since `since`.
    fn pause(&self, since: u64, dur: Duration) {
        let g = self.wake_lock.lock().unwrap();
        let _ = self.wake.wait_timeout_while(g, dur, |_| {
            !self.stop.load(Ordering::Relaxed) && self.generation.load(Ordering::SeqCst) == since
        });
    }

    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }
}

pub struct Engine {
    shared: Arc<Shared>,
    worker: Mutex<Option<JoinHandle<()>>>,
    connect: ConnectFn,
    make_detector: DetectorFn,
    detector_cache: Arc<Mutex<Option<(String, bool, Arc<dyn Detect>)>>>,
}

impl Engine {
    pub fn new(settings: Arc<RwLock<Settings>>, emit: Emit) -> Self {
        let connect: ConnectFn = Arc::new(|s: &Settings, events: u32| {
            if crate::demo::active() {
                return Ok(Box::new(crate::demo::DemoObs::new()) as Box<dyn ObsApi>);
            }
            let c = ObsConnection::connect(&s.obs_host, s.obs_port, &s.obs_password, Duration::from_secs(5), events)?;
            Ok(Box::new(c) as Box<dyn ObsApi>)
        });
        let make_detector: DetectorFn = Arc::new(|s: &Settings| {
            Ok(Arc::new(PoseDetector::new(&s.model_path(), s.use_gpu)?) as Arc<dyn Detect>)
        });
        Self::with_backends(settings, emit, connect, make_detector)
    }

    pub fn with_backends(settings: Arc<RwLock<Settings>>, emit: Emit, connect: ConnectFn, make_detector: DetectorFn) -> Self {
        Engine {
            shared: Arc::new(Shared {
                settings,
                state: Mutex::new(State::default()),
                stop: AtomicBool::new(false),
                running: AtomicBool::new(false),
                scene_dirty: AtomicBool::new(false),
                events_ok: AtomicBool::new(false),
                generation: AtomicU64::new(0),
                wake_lock: Mutex::new(()),
                wake: Condvar::new(),
                emit,
            }),
            worker: Mutex::new(None),
            connect,
            make_detector,
            detector_cache: Arc::new(Mutex::new(None)),
        }
    }

    // ------------------------------------------------------------------ public API (UI thread)

    pub fn running(&self) -> bool {
        self.shared.running.load(Ordering::SeqCst)
    }

    pub fn start(&self) {
        if self.running() {
            return;
        }
        let s = self.shared.settings();
        if s.source.is_empty() || s.modes().is_empty() {
            (self.shared.emit)(Event::Error("Выберите камеру и отметьте, в каких сценах работать.".into()));
            return;
        }
        // a previous run may still be finishing
        if let Some(h) = self.worker.lock().unwrap().take() {
            let _ = h.join();
        }
        self.shared.stop.store(false, Ordering::SeqCst);
        self.shared.running.store(true, Ordering::SeqCst);
        (self.shared.emit)(Event::Running(true));
        let shared = self.shared.clone();
        let connect = self.connect.clone();
        let make_detector = self.make_detector.clone();
        let cache = self.detector_cache.clone();
        let h = std::thread::Builder::new()
            .name("engine".into())
            .spawn(move || run(shared, connect, make_detector, cache))
            .expect("spawn engine thread");
        *self.worker.lock().unwrap() = Some(h);
    }

    /// Asks the engine to stop; the worker restores OBS and reports Running(false).
    pub fn stop(&self) {
        if !self.running() {
            return;
        }
        self.shared.stop.store(true, Ordering::SeqCst);
        self.shared.notify();
    }

    /// Stop and wait for the worker (on exit).
    pub fn stop_and_wait(&self) {
        self.stop();
        if let Some(h) = self.worker.lock().unwrap().take() {
            let _ = h.join();
        }
    }

    pub fn set_paused(&self, paused: bool) {
        self.shared.state.lock().unwrap().paused = paused;
        self.shared.notify();
    }

    pub fn retarget(&self) {
        let mut st = self.shared.state.lock().unwrap();
        st.reset_target();
        st.memory.reset(None);
    }

    pub fn active_scene(&self) -> Option<String> {
        self.shared.state.lock().unwrap().active_scene.clone()
    }

    pub fn mode(&self) -> String {
        self.shared.state.lock().unwrap().mode.clone()
    }

    pub fn partner_id(&self) -> Option<i64> {
        self.shared.state.lock().unwrap().partner_id
    }

    pub fn locked_id(&self) -> Option<i64> {
        self.shared.state.lock().unwrap().locked_id
    }

    /// Lock onto the person under a click (normalized source coords).
    /// In duo mode a click on someone else picks the second person.
    pub fn lock_at(&self, x: f64, y: f64) -> bool {
        let mut st = self.shared.state.lock().unwrap();
        let hit = st
            .last_people
            .iter()
            .filter(|p| p.contains(x, y))
            .min_by(|a, b| a.area().partial_cmp(&b.area()).unwrap_or(std::cmp::Ordering::Equal))
            .cloned();
        let Some(p) = hit else { return false };
        if st.mode == "duo" && st.locked_id.is_some() && Some(p.tid) != st.locked_id {
            st.partner_id = Some(p.tid);
            return true;
        }
        st.locked_id = Some(p.tid);
        st.last_center = Some(p.center());
        st.last_seen = now();
        let sig = st.sig(p.tid).copied();
        st.memory.reset(sig);
        if let Some(f) = st.framer.as_mut() {
            f.reset_target();
        }
        st.paused = false;
        true
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.stop_and_wait();
    }
}

// ---------------------------------------------------------------------- worker

fn get_detector(
    shared: &Shared,
    s: &Settings,
    make: &DetectorFn,
    cache: &Mutex<Option<(String, bool, Arc<dyn Detect>)>>,
) -> Result<Arc<dyn Detect>> {
    let key = (s.model_path().display().to_string(), s.use_gpu);
    let mut c = cache.lock().unwrap();
    let det = match c.as_ref() {
        Some((p, g, d)) if *p == key.0 && *g == key.1 => d.clone(),
        _ => {
            (shared.emit)(Event::Status("Загрузка нейросети…".into()));
            let d = make(s)?;
            *c = Some((key.0, key.1, d.clone()));
            d
        }
    };
    (shared.emit)(Event::Device(det.device_name()));
    Ok(det)
}

type Items = HashMap<String, (i64, Map<String, Value>)>;

fn run(shared: Arc<Shared>, connect: ConnectFn, make_detector: DetectorFn, cache: Arc<Mutex<Option<(String, bool, Arc<dyn Detect>)>>>) {
    let s = shared.settings();
    let mut items: Items = HashMap::new();
    let mut touched: HashSet<String> = HashSet::new();
    let mut render: Option<Box<dyn ObsApi>> = None;
    let mut threads: Vec<JoinHandle<()>> = Vec::new();

    let result = (|| -> Result<()> {
        (shared.emit)(Event::Status("Подключение к OBS…".into()));
        let mut conn = connect(&s, EVENTS_NONE)?;
        let detect_conn = connect(&s, EVENTS_NONE)?;

        let modes = s.modes();
        for scene in modes.keys() {
            match conn.item_id(scene, &s.source) {
                Ok(id) => {
                    let tr = conn.item_transform(scene, id)?;
                    items.insert(scene.clone(), (id, tr));
                }
                Err(_) => log::info!("Scene {scene} has no {}", s.source),
            }
        }
        if items.is_empty() {
            let first = modes.keys().next().cloned().unwrap_or_default();
            return Err(anyhow!("В сцене «{first}» нет источника «{}».", s.source));
        }
        let canvas = conn.canvas_size()?;
        // the first configured scene that has the camera
        let original = modes.keys().find_map(|k| items.get(k)).map(|(_, t)| t.clone()).unwrap_or_default();
        let num = |k: &str| original.get(k).and_then(Value::as_f64).unwrap_or(0.0) as u32;
        let (src_w, src_h) = (num("sourceWidth"), num("sourceHeight"));
        if src_w == 0 || src_h == 0 {
            return Err(anyhow!("Источник «{}» не отдаёт картинку. Камера включена?", s.source));
        }
        if (src_w as f64) < canvas.0 as f64 * 1.5 {
            (shared.emit)(Event::Status(format!("Внимание: источник {src_w}×{src_h} — приближение будет мягким")));
        }

        let detector = get_detector(&shared, &s, &make_detector, &cache)?;

        {
            let mut st = shared.state.lock().unwrap();
            let paused = st.paused;
            *st = State { paused, ..State::default() };
            st.framer = Some(Framer::new(src_w as f64 / src_h as f64, canvas.0 as f64 / canvas.1 as f64));
        }

        // scene events: a separate socket that just waits; if it fails we poll instead
        match connect(&s, EVENTS_SCENES | EVENTS_UI) {
            Ok(mut ev) => {
                shared.events_ok.store(true, Ordering::SeqCst);
                let sh = shared.clone();
                threads.push(
                    std::thread::Builder::new()
                        .name("obs-events".into())
                        .spawn(move || {
                            while !sh.stopped() {
                                match ev.wait_scene_event() {
                                    Ok(true) => {
                                        sh.scene_dirty.store(true, Ordering::SeqCst);
                                        sh.notify();
                                    }
                                    Ok(false) => {}
                                    Err(e) => {
                                        log::warn!("OBS events: {e:#}");
                                        sh.events_ok.store(false, Ordering::SeqCst);
                                        break;
                                    }
                                }
                            }
                        })
                        .expect("spawn events thread"),
                );
            }
            Err(e) => log::warn!("OBS events unavailable, polling: {e:#}"),
        }

        let sh = shared.clone();
        threads.push(
            std::thread::Builder::new()
                .name("detector".into())
                .spawn(move || detect_loop(sh, detect_conn, detector, src_w, src_h))
                .expect("spawn detector thread"),
        );
        (shared.emit)(Event::Status(format!("Работает · {src_w}×{src_h} → {}×{}", canvas.0, canvas.1)));
        let r = render_loop(&shared, conn.as_mut(), &items, &mut touched, canvas, src_w, src_h);
        render = Some(conn);
        r
    })();

    if let Err(e) = &result {
        log::error!("Engine error: {e:#}");
        (shared.emit)(Event::Error(format!("{e}")));
    }
    shared.stop.store(true, Ordering::SeqCst);
    shared.notify();
    for t in threads {
        let _ = t.join();
    }

    if s.restore_on_exit && !touched.is_empty() {
        // the render connection may be dead after an error: reconnect for the restore
        let conn = match render {
            Some(c) => Some(c),
            None => connect(&s, EVENTS_NONE).ok(),
        };
        if let Some(mut conn) = conn {
            for scene in &touched {
                let (id, original) = &items[scene];
                let mut tr = Map::new();
                for k in RESTORE_KEYS {
                    if let Some(v) = original.get(k) {
                        tr.insert(k.to_string(), v.clone());
                    }
                }
                if let Err(e) = conn.set_item_transform(scene, *id, Value::Object(tr)) {
                    log::warn!("Не удалось восстановить источник в «{scene}»: {e:#}");
                }
            }
        }
    }

    let had_fault = {
        let mut st = shared.state.lock().unwrap();
        st.active = false;
        st.active_scene = None;
        st.fault.take().is_some()
    };
    if had_fault {
        (shared.emit)(Event::CameraFault(String::new()));
    }
    shared.running.store(false, Ordering::SeqCst);
    (shared.emit)(Event::Running(false));
    (shared.emit)(Event::Status("Остановлено".into()));
}

/// The configured scene that is on air; in Studio Mode the preview counts too,
/// so the shot is ready before the transition.
fn find_active(conn: &mut dyn ObsApi, items: &Items) -> Result<Option<String>> {
    if let Some(program) = conn.program_scene()? {
        if items.contains_key(&program) {
            return Ok(Some(program));
        }
    }
    Ok(conn.preview_scene().filter(|p| items.contains_key(p)))
}

fn render_loop(
    shared: &Shared,
    conn: &mut dyn ObsApi,
    items: &Items,
    touched: &mut HashSet<String>,
    canvas: (u32, u32),
    src_w: u32,
    src_h: u32,
) -> Result<()> {
    let mut last = now();
    let mut next_check = 0.0;
    let mut failures = 0;
    let mut sent: HashMap<String, [i64; 4]> = HashMap::new();
    let (sw, shh) = (src_w as f64, src_h as f64);

    while !shared.stopped() {
        let gen = shared.generation();
        let t = now();
        let s = shared.settings();
        let interval = 1.0 / s.render_fps.max(1) as f64;
        let step = (|| -> Result<bool> {
            let dirty = shared.scene_dirty.swap(false, Ordering::SeqCst);
            if dirty || t >= next_check {
                // with events we only re-check now and then as a safety net
                next_check = t + if shared.events_ok.load(Ordering::Relaxed) { 5.0 } else { 0.5 };
                let scene = find_active(conn, items)?;
                let changed = {
                    let mut st = shared.state.lock().unwrap();
                    if scene != st.active_scene {
                        let mode = scene
                            .as_ref()
                            .and_then(|sc| s.modes().get(sc).cloned())
                            .unwrap_or_else(|| if scene.is_some() { "track".into() } else { st.mode.clone() });
                        if scene.is_some() && !st.active {
                            st.reset_target();
                        }
                        st.snap = true;
                        st.active = scene.is_some();
                        st.mode = if MODES.contains(&mode.as_str()) { mode } else { "track".into() };
                        st.cut = "close";
                        st.cut_since = t;
                        st.plate_pending = scene.is_some() && (st.mode == "track" || st.mode == "duo");
                        st.found_since = None;
                        st.active_scene = scene.clone();
                        true
                    } else {
                        false
                    }
                };
                if changed {
                    (shared.emit)(Event::Active(scene.is_some()));
                    shared.notify(); // wake the detector
                }
            }

            let dt = t - last;
            last = t;
            let job = {
                let mut st = shared.state.lock().unwrap();
                let scene = st.active_scene.clone();
                let snap = st.snap;
                match (scene, st.framer.as_mut()) {
                    (Some(scene), Some(fr)) if !snap => {
                        let f = fr.step(&s, dt);
                        Some((scene, fr.rect(&f)))
                    }
                    (scene, _) => {
                        if scene.is_none() {
                            return Ok(false);
                        }
                        None
                    }
                }
            };
            if let Some((scene, [x0, y0, x1, y1])) = job {
                let crop = [
                    (x0 * sw).round() as i64,
                    (y0 * shh).round() as i64,
                    (sw - x1 * sw).round() as i64,
                    (shh - y1 * shh).round() as i64,
                ];
                // the frame settled: nothing to tell OBS
                if sent.get(&scene) != Some(&crop) {
                    conn.set_crop(&scene, items[&scene].0, canvas, crop)?;
                    sent.insert(scene.clone(), crop);
                    touched.insert(scene);
                }
            }
            Ok(true)
        })();

        match step {
            Ok(active) => {
                failures = 0;
                if !active {
                    // nothing on air: sleep until an OBS event (or the polling interval)
                    let wait = if shared.events_ok.load(Ordering::Relaxed) { 5.0 } else { 0.5 };
                    shared.sleep(gen, Duration::from_secs_f64(wait));
                    last = now();
                    continue;
                }
            }
            Err(e) => {
                failures += 1;
                log::warn!("OBS request failed: {e:#}");
                if failures >= 5 {
                    return Err(anyhow!("Потеряно соединение с OBS."));
                }
                shared.pause(gen, Duration::from_millis(500));
            }
        }
        let left = interval - (now() - t);
        if left > 0.0 {
            std::thread::sleep(Duration::from_secs_f64(left));
        }
    }
    Ok(())
}

fn on_frame_health(shared: &Shared, conn: &mut dyn ObsApi, s: &Settings, fault: Option<Fault>) {
    let active_scene = {
        let mut st = shared.state.lock().unwrap();
        if fault == st.fault {
            return;
        }
        st.fault = fault;
        if fault.is_none() {
            st.switched = false;
        }
        st.active_scene.clone()
    };
    (shared.emit)(Event::CameraFault(fault.map(|f| f.message().to_string()).unwrap_or_default()));
    let Some(fault) = fault else { return };
    log::warn!("Camera fault: {fault:?}");
    let switched = shared.state.lock().unwrap().switched;
    if s.watchdog_switch && !s.backup_scene.is_empty() && !switched {
        let r = (|| -> Result<()> {
            if conn.program_scene()? == active_scene {
                conn.set_program_scene(&s.backup_scene)?;
                (shared.emit)(Event::Status(format!("Камера пропала — включена сцена «{}»", s.backup_scene)));
            }
            Ok(())
        })();
        match r {
            Ok(()) => shared.state.lock().unwrap().switched = true,
            Err(e) => log::warn!("Не удалось переключить на запасную сцену: {e:#}"),
        }
    }
}

fn detect_loop(shared: Arc<Shared>, mut conn: Box<dyn ObsApi>, detector: Arc<dyn Detect>, src_w: u32, src_h: u32) {
    let mut fps = 0.0;
    let mut watchdog = CameraWatchdog::default();
    let size = detector.input_size() as f64;
    let scale = (size / src_w as f64).min(size / src_h as f64).min(1.0);
    // Ask OBS for exactly the model input size: no resize on our side.
    let (gw, gh) = ((src_w as f64 * scale).round() as u32, (src_h as f64 * scale).round() as u32);

    while !shared.stopped() {
        let gen = shared.generation();
        let t0 = now();
        let s = shared.settings();
        let (active, wide) = {
            let st = shared.state.lock().unwrap();
            (st.active, st.mode == "wide")
        };
        if !active {
            watchdog.reset();
            shared.pause(gen, Duration::from_secs(5));
            continue;
        }

        let img = match conn.screenshot(&s.source, gw, gh) {
            Ok(Some(img)) => Arc::new(img),
            Ok(None) => {
                shared.pause(gen, Duration::from_millis(100));
                continue;
            }
            Err(e) => {
                log::warn!("Screenshot failed: {e:#}");
                if s.watchdog {
                    let f = watchdog.failure(now());
                    on_frame_health(&shared, conn.as_mut(), &s, f);
                }
                shared.pause(gen, Duration::from_millis(500));
                continue;
            }
        };
        if s.watchdog {
            let f = watchdog.frame(&img, now());
            on_frame_health(&shared, conn.as_mut(), &s, f);
        }

        // «Общий»: the frame is the stage zone, no need to look for anyone
        let (mut people, infer_ms) = if wide {
            (Vec::new(), 0.0)
        } else {
            let t_inf = now();
            let people = detector.detect(&img, s.confidence).unwrap_or_else(|e| {
                log::warn!("Detection failed: {e:#}");
                Vec::new()
            });
            (people, (now() - t_inf) * 1000.0)
        };
        let [zx0, zy0, zx1, zy1] = s.zone;
        people.retain(|p| {
            let (cx, cy) = p.center();
            zx0 <= cx && cx <= zx1 && zy0 <= cy && cy <= zy1
        });

        let data = {
            let mut guard = shared.state.lock().unwrap();
            let st = &mut *guard;
            st.tracker.update(&mut people);
            st.last_sigs = if s.reid && !wide {
                people.iter().map(|p| (p.tid, signature(&img, p, s.kp_conf))).collect()
            } else {
                HashMap::new()
            };
            st.last_people = people.clone();
            let now_t = now();
            let mode = st.mode.clone();
            let mut target_id = None;
            let mut partner_id = None;
            let mut fr = st.framer.take().expect("framer exists while running");

            if mode == "wide" || st.paused {
                let zf = fr.zone_frame(&s);
                if st.snap {
                    fr.set_goal(&s, zf, true);
                } else {
                    fr.force_goal(zf);
                }
                st.snap = false;
            } else {
                st.framer = Some(fr);
                let target = st.pick_target(&s, &people, now_t);
                fr = st.framer.take().unwrap();
                if let Some(ti) = target {
                    let tp = people[ti].clone();
                    target_id = Some(tp.tid);
                    st.last_center = Some(tp.center());
                    st.last_seen = now_t;
                    let (goal, cut_now) = if mode == "duo" {
                        let partner = st.pick_partner(&people, ti);
                        partner_id = partner.map(|i| people[i].tid);
                        let group: Vec<&Person> = match partner {
                            Some(pi) => vec![&tp, &people[pi]],
                            None => vec![&tp],
                        };
                        (fr.frame_for_group(&s, &group), false)
                    } else {
                        let (shot, cut_now) = st.cut_shot(&s, &tp, now_t);
                        (fr.frame_for(&s, &tp, shot.as_deref(), true), cut_now)
                    };
                    fr.set_goal(&s, goal, st.snap || cut_now);
                    st.snap = false;
                    if plate_check(st, &s, now_t) {
                        if let Some(sc) = st.active_scene.clone() {
                            (shared.emit)(Event::SpeakerOnAir(sc));
                        }
                    }
                } else {
                    st.found_since = None;
                    if s.wide_on_lost && now_t - st.last_seen > s.lost_timeout {
                        st.locked_id = None;
                        let zf = fr.zone_frame(&s);
                        if st.snap {
                            fr.set_goal(&s, zf, true);
                            st.snap = false;
                        } else {
                            fr.force_goal(zf);
                        }
                    }
                }
            }
            let crop = fr.rect(&fr.current);
            let zoom = 1.0 / fr.current.h;
            st.framer = Some(fr);

            let dt = now() - t0;
            fps = if fps > 0.0 { 0.8 * fps + 0.2 * (1.0 / dt.max(1e-3)) } else { 1.0 / dt.max(1e-3) };
            PreviewData {
                img: img.clone(),
                people,
                target_id,
                partner_id,
                crop,
                zone: s.zone,
                active: st.active,
                paused: st.paused,
                zoom,
                infer_ms,
                fps,
                scene: st.active_scene.clone().unwrap_or_default(),
                mode: mode.clone(),
                cut: if s.auto_cut && mode == "track" { st.cut.to_string() } else { String::new() },
                match_score: st.match_score,
                fault: st.fault.map(|f| f.message().to_string()).unwrap_or_default(),
            }
        };
        (shared.emit)(Event::Preview(Box::new(data)));

        // «Общий» needs no tracking: a couple of frames a second keep the preview and the watchdog alive
        let interval = if wide { 0.5 } else { 1.0 / s.detect_fps.max(1) as f64 };
        let left = interval - (now() - t0);
        if left > 0.0 {
            shared.pause(gen, Duration::from_secs_f64(left));
        }
    }
}

/// Name plate: once per scene activation, when the speaker has been in frame for a while.
fn plate_check(st: &mut State, s: &Settings, now: f64) -> bool {
    if !st.plate_pending {
        return false;
    }
    let since = *st.found_since.get_or_insert(now);
    if now - since >= s.lt_delay {
        st.plate_pending = false;
        return s.lt_auto && !s.lt_name.trim().is_empty() && st.active_scene.is_some();
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::framing::tests::make_person;

    fn engine_with(s: Settings, conn: ConnectFn, det: DetectorFn) -> (Engine, Arc<Mutex<Vec<String>>>) {
        let errors = Arc::new(Mutex::new(Vec::new()));
        let e2 = errors.clone();
        let emit: Emit = Arc::new(move |ev| {
            if let Event::Error(t) = ev {
                e2.lock().unwrap().push(t);
            }
        });
        (Engine::with_backends(Arc::new(RwLock::new(s)), emit, conn, det), errors)
    }

    #[test]
    fn reid_returns_to_speaker() {
        let s = Settings::default();
        let mut st = State { framer: Some(Framer::new(16.0 / 9.0, 16.0 / 9.0)), ..State::default() };
        let mut img = RgbImage::new(640, 360, 40);
        let speaker = make_person(0.3, 1.0, 0.27, 1);
        let other = make_person(0.7, 1.0, 0.27, 2);
        img.fill_box(speaker.bbox, [200, 30, 30]);
        img.fill_box(other.bbox, [30, 60, 200]);
        let sig_s = signature(&img, &speaker, 0.5);
        let sig_o = signature(&img, &other, 0.5);
        st.memory.reset(sig_s);

        // speaker gone: we fall back to the other person
        st.last_sigs = HashMap::from([(2, sig_o)]);
        let i = st.pick_target(&s, std::slice::from_ref(&other), now() + 100.0).unwrap();
        assert_eq!(i, 0);
        // speaker is back with a new tracker id: we switch back to them
        let mut speaker = speaker;
        speaker.tid = 5;
        st.last_sigs = HashMap::from([(2, sig_o), (5, sig_s)]);
        let people = vec![other, speaker];
        let i = st.pick_target(&s, &people, now() + 100.0).unwrap();
        assert_eq!(people[i].tid, 5);
    }

    #[test]
    fn auto_cut_goes_wide_when_walking_and_back_when_still() {
        let s = Settings { auto_cut: true, cut_min_hold: 0.0, ..Default::default() };
        let mut st = State::default();
        let mut t = 0.0;
        let mut shot = None;
        for i in 0..30 {
            // walking: 0.15 frame widths per second
            t += 0.1;
            shot = st.cut_shot(&s, &make_person(0.3 + 0.015 * i as f64, 1.0, 0.27, 1), t).0;
        }
        assert_eq!(st.cut, "wide");
        assert_eq!(shot.as_deref(), Some("knee"));
        for _ in 0..60 {
            // standing still
            t += 0.1;
            shot = st.cut_shot(&s, &make_person(0.75, 1.0, 0.27, 1), t).0;
        }
        assert_eq!(st.cut, "close");
        assert_eq!(shot, None);
    }

    // -- end-to-end with a fake OBS and a fake detector --------------------------------

    #[derive(Default)]
    struct FakeState {
        program: String,
        crops: Vec<[i64; 4]>,
        restored: Option<Value>,
        screenshots: usize,
        missing_source: bool,
    }

    struct FakeObs(Arc<Mutex<FakeState>>);

    impl ObsApi for FakeObs {
        fn item_id(&mut self, _scene: &str, _source: &str) -> Result<i64> {
            if self.0.lock().unwrap().missing_source {
                return Err(anyhow!("not found"));
            }
            Ok(7)
        }
        fn item_transform(&mut self, _scene: &str, _id: i64) -> Result<Map<String, Value>> {
            Ok(serde_json::json!({"sourceWidth": 3840, "sourceHeight": 2160, "positionX": 10.0, "cropLeft": 0})
                .as_object()
                .unwrap()
                .clone())
        }
        fn set_item_transform(&mut self, _scene: &str, _id: i64, tr: Value) -> Result<()> {
            let mut f = self.0.lock().unwrap();
            if tr.get("boundsType").is_some() {
                let c = |k: &str| tr.get(k).and_then(Value::as_i64).unwrap_or(0);
                f.crops.push([c("cropLeft"), c("cropTop"), c("cropRight"), c("cropBottom")]);
            } else {
                f.restored = Some(tr);
            }
            Ok(())
        }
        fn canvas_size(&mut self) -> Result<(u32, u32)> {
            Ok((1920, 1080))
        }
        fn program_scene(&mut self) -> Result<Option<String>> {
            Ok(Some(self.0.lock().unwrap().program.clone()))
        }
        fn preview_scene(&mut self) -> Option<String> {
            None
        }
        fn set_program_scene(&mut self, scene: &str) -> Result<()> {
            self.0.lock().unwrap().program = scene.into();
            Ok(())
        }
        fn screenshot(&mut self, _source: &str, w: u32, h: u32) -> Result<Option<RgbImage>> {
            self.0.lock().unwrap().screenshots += 1;
            // a little noise so the watchdog does not call it frozen
            let mut img = RgbImage::new(w as usize, h as usize, 90);
            img.data[0] = (now() * 1000.0) as u8;
            Ok(Some(img))
        }
    }

    struct FakeDetector;
    impl Detect for FakeDetector {
        fn input_size(&self) -> usize {
            640
        }
        fn device_name(&self) -> String {
            "fake".into()
        }
        fn detect(&self, _img: &RgbImage, _c: f64) -> Result<Vec<Person>> {
            Ok(vec![make_person(0.65, 1.0, 0.27, -1)])
        }
    }

    fn fakes(state: Arc<Mutex<FakeState>>) -> (ConnectFn, DetectorFn) {
        let conn: ConnectFn = Arc::new(move |_s: &Settings, _ev: u32| Ok(Box::new(FakeObs(state.clone())) as Box<dyn ObsApi>));
        let det: DetectorFn = Arc::new(|_s: &Settings| Ok(Arc::new(FakeDetector) as Arc<dyn Detect>));
        (conn, det)
    }

    fn wait(cond: impl Fn() -> bool, secs: f64) -> bool {
        let end = now() + secs;
        while now() < end {
            if cond() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        false
    }

    #[test]
    fn engine_tracks_only_in_active_scene() {
        let fake = Arc::new(Mutex::new(FakeState { program: "Хвала".into(), ..Default::default() }));
        let s = Settings {
            scene: "Проповедь".into(),
            source: "Камера".into(),
            active_scenes: vec!["Проповедь".into()],
            detect_fps: 20,
            ..Default::default()
        };
        let (conn, det) = fakes(fake.clone());
        let (e, errors) = engine_with(s, conn, det);

        e.start();
        std::thread::sleep(Duration::from_millis(1000));
        assert!(fake.lock().unwrap().crops.is_empty(), "must not touch OBS while the scene is not on air");
        assert_eq!(fake.lock().unwrap().screenshots, 0, "must not grab frames while idle");

        fake.lock().unwrap().program = "Проповедь".into();
        // no events socket in the fake: the polling fallback must notice within 0.5 s
        // the shot snaps onto a standing speaker, so a single crop is enough (nothing is resent)
        assert!(wait(|| !fake.lock().unwrap().crops.is_empty(), 5.0), "{:?}", errors.lock().unwrap());
        let [l, _, r, _] = *fake.lock().unwrap().crops.last().unwrap();
        // Speaker at x=0.65 of a 3840 px source -> crop center near 2496 px
        let center = (l + (3840 - r)) as f64 / 2.0;
        assert!((center - 0.65 * 3840.0).abs() < 200.0, "center {center}");
        assert!(e.locked_id().is_some());

        fake.lock().unwrap().program = "Хвала".into();
        std::thread::sleep(Duration::from_millis(1000));
        let n = fake.lock().unwrap().crops.len();
        std::thread::sleep(Duration::from_millis(500));
        assert_eq!(fake.lock().unwrap().crops.len(), n, "must stop sending when the scene leaves air");

        e.stop();
        assert!(wait(|| !e.running(), 5.0));
        let restored = fake.lock().unwrap().restored.clone().expect("restored");
        assert_eq!(restored.get("positionX").and_then(Value::as_f64), Some(10.0));
        assert!(errors.lock().unwrap().is_empty());
    }

    #[test]
    fn settled_frame_is_not_resent() {
        let fake = Arc::new(Mutex::new(FakeState { program: "Проповедь".into(), ..Default::default() }));
        let s = Settings { source: "Камера".into(), scene: "Проповедь".into(), detect_fps: 20, ..Default::default() };
        let (conn, det) = fakes(fake.clone());
        let (e, _) = engine_with(s, conn, det);
        e.start();
        // the speaker stands still: the frame settles and OBS stops getting updates
        std::thread::sleep(Duration::from_millis(6000));
        let n = fake.lock().unwrap().crops.len();
        std::thread::sleep(Duration::from_millis(1000));
        let m = fake.lock().unwrap().crops.len();
        e.stop_and_wait();
        assert!(m - n < 5, "settled frame still sent {} times a second", m - n);
    }

    #[test]
    fn engine_reports_missing_source() {
        let fake = Arc::new(Mutex::new(FakeState { missing_source: true, ..Default::default() }));
        let s = Settings { scene: "Проповедь".into(), source: "Нет такой".into(), ..Default::default() };
        let (conn, det) = fakes(fake);
        let (e, errors) = engine_with(s, conn, det);
        e.start();
        assert!(wait(|| !e.running(), 5.0));
        let errs = errors.lock().unwrap();
        assert!(!errs.is_empty() && errs[0].contains("Нет такой"), "{errs:?}");
    }
}
