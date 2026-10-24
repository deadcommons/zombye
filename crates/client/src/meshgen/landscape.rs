//! The outdoors of one 16 m chunk, read from the tile map alone (no extra sim data): the ground
//! (lawns in broad patches, rough verges and unkempt edges, dirt, a darker forest floor), roads
//! with paint at real scale (centre lines, parking bays, crossings and stop lines, worked out from
//! the road's shape and the sim's `RoadLine` tiles), kerbs, gutters, wheel tracks and repair
//! patches, sidewalk slab joints, driveways from garage doors, flower beds and shrubs in front of
//! houses, fences and hedges, trees, bushes, and forest litter (ferns, rocks, stumps, logs).
//!
//! Everything is merged into the chunk's two meshes, both single-sided: `ground` (casts no
//! shadows) and `props`. Paint, joints, patches and the like are cut into each ground tile as
//! rectangles that don't overlap ([`Paint`]), so no ground pixel is shaded twice. Variation comes
//! from [`hash01`] of tile coordinates, so a chunk always builds the same. Colours are in
//! [`super::land_palette`].

use std::f32::consts::TAU;

use bevy::prelude::*;
use zombye_sim::map::{CHUNK_SIZE, Edge, Feature, Floor, OwnedEdge, Side, Tile, TileMap};

use super::land_palette::{self as pal, Swatch};
use super::trees::{self, add_bush, add_tree};
use super::{CutTag, MeshBuilder, floor_color, hash01, shade};

/// Oil stains lie this far above the ground (everything else flat is cut into the ground itself,
/// see [`Paint`]), so they never flicker.
const Y_STAIN: f32 = 0.006;

/// Widest asphalt that still counts as one street, and how far road runs are followed.
const MAX_STREET: i32 = 14;
const RUN_CAP: i32 = 20;
/// Tiles of map cached round a chunk.
const MARGIN: i32 = 4;

/// Centre-line paint: dashed (3 m dashes, 9 m gaps) on ordinary streets, double solid yellow on a
/// street the sim paints solid (the main street).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Line {
    Dashed,
    Solid,
}

/// The cross-section of a street at one tile: which way it runs, its kerb-to-kerb extent across
/// (in metres, in world x for north–south streets, world z for east–west ones) and its paint.
#[derive(Clone, Copy, Debug)]
struct Street {
    along_x: bool,
    lo: f32,
    hi: f32,
    paint: Line,
}

impl Street {
    fn centre(&self) -> f32 {
        (self.lo + self.hi) * 0.5
    }

    fn width(&self) -> i32 {
        (self.hi - self.lo).round() as i32
    }
}

/// A driveway from a garage door to the sidewalk, and the strip of sidewalk in front of it where
/// the kerb drops.
#[derive(Clone, Copy, Debug)]
struct Driveway {
    min: Vec2,
    max: Vec2,
    cut_min: Vec2,
    cut_max: Vec2,
    /// Driveways run along x (else z).
    along_x: bool,
}

/// What a planting strip in front of a house holds.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Bed {
    Flowers,
    Shrubs,
}

/// One planted tile in front of a house: the tile, which side of it the wall is on, and what grows.
#[derive(Clone, Copy, Debug)]
struct BedTile {
    tile: IVec2,
    wall: Side,
    bed: Bed,
}

/// The chunk being built, with the map round it cached.
struct Land<'a> {
    map: &'a TileMap,
    min: IVec2,
    tiles: Vec<Tile>,
    /// Street cross-sections for the cached area (None off streets).
    streets: Vec<Option<Street>>,
    driveways: Vec<Driveway>,
    beds: Vec<BedTile>,
    /// Prefix sums over the cached area of trees and of pines, for [`Land::trees_near`].
    tree_sums: Vec<(u16, u16)>,
}

const SPAN: i32 = CHUNK_SIZE + 2 * MARGIN;

impl<'a> Land<'a> {
    fn new(map: &'a TileMap, c: IVec2) -> Self {
        let min = c * CHUNK_SIZE;
        let mut tiles = Vec::with_capacity((SPAN * SPAN) as usize);
        for y in 0..SPAN {
            for x in 0..SPAN {
                let t = min - IVec2::splat(MARGIN) + IVec2::new(x, y);
                tiles.push(if map.in_bounds(t) { map.tile(t) } else { Tile::default() });
            }
        }
        // tree_sums[(y + 1) * (SPAN + 1) + x + 1]: trees (and pines) in the cached tiles above
        // and left of (x, y), inclusive.
        let w = (SPAN + 1) as usize;
        let mut tree_sums = vec![(0u16, 0u16); w * w];
        for y in 0..SPAN as usize {
            for x in 0..SPAN as usize {
                let f = tiles[y * SPAN as usize + x].feature;
                let (a, p) = (matches!(f, Feature::Tree | Feature::PineTree) as u16, (f == Feature::PineTree) as u16);
                let up = tree_sums[y * w + x + 1];
                let left = tree_sums[(y + 1) * w + x];
                let diag = tree_sums[y * w + x];
                tree_sums[(y + 1) * w + x + 1] = (a + up.0 + left.0 - diag.0, p + up.1 + left.1 - diag.1);
            }
        }
        let mut land = Land { map, min, tiles, streets: Vec::new(), driveways: Vec::new(), beds: Vec::new(), tree_sums };
        let mut streets = Vec::with_capacity((SPAN * SPAN) as usize);
        for y in 0..SPAN {
            for x in 0..SPAN {
                streets.push(land.street_uncached(min - IVec2::splat(MARGIN) + IVec2::new(x, y)));
            }
        }
        land.streets = streets;
        land.find_houses();
        land
    }

    fn index(&self, t: IVec2) -> Option<usize> {
        let l = t - self.min + IVec2::splat(MARGIN);
        (l.x >= 0 && l.y >= 0 && l.x < SPAN && l.y < SPAN).then(|| (l.y * SPAN + l.x) as usize)
    }

    fn tile(&self, t: IVec2) -> Tile {
        match self.index(t) {
            Some(i) => self.tiles[i],
            None if self.map.in_bounds(t) => self.map.tile(t),
            None => Tile::default(),
        }
    }

    fn floor(&self, t: IVec2) -> Floor {
        self.tile(t).floor
    }

    fn outdoors(&self, t: IVec2) -> bool {
        self.tile(t).room == 0
    }

    fn is_road(&self, t: IVec2) -> bool {
        let tile = self.tile(t);
        tile.room == 0 && tile.floor.is_road() && self.map.in_bounds(t)
    }

    fn is_sidewalk(&self, t: IVec2) -> bool {
        let tile = self.tile(t);
        tile.room == 0 && tile.floor == Floor::Sidewalk
    }

    fn run(&self, t: IVec2, d: IVec2, f: impl Fn(IVec2) -> bool) -> i32 {
        (1..=RUN_CAP).take_while(|&k| f(t + d * k)).count() as i32
    }

    fn is_paint(&self, t: IVec2) -> bool {
        self.outdoors(t) && self.floor(t) == Floor::RoadLine
    }

    /// A parking-bay line: road paint with more paint three tiles to one side (bays are 3 m wide)
    /// and none right beside it in that direction. Street centre lines never have paint 3 m
    /// across from them.
    fn is_bay(&self, t: IVec2) -> bool {
        self.is_paint(t)
            && [IVec2::X, IVec2::Y].iter().any(|&d| {
                (self.is_paint(t + d * 3) || self.is_paint(t - d * 3)) && !self.is_paint(t + d) && !self.is_paint(t - d)
            })
    }

    fn is_centre_paint(&self, t: IVec2) -> bool {
        self.is_paint(t) && !self.is_bay(t)
    }

    fn street(&self, t: IVec2) -> Option<Street> {
        match self.index(t) {
            Some(i) if !self.streets.is_empty() => self.streets[i],
            _ => self.street_uncached(t),
        }
    }

