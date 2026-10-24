//! Curves drawn over the tile grid (#82): turning circles as true circles, rounded kerb corners at
//! junctions, alley mouths and parking-lot entrances, and real bends where the roads out of town
//! dog-leg through the forest. The sim's tiles stay as they are (walking and sight follow them);
//! only the drawing changes.
//!
//! Alleys and parking lots meet the sidewalk in the tiles; ways through it to the street are drawn
//! (asphalt across the sidewalk, the kerb turning in), and driveways flare where they meet it.
//!
//! [`Curves::find`] works out the shapes near a chunk from the tile map and the generator's
//! [`TurningCircle`]s. Each shape is a list of [`Override`]s: convex regions where the ground is
//! drawn as another [`Surface`] than its tile's. The landscape cuts each tile an override reaches
//! into convex pieces ([`Curves::pieces`]): the tile's own look clipped to the pieces still its
//! own, plain asphalt, concrete or ground on the rest. Kerbs follow the curves ([`Kerb`]) and the
//! straight kerbs along tile edges are left out where a curve replaces them ([`Curves::kerb_span`]).
//! Everything is worked out from tiles within a fixed distance, so neighbouring chunks agree.

use std::f32::consts::{PI, TAU};

use bevy::prelude::*;
use zombye_sim::map::{CHUNK_SIZE, Edge, Feature, Floor, TileMap, TurningCircle};

/// A kerb corner's radius where both sidewalks run on at least 4 m, else [`SMALL_RADIUS`] (alley
/// mouths, parking-lot entrances: the sidewalk's own width is all the room there is).
pub const JUNCTION_RADIUS: f32 = 4.0;
const SMALL_RADIUS: f32 = 1.5;
/// The reverse curve where a cul-de-sac's street opens into its turning circle.
const FLARE_RADIUS: f32 = 4.0;
/// A dog-leg's inside radius (whole metres), made smaller where trees stand in the way.
const BEND_INNER: i32 = 6;
/// Facets round a turning circle (about 1.1 m of kerb each at 8.5 m), and about how long a facet
/// of any other curve is.
const CIRCLE_FACETS: usize = 48;
const FACET: f32 = 0.8;
/// Pieces smaller than this (m²) are dropped.
const MIN_AREA: f32 = 1e-5;

/// What the ground is drawn as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Surface {
    Road,
    Walk,
    /// Anything else outdoors: lawn, dirt, forest floor, paths.
    Soft,
    /// A driveway's concrete (only where a curve puts it: driveways are drawn over the lawn).
    Drive,
}

impl Surface {
    fn bit(self) -> u8 {
        1 << self as u8
    }

    fn of(floor: Floor) -> Self {
        if floor.is_road() {
            Surface::Road
        } else if floor == Floor::Sidewalk {
            Surface::Walk
        } else {
            Surface::Soft
        }
    }
}

const ANY: u8 = 0b1111;
const MAX_VERTS: usize = CIRCLE_FACETS + 4;

/// A convex polygon in world x/z (either winding).
#[derive(Clone, Copy, Debug)]
pub struct Poly {
    n: u8,
    v: [Vec2; MAX_VERTS],
}

impl Poly {
    pub fn new(points: &[Vec2]) -> Self {
        let mut p = Poly { n: 0, v: [Vec2::ZERO; MAX_VERTS] };
        for &q in points {
            p.push(q);
        }
        p
    }

    fn push(&mut self, q: Vec2) {
        if (self.n as usize) < MAX_VERTS {
            self.v[self.n as usize] = q;
            self.n += 1;
        }
    }

    pub fn rect(min: Vec2, max: Vec2) -> Self {
        Self::new(&[min, Vec2::new(max.x, min.y), max, Vec2::new(min.x, max.y)])
    }

    pub fn points(&self) -> &[Vec2] {
        &self.v[..self.n as usize]
    }

    fn signed_area(&self) -> f32 {
        // From the first corner, not the origin: world coordinates in the hundreds would leave
        // little of an f32 for a small piece's area.
        let p = self.points();
        (1..p.len().saturating_sub(1)).map(|i| (p[i] - p[0]).perp_dot(p[i + 1] - p[0])).sum::<f32>() * 0.5
    }

    pub fn area(&self) -> f32 {
        self.signed_area().abs()
    }

    fn bounds(&self) -> (Vec2, Vec2) {
        self.points().iter().fold((Vec2::MAX, Vec2::MIN), |(lo, hi), &p| (lo.min(p), hi.max(p)))
    }

    pub fn centroid(&self) -> Vec2 {
        self.points().iter().sum::<Vec2>() / self.n.max(1) as f32
    }

    /// The part on the side of the line through `a` that `n` points to.
    fn clip(&self, a: Vec2, n: Vec2) -> Poly {
        let mut out = Poly::new(&[]);
        let p = self.points();
        for i in 0..p.len() {
            let (s, e) = (p[i], p[(i + 1) % p.len()]);
            let (ds, de) = ((s - a).dot(n), (e - a).dot(n));
            if ds >= 0.0 {
                out.push(s);
            }
            if (ds >= 0.0) != (de >= 0.0) {
                out.push(s + (e - s) * (ds / (ds - de)));
            }
        }
        out
    }

