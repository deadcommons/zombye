//! Flow fields: one shortest-path field toward a set of goals, shared by every zombie heading there.
//! With several goals each tile points toward its nearest goal by path distance.
//!
//! This is what makes hundreds of zombies cheap: the cost is one Dijkstra over the map per goal,
//! not one A* per zombie. 8-way movement that respects wall edges and never cuts corners.
//!
//! Zombie fields can also lead *through* shut doors and intact windows (at an extra cost, as if the
//! way were that much longer): that's how a horde finds the way into a house, and why it ends up
//! battering the nearest door or window. Climbing through an open window costs a little extra too.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use glam::{IVec2, Vec2};

use crate::map::{Edge, Side, TileMap};

/// Cost of a straight step and a diagonal step (×10 fixed point).
const STRAIGHT: u32 = 10;
const DIAGONAL: u32 = 14;
pub const UNREACHABLE: u32 = u32::MAX;
/// Extra path cost (same fixed point) of climbing through an open or broken window, and of
/// battering through an intact window or a shut door. Our own values: a window is quicker to get
/// through than a door, so zombies prefer one that isn't much farther away.
pub const CLIMB_COST: u32 = 20;
pub const WINDOW_BREACH_COST: u32 = 60;
pub const DOOR_BREACH_COST: u32 = 140;

/// The 8 step directions as unit vectors, indexed by [`step_index`]; index 8 means "no step".
const STEPS: [Vec2; 9] = {
    const D: f32 = std::f32::consts::FRAC_1_SQRT_2;
    [
        Vec2::new(1.0, 0.0),
        Vec2::new(D, D),
        Vec2::new(0.0, 1.0),
        Vec2::new(-D, D),
        Vec2::new(-1.0, 0.0),
        Vec2::new(-D, -D),
        Vec2::new(0.0, -1.0),
        Vec2::new(D, -D),
        Vec2::ZERO,
    ]
};
const NO_STEP: u8 = 8;

#[derive(Clone, Debug, Default)]
pub struct FlowField {
    /// Goal tiles, in the order given (cost 0).
    pub goals: Vec<IVec2>,
    /// The window of the map this field covers: `origin .. origin + (width, height)`.
    origin: IVec2,
    width: i32,
    height: i32,
    /// Path cost to the goal in tenths of a metre, or [`UNREACHABLE`].
    cost: Vec<u32>,
    /// Next step on the shortest path, as an index into [`STEPS`] ([`NO_STEP`] at the goal or
    /// where unreachable). Recorded during the search, so building needs no second pass.
    dir: Vec<u8>,
    heap: BinaryHeap<Reverse<(u32, i32, i32)>>,
}

/// The 8 one-tile steps in [`STEPS`] order.
const OFFSETS: [IVec2; 8] = [
    IVec2::new(1, 0),
    IVec2::new(1, 1),
    IVec2::new(0, 1),
    IVec2::new(-1, 1),
    IVec2::new(-1, 0),
    IVec2::new(-1, -1),
    IVec2::new(0, -1),
    IVec2::new(1, -1),
];

/// Which of the 8 steps out of each tile are walkable (bit `k` = step `k` in [`STEPS`] order),
/// precomputed from the map so flow-field searches read one byte per tile instead of asking the
/// map about edges and neighbours. Update it when the map changes (doors, windows).
///
/// Alongside, per tile, which of the 4 straight steps climb through an open window, or would
/// breach a shut window or door (see [`breach_side`](Self::breach_side)).
#[derive(Clone, Debug, Default)]
pub struct WalkGrid {
    width: i32,
    height: i32,
    mask: Vec<u8>,
    /// Bits 0-3: climbing step (straight step `k` at bit `k / 2`); 4-7: through an intact window;
    /// 8-11: through a shut door.
    extra: Vec<u16>,
}

const CLIMB_BITS: u32 = 0;
const WINDOW_BITS: u32 = 4;
const DOOR_BITS: u32 = 8;

/// The side of a tile a straight step `k` (even, in [`STEPS`] order) leaves through.
fn step_side(k: usize) -> Side {
    match k {
        0 => Side::E,
        2 => Side::S,
        4 => Side::W,
        _ => Side::N,
    }
}

impl WalkGrid {
    pub fn new(map: &TileMap) -> Self {
        let n = (map.width() * map.height()) as usize;
        let mut g = Self { width: map.width(), height: map.height(), mask: vec![0; n], extra: vec![0; n] };
        for y in 0..g.height {
            for x in 0..g.width {
                g.refresh(map, IVec2::new(x, y));
            }
        }
        g
    }

