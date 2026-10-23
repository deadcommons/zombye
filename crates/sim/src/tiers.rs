//! Simulation tiers: zombies far from every player get less of the simulation, so a world can hold
//! tens of thousands of them.
//!
//! - **Active** (its chunk within [`TierTuning::active_range`] of a player): everything, every tick
//!   ([`crate::ai`], [`crate::senses`], crowd separation, combat). Only these go to clients.
//! - **Near** (out to [`TierTuning::near_range`]): every [`TierTuning::near_interval_ticks`] ticks,
//!   staggered by id ([`step_near`]): hearing (the normal pass: near zombies are in the spatial
//!   hash), heading for what it heard or remembers (along a flow field active zombies already keep
//!   built, else straight), sliding along walls. No sight, no strikes, no thumps, no separation.
//! - **Virtual** (beyond): bulk updates per [`TierTuning::region_size`] region, each region every
//!   [`TierTuning::virtual_interval_ticks`] ticks ([`step_virtual`]): a slow wander, or a drift
//!   toward whatever it last heard (loud noises reach them, [`virtual_hearing`]). Moves never cross
//!   anything that blocks walking, so a virtual zombie stays on the side of every wall it was on.
//!
//! All tiers live in the one struct-of-arrays ([`Zombies`], with a [`Tier`] column): a zombie keeps
//! its id, index order and every AI column (state, target, timers) whatever tier it is on, so a
//! zombie investigating a noise is still heading there when it's promoted, and walking away and
//! back finds the same zombies. Virtual zombies are indexed per region ([`RegionIndex`], rebuilt
//! each tick like the spatial hash), which is what their bulk update walks; on its turn a region
//! also feels the pull of loud events and sends some of its wanderers off toward them
//! ([`crate::population`]). The per-tick cost of a virtual zombie is one skipped iteration in a
//! couple of column scans.
//!
//! Tiers are decided per chunk ([`ChunkDistances`]): every zombie in a chunk is promoted together
//! when the chunk comes within range of a player, with [`TierTuning::hysteresis`] metres of slack
//! before it is demoted again (more than a chunk, so a zombie crossing a chunk border can't flip).
//! Promotion out of the virtual tier settles the zombie on a free tile it can walk to from where it
//! is ([`settle`]), so it never lands in a wall, a tree, furniture or a sealed building.
//! With no players at all nothing changes tier (tests and debug worlds stay fully active).

use std::f32::consts::TAU;

use glam::{IVec2, Vec2};
use serde::{Deserialize, Serialize};

use crate::TICK_DT;
use crate::ai::{self, GOAL_RANGE, ZOMBIE_RADIUS, ZombieTuning};
use crate::flow::{FlowCache, FlowField, FlowKey};
use crate::map::{CHUNK_SIZE, Side, TileMap};
use crate::population::{self, MigrationDef, Population};
use crate::rng::{Rng, hash01};
use crate::senses::{self, Sound, SoundSource};
use crate::vision::clear_path;
use crate::zombies::{Tier, ZombieAnim, ZombieState, Zombies};

/// Tier ranges and update rates. Part of `WorldConfig`; serde with defaults. Our own numbers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TierTuning {
    /// Off: every zombie stays active (the old behaviour).
    pub enabled: bool,
    /// A chunk this close to a player (metres, to the nearest point of the chunk) is active, and
    /// one this close is near; beyond is virtual.
    pub active_range: f32,
    pub near_range: f32,
    /// A zombie is only demoted once its chunk is this much farther than the range it was promoted
    /// at. More than a chunk (16 m), so walking over one chunk border can't flip it back and forth.
    pub hysteresis: f32,
    /// Near zombies update once every this many ticks (staggered by id).
    pub near_interval_ticks: u32,
    /// Each region of virtual zombies updates once every this many ticks (regions staggered).
    pub virtual_interval_ticks: u32,
    /// Side of a virtual region in metres.
    pub region_size: f32,
    /// Only sounds at least this loud (radius, metres) are checked against virtual zombies: the
    /// everyday noises (footsteps, doors, groans, thumps) happen around players, where there are none.
    pub virtual_hearing_min: f32,
    /// Snapshots list active zombies within this distance of a player.
    pub interest_radius: f32,
}

