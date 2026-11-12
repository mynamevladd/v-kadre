//! Remembering the speaker by clothing color.
//!
//! A person is described by a small color histogram of the torso (shoulders to hips): hue bins for
//! colored pixels plus brightness bins for grey/black/white ones. Cheap enough to compute for every
//! person on every detection frame, and stable when the person turns sideways or walks.

use crate::framing::Person;
use crate::rgb::RgbImage;

pub const HUE_BINS: usize = 16;
pub const GREY_BINS: usize = 4;
pub const SIG_LEN: usize = HUE_BINS + GREY_BINS;

pub type Signature = [f32; SIG_LEN];

/// RGB -> (hue 0..1, saturation 0..1, value 0..1)
fn hsv(px: [u8; 3]) -> (f32, f32, f32) {
    let r = px[0] as f32 / 255.0;
    let g = px[1] as f32 / 255.0;
    let b = px[2] as f32 / 255.0;
    let mx = r.max(g).max(b);
    let mn = r.min(g).min(b);
    let d = mx - mn;
    let s = if mx > 0.0 { d / mx.max(1e-6) } else { 0.0 };
    let h = if d <= 1e-6 {
        0.0
    } else if mx == r {
        ((g - b) / d).rem_euclid(6.0)
    } else if mx == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    (h / 6.0, s, mx)
}

/// Normalized box of the torso: between the shoulders, from shoulders to hips.
pub fn torso_region(p: &Person, kp_conf: f64) -> [f64; 4] {
    let [x0, y0, x1, y1] = p.bbox;
    if let Some(kp) = &p.kp {
        let sh: Vec<[f64; 2]> = [5, 6].iter().filter(|&&i| kp.conf[i] >= kp_conf).map(|&i| kp.xy[i]).collect();
        let hip: Vec<[f64; 2]> = [11, 12].iter().filter(|&&i| kp.conf[i] >= kp_conf).map(|&i| kp.xy[i]).collect();
        if sh.len() == 2 {
            let (sx0, sx1) = if sh[0][0] <= sh[1][0] { (sh[0][0], sh[1][0]) } else { (sh[1][0], sh[0][0]) };
            let sy = (sh[0][1] + sh[1][1]) / 2.0;
            let hy = if hip.is_empty() {
                sy + (y1 - y0) * 0.3
            } else {
                hip.iter().map(|h| h[1]).sum::<f64>() / hip.len() as f64
            };
            if sx1 - sx0 > 0.004 && hy > sy {
                let inset = (sx1 - sx0) * 0.1;
                return [sx0 + inset, sy + (hy - sy) * 0.1, sx1 - inset, hy];
            }
        }
    }
    let (w, h) = (x1 - x0, y1 - y0);
    [x0 + w * 0.25, y0 + h * 0.22, x1 - w * 0.25, y0 + h * 0.55]
}

/// Normalized color histogram of the person's torso, or None if the region is too small.
pub fn signature(img: &RgbImage, p: &Person, kp_conf: f64) -> Option<Signature> {
    let (iw, ih) = (img.width as f64, img.height as f64);
    let [x0, y0, x1, y1] = torso_region(p, kp_conf);
    let a = (x0.max(0.0) * iw) as i64;
    let b = (x1.min(1.0) * iw) as i64;
    let c = (y0.max(0.0) * ih) as i64;
    let d = (y1.min(1.0) * ih) as i64;
    if b - a < 3 || d - c < 3 {
        return None;
    }
    let (a, b, c, d) = (a as usize, b as usize, c as usize, d as usize);
    let rw = b - a;
    let n = rw * (d - c);
    // plenty for a histogram
    let step = if n > 1500 { n / 1500 + 1 } else { 1 };

    let mut hist = [0f32; SIG_LEN];
    let mut i = 0;
    while i < n {
        let px = img.pixel(a + i % rw, c + i / rw);
        let (h, s, v) = hsv(px);
        if s > 0.25 && v > 0.18 {
            hist[((h * HUE_BINS as f32) as usize).min(HUE_BINS - 1)] += 1.0;
        } else {
            hist[HUE_BINS + ((v * GREY_BINS as f32) as usize).min(GREY_BINS - 1)] += 1.0;
        }
        i += step;
    }
    let total: f32 = hist.iter().sum();
    if total == 0.0 {
        return None;
    }
    hist.iter_mut().for_each(|x| *x /= total);
    Some(hist)
}

/// Bhattacharyya coefficient: 1 = same colors, 0 = nothing in common.
pub fn similarity(a: Option<&Signature>, b: Option<&Signature>) -> f64 {
    match (a, b) {
        (Some(a), Some(b)) => a.iter().zip(b.iter()).map(|(x, y)| (x * y).sqrt() as f64).sum(),
        _ => 0.0,
    }
}

/// Color model of the chosen speaker, updated slowly while we are sure it is them.
#[derive(Debug, Clone)]
pub struct AppearanceMemory {
    pub model: Option<Signature>,
    pub rate: f32,
}

impl Default for AppearanceMemory {
    fn default() -> Self {
        AppearanceMemory { model: None, rate: 0.08 }
    }
}

impl AppearanceMemory {
    pub fn reset(&mut self, sig: Option<Signature>) {
        self.model = sig;
    }

    pub fn score(&self, sig: Option<&Signature>) -> f64 {
        similarity(self.model.as_ref(), sig)
    }

    pub fn update(&mut self, sig: Option<&Signature>) {
        let Some(sig) = sig else { return };
        match self.model {
            None => self.model = Some(*sig),
            Some(_) if self.score(Some(sig)) >= 0.5 => {
                // never drift onto someone else
                let m = self.model.as_mut().unwrap();
                for (x, y) in m.iter_mut().zip(sig.iter()) {
                    *x = (1.0 - self.rate) * *x + self.rate * y;
                }
                let total: f32 = m.iter().sum();
                if total > 0.0 {
                    m.iter_mut().for_each(|x| *x /= total);
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::framing::tests::make_person;

    #[test]
    fn signature_tells_shirts_apart() {
        let mut img = RgbImage::new(640, 360, 40);
        let red = make_person(0.2, 1.0, 0.27, 1);
        let blue = make_person(0.5, 1.0, 0.27, 2);
        let red2 = make_person(0.8, 1.0, 0.27, 3);
        img.fill_box(red.bbox, [200, 30, 30]);
        img.fill_box(blue.bbox, [30, 60, 200]);
        img.fill_box(red2.bbox, [190, 40, 35]);
        let sr = signature(&img, &red, 0.5);
        let sb = signature(&img, &blue, 0.5);
        let sr2 = signature(&img, &red2, 0.5);
        assert!(similarity(sr.as_ref(), sr2.as_ref()) > 0.9);
        assert!(similarity(sr.as_ref(), sb.as_ref()) < 0.3);
        let mut mem = AppearanceMemory::default();
        mem.reset(sr);
        mem.update(sb.as_ref()); // someone else: must not drift
        assert!(mem.score(sr.as_ref()) > 0.95);
    }
}