    /// The street through `t`: asphalt narrow one way and running on the other, with the sim's
    /// centre paint near its middle (so a parking lot, a junction or a turning circle isn't one).
    fn street_uncached(&self, t: IVec2) -> Option<Street> {
        if !self.is_road(t) {
            return None;
        }
        let road = |p: IVec2| self.is_road(p);
        let (w, e, n, s) = (self.run(t, IVec2::NEG_X, road), self.run(t, IVec2::X, road), self.run(t, IVec2::NEG_Y, road), self.run(t, IVec2::Y, road));
        let (wx, wy) = (w + e + 1, n + s + 1);
        let (along_x, before, after) = if wy <= MAX_STREET && wx >= wy + 4 {
            (true, n, s)
        } else if wx <= MAX_STREET && wy >= wx + 4 {
            (false, w, e)
        } else {
            return None;
        };
        let (along, across) = split(t, along_x);
        let lo = (across - before) as f32;
        let hi = (across + after + 1) as f32;
        let centre = (lo + hi) * 0.5;
        let rows = [(centre - 0.5).floor() as i32, centre.floor() as i32];
        let (mut near, mut close) = (0, 0);
        for d in -6..=6 {
            for (i, &row) in rows.iter().enumerate() {
                if i == 1 && row == rows[0] {
                    continue;
                }
                if self.is_centre_paint(join(along + d, row, along_x)) {
                    near += 1;
                    close += (d.abs() <= 3) as i32;
                }
            }
        }
        if near == 0 {
            return None;
        }
        Some(Street { along_x, lo, hi, paint: if close >= 6 { Line::Solid } else { Line::Dashed } })
    }

    /// How many tiles ahead (1..=3, along `dir` = ±1) a junction with a crossing street starts,
    /// seen from street tile `t`. A junction is road that isn't a street, with road running a
    /// street's width or more both ways across: not a T we pass the mouth of, not a turning circle.
    fn junction_ahead(&self, t: IVec2, st: &Street, dir: i32) -> Option<i32> {
        let step = if st.along_x { IVec2::X } else { IVec2::Y } * dir;
        let side = if st.along_x { IVec2::Y } else { IVec2::X };
        for k in 1..=3 {
            let p = t + step * k;
            if !self.is_road(p) {
                return None;
            }
            if self.street(p).is_some() {
                continue;
            }
            let need = st.width() + 2;
            let road = |q: IVec2| self.is_road(q);
            let crossing = self.run(p, side, road) >= need && self.run(p, -side, road) >= need;
            return crossing.then_some(k);
        }
        None
    }

    /// Driveways and front beds of the buildings near this chunk.
    fn find_houses(&mut self) {
        let lo = self.min - IVec2::splat(24);
        let hi = self.min + IVec2::splat(CHUNK_SIZE + 24);
        let map = self.map;
        let (mut drives, mut beds) = (Vec::new(), Vec::new());
        for (bi, b) in map.buildings.iter().enumerate() {
            let (bmin, bmax) = (b.min, b.min + b.size);
            if bmax.x < lo.x || bmax.y < lo.y || bmin.x > hi.x || bmin.y > hi.y {
                continue;
            }
            let of_building = |p: IVec2| {
                let r = self.tile(p).room;
                r != 0 && map.rooms[r as usize].building as usize == bi
            };
            // Garage doors: openings from a garage to outside, grouped by the side they face.
            for side in Side::ALL {
                let mut opening: Vec<IVec2> = Vec::new();
                for y in bmin.y..bmax.y {
                    for x in bmin.x..bmax.x {
                        let p = IVec2::new(x, y);
                        let r = self.tile(p).room;
                        if r == 0 || !of_building(p) || map.rooms[r as usize].room_type != "garage" {
                            continue;
                        }
                        let q = p + side.offset();
                        let edge = map.edge(p, side);
                        if self.outdoors(q) && matches!(edge, Edge::Arch | Edge::DoorOpen | Edge::DoorClosed | Edge::DoorBroken) {
                            opening.push(p);
                        }
                    }
                }
                if let Some(d) = self.driveway(&opening, side) {
                    drives.push(d);
                }
            }
            // A bed or a row of shrubs along the front of some houses: the side whose door has a
            // garden path.
            if !b.template.starts_with("house") {
                continue;
            }
            let roll = hash01(b.min.x, b.min.y, 55);
            let bed = if roll < 0.4 {
                Bed::Flowers
            } else if roll < 0.75 {
                Bed::Shrubs
            } else {
                continue;
            };
            let mut front = None;
            for y in bmin.y..bmax.y {
                for x in bmin.x..bmax.x {
                    let p = IVec2::new(x, y);
                    if !of_building(p) {
                        continue;
                    }
                    for side in Side::ALL {
                        let q = p + side.offset();
                        if self.outdoors(q) && map.edge(p, side).is_door() && self.floor(q) == Floor::Path {
                            front = Some(side);
                        }
                    }
                }
            }
            let Some(front) = front else { continue };
            for y in bmin.y..bmax.y {
                for x in bmin.x..bmax.x {
                    let p = IVec2::new(x, y);
                    let q = p + front.offset();
                    if !of_building(p) || !self.outdoors(q) || map.edge(p, front) == Edge::Fence {
                        continue;
                    }
                    let along = if matches!(front, Side::N | Side::S) { IVec2::X } else { IVec2::Y };
                    let free = |r: IVec2| self.floor(r) == Floor::Grass && self.tile(r).feature == Feature::None && self.outdoors(r);
                    let by_path = [q + along, q - along].iter().any(|&r| self.floor(r) == Floor::Path);
                    if free(q) && !by_path && !map.edge(p, front).is_door() {
                        beds.push(BedTile { tile: q, wall: front.opposite(), bed });
                    }
                }
            }
        }
        // Beds don't go where a driveway runs.
        beds.retain(|b: &BedTile| !drives.iter().any(|d: &Driveway| inside(b.tile.as_vec2() + Vec2::splat(0.5), d.min, d.max)));
        self.driveways = drives;
        self.beds = beds;
    }

    /// A driveway in front of a run of garage openings on `side`, out to the sidewalk (or the
    /// road), if the way is clear.
    fn driveway(&self, opening: &[IVec2], side: Side) -> Option<Driveway> {
        let first = *opening.first()?;
        let last = *opening.last()?;
        let out = side.offset();
        let along_x = out.x != 0;
        let mid = (first + last) / 2;
        let mut k = 1;
        loop {
            let q = mid + out * k;
            if self.is_sidewalk(q) || self.is_road(q) {
                break;
            }
            let tile = self.tile(q);
            if k > 20 || tile.room != 0 || !matches!(tile.floor, Floor::Grass | Floor::Path | Floor::Dirt | Floor::Concrete) {
                return None;
            }
            k += 1;
        }
        let len = k - 1;
        if len < 1 {
            return None;
        }
        // Every tile it covers must be clear.
        let (a, b) = (first.min(last), first.max(last));
        for j in 1..=len {
            for p in [a, b, mid] {
                if self.tile(p + out * j).feature != Feature::None {
                    return None;
                }
            }
        }
        let flare = 0.3;
        let wall_tile_lo = a.as_vec2();
        let wall_tile_hi = b.as_vec2() + Vec2::ONE;
        // The wall line, and the line where the sidewalk starts.
        let (w, s) = match side {
            Side::S => (wall_tile_hi.y, wall_tile_hi.y + len as f32),
            Side::N => (wall_tile_lo.y, wall_tile_lo.y - len as f32),
            Side::E => (wall_tile_hi.x, wall_tile_hi.x + len as f32),
            Side::W => (wall_tile_lo.x, wall_tile_lo.x - len as f32),
        };
        // How far the sidewalk runs on to the road there.
        let mut walk = 0;
        while walk < 6 && self.is_sidewalk(mid + out * (k + walk)) {
            walk += 1;
        }
        let sw_end = s + walk as f32 * if matches!(side, Side::S | Side::E) { 1.0 } else { -1.0 };
        let (lo_across, hi_across) = if along_x { (wall_tile_lo.y - flare, wall_tile_hi.y + flare) } else { (wall_tile_lo.x - flare, wall_tile_hi.x + flare) };
        let rect = |p0: f32, p1: f32| {
            let (u0, u1) = (p0.min(p1), p0.max(p1));
            if along_x { (Vec2::new(u0, lo_across), Vec2::new(u1, hi_across)) } else { (Vec2::new(lo_across, u0), Vec2::new(hi_across, u1)) }
        };
        let (min, max) = rect(w, s);
        let (cut_min, cut_max) = rect(s, sw_end);
        Some(Driveway { min, max, cut_min, cut_max, along_x })
    }