impl Default for TierTuning {
    fn default() -> Self {
        Self {
            enabled: true,
            active_range: 60.0,
            near_range: 150.0,
            hysteresis: 20.0,
            near_interval_ticks: 5,
            virtual_interval_ticks: 100,
            region_size: 64.0,
            virtual_hearing_min: 30.0,
            interest_radius: 80.0,
        }
    }
}

/// The tier a zombie on tier `cur` should be on when its chunk is `d` metres from the nearest
/// player. Promotion happens at the range; demotion only `hysteresis` beyond it.
pub fn next_tier(cur: Tier, d: f32, t: &TierTuning) -> Tier {
    let h = t.hysteresis;
    if d <= t.active_range || (cur == Tier::Active && d <= t.active_range + h) {
        Tier::Active
    } else if d <= t.near_range || (cur <= Tier::Near && d <= t.near_range + h) {
        Tier::Near
    } else {
        Tier::Virtual
    }
}

/// Per chunk, the distance from the nearest player to the nearest point of the chunk.
#[derive(Clone, Debug, Default)]
pub struct ChunkDistances {
    chunks_x: i32,
    chunks_y: i32,
    dist: Vec<f32>,
}

impl ChunkDistances {
    pub fn update(&mut self, map: &TileMap, players: impl Iterator<Item = Vec2> + Clone) {
        (self.chunks_x, self.chunks_y) = (map.chunks_x, map.chunks_y);
        self.dist.clear();
        let size = CHUNK_SIZE as f32;
        for cy in 0..self.chunks_y {
            for cx in 0..self.chunks_x {
                let lo = Vec2::new(cx as f32, cy as f32) * size;
                let hi = lo + Vec2::splat(size);
                let d2 = players.clone().map(|p| p.clamp(lo, hi).distance_squared(p)).fold(f32::INFINITY, f32::min);
                self.dist.push(d2.sqrt());
            }
        }
    }

    /// Distance for the chunk holding `p` (positions off the map use the nearest chunk).
    pub fn of(&self, p: Vec2) -> f32 {
        if self.dist.is_empty() {
            return f32::INFINITY;
        }
        let c = (p / CHUNK_SIZE as f32).floor().as_ivec2();
        let (cx, cy) = (c.x.clamp(0, self.chunks_x - 1), c.y.clamp(0, self.chunks_y - 1));
        self.dist[(cy * self.chunks_x + cx) as usize]
    }
}

/// Indices of zombies per fixed square region of the map (counting sort; no allocation after
/// warm-up). Holds the virtual tier.
#[derive(Clone, Debug, Default)]
pub struct RegionIndex {
    size: f32,
    cols: i32,
    rows: i32,
    starts: Vec<u32>,
    items: Vec<u32>,
    fill: Vec<u32>,
}

impl RegionIndex {
    pub fn new(map: &TileMap, size: f32) -> Self {
        let size = size.max(CHUNK_SIZE as f32);
        let cols = (map.width() as f32 / size).ceil() as i32;
        let rows = (map.height() as f32 / size).ceil() as i32;
        Self { size, cols, rows, starts: vec![0; (cols * rows + 1) as usize], items: Vec::new(), fill: Vec::new() }
    }

    pub fn regions(&self) -> usize {
        (self.cols * self.rows) as usize
    }

    /// Region size in metres.
    pub fn size(&self) -> f32 {
        self.size
    }

    pub fn region_of(&self, p: Vec2) -> usize {
        let c = (p / self.size).floor().as_ivec2();
        (c.y.clamp(0, self.rows - 1) * self.cols + c.x.clamp(0, self.cols - 1)) as usize
    }

    /// Region `r`'s min corner in metres.
    pub fn corner(&self, r: usize) -> Vec2 {
        Vec2::new((r as i32 % self.cols) as f32, (r as i32 / self.cols) as f32) * self.size
    }

    /// Region `r`'s min and max corners in metres.
    pub fn rect(&self, r: usize) -> (Vec2, Vec2) {
        let lo = self.corner(r);
        (lo, lo + Vec2::splat(self.size))
    }

    /// Regions across and down.
    pub fn cols(&self) -> i32 {
        self.cols
    }

    pub fn rows(&self) -> i32 {
        self.rows
    }