    /// The part inside the convex `other`.
    pub fn intersect(&self, other: &Poly) -> Poly {
        let sign = other.signed_area().signum();
        let p = other.points();
        let mut out = *self;
        for i in 0..p.len() {
            out = out.clip(p[i], (p[(i + 1) % p.len()] - p[i]).perp() * sign);
        }
        out
    }

    pub fn clip_rect(&self, min: Vec2, max: Vec2) -> Poly {
        self.clip(min, Vec2::X).clip(min, Vec2::Y).clip(max, Vec2::NEG_X).clip(max, Vec2::NEG_Y)
    }

    pub fn contains(&self, q: Vec2) -> bool {
        let p = self.points();
        let sign = self.signed_area().signum();
        (0..p.len()).all(|i| (p[(i + 1) % p.len()] - p[i]).perp_dot(q - p[i]) * sign >= -1e-5)
    }
}

/// Cut `piece` by the convex `part`: what lies inside it (if anything), with the rest pushed to
/// `outside` as convex pieces.
fn split(piece: &Poly, part: &Poly, outside: &mut Vec<Poly>) -> Option<Poly> {
    let sign = part.signed_area().signum();
    let p = part.points();
    let mut rest = *piece;
    for i in 0..p.len() {
        let (a, b) = (p[i], p[(i + 1) % p.len()]);
        let n = (b - a).perp().normalize_or_zero() * sign;
        if n == Vec2::ZERO || rest.points().iter().all(|&q| (q - a).dot(n) >= -1e-5) {
            continue;
        }
        let out = rest.clip(a, -n);
        if out.area() > MIN_AREA {
            outside.push(out);
        }
        rest = rest.clip(a, n);
        if rest.area() <= MIN_AREA {
            return None;
        }
    }
    Some(rest)
}

/// A circle about `c` as a polygon, with a corner every [`CIRCLE_FACETS`]th of a turn from +x.
fn disc(c: Vec2, r: f32) -> Poly {
    let mut p = Poly::new(&[]);
    for k in 0..CIRCLE_FACETS {
        p.push(c + Vec2::from_angle(k as f32 * TAU / CIRCLE_FACETS as f32) * r);
    }
    p
}

/// `a` minus `b`, as an angle in (-π, π].
fn turn(a: f32, b: f32) -> f32 {
    let d = (a - b).rem_euclid(TAU);
    if d > PI { d - TAU } else { d }
}

/// Points along a circle about `c` from `from` to `to` (both on it), the shorter way round, about
/// [`FACET`] apart.
fn arc(c: Vec2, r: f32, from: Vec2, to: Vec2) -> Vec<Vec2> {
    let a0 = (from - c).to_angle();
    let d = turn((to - c).to_angle(), a0);
    let n = ((d.abs() * r / FACET).ceil() as usize).max(2);
    let mut pts: Vec<Vec2> = (0..=n).map(|i| c + Vec2::from_angle(a0 + d * i as f32 / n as f32) * r).collect();
    pts[0] = from;
    pts[n] = to;
    pts
}

/// The region between `apex` and an arc it sees whole, as triangles.
fn fan(apex: Vec2, arc: &[Vec2]) -> Vec<Poly> {
    arc.windows(2).map(|w| Poly::new(&[apex, w[0], w[1]])).filter(|p| p.area() > MIN_AREA).collect()
}

/// Ground drawn as `to` instead of what it was (any of the `from` surfaces) inside `parts`.
struct Override {
    parts: Vec<(Poly, Vec2, Vec2)>,
    to: Surface,
    from: u8,
    min: Vec2,
    max: Vec2,
}

impl Override {
    fn new(parts: Vec<Poly>, to: Surface, from: u8) -> Self {
        let parts: Vec<(Poly, Vec2, Vec2)> = parts
            .into_iter()
            .map(|p| {
                let (lo, hi) = p.bounds();
                (p, lo, hi)
            })
            .collect();
        let (min, max) = parts.iter().fold((Vec2::MAX, Vec2::MIN), |(lo, hi), p| (lo.min(p.1), hi.max(p.2)));
        Override { parts, to, from, min, max }
    }
}

/// Where a straight kerb along tile edges gives way to a curve.
enum Gap {
    /// Along this axis-aligned stretch.
    Line(Vec2, Vec2),
    /// Within this distance of a point.
    Disc(Vec2, f32),
}

/// A kerb along a curve: its line (the road's edge) and, at each point, which way the sidewalk is.
pub struct Kerb {
    pub points: Vec<Vec2>,
    pub walk: Vec<Vec2>,
}

/// A stretch of a circle: centre, radius, start angle and signed turn.
#[derive(Clone, Copy, Debug)]
pub struct ArcSpan {
    pub centre: Vec2,
    pub r: f32,
    pub a0: f32,
    pub turn: f32,
}