    fn kerb_cut(&self, t: IVec2) -> bool {
        let c = t.as_vec2() + Vec2::splat(0.5);
        self.driveways.iter().any(|d| inside(c, d.cut_min, d.cut_max))
    }

    /// Trees, and pines among them, within `r` tiles of `t` (itself included); `r` up to `MARGIN`
    /// for tiles of this chunk.
    fn trees_near(&self, t: IVec2, r: i32) -> (i32, i32) {
        let lo = t - IVec2::splat(r) - self.min + IVec2::splat(MARGIN);
        let hi = t + IVec2::splat(r + 1) - self.min + IVec2::splat(MARGIN);
        if lo.min_element() < 0 || hi.max_element() > SPAN {
            return self.trees_near_slow(t, r);
        }
        let w = SPAN + 1;
        let at = |p: IVec2| self.tree_sums[(p.y * w + p.x) as usize];
        let (a, b, c, d) = (at(hi), at(IVec2::new(lo.x, hi.y)), at(IVec2::new(hi.x, lo.y)), at(lo));
        ((a.0 + d.0) as i32 - (b.0 + c.0) as i32, (a.1 + d.1) as i32 - (b.1 + c.1) as i32)
    }

    fn trees_near_slow(&self, t: IVec2, r: i32) -> (i32, i32) {
        let (mut all, mut pines) = (0, 0);
        for dy in -r..=r {
            for dx in -r..=r {
                match self.tile(t + IVec2::new(dx, dy)).feature {
                    Feature::Tree => all += 1,
                    Feature::PineTree => {
                        all += 1;
                        pines += 1;
                    }
                    _ => {}
                }
            }
        }
        (all, pines)
    }

    fn has_fence(&self, t: IVec2) -> bool {
        let tile = self.tile(t);
        tile.north == Edge::Fence || tile.west == Edge::Fence || self.tile(t + IVec2::X).west == Edge::Fence || self.tile(t + IVec2::Y).north == Edge::Fence
    }
}

/// (along, across) coordinates of a tile for a street running along x (or z).
fn split(t: IVec2, along_x: bool) -> (i32, i32) {
    if along_x { (t.x, t.y) } else { (t.y, t.x) }
}

fn join(along: i32, across: i32, along_x: bool) -> IVec2 {
    if along_x { IVec2::new(along, across) } else { IVec2::new(across, along) }
}

fn inside(p: Vec2, min: Vec2, max: Vec2) -> bool {
    p.x >= min.x && p.y >= min.y && p.x <= max.x && p.y <= max.y
}

/// A flat axis-aligned rectangle (world x/z) at height `y`, facing up.
fn decal(mb: &mut MeshBuilder, min: Vec2, max: Vec2, y: f32, colour: LinearRgba) {
    if max.x - min.x <= 1e-4 || max.y - min.y <= 1e-4 {
        return;
    }
    mb.quad([Vec3::new(min.x, y, min.y), Vec3::new(min.x, y, max.y), Vec3::new(max.x, y, max.y), Vec3::new(max.x, y, min.y)], Vec3::Y, colour);
}

/// A triangle wound to face up (ground clutter; the ground material culls back faces).
fn tri_up(mb: &mut MeshBuilder, [a, b, c]: [Vec3; 3], colour: LinearRgba) {
    if (b - a).cross(c - a).y >= 0.0 { mb.triangle([a, b, c], colour) } else { mb.triangle([a, c, b], colour) }
}

/// A triangle wound to face away from `inside`.
fn tri_out(mb: &mut MeshBuilder, [a, b, c]: [Vec3; 3], inside: Vec3, colour: LinearRgba) {
    if (b - a).cross(c - a).dot((a + b + c) / 3.0 - inside) >= 0.0 { mb.triangle([a, b, c], colour) } else { mb.triangle([a, c, b], colour) }
}

/// Smooth value noise in [0, 1) over world metres (`cell` metres per lattice step).
fn noise(p: Vec2, cell: f32, salt: u32) -> f32 {
    let q = p / cell;
    let i = q.floor();
    let f = q - i;
    let (x, y) = (i.x as i32, i.y as i32);
    let s = f * f * (Vec2::splat(3.0) - 2.0 * f);
    let a = hash01(x, y, salt);
    let b = hash01(x + 1, y, salt);
    let c = hash01(x, y + 1, salt);
    let d = hash01(x + 1, y + 1, salt);
    (a + (b - a) * s.x) + ((c + (d - c) * s.x) - (a + (b - a) * s.x)) * s.y
}

/// The flat colour areas of one ground tile, later ones on top. [`Paint::draw`] cuts the tile into rectangles that don't overlap, so every ground pixel
/// is drawn once: stacked decals cost a full shading pass per layer (the world shader's discards
/// rule out early depth tests), which at 4K was 3 ms on the main street.
struct Paint {
    lo: Vec2,
    rects: Vec<(Vec2, Vec2, LinearRgba)>,
}

/// At most this many rectangles on one tile (a busy main-street tile has about ten).
const MAX_RECTS: usize = 24;
const MAX_BREAKS: usize = 2 * MAX_RECTS + 2;

impl Paint {
    fn new() -> Self {
        Paint { lo: Vec2::ZERO, rects: Vec::with_capacity(MAX_RECTS) }
    }

    /// Start on tile `t`.
    fn reset(&mut self, t: IVec2) {
        self.lo = t.as_vec2();
        self.rects.clear();
    }

    fn rect(&mut self, min: Vec2, max: Vec2, colour: LinearRgba) {
        let (min, max) = (min.max(self.lo), max.min(self.lo + Vec2::ONE));
        if max.x - min.x > 1e-3 && max.y - min.y > 1e-3 && self.rects.len() < MAX_RECTS {
            self.rects.push((min, max, colour));
        }
    }

    /// A rectangle given in a street's (along, across) metres.
    fn street(&mut self, along_x: bool, a0: f32, a1: f32, c0: f32, c1: f32, colour: LinearRgba) {
        if along_x { self.rect(Vec2::new(a0, c0), Vec2::new(a1, c1), colour) } else { self.rect(Vec2::new(c0, a0), Vec2::new(c1, a1), colour) }
    }