    pub fn rebuild_where(&mut self, positions: &[Vec2], keep: impl Fn(usize) -> bool) {
        self.starts.iter_mut().for_each(|s| *s = 0);
        let mut kept = 0;
        for (i, p) in positions.iter().enumerate() {
            if keep(i) {
                let r = self.region_of(*p);
                self.starts[r + 1] += 1;
                kept += 1;
            }
        }
        for i in 1..self.starts.len() {
            self.starts[i] += self.starts[i - 1];
        }
        self.items.resize(kept, 0);
        let mut fill = std::mem::take(&mut self.fill);
        fill.clear();
        fill.extend_from_slice(&self.starts);
        for (i, p) in positions.iter().enumerate() {
            if keep(i) {
                let r = self.region_of(*p);
                self.items[fill[r] as usize] = i as u32;
                fill[r] += 1;
            }
        }
        self.fill = fill;
    }

    /// Indices in region `r`, in index (so id) order.
    pub fn members(&self, r: usize) -> &[u32] {
        &self.items[self.starts[r] as usize..self.starts[r + 1] as usize]
    }

    pub fn count(&self, r: usize) -> usize {
        self.members(r).len()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Calls `f(index)` for everything in the regions overlapping the square of half-size `r`
    /// around `p` (a superset: check the distance).
    pub fn for_each_within(&self, p: Vec2, r: f32, mut f: impl FnMut(usize)) {
        let lo = ((p - Vec2::splat(r)) / self.size).floor().as_ivec2().max(IVec2::ZERO);
        let hi = ((p + Vec2::splat(r)) / self.size).floor().as_ivec2().min(IVec2::new(self.cols - 1, self.rows - 1));
        for y in lo.y..=hi.y {
            for x in lo.x..=hi.x {
                for &i in self.members((y * self.cols + x) as usize) {
                    f(i as usize);
                }
            }
        }
    }
}

/// Tier changes made by one [`assign`] pass.
#[derive(Clone, Copy, Debug, Default)]
pub struct TierChanges {
    pub promoted: u32,
    pub demoted: u32,
}

/// Move every zombie to the tier its chunk calls for (see [`next_tier`]), fixing up what a tier
/// change needs: out of the virtual tier it is settled on a free tile it can walk to ([`settle`]);
/// demoted, it drops a lunge or strike (back to chasing, from memory); either way it forgets what
/// it saw and rechecks its straight path. Its intent (state, target, timers) is kept.
pub(crate) fn assign(z: &mut Zombies, dist: &ChunkDistances, t: &TierTuning, zt: &ZombieTuning, map: &TileMap, scratch: &mut Vec<IVec2>) -> TierChanges {
    let mut changes = TierChanges::default();
    for i in 0..z.len() {
        let old = z.tier[i];
        let new = next_tier(old, dist.of(z.pos[i]), t);
        if new == old {
            continue;
        }
        z.tier[i] = new;
        if z.state[i] == ZombieState::Dead {
            continue;
        }
        if new < old {
            changes.promoted += 1;
            if old == Tier::Virtual {
                z.pos[i] = settle(map, z.pos[i], scratch);
                z.vel[i] = Vec2::ZERO;
            }
        } else {
            changes.demoted += 1;
            if matches!(z.state[i], ZombieState::Lunge | ZombieState::Attack) {
                z.state[i] = ZombieState::Chase;
                z.timer[i] = zt.memory_secs;
            }
            if new == Tier::Virtual {
                z.vel[i] = Vec2::ZERO;
                // Virtual zombies stand on free tiles (a body's centre can be in a tree's tile).
                if map.tile_blocked(z.pos[i].floor().as_ivec2()) {
                    z.pos[i] = settle(map, z.pos[i], scratch);
                }
            }
        }
        z.sees[i] = 0;
        z.direct[i] = false;
        z.anchor[i] = z.pos[i];
        z.stuck[i] = 0.0;
    }
    changes
}

/// Tiles searched at most when settling a zombie whose tile is blocked.
const SETTLE_SEARCH: usize = 96;

/// A spot for a zombie coming out of the virtual tier at `p` (or going into it from a blocked
/// tile): on its own tile if that is free, else
/// on the nearest free tile it could walk to from there (a breadth-first search over walkable
/// steps, so never through a wall, a shut door or window, or into a sealed building). Kept well
/// inside the tile and pushed out of anything it overlaps. `scratch` is reused between calls.
pub fn settle(map: &TileMap, p: Vec2, scratch: &mut Vec<IVec2>) -> Vec2 {
    let start = p.floor().as_ivec2().clamp(IVec2::ZERO, IVec2::new(map.width() - 1, map.height() - 1));
    scratch.clear();
    scratch.push(start);
    let mut k = 0;
    let mut found = None;
    while k < scratch.len() && k < SETTLE_SEARCH {
        let t = scratch[k];
        k += 1;
        if !map.tile_blocked(t) {
            found = Some(t);
            break;
        }
        for side in Side::ALL {
            let n = t + side.offset();
            if map.in_bounds(n) && !map.edge(t, side).blocks_movement() && !scratch.contains(&n) {
                scratch.push(n);
            }
        }
    }
    let tile = found.unwrap_or(start);
    // Where in the tile it was, kept off the very edges, so being pushed out of a wall can only push
    // it further into its own tile.
    let f = (p - tile.as_vec2()).clamp(Vec2::splat(0.05), Vec2::splat(0.95));
    map.resolve_circle(tile.as_vec2() + f, ZOMBIE_RADIUS)
}

/// What the near-tier update reads or writes besides the zombies.
pub(crate) struct NearCtx<'a> {
    pub map: &'a TileMap,
    pub flow: &'a FlowCache,
    pub goal: Option<&'a FlowField>,
    pub tuning: &'a ZombieTuning,
    pub tiers: &'a TierTuning,
    pub rng: &'a mut Rng,
    pub tick: u64,
}

