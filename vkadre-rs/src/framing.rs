//! Pure framing math: where the virtual camera should look, given a detected person.
//!
//! All coordinates are normalized to the camera source: x, y in 0..1.
//! A crop window ([`Frame`]) is described by its center (cx, cy) and its height fraction h
//! (h = 1 / zoom). Its width follows from the output and source aspect ratios.

use crate::settings::Settings;

// COCO-17 keypoint indices
pub const HEAD_KP: [usize; 5] = [0, 1, 2, 3, 4]; // nose, eyes, ears
pub const SHOULDER_KP: [usize; 2] = [5, 6];
pub const HIP_KP: [usize; 2] = [11, 12];

/// Where the bottom edge of the shot cuts the body, in torso lengths below the shoulder line.
pub fn shot_depth(shot: &str) -> f64 {
    match shot {
        "chest" => 0.6, // по грудь
        "waist" => 1.0, // по пояс
        "hips" => 1.4,  // чуть ниже пояса
        "knee" => 2.1,  // по колено
        "full" => 3.4,  // в полный рост
        _ => 1.0,
    }
}

/// top of head is ~0.25 torso lengths above the nose
const HEAD_TOP_ABOVE_NOSE: f64 = 0.25;
/// nose-to-shoulder-line distance in torso lengths
const NOSE_TO_SHOULDER: f64 = 0.4;

pub fn clamp(v: f64, lo: f64, hi: f64) -> f64 {
    lo.max(hi.min(v))
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    pub cx: f64,
    pub cy: f64,
    pub h: f64,
}

impl Default for Frame {
    fn default() -> Self {
        Frame { cx: 0.5, cy: 0.5, h: 1.0 }
    }
}