    fn draw(&self, mb: &mut MeshBuilder, base: LinearRgba) {
        let (lo, hi) = (self.lo, self.lo + Vec2::ONE);
        if self.rects.is_empty() {
            decal(mb, lo, hi, 0.0, base);
            return;
        }
        let breaks = |f: fn(Vec2) -> f32| {
            let mut v = [0.0f32; MAX_BREAKS];
            let mut n = 0;
            for p in [f(lo), f(hi)].into_iter().chain(self.rects.iter().flat_map(|(a, b, _)| [f(*a), f(*b)])) {
                v[n] = p;
                n += 1;
            }
            let v = &mut v[..n];
            v.sort_unstable_by(f32::total_cmp);
            let mut m = 1;
            for i in 1..n {
                if v[i] - v[m - 1] >= 1e-4 {
                    v[m] = v[i];
                    m += 1;
                }
            }
            let mut out = [0.0f32; MAX_BREAKS];
            out[..m].copy_from_slice(&v[..m]);
            (out, m)
        };
        let ((xs, nx), (zs, nz)) = (breaks(|p| p.x), breaks(|p| p.y));
        let colour_at = |p: Vec2| self.rects.iter().rev().find(|(a, b, _)| p.x > a.x && p.y > a.y && p.x < b.x && p.y < b.y).map_or(base, |r| r.2);
        type Row = ([(f32, f32, LinearRgba); MAX_BREAKS], usize);
        let emit = |mb: &mut MeshBuilder, row: &Row, z0: f32, z1: f32| {
            for &(x0, x1, c) in &row.0[..row.1] {
                decal(mb, Vec2::new(x0, z0), Vec2::new(x1, z1), 0.0, c);
            }
        };
        let empty = (0.0, 0.0, LinearRgba::NONE);
        // Rows of runs of one colour; a row the same as the one before just stretches it.
        let mut open: Row = ([empty; MAX_BREAKS], 0);
        let mut open_z = zs[0];
        for j in 0..nz - 1 {
            let zc = (zs[j] + zs[j + 1]) * 0.5;
            let mut row: Row = ([empty; MAX_BREAKS], 0);
            for i in 0..nx - 1 {
                let c = colour_at(Vec2::new((xs[i] + xs[i + 1]) * 0.5, zc));
                if row.1 > 0 && row.0[row.1 - 1].2 == c {
                    row.0[row.1 - 1].1 = xs[i + 1];
                } else {
                    row.0[row.1] = (xs[i], xs[i + 1], c);
                    row.1 += 1;
                }
            }
            if j > 0 && row.0[..row.1] != open.0[..open.1] {
                emit(mb, &open, open_z, zs[j]);
                open_z = zs[j];
            }
            open = row;
        }
        emit(mb, &open, open_z, zs[nz - 1]);
    }
}

pub struct ChunkMeshes {
    pub ground: MeshBuilder,
    pub props: MeshBuilder,
}

/// Build the ground and props of chunk `c`.
pub fn build_chunk(map: &TileMap, c: IVec2) -> ChunkMeshes {
    let land = Land::new(map, c);
    let mut ground = MeshBuilder::with_capacity(1500);
    let mut props = MeshBuilder::with_capacity(1500);
    let mut paint = Paint::new();
    for y in land.min.y..land.min.y + CHUNK_SIZE {
        for x in land.min.x..land.min.x + CHUNK_SIZE {
            let t = IVec2::new(x, y);
            let tile = land.tile(t);
            if tile.room != 0 {
                // Floors inside buildings.
                flat_tile(&mut ground, t, floor_color(tile.floor, x, y));
                continue;
            }
            paint.reset(t);
            // Driveways and front beds lie over the grass.
            if !tile.floor.is_road() && tile.floor != Floor::Sidewalk {
                for d in &land.driveways {
                    driveway_paint(&mut paint, d);
                }
                if let Some(b) = land.beds.iter().find(|b| b.tile == t) {
                    bed_tile(&mut paint, &mut ground, &mut props, b);
                }
            }
            match tile.floor {
                Floor::Asphalt | Floor::RoadLine => road_tile(&mut paint, &mut ground, &land, t),
                Floor::Sidewalk => sidewalk_tile(&mut paint, &mut ground, &land, t),
                Floor::Concrete => {
                    slab_joints(&mut paint, t, true, true, 0.0, 0.0);
                    paint.draw(&mut ground, pal::SIDEWALK.lin());
                }
                Floor::Path => path_tile(&mut paint, &mut ground, &land, t),
                Floor::Dirt => soft_tile(&mut paint, &mut ground, &mut props, &land, t, true),
                _ => soft_tile(&mut paint, &mut ground, &mut props, &land, t, false),
            }
            match tile.feature {
                Feature::Tree | Feature::PineTree => {
                    if let Some(spec) = trees::tree_at(x, y, tile.feature) {
                        // Deep in the forest the neighbours hide most of a tree: fewer pieces.
                        let simple = land.trees_near(t, 2).0 >= 5;
                        add_tree(&mut props, &spec, simple);
                    }
                }
                Feature::Bush => {
                    let centre = Vec3::new(x as f32 + 0.3 + 0.4 * hash01(x, y, 3), 0.0, y as f32 + 0.3 + 0.4 * hash01(x, y, 4));
                    add_bush(&mut props, x, y, centre, 0.85 + 0.4 * hash01(x, y, 5));
                }
                _ => {}
            }
            for (which, kind) in [(OwnedEdge::North, tile.north), (OwnedEdge::West, tile.west)] {
                if kind == Edge::Fence {
                    props.set_tag(CutTag::default());
                    fence_or_hedge(&mut props, &land, t, which);
                }
            }
        }
    }
    let (lo, hi) = (land.min.as_vec2(), (land.min + IVec2::splat(CHUNK_SIZE)).as_vec2());
    for d in &land.driveways {
        driveway_stain(&mut ground, d, lo, hi);
    }
    ChunkMeshes { ground, props }
}

fn flat_tile(mb: &mut MeshBuilder, t: IVec2, colour: LinearRgba) {
    decal(mb, t.as_vec2(), t.as_vec2() + Vec2::ONE, 0.0, colour);
}

/// A ground tile as two triangles, each coloured by `colour(centroid)`: patches of colour then
/// follow the triangles' diagonals rather than square tile steps. The diagonal flips by hash.
fn split_tile(mb: &mut MeshBuilder, t: IVec2, colour: impl Fn(Vec2) -> LinearRgba) {
    let (x0, z0) = (t.x as f32, t.y as f32);
    let p = |dx: f32, dz: f32| Vec3::new(x0 + dx, 0.0, z0 + dz);
    let tris = if hash01(t.x, t.y, 11) < 0.5 {
        [[p(0.0, 0.0), p(0.0, 1.0), p(1.0, 1.0)], [p(0.0, 0.0), p(1.0, 1.0), p(1.0, 0.0)]]
    } else {
        [[p(0.0, 0.0), p(0.0, 1.0), p(1.0, 0.0)], [p(0.0, 1.0), p(1.0, 1.0), p(1.0, 0.0)]]
    };
    for tri in tris {
        let c = (tri[0] + tri[1] + tri[2]) / 3.0;
        tri_up(mb, tri, colour(c.xz()));
    }
}

fn road_tile(paint: &mut Paint, mb: &mut MeshBuilder, land: &Land, t: IVec2) {
    road_paint(paint, land, t);
    paint.draw(mb, pal::ASPHALT.lin());
    // Oil where cars stood in the bays (small, so drawn over the asphalt).
    // A bay is the two tiles after a bay line; the stain sits in its middle.
    let bay = [IVec2::X, IVec2::Y].into_iter().find(|&d| land.is_bay(t - d) && land.is_bay(t + d * 2));
    if let Some(d) = bay
        && !land.is_bay(t)
        && hash01(t.x, t.y, 36) < 0.2
    {
        let mid = Vec2::splat(0.5) + d.as_vec2() * 0.5;
        let at = t.as_vec2() + mid + (Vec2::new(hash01(t.x, t.y, 37), hash01(t.x, t.y, 38)) - 0.5) * 0.4;
        stain(mb, at, 0.8 + 0.4 * hash01(t.x, t.y, 39), hash01(t.x, t.y, 40));
    }
}