    pub fn in_bounds(&self, t: IVec2) -> bool {
        t.x >= 0 && t.y >= 0 && t.x < self.width && t.y < self.height
    }

    fn refresh(&mut self, map: &TileMap, t: IVec2) {
        let mut m = 0u8;
        let mut extra = 0u16;
        for (k, d) in OFFSETS.iter().enumerate() {
            let ok = if d.x == 0 || d.y == 0 {
                let side = step_side(k);
                let edge = map.edge(t, side);
                let open_beyond = !map.tile_blocked(t + *d);
                let bit = (k / 2) as u32;
                if open_beyond && edge.is_climbable() {
                    extra |= 1 << (CLIMB_BITS + bit);
                } else if open_beyond && edge == Edge::Window {
                    extra |= 1 << (WINDOW_BITS + bit);
                } else if open_beyond && edge == Edge::DoorClosed {
                    extra |= 1 << (DOOR_BITS + bit);
                }
                map.can_step(t, side)
            } else {
                // No cutting through a window frame diagonally: climbing is a straight step.
                let (sx, sy) = (if d.x > 0 { Side::E } else { Side::W }, if d.y > 0 { Side::S } else { Side::N });
                let climbs = [map.edge(t, sx), map.edge(t, sy), map.edge(t + sx.offset(), sy), map.edge(t + sy.offset(), sx)].iter().any(|e| e.is_climbable());
                !climbs && map.can_step_diagonal(t, d.x, d.y)
            };
            m |= (ok as u8) << k;
        }
        let i = (t.y * self.width + t.x) as usize;
        self.mask[i] = m;
        self.extra[i] = extra;
    }

    /// Recompute the tiles whose steps an edit at tile `t` (its edges or contents) can affect.
    pub fn update_around(&mut self, map: &TileMap, t: IVec2) {
        for y in t.y - 2..=t.y + 2 {
            for x in t.x - 2..=t.x + 2 {
                if self.in_bounds(IVec2::new(x, y)) {
                    self.refresh(map, IVec2::new(x, y));
                }
            }
        }
    }

    fn steps(&self, t: IVec2) -> u8 {
        if self.in_bounds(t) { self.mask[(t.y * self.width + t.x) as usize] } else { 0 }
    }

    fn extra(&self, t: IVec2) -> u16 {
        if self.in_bounds(t) { self.extra[(t.y * self.width + t.x) as usize] } else { 0 }
    }

    /// Extra cost of straight step `k` out of `t` beyond a plain step, or `None` if it isn't
    /// possible. With `breach` off, shut doors and windows are walls.
    fn straight_extra(&self, extra: u16, k: usize, breach: bool) -> Option<u32> {
        let bit = (k / 2) as u32;
        if extra & (1 << (CLIMB_BITS + bit)) != 0 {
            Some(CLIMB_COST)
        } else if breach && extra & (1 << (WINDOW_BITS + bit)) != 0 {
            Some(WINDOW_BREACH_COST)
        } else if breach && extra & (1 << (DOOR_BITS + bit)) != 0 {
            Some(DOOR_BREACH_COST)
        } else {
            None
        }
    }
}

impl FlowField {
    /// Shortest walking paths to `goal` from every tile within `max_dist` metres of path distance.
    /// Tiles farther away are left unreachable, which keeps rebuilds cheap on big maps.
    pub fn build(walk: &WalkGrid, goal: IVec2, max_dist: f32) -> Self {
        Self::build_multi(walk, &[goal], max_dist, false)
    }

    /// Toward whichever of `goals` is nearest from each tile. With `breach`, paths may go through
    /// shut doors and intact windows at [`DOOR_BREACH_COST`] / [`WINDOW_BREACH_COST`] (zombie fields).
    pub fn build_multi(walk: &WalkGrid, goals: &[IVec2], max_dist: f32, breach: bool) -> Self {
        let mut f = Self::default();
        f.rebuild(walk, goals, max_dist, breach);
        f
    }