impl ArcSpan {
    pub fn at(&self, s: f32) -> Vec2 {
        self.centre + Vec2::from_angle(self.a0 + self.turn * s) * self.r
    }

    pub fn length(&self) -> f32 {
        self.turn.abs() * self.r
    }
}

/// One convex piece of a tile and what it's drawn as; `own` when that's still the tile's own look.
#[derive(Clone, Copy)]
pub struct Piece {
    pub poly: Poly,
    pub surface: Surface,
    pub own: bool,
    by: usize,
}

/// Room for [`Curves::pieces`] to work in.
#[derive(Default)]
pub struct PieceBuf {
    pub pieces: Vec<Piece>,
    tmp: Vec<Piece>,
    outside: Vec<Poly>,
}

#[derive(Default)]
pub struct Curves {
    overrides: Vec<Override>,
    gaps: Vec<Gap>,
    pub kerbs: Vec<Kerb>,
    /// Centre lines round the bends (dashed).
    pub dashes: Vec<ArcSpan>,
    /// Sidewalks round turning circles (slab joints across them): the arc of the kerb's back edge,
    /// and the sidewalk's outer radius.
    pub rings: Vec<(ArcSpan, f32)>,
    /// Rounded kerb corners: the corner of the tiles, and the radius.
    corners: Vec<(Vec2, f32)>,
    /// Sidewalk tiles drawn as asphalt: ways into parking lots.
    entrances: Vec<IVec2>,
    /// What the tiles round the chunk are drawn as.
    grid: Grid,
}

/// Tiles round a chunk this far out (its eight neighbours) are looked up once, in [`Grid`].
const GRID_MARGIN: i32 = CHUNK_SIZE;
const GRID_SIZE: i32 = CHUNK_SIZE + 2 * GRID_MARGIN;

/// The surfaces of the tiles round a chunk (0 indoors or off the map, else 1 + [`Surface`]).
#[derive(Default)]
struct Grid {
    lo: IVec2,
    cells: Vec<u8>,
}

impl Grid {
    /// The chunks round chunk `c`, read a chunk at a time.
    fn new(map: &TileMap, c: IVec2) -> Self {
        let lo = (c - IVec2::ONE) * CHUNK_SIZE;
        let mut cells = vec![0u8; (GRID_SIZE * GRID_SIZE) as usize];
        for cy in -1..=1 {
            for cx in -1..=1 {
                let Some(chunk) = map.chunk(c.x + cx, c.y + cy) else { continue };
                let base = ((cy + 1) * CHUNK_SIZE * GRID_SIZE + (cx + 1) * CHUNK_SIZE) as usize;
                for (i, tile) in chunk.tiles.iter().enumerate() {
                    let (x, y) = (i as i32 % CHUNK_SIZE, i as i32 / CHUNK_SIZE);
                    cells[base + (y * GRID_SIZE + x) as usize] = if tile.room == 0 { Surface::of(tile.floor) as u8 + 1 } else { 0 };
                }
            }
        }
        Grid { lo, cells }
    }

    fn index(&self, t: IVec2) -> Option<usize> {
        let l = t - self.lo;
        (l.x >= 0 && l.y >= 0 && l.x < GRID_SIZE && l.y < GRID_SIZE && !self.cells.is_empty()).then(|| (l.y * GRID_SIZE + l.x) as usize)
    }

    /// What `t` is drawn as, with `entrances` drawn as asphalt.
    fn get(&self, map: &TileMap, entrances: &[IVec2], t: IVec2) -> Option<Surface> {
        match self.index(t) {
            Some(i) => [None, Some(Surface::Road), Some(Surface::Walk), Some(Surface::Soft)][self.cells[i] as usize],
            None => drawn_surface(map, entrances, t),
        }
    }
}

/// The tile's surface, or None indoors or off the map.
fn sim_surface(map: &TileMap, t: IVec2) -> Option<Surface> {
    if !map.in_bounds(t) {
        return None;
    }
    let tile = map.tile(t);
    (tile.room == 0).then(|| Surface::of(tile.floor))
}

/// The tile's surface as drawn: sidewalk in a way into a lot (`entrances`) is asphalt.
fn drawn_surface(map: &TileMap, entrances: &[IVec2], t: IVec2) -> Option<Surface> {
    match sim_surface(map, t) {
        Some(Surface::Walk) if entrances.contains(&t) => Some(Surface::Road),
        s => s,
    }
}

/// The tile to the `o` side of a corner (`o` in -1..=0 each way), and from it the quadrant's
/// direction away from the corner (±1 each way).
const QUADRANTS: [IVec2; 4] = [IVec2::new(-1, -1), IVec2::new(0, -1), IVec2::new(-1, 0), IVec2::new(0, 0)];

fn away(o: IVec2) -> IVec2 {
    IVec2::new(if o.x == 0 { 1 } else { -1 }, if o.y == 0 { 1 } else { -1 })
}

