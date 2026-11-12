"""
Pure framing math: where the virtual camera should look, given a detected person.

All coordinates are normalized to the camera source: x, y in 0..1.
A crop window ("Frame") is described by its center (cx, cy) and its height fraction h
(h = 1 / zoom). Its width follows from the output and source aspect ratios.
"""

from __future__ import annotations

import math
from dataclasses import dataclass

import numpy as np

# COCO-17 keypoint indices
HEAD_KP = (0, 1, 2, 3, 4)       # nose, eyes, ears
SHOULDER_KP = (5, 6)
HIP_KP = (11, 12)

# Where the bottom edge of the shot cuts the body, in torso lengths below the shoulder line.
SHOTS = {
    "chest": 0.6,   # по грудь
    "waist": 1.0,   # по пояс
    "hips": 1.4,    # чуть ниже пояса
    "knee": 2.1,    # по колено
    "full": 3.4,    # в полный рост
}

HEAD_TOP_ABOVE_NOSE = 0.25   # top of head is ~0.25 torso lengths above the nose
NOSE_TO_SHOULDER = 0.4       # nose-to-shoulder-line distance in torso lengths


def clamp(v: float, lo: float, hi: float) -> float:
    return max(lo, min(hi, v))


@dataclass
class Frame:
    cx: float = 0.5
    cy: float = 0.5
    h: float = 1.0

    def copy(self) -> "Frame":
        return Frame(self.cx, self.cy, self.h)


@dataclass
class Person:
    """One detected person. box = (x0, y0, x1, y1); kxy (17, 2) and kconf (17,) may be None."""
    box: tuple
    score: float = 1.0
    kxy: np.ndarray | None = None
    kconf: np.ndarray | None = None
    tid: int = -1

    @property
    def center(self) -> tuple[float, float]:
        x0, y0, x1, y1 = self.box
        return (x0 + x1) / 2, (y0 + y1) / 2

    @property
    def area(self) -> float:
        x0, y0, x1, y1 = self.box
        return max(0.0, x1 - x0) * max(0.0, y1 - y0)

    def contains(self, x: float, y: float) -> bool:
        x0, y0, x1, y1 = self.box
        return x0 <= x <= x1 and y0 <= y <= y1

    def _mean(self, idx, thr):
        if self.kxy is None or self.kconf is None:
            return None
        pts = [self.kxy[i] for i in idx if self.kconf[i] >= thr]
        if not pts:
            return None
        return float(np.mean([p[0] for p in pts])), float(np.mean([p[1] for p in pts]))

    def pose(self, thr: float):
        """(cx, head_y, shoulder_y, torso) built from head and torso keypoints, or None."""
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
        return (head[0] + sh[0]) / 2, head[1], sh[1], torso


