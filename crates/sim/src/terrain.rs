//! The height of the ground (Linear DCM-83): low-poly hills outside a flat town.
//!
//! The ground is a grid of corner heights every [`TerrainDef::cell_m`] metres, each square split
//! into two flat triangles (the diagonal alternating like a chequerboard), so what the client draws
//! is faceted and [`Terrain::height`] is exactly the facet a body stands on. The town (its bounds,
//! [`TerrainDef::flat_margin_m`] round it) is flat at 0, so buildings, streets and lots are as they
//! always were; beyond it the hills come in over [`TerrainDef::ramp_m`] metres. Slopes are kept
//! gentle ([`TerrainDef::max_grade`]): the camera looks down at 30°, and a steep rise would hide
//! what is behind it.
//!
//! Worked out from the seed and the town's bounds with nothing but `+ - * /` and `sqrt` (no
//! transcendental functions), so the server and every client get the same heights on any machine:
//! derived, never saved or sent. Walking uphill is slower ([`Terrain::slope_factor`]); that's the
//! only thing the simulation reads from it, besides where the client draws things.

use glam::{IVec2, Vec2};
use serde::{Deserialize, Serialize};

/// How the ground rises outside town. Part of the world's generation settings
/// (`GenConfig::terrain`), so saved with them and sent to joining clients; every field defaults,
/// so settings from before it get the shipped hills.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TerrainDef {
    /// How high the hills rise above the town at most, in metres (a little of it below: hollows).
    /// 0 makes the world flat.
    pub relief_m: f32,
    /// The side of one square of the ground's grid, in metres; each is two flat facets. Bigger is
    /// more low-poly.
    pub cell_m: i32,
    /// About how far apart hills are, in metres.
    pub wavelength_m: f32,
    /// How far round the town the ground stays flat, in metres.
    pub flat_margin_m: f32,
    /// Over how many metres beyond that the hills come up to their full height.
    pub ramp_m: f32,
    /// The steepest any facet may be (rise over run): 0.14 is 14 m up over 100 m.
    pub max_grade: f32,
    /// How much a slope slows a body walking up it: its speed times 1 − `uphill_slow` × the grade
    /// it climbs (never below half). Walking down or across is as on the flat.
    pub uphill_slow: f32,
    /// How far each corner of the grid is raised or lowered at random, in metres, on top of the
    /// hills (none in town): the facets then tilt each its own way and the light picks each one
    /// out, even on a gentle slope. 0 leaves the hills smooth.
    pub roughness_m: f32,
}

impl Default for TerrainDef {
    fn default() -> Self {
        Self { relief_m: 18.0, cell_m: 6, wavelength_m: 130.0, flat_margin_m: 6.0, ramp_m: 70.0, max_grade: 0.22, uphill_slow: 1.5, roughness_m: 0.6 }
    }
}

impl TerrainDef {
    /// Flat ground everywhere.
    pub fn flat() -> Self {
        Self { relief_m: 0.0, ..Self::default() }
    }

    /// What's wrong with these settings, if anything (for checking a save's).
    pub fn check(&self) -> Result<(), String> {
        let finite = [self.relief_m, self.wavelength_m, self.flat_margin_m, self.ramp_m, self.max_grade, self.uphill_slow].iter().all(|v| v.is_finite());
        if !finite {
            return Err("terrain: every number must be finite".into());
        }
        if !(0.0..=60.0).contains(&self.relief_m) {
            return Err(format!("terrain: relief_m {} is not 0 to 60", self.relief_m));
        }
        if !(1..=32).contains(&self.cell_m) {
            return Err(format!("terrain: cell_m {} is not 1 to 32", self.cell_m));
        }
        if !(8.0..=4000.0).contains(&self.wavelength_m) || self.ramp_m < 1.0 || self.flat_margin_m < 0.0 {
            return Err("terrain: wavelength_m must be 8 to 4000, ramp_m at least 1, flat_margin_m at least 0".into());
        }
        if !(0.01..=1.0).contains(&self.max_grade) || !(0.0..=3.0).contains(&self.uphill_slow) {
            return Err("terrain: max_grade must be 0.01 to 1, uphill_slow 0 to 3".into());
        }
        if !(0.0..=5.0).contains(&self.roughness_m) || !self.roughness_m.is_finite() {
            return Err(format!("terrain: roughness_m {} is not 0 to 5", self.roughness_m));
        }
        Ok(())
    }
}

/// The ground's heights: corners every `cell` metres from the map's corner. Empty is flat.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Terrain {
    cell: i32,
    /// Corners across and down.
    nx: i32,
    ny: i32,
    h: Vec<f32>,
    uphill_slow: f32,
}

/// One flat facet of the ground: its three corners as (x, height, y) in world metres.
pub type Facet = [glam::Vec3; 3];