impl Curves {
    /// The curves that reach chunk `c`.
    pub fn find(map: &TileMap, c: IVec2) -> Self {
        let lo = c * CHUNK_SIZE;
        let hi = lo + IVec2::splat(CHUNK_SIZE);
        let mut cv = Curves { grid: Grid::new(map, c), ..Default::default() };
        for tc in &map.turning_circles {
            let reach = Vec2::splat(tc.walk + 2.0);
            if (tc.centre + reach).cmpgt(lo.as_vec2()).all() && (tc.centre - reach).cmplt(hi.as_vec2()).all() {
                cv.turning_circle(tc);
            }
        }
        cv.find_entrances(map, lo - IVec2::splat(11), hi + IVec2::splat(11));
        // Corners of tiles with one sidewalk and three road tiles round them (within reach of a
        // kerb corner's curve), or one road and three of open ground (a bend's), read straight
        // off the grid.
        let (mut corners, mut bends) = (Vec::new(), Vec::new());
        let (g, w) = (&cv.grid, GRID_SIZE as usize);
        for y in 1..GRID_SIZE {
            for x in 1..GRID_SIZE {
                let i = (y * GRID_SIZE + x) as usize;
                let cells = [g.cells[i - w - 1], g.cells[i - w], g.cells[i - 1], g.cells[i]];
                let count = |s: Surface| cells.iter().filter(|&&c| c == s as u8 + 1).count();
                let v = g.lo + IVec2::new(x, y);
                let roads = count(Surface::Road);
                if roads == 3 && count(Surface::Walk) == 1 && v.cmpge(lo - IVec2::splat(5)).all() && v.cmple(hi + IVec2::splat(5)).all() {
                    corners.push(v);
                } else if roads == 1 && count(Surface::Soft) == 3 {
                    bends.push(v);
                }
            }
        }
        for v in corners {
            cv.kerb_corner(map, v);
        }
        for v in bends {
            cv.bend(map, v);
        }
        cv
    }

    /// What tile `t` is drawn as (None indoors or off the map).
    pub fn surface(&self, map: &TileMap, t: IVec2) -> Option<Surface> {
        self.grid.get(map, &self.entrances, t)
    }

    /// Ways through the sidewalk between a street and an alley's end or a parking lot: where the
    /// alley meets it, or where a lot's aisle does (4 m or more wide, running 6 m or more into the
    /// lot with no paint), up to 6 m of that sidewalk (in the middle) is drawn as asphalt, the kerb
    /// turning in at both sides. The tiles have none.
    fn find_entrances(&mut self, map: &TileMap, lo: IVec2, hi: IVec2) {
        let surf = |t: IVec2| self.grid.get(map, &[], t);
        let mut entrances = Vec::new();
        let paint = |t: IVec2| map.in_bounds(t) && map.tile(t).floor == Floor::RoadLine;
        let run = |t: IVec2, d: IVec2| (1..=20).take_while(|&k| surf(t + d * k) == Some(Surface::Road)).count() as i32;
        let mut done: Vec<(IVec2, IVec2)> = Vec::new();
        for y in lo.y..hi.y {
            for x in lo.x..hi.x {
                let s = IVec2::new(x, y);
                if surf(s) != Some(Surface::Walk) {
                    continue;
                }
                // `d` points from the street across the sidewalk into the lot; `e` along it.
                for d in [IVec2::X, IVec2::NEG_X, IVec2::Y, IVec2::NEG_Y] {
                    let e = IVec2::new(d.y.abs(), d.x.abs());
                    let open = |s: IVec2| -> Option<i32> {
                        if surf(s) != Some(Surface::Walk) || surf(s - d) != Some(Surface::Road) {
                            return None;
                        }
                        let street = run(s - d, -d) + 1;
                        let k = (1..=4).find(|&k| surf(s + d * k) != Some(Surface::Walk))?;
                        let lot = s + d * k;
                        if !(4..=14).contains(&street) || surf(lot) != Some(Surface::Road) {
                            return None;
                        }
                        let (deep, wide) = (run(lot, d) + 1, run(lot, e) + run(lot, -e) + 1);
                        let alley = deep >= 6 && wide <= 6;
                        let aisle = deep >= 10 && wide >= 10 && (0..6).all(|i| !paint(lot + d * i));
                        (alley || aisle).then_some(k)
                    };
                    let Some(k) = open(s) else { continue };
                    let mut start = s;
                    while (s - start).abs().max_element() < 64 && open(start - e) == Some(k) {
                        start -= e;
                    }
                    if done.contains(&(start, d)) {
                        continue;
                    }
                    done.push((start, d));
                    let len = (0..64).take_while(|&i| open(start + e * i) == Some(k)).count() as i32;
                    if len < 4 {
                        continue;
                    }
                    let w = len.min(6);
                    for i in (len - w) / 2..(len - w) / 2 + w {
                        for j in 0..k {
                            entrances.push(start + e * i + d * j);
                        }
                    }
                }
            }
        }
        for &t in &entrances {
            if let Some(i) = self.grid.index(t) {
                self.grid.cells[i] = Surface::Road as u8 + 1;
            }
        }
        self.entrances = entrances;
    }

