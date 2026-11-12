"""
Tracking engine: connects OBS, detector, tracker and framer.

Threads:
  * worker   — connects, loads the model, then runs the render loop (~30 Hz):
               finds which configured scene is on air, smooths the frame, writes the crop to OBS.
  * detector — grabs low-res screenshots of the raw camera source (~10 Hz), finds people,
               picks the speaker (and a partner in duo mode), decides cuts, watches the camera.
The UI talks to the engine through Qt signals (emitted from worker threads, delivered queued).

Scene modes (settings.scene_modes) — the camera source can sit in several scenes, each scene
item is cropped separately:
  track — follow the speaker;      duo — keep two people in the frame;
  wide  — a calm wide shot of the stage zone.
Строчка switches OBS scenes by what is on screen (song / sermon / Bible), so the modes follow it.
"""

from __future__ import annotations

import logging
import math
import threading
import time
from dataclasses import dataclass, field

import numpy as np
from PySide6.QtCore import QObject, Signal

from speakertrack.appearance import AppearanceMemory, signature
from speakertrack.detector import PoseDetector
from speakertrack.framing import Framer, Person
from speakertrack.obs_client import RESTORE_KEYS, ObsConnection
from speakertrack.settings import Settings
from speakertrack.tracker import SimpleTracker, iou
from speakertrack.watchdog import MESSAGES, CameraWatchdog

log = logging.getLogger(__name__)

MODES = ("track", "duo", "wide")


@dataclass
class PreviewData:
    img: np.ndarray
    people: list = field(default_factory=list)
    target_id: int | None = None
    partner_id: int | None = None
    crop: tuple = (0.0, 0.0, 1.0, 1.0)
    zone: tuple = (0.0, 0.0, 1.0, 1.0)
    active: bool = True
    paused: bool = False
    zoom: float = 1.0
    infer_ms: float = 0.0
    fps: float = 0.0
    scene: str = ""
    mode: str = "track"
    cut: str = ""          # "close" / "wide" when auto-cut is on
    match: float = 0.0     # how sure we are it is the remembered speaker (0..1)
    fault: str = ""