impl Terrain {
    /// The ground for a map of `tiles` with the town in `town_min..town_max` (tiles).
    pub fn generate(def: &TerrainDef, seed: u64, tiles: IVec2, town_min: IVec2, town_max: IVec2) -> Self {
        if def.relief_m <= 0.0 || def.check().is_err() {
            return Self::default();
        }
        let cell = def.cell_m;
        let (nx, ny) = (tiles.x.div_euclid(cell) + 2, tiles.y.div_euclid(cell) + 2);
        let (lo, hi) = (town_min.as_vec2(), town_max.as_vec2());
        let mut h = vec![0.0f32; (nx * ny) as usize];
        let mut locked = vec![false; h.len()];
        for j in 0..ny {
            for i in 0..nx {
                let p = Vec2::new((i * cell) as f32, (j * cell) as f32);
                let d = (p.clamp(lo, hi) - p).length();
                let m = smooth((d - def.flat_margin_m) / def.ramp_m);
                let k = (j * nx + i) as usize;
                if m <= 0.0 {
                    locked[k] = true;
                    continue;
                }
                let n = fbm(p / def.wavelength_m, seed);
                // Mostly rising, a little falling: hollows below the town's level.
                h[k] = def.relief_m * m * (n * 1.25 - 0.25);
                // Each corner a little up or down, so neighbouring facets catch the light differently.
                let jitter = crate::rng::hash01(seed, i as u64, j as u64, 0x7e45) * 2.0 - 1.0;
                h[k] += def.roughness_m * m * jitter;
            }
        }
        // Keep every facet gentle: the difference along a side at most this (a facet's grade is
        // then at most `max_grade`, its two sides' grades combined). The town's flat corners never
        // move; their neighbours give way.
        let lim = def.max_grade * cell as f32 * std::f32::consts::FRAC_1_SQRT_2;
        for _ in 0..256 {
            let mut changed = false;
            for j in 0..ny {
                for i in 0..nx {
                    let a = (j * nx + i) as usize;
                    for (di, dj) in [(1, 0), (0, 1)] {
                        if i + di >= nx || j + dj >= ny {
                            continue;
                        }
                        let b = ((j + dj) * nx + i + di) as usize;
                        let diff = h[a] - h[b];
                        if diff.abs() <= lim + 1e-5 {
                            continue;
                        }
                        let (hi_k, lo_k) = if diff > 0.0 { (a, b) } else { (b, a) };
                        // Bring the higher down to the lower, unless it is held: then the lower up.
                        if !locked[hi_k] {
                            h[hi_k] = h[lo_k] + lim;
                        } else {
                            h[lo_k] = h[hi_k] - lim;
                        }
                        changed = true;
                    }
                }
            }
            if !changed {
                break;
            }
        }
        Self { cell, nx, ny, h, uphill_slow: def.uphill_slow }
    }

    /// Flat everywhere?
    pub fn is_flat(&self) -> bool {
        self.h.is_empty()
    }

    /// The grid's square size in metres (0 when flat).
    pub fn cell(&self) -> i32 {
        if self.is_flat() { 0 } else { self.cell }
    }

    fn corner(&self, i: i32, j: i32) -> f32 {
        let (i, j) = (i.clamp(0, self.nx - 1), j.clamp(0, self.ny - 1));
        self.h[(j * self.nx + i) as usize]
    }

    /// The height of the ground at `p` (x, y in world metres), on the facet `p` is over.
    pub fn height(&self, p: Vec2) -> f32 {
        if self.is_flat() {
            return 0.0;
        }
        let c = self.cell as f32;
        let (gx, gy) = (p.x / c, p.y / c);
        let (i, j) = (gx.floor() as i32, gy.floor() as i32);
        let (fx, fy) = (gx - i as f32, gy - j as f32);
        let (h00, h10, h01, h11) = (self.corner(i, j), self.corner(i + 1, j), self.corner(i, j + 1), self.corner(i + 1, j + 1));
        if (i + j).rem_euclid(2) == 0 {
            // Split along (0,0)-(1,1).
            if fx >= fy { h00 + fx * (h10 - h00) + fy * (h11 - h10) } else { h00 + fy * (h01 - h00) + fx * (h11 - h01) }
        } else if fx + fy <= 1.0 {
            // Split along (1,0)-(0,1).
            h00 + fx * (h10 - h00) + fy * (h01 - h00)
        } else {
            h11 + (1.0 - fx) * (h01 - h11) + (1.0 - fy) * (h10 - h11)
        }
    }