/// Longest move a near zombie makes in one collision step (less than its radius, so it can't
/// tunnel through a wall).
const NEAR_SUBSTEP: f32 = 0.25;

/// The near tier's update: every zombie whose turn it is (`(id + tick) % near_interval_ticks == 0`)
/// runs a cut-down state machine over the time since its last update and moves, with collision but
/// no crowd separation. New positions go to `next_pos`. Returns how many were updated.
#[allow(clippy::needless_range_loop)]
pub(crate) fn step_near(z: &mut Zombies, next_pos: &mut [Vec2], cx: &mut NearCtx) -> u32 {
    let t = cx.tuning;
    let every = cx.tiers.near_interval_ticks.max(1) as u64;
    let dt = every as f32 * TICK_DT;
    let mut updated = 0;
    for i in 0..z.len() {
        if z.tier[i] != Tier::Near || z.state[i] == ZombieState::Dead || !(z.id[i] as u64 + cx.tick).is_multiple_of(every) {
            continue;
        }
        updated += 1;
        let p = z.pos[i];
        z.sees[i] = 0;
        z.cooldown[i] = (z.cooldown[i] - dt).max(0.0);
        z.bored[i] = (z.bored[i] - dt).max(0.0);
        let mut desired = Vec2::ZERO;
        if z.state[i].is_reeling() {
            z.timer[i] -= dt;
            if z.timer[i] <= 0.0 {
                ai::recover(z, i, t);
            }
        } else if let Some(goal) = cx.goal {
            desired = match goal.distance(p) {
                Some(d) if d <= GOAL_RANGE => goal.direction(p) * z.speed[i],
                _ => ai::wander(z, i, cx.rng, t, dt),
            };
        } else {
            match z.state[i] {
                ZombieState::Wander => desired = ai::wander(z, i, cx.rng, t, dt),
                ZombieState::Investigate | ZombieState::Chase | ZombieState::Lunge | ZombieState::Attack => {
                    // Without sight a chaser only has the spot it remembers (hearing its player
                    // moves that spot), so both go to a point and look about there.
                    let chasing = z.state[i] != ZombieState::Investigate;
                    if chasing {
                        z.state[i] = ZombieState::Chase;
                    }
                    z.timer[i] -= dt;
                    let target = z.target_pos[i];
                    if z.timer[i] <= 0.0 {
                        if chasing {
                            ai::start_investigating(z, i, t);
                        } else {
                            z.state[i] = ZombieState::Wander;
                            z.wander_timer[i] = 0.0;
                        }
                    } else if z.bored[i] > 0.0 {
                        desired = ai::mill(z, i, cx.rng, t, target, dt);
                    } else {
                        let speed = if chasing { z.speed[i] } else { z.speed[i] * t.investigate_speed_mult };
                        desired = match heading(cx.flow, p, target) {
                            Some(dir) => dir * speed,
                            None => ai::mill(z, i, cx.rng, t, target, dt),
                        };
                    }
                }
                ZombieState::Thump => {
                    // Out of reach of anyone to batter its way to: it waits, its interest running
                    // down (any player's noise tops it up), and carries on if promoted in time.
                    z.timer[i] -= dt;
                    if z.timer[i] <= 0.0 {
                        ai::lose_interest(z, i, t);
                    }
                }
                ZombieState::Staggered | ZombieState::Down | ZombieState::Dead => {}
            }
        }
        // Move in short collision steps; slow through a window frame.
        let total = desired * dt;
        let steps = (total.length() / NEAR_SUBSTEP).ceil().clamp(1.0, 16.0);
        let step = total / steps;
        let mut q = p;
        for _ in 0..steps as u32 {
            let s = if cx.map.in_window_opening(q) { step.clamp_length_max(t.climb_speed * dt / steps) } else { step };
            q = cx.map.resolve_circle(q + s, ZOMBIE_RADIUS);
        }
        next_pos[i] = q;
        z.vel[i] = desired;
        if desired.length_squared() > 1e-4 {
            z.facing[i] = desired.y.atan2(desired.x);
        }
        z.anim[i] = if z.state[i].is_reeling() {
            if z.state[i] == ZombieState::Down { ZombieAnim::Down } else { ZombieAnim::Stagger }
        } else if desired.length_squared() > 0.0025 {
            ZombieAnim::Walk
        } else {
            ZombieAnim::Idle
        };
    }
    updated
}

