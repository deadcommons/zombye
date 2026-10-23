//! Uniform-grid spatial hash rebuilt every tick with a counting sort (no allocation after warm-up).
//!
//! The grid only spans the bounding box of the items in it (rebuilt each time), so a big map with
//! the indexed zombies all near the players doesn't pay for a prefix sum over every cell of the map.

use glam::{IVec2, Vec2};

#[derive(Clone, Debug, Default)]
pub struct SpatialHash {
    cell: f32,
    /// Cell coordinates of the window's first cell, and its size in cells.
    origin: IVec2,
    cols: i32,
    rows: i32,
    /// `starts[c]..starts[c + 1]` indexes `items` for cell `c`.
    starts: Vec<u32>,
    items: Vec<u32>,
    /// Scratch for `rebuild`.
    fill: Vec<u32>,
}

impl SpatialHash {
    /// A hash with cells `cell` metres wide. (The world size is no longer needed: the grid spans
    /// whatever its items cover.)
    pub fn new(_world_w: f32, _world_h: f32, cell: f32) -> Self {
        Self { cell, origin: IVec2::ZERO, cols: 0, rows: 0, starts: vec![0], items: Vec::new(), fill: Vec::new() }
    }

    fn cell_xy(&self, p: Vec2) -> IVec2 {
        (p / self.cell).floor().as_ivec2()
    }

    fn cell_of(&self, p: Vec2) -> usize {
        let c = self.cell_xy(p) - self.origin;
        let (cx, cy) = (c.x.clamp(0, self.cols - 1), c.y.clamp(0, self.rows - 1));
        (cy * self.cols + cx) as usize
    }

    pub fn rebuild(&mut self, positions: &[Vec2]) {
        self.rebuild_where(positions, |_| true);
    }

    /// Rebuild with only the items `keep` accepts (indices still refer to `positions`).
    pub fn rebuild_where(&mut self, positions: &[Vec2], keep: impl Fn(usize) -> bool) {
        // The window: the cells the kept items cover.
        let (mut lo, mut hi) = (IVec2::splat(i32::MAX), IVec2::splat(i32::MIN));
        let mut kept = 0;
        for (i, p) in positions.iter().enumerate() {
            if keep(i) {
                let c = self.cell_xy(*p);
                lo = lo.min(c);
                hi = hi.max(c);
                kept += 1;
            }
        }
        if kept == 0 {
            (lo, hi) = (IVec2::ZERO, IVec2::ZERO);
        }
        self.origin = lo;
        self.cols = hi.x - lo.x + 1;
        self.rows = hi.y - lo.y + 1;
        self.starts.clear();
        self.starts.resize((self.cols * self.rows + 1) as usize, 0);
        for (i, p) in positions.iter().enumerate() {
            if keep(i) {
                let c = self.cell_of(*p);
                self.starts[c + 1] += 1;
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

    /// Number of items indexed.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Calls `f(index)` for every item in the 3×3 cells around `p`.
    pub fn for_each_near(&self, p: Vec2, mut f: impl FnMut(usize)) {
        let c = self.cell_xy(p) - self.origin;
        for y in (c.y - 1).max(0)..=(c.y + 1).min(self.rows - 1) {
            for x in (c.x - 1).max(0)..=(c.x + 1).min(self.cols - 1) {
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
        let lo = ((p - Vec2::splat(r)) / self.cell).floor().as_ivec2() - self.origin;
        let hi = ((p + Vec2::splat(r)) / self.cell).floor().as_ivec2() - self.origin;
        let (x0, x1) = (lo.x.max(0), hi.x.min(self.cols - 1));
        let (y0, y1) = (lo.y.max(0), hi.y.min(self.rows - 1));
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
        // Queries outside the window find nothing (and don't panic).
        let mut far = 0;
        h.for_each_near(Vec2::new(500.0, 500.0), |_| far += 1);
        h.for_each_within(Vec2::new(500.0, -500.0), 10.0, |_| far += 1);
        assert_eq!(far, 0);
        // Only some items: the window shrinks to them, and nothing else is found.
        h.rebuild_where(&pts, |i| i % 50 == 0);
        let mut found = Vec::new();
        h.for_each_within(Vec2::new(20.0, 20.0), 40.0, |i| found.push(i));
        found.sort_unstable();
        assert_eq!(found, (0..400).filter(|i| i % 50 == 0).collect::<Vec<_>>());
        h.rebuild_where(&pts, |_| false);
        h.for_each_near(Vec2::new(1.0, 1.0), |_| panic!("empty"));
    }
}

