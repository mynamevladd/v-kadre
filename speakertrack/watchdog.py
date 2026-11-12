"""
Camera watchdog: notices when the camera picture dies while it is on air.

  * black   — the frame is (almost) black for a while: lens cap, camera off, dead battery;
  * frozen  — the frame does not change at all: a live camera always has some sensor noise,
              so identical frames mean the capture card or the camera hung;
  * lost    — OBS cannot give us a frame from the source at all.
"""

from __future__ import annotations

import numpy as np

BLACK_LEVEL = 10        # mean brightness (0..255) below this counts as black
BLACK_SECONDS = 2.0
FROZEN_DIFF = 0.08      # mean abs difference between frames below this counts as "same frame"
FROZEN_SECONDS = 3.0
LOST_SECONDS = 2.5

MESSAGES = {
    "black": "Камера показывает чёрный кадр",
    "frozen": "Картинка с камеры зависла",
    "lost": "Нет сигнала с камеры",
}


class CameraWatchdog:
    def __init__(self):
        self.reset()

    def reset(self):
        self._prev: np.ndarray | None = None
        self._black_since: float | None = None
        self._still_since: float | None = None
        self._fail_since: float | None = None
        self.fault: str | None = None

    def frame(self, img: np.ndarray, now: float) -> str | None:
        """Feed every grabbed frame. Returns the current fault ('black'/'frozen') or None."""
        self._fail_since = None
        small = img[::4, ::4].astype(np.int16)

        if small.mean() < BLACK_LEVEL:
            self._black_since = now if self._black_since is None else self._black_since
        else:
            self._black_since = None

        if self._prev is not None and self._prev.shape == small.shape \
                and np.abs(small - self._prev).mean() < FROZEN_DIFF:
            self._still_since = now if self._still_since is None else self._still_since
        else:
            self._still_since = None
        self._prev = small

        if self._black_since is not None and now - self._black_since >= BLACK_SECONDS:
            self.fault = "black"
        elif self._still_since is not None and now - self._still_since >= FROZEN_SECONDS:
            self.fault = "frozen"
        else:
            self.fault = None
        return self.fault

    def failure(self, now: float) -> str | None:
        """Feed every failed grab."""
        self._fail_since = now if self._fail_since is None else self._fail_since
        if now - self._fail_since >= LOST_SECONDS:
            self.fault = "lost"
        return self.fault