    fn near_turning_circle(map: &TileMap, p: Vec2) -> bool {
        map.turning_circles.iter().any(|tc| p.distance(tc.centre) < tc.walk + 2.0)
    }

    /// A driveway widening where it meets the sidewalk: the lawn's corner at `corner` (where the
    /// driveway's side meets the back of the sidewalk; `q` points away from both, into the lawn)
    /// rounded off in concrete, where nothing grows there.
    pub fn driveway_flare(&mut self, map: &TileMap, corner: Vec2, q: Vec2) {
        let r = SMALL_RADIUS;
        if Self::near_turning_circle(map, corner) {
            return;
        }
        let (lo, hi) = (corner.min(corner + q * r), corner.max(corner + q * r));
        for y in lo.y.floor() as i32..hi.y.ceil() as i32 {
            for x in lo.x.floor() as i32..hi.x.ceil() as i32 {
                let tile = map.tile(IVec2::new(x, y));
                if !map.in_bounds(IVec2::new(x, y)) || tile.room != 0 || tile.floor != Floor::Grass || tile.feature != Feature::None {
                    return;
                }
            }
        }
        let centre = corner + q * r;
        let pts = arc(centre, r, corner + Vec2::new(q.x, 0.0) * r, corner + Vec2::new(0.0, q.y) * r);
        self.overrides.push(Override::new(fan(corner, &pts), Surface::Drive, Surface::Soft.bit()));
    }

    /// A corner of sidewalk with road on its three other sides (a junction, an alley mouth, a way
    /// into a lot): the kerb rounds it, and on a wide corner the back of the sidewalk follows.
    fn kerb_corner(&mut self, map: &TileMap, v: IVec2) {
        let (grid, entrances) = (&self.grid, &self.entrances);
        let drawn = |t: IVec2| grid.get(map, entrances, t);
        let (mut roads, mut walk) = (0, None);
        for o in QUADRANTS {
            match drawn(v + o) {
                Some(Surface::Road) => roads += 1,
                Some(Surface::Walk) => walk = Some(o),
                _ => {}
            }
        }
        let (3, Some(o)) = (roads, walk) else { return };
        if Self::near_turning_circle(map, v.as_vec2()) {
            return;
        }
        let q = away(o);
        let w = v + o;
        let (qx, qy) = (IVec2::new(q.x, 0), IVec2::new(0, q.y));
        // How far each kerb runs on straight from the corner, and the room that leaves the curve:
        // half of it where the sidewalk ends in road there too (the end of a strip between a
        // street and a lot), so the corners at both ends meet.
        let arm = |along: IVec2, across: IVec2| {
            let n = (0..6).take_while(|&i| drawn(w + along * i) == Some(Surface::Walk) && drawn(w + along * i + across) == Some(Surface::Road)).count() as i32;
            let room = if drawn(w + along * n) == Some(Surface::Road) { n as f32 * 0.5 } else { n as f32 };
            (n, room)
        };
        let ((nx, room_x), (ny, room_y)) = (arm(qx, -qy), arm(qy, -qx));
        let r = if nx.min(ny) >= 4 { JUNCTION_RADIUS } else { SMALL_RADIUS.min(room_x).min(room_y) };
        let (vf, qf) = (v.as_vec2(), q.as_vec2());
        let (tx, ty, centre) = (vf + Vec2::new(qf.x, 0.0) * r, vf + Vec2::new(0.0, qf.y) * r, vf + qf * r);
        let pts = arc(centre, r, tx, ty);
        self.overrides.push(Override::new(fan(vf, &pts), Surface::Road, Surface::Walk.bit()));
        self.gaps.push(Gap::Line(vf, tx));
        self.gaps.push(Gap::Line(vf, ty));
        let walk_dirs = pts.iter().map(|&p| (centre - p).normalize()).collect();
        self.kerbs.push(Kerb { points: pts, walk: walk_dirs });
        self.corners.push((vf, r));
        if r < JUNCTION_RADIUS {
            return;
        }
        // The back of the sidewalk: as wide all the way round, where it meets open lawn.
        let width = |at: IVec2, step: IVec2| (0..5).take_while(|&i| drawn(at + step * i) == Some(Surface::Walk)).count() as i32;
        let (wx, wy) = (width(w + qx * (nx - 1), qy), width(w + qy * (ny - 1), qx));
        let rb = r - wx as f32;
        if wx != wy || rb < 1.0 {
            return;
        }
        let vb = vf + qf * wx as f32;
        let corner = v + q * wx + o;
        let lawn = |t: IVec2| {
            let tile = map.tile(t);
            let fenced = tile.north == Edge::Fence || tile.west == Edge::Fence || map.tile(t + IVec2::X).west == Edge::Fence || map.tile(t + IVec2::Y).north == Edge::Fence;
            map.in_bounds(t) && tile.room == 0 && matches!(tile.floor, Floor::Grass | Floor::Dirt) && tile.feature == Feature::None && !fenced
        };
        let span = rb.ceil() as i32;
        if (0..span).any(|i| (0..span).any(|j| !lawn(corner + IVec2::new(q.x * i, q.y * j)))) {
            return;
        }
        let back = arc(centre, rb, vb + Vec2::new(qf.x, 0.0) * rb, vb + Vec2::new(0.0, qf.y) * rb);
        self.overrides.push(Override::new(fan(vb, &back), Surface::Walk, Surface::Soft.bit()));
    }