    /// How the ground rises at `p`: metres up per metre east (x) and south (y), the facet's slope.
    pub fn gradient(&self, p: Vec2) -> Vec2 {
        if self.is_flat() {
            return Vec2::ZERO;
        }
        let f = self.facet_at(p);
        let (a, b, c) = (f[0], f[1], f[2]);
        let (u, v) = (b - a, c - a);
        // Normal of the facet: (u × v); height gradient = -(n.x, n.z) / n.y.
        let n = u.cross(v);
        if n.y.abs() < 1e-6 { Vec2::ZERO } else { Vec2::new(-n.x / n.y, -n.z / n.y) }
    }

    /// How steep the ground is round `p` at the scale of the hills, rise over run: the slope over
    /// a few cells either way, so it follows the hillside and not one facet's tilt (the
    /// [`TerrainDef::roughness_m`] each corner has). What grows there and what colour the ground is
    /// go by it.
    pub fn broad_grade(&self, p: Vec2) -> f32 {
        if self.is_flat() {
            return 0.0;
        }
        let d = 1.5 * self.cell as f32;
        let (dx, dy) = (Vec2::new(d, 0.0), Vec2::new(0.0, d));
        let gx = (self.height(p + dx) - self.height(p - dx)) / (2.0 * d);
        let gy = (self.height(p + dy) - self.height(p - dy)) / (2.0 * d);
        Vec2::new(gx, gy).length()
    }

    /// What a body moving along `dir` at `p` gets of its speed: less climbing (see
    /// [`TerrainDef::uphill_slow`]), never below half; 1 on the flat, across and downhill.
    pub fn slope_factor(&self, p: Vec2, dir: Vec2) -> f32 {
        if self.is_flat() || self.uphill_slow <= 0.0 {
            return 1.0;
        }
        let len = dir.length();
        if len < 1e-6 {
            return 1.0;
        }
        let climb = self.gradient(p).dot(dir / len);
        if climb <= 0.0 { 1.0 } else { (1.0 - self.uphill_slow * climb).max(0.5) }
    }

    /// The facet `p` is over.
    pub fn facet_at(&self, p: Vec2) -> Facet {
        let c = self.cell as f32;
        let (i, j) = ((p.x / c).floor() as i32, (p.y / c).floor() as i32);
        let (fx, fy) = (p.x / c - i as f32, p.y / c - j as f32);
        let [a, b] = self.facets(i, j);
        let first = if (i + j).rem_euclid(2) == 0 { fx >= fy } else { fx + fy <= 1.0 };
        if first { a } else { b }
    }

    /// The two facets of grid square `(i, j)` (corners at `i * cell`, `j * cell`): for an even
    /// square the one east of its (0,0)-(1,1) diagonal first, for an odd one the one north-west of
    /// its (1,0)-(0,1) diagonal first.
    pub fn facets(&self, i: i32, j: i32) -> [Facet; 2] {
        let v = |di: i32, dj: i32| glam::Vec3::new(((i + di) * self.cell) as f32, self.corner(i + di, j + dj), ((j + dj) * self.cell) as f32);
        if (i + j).rem_euclid(2) == 0 {
            [[v(0, 0), v(1, 0), v(1, 1)], [v(0, 0), v(1, 1), v(0, 1)]]
        } else {
            [[v(0, 0), v(1, 0), v(0, 1)], [v(1, 0), v(1, 1), v(0, 1)]]
        }
    }

    /// The grid squares (i, j) that the box `min..max` (world x, y) touches, as an inclusive range.
    pub fn squares(&self, min: Vec2, max: Vec2) -> (IVec2, IVec2) {
        let c = self.cell as f32;
        (IVec2::new((min.x / c).floor() as i32, (min.y / c).floor() as i32), IVec2::new(((max.x / c).ceil() as i32 - 1).max((min.x / c).floor() as i32), ((max.y / c).ceil() as i32 - 1).max((min.y / c).floor() as i32)))
    }

    /// The one height of the ground over all of `min..max`, if it is level there.
    pub fn level_over(&self, min: Vec2, max: Vec2) -> Option<f32> {
        if self.is_flat() {
            return Some(0.0);
        }
        let (lo, hi) = self.squares(min, max);
        let h = self.corner(lo.x, lo.y);
        for j in lo.y..=hi.y + 1 {
            for i in lo.x..=hi.x + 1 {
                if self.corner(i, j) != h {
                    return None;
                }
            }
        }
        Some(h)
    }

    /// The highest and lowest the ground goes.
    pub fn range(&self) -> (f32, f32) {
        self.h.iter().fold((0.0f32, 0.0f32), |(lo, hi), &v| (lo.min(v), hi.max(v)))
    }
}