    /// Rebuild in place toward `goals`, reusing this field's buffers.
    pub fn rebuild(&mut self, walk: &WalkGrid, goals: &[IVec2], max_dist: f32, breach: bool) {
        let limit = (max_dist * STRAIGHT as f32) as u32;
        self.goals.clear();
        self.goals.extend_from_slice(goals);
        // Every step moves at most one tile per axis and costs at least a metre, so nothing within
        // `limit` lies farther than `max_dist` tiles from a goal on either axis.
        let reach = max_dist.ceil() as i32 + 2;
        let in_map: Vec<IVec2> = goals.iter().copied().filter(|g| walk.in_bounds(*g)).collect();
        let (mut lo, mut hi) = (IVec2::splat(i32::MAX), IVec2::splat(i32::MIN));
        for g in &in_map {
            lo = lo.min(*g);
            hi = hi.max(*g);
        }
        if in_map.is_empty() {
            (lo, hi) = (IVec2::ZERO, IVec2::new(-1, -1));
        } else {
            lo = (lo - IVec2::splat(reach)).max(IVec2::ZERO);
            hi = (hi + IVec2::splat(reach)).min(IVec2::new(walk.width - 1, walk.height - 1));
        }
        self.origin = lo;
        self.width = (hi.x - lo.x + 1).max(0);
        self.height = (hi.y - lo.y + 1).max(0);
        let (origin, width, height) = (self.origin, self.width, self.height);
        let inside = |t: IVec2| t.x >= origin.x && t.y >= origin.y && t.x < origin.x + width && t.y < origin.y + height;
        let index = |t: IVec2| ((t.y - origin.y) * width + (t.x - origin.x)) as usize;
        let n = (width * height) as usize;
        let cost = &mut self.cost;
        cost.clear();
        cost.resize(n, UNREACHABLE);
        let dir = &mut self.dir;
        dir.clear();
        dir.resize(n, NO_STEP);
        let heap = &mut self.heap;
        heap.clear();
        for &goal in &in_map {
            if cost[index(goal)] != 0 {
                cost[index(goal)] = 0;
                heap.push(Reverse((0u32, goal.x, goal.y)));
            }
        }
        while let Some(Reverse((c, x, y))) = heap.pop() {
            let t = IVec2::new(x, y);
            if c > cost[index(t)] || c > limit {
                continue;
            }
            // Movement rules are symmetric, so expanding outward from the goal is valid.
            // (Climbing and breaching are symmetric too: the same edge from either side.)
            let steps = walk.steps(t);
            let extra = walk.extra(t);
            for (k, d) in OFFSETS.iter().enumerate() {
                let step_cost = if k % 2 == 0 {
                    match walk.straight_extra(extra, k, breach) {
                        Some(x) => STRAIGHT + x,
                        None if steps & (1 << k) != 0 => STRAIGHT,
                        None => continue,
                    }
                } else if steps & (1 << k) != 0 {
                    DIAGONAL
                } else {
                    continue;
                };
                let nt = t + *d;
                if !inside(nt) {
                    continue;
                }
                let nc = c + step_cost;
                let ni = index(nt);
                if nc < cost[ni] {
                    cost[ni] = nc;
                    // From `nt` the way home is the opposite step, back to `t`.
                    dir[ni] = ((k + 4) % 8) as u8;
                    heap.push(Reverse((nc, nt.x, nt.y)));
                }
            }
        }
    }

    fn index(&self, t: IVec2) -> Option<usize> {
        let l = t - self.origin;
        (l.x >= 0 && l.y >= 0 && l.x < self.width && l.y < self.height).then(|| (l.y * self.width + l.x) as usize)
    }

    /// Path distance to the goal in metres, or `None` if unreachable.
    pub fn distance(&self, p: Vec2) -> Option<f32> {
        let c = self.cost[self.index(p.floor().as_ivec2())?];
        (c != UNREACHABLE).then(|| c as f32 / STRAIGHT as f32)
    }

    /// The side of `p`'s tile the path leaves through, when its next step is a straight one (the
    /// only kind that can climb or breach).
    pub fn straight_step_side(&self, p: Vec2) -> Option<Side> {
        self.straight_step_side_of(p.floor().as_ivec2())
    }

    /// [`straight_step_side`](Self::straight_step_side) for tile `t`.
    pub fn straight_step_side_of(&self, t: IVec2) -> Option<Side> {
        let i = self.index(t)?;
        let k = self.dir[i] as usize;
        (k != NO_STEP as usize && k.is_multiple_of(2) && self.cost[i] != 0).then(|| step_side(k))
    }

    /// The tile one step along the path from `t` (`None` at the goal, or off the field).
    pub fn next_tile(&self, t: IVec2) -> Option<IVec2> {
        let i = self.index(t)?;
        let k = self.dir[i] as usize;
        (k != NO_STEP as usize && self.cost[i] != 0 && self.cost[i] != UNREACHABLE).then(|| t + OFFSETS[k])
    }

