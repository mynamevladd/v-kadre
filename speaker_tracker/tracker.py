"""
Speaker tracker for OBS: virtual camera operator over a fixed 4K camera.

How it works:
  1. Grabs low-res frames of the camera source via obs-websocket (GetSourceScreenshot).
     Screenshots are of the raw source, so our own crop never feeds back into detection.
  2. YOLO-pose finds people and their skeleton keypoints; ByteTrack keeps the same person locked.
  3. Framing is built from HEAD + TORSO (eyes/ears/shoulders/hips), not the whole-body box,
     so raised hands, gestures and a pulpit hiding the legs don't make the shot jump.
  4. Dead zones + smoothing, crop written to OBS ~30 times per second.
  5. Works only while one of `active_scenes` is on Program (or Preview in Studio Mode).

Run:  python tracker.py [--config config.json] [--preview]
"""

import argparse
import base64
import json
import math
import signal
import sys
import threading
import time
from pathlib import Path

import cv2
import numpy as np
import obsws_python as obs
from ultralytics import YOLO

# COCO keypoint indices
HEAD_KP = [0, 1, 2, 3, 4]      # nose, eyes, ears
SHOULDER_KP = [5, 6]
HIP_KP = [11, 12]

# Where the bottom edge of the shot cuts the body, in torso lengths below the shoulder line.
SHOTS = {
    "chest": 0.6,   # по грудь
    "waist": 1.0,   # по пояс
    "hips": 1.4,    # чуть ниже пояса
    "knee": 2.1,    # по колено
}

HEAD_TOP_ABOVE_NOSE = 0.25   # top of head is ~0.25 torso lengths above the nose
NOSE_TO_SHOULDER = 0.4       # nose-to-shoulder-line distance in torso lengths


# ----------------------------------------------------------------------------- helpers

def clamp(v, lo, hi):
    return max(lo, min(hi, v))


def load_config(path):
    with open(path, "r", encoding="utf-8") as f:
        return json.load(f)


class Frame:
    """Crop window in normalized source coordinates (0..1). cx, cy = center, h = height fraction."""

    def __init__(self, cx=0.5, cy=0.5, h=1.0):
        self.cx, self.cy, self.h = cx, cy, h

    def copy(self):
        return Frame(self.cx, self.cy, self.h)


class Person:
    def __init__(self, tid, box, kxy=None, kconf=None):
        self.tid = tid
        self.x0, self.y0, self.x1, self.y1 = box
        self.kxy = kxy        # (17, 2) normalized, or None
        self.kconf = kconf    # (17,), or None

    @property
    def center(self):
        return (self.x0 + self.x1) / 2, (self.y0 + self.y1) / 2

    def _mean(self, idx, thr):
        if self.kxy is None:
            return None
        pts = [self.kxy[i] for i in idx if self.kconf[i] >= thr]
        if not pts:
            return None
        return float(np.mean([p[0] for p in pts])), float(np.mean([p[1] for p in pts]))

    def pose(self, thr):
        """Returns (cx, head_y, shoulder_y, torso) or None if not enough keypoints."""
        head = self._mean(HEAD_KP, thr)
        sh = self._mean(SHOULDER_KP, thr)
        hip = self._mean(HIP_KP, thr)

        torso = None
        if sh and hip and hip[1] - sh[1] > 0.01:
            torso = hip[1] - sh[1]
        elif head and sh and sh[1] - head[1] > 0.005:
            torso = (sh[1] - head[1]) / NOSE_TO_SHOULDER
        if torso is None:
            return None

        if head is None:
            head = (sh[0], sh[1] - NOSE_TO_SHOULDER * torso)
        if sh is None:
            sh = (head[0], head[1] + NOSE_TO_SHOULDER * torso)
        cx = (head[0] + sh[0]) / 2
        return cx, head[1], sh[1], torso


# ----------------------------------------------------------------------------- tracker