/// Everything flat on a road tile: repair patches, grit along the kerb, wheel tracks and paint.
/// Each is laid out in street metres and clipped to the tile, so a line crossing a tile edge is
/// drawn by both tiles.
fn road_paint(paint: &mut Paint, land: &Land, t: IVec2) {
    let tv = t.as_vec2();
    // Repair patches: rectangles of newer tar in some 5 m cells.
    let cell = t.div_euclid(IVec2::splat(5));
    for dy in -1..=0 {
        for dx in -1..=0 {
            let k = cell + IVec2::new(dx, dy);
            if hash01(k.x, k.y, 31) < 0.16 {
                let min = k.as_vec2() * 5.0 + Vec2::new(hash01(k.x, k.y, 32), hash01(k.x, k.y, 33)) * 3.0;
                let size = Vec2::new(1.2 + 2.6 * hash01(k.x, k.y, 34), 1.0 + 2.2 * hash01(k.x, k.y, 35));
                paint.rect(min, min + size, pal::ASPHALT_PATCH.lin());
            }
        }
    }
    // Grit along the kerb.
    for side in Side::ALL {
        if land.is_sidewalk(t + side.offset()) {
            let w = 0.28;
            let (min, max) = match side {
                Side::N => (tv, tv + Vec2::new(1.0, w)),
                Side::S => (tv + Vec2::new(0.0, 1.0 - w), tv + Vec2::ONE),
                Side::W => (tv, tv + Vec2::new(w, 1.0)),
                Side::E => (tv + Vec2::new(1.0 - w, 0.0), tv + Vec2::ONE),
            };
            paint.rect(min, max, pal::GUTTER.lin());
        }
    }
    if land.is_bay(t) {
        bay_line(paint, land, t);
        return;
    }
    let Some(st) = land.street(t) else { return };
    let (along, _) = split(t, st.along_x);
    let (a0, a1) = (along as f32, along as f32 + 1.0);
    let c = st.centre();
    // Wheel tracks: two worn bands in each lane.
    let lane = (st.hi - st.lo) * 0.5;
    if lane >= 2.6 {
        for mid in [st.lo + lane * 0.5, c + lane * 0.5] {
            for p in [mid - 0.82, mid + 0.82] {
                paint.street(st.along_x, a0, a1, p - 0.3, p + 0.3, pal::WHEEL_TRACK.lin());
            }
        }
    }
    let yellow = pal::PAINT_YELLOW.lin();
    let white = pal::PAINT_WHITE.lin();
    let ahead = [1, -1].map(|dir| land.junction_ahead(t, &st, dir).map(|k| (dir, k)));
    // Main street: zebra crossings before each junction, the centre line stopping short of them.
    if st.paint == Line::Solid {
        let mut crossing = false;
        for (dir, k) in ahead.into_iter().flatten() {
            crossing = true;
            // The junction's edge, and the crossing's band in the 2.6 m before it.
            let edge = if dir > 0 { along as f32 + k as f32 } else { along as f32 + 1.0 - k as f32 };
            let (z0, z1) = if dir > 0 { (edge - 2.9, edge - 0.3) } else { (edge + 0.3, edge + 2.9) };
            let mut s = st.lo + 0.35;
            while s + 0.5 <= st.hi - 0.3 {
                paint.street(st.along_x, z0, z1, s, s + 0.5, white);
                s += 1.0;
            }
        }
        if !crossing {
            for off in [-0.11, 0.11] {
                paint.street(st.along_x, a0, a1, c + off - 0.05, c + off + 0.05, yellow);
            }
        }
        return;
    }
    // Dashed centre line: 3 m dashes, 9 m gaps, laid out along the street in world metres.
    if along.rem_euclid(12) < 3 {
        paint.street(st.along_x, a0, a1, c - 0.06, c + 0.06, yellow);
    }
    // A stop line across the right-hand lane where the street meets a crossing street.
    for (dir, k) in ahead.into_iter().flatten() {
        if k != 1 {
            continue;
        }
        let edge = if dir > 0 { a1 } else { a0 };
        let (b0, b1) = if dir > 0 { (edge - 0.75, edge - 0.45) } else { (edge + 0.45, edge + 0.75) };
        // Right-hand side of the direction of travel: +across going +x, -across going +z.
        let plus = if st.along_x { dir > 0 } else { dir < 0 };
        let (s0, s1) = if plus { (c + 0.15, st.hi - 0.3) } else { (st.lo + 0.3, c - 0.15) };
        paint.street(st.along_x, b0, b1, s0, s1, white);
    }
}

/// A white parking-bay line through the middle of a bay-line tile, 10 cm wide.
fn bay_line(paint: &mut Paint, land: &Land, t: IVec2) {
    let runs_y = land.is_paint(t + IVec2::Y) || land.is_paint(t - IVec2::Y) || land.is_paint(t + IVec2::X * 3) || land.is_paint(t - IVec2::X * 3);
    let along_x = !runs_y;
    let (along, across) = split(t, along_x);
    let next = |d: i32| land.is_bay(join(along + d, across, along_x));
    // Stop 25 cm short of the open end of a bay.
    let a0 = along as f32 + if next(-1) { 0.0 } else { 0.25 };
    let a1 = along as f32 + 1.0 - if next(1) { 0.0 } else { 0.25 };
    let c = across as f32 + 0.5;
    paint.street(along_x, a0, a1, c - 0.05, c + 0.05, pal::PAINT_WHITE.lin());
}

fn sidewalk_tile(paint: &mut Paint, mb: &mut MeshBuilder, land: &Land, t: IVec2) {
    // Which way the sidewalk runs: joints across it every 1.5 m, and along it on wide ones.
    let paved = |p: IVec2| land.is_sidewalk(p);
    let (w, e, n, s) = (land.run(t, IVec2::NEG_X, paved), land.run(t, IVec2::X, paved), land.run(t, IVec2::NEG_Y, paved), land.run(t, IVec2::Y, paved));
    let (wx, wy) = (w + e + 1, n + s + 1);
    if wy <= 4 && wx > wy {
        slab_joints(paint, t, true, wy >= 3, (t.y - n) as f32, 0.0);
    } else if wx <= 4 && wy > wx {
        slab_joints(paint, t, wx >= 3, true, 0.0, (t.x - w) as f32);
    } else {
        slab_joints(paint, t, true, true, 0.0, 0.0);
    }
    // Kerbs: a low lip along every edge with road, dropped in front of driveways.
    if !land.kerb_cut(t) {
        let tv = t.as_vec2();
        for side in Side::ALL {
            if land.is_road(t + side.offset()) {
                kerb(mb, tv, side);
            }
        }
    }
    paint.draw(mb, pal::SIDEWALK.lin());
}

/// A kerb 16 cm wide and 6 cm high on a sidewalk tile's edge facing `side`: its top and its face
/// toward the road.
fn kerb(mb: &mut MeshBuilder, tv: Vec2, side: Side) {
    let (w, h) = (0.16, 0.06);
    let top = pal::KERB_TOP.lin();
    let face = pal::KERB_FACE.lin();
    let p = |x: f32, y: f32, z: f32| Vec3::new(tv.x + x, y, tv.y + z);
    match side {
        Side::N => {
            mb.quad([p(0.0, h, 0.0), p(0.0, h, w), p(1.0, h, w), p(1.0, h, 0.0)], Vec3::Y, top);
            mb.quad([p(0.0, 0.0, 0.0), p(0.0, h, 0.0), p(1.0, h, 0.0), p(1.0, 0.0, 0.0)], Vec3::NEG_Z, face);
        }
        Side::S => {
            mb.quad([p(0.0, h, 1.0 - w), p(0.0, h, 1.0), p(1.0, h, 1.0), p(1.0, h, 1.0 - w)], Vec3::Y, top);
            mb.quad([p(1.0, 0.0, 1.0), p(1.0, h, 1.0), p(0.0, h, 1.0), p(0.0, 0.0, 1.0)], Vec3::Z, face);
        }
        Side::W => {
            mb.quad([p(0.0, h, 0.0), p(0.0, h, 1.0), p(w, h, 1.0), p(w, h, 0.0)], Vec3::Y, top);
            mb.quad([p(0.0, 0.0, 1.0), p(0.0, h, 1.0), p(0.0, h, 0.0), p(0.0, 0.0, 0.0)], Vec3::NEG_X, face);
        }
        Side::E => {
            mb.quad([p(1.0 - w, h, 0.0), p(1.0 - w, h, 1.0), p(1.0, h, 1.0), p(1.0, h, 0.0)], Vec3::Y, top);
            mb.quad([p(1.0, 0.0, 0.0), p(1.0, h, 0.0), p(1.0, h, 1.0), p(1.0, 0.0, 1.0)], Vec3::X, face);
        }
    }
}