class Framer:
    """Turns a target person into a crop window, with dead zones and smoothing."""

    def __init__(self, settings, src_aspect: float, out_aspect: float):
        self.s = settings
        self.src_aspect = src_aspect
        self.out_aspect = out_aspect
        self.goal = Frame()
        self.current = Frame()
        self.torso: float | None = None

    # -- geometry ---------------------------------------------------------------

    def crop_width(self, h: float) -> float:
        return h * self.out_aspect / self.src_aspect

    def clamp_frame(self, f: Frame) -> Frame:
        w = self.crop_width(f.h)
        if w > 1.0:  # wider than the source: shrink to fit
            f.h /= w
            w = 1.0
        f.h = min(f.h, 1.0)
        f.cx = clamp(f.cx, w / 2, 1 - w / 2)
        f.cy = clamp(f.cy, f.h / 2, 1 - f.h / 2)
        return f

    def rect(self, f: Frame) -> tuple[float, float, float, float]:
        w = self.crop_width(f.h)
        return f.cx - w / 2, f.cy - f.h / 2, f.cx + w / 2, f.cy + f.h / 2

    def zoom_to_h(self, zoom: float) -> float:
        return 1.0 / clamp(zoom, self.s.min_zoom, self.s.max_zoom)

    # -- target -> desired frame --------------------------------------------------

    def reset_target(self):
        self.torso = None

    def frame_for(self, p: Person, shot: str | None = None, smooth: bool = True) -> Frame:
        """Frame for one person. `shot` overrides the configured shot (auto-cut uses it);
        smooth=False leaves the torso filter alone (used for the second person in a duo)."""
        s = self.s
        pose = p.pose(s.kp_conf)
        if pose is None:
            # No usable skeleton: follow the box top, keep current zoom.
            h = self.goal.h
            x0, y0, x1, _ = p.box
            top = y0 - s.headroom * h
            return Frame((x0 + x1) / 2, top + h / 2, h)

        cx, head_y, shoulder_y, torso = pose
        if smooth:
            a = s.torso_smoothing
            self.torso = torso if self.torso is None else self.torso * a + torso * (1 - a)
            t = self.torso
        else:
            t = torso

        head_top = head_y - HEAD_TOP_ABOVE_NOSE * t
        if s.zoom_mode == "auto" or shot:
            cut = shoulder_y + SHOTS.get(shot or s.shot, 1.0) * t
            h = (cut - head_top) / (1 - s.headroom)
            h = self.zoom_to_h(1.0 / max(h, 1e-3))
        else:
            h = self.zoom_to_h(s.fixed_zoom)

        top = head_top - s.headroom * h
        return Frame(cx, top + h / 2, h)

    def wide_frame(self) -> Frame:
        return Frame(0.5, 0.5, 1.0 / max(self.s.wide_zoom, 1.0))

    def zone_frame(self) -> Frame:
        """Wide shot of the stage zone (used by the «Общий» scene mode)."""
        x0, y0, x1, y1 = self.s.zone
        if (x0, y0, x1, y1) == (0.0, 0.0, 1.0, 1.0):
            return self.wide_frame()
        w = (x1 - x0) * 1.06
        h = max((y1 - y0) * 1.06, w * self.src_aspect / self.out_aspect)
        return self.clamp_frame(Frame((x0 + x1) / 2, (y0 + y1) / 2, min(h, 1.0)))

    def frame_for_group(self, people: list[Person]) -> Frame:
        """One frame that holds everybody (interview / duo): heads with headroom on top,
        each person's shot cut at the bottom, side padding, then fit the output aspect."""
        if len(people) == 1:
            return self.frame_for(people[0])
        frames = [self.frame_for(p, smooth=(i == 0)) for i, p in enumerate(people)]
        top = min(f.cy - f.h / 2 for f in frames)
        bottom = max(f.cy + f.h / 2 for f in frames)
        widths = [p.box[2] - p.box[0] for p in people]
        pad = 0.6 * (sum(widths) / len(widths))
        left = min(p.box[0] for p in people) - pad
        right = max(p.box[2] for p in people) + pad
        h = max(bottom - top, (right - left) * self.src_aspect / self.out_aspect)
        h = clamp(h, 1.0 / self.s.max_zoom, 1.0)
        return Frame((left + right) / 2, top + h / 2, h)

    # -- goal update with dead zones ----------------------------------------------

    def set_goal(self, f: Frame, snap: bool = False):
        if snap:
            self.goal = f.copy()
            self.current = self.clamp_frame(f.copy())
            return
        s = self.s
        g = self.goal
        w = self.crop_width(g.h)
        if abs(f.cx - g.cx) > s.deadzone_x * w:
            g.cx = f.cx
        if abs(f.cy - g.cy) > s.deadzone_y * g.h:
            g.cy = f.cy
        if abs(f.h - g.h) / g.h > s.deadzone_zoom:
            g.h = f.h

    def force_goal(self, f: Frame):
        self.goal = f.copy()

    # -- smoothing ----------------------------------------------------------------

    def step(self, dt: float) -> Frame:
        s = self.s
        a_pos = 1 - math.exp(-dt / max(s.smooth_time, 1e-3))
        a_zoom = 1 - math.exp(-dt / max(s.zoom_smooth_time, 1e-3))
        max_d = s.max_speed * dt
        g = self.clamp_frame(self.goal.copy())
        cur = self.current
        cur.cx += clamp((g.cx - cur.cx) * a_pos, -max_d, max_d)
        cur.cy += clamp((g.cy - cur.cy) * a_pos, -max_d, max_d)
        cur.h += (g.h - cur.h) * a_zoom
        self.clamp_frame(cur)
        return cur.copy()