    /// Does this field's window include tile `t` (so a map edit there can change it)?
    pub fn covers(&self, t: IVec2) -> bool {
        self.index(t).is_some()
    }

    /// Direction to move from `p`. On a goal tile, points straight at that tile's centre.
    pub fn direction(&self, p: Vec2) -> Vec2 {
        let t = p.floor().as_ivec2();
        match self.index(t) {
            Some(i) if self.cost[i] == 0 => (t.as_vec2() + Vec2::splat(0.5) - p).normalize_or_zero(),
            Some(i) => STEPS[self.dir[i] as usize],
            None => Vec2::ZERO,
        }
    }
}

/// Noise points are snapped to cells this many tiles wide, so nearby noises share one field.
pub const NOISE_CELL: i32 = 6;

/// A player field nobody used for this many ticks is dropped instead of following the player.
const PLAYER_IDLE_TICKS: u64 = 40;

/// What a cached field leads to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlowKey {
    /// A player's current tile (the field follows the player as it moves).
    Player(u32),
    /// A point (a noise, a last known position) snapped to a [`NOISE_CELL`] grid.
    Point(IVec2),
}

impl FlowKey {
    pub fn point(p: Vec2) -> Self {
        FlowKey::Point(p.floor().as_ivec2().div_euclid(IVec2::splat(NOISE_CELL)))
    }
}

#[derive(Clone, Debug)]
struct Entry {
    key: FlowKey,
    field: FlowField,
    /// Tile the field should lead to (for players, where the player is now).
    want: IVec2,
    built_tick: u64,
    used_tick: u64,
    /// The map changed since the build (a door).
    map_stale: bool,
}

/// A small cache of zombie flow fields (which may lead through shut doors and windows) with
/// least-recently-used eviction and a rebuild budget per tick.
/// Zombies ask with [`get`](Self::get); a miss queues a build for the next [`update`](Self::update)
/// and the zombie steers straight at its target meanwhile.
#[derive(Clone, Debug)]
pub struct FlowCache {
    entries: Vec<Entry>,
    /// Missing fields asked for since the last update, in request order.
    requests: Vec<(FlowKey, IVec2)>,
    capacity: usize,
    budget: u32,
    player_range: f32,
    point_range: f32,
    /// A moving player's field is rebuilt at most this often.
    player_rebuild_ticks: u64,
    /// Fields built by the last update.
    pub builds: u32,
}

