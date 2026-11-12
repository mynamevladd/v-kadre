import numpy as np
import pytest

from speakertrack.detector import decode, letterbox, nms
from speakertrack.framing import Framer, Person
from speakertrack.settings import Settings
from speakertrack.tracker import SimpleTracker


def make_person(cx=0.65, hips_conf=1.0, box_top=0.27, tid=1):
    kxy = np.zeros((17, 2))
    kc = np.zeros(17)
    for i in range(5):
        kxy[i] = (cx, 0.30)
        kc[i] = 0.9
    kxy[5], kxy[6] = (cx - 0.03, 0.36), (cx + 0.03, 0.36)
    kc[5] = kc[6] = 0.9
    kxy[11], kxy[12] = (cx - 0.02, 0.52), (cx + 0.02, 0.52)
    kc[11] = kc[12] = hips_conf
    kxy[9], kc[9] = (cx + 0.05, 0.20), 0.9   # raised wrist, must be ignored
    return Person(box=(cx - 0.06, box_top, cx + 0.06, 0.78), kxy=kxy, kconf=kc, tid=tid)


@pytest.fixture
def framer():
    return Framer(Settings(), src_aspect=16 / 9, out_aspect=16 / 9)


def test_raised_hand_does_not_change_frame(framer):
    a = framer.frame_for(make_person())
    framer.reset_target()
    b = framer.frame_for(make_person(box_top=0.15))
    assert (a.cx, a.cy, a.h) == pytest.approx((b.cx, b.cy, b.h))


def test_hidden_hips_close_to_full_pose(framer):
    a = framer.frame_for(make_person())
    framer.reset_target()
    b = framer.frame_for(make_person(hips_conf=0.0))
    assert b.h == pytest.approx(a.h, rel=0.05)
    assert b.cy == pytest.approx(a.cy, abs=0.02)


def test_head_is_inside_frame_with_headroom(framer):
    f = framer.clamp_frame(framer.frame_for(make_person()))
    top = f.cy - f.h / 2
    assert top < 0.30 - 0.04       # above top of head
    assert f.cy + f.h / 2 > 0.36   # shoulders inside


def test_zoom_respects_limits():
    s = Settings(max_zoom=2.0)
    fr = Framer(s, 16 / 9, 16 / 9)
    f = fr.frame_for(make_person())
    assert 1 / f.h <= 2.0 + 1e-9


def test_frame_clamped_inside_source(framer):
    f = framer.clamp_frame(framer.frame_for(make_person(cx=0.99)))
    x0, y0, x1, y1 = framer.rect(f)
    assert x0 >= -1e-9 and x1 <= 1 + 1e-9 and y0 >= -1e-9 and y1 <= 1 + 1e-9


def test_dead_zone(framer):
    framer.force_goal(framer.wide_frame().__class__(0.5, 0.4, 0.33))
    framer.set_goal(framer.goal.__class__(0.52, 0.41, 0.34))
    assert framer.goal.cx == 0.5
    framer.set_goal(framer.goal.__class__(0.7, 0.41, 0.34))
    assert framer.goal.cx == 0.7


def test_smoothing_converges(framer):
    framer.set_goal(framer.frame_for(make_person()), snap=False)
    framer.force_goal(framer.clamp_frame(framer.frame_for(make_person())))
    for _ in range(300):
        cur = framer.step(1 / 30)
    assert cur.cx == pytest.approx(framer.goal.cx, abs=1e-3)


def test_tracker_keeps_ids():
    tr = SimpleTracker()
    a = [Person(box=(0.1, 0.2, 0.2, 0.8)), Person(box=(0.6, 0.2, 0.7, 0.8))]
    tr.update(a)
    ids = (a[0].tid, a[1].tid)
    b = [Person(box=(0.62, 0.2, 0.72, 0.8)), Person(box=(0.11, 0.2, 0.21, 0.8))]  # order swapped, moved
    tr.update(b)
    assert (b[1].tid, b[0].tid) == ids


def test_tracker_remembers_briefly_lost():
    tr = SimpleTracker(max_missed=5)
    a = [Person(box=(0.4, 0.2, 0.5, 0.8))]
    tr.update(a)
    for _ in range(3):
        tr.update([])
    b = [Person(box=(0.41, 0.2, 0.51, 0.8))]
    tr.update(b)
    assert b[0].tid == a[0].tid


def test_nms_and_decode_roundtrip():
    size = 640
    img = np.zeros((540, 960, 3), dtype=np.uint8)
    _, scale, px, py = letterbox(img, size)
    n = 10
    out = np.zeros((1, 56, n), dtype=np.float32)
    # one person at image center, 2 duplicates + low-score noise
    for j in range(3):
        out[0, 0:4, j] = [320 + j, 320, 60, 200]
        out[0, 4, j] = 0.9 - j * 0.1
        out[0, 5::3, j] = 320
        out[0, 6::3, j] = 300
        out[0, 7::3, j] = 0.95
    out[0, 4, 3:] = 0.05
    people = decode(out, 960, 540, scale, px, py, conf_thr=0.4)
    assert len(people) == 1
    cx, cy = people[0].center
    assert cx == pytest.approx(0.5, abs=0.01)
    assert people[0].kconf[0] == pytest.approx(0.95)


def test_nms_basic():
    boxes = np.array([[0, 0, 10, 10], [1, 1, 11, 11], [50, 50, 60, 60]], dtype=float)
    keep = nms(boxes, np.array([0.9, 0.8, 0.7]), 0.5)
    assert keep == [0, 2]
