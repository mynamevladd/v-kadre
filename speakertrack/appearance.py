"""
Remembering the speaker by clothing color.

A person is described by a small color histogram of the torso (shoulders to hips): hue bins for
colored pixels plus brightness bins for grey/black/white ones. Cheap enough to compute for every
person on every detection frame, and stable when the person turns sideways or walks.
"""

from __future__ import annotations

import numpy as np

from speakertrack.framing import Person

HUE_BINS = 16
GREY_BINS = 4


def _hsv(rgb: np.ndarray):
    """uint8 RGB (n, 3) -> hue 0..1, saturation 0..1, value 0..1."""
    c = rgb.astype(np.float32) / 255.0
    r, g, b = c[:, 0], c[:, 1], c[:, 2]
    mx = c.max(axis=1)
    mn = c.min(axis=1)
    d = mx - mn
    s = np.where(mx > 0, d / np.maximum(mx, 1e-6), 0.0)
    h = np.zeros_like(mx)
    nz = d > 1e-6
    rm = nz & (mx == r)
    gm = nz & (mx == g) & ~rm
    bm = nz & ~rm & ~gm
    h[rm] = ((g[rm] - b[rm]) / d[rm]) % 6
    h[gm] = (b[gm] - r[gm]) / d[gm] + 2
    h[bm] = (r[bm] - g[bm]) / d[bm] + 4
    return h / 6.0, s, mx


def torso_region(p: Person, kp_conf: float = 0.5) -> tuple[float, float, float, float]:
    """Normalized box of the torso: between the shoulders, from shoulders to hips."""
    x0, y0, x1, y1 = p.box
    if p.kxy is not None and p.kconf is not None:
        sh = [p.kxy[i] for i in (5, 6) if p.kconf[i] >= kp_conf]
        hip = [p.kxy[i] for i in (11, 12) if p.kconf[i] >= kp_conf]
        if len(sh) == 2:
            sx0, sx1 = sorted((sh[0][0], sh[1][0]))
            sy = (sh[0][1] + sh[1][1]) / 2
            hy = np.mean([h[1] for h in hip]) if hip else sy + (y1 - y0) * 0.3
            if sx1 - sx0 > 0.004 and hy > sy:
                inset = (sx1 - sx0) * 0.1
                return sx0 + inset, sy + (hy - sy) * 0.1, sx1 - inset, hy
    w, h = x1 - x0, y1 - y0
    return x0 + w * 0.25, y0 + h * 0.22, x1 - w * 0.25, y0 + h * 0.55


def signature(img: np.ndarray, p: Person, kp_conf: float = 0.5) -> np.ndarray | None:
    """Normalized color histogram of the person's torso, or None if the region is too small."""
    ih, iw = img.shape[:2]
    x0, y0, x1, y1 = torso_region(p, kp_conf)
    a, b = int(max(0, x0) * iw), int(min(1, x1) * iw)
    c, d = int(max(0, y0) * ih), int(min(1, y1) * ih)
    if b - a < 3 or d - c < 3:
        return None
    px = img[c:d, a:b].reshape(-1, 3)
    if len(px) > 1500:  # plenty for a histogram
        px = px[:: len(px) // 1500 + 1]
    h, s, v = _hsv(px)
    colored = (s > 0.25) & (v > 0.18)
    hue_hist = np.bincount(np.minimum((h[colored] * HUE_BINS).astype(int), HUE_BINS - 1),
                           minlength=HUE_BINS)
    grey_hist = np.bincount(np.minimum((v[~colored] * GREY_BINS).astype(int), GREY_BINS - 1),
                            minlength=GREY_BINS)
    hist = np.concatenate([hue_hist, grey_hist]).astype(np.float32)
    total = hist.sum()
    return hist / total if total else None


def similarity(a: np.ndarray | None, b: np.ndarray | None) -> float:
    """Bhattacharyya coefficient: 1 = same colors, 0 = nothing in common."""
    if a is None or b is None:
        return 0.0
    return float(np.sum(np.sqrt(a * b)))


class AppearanceMemory:
    """Color model of the chosen speaker, updated slowly while we are sure it is them."""

    def __init__(self, rate: float = 0.08):
        self.model: np.ndarray | None = None
        self.rate = rate

    def reset(self, sig: np.ndarray | None = None):
        self.model = None if sig is None else sig.copy()

    def score(self, sig: np.ndarray | None) -> float:
        return similarity(self.model, sig)

    def update(self, sig: np.ndarray | None):
        if sig is None:
            return
        if self.model is None:
            self.model = sig.copy()
        elif self.score(sig) >= 0.5:  # never drift onto someone else
            self.model = (1 - self.rate) * self.model + self.rate * sig
            self.model /= self.model.sum()