    /// A road with no sidewalks turning a right angle through open ground (the roads out of
    /// town): drawn as a bend of constant width, round the inside corner where no tree stands in
    /// the way, with its centre line following.
    fn bend(&mut self, map: &TileMap, v: IVec2) {
        let (grid, entrances) = (&self.grid, &self.entrances);
        let surf = |t: IVec2| grid.get(map, entrances, t);
        let (mut roads, mut softs, mut road) = (0, 0, None);
        for o in QUADRANTS {
            match surf(v + o) {
                Some(Surface::Road) => {
                    roads += 1;
                    road = Some(o);
                }
                Some(Surface::Soft) => softs += 1,
                _ => {}
            }
        }
        let (1, 3, Some(o)) = (roads, softs, road) else { return };
        let q = away(o);
        let t = v + o;
        let (qx, qy) = (IVec2::new(q.x, 0), IVec2::new(0, q.y));
        let arm = |along: IVec2, across: IVec2| (0..24).take_while(|&i| surf(t + along * i) == Some(Surface::Road) && surf(t + along * i + across) == Some(Surface::Soft)).count() as i32;
        let (nx, ny) = (arm(qx, -qy), arm(qy, -qx));
        if nx.min(ny) < 12 {
            return;
        }
        let width = |at: IVec2, step: IVec2| (0..12).take_while(|&i| surf(at + step * i) == Some(Surface::Road)).count() as i32;
        let w = width(t + qx * (nx - 1), qy);
        // (At most 9 m wide, so the bend reaches at most 15 m from its corner: within the grid.)
        if w != width(t + qy * (ny - 1), qx) || !(4..=9).contains(&w) {
            return;
        }
        // The inside corner: three road tiles and the open ground beyond, facing the same way.
        let vi = v + q * w;
        let inner: Vec<Option<Surface>> = QUADRANTS.iter().map(|&p| surf(vi + p)).collect();
        if QUADRANTS.iter().zip(&inner).any(|(&p, &s)| s != Some(if p == o { Surface::Soft } else { Surface::Road })) {
            return;
        }
        let clear = |r: i32| {
            let c = (vi + q * r).as_vec2();
            (0..r).all(|i| {
                (0..r).all(|j| {
                    let tile_lo = (vi + o + IVec2::new(q.x * i, q.y * j)).as_vec2();
                    let far = [Vec2::ZERO, Vec2::X, Vec2::Y, Vec2::ONE].iter().map(|&k| (tile_lo + k).distance(c)).fold(0.0, f32::max);
                    let tile = map.tile(vi + o + IVec2::new(q.x * i, q.y * j));
                    far <= r as f32 + 1e-3 || (surf(vi + o + IVec2::new(q.x * i, q.y * j)) == Some(Surface::Soft) && tile.feature == Feature::None)
                })
            })
        };
        let r_in = (1..=BEND_INNER).rev().find(|&r| nx >= w + r && ny >= w + r && clear(r)).unwrap_or(0);
        let big = (w + r_in) as f32;
        let (vf, qf) = (v.as_vec2(), q.as_vec2());
        let centre = vf + qf * big;
        // All of the bend is plain asphalt (no straight wheel tracks or centre line through it).
        self.overrides.push(Override::new(vec![Poly::rect(vf.min(centre), vf.max(centre))], Surface::Road, Surface::Road.bit()));
        let outer = arc(centre, big, vf + Vec2::new(qf.x, 0.0) * big, vf + Vec2::new(0.0, qf.y) * big);
        self.overrides.push(Override::new(fan(vf, &outer), Surface::Soft, Surface::Road.bit()));
        if r_in > 0 {
            let (vif, r) = (vi.as_vec2(), r_in as f32);
            let inside = arc(centre, r, vif + Vec2::new(qf.x, 0.0) * r, vif + Vec2::new(0.0, qf.y) * r);
            self.overrides.push(Override::new(fan(vif, &inside), Surface::Road, Surface::Soft.bit()));
        }
        let a0 = Vec2::new(0.0, -qf.y).to_angle();
        self.dashes.push(ArcSpan { centre, r: r_in as f32 + w as f32 * 0.5, a0, turn: turn(Vec2::new(-qf.x, 0.0).to_angle(), a0) });
    }

