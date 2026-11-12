//! Plain RGB8 image buffer: what OBS screenshots decode into.

#[derive(Debug, Clone, PartialEq)]
pub struct RgbImage {
    pub width: usize,
    pub height: usize,
    /// row-major, 3 bytes per pixel, no padding
    pub data: Vec<u8>,
}

impl RgbImage {
    pub fn new(width: usize, height: usize, fill: u8) -> Self {
        RgbImage { width, height, data: vec![fill; width * height * 3] }
    }

    #[inline]
    pub fn pixel(&self, x: usize, y: usize) -> [u8; 3] {
        let i = (y * self.width + x) * 3;
        [self.data[i], self.data[i + 1], self.data[i + 2]]
    }

    #[inline]
    pub fn set_pixel(&mut self, x: usize, y: usize, c: [u8; 3]) {
        let i = (y * self.width + x) * 3;
        self.data[i..i + 3].copy_from_slice(&c);
    }

    pub fn from_jpeg(bytes: &[u8]) -> Option<RgbImage> {
        let img = image::load_from_memory_with_format(bytes, image::ImageFormat::Jpeg).ok()?.to_rgb8();
        let (w, h) = img.dimensions();
        Some(RgbImage { width: w as usize, height: h as usize, data: img.into_raw() })
    }

    /// Fills a normalized box (test helper and debug drawing).
    pub fn fill_box(&mut self, b: [f64; 4], c: [u8; 3]) {
        let (w, h) = (self.width as f64, self.height as f64);
        let x0 = (b[0] * w) as usize;
        let x1 = ((b[2] * w) as usize).min(self.width);
        let y0 = (b[1] * h) as usize;
        let y1 = ((b[3] * h) as usize).min(self.height);
        for y in y0..y1 {
            for x in x0..x1 {
                self.set_pixel(x, y, c);
            }
        }
    }
}
