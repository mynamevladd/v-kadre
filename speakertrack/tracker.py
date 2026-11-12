"""
Lightweight multi-person tracker: keeps stable IDs between frames.

Greedy matching by IoU, falling back to center distance, with a short memory for
people that disappear for a few frames (occlusion by the pulpit, other people walking by).
That's all the speaker lock needs; no heavy ByteTrack dependency.
"""

from __future__ import annotations

import math
from dataclasses import dataclass

from speakertrack.framing import Person


def iou(a, b) -> float:
    ix0, iy0 = max(a[0], b[0]), max(a[1], b[1])
    ix1, iy1 = min(a[2], b[2]), min(a[3], b[3])
    inter = max(0.0, ix1 - ix0) * max(0.0, iy1 - iy0)
    if inter <= 0:
        return 0.0
    ua = (a[2] - a[0]) * (a[3] - a[1]) + (b[2] - b[0]) * (b[3] - b[1]) - inter
    return inter / ua if ua > 0 else 0.0


@dataclass
class _Track:
    tid: int
    box: tuple
    missed: int = 0


class SimpleTracker:
    def __init__(self, iou_thr: float = 0.25, max_dist: float = 0.12, max_missed: int = 15):
        self.iou_thr = iou_thr
        self.max_dist = max_dist          # normalized center distance for non-overlapping match
        self.max_missed = max_missed      # frames a lost track is remembered
        self.tracks: list[_Track] = []
        self._next_id = 1

    def reset(self):
        self.tracks.clear()

    def update(self, people: list[Person]) -> list[Person]:
        pairs = []
        for ti, t in enumerate(self.tracks):
            tcx, tcy = (t.box[0] + t.box[2]) / 2, (t.box[1] + t.box[3]) / 2
            for pi, p in enumerate(people):
                o = iou(t.box, p.box)
                cx, cy = p.center
                d = math.hypot(cx - tcx, cy - tcy)
                if o >= self.iou_thr or d <= self.max_dist:
                    # Prefer overlap; distance breaks ties.
                    pairs.append((-(o) + d * 0.5, ti, pi))
        pairs.sort()

        used_t, used_p = set(), set()
        for _, ti, pi in pairs:
            if ti in used_t or pi in used_p:
                continue
            used_t.add(ti)
            used_p.add(pi)
            t = self.tracks[ti]
            t.box, t.missed = people[pi].box, 0
            people[pi].tid = t.tid

        for ti, t in enumerate(self.tracks):
            if ti not in used_t:
                t.missed += 1
        self.tracks = [t for t in self.tracks if t.missed <= self.max_missed]

        for pi, p in enumerate(people):
            if pi not in used_p:
                p.tid = self._next_id
                self.tracks.append(_Track(self._next_id, p.box))
                self._next_id += 1
        return people
