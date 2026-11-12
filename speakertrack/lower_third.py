"""
Name plate (lower third) in OBS.

The page lives in %APPDATA%\\VKadre\\overlay\\lower_third.html (copied from the bundle together
with the fonts). OBS shows it through a Browser Source named «В кадре · Плашка» that we add to
each scene where we track the speaker. To show the plate we write the name into the page, turn
the source on (the page reloads and plays in), and turn it off after it has played out.
"""

from __future__ import annotations

import json
import logging
import re
import shutil
import threading
import time
from pathlib import Path

from speakertrack.obs_client import ObsConnection
from speakertrack.settings import Settings, app_data_dir, resource_dir

log = logging.getLogger(__name__)

INPUT_NAME = "В кадре · Плашка"
FONTS = ("Unbounded-Black.ttf", "Manrope-Bold.ttf", "Manrope-ExtraBold.ttf")
OUT_SECONDS = 2.0   # play-out animation + a margin


def overlay_dir() -> Path:
    d = app_data_dir() / "overlay"
    (d / "fonts").mkdir(parents=True, exist_ok=True)
    return d


def anim_seconds(s: Settings) -> float:
    """Whole on-screen time of one play: in + hold + out, scaled by the animation speed."""
    return s.lt_duration + (1.2 + OUT_SECONDS) / max(s.lt_anim_speed, 0.25)


def write_page(s: Settings, preview: bool = False) -> Path:
    """Copy the template and fonts next to each other and bake the text and look into the page.

    preview=True writes a separate looping page with a backdrop, for opening in a browser.
    """
    src = resource_dir() / "speakertrack" / "overlay" / "lower_third.html"
    dst_dir = overlay_dir()
    for f in FONTS:
        target = dst_dir / "fonts" / f
        if not target.exists():
            shutil.copy(resource_dir() / "speakertrack" / "ui" / "fonts" / f, target)
    name = s.lt_name.strip() or ("Имя Фамилия" if preview else "")
    title = s.lt_title.strip() if s.lt_name.strip() or not preview else "Подпись"
    data = json.dumps({"name": name, "title": title, "duration": s.lt_duration, "accent": s.lt_color,
                       "plate": s.lt_plate, "anim": s.lt_anim, "speed": s.lt_anim_speed, "loop": preview},
                      ensure_ascii=False)
    html = src.read_text(encoding="utf-8")
    html = re.sub(r"/\*DATA\*/.*?/\*END\*/", lambda _: f"/*DATA*/var DATA = {data};/*END*/", html,
                  flags=re.S)
    page = dst_dir / ("lower_third_preview.html" if preview else "lower_third.html")
    page.write_text(html, encoding="utf-8")
    return page


class LowerThird:
    """Shows the plate in OBS from a background thread, so the UI never waits for OBS."""

    def __init__(self, settings: Settings):
        self.s = settings
        self._busy = threading.Lock()

    def show(self, scene: str | None, on_error=None) -> bool:
        """Show the plate in `scene` (None = the scene that is on air now)."""
        if not self.s.lt_name.strip():
            return False
        if not self._busy.acquire(blocking=False):
            return False  # already on screen
        threading.Thread(target=self._run, args=(scene, on_error), daemon=True, name="lower-third").start()
        return True

    def _run(self, scene: str, on_error):
        conn = None
        try:
            page = write_page(self.s)
            conn = ObsConnection(self.s.obs_host, self.s.obs_port, self.s.obs_password)
            scene = scene or conn.program_scene()
            item_id = self._ensure(conn, scene, page)
            conn.refresh_browser(INPUT_NAME)
            time.sleep(0.4)
            conn.set_item_enabled(scene, item_id, True)
            time.sleep(anim_seconds(self.s))
            conn.set_item_enabled(scene, item_id, False)
        except Exception as e:
            log.warning("Плашка: %s", e)
            if on_error:
                on_error(f"Не удалось показать плашку: {e}")
        finally:
            if conn:
                conn.close()
            self._busy.release()

    def _ensure(self, conn: ObsConnection, scene: str, page: Path) -> int:
        """Browser source exists, points at our page and sits on top of `scene`."""
        settings = {"is_local_file": True, "local_file": str(page), "width": 1920, "height": 1080,
                    "shutdown": True, "restart_when_active": True, "reroute_audio": False, "css": ""}
        if INPUT_NAME in conn.input_names():
            conn.set_input_settings(INPUT_NAME, settings)
            if INPUT_NAME not in conn.scene_sources(scene):
                item_id = conn.create_scene_item(scene, INPUT_NAME, enabled=False)
            else:
                item_id = conn.item_id(scene, INPUT_NAME)
        else:
            item_id = conn.create_input(scene, INPUT_NAME, "browser_source", settings, enabled=False)
        canvas = conn.canvas_size()
        conn.set_item_transform(scene, item_id, {
            "positionX": 0.0, "positionY": 0.0, "alignment": 5, "rotation": 0.0,
            "boundsType": "OBS_BOUNDS_STRETCH", "boundsAlignment": 0,
            "boundsWidth": float(canvas[0]), "boundsHeight": float(canvas[1]),
        })
        conn.move_to_top(scene, item_id)
        return item_id
