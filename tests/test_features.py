"""Duo framing, auto-cut, re-identification by clothes, camera watchdog, name plate page."""

import time

import numpy as np

from speakertrack.appearance import AppearanceMemory, signature, similarity
from speakertrack.engine import Engine
from speakertrack.framing import Framer
from speakertrack.lower_third import write_page
from speakertrack.settings import Settings
from speakertrack.watchdog import CameraWatchdog
from tests.test_core import make_person


def paint(img, p, color):
    """Fill a person's box with a shirt color on a 640x360 image."""
    h, w = img.shape[:2]
    x0, y0, x1, y1 = p.box
    img[int(y0 * h):int(y1 * h), int(x0 * w):int(x1 * w)] = color


def test_group_frame_holds_both_people():
    fr = Framer(Settings(), 16 / 9, 16 / 9)
    a, b = make_person(cx=0.35, tid=1), make_person(cx=0.62, tid=2)
    f = fr.frame_for_group([a, b])
    x0, y0, x1, y1 = fr.rect(f)
    assert x0 < a.box[0] and x1 > b.box[2], "both people inside"
    assert f.h > fr.frame_for(a).h, "wider than a single shot"


def test_scene_modes_migrate_old_settings():
    assert Settings(scene="A").modes() == {"A": "track"}
    assert Settings(scene="A", active_scenes=["B", "C"]).modes() == {"B": "track", "C": "track"}
    assert Settings(scene_modes={"B": "duo"}).modes() == {"B": "duo"}


def test_signature_tells_shirts_apart():
    img = np.full((360, 640, 3), 40, np.uint8)
    red, blue, red2 = make_person(cx=0.2, tid=1), make_person(cx=0.5, tid=2), make_person(cx=0.8, tid=3)
    paint(img, red, (200, 30, 30))
    paint(img, blue, (30, 60, 200))
    paint(img, red2, (190, 40, 35))
    sr, sb, sr2 = (signature(img, p) for p in (red, blue, red2))
    assert similarity(sr, sr2) > 0.9
    assert similarity(sr, sb) < 0.3
    mem = AppearanceMemory()
    mem.reset(sr)
    mem.update(sb)          # someone else: must not drift
    assert mem.score(sr) > 0.95


def test_reid_returns_to_speaker():
    s = Settings()
    e = Engine(s)
    e.framer = Framer(s, 16 / 9, 16 / 9)
    img = np.full((360, 640, 3), 40, np.uint8)
    speaker, other = make_person(cx=0.3, tid=1), make_person(cx=0.7, tid=2)
    paint(img, speaker, (200, 30, 30))
    paint(img, other, (30, 60, 200))
    sigs = {1: signature(img, speaker), 2: signature(img, other)}
    e.memory.reset(sigs[1])

    # speaker gone: we fall back to the other person
    e.last_sigs = {2: sigs[2]}
    assert e._pick_target([other], time.time() + 100).tid == 2
    # speaker is back with a new tracker id: we switch back to them
    speaker.tid = 5
    e.last_sigs = {2: sigs[2], 5: sigs[1]}
    assert e._pick_target([other, speaker], time.time() + 100).tid == 5


def test_auto_cut_goes_wide_when_walking_and_back_when_still():
    s = Settings(auto_cut=True, cut_min_hold=0.0)
    e = Engine(s)
    t = 0.0
    shot = None
    for i in range(30):           # walking: 0.15 frame widths per second
        t += 0.1
        shot, _ = e._cut_shot(make_person(cx=0.3 + 0.015 * i), t)
    assert e.cut == "wide" and shot == "knee"
    for _ in range(60):           # standing still
        t += 0.1
        shot, _ = e._cut_shot(make_person(cx=0.75), t)
    assert e.cut == "close" and shot is None


def test_watchdog_black_and_frozen():
    wd = CameraWatchdog()
    black = np.zeros((360, 640, 3), np.uint8)
    assert wd.frame(black, 0.0) is None
    assert wd.frame(black, 2.5) == "black"

    wd.reset()
    rng = np.random.default_rng(0)
    for i in range(10):           # live camera: noise every frame
        assert wd.frame(rng.integers(30, 200, (360, 640, 3), dtype=np.uint8), i * 0.5) is None
    still = rng.integers(30, 200, (360, 640, 3), dtype=np.uint8)
    wd.frame(still, 10.0)
    assert wd.frame(still.copy(), 10.5) is None   # same frame, not long enough yet
    assert wd.frame(still.copy(), 14.0) == "frozen"
    assert wd.frame(rng.integers(30, 200, (360, 640, 3), dtype=np.uint8), 14.0) is None


def test_lower_third_page(tmp_path, monkeypatch):
    monkeypatch.setenv("APPDATA", str(tmp_path))
    from speakertrack.settings import Settings
    s = Settings(lt_name="Иван «Ваня» Петров", lt_title="Пастор", lt_duration=5, lt_color="#c6f36b",
                 lt_plate="light", lt_anim="slide", lt_anim_speed=1.5)
    page = write_page(s)
    html = page.read_text(encoding="utf-8")
    assert '"name": "Иван «Ваня» Петров"' in html and '"duration": 5' in html
    assert '"accent": "#c6f36b"' in html and '"plate": "light"' in html and '"anim": "slide"' in html
    assert '"loop": false' in html
    assert (page.parent / "fonts" / "Unbounded-Black.ttf").exists()
    preview = write_page(s, preview=True)
    assert preview.name == "lower_third_preview.html" and '"loop": true' in preview.read_text(encoding="utf-8")
