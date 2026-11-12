"""User settings, stored as JSON in %APPDATA%\\VKadre\\settings.json."""

from __future__ import annotations

import json
import logging
import os
import sys
from dataclasses import asdict, dataclass, field, fields
from pathlib import Path

from speakertrack import __app_id__

log = logging.getLogger(__name__)


def app_data_dir() -> Path:
    base = os.environ.get("APPDATA") or str(Path.home() / ".config")
    d = Path(base) / __app_id__
    d.mkdir(parents=True, exist_ok=True)
    return d


def resource_dir() -> Path:
    """Folder with bundled resources (models/), both in dev and in the PyInstaller build."""
    if getattr(sys, "frozen", False):
        return Path(getattr(sys, "_MEIPASS", Path(sys.executable).parent))
    return Path(__file__).resolve().parent.parent


@dataclass
class Settings:
    # OBS connection
    obs_host: str = "localhost"
    obs_port: int = 4455
    obs_password: str = ""
    auto_connect: bool = True

    # What to control
    scene: str = ""                 # legacy: main scene (used when scene_modes is empty)
    source: str = ""                # camera source (the same source can sit in several scenes)
    active_scenes: list = field(default_factory=list)   # legacy, migrated into scene_modes
    # What to do in each OBS scene: track | duo | wide. Scenes not listed are not touched.
    # Строчка switches OBS scenes by item type, so this is how the two apps work together.
    scene_modes: dict = field(default_factory=dict)

    # Detection
    model: str = "yolo11n-pose.onnx"
    use_gpu: bool = True
    confidence: float = 0.4
    kp_conf: float = 0.5
    detect_fps: int = 10
    render_fps: int = 30

    # Shot
    zoom_mode: str = "auto"         # auto | fixed
    shot: str = "waist"             # chest | waist | hips | knee
    fixed_zoom: float = 2.0
    min_zoom: float = 1.3
    max_zoom: float = 3.0
    wide_zoom: float = 1.0
    headroom: float = 0.08
    torso_smoothing: float = 0.8

    # Motion
    deadzone_x: float = 0.12
    deadzone_y: float = 0.15
    deadzone_zoom: float = 0.2
    smooth_time: float = 0.7
    zoom_smooth_time: float = 2.0
    max_speed: float = 0.35

    # Behaviour
    lost_timeout: float = 4.0
    wide_on_lost: bool = True
    zone: list = field(default_factory=lambda: [0.0, 0.0, 1.0, 1.0])
    restore_on_exit: bool = True

    # Auto-cut: two shots from one camera
    auto_cut: bool = False
    cut_wide_shot: str = "knee"     # hips | knee | full
    cut_min_hold: float = 6.0       # seconds a shot stays before the next cut
    cut_move_speed: float = 0.06    # speaker speed (frame widths / s) that counts as walking

    # Remember the speaker by clothing color
    reid: bool = True
    reid_threshold: float = 0.72

    # Camera watchdog
    watchdog: bool = True
    watchdog_sound: bool = True
    watchdog_switch: bool = False
    backup_scene: str = ""

    # Name plate (lower third)
    lt_name: str = ""
    lt_title: str = ""
    lt_auto: bool = True
    lt_duration: float = 7.0
    lt_delay: float = 4.0           # after the speaker is found in a newly active scene
    lt_color: str = "#ff9a8b"       # accent: spine, dash, corner
    lt_plate: str = "dark"          # plate background: dark | light | color
    lt_anim: str = "wipe"           # wipe | slide | rise | fade
    lt_anim_speed: float = 1.0      # 0.5 = twice slower, 2 = twice faster

    def modes(self) -> dict:
        """Scene -> mode, with old settings migrated."""
        if self.scene_modes:
            return dict(self.scene_modes)
        if self.active_scenes:
            return {sc: "track" for sc in self.active_scenes}
        return {self.scene: "track"} if self.scene else {}

    # -- persistence -------------------------------------------------------------

    @classmethod
    def path(cls) -> Path:
        return app_data_dir() / "settings.json"

    @classmethod
    def load(cls) -> "Settings":
        s = cls()
        p = cls.path()
        if p.exists():
            try:
                data = json.loads(p.read_text(encoding="utf-8"))
                known = {f.name for f in fields(cls)}
                for k, v in data.items():
                    if k in known:
                        setattr(s, k, v)
            except Exception as e:  # corrupted file: keep defaults
                log.warning("Не удалось прочитать настройки: %s", e)
        return s

    def save(self):
        p = self.path()
        tmp = p.with_suffix(".tmp")
        tmp.write_text(json.dumps(asdict(self), ensure_ascii=False, indent=2), encoding="utf-8")
        tmp.replace(p)

    def model_path(self) -> Path:
        user = app_data_dir() / "models" / self.model
        if user.exists():
            return user
        return resource_dir() / "models" / self.model