impl FlowCache {
    pub fn new(capacity: usize, budget: u32, player_range: f32, point_range: f32, player_rebuild_ticks: u64) -> Self {
        Self { entries: Vec::with_capacity(capacity), requests: Vec::new(), capacity, budget, player_range, point_range, player_rebuild_ticks, builds: 0 }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The field for `key`, if built. `goal` is the tile to build toward on a miss.
    pub fn get(&mut self, key: FlowKey, goal: IVec2, tick: u64) -> Option<&FlowField> {
        match self.entries.iter().position(|e| e.key == key) {
            Some(i) => {
                let e = &mut self.entries[i];
                e.used_tick = tick;
                Some(&e.field)
            }
            None => {
                if self.requests.len() < self.capacity && !self.requests.iter().any(|(k, _)| *k == key) {
                    self.requests.push((key, goal));
                }
                None
            }
        }
    }

    /// The field for `key` if it is already built, without asking for one or counting as a use
    /// (near-tier zombies ride along on fields active zombies keep alive, but never cause builds).
    pub fn peek(&self, key: FlowKey) -> Option<&FlowField> {
        self.entries.iter().find(|e| e.key == key).map(|e| &e.field)
    }

    /// The map changed: every field needs rebuilding (they keep serving until then).
    pub fn invalidate(&mut self) {
        self.entries.iter_mut().for_each(|e| e.map_stale = true);
    }

    /// The map changed at tile `t` (a door, a window): rebuild the fields that cover it.
    pub fn invalidate_near(&mut self, t: IVec2) {
        for e in &mut self.entries {
            // A margin, since an edit can change the steps of the tiles around it.
            if [IVec2::ZERO, IVec2::X, IVec2::Y, IVec2::NEG_X, IVec2::NEG_Y].iter().any(|d| e.field.covers(t + *d)) {
                e.map_stale = true;
            }
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.requests.clear();
    }

    /// Spend this tick's build budget: follow moving players first, then build requested fields,
    /// then refresh fields the map made stale. `player_tile(id)` is where player `id` stands (None
    /// once it's gone, which drops its field).
    pub fn update(&mut self, walk: &WalkGrid, tick: u64, player_tile: impl Fn(u32) -> Option<IVec2>) {
        self.builds = 0;
        // A player's field is only kept up to date while zombies use it; one nobody asked for
        // lately is dropped (and rebuilt fresh when someone does).
        self.entries.retain(|e| !matches!(e.key, FlowKey::Player(id) if player_tile(id).is_none() || e.used_tick + PLAYER_IDLE_TICKS < tick));
        for e in &mut self.entries {
            if let FlowKey::Player(id) = e.key {
                e.want = player_tile(id).unwrap_or(e.want);
            }
        }
        let mut budget = self.budget;
        // 1. Players who moved to another tile (rate-limited), or whose map changed.
        for e in &mut self.entries {
            if budget == 0 {
                break;
            }
            let moved = e.field.goals.first() != Some(&e.want) && tick >= e.built_tick + self.player_rebuild_ticks;
            if matches!(e.key, FlowKey::Player(_)) && (moved || e.map_stale) {
                e.field.rebuild(walk, &[e.want], self.player_range, true);
                (e.built_tick, e.map_stale) = (tick, false);
                budget -= 1;
                self.builds += 1;
            }
        }
        // 2. Missing fields, evicting the least recently used when full.
        let mut taken = 0;
        for &(key, goal) in &self.requests {
            if budget == 0 {
                break;
            }
            taken += 1;
            if self.entries.iter().any(|e| e.key == key) {
                continue;
            }
            let range = match key {
                FlowKey::Player(_) => self.player_range,
                FlowKey::Point(_) => self.point_range,
            };
            let want = match key {
                FlowKey::Player(id) => player_tile(id).unwrap_or(goal),
                FlowKey::Point(_) => goal,
            };
            let slot = if self.entries.len() < self.capacity {
                self.entries.push(Entry { key, field: FlowField::default(), want, built_tick: tick, used_tick: tick, map_stale: false });
                self.entries.len() - 1
            } else {
                // Oldest use loses; ties go to the earliest entry, so this is deterministic. A field
                // still in use is never evicted (that only thrashes): the request waits instead.
                let (i, lru) = self.entries.iter().enumerate().min_by_key(|(_, e)| e.used_tick).expect("capacity > 0");
                if lru.used_tick + 2 >= tick {
                    taken -= 1;
                    break;
                }
                let e = &mut self.entries[i];
                (e.key, e.want, e.used_tick, e.map_stale) = (key, want, tick, false);
                i
            };
            let e = &mut self.entries[slot];
            e.field.rebuild(walk, &[want], range, true);
            e.built_tick = tick;
            budget -= 1;
            self.builds += 1;
        }
        self.requests.drain(..taken);
        // 3. Point fields the map made stale, oldest first.
        while budget > 0 {
            let Some(e) = self.entries.iter_mut().filter(|e| e.map_stale).min_by_key(|e| e.built_tick) else { break };
            e.field.rebuild(walk, &[e.want], self.point_range, true);
            (e.built_tick, e.map_stale) = (tick, false);
            budget -= 1;
            self.builds += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map::Edge;

    #[test]
    fn local_field_routes_around_a_wall() {
        let mut m = TileMap::new(4, 4);
        // A wall along x = 20 from y = 0 to 40, open below.
        for y in 0..40 {
            m.set_edge(IVec2::new(20, y), Side::W, Edge::Wall);
        }
        let walk = WalkGrid::new(&m);
        let f = FlowField::build(&walk, IVec2::new(25, 10), 100.0);
        let p = Vec2::new(15.5, 10.5);
        let d = f.distance(p).expect("reachable around the wall");
        assert!(d > 50.0, "path goes around: {d}");
        assert!(f.direction(p).y > 0.5, "heads south to get round: {}", f.direction(p));
        // A short-range field only covers a small window; outside it, unknown.
        let small = FlowField::build(&walk, IVec2::new(5, 5), 10.0);
        assert_eq!(small.distance(Vec2::new(5.5, 9.5)), Some(4.0));
        assert!(small.distance(Vec2::new(40.5, 40.5)).is_none());
    }

    /// A walled room with a shut door on the west and an intact window on the east: walking fields
    /// can't get in, zombie (breach) fields go through whichever is cheaper.
    #[test]
    fn breach_fields_lead_through_doors_and_windows() {
        let mut m = TileMap::new(4, 4);
        let (lo, hi) = (IVec2::new(20, 20), IVec2::new(26, 26));
        for i in lo.x..hi.x {
            m.set_edge(IVec2::new(i, lo.y), Side::N, Edge::Wall);
            m.set_edge(IVec2::new(i, hi.y - 1), Side::S, Edge::Wall);
        }
        for j in lo.y..hi.y {
            m.set_edge(IVec2::new(lo.x, j), Side::W, Edge::Wall);
            m.set_edge(IVec2::new(hi.x - 1, j), Side::E, Edge::Wall);
        }
        m.set_edge(IVec2::new(lo.x, 23), Side::W, Edge::DoorClosed);
        m.set_edge(IVec2::new(hi.x - 1, 23), Side::E, Edge::Window);
        let walk = WalkGrid::new(&m);
        let inside = IVec2::new(23, 23);
        assert!(FlowField::build(&walk, inside, 60.0).distance(Vec2::new(10.5, 23.5)).is_none(), "walked through a shut house");
        let f = FlowField::build_multi(&walk, &[inside], 60.0, true);
        // From due west, through the door (cost of the door on top of the walk).
        let west = f.distance(Vec2::new(18.5, 23.5)).unwrap();
        assert!((west - (5.0 + DOOR_BREACH_COST as f32 / 10.0)).abs() < 0.01, "{west}");
        assert_eq!(f.straight_step_side(Vec2::new(19.5, 23.5)), Some(Side::E));
        // From just off the north-west corner it's cheaper to go round to the window.
        let corner = Vec2::new(19.5, 19.5);
        assert!(f.distance(corner).unwrap() < 8.0 + DOOR_BREACH_COST as f32 / 10.0);
        assert!(f.direction(corner).x > 0.5, "should head east for the window: {}", f.direction(corner));
        // Open the window: climbing is cheaper than breaking it, and no diagonal cuts through it.
        m.set_edge(IVec2::new(hi.x - 1, 23), Side::E, Edge::WindowOpen);
        let walk = WalkGrid::new(&m);
        let f = FlowField::build(&walk, inside, 60.0);
        let east = f.distance(Vec2::new(27.5, 23.5)).unwrap();
        assert!((east - (4.0 + CLIMB_COST as f32 / 10.0)).abs() < 0.01, "{east}");
        assert_eq!(f.straight_step_side(Vec2::new(26.5, 23.5)), Some(Side::W));
        assert!(f.distance(Vec2::new(27.5, 22.5)).unwrap() > east, "cut diagonally through a window");
    }

    #[test]
    fn cache_builds_on_request_within_budget_and_evicts_lru() {
        let m = WalkGrid::new(&TileMap::new(4, 4));
        let mut c = FlowCache::new(2, 1, 30.0, 20.0, 4);
        let a = FlowKey::point(Vec2::new(5.0, 5.0));
        let b = FlowKey::point(Vec2::new(30.0, 30.0));
        assert!(c.get(a, IVec2::new(5, 5), 0).is_none());
        assert!(c.get(b, IVec2::new(30, 30), 0).is_none());
        c.update(&m, 1, |_| None);
        assert_eq!((c.builds, c.len()), (1, 1), "budget of one build per tick");
        assert!(c.get(a, IVec2::new(5, 5), 1).is_some());
        c.update(&m, 2, |_| None);
        assert!(c.get(b, IVec2::new(30, 30), 2).is_some());
        // Nearby noises share a field.
        assert_eq!(FlowKey::point(Vec2::new(4.5, 5.5)), a);
        // A third key waits while both fields are in use...
        let third = FlowKey::point(Vec2::new(50.0, 10.0));
        assert!(c.get(third, IVec2::new(50, 10), 3).is_none());
        c.update(&m, 3, |_| None);
        assert_eq!(c.builds, 0);
        // ...then evicts the least recently used (a, last used at tick 1) once it goes cold.
        assert!(c.get(b, IVec2::new(30, 30), 5).is_some());
        assert!(c.get(third, IVec2::new(50, 10), 5).is_none());
        c.update(&m, 6, |_| None);
        assert_eq!((c.builds, c.len()), (1, 2));
        assert!(c.get(third, IVec2::new(50, 10), 6).is_some());
        assert!(c.get(a, IVec2::new(5, 5), 6).is_none());
        assert!(c.get(b, IVec2::new(30, 30), 6).is_some());
    }
}