/// Which way a near zombie at `p` goes for `target`: along a flow field that active zombies keep
/// built for that spot, if one covers it, else straight. `None` once it's there.
fn heading(flow: &FlowCache, p: Vec2, target: Vec2) -> Option<Vec2> {
    let to = target - p;
    let d = to.length();
    if d <= 1.5 {
        return None;
    }
    if let Some(field) = flow.peek(FlowKey::point(target))
        && field.distance(p).is_some_and(|fd| fd > 1.5)
    {
        let dir = field.direction(p);
        if dir != Vec2::ZERO {
            return Some(dir);
        }
    }
    Some(to / d)
}

/// Sounds loud enough to reach the virtual tier: virtual zombies in earshot react as any zombie
/// would ([`senses::react`]). There's no ray cast for them: a sound carries its full radius when
/// both ends are outdoors, else only `wall_muffle` of it.
pub(crate) fn virtual_hearing(z: &mut Zombies, regions: &RegionIndex, map: &TileMap, sounds: &[Sound], t: &ZombieTuning, tiers: &TierTuning) {
    for s in sounds {
        if s.radius < tiers.virtual_hearing_min {
            continue;
        }
        let outdoors = map.room_at(s.pos) == 0;
        regions.for_each_within(s.pos, s.radius, |i| {
            let d = z.pos[i].distance(s.pos);
            if d > s.radius || s.source == SoundSource::Zombie(z.id[i]) {
                return;
            }
            if d > s.radius * t.wall_muffle && !(outdoors && map.room_at(z.pos[i]) == 0) {
                return;
            }
            senses::react(z, i, s, d, t);
        });
    }
}