/// Slab joints on a paved tile, 5 cm wide: lines of constant x every 1.5 m from `x0` (when
/// `across_x`) and of constant z every 1.5 m from `z0` (when `across_z`).
fn slab_joints(paint: &mut Paint, t: IVec2, across_x: bool, across_z: bool, z0: f32, x0: f32) {
    let colour = pal::SIDEWALK_JOINT.lin();
    let tv = t.as_vec2();
    let lines = |from: f32, base: f32| {
        let mut out = Vec::new();
        let mut k = ((from - base) / 1.5).ceil();
        while base + k * 1.5 < from + 1.0 {
            let p = base + k * 1.5;
            if p > from + 0.02 && p < from + 0.98 || (p - from).abs() < 0.02 {
                out.push(p);
            }
            k += 1.0;
        }
        out
    };
    if across_x {
        for p in lines(tv.x, x0) {
            paint.rect(Vec2::new(p - 0.025, tv.y), Vec2::new(p + 0.025, tv.y + 1.0), colour);
        }
    }
    if across_z {
        for p in lines(tv.y, z0) {
            paint.rect(Vec2::new(tv.x, p - 0.025), Vec2::new(tv.x + 1.0, p + 0.025), colour);
        }
    }
}

fn path_tile(paint: &mut Paint, mb: &mut MeshBuilder, land: &Land, t: IVec2) {
    // Square slabs: a joint where this tile meets the next along the path.
    let colour = pal::SIDEWALK_JOINT.lin();
    let tv = t.as_vec2();
    if land.floor(t + IVec2::Y) == Floor::Path {
        paint.rect(Vec2::new(tv.x, tv.y + 0.95), Vec2::new(tv.x + 1.0, tv.y + 1.0), colour);
    }
    if land.floor(t + IVec2::X) == Floor::Path {
        paint.rect(Vec2::new(tv.x + 0.95, tv.y), Vec2::new(tv.x + 1.0, tv.y + 1.0), colour);
    }
    paint.draw(mb, pal::PATH.lin());
}

/// Grass or dirt: lawn patches, rough verges, long grass at the edges, the forest floor and its
/// litter.
fn soft_tile(paint: &mut Paint, mb: &mut MeshBuilder, props: &mut MeshBuilder, land: &Land, t: IVec2, dirt: bool) {
    let (x, y) = (t.x, t.y);
    let tile = land.tile(t);
    let (trees, pines) = land.trees_near(t, 2);
    let forest = trees >= 3;
    let verge = Side::ALL.iter().any(|s| land.is_road(t + s.offset()));
    let edge = land.has_fence(t) || Side::ALL.iter().any(|s| !land.outdoors(t + s.offset()));
    let near_dirt = !dirt && [IVec2::X, IVec2::NEG_X, IVec2::Y, IVec2::NEG_Y, IVec2::ONE, IVec2::NEG_ONE].iter().any(|&d| land.floor(t + d) == Floor::Dirt);
    let tv = t.as_vec2();
    let overgrown = noise(tv, 9.0, 41);
    // How unkempt this tile is, 0..1.
    // (Trees nearby, forest included: the shade keeps it short.)
    let rough: f32 = if trees > 0 {
        0.45
    } else if edge || verge {
        0.85
    } else if near_dirt || dirt {
        0.6
    } else {
        ((overgrown - 0.62) * 3.0).clamp(0.0, 0.8)
    };
    let ground_at = |c: Vec2| -> LinearRgba {
        if dirt {
            shade(pal::DIRT.lin(), 0.94 + 0.1 * noise(c, 2.5, 43))
        } else if forest {
            let n = noise(c, 4.0, 44) + 0.25 * hash01((c.x * 3.0) as i32, (c.y * 3.0) as i32, 45);
            if pines > 0 && n > 0.75 { pal::NEEDLES.lin() } else { pal::FOREST_FLOOR[(n > 0.6) as usize].lin() }
        } else {
            // Small blobs (not single triangles) where long grass meets the lawn.
            let fine = noise(c, 1.8, 46);
            if verge && fine < 0.7 {
                pal::VERGE.lin()
            } else if rough + 0.35 * fine > 0.85 {
                pal::LONG_GRASS.lin()
            } else {
                let n = noise(c, 7.0, 47) * 0.85 + noise(c, 2.5, 48) * 0.15;
                pal::LAWN[if n < 0.4 { 1 } else if n < 0.62 { 0 } else { 2 }].lin()
            }
        }
    };
    if paint.rects.is_empty() {
        split_tile(mb, t, ground_at);
    } else {
        // Under a driveway or a bed: one colour for the ground left showing.
        paint.draw(mb, ground_at(tv + Vec2::splat(0.5)));
    }
    // Tufts of long grass: many at the edges, a few weeds anywhere.
    let count = if rough < 0.55 { 0 } else { (rough * 2.2 + hash01(x, y, 50) - 0.9).floor() as i32 };
    for i in 0..count.min(3) {
        let p = Vec3::new(x as f32 + 0.15 + 0.7 * hash01(x, y, 60 + i as u32), 0.0, y as f32 + 0.15 + 0.7 * hash01(x, y, 70 + i as u32));
        let dry = hash01(x, y, 80 + i as u32) < 0.35;
        tuft(mb, p, 0.15 + 0.08 * hash01(x, y, 90 + i as u32), 0.16 + 0.16 * rough * hash01(x, y, 100 + i as u32), if dry { pal::TUFT_DRY } else { pal::TUFT }, hash01(x, y, 110 + i as u32));
    }
    if tile.feature != Feature::None {
        return;
    }
    let roll = hash01(x, y, 120);
    let at = Vec3::new(x as f32 + 0.25 + 0.5 * hash01(x, y, 121), 0.0, y as f32 + 0.25 + 0.5 * hash01(x, y, 122));
    if forest {
        // Forest litter.
        if roll < 0.24 {
            fern(mb, at, 0.45 + 0.25 * hash01(x, y, 123), hash01(x, y, 124));
        } else if roll < 0.29 {
            rock(mb, at, 0.18 + 0.2 * hash01(x, y, 125), hash01(x, y, 126));
        } else if roll < 0.32 {
            stump(mb, at, 0.18 + 0.1 * hash01(x, y, 127), hash01(x, y, 128));
        } else if roll < 0.34 {
            log(mb, at, 1.1 + 0.6 * hash01(x, y, 129), 0.13 + 0.06 * hash01(x, y, 130), hash01(x, y, 131));
        }
    } else if trees > 0 && !dirt && roll < 0.07 {
        // Scrub where town meets the trees.
        add_bush(props, x, y, at, 0.5 + 0.25 * hash01(x, y, 132));
    } else if dirt && roll < 0.05 {
        rock(mb, at, 0.12 + 0.1 * hash01(x, y, 125), hash01(x, y, 126));
    }
}

