"""Thin wrapper over obs-websocket v5 (obsws-python)."""

from __future__ import annotations

import base64
import logging
import threading

import numpy as np
import obsws_python as obs
from PySide6.QtGui import QImage

log = logging.getLogger(__name__)


def _attr(resp, *names, default=None):
    for n in names:
        v = getattr(resp, n, None)
        if v is not None:
            return v
    return default


class ObsConnection:
    """One websocket connection. Requests are serialized with a lock, so it is safe across threads,
    but each worker thread should get its own connection to avoid blocking each other."""

    def __init__(self, host: str, port: int, password: str, timeout: float = 5):
        self._cl = obs.ReqClient(host=host, port=port, password=password or "", timeout=timeout)
        self._lock = threading.Lock()

    def close(self):
        try:
            self._cl.disconnect()
        except Exception:
            pass

    def _call(self, name, *args):
        with self._lock:
            return getattr(self._cl, name)(*args)

    # -- info ----------------------------------------------------------------------

    def version(self) -> str:
        r = self._call("get_version")
        return f"OBS {_attr(r, 'obs_version', default='?')}"

    def scenes(self) -> list[str]:
        r = self._call("get_scene_list")
        items = _attr(r, "scenes", default=[])
        # OBS returns scenes bottom-to-top; show them in the order the user sees.
        return [s["sceneName"] for s in reversed(items)]

    def scene_sources(self, scene: str) -> list[str]:
        r = self._call("get_scene_item_list", scene)
        return [i["sourceName"] for i in _attr(r, "scene_items", default=[])]

    def canvas_size(self) -> tuple[int, int]:
        r = self._call("get_video_settings")
        return int(r.base_width), int(r.base_height)

    def program_scene(self) -> str | None:
        r = self._call("get_current_program_scene")
        return _attr(r, "current_program_scene_name", "scene_name")

    def preview_scene(self) -> str | None:
        """Preview scene in Studio Mode, None otherwise."""
        try:
            r = self._call("get_current_preview_scene")
            return _attr(r, "current_preview_scene_name", "scene_name")
        except Exception:
            return None

    # -- scene item ------------------------------------------------------------------

    def item_id(self, scene: str, source: str) -> int:
        return int(self._call("get_scene_item_id", scene, source).scene_item_id)

    def item_transform(self, scene: str, item_id: int) -> dict:
        return dict(self._call("get_scene_item_transform", scene, item_id).scene_item_transform)

    def set_item_transform(self, scene: str, item_id: int, transform: dict):
        self._call("set_scene_item_transform", scene, item_id, transform)

    def set_crop(self, scene: str, item_id: int, canvas: tuple[int, int],
                 left: int, top: int, right: int, bottom: int):
        cw, ch = canvas
        self.set_item_transform(scene, item_id, {
            "positionX": 0.0,
            "positionY": 0.0,
            "alignment": 5,  # top-left
            "rotation": 0.0,
            "boundsType": "OBS_BOUNDS_SCALE_INNER",
            "boundsAlignment": 0,
            "boundsWidth": float(cw),
            "boundsHeight": float(ch),
            "cropLeft": max(0, int(left)),
            "cropTop": max(0, int(top)),
            "cropRight": max(0, int(right)),
            "cropBottom": max(0, int(bottom)),
        })

    def set_item_enabled(self, scene: str, item_id: int, enabled: bool):
        self._call("set_scene_item_enabled", scene, item_id, enabled)

    def move_to_top(self, scene: str, item_id: int):
        r = self._call("get_scene_item_list", scene)
        top = max((i.get("sceneItemIndex", 0) for i in _attr(r, "scene_items", default=[])), default=0)
        self._call("set_scene_item_index", scene, item_id, top)

    # -- scenes / inputs ---------------------------------------------------------------

    def set_program_scene(self, scene: str):
        self._call("set_current_program_scene", scene)

    def input_names(self) -> list[str]:
        r = self._call("get_input_list", None)
        return [i["inputName"] for i in _attr(r, "inputs", default=[])]

    def create_input(self, scene: str, name: str, kind: str, settings: dict, enabled: bool) -> int:
        r = self._call("create_input", scene, name, kind, settings, enabled)
        return int(_attr(r, "scene_item_id"))

    def create_scene_item(self, scene: str, source: str, enabled: bool = True) -> int:
        r = self._call("create_scene_item", scene, source, enabled)
        return int(_attr(r, "scene_item_id"))

    def set_input_settings(self, name: str, settings: dict):
        self._call("set_input_settings", name, settings, True)

    def refresh_browser(self, name: str):
        try:
            self._call("press_input_properties_button", name, "refreshnocache")
        except Exception as e:  # older OBS: the page reloads on show anyway
            log.debug("refreshnocache: %s", e)

    # -- frames ----------------------------------------------------------------------

    def screenshot(self, source: str, width: int, height: int, quality: int = 80) -> np.ndarray | None:
        """RGB uint8 array (h, w, 3), already scaled by OBS to the requested size."""
        r = self._call("get_source_screenshot", source, "jpg", width, height, quality)
        data = r.image_data
        b64 = data.split(",", 1)[1] if "," in data else data
        return decode_jpeg(base64.b64decode(b64))


def decode_jpeg(data: bytes) -> np.ndarray | None:
    img = QImage.fromData(data, "JPG")
    if img.isNull():
        return None
    img = img.convertToFormat(QImage.Format_RGB888)
    w, h, bpl = img.width(), img.height(), img.bytesPerLine()
    arr = np.frombuffer(img.constBits(), dtype=np.uint8, count=bpl * h).reshape(h, bpl)
    return arr[:, : w * 3].reshape(h, w, 3).copy()


RESTORE_KEYS = ("positionX", "positionY", "alignment", "rotation", "boundsType", "boundsAlignment",
                "boundsWidth", "boundsHeight", "cropLeft", "cropTop", "cropRight", "cropBottom",
                "scaleX", "scaleY")