    /// A cul-de-sac's turning circle drawn round: asphalt, kerb and a sidewalk ring of constant
    /// width, opening into its street through a reverse curve each side.
    fn turning_circle(&mut self, tc: &TurningCircle) {
        let c = tc.centre;
        let a = tc.entry.offset().as_vec2();
        let b = a.perp();
        let zone = tc.walk + 1.2;
        let band = |half: f32| Poly::new(&[c - b * half, c - b * half + a * (zone + 0.5), c + b * half + a * (zone + 0.5), c + b * half]);
        // Everything the staircase of tiles touched starts as plain ground.
        self.overrides.push(Override::new(vec![disc(c, zone)], Surface::Soft, Surface::Road.bit() | Surface::Walk.bit()));
        self.overrides.push(Override::new(vec![disc(c, tc.walk), band(tc.stem_walk)], Surface::Walk, ANY));
        let h = ((tc.road + FLARE_RADIUS).powi(2) - (tc.stem + FLARE_RADIUS).powi(2)).sqrt();
        let back = FLARE_RADIUS - (tc.stem_walk - tc.stem);
        let (mut road_flares, mut walk_flares) = (Vec::new(), Vec::new());
        let mut sides: [Vec<Vec2>; 2] = Default::default();
        let mut ends = [Vec2::ZERO; 2];
        for (i, s) in [-1.0f32, 1.0].into_iter().enumerate() {
            let f = c + b * s * (tc.stem + FLARE_RADIUS) + a * h;
            let out = (f - c).normalize();
            let on_line = |half: f32, along: f32| c + b * s * half + a * along;
            let pts = arc(f, FLARE_RADIUS, on_line(tc.stem, h), c + out * tc.road);
            road_flares.extend(fan(on_line(tc.stem, (tc.road * tc.road - tc.stem * tc.stem).sqrt()), &pts));
            self.gaps.push(Gap::Line(on_line(tc.stem, 0.0), on_line(tc.stem, h)));
            if back > 0.1 {
                let pts = arc(f, back, on_line(tc.stem_walk, h), c + out * tc.walk);
                walk_flares.extend(fan(on_line(tc.stem_walk, (tc.walk * tc.walk - tc.stem_walk * tc.stem_walk).sqrt()), &pts));
            }
            ends[i] = c + out * tc.road;
            sides[i] = pts;
        }
        self.overrides.push(Override::new(walk_flares, Surface::Walk, ANY));
        let mut road = vec![disc(c, tc.road), band(tc.stem)];
        road.extend(road_flares);
        self.overrides.push(Override::new(road, Surface::Road, ANY));
        self.gaps.push(Gap::Disc(c, tc.road + 1.5));
        // The kerb: in along one flare, round the circle the long way (through the polygon's own
        // corners), and out along the other.
        let (a_first, a_last) = ((ends[0] - c).to_angle(), (ends[1] - c).to_angle());
        let short = turn(a_last, a_first);
        let long = short - short.signum() * TAU;
        let step = TAU / CIRCLE_FACETS as f32;
        let mut points = sides[0].clone();
        let mut walk: Vec<Vec2> = points.iter().map(|&p| (c + b * -(tc.stem + FLARE_RADIUS) + a * h - p).normalize()).collect();
        let k0 = if long > 0.0 { (a_first / step).floor() as i32 + 1 } else { (a_first / step).ceil() as i32 - 1 };
        let mut k = k0;
        while (k as f32 * step - a_first) * long.signum() < long.abs() - 1e-3 {
            let p = c + Vec2::from_angle(k as f32 * step) * tc.road;
            if (k as f32 * step - a_first).abs() > 1e-3 {
                points.push(p);
                walk.push((p - c).normalize());
            }
            k += long.signum() as i32;
        }
        let f1 = c + b * (tc.stem + FLARE_RADIUS) + a * h;
        for &p in sides[1].iter().rev() {
            points.push(p);
            walk.push((f1 - p).normalize());
        }
        self.kerbs.push(Kerb { points, walk });
        self.rings.push((ArcSpan { centre: c, r: tc.road + 0.16, a0: a_first, turn: long }, tc.walk));
    }