/// A tuft of long grass: a thin three-sided spike.
fn tuft(mb: &mut MeshBuilder, p: Vec3, r: f32, h: f32, colour: Swatch, k: f32) {
    let a0 = k * TAU;
    let lean = Vec3::new((k * 7.0).sin(), 0.0, (k * 5.0).cos()) * h * 0.25;
    let apex = p + Vec3::Y * h + lean;
    let base = |i: u32| {
        let a = a0 + i as f32 * TAU / 3.0;
        p + Vec3::new(a.cos(), 0.0, a.sin()) * r
    };
    let inside = p + Vec3::Y * h * 0.3;
    let c = colour.lin();
    for i in 0..3 {
        tri_out(mb, [base(i), base(i + 1), apex], inside, shade(c, 0.92 + 0.06 * i as f32));
    }
}

/// A low fern: four folded fronds arching out from the middle.
fn fern(mb: &mut MeshBuilder, p: Vec3, size: f32, k: f32) {
    let colour = pal::FERN[(k * 2.0) as usize % 2].lin();
    for i in 0..4 {
        let a = k * TAU + i as f32 * TAU / 4.0 + (hash01(i, (k * 977.0) as i32, 7) - 0.5) * 0.6;
        let dir = Vec3::new(a.cos(), 0.0, a.sin());
        let side = Vec3::new(-dir.z, 0.0, dir.x);
        let base = p + Vec3::Y * 0.04;
        let tip = p + dir * size + Vec3::Y * size * 0.1;
        let ridge = p + dir * size * 0.45 + Vec3::Y * size * 0.42;
        let (l, r) = (ridge + side * size * 0.2 - Vec3::Y * size * 0.12, ridge - side * size * 0.2 - Vec3::Y * size * 0.12);
        // Each frond is folded along its rib: two faces, lit differently.
        tri_up(mb, [base, l, tip], colour);
        tri_up(mb, [base, tip, r], shade(colour, 0.88));
    }
}

/// A grey rock: a low, lopsided six-sided pyramid.
fn rock(mb: &mut MeshBuilder, p: Vec3, r: f32, k: f32) {
    let colour = pal::ROCK.lin();
    let top = p + Vec3::new((k - 0.5) * r * 0.5, r * (0.5 + 0.3 * k), (0.5 - k) * r * 0.3);
    let ring = |i: u32| {
        let a = k * TAU + i as f32 * TAU / 6.0;
        let j = 0.75 + 0.4 * hash01(i as i32, (k * 1000.0) as i32, 5);
        p + Vec3::new(a.cos() * r * j, 0.0, a.sin() * r * j)
    };
    for i in 0..6 {
        tri_out(mb, [ring(i), ring(i + 1), top], p, shade(colour, 0.88 + 0.05 * (i % 3) as f32));
    }
}

/// A tree stump: a short five-sided trunk with a pale cut top.
fn stump(mb: &mut MeshBuilder, p: Vec3, r: f32, k: f32) {
    let h = 0.22 + 0.2 * k;
    let ring = |i: u32, y: f32, rr: f32| {
        let a = k * TAU + i as f32 * TAU / 5.0;
        p + Vec3::new(a.cos() * rr, y, a.sin() * rr)
    };
    let inside = p + Vec3::Y * h * 0.5;
    for i in 0..5 {
        tri_out(mb, [ring(i, 0.0, r * 1.15), ring(i + 1, 0.0, r * 1.15), ring(i + 1, h, r)], inside, pal::DEADWOOD.lin());
        tri_out(mb, [ring(i, 0.0, r * 1.15), ring(i + 1, h, r), ring(i, h, r)], inside, pal::DEADWOOD.lin());
        tri_up(mb, [p + Vec3::Y * h, ring(i, h, r), ring(i + 1, h, r)], pal::CUT_WOOD.lin());
    }
}

/// A fallen log: a six-sided tube lying on the ground with cut ends.
fn log(mb: &mut MeshBuilder, p: Vec3, len: f32, r: f32, k: f32) {
    let a = k * TAU;
    let dir = Vec3::new(a.cos(), 0.0, a.sin());
    let side = Vec3::new(-dir.z, 0.0, dir.x);
    let centre = p + Vec3::Y * r * 0.85;
    let ring = |i: u32, end: f32| {
        let ang = i as f32 * TAU / 6.0;
        centre + dir * end + (side * ang.cos() + Vec3::Y * ang.sin()) * r
    };
    let (e0, e1) = (-len * 0.5, len * 0.5);
    for i in 0..6 {
        tri_out(mb, [ring(i, e0), ring(i + 1, e0), ring(i + 1, e1)], centre, pal::DEADWOOD.lin());
        tri_out(mb, [ring(i, e0), ring(i + 1, e1), ring(i, e1)], centre, pal::DEADWOOD.lin());
    }
    for e in [e0, e1] {
        let mid = centre + dir * e;
        for i in 0..6 {
            tri_out(mb, [mid, ring(i, e), ring(i + 1, e)], centre, pal::CUT_WOOD.lin());
        }
    }
}

/// A run of fence: some runs (picked by where they start) are a trimmed hedge instead. Both block
/// walking and not sight, like the sim's fence.
fn fence_or_hedge(mb: &mut MeshBuilder, land: &Land, t: IVec2, which: OwnedEdge) {
    let step = if which == OwnedEdge::North { IVec2::X } else { IVec2::Y };
    let fenced = |p: IVec2| {
        let tile = land.tile(p);
        (if which == OwnedEdge::North { tile.north } else { tile.west }) == Edge::Fence
    };
    // Runs are followed to their ends (not just `RUN_CAP`), so a whole run decides the same.
    let walk = |d: IVec2| (1..256).take_while(|&k| fenced(t + d * k)).count() as i32;
    let back = walk(-step);
    let start = t - step * back;
    let ahead = walk(step);
    let len = back + ahead + 1;
    if len >= 4 && hash01(start.x, start.y, 77) < 0.4 {
        let (a, b) = TileMap::edge_segment(t, which);
        let h = 1.05 + 0.25 * hash01(start.x, start.y, 78);
        hedge(mb, a, b, h, back == 0, ahead == 0);
    } else {
        fence(mb, t, which);
    }
}

fn fence(mb: &mut MeshBuilder, t: IVec2, which: OwnedEdge) {
    let (a, b) = TileMap::edge_segment(t, which);
    let wood = pal::FENCE_WOOD.lin();
    let along = (b - a).normalize();
    let half = Vec2::new(along.y.abs(), along.x.abs()) * 0.03;
    for (h0, h1) in [(0.35, 0.45), (0.8, 0.9)] {
        let lo = a.min(b) - half;
        let hi = a.max(b) + half;
        mb.add_box(Vec3::new(lo.x, h0, lo.y), Vec3::new(hi.x, h1, hi.y), super::same(wood));
    }
    mb.add_box(Vec3::new(a.x - 0.05, 0.0, a.y - 0.05), Vec3::new(a.x + 0.05, 1.05, a.y + 0.05), super::same(shade(wood, 0.8)));
}

/// One metre of hedge along `a`–`b`: straight sides, chamfered top edges, a flat top, and an end
/// face where the run starts or stops.
fn hedge(mb: &mut MeshBuilder, a: Vec2, b: Vec2, h: f32, cap_a: bool, cap_b: bool) {
    let along = (b - a).normalize();
    let n = Vec2::new(-along.y, along.x);
    let w = 0.34;
    // Cross-section, from one foot over the top to the other.
    let section = [(-w, 0.0), (-w, h - 0.2), (-w * 0.55, h), (w * 0.55, h), (w, h - 0.2), (w, 0.0)];
    let at = |p: Vec2, (s, y): (f32, f32)| {
        let q = p + n * s;
        Vec3::new(q.x, y, q.y)
    };
    let mid = Vec3::new((a.x + b.x) * 0.5, h * 0.5, (a.y + b.y) * 0.5);
    let side = pal::HEDGE.lin();
    let top = pal::HEDGE_TOP.lin();
    for i in 0..section.len() - 1 {
        let colour = match i {
            0 | 4 => side,
            2 => top,
            _ => shade(top, 0.92),
        };
        let (p0, p1, p2, p3) = (at(a, section[i]), at(a, section[i + 1]), at(b, section[i + 1]), at(b, section[i]));
        tri_out(mb, [p0, p1, p2], mid, colour);
        tri_out(mb, [p0, p2, p3], mid, colour);
    }
    for (end, cap) in [(a, cap_a), (b, cap_b)] {
        if !cap {
            continue;
        }
        let pts: Vec<Vec3> = section.iter().map(|&s| at(end, s)).collect();
        let c = at(end, (0.0, h * 0.5));
        for i in 0..pts.len() {
            tri_out(mb, [c, pts[i], pts[(i + 1) % pts.len()]], mid, shade(side, 0.9));
        }
    }
}

