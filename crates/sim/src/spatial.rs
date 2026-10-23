//! Uniform-grid spatial hash rebuilt every tick with a counting sort (no allocation after warm-up).

use glam::Vec2;

#[derive(Clone, Debug, Default)]
pub struct SpatialHash {
    cell: f32,
    cols: i32,
    rows: i32,
    /// `starts[c]..starts[c + 1]` indexes `items` for cell `c`.
    starts: Vec<u32>,
    items: Vec<u32>,
    /// Scratch for `rebuild`.
    fill: Vec<u32>,
}

impl SpatialHash {
    pub fn new(world_w: f32, world_h: f32, cell: f32) -> Self {
        let cols = (world_w / cell).ceil() as i32 + 1;
        let rows = (world_h / cell).ceil() as i32 + 1;
        Self { cell, cols, rows, starts: vec![0; (cols * rows + 1) as usize], items: Vec::new(), fill: Vec::new() }
    }

    fn cell_of(&self, p: Vec2) -> usize {
        let cx = ((p.x / self.cell) as i32).clamp(0, self.cols - 1);
        let cy = ((p.y / self.cell) as i32).clamp(0, self.rows - 1);
        (cy * self.cols + cx) as usize
    }

    pub fn rebuild(&mut self, positions: &[Vec2]) {
        self.rebuild_where(positions, |_| true);
    }

    /// Rebuild with only the items `keep` accepts (indices still refer to `positions`).
    pub fn rebuild_where(&mut self, positions: &[Vec2], keep: impl Fn(usize) -> bool) {
        self.starts.iter_mut().for_each(|s| *s = 0);
        let mut kept = 0;
        for (i, p) in positions.iter().enumerate() {
            if keep(i) {
                let c = self.cell_of(*p);
                self.starts[c + 1] += 1;
                kept += 1;
            }
        }
        for i in 1..self.starts.len() {
            self.starts[i] += self.starts[i - 1];
        }
        self.items.resize(kept, 0);
        // Reuse `fill` across rebuilds instead of cloning `starts` each time.
        let mut fill = std::mem::take(&mut self.fill);
        fill.clear();
        fill.extend_from_slice(&self.starts);
        for (i, p) in positions.iter().enumerate() {
            if keep(i) {
                let c = self.cell_of(*p);
                self.items[fill[c] as usize] = i as u32;
                fill[c] += 1;
            }
        }
        self.fill = fill;
    }

    /// Calls `f(index)` for every item in the 3×3 cells around `p`.
    pub fn for_each_near(&self, p: Vec2, mut f: impl FnMut(usize)) {
        let cx = ((p.x / self.cell) as i32).clamp(0, self.cols - 1);
        let cy = ((p.y / self.cell) as i32).clamp(0, self.rows - 1);
        for y in (cy - 1).max(0)..=(cy + 1).min(self.rows - 1) {
            for x in (cx - 1).max(0)..=(cx + 1).min(self.cols - 1) {
                let c = (y * self.cols + x) as usize;
                for &i in &self.items[self.starts[c] as usize..self.starts[c + 1] as usize] {
                    f(i as usize);
                }
            }
        }
    }

    /// Calls `f(index)` for every item in the cells overlapping the square of half-size `r` around
    /// `p`: a superset of the items within `r`, so callers check the exact distance.
    pub fn for_each_within(&self, p: Vec2, r: f32, mut f: impl FnMut(usize)) {
        let lo = ((p - Vec2::splat(r)) / self.cell).floor();
        let hi = ((p + Vec2::splat(r)) / self.cell).floor();
        let (x0, x1) = ((lo.x as i32).max(0), (hi.x as i32).min(self.cols - 1));
        let (y0, y1) = ((lo.y as i32).max(0), (hi.y as i32).min(self.rows - 1));
        if x0 > x1 {
            return;
        }
        for y in y0..=y1 {
            // A row of cells is contiguous in `items`, so one slice covers it.
            let row = (y * self.cols) as usize;
            let (a, b) = (self.starts[row + x0 as usize] as usize, self.starts[row + x1 as usize + 1] as usize);
            for &i in &self.items[a..b] {
                f(i as usize);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn within_finds_everything_in_range() {
        let pts: Vec<Vec2> = (0..400).map(|i| Vec2::new((i % 20) as f32 * 1.7 + 0.3, (i / 20) as f32 * 1.3 + 0.2)).collect();
        let mut h = SpatialHash::new(40.0, 40.0, 1.0);
        h.rebuild(&pts);
        let c = Vec2::new(14.0, 11.0);
        let mut found = Vec::new();
        h.for_each_within(c, 6.0, |i| found.push(i));
        for (i, p) in pts.iter().enumerate() {
            if p.distance(c) <= 6.0 {
                assert!(found.contains(&i), "missed {i} at {p}");
            }
        }
        // Clamped at the map edge without panicking.
        h.for_each_within(Vec2::new(-5.0, 45.0), 8.0, |_| {});
    }
}