    /// Cut tile `t` (drawn as `own`) by the curves over it into `buf.pieces`. False when none
    /// touch it: draw it as usual.
    pub fn pieces(&self, t: IVec2, own: Surface, buf: &mut PieceBuf) -> bool {
        if self.overrides.is_empty() {
            return false;
        }
        let (lo, hi) = (t.as_vec2(), t.as_vec2() + Vec2::ONE);
        buf.pieces.clear();
        buf.pieces.push(Piece { poly: Poly::rect(lo, hi), surface: own, own: true, by: usize::MAX });
        let mut touched = false;
        let overlaps = |min: Vec2, max: Vec2| max.x > lo.x && min.x < hi.x && max.y > lo.y && min.y < hi.y;
        for (k, ov) in self.overrides.iter().enumerate() {
            if !overlaps(ov.min, ov.max) {
                continue;
            }
            for (part, pmin, pmax) in &ov.parts {
                if !overlaps(*pmin, *pmax) {
                    continue;
                }
                buf.tmp.clear();
                for piece in buf.pieces.drain(..) {
                    if piece.by == k || ov.from & piece.surface.bit() == 0 {
                        buf.tmp.push(piece);
                        continue;
                    }
                    buf.outside.clear();
                    let inside = split(&piece.poly, part, &mut buf.outside);
                    buf.tmp.extend(buf.outside.drain(..).map(|poly| Piece { poly, ..piece }));
                    if let Some(poly) = inside {
                        touched = true;
                        buf.tmp.push(Piece { poly, surface: ov.to, own: false, by: k });
                    }
                }
                std::mem::swap(&mut buf.pieces, &mut buf.tmp);
            }
        }
        touched
    }

    /// The part (as fractions from `a` to `b`) of the tile edge `a`–`b` where a straight kerb still
    /// runs, or None where a curve has taken all of it.
    pub fn kerb_span(&self, a: Vec2, b: Vec2) -> Option<(f32, f32)> {
        if self.gaps.is_empty() {
            return Some((0.0, 1.0));
        }
        let d = b - a;
        // What's left of 0..1, as a few stretches.
        let mut free = [(0.0f32, 1.0f32); 8];
        let mut n = 1;
        for g in &self.gaps {
            let (s, e) = match *g {
                Gap::Line(p, q) => {
                    // Only a stretch on the same line.
                    let (axis, fixed) = if d.x.abs() > d.y.abs() { (0, 1) } else { (1, 0) };
                    if (p[fixed] - a[fixed]).abs() > 1e-3 || (q[fixed] - a[fixed]).abs() > 1e-3 {
                        continue;
                    }
                    let (s, e) = ((p[axis] - a[axis]) / d[axis], (q[axis] - a[axis]) / d[axis]);
                    (s.min(e), s.max(e))
                }
                Gap::Disc(c, r) => {
                    // |a + s d - c|² = r²
                    let f = a - c;
                    let (qa, qb, qc) = (d.dot(d), 2.0 * f.dot(d), f.dot(f) - r * r);
                    let disc = qb * qb - 4.0 * qa * qc;
                    if disc <= 0.0 {
                        continue;
                    }
                    let root = disc.sqrt();
                    ((-qb - root) / (2.0 * qa), (-qb + root) / (2.0 * qa))
                }
            };
            if e <= 0.0 || s >= 1.0 {
                continue;
            }
            let mut next = [(0.0f32, 0.0f32); 8];
            let mut m = 0;
            for &(f0, f1) in &free[..n] {
                for piece in [(f0, f1.min(s)), (f0.max(e), f1)] {
                    if piece.1 - piece.0 > 1e-4 && m < next.len() {
                        next[m] = piece;
                        m += 1;
                    }
                }
            }
            (free, n) = (next, m);
        }
        // The longest stretch left.
        free[..n].iter().copied().filter(|(f0, f1)| f1 - f0 > 0.02).max_by(|x, y| (x.1 - x.0).total_cmp(&(y.1 - y.0)))
    }

    /// The radius of the rounded kerb corner at `v` (a tile corner), if there is one.
    pub fn corner_radius(&self, v: Vec2) -> Option<f32> {
        self.corners.iter().find(|(p, _)| p.distance_squared(v) < 1e-4).map(|c| c.1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splitting_a_square_by_a_disc_keeps_its_area() {
        let square = Poly::rect(Vec2::new(8.0, 0.0), Vec2::new(9.0, 1.0));
        let part = disc(Vec2::ZERO, 8.5);
        let mut outside = Vec::new();
        let inside = split(&square, &part, &mut outside).expect("part of it is inside");
        let total = inside.area() + outside.iter().map(|p| p.area()).sum::<f32>();
        assert!((total - 1.0).abs() < 1e-4, "{total}");
        assert!(inside.area() > 0.3 && inside.area() < 0.7, "{}", inside.area());
        assert!(outside.iter().all(|p| p.points().iter().all(|q| q.length() >= 8.45)));
    }

    #[test]
    fn a_kerb_corner_gap_shortens_the_straight_kerb() {
        let mut cv = Curves::default();
        cv.gaps.push(Gap::Line(Vec2::new(10.0, 5.0), Vec2::new(14.0, 5.0)));
        assert_eq!(cv.kerb_span(Vec2::new(12.0, 5.0), Vec2::new(13.0, 5.0)), None);
        let s = cv.kerb_span(Vec2::new(13.5, 5.0), Vec2::new(14.5, 5.0)).unwrap();
        assert!((s.0 - 0.5).abs() < 1e-5 && s.1 == 1.0, "{s:?}");
        assert_eq!(cv.kerb_span(Vec2::new(13.0, 6.0), Vec2::new(14.0, 6.0)), Some((0.0, 1.0)));
    }
}