/// A driveway's concrete on one tile, with a joint every 3 m.
fn driveway_paint(paint: &mut Paint, d: &Driveway) {
    paint.rect(d.min, d.max, pal::DRIVEWAY.lin());
    let joint = pal::SIDEWALK_JOINT.lin();
    let (u0, u1) = if d.along_x { (d.min.x, d.max.x) } else { (d.min.y, d.max.y) };
    let mut u = u0 + 3.0;
    while u < u1 - 0.5 {
        let (j0, j1) = if d.along_x { (Vec2::new(u - 0.025, d.min.y), Vec2::new(u + 0.025, d.max.y)) } else { (Vec2::new(d.min.x, u - 0.025), Vec2::new(d.max.x, u + 0.025)) };
        paint.rect(j0, j1, joint);
        u += 3.0;
    }
}

/// The oil stain where a car stood on a driveway, if it falls in this chunk.
fn driveway_stain(mb: &mut MeshBuilder, d: &Driveway, lo: Vec2, hi: Vec2) {
    let c = (d.min + d.max) * 0.5;
    let k = hash01(d.min.x as i32, d.min.y as i32, 140);
    let centre = c + Vec2::new(k - 0.5, hash01(d.min.x as i32, d.min.y as i32, 141) - 0.5) * 0.8;
    if centre.x >= lo.x && centre.y >= lo.y && centre.x < hi.x && centre.y < hi.y {
        stain(mb, centre, 0.8 + 0.5 * k, k);
    }
}

/// A dark, irregular oil stain on the ground.
fn stain(mb: &mut MeshBuilder, centre: Vec2, size: f32, k: f32) {
    let colour = pal::OIL_STAIN.lin();
    let r = Vec2::new(0.45, 0.32) * size;
    for i in 0..7 {
        let a0 = i as f32 * TAU / 7.0;
        let a1 = (i + 1) as f32 * TAU / 7.0;
        let j0 = 0.75 + 0.4 * hash01(i, (k * 999.0) as i32, 142);
        let j1 = 0.75 + 0.4 * hash01((i + 1) % 7, (k * 999.0) as i32, 142);
        let p = |a: f32, j: f32| Vec3::new(centre.x + a.cos() * r.x * j, Y_STAIN, centre.y + a.sin() * r.y * j);
        tri_up(mb, [Vec3::new(centre.x, Y_STAIN, centre.y), p(a0, j0), p(a1, j1)], colour);
    }
}

/// One tile of planting along a house front: turned soil with a few muted flowers, or a low shrub.
fn bed_tile(paint: &mut Paint, ground: &mut MeshBuilder, props: &mut MeshBuilder, b: &BedTile) {
    let tv = b.tile.as_vec2();
    let depth = 0.8;
    // The strip against the wall.
    let (min, max, into) = match b.wall {
        Side::N => (tv, tv + Vec2::new(1.0, depth), Vec2::Y),
        Side::S => (tv + Vec2::new(0.0, 1.0 - depth), tv + Vec2::ONE, Vec2::NEG_Y),
        Side::W => (tv, tv + Vec2::new(depth, 1.0), Vec2::X),
        Side::E => (tv + Vec2::new(1.0 - depth, 0.0), tv + Vec2::ONE, Vec2::NEG_X),
    };
    let (x, y) = (b.tile.x, b.tile.y);
    let mid = (min + max) * 0.5;
    match b.bed {
        Bed::Flowers => {
            paint.rect(min, max, pal::SOIL.lin());
            for i in 0..3 {
                let along = Vec2::new(into.y.abs(), into.x.abs());
                let p = mid + along * (-0.33 + 0.33 * i as f32 + (hash01(x, y, 150 + i) - 0.5) * 0.15) + into * (hash01(x, y, 160 + i) - 0.5) * 0.35;
                let colour = pal::FLOWERS[(hash01(x, y, 170) * 4.0) as usize % 4];
                let leaf = pal::SHRUB[0];
                let base = Vec3::new(p.x, 0.0, p.y);
                tuft(ground, base, 0.12, 0.22, leaf, hash01(x, y, 180 + i));
                let bloom = base + Vec3::Y * (0.2 + 0.08 * hash01(x, y, 190 + i));
                blossom(ground, bloom, 0.09, colour.lin(), hash01(x, y, 200 + i));
            }
        }
        Bed::Shrubs => {
            paint.rect(min, max, shade(pal::SOIL.lin(), 1.15));
            if (x + y).rem_euclid(2) == 0 {
                add_bush(props, x, y, Vec3::new(mid.x, 0.0, mid.y), 0.6 + 0.15 * hash01(x, y, 210));
            }
        }
    }
}

/// A flower head: a small faceted double pyramid.
fn blossom(mb: &mut MeshBuilder, c: Vec3, r: f32, colour: LinearRgba, k: f32) {
    let ring = |i: u32| {
        let a = k * TAU + i as f32 * TAU / 4.0;
        c + Vec3::new(a.cos() * r, 0.0, a.sin() * r)
    };
    let (top, bottom) = (c + Vec3::Y * r * 0.8, c - Vec3::Y * r * 0.6);
    for i in 0..4 {
        tri_out(mb, [ring(i), ring(i + 1), top], c, shade(colour, 0.95 + 0.05 * i as f32));
        tri_out(mb, [ring(i), bottom, ring(i + 1)], c, shade(colour, 0.8));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zombye_sim::worldgen::{GenConfig, generate};

    fn town() -> TileMap {
        let data = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/data");
        let templates = zombye_net::content::load_building_templates(&data).expect("templates");
        generate(&GenConfig::default(), &templates).map
    }

    #[test]
    fn streets_are_found_and_lots_are_not() {
        let map = town();
        let (mut solid, mut dashed, mut bays) = (0, 0, 0);
        for cy in 0..map.chunks_y {
            for cx in 0..map.chunks_x {
                let land = Land::new(&map, IVec2::new(cx, cy));
                for y in land.min.y..land.min.y + CHUNK_SIZE {
                    for x in land.min.x..land.min.x + CHUNK_SIZE {
                        let t = IVec2::new(x, y);
                        bays += land.is_bay(t) as u32;
                        if let Some(st) = land.street(t) {
                            assert!(st.hi - st.lo >= 5.0 && st.hi - st.lo <= MAX_STREET as f32, "{t} {st:?}");
                            match st.paint {
                                Line::Solid => solid += 1,
                                Line::Dashed => dashed += 1,
                            }
                        }
                    }
                }
            }
        }
        eprintln!("street tiles: solid {solid}, dashed {dashed}; bay tiles {bays}");
        assert!(solid > 1000 && dashed > 5000 && bays > 20);
    }

    #[test]
    fn chunks_build_the_same_every_time() {
        let map = town();
        let c = IVec2::new(map.chunks_x / 2, map.chunks_y / 2);
        let a = build_chunk(&map, c);
        let b = build_chunk(&map, c);
        assert_eq!(a.ground.tri_count(), b.ground.tri_count());
        assert_eq!(a.props.tri_count(), b.props.tri_count());
        assert!(a.ground.tri_count() >= 512);
    }
}