/// The virtual tier's update: each region whose turn it is (`(region + tick) %
/// virtual_interval_ticks == 0`) moves all its zombies in one go over the whole interval. Returns
/// how many were updated. Randomness is a hash of (seed, tick, id), so it doesn't depend on
/// iteration order or touch the world's RNG.
#[allow(clippy::too_many_arguments)]
pub(crate) fn step_virtual(z: &mut Zombies, regions: &RegionIndex, map: &TileMap, t: &ZombieTuning, tiers: &TierTuning, pop: &Population, migration: &MigrationDef, tick: u64, seed: u64) -> u32 {
    let every = tiers.virtual_interval_ticks.max(1) as u64;
    let mut updated = 0;
    for r in 0..regions.regions() {
        if !(r as u64 + tick).is_multiple_of(every) || regions.count(r) == 0 {
            continue;
        }
        // Migration: the region feels the strongest attractor's pull, and some of its wanderers
        // set off toward it (as investigators, so they keep going on any tier).
        if !pop.attractors.is_empty() {
            let pull = pop.pull_on(regions, r, migration, tick);
            population::migrate_region(z, regions.members(r), pull, migration, t, tick, seed);
        }
        for &i in regions.members(r) {
            step_one_virtual(z, i as usize, map, t, every as f32 * TICK_DT, tick, seed);
            updated += 1;
        }
    }
    updated
}

fn step_one_virtual(z: &mut Zombies, i: usize, map: &TileMap, t: &ZombieTuning, dt: f32, tick: u64, seed: u64) {
    let roll = |k: u64| hash01(seed ^ 0x5649_5254, tick, z.id[i] as u64, k);
    let p = z.pos[i];
    z.sees[i] = 0;
    z.cooldown[i] = (z.cooldown[i] - dt).max(0.0);
    z.bored[i] = (z.bored[i] - dt).max(0.0);
    let mut to = p;
    match z.state[i] {
        ZombieState::Dead => return,
        ZombieState::Staggered | ZombieState::Down => {
            z.timer[i] -= dt;
            if z.timer[i] <= 0.0 {
                ai::recover(z, i, t);
            }
        }
        ZombieState::Wander => {
            if roll(1) < 0.5 {
                to = p + Vec2::from_angle(roll(2) * TAU) * t.wander_speed * dt;
            }
        }
        ZombieState::Thump => {
            z.timer[i] -= dt;
            if z.timer[i] <= 0.0 {
                ai::lose_interest(z, i, t);
            }
        }
        ZombieState::Investigate | ZombieState::Chase | ZombieState::Lunge | ZombieState::Attack => {
            let chasing = z.state[i] != ZombieState::Investigate;
            if chasing {
                z.state[i] = ZombieState::Chase;
            }
            z.timer[i] -= dt;
            let target = z.target_pos[i];
            if z.timer[i] <= 0.0 {
                if chasing {
                    ai::start_investigating(z, i, t);
                } else {
                    z.state[i] = ZombieState::Wander;
                    z.wander_timer[i] = 0.0;
                }
            } else {
                let off = target - p;
                let d = off.length();
                if z.bored[i] > 0.0 || d <= t.search_radius {
                    // There (or given up): shuffle about within the search radius of the spot.
                    let hop = target + Vec2::from_angle(roll(3) * TAU) * t.search_radius * roll(4);
                    to = p + (hop - p).clamp_length_max(t.wander_speed * 1.5 * dt);
                } else {
                    let speed = if chasing { z.speed[i] } else { z.speed[i] * t.investigate_speed_mult };
                    to = p + off / d * (speed * dt).min(d);
                }
            }
        }
    }
    let q = virtual_move(map, p, to);
    if q != p {
        let d = q - p;
        z.facing[i] = d.y.atan2(d.x);
        z.anim[i] = ZombieAnim::Walk;
    } else {
        z.anim[i] = ZombieAnim::Idle;
    }
    z.pos[i] = q;
}

