//! Lightweight multi-person tracker: keeps stable IDs between frames.
//!
//! Greedy matching by IoU, falling back to center distance, with a short memory for
//! people that disappear for a few frames (occlusion by the pulpit, other people walking by).

use crate::framing::Person;

pub fn iou(a: &[f64; 4], b: &[f64; 4]) -> f64 {
    let (ix0, iy0) = (a[0].max(b[0]), a[1].max(b[1]));
    let (ix1, iy1) = (a[2].min(b[2]), a[3].min(b[3]));
    let inter = (ix1 - ix0).max(0.0) * (iy1 - iy0).max(0.0);
    if inter <= 0.0 {
        return 0.0;
    }
    let ua = (a[2] - a[0]) * (a[3] - a[1]) + (b[2] - b[0]) * (b[3] - b[1]) - inter;
    if ua > 0.0 { inter / ua } else { 0.0 }
}

#[derive(Debug, Clone)]
struct Track {
    tid: i64,
    bbox: [f64; 4],
    missed: u32,
}

#[derive(Debug, Clone)]
pub struct SimpleTracker {
    iou_thr: f64,
    /// normalized center distance for non-overlapping match
    max_dist: f64,
    /// frames a lost track is remembered
    max_missed: u32,
    tracks: Vec<Track>,
    next_id: i64,
}

impl Default for SimpleTracker {
    fn default() -> Self {
        Self::new(0.25, 0.12, 15)
    }
}

impl SimpleTracker {
    pub fn new(iou_thr: f64, max_dist: f64, max_missed: u32) -> Self {
        SimpleTracker { iou_thr, max_dist, max_missed, tracks: Vec::new(), next_id: 1 }
    }

    pub fn reset(&mut self) {
        self.tracks.clear();
    }

    pub fn update(&mut self, people: &mut [Person]) {
        let mut pairs: Vec<(f64, usize, usize)> = Vec::new();
        for (ti, t) in self.tracks.iter().enumerate() {
            let (tcx, tcy) = ((t.bbox[0] + t.bbox[2]) / 2.0, (t.bbox[1] + t.bbox[3]) / 2.0);
            for (pi, p) in people.iter().enumerate() {
                let o = iou(&t.bbox, &p.bbox);
                let (cx, cy) = p.center();
                let d = (cx - tcx).hypot(cy - tcy);
                if o >= self.iou_thr || d <= self.max_dist {
                    // Prefer overlap; distance breaks ties.
                    pairs.push((-o + d * 0.5, ti, pi));
                }
            }
        }
        pairs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

        let mut used_t = vec![false; self.tracks.len()];
        let mut used_p = vec![false; people.len()];
        for (_, ti, pi) in pairs {
            if used_t[ti] || used_p[pi] {
                continue;
            }
            used_t[ti] = true;
            used_p[pi] = true;
            let t = &mut self.tracks[ti];
            t.bbox = people[pi].bbox;
            t.missed = 0;
            people[pi].tid = t.tid;
        }

        for (ti, t) in self.tracks.iter_mut().enumerate() {
            if !used_t[ti] {
                t.missed += 1;
            }
        }
        let max_missed = self.max_missed;
        self.tracks.retain(|t| t.missed <= max_missed);

        for (pi, p) in people.iter_mut().enumerate() {
            if !used_p[pi] {
                p.tid = self.next_id;
                self.tracks.push(Track { tid: self.next_id, bbox: p.bbox, missed: 0 });
                self.next_id += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_ids() {
        let mut tr = SimpleTracker::default();
        let mut a = vec![Person::new([0.1, 0.2, 0.2, 0.8]), Person::new([0.6, 0.2, 0.7, 0.8])];
        tr.update(&mut a);
        let ids = (a[0].tid, a[1].tid);
        // order swapped, moved
        let mut b = vec![Person::new([0.62, 0.2, 0.72, 0.8]), Person::new([0.11, 0.2, 0.21, 0.8])];
        tr.update(&mut b);
        assert_eq!((b[1].tid, b[0].tid), ids);
    }

    #[test]
    fn remembers_briefly_lost() {
        let mut tr = SimpleTracker::new(0.25, 0.12, 5);
        let mut a = vec![Person::new([0.4, 0.2, 0.5, 0.8])];
        tr.update(&mut a);
        for _ in 0..3 {
            tr.update(&mut []);
        }
        let mut b = vec![Person::new([0.41, 0.2, 0.51, 0.8])];
        tr.update(&mut b);
        assert_eq!(b[0].tid, a[0].tid);
    }
}
