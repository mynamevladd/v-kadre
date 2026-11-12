"""Engine end-to-end with a fake OBS and a fake detector (no OBS / GPU needed)."""

import threading
import time

import numpy as np
import pytest
from PySide6.QtCore import QCoreApplication

from speakertrack import engine as engine_mod
from speakertrack.engine import Engine
from speakertrack.settings import Settings
from tests.test_core import make_person


class FakeObs:
    def __init__(self):
        self.program = "Хвала"
        self.crops = []
        self.restored = None
        self.lock = threading.Lock()
        self.original = {"sourceWidth": 3840, "sourceHeight": 2160, "positionX": 10.0, "cropLeft": 0}

    # ObsConnection API used by the engine
    def item_id(self, scene, source):
        return 7

    def canvas_size(self):
        return 1920, 1080

    def item_transform(self, scene, item_id):
        return dict(self.original)

    def program_scene(self):
        return self.program

    def preview_scene(self):
        return None

    def set_crop(self, scene, item_id, canvas, l, t, r, b):
        with self.lock:
            self.crops.append((l, t, r, b))

    def set_item_transform(self, scene, item_id, tr):
        self.restored = tr

    def screenshot(self, source, w, h):
        return np.zeros((h, w, 3), np.uint8)

    def close(self):
        pass


class FakeDetector:
    device_name = "fake"
    size = 640

    def detect(self, img, conf):
        return [make_person(cx=0.65)]


@pytest.fixture(scope="module")
def qapp():
    return QCoreApplication.instance() or QCoreApplication([])


def wait(cond, timeout=5.0):
    end = time.time() + timeout
    while time.time() < end:
        QCoreApplication.processEvents()   # deliver queued signals from worker threads
        if cond():
            return True
        time.sleep(0.05)
    return False


def test_engine_tracks_only_in_active_scene(qapp, monkeypatch):
    fake = FakeObs()
    s = Settings(scene="Проповедь", source="Камера", active_scenes=["Проповедь"], detect_fps=20)
    e = Engine(s)
    monkeypatch.setattr(e, "_connect", lambda: fake)
    monkeypatch.setattr(e, "_get_detector", lambda: FakeDetector())
    errors = []
    e.error.connect(errors.append)

    e.start()
    time.sleep(1.0)
    assert not fake.crops, "must not touch OBS while the scene is not on air"

    fake.program = "Проповедь"
    assert wait(lambda: len(fake.crops) > 5), errors
    l, t, r, b = fake.crops[-1]
    # Speaker at x=0.65 of a 3840 px source -> crop center near 2496 px
    center = (l + (3840 - r)) / 2
    assert abs(center - 0.65 * 3840) < 200
    assert e.locked_id is not None

    fake.program = "Хвала"
    time.sleep(1.0)
    n = len(fake.crops)
    time.sleep(0.5)
    assert len(fake.crops) == n, "must stop sending when the scene leaves air"

    e.stop()
    assert wait(lambda: not e.running)
    assert fake.restored is not None and fake.restored.get("positionX") == 10.0
    assert not errors


def test_engine_reports_missing_source(qapp, monkeypatch):
    fake = FakeObs()
    fake.item_id = lambda scene, source: (_ for _ in ()).throw(Exception("not found"))
    s = Settings(scene="Проповедь", source="Нет такой")
    e = Engine(s)
    monkeypatch.setattr(e, "_connect", lambda: fake)
    errors = []
    e.error.connect(errors.append)
    e.start()
    assert wait(lambda: not e.running)
    assert errors and "Нет такой" in errors[0]