/// Move a virtual zombie from `p` toward `to` in one go, never across anything that blocks walking
/// and never onto a blocked tile: straight if it can, else turned up to 90° either way (shorter),
/// else it stays put.
fn virtual_move(map: &TileMap, p: Vec2, to: Vec2) -> Vec2 {
    let off = to - p;
    let len = off.length();
    if len < 1e-4 {
        return p;
    }
    let ok = |q: Vec2| map.in_bounds(q.floor().as_ivec2()) && !map.tile_blocked(q.floor().as_ivec2()) && clear_path(map, p, q);
    let dir = off / len;
    for (angle, frac) in [(0.0, 1.0), (0.785, 0.8), (-0.785, 0.8), (1.571, 0.5), (-1.571, 0.5)] {
        let q = p + Vec2::from_angle(angle).rotate(dir) * len * frac;
        if ok(q) {
            return q;
        }
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiers_have_hysteresis() {
        let t = TierTuning::default();
        use Tier::*;
        // Promotion at the range...
        assert_eq!(next_tier(Virtual, t.near_range + 1.0, &t), Virtual);
        assert_eq!(next_tier(Virtual, t.near_range, &t), Near);
        assert_eq!(next_tier(Virtual, t.active_range, &t), Active);
        assert_eq!(next_tier(Near, t.active_range, &t), Active);
        // ...demotion only well beyond it.
        assert_eq!(next_tier(Active, t.active_range + 1.0, &t), Active);
        assert_eq!(next_tier(Active, t.active_range + t.hysteresis, &t), Active);
        assert_eq!(next_tier(Active, t.active_range + t.hysteresis + 0.1, &t), Near);
        assert_eq!(next_tier(Near, t.near_range + t.hysteresis, &t), Near);
        assert_eq!(next_tier(Near, t.near_range + t.hysteresis + 0.1, &t), Virtual);
        // A jump goes straight to the far tier.
        assert_eq!(next_tier(Active, 1000.0, &t), Virtual);
        // Within the band, whatever it was it stays.
        for d in [t.active_range + 5.0, t.active_range + 15.0] {
            assert_eq!(next_tier(Active, d, &t), Active);
            assert_eq!(next_tier(Near, d, &t), Near);
        }
        assert!(t.hysteresis > CHUNK_SIZE as f32, "a chunk border crossing could flip tiers");
    }

    #[test]
    fn region_index_groups_by_region() {
        let map = TileMap::new(8, 8);
        let mut r = RegionIndex::new(&map, 64.0);
        assert_eq!(r.regions(), 4);
        let pts = [Vec2::new(1.0, 1.0), Vec2::new(70.0, 5.0), Vec2::new(2.0, 2.0), Vec2::new(100.0, 100.0), Vec2::new(-5.0, 300.0)];
        r.rebuild_where(&pts, |i| i != 3);
        assert_eq!(r.members(0), &[0, 2]);
        assert_eq!(r.members(1), &[1]);
        assert_eq!(r.members(2), &[4], "off the map: the nearest region");
        assert_eq!(r.count(3), 0);
        let mut near = Vec::new();
        r.for_each_within(Vec2::new(10.0, 10.0), 20.0, |i| near.push(i));
        assert_eq!(near, vec![0, 2]);
    }

    #[test]
    fn settling_never_lands_in_a_wall_or_a_sealed_room() {
        use crate::map::{Edge, Feature};
        let mut map = TileMap::new(2, 2);
        // A sealed 3x3 room at (10..13, 10..13), and a tree just outside it.
        for k in 10..13 {
            map.set_edge(IVec2::new(k, 10), Side::N, Edge::Wall);
            map.set_edge(IVec2::new(k, 12), Side::S, Edge::Wall);
            map.set_edge(IVec2::new(10, k), Side::W, Edge::Wall);
            map.set_edge(IVec2::new(12, k), Side::E, Edge::Wall);
        }
        map.tile_mut(IVec2::new(13, 11)).unwrap().feature = Feature::Tree;
        // On the tree, right against the room's wall: out, on the outside.
        let q = settle(&map, Vec2::new(13.02, 11.5), &mut Vec::new());
        assert!(!map.tile_blocked(q.floor().as_ivec2()));
        assert!(q.x >= 13.0 || q.y < 10.0 || q.y >= 13.0, "settled inside the sealed room: {q}");
        assert!(map.resolve_circle(q, ZOMBIE_RADIUS).distance(q) < 1e-3);
        // Right on the wall line inside: stays inside.
        let q = settle(&map, Vec2::new(12.999, 11.5), &mut Vec::new());
        assert_eq!(q.floor().as_ivec2(), IVec2::new(12, 11));
        // Furniture filling the room except one tile: it ends up on that tile.
        for (x, y) in [(10, 10), (11, 10), (12, 10), (10, 11), (11, 11), (12, 11), (10, 12), (11, 12)] {
            map.tile_mut(IVec2::new(x, y)).unwrap().feature = Feature::Furniture;
        }
        let q = settle(&map, Vec2::new(10.5, 10.5), &mut Vec::new());
        assert_eq!(q.floor().as_ivec2(), IVec2::new(12, 12));
    }
}
