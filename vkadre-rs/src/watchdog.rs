//! Camera watchdog: notices when the camera picture dies while it is on air.
//!
//!   * black  — the frame is (almost) black for a while: lens cap, camera off, dead battery;
//!   * frozen — the frame does not change at all: a live camera always has some sensor noise,
//!              so identical frames mean the capture card or the camera hung;
//!   * lost   — OBS cannot give us a frame from the source at all.

use crate::rgb::RgbImage;

/// mean brightness (0..255) below this counts as black
const BLACK_LEVEL: f64 = 10.0;
const BLACK_SECONDS: f64 = 2.0;
/// mean abs difference between frames below this counts as "same frame"
const FROZEN_DIFF: f64 = 0.08;
const FROZEN_SECONDS: f64 = 3.0;
const LOST_SECONDS: f64 = 2.5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    Black,
    Frozen,
    Lost,
}

impl Fault {
    pub fn message(self) -> &'static str {
        match self {
            Fault::Black => "Камера показывает чёрный кадр",
            Fault::Frozen => "Картинка с камеры зависла",
            Fault::Lost => "Нет сигнала с камеры",
        }
    }
}

#[derive(Debug, Default)]
pub struct CameraWatchdog {
    prev: Option<(usize, usize, Vec<u8>)>,
    black_since: Option<f64>,
    still_since: Option<f64>,
    fail_since: Option<f64>,
    pub fault: Option<Fault>,
}

impl CameraWatchdog {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Feed every grabbed frame. Returns the current fault (black / frozen) or None.
    pub fn frame(&mut self, img: &RgbImage, now: f64) -> Option<Fault> {
        self.fail_since = None;
        // every 4th pixel in both directions, all channels
        let (sw, sh) = (img.width.div_ceil(4), img.height.div_ceil(4));
        let mut small = Vec::with_capacity(sw * sh * 3);
        for y in (0..img.height).step_by(4) {
            for x in (0..img.width).step_by(4) {
                small.extend_from_slice(&img.pixel(x, y));
            }
        }
        let n = small.len().max(1) as f64;
        let mean = small.iter().map(|&v| v as u64).sum::<u64>() as f64 / n;

        if mean < BLACK_LEVEL {
            self.black_since.get_or_insert(now);
        } else {
            self.black_since = None;
        }

        let still = match &self.prev {
            Some((pw, ph, prev)) if *pw == sw && *ph == sh => {
                let diff: u64 = prev.iter().zip(small.iter()).map(|(&a, &b)| (a as i32 - b as i32).unsigned_abs() as u64).sum();
                (diff as f64 / n) < FROZEN_DIFF
            }
            _ => false,
        };
        if still {
            self.still_since.get_or_insert(now);
        } else {
            self.still_since = None;
        }
        self.prev = Some((sw, sh, small));

        self.fault = if self.black_since.is_some_and(|t| now - t >= BLACK_SECONDS) {
            Some(Fault::Black)
        } else if self.still_since.is_some_and(|t| now - t >= FROZEN_SECONDS) {
            Some(Fault::Frozen)
        } else {
            None
        };
        self.fault
    }

    /// Feed every failed grab.
    pub fn failure(&mut self, now: f64) -> Option<Fault> {
        let since = *self.fail_since.get_or_insert(now);
        if now - since >= LOST_SECONDS {
            self.fault = Some(Fault::Lost);
        }
        self.fault
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tiny xorshift so the test needs no rand crate.
    struct Rng(u64);
    impl Rng {
        fn image(&mut self) -> RgbImage {
            let mut img = RgbImage::new(640, 360, 0);
            for v in img.data.iter_mut() {
                self.0 ^= self.0 << 13;
                self.0 ^= self.0 >> 7;
                self.0 ^= self.0 << 17;
                *v = 30 + (self.0 % 170) as u8;
            }
            img
        }
    }

    #[test]
    fn black_and_frozen() {
        let mut wd = CameraWatchdog::default();
        let black = RgbImage::new(640, 360, 0);
        assert_eq!(wd.frame(&black, 0.0), None);
        assert_eq!(wd.frame(&black, 2.5), Some(Fault::Black));

        wd.reset();
        let mut rng = Rng(42);
        for i in 0..10 {
            // live camera: noise every frame
            assert_eq!(wd.frame(&rng.image(), i as f64 * 0.5), None);
        }
        let still = rng.image();
        wd.frame(&still, 10.0);
        assert_eq!(wd.frame(&still.clone(), 10.5), None); // same frame, not long enough yet
        assert_eq!(wd.frame(&still.clone(), 14.0), Some(Fault::Frozen));
        assert_eq!(wd.frame(&rng.image(), 14.0), None);
    }

    #[test]
    fn lost_after_failures() {
        let mut wd = CameraWatchdog::default();
        assert_eq!(wd.failure(0.0), None);
        assert_eq!(wd.failure(3.0), Some(Fault::Lost));
    }
}