class SpeakerTracker:
    def __init__(self, cfg, preview=False):
        self.cfg = cfg
        self.preview = preview
        self.stop_event = threading.Event()
        self.lock = threading.Lock()

        o = cfg["obs"]
        # Two clients: one for screenshots (detector thread), one for transforms (render loop).
        self.cl_detect = obs.ReqClient(host=o["host"], port=o["port"], password=o["password"], timeout=5)
        self.cl_render = obs.ReqClient(host=o["host"], port=o["port"], password=o["password"], timeout=5)

        self.scene = cfg["scene"]
        self.source = cfg["source"]
        self.item_id = self.cl_render.get_scene_item_id(self.scene, self.source).scene_item_id

        vs = self.cl_render.get_video_settings()
        self.canvas_w, self.canvas_h = vs.base_width, vs.base_height

        tr = self.cl_render.get_scene_item_transform(self.scene, self.item_id).scene_item_transform
        self.original_transform = tr
        self.src_w, self.src_h = int(tr["sourceWidth"]), int(tr["sourceHeight"])
        if self.src_w == 0 or self.src_h == 0:
            raise RuntimeError(f"Источник '{self.source}' не отдаёт картинку (размер 0). Камера включена?")

        self.src_aspect = self.src_w / self.src_h
        self.out_aspect = self.canvas_w / self.canvas_h

        print(f"OBS: холст {self.canvas_w}x{self.canvas_h}, источник {self.src_w}x{self.src_h}")
        if self.src_w < self.canvas_w * 1.5:
            print("ВНИМАНИЕ: разрешение источника небольшое, приближение будет мылить картинку.")

        if cfg["shot"] not in SHOTS:
            raise RuntimeError(f"shot должен быть одним из: {', '.join(SHOTS)}")

        self.model = YOLO(cfg["model"])

        active = cfg.get("active_scenes") or []
        print("Слежение активно в сценах:", ", ".join(active) if active else "всегда")

        # Shared state
        self.goal = Frame()      # where the virtual camera wants to be (after dead zones)
        self.current = Frame()   # where it is now (smoothed)
        self.locked_id = None
        self.last_center = None
        self.last_seen = 0.0
        self.torso = None        # smoothed torso length of locked person
        self.paused = False
        self.active = not active
        self.snap = True         # next detection jumps instantly (no pan) — used on scene activation

    # ------------------------------------------------------------------ scene gating

    def check_active(self):
        names = self.cfg.get("active_scenes") or []
        if not names:
            return True
        r = self.cl_render.get_current_program_scene()
        prog = getattr(r, "current_program_scene_name", None) or getattr(r, "scene_name", None)
        if prog in names:
            return True
        try:  # Studio Mode: also track while the scene sits in Preview, so it is ready before the cut
            r = self.cl_render.get_current_preview_scene()
            prev = getattr(r, "current_preview_scene_name", None) or getattr(r, "scene_name", None)
            return prev in names
        except Exception:
            return False

    def reset_target(self):
        self.locked_id = None
        self.last_center = None
        self.torso = None

    # ------------------------------------------------------------------ detection thread

    def grab_frame(self):
        w = self.cfg["detect_width"]
        h = int(round(w / self.src_aspect))
        resp = self.cl_detect.get_source_screenshot(self.source, "jpg", w, h, 70)
        data = resp.image_data
        b64 = data.split(",", 1)[1] if "," in data else data
        buf = np.frombuffer(base64.b64decode(b64), dtype=np.uint8)
        return cv2.imdecode(buf, cv2.IMREAD_COLOR)

    def in_zone(self, cx, cy):
        x0, y0, x1, y1 = self.cfg["zone"]
        return x0 <= cx <= x1 and y0 <= cy <= y1

    def pick_target(self, people):
        if not people:
            return None
        if self.locked_id is not None:
            for p in people:
                if p.tid == self.locked_id:
                    return p
        # Lost the locked ID (or none yet): prefer closest to last known position, else biggest.
        if self.last_center is not None and time.time() - self.last_seen < self.cfg["lost_timeout"]:
            lx, ly = self.last_center
            best = min(people, key=lambda p: math.hypot(p.center[0] - lx, p.center[1] - ly))
        else:
            best = max(people, key=lambda p: (p.x1 - p.x0) * (p.y1 - p.y0))
            self.torso = None
        self.locked_id = best.tid
        return best

    def zoom_to_h(self, zoom):
        c = self.cfg
        return 1.0 / clamp(zoom, c["min_zoom"], c["max_zoom"])

    def frame_for_person(self, p):
        c = self.cfg
        pose = p.pose(c["kp_conf"])

        if pose is None:
            # No usable skeleton (e.g. far away / heavy occlusion): follow the box, keep current zoom.
            with self.lock:
                h = self.goal.h
            cx = (p.x0 + p.x1) / 2
            top = p.y0 - c["headroom"] * h
            return Frame(cx, top + h / 2, h)

        cx, head_y, shoulder_y, torso = pose
        a = c["torso_smoothing"]
        self.torso = torso if self.torso is None else self.torso * a + torso * (1 - a)
        t = self.torso

        head_top = head_y - HEAD_TOP_ABOVE_NOSE * t
        if c["zoom_mode"] == "auto":
            cut = shoulder_y + SHOTS[c["shot"]] * t
            h = (cut - head_top) / (1 - c["headroom"])
            h = self.zoom_to_h(1.0 / max(h, 1e-3))
        else:
            h = self.zoom_to_h(c["fixed_zoom"])

        top = head_top - c["headroom"] * h
        return Frame(cx, top + h / 2, h)

    def update_goal(self, f):
        """Apply dead zones: only move goal when target drifts far enough."""
        c = self.cfg
        with self.lock:
            if self.snap:
                self.goal = f.copy()
                self.current = self.clamp_frame(f.copy())
                self.snap = False
                return
            g = self.goal
            w = self.crop_width(g.h)
            if abs(f.cx - g.cx) > c["deadzone_x"] * w:
                g.cx = f.cx
            if abs(f.cy - g.cy) > c["deadzone_y"] * g.h:
                g.cy = f.cy
            if abs(f.h - g.h) / g.h > c["deadzone_zoom"]:
                g.h = f.h

    def go_wide(self):
        self.locked_id = None
        with self.lock:
            self.goal = Frame(0.5, 0.5, 1.0 / self.cfg["wide_zoom"])
            if self.snap:
                self.current = self.clamp_frame(self.goal.copy())
                self.snap = False

    def detect_loop(self):
        c = self.cfg
        interval = 1.0 / c["detect_fps"]
        while not self.stop_event.is_set():
            t0 = time.time()
            if not self.active:
                time.sleep(0.2)
                continue
            try:
                img = self.grab_frame()
            except Exception as e:
                print("Ошибка скриншота:", e)
                time.sleep(1)
                continue
            if img is None:
                continue
            ih, iw = img.shape[:2]

            res = self.model.track(img, persist=True, classes=[0], conf=c["confidence"],
                                   tracker="bytetrack.yaml", imgsz=c["imgsz"], verbose=False)[0]
            people = []
            if res.boxes is not None and res.boxes.id is not None:
                boxes = res.boxes.xyxy.cpu().numpy()
                ids = res.boxes.id.cpu().numpy().astype(int)
                kxy = kconf = None
                if res.keypoints is not None and res.keypoints.conf is not None:
                    kxy = res.keypoints.xy.cpu().numpy() / np.array([iw, ih])
                    kconf = res.keypoints.conf.cpu().numpy()
                for i, ((bx0, by0, bx1, by1), tid) in enumerate(zip(boxes, ids)):
                    p = Person(tid, (bx0 / iw, by0 / ih, bx1 / iw, by1 / ih),
                               None if kxy is None else kxy[i], None if kconf is None else kconf[i])
                    if self.in_zone(*p.center):
                        people.append(p)

            target = None if self.paused else self.pick_target(people)
            now = time.time()
            if target is not None:
                self.last_center = target.center
                self.last_seen = now
                self.update_goal(self.frame_for_person(target))
            elif self.paused or (c["wide_on_lost"] and now - self.last_seen > c["lost_timeout"]):
                self.go_wide()

            if self.preview:
                self.draw_preview(img, people, target)

            dt = time.time() - t0
            if dt < interval:
                time.sleep(interval - dt)

    def draw_preview(self, img, people, target):
        ih, iw = img.shape[:2]
        x0, y0, x1, y1 = self.cfg["zone"]
        cv2.rectangle(img, (int(x0 * iw), int(y0 * ih)), (int(x1 * iw), int(y1 * ih)), (255, 255, 0), 1)
        thr = self.cfg["kp_conf"]
        for p in people:
            is_t = target is not None and p.tid == target.tid
            color = (0, 0, 255) if is_t else (0, 200, 0)
            cv2.rectangle(img, (int(p.x0 * iw), int(p.y0 * ih)), (int(p.x1 * iw), int(p.y1 * ih)), color, 1)
            cv2.putText(img, f"id {p.tid}", (int(p.x0 * iw), int(p.y0 * ih) - 5),
                        cv2.FONT_HERSHEY_SIMPLEX, 0.5, color, 1)
            if p.kxy is not None:
                for i in HEAD_KP + SHOULDER_KP + HIP_KP:
                    if p.kconf[i] >= thr:
                        cv2.circle(img, (int(p.kxy[i][0] * iw), int(p.kxy[i][1] * ih)), 3, color, -1)
        with self.lock:
            cur = self.current.copy()
        w = self.crop_width(cur.h)
        cv2.rectangle(img, (int((cur.cx - w / 2) * iw), int((cur.cy - cur.h / 2) * ih)),
                      (int((cur.cx + w / 2) * iw), int((cur.cy + cur.h / 2) * ih)), (0, 255, 255), 2)
        status = "PAUSED (wide)" if self.paused else f"lock: {self.locked_id}  zoom {1 / cur.h:.2f}x"
        cv2.putText(img, f"{status}   [r] new target  [p] pause  [q] quit",
                    (10, 20), cv2.FONT_HERSHEY_SIMPLEX, 0.5, (255, 255, 255), 1)
        cv2.imshow("Speaker tracker", img)
        key = cv2.waitKey(1) & 0xFF
        if key == ord("q"):
            self.stop_event.set()
        elif key == ord("r"):
            self.reset_target()
        elif key == ord("p"):
            self.paused = not self.paused

    # ------------------------------------------------------------------ render loop

    def crop_width(self, h):
        return h * self.out_aspect / self.src_aspect

    def clamp_frame(self, f):
        w = self.crop_width(f.h)
        if w > 1.0:  # too wide for source: shrink to fit
            f.h = f.h / w
            w = 1.0
        f.h = min(f.h, 1.0)
        f.cx = clamp(f.cx, w / 2, 1 - w / 2)
        f.cy = clamp(f.cy, f.h / 2, 1 - f.h / 2)
        return f

    def smooth_step(self, dt):
        c = self.cfg
        a_pos = 1 - math.exp(-dt / c["smooth_time"])
        a_zoom = 1 - math.exp(-dt / c["zoom_smooth_time"])
        max_d = c["max_speed"] * dt
        with self.lock:
            g = self.clamp_frame(self.goal.copy())
            cur = self.current
            cur.cx += clamp((g.cx - cur.cx) * a_pos, -max_d, max_d)
            cur.cy += clamp((g.cy - cur.cy) * a_pos, -max_d, max_d)
            cur.h += (g.h - cur.h) * a_zoom
            self.clamp_frame(cur)
            return cur.copy()

    def apply(self, f):
        w = self.crop_width(f.h)
        left = (f.cx - w / 2) * self.src_w
        top = (f.cy - f.h / 2) * self.src_h
        right = self.src_w - (f.cx + w / 2) * self.src_w
        bottom = self.src_h - (f.cy + f.h / 2) * self.src_h
        self.cl_render.set_scene_item_transform(self.scene, self.item_id, {
            "positionX": 0.0,
            "positionY": 0.0,
            "alignment": 5,  # top-left
            "rotation": 0.0,
            "boundsType": "OBS_BOUNDS_SCALE_INNER",
            "boundsAlignment": 0,
            "boundsWidth": float(self.canvas_w),
            "boundsHeight": float(self.canvas_h),
            "cropLeft": max(0, int(round(left))),
            "cropTop": max(0, int(round(top))),
            "cropRight": max(0, int(round(right))),
            "cropBottom": max(0, int(round(bottom))),
        })

    def render_loop(self):
        interval = 1.0 / self.cfg["render_fps"]
        last = time.time()
        next_check = 0.0
        while not self.stop_event.is_set():
            now = time.time()
            if now >= next_check:
                next_check = now + 0.5
                try:
                    active = self.check_active()
                except Exception as e:
                    print("Ошибка запроса сцены:", e)
                    active = self.active
                if active and not self.active:
                    print("Сцена проповеди активна — слежение включено.")
                    self.reset_target()
                    self.snap = True
                elif not active and self.active:
                    print("Сцена неактивна — слежение на паузе.")
                self.active = active

            dt = now - last
            last = now
            if self.active and not self.snap:
                f = self.smooth_step(dt)
                try:
                    self.apply(f)
                except Exception as e:
                    print("Ошибка отправки в OBS:", e)
                    time.sleep(1)
            time.sleep(max(0.0, interval - (time.time() - now)))

    # ------------------------------------------------------------------ lifecycle

    def restore(self):
        t = self.original_transform
        keys = ["positionX", "positionY", "alignment", "rotation", "boundsType", "boundsAlignment",
                "boundsWidth", "boundsHeight", "cropLeft", "cropTop", "cropRight", "cropBottom",
                "scaleX", "scaleY"]
        try:
            self.cl_render.set_scene_item_transform(self.scene, self.item_id, {k: t[k] for k in keys if k in t})
            print("Исходное положение источника восстановлено.")
        except Exception as e:
            print("Не удалось восстановить источник:", e)

    def run(self):
        det = threading.Thread(target=self.detect_loop, daemon=True)
        det.start()
        print("Слежение запущено. Ctrl+C — остановить.")
        try:
            self.render_loop()
        finally:
            self.stop_event.set()
            det.join(timeout=2)
            if self.cfg["restore_on_exit"]:
                self.restore()
            if self.preview:
                cv2.destroyAllWindows()


def main():
    ap = argparse.ArgumentParser(description="OBS speaker tracker")
    ap.add_argument("--config", default=str(Path(__file__).with_name("config.json")))
    ap.add_argument("--preview", action="store_true", help="окно с детекцией для настройки")
    args = ap.parse_args()

    cfg = load_config(args.config)
    try:
        tracker = SpeakerTracker(cfg, preview=args.preview)
    except Exception as e:
        print("Не удалось запуститься:", e)
        print("Проверьте: OBS запущен, WebSocket включён, имена сцены/источника в config.json совпадают.")
        sys.exit(1)

    signal.signal(signal.SIGINT, lambda *_: tracker.stop_event.set())
    tracker.run()


if __name__ == "__main__":
    main()