/// 0 below 0, 1 above 1, eased between.
fn smooth(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// A lattice value from 0 to 1 for corner `(x, y)` of octave `o` (integer hashing only).
fn lattice(x: i32, y: i32, o: u32, seed: u64) -> f32 {
    let s = seed ^ 0x7e44_a15e_d00d;
    let mut h = (s as u32) ^ ((s >> 32) as u32).rotate_left(13) ^ (x as u32).wrapping_mul(0x8da6_b343) ^ (y as u32).wrapping_mul(0xd816_3841) ^ o.wrapping_mul(0xcb1a_b31f);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2c1b_3c6d);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297a_2d39);
    h ^= h >> 15;
    (h >> 8) as f32 / (1u32 << 24) as f32
}

/// Smooth value noise at `p`, octave `o`, from 0 to 1.
fn value(p: Vec2, o: u32, seed: u64) -> f32 {
    let (i, j) = (p.x.floor() as i32, p.y.floor() as i32);
    let (fx, fy) = (smooth(p.x - i as f32), smooth(p.y - j as f32));
    let a = lattice(i, j, o, seed);
    let b = lattice(i + 1, j, o, seed);
    let c = lattice(i, j + 1, o, seed);
    let d = lattice(i + 1, j + 1, o, seed);
    let top = a + (b - a) * fx;
    let bottom = c + (d - c) * fx;
    top + (bottom - top) * fy
}

/// Three octaves of value noise, from 0 to 1.
fn fbm(p: Vec2, seed: u64) -> f32 {
    let mut sum = 0.0;
    let mut amp = 1.0;
    let mut freq = 1.0;
    let mut total = 0.0;
    for o in 0..3 {
        sum += value(p * freq + Vec2::splat(o as f32 * 17.31), o, seed) * amp;
        total += amp;
        amp *= 0.45;
        freq *= 2.1;
    }
    sum / total
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Terrain {
        Terrain::generate(&TerrainDef::default(), 1, IVec2::splat(480), IVec2::new(60, 80), IVec2::new(420, 400))
    }

    #[test]
    fn the_town_is_flat_and_the_hills_rise_outside_it() {
        let t = sample();
        for p in [Vec2::new(100.0, 100.0), Vec2::new(240.0, 240.0), Vec2::new(419.0, 399.0), Vec2::new(57.0, 240.0)] {
            assert_eq!(t.height(p), 0.0, "{p}");
        }
        let (lo, hi) = t.range();
        assert!(hi > 1.0, "the hills rise: {lo}..{hi}");
        assert!(lo >= -TerrainDef::default().relief_m * 0.25 - 1e-3);
    }

    #[test]
    fn no_facet_is_steeper_than_the_limit() {
        let t = sample();
        let max = TerrainDef::default().max_grade;
        for j in 0..t.ny - 1 {
            for i in 0..t.nx - 1 {
                for f in t.facets(i, j) {
                    let mid = (f[0] + f[1] + f[2]) / 3.0;
                    let g = t.gradient(Vec2::new(mid.x, mid.z)).length();
                    assert!(g <= max + 1e-3, "square {i},{j}: grade {g}");
                }
            }
        }
    }

    #[test]
    fn heights_are_the_facets_and_continuous() {
        let t = sample();
        for k in 0..2000 {
            let p = Vec2::new((k * 37 % 470) as f32 + 0.37, (k * 53 % 470) as f32 + 0.61);
            let f = t.facet_at(p);
            // The facet's plane at p is the height.
            let n = (f[1] - f[0]).cross(f[2] - f[0]);
            let h = f[0].y - (n.x * (p.x - f[0].x) + n.z * (p.y - f[0].z)) / n.y;
            assert!((h - t.height(p)).abs() < 1e-3, "{p}: {h} vs {}", t.height(p));
            // No step across a facet's edge.
            let q = p + Vec2::new(0.01, 0.01);
            assert!((t.height(p) - t.height(q)).abs() < 0.01, "{p}");
        }
    }

    #[test]
    fn the_same_seed_gives_the_same_ground() {
        assert_eq!(sample(), sample());
        let other = Terrain::generate(&TerrainDef::default(), 2, IVec2::splat(480), IVec2::new(60, 80), IVec2::new(420, 400));
        assert_ne!(sample(), other);
        assert!(Terrain::generate(&TerrainDef::flat(), 1, IVec2::splat(480), IVec2::ZERO, IVec2::ONE).is_flat());
    }

    #[test]
    fn climbing_is_slower() {
        let t = sample();
        let p = (0..470).flat_map(|y| (0..470).map(move |x| Vec2::new(x as f32 + 0.5, y as f32 + 0.5))).find(|&p| t.gradient(p).length() > 0.05).expect("a slope");
        let up = t.gradient(p).normalize();
        assert!(t.slope_factor(p, up) < 0.95);
        assert_eq!(t.slope_factor(p, -up), 1.0);
        assert_eq!(t.slope_factor(Vec2::new(240.0, 240.0), up), 1.0);
    }
}