impl Frame {
    pub fn new(cx: f64, cy: f64, h: f64) -> Self {
        Frame { cx, cy, h }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Keypoints {
    pub xy: [[f64; 2]; 17],
    pub conf: [f64; 17],
}

/// One detected person. `bbox` = (x0, y0, x1, y1), normalized.
#[derive(Debug, Clone, PartialEq)]
pub struct Person {
    pub bbox: [f64; 4],
    pub score: f64,
    pub kp: Option<Keypoints>,
    pub tid: i64,
}

impl Person {
    pub fn new(bbox: [f64; 4]) -> Self {
        Person { bbox, score: 1.0, kp: None, tid: -1 }
    }

    pub fn center(&self) -> (f64, f64) {
        let [x0, y0, x1, y1] = self.bbox;
        ((x0 + x1) / 2.0, (y0 + y1) / 2.0)
    }

    pub fn area(&self) -> f64 {
        let [x0, y0, x1, y1] = self.bbox;
        (x1 - x0).max(0.0) * (y1 - y0).max(0.0)
    }

    pub fn contains(&self, x: f64, y: f64) -> bool {
        let [x0, y0, x1, y1] = self.bbox;
        x0 <= x && x <= x1 && y0 <= y && y <= y1
    }

    fn mean(&self, idx: &[usize], thr: f64) -> Option<(f64, f64)> {
        let kp = self.kp.as_ref()?;
        let pts: Vec<[f64; 2]> = idx.iter().filter(|&&i| kp.conf[i] >= thr).map(|&i| kp.xy[i]).collect();
        if pts.is_empty() {
            return None;
        }
        let n = pts.len() as f64;
        Some((pts.iter().map(|p| p[0]).sum::<f64>() / n, pts.iter().map(|p| p[1]).sum::<f64>() / n))
    }

    /// (cx, head_y, shoulder_y, torso) built from head and torso keypoints, or None.
    pub fn pose(&self, thr: f64) -> Option<(f64, f64, f64, f64)> {
        let head = self.mean(&HEAD_KP, thr);
        let sh = self.mean(&SHOULDER_KP, thr);
        let hip = self.mean(&HIP_KP, thr);

        let mut torso = None;
        if let (Some(s), Some(h)) = (sh, hip) {
            if h.1 - s.1 > 0.01 {
                torso = Some(h.1 - s.1);
            }
        }
        if torso.is_none() {
            if let (Some(hd), Some(s)) = (head, sh) {
                if s.1 - hd.1 > 0.005 {
                    torso = Some((s.1 - hd.1) / NOSE_TO_SHOULDER);
                }
            }
        }
        let t = torso?;
        let head = head.unwrap_or_else(|| {
            let s = sh.expect("shoulders exist when torso is known");
            (s.0, s.1 - NOSE_TO_SHOULDER * t)
        });
        let sh = sh.unwrap_or((head.0, head.1 + NOSE_TO_SHOULDER * t));
        Some(((head.0 + sh.0) / 2.0, head.1, sh.1, t))
    }
}

/// Turns a target person into a crop window, with dead zones and smoothing.
#[derive(Debug, Clone)]
pub struct Framer {
    pub src_aspect: f64,
    pub out_aspect: f64,
    pub goal: Frame,
    pub current: Frame,
    pub torso: Option<f64>,
}

impl Framer {
    pub fn new(src_aspect: f64, out_aspect: f64) -> Self {
        Framer { src_aspect, out_aspect, goal: Frame::default(), current: Frame::default(), torso: None }
    }

    // -- geometry ---------------------------------------------------------------

    pub fn crop_width(&self, h: f64) -> f64 {
        h * self.out_aspect / self.src_aspect
    }

    pub fn clamp_frame(&self, mut f: Frame) -> Frame {
        let mut w = self.crop_width(f.h);
        if w > 1.0 {
            // wider than the source: shrink to fit
            f.h /= w;
            w = 1.0;
        }
        f.h = f.h.min(1.0);
        f.cx = clamp(f.cx, w / 2.0, 1.0 - w / 2.0);
        f.cy = clamp(f.cy, f.h / 2.0, 1.0 - f.h / 2.0);
        f
    }

    pub fn rect(&self, f: &Frame) -> [f64; 4] {
        let w = self.crop_width(f.h);
        [f.cx - w / 2.0, f.cy - f.h / 2.0, f.cx + w / 2.0, f.cy + f.h / 2.0]
    }

    pub fn zoom_to_h(&self, s: &Settings, zoom: f64) -> f64 {
        1.0 / clamp(zoom, s.min_zoom, s.max_zoom)
    }

    // -- target -> desired frame --------------------------------------------------

    pub fn reset_target(&mut self) {
        self.torso = None;
    }

    /// Frame for one person. `shot` overrides the configured shot (auto-cut uses it);
    /// smooth=false leaves the torso filter alone (used for the second person in a duo).
    pub fn frame_for(&mut self, s: &Settings, p: &Person, shot: Option<&str>, smooth: bool) -> Frame {
        let Some((cx, head_y, shoulder_y, torso)) = p.pose(s.kp_conf) else {
            // No usable skeleton: follow the box top, keep current zoom.
            let h = self.goal.h;
            let [x0, y0, x1, _] = p.bbox;
            let top = y0 - s.headroom * h;
            return Frame::new((x0 + x1) / 2.0, top + h / 2.0, h);
        };
        let t = if smooth {
            let a = s.torso_smoothing;
            let v = match self.torso {
                None => torso,
                Some(old) => old * a + torso * (1.0 - a),
            };
            self.torso = Some(v);
            v
        } else {
            torso
        };

        let head_top = head_y - HEAD_TOP_ABOVE_NOSE * t;
        let h = if s.zoom_mode == "auto" || shot.is_some() {
            let cut = shoulder_y + shot_depth(shot.unwrap_or(&s.shot)) * t;
            let h = (cut - head_top) / (1.0 - s.headroom);
            self.zoom_to_h(s, 1.0 / h.max(1e-3))
        } else {
            self.zoom_to_h(s, s.fixed_zoom)
        };
        let top = head_top - s.headroom * h;
        Frame::new(cx, top + h / 2.0, h)
    }

    pub fn wide_frame(&self, s: &Settings) -> Frame {
        Frame::new(0.5, 0.5, 1.0 / s.wide_zoom.max(1.0))
    }

    /// Wide shot of the stage zone (used by the «Общий» scene mode).
    pub fn zone_frame(&self, s: &Settings) -> Frame {
        let [x0, y0, x1, y1] = s.zone;
        if s.zone == [0.0, 0.0, 1.0, 1.0] {
            return self.wide_frame(s);
        }
        let w = (x1 - x0) * 1.06;
        let h = ((y1 - y0) * 1.06).max(w * self.src_aspect / self.out_aspect);
        self.clamp_frame(Frame::new((x0 + x1) / 2.0, (y0 + y1) / 2.0, h.min(1.0)))
    }

    /// One frame that holds everybody (interview / duo): heads with headroom on top,
    /// each person's shot cut at the bottom, side padding, then fit the output aspect.
    pub fn frame_for_group(&mut self, s: &Settings, people: &[&Person]) -> Frame {
        if people.len() == 1 {
            return self.frame_for(s, people[0], None, true);
        }
        let frames: Vec<Frame> =
            people.iter().enumerate().map(|(i, p)| self.frame_for(s, p, None, i == 0)).collect();
        let top = frames.iter().map(|f| f.cy - f.h / 2.0).fold(f64::INFINITY, f64::min);
        let bottom = frames.iter().map(|f| f.cy + f.h / 2.0).fold(f64::NEG_INFINITY, f64::max);
        let avg_w = people.iter().map(|p| p.bbox[2] - p.bbox[0]).sum::<f64>() / people.len() as f64;
        let pad = 0.6 * avg_w;
        let left = people.iter().map(|p| p.bbox[0]).fold(f64::INFINITY, f64::min) - pad;
        let right = people.iter().map(|p| p.bbox[2]).fold(f64::NEG_INFINITY, f64::max) + pad;
        let h = (bottom - top).max((right - left) * self.src_aspect / self.out_aspect);
        let h = clamp(h, 1.0 / s.max_zoom, 1.0);
        Frame::new((left + right) / 2.0, top + h / 2.0, h)
    }

    // -- goal update with dead zones ----------------------------------------------

    pub fn set_goal(&mut self, s: &Settings, f: Frame, snap: bool) {
        if snap {
            self.goal = f;
            self.current = self.clamp_frame(f);
            return;
        }
        let w = self.crop_width(self.goal.h);
        let g = &mut self.goal;
        if (f.cx - g.cx).abs() > s.deadzone_x * w {
            g.cx = f.cx;
        }
        if (f.cy - g.cy).abs() > s.deadzone_y * g.h {
            g.cy = f.cy;
        }
        if (f.h - g.h).abs() / g.h > s.deadzone_zoom {
            g.h = f.h;
        }
    }

    pub fn force_goal(&mut self, f: Frame) {
        self.goal = f;
    }

    // -- smoothing ----------------------------------------------------------------

    pub fn step(&mut self, s: &Settings, dt: f64) -> Frame {
        let a_pos = 1.0 - (-dt / s.smooth_time.max(1e-3)).exp();
        let a_zoom = 1.0 - (-dt / s.zoom_smooth_time.max(1e-3)).exp();
        let max_d = s.max_speed * dt;
        let g = self.clamp_frame(self.goal);
        let mut cur = self.current;
        cur.cx += clamp((g.cx - cur.cx) * a_pos, -max_d, max_d);
        cur.cy += clamp((g.cy - cur.cy) * a_pos, -max_d, max_d);
        cur.h += (g.h - cur.h) * a_zoom;
        self.current = self.clamp_frame(cur);
        self.current
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    pub fn make_person(cx: f64, hips_conf: f64, box_top: f64, tid: i64) -> Person {
        let mut xy = [[0.0; 2]; 17];
        let mut conf = [0.0; 17];
        for i in 0..5 {
            xy[i] = [cx, 0.30];
            conf[i] = 0.9;
        }
        xy[5] = [cx - 0.03, 0.36];
        xy[6] = [cx + 0.03, 0.36];
        conf[5] = 0.9;
        conf[6] = 0.9;
        xy[11] = [cx - 0.02, 0.52];
        xy[12] = [cx + 0.02, 0.52];
        conf[11] = hips_conf;
        conf[12] = hips_conf;
        xy[9] = [cx + 0.05, 0.20]; // raised wrist, must be ignored
        conf[9] = 0.9;
        Person { bbox: [cx - 0.06, box_top, cx + 0.06, 0.78], score: 1.0, kp: Some(Keypoints { xy, conf }), tid }
    }

    pub fn person(cx: f64) -> Person {
        make_person(cx, 1.0, 0.27, 1)
    }

    fn framer() -> Framer {
        Framer::new(16.0 / 9.0, 16.0 / 9.0)
    }

    fn approx(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol
    }

    #[test]
    fn raised_hand_does_not_change_frame() {
        let s = Settings::default();
        let mut fr = framer();
        let a = fr.frame_for(&s, &person(0.65), None, true);
        fr.reset_target();
        let b = fr.frame_for(&s, &make_person(0.65, 1.0, 0.15, 1), None, true);
        assert!(approx(a.cx, b.cx, 1e-9) && approx(a.cy, b.cy, 1e-9) && approx(a.h, b.h, 1e-9));
    }

    #[test]
    fn hidden_hips_close_to_full_pose() {
        let s = Settings::default();
        let mut fr = framer();
        let a = fr.frame_for(&s, &person(0.65), None, true);
        fr.reset_target();
        let b = fr.frame_for(&s, &make_person(0.65, 0.0, 0.27, 1), None, true);
        assert!(approx(b.h, a.h, a.h * 0.05));
        assert!(approx(b.cy, a.cy, 0.02));
    }

    #[test]
    fn head_is_inside_frame_with_headroom() {
        let s = Settings::default();
        let mut fr = framer();
        let f = fr.frame_for(&s, &person(0.65), None, true);
        let f = fr.clamp_frame(f);
        assert!(f.cy - f.h / 2.0 < 0.30 - 0.04);
        assert!(f.cy + f.h / 2.0 > 0.36);
    }

    #[test]
    fn zoom_respects_limits() {
        let s = Settings { max_zoom: 2.0, ..Default::default() };
        let mut fr = framer();
        let f = fr.frame_for(&s, &person(0.65), None, true);
        assert!(1.0 / f.h <= 2.0 + 1e-9);
    }

    #[test]
    fn frame_clamped_inside_source() {
        let s = Settings::default();
        let mut fr = framer();
        let f = fr.frame_for(&s, &person(0.99), None, true);
        let [x0, y0, x1, y1] = fr.rect(&fr.clamp_frame(f));
        assert!(x0 >= -1e-9 && x1 <= 1.0 + 1e-9 && y0 >= -1e-9 && y1 <= 1.0 + 1e-9);
    }

    #[test]
    fn dead_zone() {
        let s = Settings::default();
        let mut fr = framer();
        fr.force_goal(Frame::new(0.5, 0.4, 0.33));
        fr.set_goal(&s, Frame::new(0.52, 0.41, 0.34), false);
        assert_eq!(fr.goal.cx, 0.5);
        fr.set_goal(&s, Frame::new(0.7, 0.41, 0.34), false);
        assert_eq!(fr.goal.cx, 0.7);
    }

    #[test]
    fn smoothing_converges() {
        let s = Settings::default();
        let mut fr = framer();
        let f = fr.frame_for(&s, &person(0.65), None, true);
        fr.set_goal(&s, f, false);
        let f = fr.frame_for(&s, &person(0.65), None, true);
        fr.force_goal(fr.clamp_frame(f));
        let mut cur = fr.current;
        for _ in 0..300 {
            cur = fr.step(&s, 1.0 / 30.0);
        }
        assert!(approx(cur.cx, fr.goal.cx, 1e-3));
    }

    #[test]
    fn group_frame_holds_both_people() {
        let s = Settings::default();
        let mut fr = framer();
        let a = make_person(0.35, 1.0, 0.27, 1);
        let b = make_person(0.62, 1.0, 0.27, 2);
        let f = fr.frame_for_group(&s, &[&a, &b]);
        let [x0, _, x1, _] = fr.rect(&f);
        assert!(x0 < a.bbox[0] && x1 > b.bbox[2], "both people inside");
        assert!(f.h > fr.frame_for(&s, &a, None, true).h, "wider than a single shot");
    }
}