class Engine(QObject):
    preview = Signal(object)          # PreviewData
    status = Signal(str)
    running_changed = Signal(bool)
    active_changed = Signal(bool)
    error = Signal(str)
    device = Signal(str)
    camera_fault = Signal(str)        # "" when fine, else a human message
    speaker_on_air = Signal(str)      # scene name: the speaker is steadily in frame (name plate)

    def __init__(self, settings: Settings):
        super().__init__()
        self.s = settings
        self._stop = threading.Event()
        self._worker: threading.Thread | None = None
        self._lock = threading.Lock()
        self._detector: PoseDetector | None = None
        self._detector_key = None
        self.running = False

        # tracking state (guarded by _lock)
        self.framer: Framer | None = None
        self.tracker = SimpleTracker()
        self.memory = AppearanceMemory()
        self.locked_id: int | None = None
        self.partner_id: int | None = None
        self.last_center = None
        self.last_seen = 0.0
        self.last_people: list[Person] = []
        self.last_sigs: dict = {}
        self.paused = False
        self.active = False
        self.active_scene: str | None = None
        self.mode = "track"
        self.snap = True
        self.match = 0.0
        # auto-cut
        self.cut = "close"
        self.cut_since = 0.0
        self.speed = 0.0
        self._move_since: float | None = None
        self._still_since: float | None = None
        self._prev_cx: tuple | None = None
        # name plate
        self._plate_pending = False
        self._found_since: float | None = None
        # watchdog
        self.watchdog = CameraWatchdog()
        self.fault: str | None = None
        self._switched = False

    # ------------------------------------------------------------------ public API (UI thread)

    def start(self):
        if self.running:
            return
        if not self.s.source or not self.s.modes():
            self.error.emit("Выберите камеру и отметьте, в каких сценах работать.")
            return
        self._stop.clear()
        self.running = True
        self.running_changed.emit(True)
        self._worker = threading.Thread(target=self._run, name="engine", daemon=True)
        self._worker.start()

    def stop(self):
        if not self.running:
            return
        self._stop.set()
        if self._worker:
            self._worker.join(timeout=5)
        self._worker = None

    def set_paused(self, paused: bool):
        with self._lock:
            self.paused = paused

    def retarget(self):
        with self._lock:
            self._reset_target()
            self.memory.reset()

    def lock_at(self, x: float, y: float) -> bool:
        """Lock onto the person under a click (normalized source coords).
        In duo mode a click on someone else picks the second person."""
        with self._lock:
            hits = [p for p in self.last_people if p.contains(x, y)]
            if not hits:
                return False
            p = min(hits, key=lambda p: p.area)
            if self.mode == "duo" and self.locked_id is not None and p.tid != self.locked_id:
                self.partner_id = p.tid
                return True
            self.locked_id = p.tid
            self.last_center = p.center
            self.last_seen = time.time()
            self.memory.reset(self.last_sigs.get(p.tid))
            if self.framer:
                self.framer.reset_target()
            self.paused = False
            return True

    # ------------------------------------------------------------------ internals

    def _reset_target(self):
        self.locked_id = None
        self.partner_id = None
        self.last_center = None
        if self.framer:
            self.framer.reset_target()

    def _get_detector(self) -> PoseDetector:
        key = (str(self.s.model_path()), self.s.use_gpu)
        if self._detector is None or self._detector_key != key:
            self.status.emit("Загрузка нейросети…")
            self._detector = PoseDetector(self.s.model_path(), use_gpu=self.s.use_gpu)
            self._detector_key = key
        self.device.emit(self._detector.device_name)
        return self._detector

    def _connect(self) -> ObsConnection:
        return ObsConnection(self.s.obs_host, self.s.obs_port, self.s.obs_password)

    def _run(self):
        conns = []
        items: dict[str, tuple[int, dict]] = {}
        touched: set[str] = set()
        try:
            self.status.emit("Подключение к OBS…")
            render = self._connect()
            conns.append(render)
            detect = self._connect()
            conns.append(detect)

            source = self.s.source
            modes = self.s.modes()
            for scene in modes:
                try:
                    item_id = render.item_id(scene, source)
                    items[scene] = (item_id, render.item_transform(scene, item_id))
                except Exception:
                    log.info("Scene %s has no %s", scene, source)
            if not items:
                first = next(iter(modes))
                raise RuntimeError(f"В сцене «{first}» нет источника «{source}».")
            canvas = render.canvas_size()
            original = next(iter(items.values()))[1]
            src_w, src_h = int(original.get("sourceWidth", 0)), int(original.get("sourceHeight", 0))
            if not src_w or not src_h:
                raise RuntimeError(f"Источник «{source}» не отдаёт картинку. Камера включена?")

            if src_w < canvas[0] * 1.5:
                self.status.emit(f"Внимание: источник {src_w}×{src_h} — приближение будет мягким")

            detector = self._get_detector()

            with self._lock:
                self.framer = Framer(self.s, src_w / src_h, canvas[0] / canvas[1])
                self.tracker.reset()
                self._reset_target()
                self.last_seen = 0.0
                self.snap = True
                self.active = False
                self.active_scene = None
                self.watchdog.reset()
                self.fault = None

            det = threading.Thread(target=self._detect_loop, args=(detect, detector, src_w, src_h),
                                   name="detector", daemon=True)
            det.start()
            self.status.emit(f"Работает · {src_w}×{src_h} → {canvas[0]}×{canvas[1]}")
            self._render_loop(render, items, touched, canvas, src_w, src_h)
            det.join(timeout=3)
        except Exception as e:
            log.exception("Engine error")
            self.error.emit(str(e) or e.__class__.__name__)
        finally:
            self._stop.set()
            if self.s.restore_on_exit and conns:
                for scene in touched:
                    item_id, original = items[scene]
                    try:
                        conns[0].set_item_transform(scene, item_id,
                                                    {k: original[k] for k in RESTORE_KEYS if k in original})
                    except Exception as e:
                        log.warning("Не удалось восстановить источник в «%s»: %s", scene, e)
            for c in conns:
                c.close()
            self.running = False
            self.active = False
            if self.fault:
                self.camera_fault.emit("")
            self.running_changed.emit(False)
            self.status.emit("Остановлено")

    def _find_active(self, conn: ObsConnection, items: dict) -> str | None:
        """The configured scene that is on air; in Studio Mode the preview counts too,
        so the shot is ready before the transition."""
        program = conn.program_scene()
        if program in items:
            return program
        preview = conn.preview_scene()
        return preview if preview in items else None

    def _render_loop(self, conn, items, touched, canvas, src_w, src_h):
        last = time.time()
        next_check = 0.0
        failures = 0
        while not self._stop.is_set():
            now = time.time()
            interval = 1.0 / max(self.s.render_fps, 1)
            try:
                if now >= next_check:
                    next_check = now + 0.5
                    scene = self._find_active(conn, items)
                    if scene != self.active_scene:
                        mode = self.s.modes().get(scene, "track") if scene else self.mode
                        with self._lock:
                            if scene and not self.active:
                                self._reset_target()
                            self.snap = True
                            self.active_scene = scene
                            self.active = scene is not None
                            self.mode = mode if mode in MODES else "track"
                            self.cut, self.cut_since = "close", now
                            self._plate_pending = scene is not None and self.mode in ("track", "duo")
                            self._found_since = None
                        self.active_changed.emit(scene is not None)

                dt = now - last
                last = now
                with self._lock:
                    scene = self.active_scene
                    send = scene is not None and not self.snap and self.framer is not None
                    f = self.framer.step(dt) if send else None
                    rect = self.framer.rect(f) if send else None
                if rect:
                    x0, y0, x1, y1 = rect
                    conn.set_crop(scene, items[scene][0], canvas,
                                  round(x0 * src_w), round(y0 * src_h),
                                  round(src_w - x1 * src_w), round(src_h - y1 * src_h))
                    touched.add(scene)
                failures = 0
            except Exception as e:
                failures += 1
                log.warning("OBS request failed: %s", e)
                if failures >= 5:
                    raise RuntimeError("Потеряно соединение с OBS.")
                time.sleep(0.5)
            time.sleep(max(0.0, interval - (time.time() - now)))

    # ------------------------------------------------------------------ picking people

    def _in_zone(self, p: Person) -> bool:
        x0, y0, x1, y1 = self.s.zone
        cx, cy = p.center
        return x0 <= cx <= x1 and y0 <= cy <= y1

    def _pick_target(self, people: list[Person], now: float) -> Person | None:
        if not people:
            self.match = 0.0
            return None
        cur = None
        if self.locked_id is not None:
            cur = next((p for p in people if p.tid == self.locked_id), None)

        use_reid = self.s.reid and self.memory.model is not None
        scores = {p.tid: self.memory.score(self.last_sigs.get(p.tid)) for p in people} if use_reid else {}
        if use_reid:
            thr = self.s.reid_threshold
            best = max(people, key=lambda p: scores[p.tid])
            if cur is not None:
                # we followed someone else while the speaker was away — the speaker is back
                if best is not cur and scores[cur.tid] < thr - 0.1 and scores[best.tid] >= thr + 0.05:
                    cur = best
            elif scores[best.tid] >= thr:
                cur = best

        if cur is None:
            if self.last_center is not None and now - self.last_seen < self.s.lost_timeout:
                lx, ly = self.last_center
                cur = min(people, key=lambda p: math.hypot(p.center[0] - lx, p.center[1] - ly))
            else:
                cur = max(people, key=lambda p: p.area)
                self.framer.reset_target()
        self.locked_id = cur.tid

        # learn the speaker's colors only while nobody overlaps them
        if self.s.reid and all(iou(cur.box, o.box) < 0.05 for o in people if o is not cur):
            self.memory.update(self.last_sigs.get(cur.tid))
        self.match = scores.get(cur.tid, self.memory.score(self.last_sigs.get(cur.tid)))
        return cur

    def _pick_partner(self, people: list[Person], target: Person) -> Person | None:
        others = [p for p in people if p is not target]
        if not others:
            return None
        p = next((o for o in others if o.tid == self.partner_id), None)
        if p is None:
            tx = target.center[0]
            # the biggest person, preferring someone near the speaker
            p = max(others, key=lambda o: o.area / (1 + 2 * abs(o.center[0] - tx)))
        self.partner_id = p.tid
        return p

    # ------------------------------------------------------------------ auto-cut

    def _cut_shot(self, target: Person, now: float) -> tuple[str | None, bool]:
        """(shot override, cut now?). Close shot while the speaker stands, wider while they walk."""
        cx = target.center[0]
        if self._prev_cx is not None:
            pcx, pt = self._prev_cx
            v = abs(cx - pcx) / max(now - pt, 1e-3)
            self.speed = 0.7 * self.speed + 0.3 * v
        self._prev_cx = (cx, now)
        if not self.s.auto_cut:
            return None, False

        thr = self.s.cut_move_speed
        can_cut = now - self.cut_since >= self.s.cut_min_hold
        cut = False
        if self.speed > thr:
            self._move_since = self._move_since or now
            self._still_since = None
        elif self.speed < thr * 0.5:
            self._still_since = self._still_since or now
            self._move_since = None
        if self.cut == "close" and self._move_since and now - self._move_since > 0.8 and can_cut:
            self.cut, self.cut_since, cut = "wide", now, True
        elif self.cut == "wide" and self._still_since and now - self._still_since > 2.5 and can_cut:
            self.cut, self.cut_since, cut = "close", now, True
        return (self.s.cut_wide_shot if self.cut == "wide" else None), cut

    # ------------------------------------------------------------------ watchdog

    def _on_frame_health(self, conn: ObsConnection, fault: str | None):
        if fault == self.fault:
            return
        self.fault = fault
        self.camera_fault.emit(MESSAGES.get(fault, "") if fault else "")
        if not fault:
            self._switched = False
            return
        log.warning("Camera fault: %s", fault)
        if self.s.watchdog_switch and self.s.backup_scene and not self._switched:
            try:
                if conn.program_scene() == self.active_scene:
                    conn.set_program_scene(self.s.backup_scene)
                    self.status.emit(f"Камера пропала — включена сцена «{self.s.backup_scene}»")
                self._switched = True
            except Exception as e:
                log.warning("Не удалось переключить на запасную сцену: %s", e)

    # ------------------------------------------------------------------ detection loop

    def _detect_loop(self, conn: ObsConnection, detector: PoseDetector, src_w: int, src_h: int):
        fps = 0.0
        while not self._stop.is_set():
            t0 = time.time()
            if not self.active:
                time.sleep(0.2)
                continue
            try:
                # Ask OBS for exactly the model input size: no resize on our side.
                scale = min(detector.size / src_w, detector.size / src_h, 1.0)
                img = conn.screenshot(self.s.source, round(src_w * scale), round(src_h * scale))
            except Exception as e:
                log.warning("Screenshot failed: %s", e)
                if self.s.watchdog:
                    self._on_frame_health(conn, self.watchdog.failure(time.time()))
                time.sleep(0.5)
                continue
            if img is None:
                continue
            if self.s.watchdog:
                self._on_frame_health(conn, self.watchdog.frame(img, time.time()))

            t_inf = time.time()
            people = detector.detect(img, self.s.confidence)
            infer_ms = (time.time() - t_inf) * 1000
            people = [p for p in people if self._in_zone(p)]

            with self._lock:
                self.tracker.update(people)
                self.last_people = people
                self.last_sigs = ({p.tid: signature(img, p, self.s.kp_conf) for p in people}
                                  if self.s.reid else {})
                fr = self.framer
                now = time.time()
                mode = self.mode
                target = partner = None
                shot = None
                cut_now = False

                if mode == "wide" or self.paused:
                    fr.set_goal(fr.zone_frame(), snap=self.snap) if self.snap else fr.force_goal(fr.zone_frame())
                    self.snap = False
                else:
                    target = self._pick_target(people, now)
                    if target is not None:
                        self.last_center = target.center
                        self.last_seen = now
                        if mode == "duo":
                            partner = self._pick_partner(people, target)
                            goal = fr.frame_for_group([target, partner] if partner else [target])
                        else:
                            shot, cut_now = self._cut_shot(target, now)
                            goal = fr.frame_for(target, shot=shot)
                        fr.set_goal(goal, snap=self.snap or cut_now)
                        self.snap = False
                        self._plate_check(now)
                    else:
                        self._found_since = None
                        if self.s.wide_on_lost and now - self.last_seen > self.s.lost_timeout:
                            self.locked_id = None
                            if self.snap:
                                fr.set_goal(fr.zone_frame(), snap=True)
                                self.snap = False
                            else:
                                fr.force_goal(fr.zone_frame())
                crop = fr.rect(fr.current)
                zoom = 1.0 / fr.current.h
                data = dict(target_id=target.tid if target else None,
                            partner_id=partner.tid if partner else None,
                            scene=self.active_scene or "", mode=mode, match=self.match,
                            cut=self.cut if (self.s.auto_cut and mode == "track") else "",
                            fault=self.fault or "")

            dt = time.time() - t0
            fps = 0.8 * fps + 0.2 * (1.0 / max(dt, 1e-3)) if fps else 1.0 / max(dt, 1e-3)
            self.preview.emit(PreviewData(img=img, people=people, crop=crop, zone=tuple(self.s.zone),
                                          active=self.active, paused=self.paused, zoom=zoom,
                                          infer_ms=infer_ms, fps=fps, **data))

            interval = 1.0 / max(self.s.detect_fps, 1)
            dt = time.time() - t0
            if dt < interval:
                time.sleep(interval - dt)

    def _plate_check(self, now: float):
        """Name plate: once per scene activation, when the speaker has been in frame for a while."""
        if not self._plate_pending:
            return
        self._found_since = self._found_since or now
        if now - self._found_since >= self.s.lt_delay:
            self._plate_pending = False
            if self.s.lt_auto and self.s.lt_name.strip() and self.active_scene:
                self.speaker_on_air.emit(self.active_scene)
