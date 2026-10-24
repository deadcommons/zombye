//! Where zombies live, and how the far horde moves (issue #4).
//!
//! - **Density is data** ([`PopulationDef`], `assets/data/population.ron`, parsed with
//!   [`parse_population`] and loaded by `net::content::load_population`): zombies per km² of open
//!   ground in each [`Zone`]. The town generator decides every tile's zone
//!   ([`crate::worldgen::zone_at`]: it knows the town bounds, roads, lots, buildings and forest).
//!   The world places its zombies by those densities ([`PopulationDef::tile_density`]) and keeps a
//!   target count per region ([`Population::target`], the same 64 m regions the virtual tier uses).
//! - **Respawn** (off by default, [`RespawnDef`]): every `interval_secs`, a region far from every
//!   player, that no player has been near for `unseen_secs` and that has fallen below `top_up_below`
//!   of its target, is topped up (never inside the player's spawn building).
//! - **Migration toward noise** ([`MigrationDef`]): a loud sound leaves an [`Attractor`] where it
//!   lures to. Its pull spreads outward at `spread_speed` (so far regions feel it minutes later, not
//!   at once), reaches `reach_mult` times the sound's radius (less from indoors, but not to nearly
//!   nothing like hearing through walls), and fades out over `life_secs`. When a virtual region's
//!   turn comes it feels the strongest pull on it ([`Population::pull_on`]: the per-region field)
//!   and a few of its wandering zombies set off for the attractor, as investigators, so they keep
//!   going when promoted. Near and active zombies only have their normal hearing.
//!
//! Everything here is deterministic: rolls are hashes of (seed, tick, id), attractors live in a
//! small vector in creation order, and [`Population::hash_into`] feeds the world's state hash.

use std::collections::BTreeMap;
use std::f32::consts::TAU;

use glam::{IVec2, Vec2};
use serde::{Deserialize, Serialize};

use crate::TICK_DT;
use crate::ai::ZombieTuning;
use crate::map::TileMap;
use crate::rng::hash01;
use crate::senses::Sound;
use crate::tiers::RegionIndex;
use crate::worldgen::{self, Town};
use crate::zombies::{ZombieState, Zombies};

/// What kind of ground a tile is, for population density. Decided by the town generator
/// ([`worldgen::zone_at`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[repr(u8)]
pub enum Zone {
    /// Outdoors in the middle of town (streets, yards and lots alike).
    TownCore = 0,
    /// Roads and sidewalks in town, outside the core.
    Street = 1,
    /// Other outdoor ground in town: front and back yards, lots.
    Yard = 2,
    /// Inside a building, anywhere (scaled per room type by [`PopulationDef::rooms`]).
    Indoors = 3,
    /// Roads out of town through the forest.
    ForestRoad = 4,
    /// Forest within [`PopulationDef::forest_edge_width`] of the town.
    ForestEdge = 5,
    /// Forest beyond that.
    #[default]
    DeepForest = 6,
}

impl Zone {
    pub const ALL: [Zone; 7] = [Zone::TownCore, Zone::Street, Zone::Yard, Zone::Indoors, Zone::ForestRoad, Zone::ForestEdge, Zone::DeepForest];

    /// The field name in the data file.
    pub fn name(self) -> &'static str {
        match self {
            Zone::TownCore => "town_core",
            Zone::Street => "street",
            Zone::Yard => "yard",
            Zone::Indoors => "indoors",
            Zone::ForestRoad => "forest_road",
            Zone::ForestEdge => "forest_edge",
            Zone::DeepForest => "deep_forest",
        }
    }

    /// One letter for ASCII maps.
    pub fn letter(self) -> char {
        match self {
            Zone::TownCore => 'C',
            Zone::Street => 'S',
            Zone::Yard => 'Y',
            Zone::Indoors => 'I',
            Zone::ForestRoad => 'R',
            Zone::ForestEdge => 'e',
            Zone::DeepForest => 'f',
        }
    }
}

/// Zombies per km² of open ground (tiles not blocked by walls, trees or furniture), per zone.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ZoneDensity {
    pub town_core: f32,
    pub street: f32,
    pub yard: f32,
    pub indoors: f32,
    pub forest_road: f32,
    pub forest_edge: f32,
    pub deep_forest: f32,
}

impl Default for ZoneDensity {
    fn default() -> Self {
        Self { town_core: 40_000.0, street: 32_000.0, yard: 19_000.0, indoors: 10_000.0, forest_road: 7_000.0, forest_edge: 2_600.0, deep_forest: 650.0 }
    }
}

impl ZoneDensity {
    pub fn get(&self, zone: Zone) -> f32 {
        match zone {
            Zone::TownCore => self.town_core,
            Zone::Street => self.street,
            Zone::Yard => self.yard,
            Zone::Indoors => self.indoors,
            Zone::ForestRoad => self.forest_road,
            Zone::ForestEdge => self.forest_edge,
            Zone::DeepForest => self.deep_forest,
        }
    }
}

/// Topping up emptied regions far from everyone. Off by default.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RespawnDef {
    pub enabled: bool,
    /// Seconds between checks.
    pub interval_secs: f32,
    /// Only regions whose nearest point is at least this far from every player (metres).
    pub min_distance: f32,
    /// A player within this distance of a region (its nearest point) has "seen" it...
    pub seen_distance: f32,
    /// ...and it isn't topped up until this many seconds after that.
    pub unseen_secs: f32,
    /// A region is topped up (back to its target) once it holds fewer than this fraction of it.
    pub top_up_below: f32,
    /// At most this many new zombies per region per check, and in all per check.
    pub max_per_region: u32,
    pub max_per_check: u32,
}

impl Default for RespawnDef {
    fn default() -> Self {
        Self { enabled: false, interval_secs: 120.0, min_distance: 250.0, seen_distance: 100.0, unseen_secs: 1800.0, top_up_below: 0.75, max_per_region: 6, max_per_check: 60 }
    }
}

/// Loud events drawing the far horde in over minutes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MigrationDef {
    pub enabled: bool,
    /// Sounds at least this loud (radius, metres) leave an attractor.
    pub min_radius: f32,
    /// An attractor pulls from this many times the sound's radius...
    pub reach_mult: f32,
    /// ...but never more than this (metres).
    pub max_reach: f32,
    /// A sound made indoors (or heard from a room) pulls from only this fraction of that.
    pub indoor_mult: f32,
    /// Seconds an attractor lasts (its pull fades to nothing over this time), and how long the
    /// lasting kind (MCP `make_noise` with `lasting`; later alarms, a helicopter) does.
    pub life_secs: f32,
    pub lasting_secs: f32,
    /// Metres per second the pull spreads outward from the attractor.
    pub spread_speed: f32,
    /// Chance per region update (every few seconds) that a wandering virtual zombie feeling the
    /// full pull sets off; scaled by the pull it feels (0..1).
    pub join_chance: f32,
    /// Migrants head for a point within this distance of the attractor (so they don't all pile
    /// onto one tile).
    pub gather_radius: f32,
    /// A loud sound this close to an existing attractor strengthens that one instead.
    pub merge_radius: f32,
    /// Attractors kept at once (the weakest gives way).
    pub max_attractors: u32,
}

impl Default for MigrationDef {
    fn default() -> Self {
        Self {
            enabled: true,
            min_radius: 25.0,
            reach_mult: 4.0,
            max_reach: 700.0,
            indoor_mult: 0.6,
            life_secs: 600.0,
            lasting_secs: 1800.0,
            spread_speed: 3.0,
            join_chance: 0.5,
            gather_radius: 12.0,
            merge_radius: 24.0,
            max_attractors: 16,
        }
    }
}

/// The population data file (`assets/data/population.ron`). Every field has a default (the
/// built-in values below), so a file only needs what it changes. Our own numbers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PopulationDef {
    pub density: ZoneDensity,
    /// Multipliers on `density.indoors` by room type (`kitchen`, `bedroom`, ...); others 1.
    pub rooms: BTreeMap<String, f32>,
    /// The town core is the civic and commercial blocks along the main street (the generator lays
    /// it out): this fraction of its length along the street, round its middle (1 = all of it).
    pub core_fraction: f32,
    /// Forest within this many metres of the town is forest edge; beyond is deep forest.
    pub forest_edge_width: f32,
    /// Multiplies every density (a sandbox "population" setting). A zombie count given to the
    /// world (`--zombies N`) overrides the total instead, keeping the proportions.
    pub scale: f32,
    /// Share of fast zombies, and walking speeds (m/s, uniform in the range) of fast and slow ones.
    pub fast_share: f32,
    pub fast_speed: (f32, f32),
    pub slow_speed: (f32, f32),
    pub respawn: RespawnDef,
    pub migration: MigrationDef,
}

impl Default for PopulationDef {
    fn default() -> Self {
        let rooms = [("kitchen", 1.3), ("living", 1.3), ("dining", 1.1), ("hall", 1.0), ("bedroom", 0.8), ("bathroom", 0.6), ("laundry", 0.7)];
        Self {
            density: ZoneDensity::default(),
            rooms: rooms.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
            core_fraction: 1.0,
            forest_edge_width: 60.0,
            scale: 1.0,
            fast_share: 0.1,
            fast_speed: (2.0, 3.0),
            slow_speed: (0.6, 1.1),
            respawn: RespawnDef::default(),
            migration: MigrationDef::default(),
        }
    }
}

/// Parse and validate a population file.
pub fn parse_population(text: &str) -> Result<PopulationDef, String> {
    let def: PopulationDef = ron::from_str(text).map_err(|e| e.to_string())?;
    def.validate()?;
    Ok(def)
}

impl PopulationDef {
    /// Explains the first mistake, if any.
    pub fn validate(&self) -> Result<(), String> {
        let finite = |name: &str, v: f32, lo: f32, hi: f32| {
            if v.is_finite() && (lo..=hi).contains(&v) { Ok(()) } else { Err(format!("{name} must be in {lo}..{hi}, got {v}")) }
        };
        for z in Zone::ALL {
            finite(&format!("density.{}", z.name()), self.density.get(z), 0.0, 1e6)?;
        }
        if Zone::ALL.iter().all(|z| self.density.get(*z) <= 0.0) {
            return Err("density: at least one zone needs zombies".into());
        }
        for (room, m) in &self.rooms {
            finite(&format!("rooms.{room}"), *m, 0.0, 100.0)?;
        }
        finite("core_fraction", self.core_fraction, 0.0, 1.0)?;
        finite("forest_edge_width", self.forest_edge_width, 0.0, 5000.0)?;
        finite("scale", self.scale, 0.0, 100.0)?;
        finite("fast_share", self.fast_share, 0.0, 1.0)?;
        for (name, (lo, hi)) in [("fast_speed", self.fast_speed), ("slow_speed", self.slow_speed)] {
            if !(lo.is_finite() && hi.is_finite() && 0.05 <= lo && lo <= hi && hi <= 10.0) {
                return Err(format!("{name} must be (min, max) m/s with 0.05 <= min <= max <= 10, got ({lo}, {hi})"));
            }
        }
        let r = &self.respawn;
        finite("respawn.interval_secs", r.interval_secs, 1.0, 1e6)?;
        finite("respawn.min_distance", r.min_distance, 0.0, 1e5)?;
        finite("respawn.seen_distance", r.seen_distance, 0.0, 1e5)?;
        finite("respawn.unseen_secs", r.unseen_secs, 0.0, 1e7)?;
        finite("respawn.top_up_below", r.top_up_below, 0.0, 1.0)?;
        if !(1..=10_000).contains(&r.max_per_region) || !(1..=100_000).contains(&r.max_per_check) {
            return Err(format!("respawn.max_per_region must be 1..10000 and max_per_check 1..100000, got {} and {}", r.max_per_region, r.max_per_check));
        }
        let m = &self.migration;
        finite("migration.min_radius", m.min_radius, 0.0, 1e4)?;
        finite("migration.reach_mult", m.reach_mult, 0.0, 50.0)?;
        finite("migration.max_reach", m.max_reach, 0.0, 1e5)?;
        finite("migration.indoor_mult", m.indoor_mult, 0.0, 1.0)?;
        finite("migration.life_secs", m.life_secs, 1.0, 1e6)?;
        finite("migration.lasting_secs", m.lasting_secs, 1.0, 1e7)?;
        finite("migration.spread_speed", m.spread_speed, 0.01, 1000.0)?;
        finite("migration.join_chance", m.join_chance, 0.0, 1.0)?;
        finite("migration.gather_radius", m.gather_radius, 0.0, 500.0)?;
        finite("migration.merge_radius", m.merge_radius, 0.0, 1000.0)?;
        if !(1..=64).contains(&m.max_attractors) {
            return Err(format!("migration.max_attractors must be 1..64, got {}", m.max_attractors));
        }
        Ok(())
    }

    /// The zone of tile `t` on a map with `town` on it.
    pub fn zone_at(&self, map: &TileMap, town: &Town, t: IVec2) -> Zone {
        worldgen::zone_at(map, town, t, self.core_fraction, self.forest_edge_width)
    }

    /// Zombies per km² on tile `t` (0 where nothing can stand), `scale` included.
    pub fn tile_density(&self, map: &TileMap, town: &Town, t: IVec2) -> f32 {
        if !map.in_bounds(t) || map.tile_blocked(t) {
            return 0.0;
        }
        let zone = self.zone_at(map, town, t);
        let mut d = self.density.get(zone);
        if zone == Zone::Indoors {
            let room = &map.rooms[map.tile(t).room as usize].room_type;
            if let Some(m) = self.rooms.get(room) {
                d *= m;
            }
        }
        d * self.scale
    }

    /// The highest density any tile can have (for rejection sampling).
    pub fn max_density(&self) -> f32 {
        let rooms = self.rooms.values().copied().fold(1.0f32, f32::max);
        Zone::ALL.iter().map(|z| self.density.get(*z) * if *z == Zone::Indoors { rooms } else { 1.0 }).fold(0.0, f32::max) * self.scale
    }

    /// Walking speed for a new zombie from two rolls in [0, 1).
    pub fn speed(&self, share_roll: f32, speed_roll: f32) -> f32 {
        let (lo, hi) = if share_roll < self.fast_share { self.fast_speed } else { self.slow_speed };
        lo + (hi - lo) * speed_roll
    }
}

/// A loud event the far horde drifts toward. Its pull spreads outward at
/// [`MigrationDef::spread_speed`] up to `reach` and fades linearly over `life_secs`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Attractor {
    /// Stable id (never reused within a world).
    pub id: u32,
    /// Where migrants head (the sound's lure point).
    pub pos: Vec2,
    pub reach: f32,
    /// Tick it was made.
    pub born: u64,
    /// Seconds from `born` until it's gone (extended when a new loud sound merges into it).
    pub life_secs: f32,
}

impl Attractor {
    pub fn age_secs(&self, tick: u64) -> f32 {
        tick.saturating_sub(self.born) as f32 * TICK_DT
    }

    /// 1 when new, falling to 0 at the end of its life.
    pub fn strength(&self, tick: u64) -> f32 {
        (1.0 - self.age_secs(tick) / self.life_secs).clamp(0.0, 1.0)
    }

    /// How far out its pull has spread so far.
    pub fn felt_radius(&self, tick: u64, spread_speed: f32) -> f32 {
        (self.age_secs(tick) * spread_speed).min(self.reach)
    }

    /// Pull (0..1) felt `d` metres away: none beyond the spread so far, else its strength, falling
    /// off toward the edge of its reach.
    pub fn pull(&self, d: f32, tick: u64, spread_speed: f32) -> f32 {
        if d > self.felt_radius(tick, spread_speed) || self.reach <= 0.0 {
            return 0.0;
        }
        let f = d / self.reach;
        self.strength(tick) * (1.0 - f * f)
    }
}

/// What a region feels: the attractor pulling it hardest (by id; 0 = none) and how hard (0..1).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Pull {
    pub attractor: u32,
    pub strength: f32,
    pub toward: Vec2,
}

/// "Never seen by a player" in [`Population::last_seen`].
pub const NEVER: u64 = u64::MAX;

/// The world's population state: per-region targets and sightings, and the attractors.
#[derive(Clone, Debug, Default)]
pub struct Population {
    /// Per region (the virtual tier's [`RegionIndex`]): how many zombies it should hold.
    pub target: Vec<f32>,
    /// Per region: the zone with the most open ground in it.
    pub zone: Vec<Zone>,
    /// Per region: the last tick a player was within `respawn.seen_distance` of it ([`NEVER`]).
    pub last_seen: Vec<u64>,
    /// Live attractors, in creation (id) order.
    pub attractors: Vec<Attractor>,
    pub(crate) next_attractor: u32,
    /// Zombies added by respawn so far.
    pub respawned: u32,
    /// Living zombies per region, filled by [`Population::count_into`] (scratch).
    pub(crate) counts: Vec<u32>,
}

impl Population {
    /// Targets and zones per region for `map`, from the densities. `spawnable(t)` rules out tiles
    /// nothing should spawn on (the player's start). Targets are the natural numbers (density ×
    /// area); scale them with [`Population::scale_targets`] to hit a given total.
    pub fn new(map: &TileMap, regions: &RegionIndex, def: &PopulationDef, town: &Town, spawnable: impl Fn(IVec2) -> bool) -> Self {
        let n = regions.regions();
        let mut target = vec![0.0f64; n];
        let mut zone_tiles = vec![[0u32; 7]; n];
        for y in 0..map.height() {
            for x in 0..map.width() {
                let t = IVec2::new(x, y);
                if map.tile_blocked(t) {
                    continue;
                }
                let r = regions.region_of(t.as_vec2() + Vec2::splat(0.5));
                zone_tiles[r][def.zone_at(map, town, t) as usize] += 1;
                if spawnable(t) {
                    target[r] += def.tile_density(map, town, t) as f64 * 1e-6;
                }
            }
        }
        let zone = zone_tiles
            .iter()
            .map(|c| {
                // Most open ground; ties go to the earlier (denser-sounding) zone.
                let best = (0..7).rev().max_by_key(|&k| c[k]).unwrap_or(6);
                if c[best] == 0 { Zone::DeepForest } else { Zone::ALL[best] }
            })
            .collect();
        Self { target: target.iter().map(|t| *t as f32).collect(), zone, last_seen: vec![NEVER; n], ..Default::default() }
    }

    /// Sum of the region targets.
    pub fn total_target(&self) -> f32 {
        self.target.iter().sum()
    }

    pub fn scale_targets(&mut self, k: f32) {
        self.target.iter_mut().for_each(|t| *t *= k);
    }

    /// Living zombies per region (every tier) into `self.counts`.
    pub fn count_into(&mut self, z: &Zombies, regions: &RegionIndex) {
        self.counts.clear();
        self.counts.resize(regions.regions(), 0);
        for i in 0..z.len() {
            if z.state[i] != ZombieState::Dead {
                self.counts[regions.region_of(z.pos[i])] += 1;
            }
        }
    }

    /// Note which regions players are close to this tick.
    pub fn update_seen(&mut self, regions: &RegionIndex, players: impl Iterator<Item = Vec2> + Clone, seen_distance: f32, tick: u64) {
        let d2 = seen_distance * seen_distance;
        for r in 0..self.last_seen.len() {
            let (lo, hi) = regions.rect(r);
            if players.clone().any(|p| p.clamp(lo, hi).distance_squared(p) <= d2) {
                self.last_seen[r] = tick;
            }
        }
    }

    /// Loud sounds heard this tick leave (or strengthen) attractors.
    pub fn hear(&mut self, sounds: &[Sound], map: &TileMap, def: &MigrationDef, tick: u64) {
        if !def.enabled {
            return;
        }
        for s in sounds {
            if s.radius >= def.min_radius {
                let indoors = map.room_at(s.pos) != 0 || map.room_at(s.lure) != 0;
                self.attract(s.lure, s.radius, indoors, def.life_secs, def, tick);
            }
        }
    }

    /// Leave an attractor at `pos` for a sound of `radius` lasting `life_secs`. Within
    /// `merge_radius` of an existing one, that one is strengthened instead (wider reach, longer
    /// life; its pull keeps spreading from when it began, so a siren going off every few seconds
    /// keeps reaching farther). With no room, the weakest gives way.
    pub fn attract(&mut self, pos: Vec2, radius: f32, indoors: bool, life_secs: f32, def: &MigrationDef, tick: u64) {
        let reach = (radius * def.reach_mult * if indoors { def.indoor_mult } else { 1.0 }).min(def.max_reach);
        if reach.is_nan() || reach <= 0.0 || !pos.is_finite() {
            return;
        }
        if let Some(a) = self.attractors.iter_mut().find(|a| a.pos.distance(pos) <= def.merge_radius) {
            a.reach = a.reach.max(reach);
            a.life_secs = a.life_secs.max(a.age_secs(tick) + life_secs);
            return;
        }
        self.next_attractor += 1;
        let new = Attractor { id: self.next_attractor, pos, reach, born: tick, life_secs };
        if self.attractors.len() >= def.max_attractors as usize {
            let weakest = (0..self.attractors.len())
                .min_by(|&a, &b| {
                    let w = |k: usize| self.attractors[k].strength(tick) * self.attractors[k].reach;
                    w(a).total_cmp(&w(b))
                })
                .unwrap_or(0);
            self.attractors.remove(weakest);
        }
        self.attractors.push(new);
    }

    /// Drop attractors that have faded out.
    pub fn expire(&mut self, tick: u64) {
        self.attractors.retain(|a| a.strength(tick) > 0.0);
    }

    /// The strongest pull on region `r` (the per-region field), from its nearest point to each
    /// attractor.
    pub fn pull_on(&self, regions: &RegionIndex, r: usize, def: &MigrationDef, tick: u64) -> Pull {
        let mut best = Pull::default();
        if !def.enabled {
            return best;
        }
        let (lo, hi) = regions.rect(r);
        for a in &self.attractors {
            let s = a.pull(a.pos.clamp(lo, hi).distance(a.pos), tick, def.spread_speed);
            if s > best.strength {
                best = Pull { attractor: a.id, strength: s, toward: a.pos };
            }
        }
        best
    }

    /// Feed everything here into a state hash.
    pub fn hash_into(&self, mix: &mut impl FnMut(u32)) {
        for a in &self.attractors {
            mix(a.id);
            mix(a.pos.x.to_bits());
            mix(a.pos.y.to_bits());
            mix(a.reach.to_bits());
            mix(a.born as u32);
            mix((a.born >> 32) as u32);
            mix(a.life_secs.to_bits());
        }
        mix(self.next_attractor);
        mix(self.respawned);
        for &s in &self.last_seen {
            mix(s as u32);
            mix((s >> 32) as u32);
        }
    }
}

/// Migration for one virtual region on its turn: each wandering zombie in it (not bored) sets off
/// for the attractor pulling the region with a chance of `join_chance` × the pull, heading for a
/// point within `gather_radius` of it as an investigator (with time enough to get there).
pub(crate) fn migrate_region(z: &mut Zombies, members: &[u32], pull: Pull, def: &MigrationDef, t: &ZombieTuning, tick: u64, seed: u64) {
    if pull.strength <= 0.0 {
        return;
    }
    let chance = def.join_chance * pull.strength;
    for &i in members {
        let i = i as usize;
        if z.state[i] != ZombieState::Wander || z.bored[i] > 0.0 {
            continue;
        }
        let roll = |k: u64| hash01(seed ^ 0x4D49_4752, tick, z.id[i] as u64, k);
        if roll(0) >= chance {
            continue;
        }
        let target = pull.toward + Vec2::from_angle(roll(1) * TAU) * def.gather_radius * roll(2).sqrt();
        let travel = z.pos[i].distance(target) / (z.speed[i] * t.investigate_speed_mult).max(0.1);
        z.state[i] = ZombieState::Investigate;
        z.target_player[i] = 0;
        z.target_pos[i] = target;
        z.timer[i] = t.investigate_secs + travel * 1.5;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn population_file_is_valid() {
        let def = parse_population(include_str!("../../../assets/data/population.ron")).unwrap();
        assert!(def.density.town_core > def.density.deep_forest * 10.0);
        assert!(!def.respawn.enabled, "respawn should be off by default");
        assert!(def.migration.enabled);
        // A file only needs what it changes.
        let partial = parse_population("(density: (deep_forest: 1.0))").unwrap();
        assert_eq!(partial.density.deep_forest, 1.0);
        assert_eq!(partial.density.town_core, ZoneDensity::default().town_core);
        assert_eq!(partial.migration, MigrationDef::default());
    }

    #[test]
    fn validation_explains_mistakes() {
        let bad = |text: &str, want: &str| {
            let e = parse_population(text).unwrap_err();
            assert!(e.contains(want), "{text}: {e}");
        };
        bad("(density: (street: -1.0))", "density.street");
        bad("(fast_share: 1.5)", "fast_share");
        bad("(slow_speed: (2.0, 1.0))", "slow_speed");
        bad("(respawn: (top_up_below: 2.0))", "top_up_below");
        bad("(migration: (max_attractors: 0))", "max_attractors");
        bad("(densty: ())", "densty");
        bad("(density: (town_core: 0.0, street: 0.0, yard: 0.0, indoors: 0.0, forest_road: 0.0, forest_edge: 0.0, deep_forest: 0.0))", "at least one");
    }

    #[test]
    fn attractors_spread_fade_and_merge() {
        let def = MigrationDef::default();
        let mut p = Population::default();
        p.attract(Vec2::new(100.0, 100.0), 50.0, false, def.life_secs, &def, 0);
        let a = p.attractors[0];
        assert_eq!(a.reach, 50.0 * def.reach_mult);
        // Nothing far away at first; the pull spreads out, then fades.
        assert_eq!(a.pull(150.0, 1, def.spread_speed), 0.0);
        let when = (150.0 / def.spread_speed / TICK_DT) as u64 + 2;
        assert!(a.pull(150.0, when, def.spread_speed) > 0.0);
        assert!(a.pull(150.0, when, def.spread_speed) < a.pull(10.0, when, def.spread_speed));
        let end = (def.life_secs / TICK_DT) as u64 + 1;
        assert_eq!(a.pull(10.0, end, def.spread_speed), 0.0);
        // Indoors: muffled, but far from nothing.
        p.attract(Vec2::new(500.0, 500.0), 50.0, true, def.life_secs, &def, 0);
        assert_eq!(p.attractors[1].reach, 50.0 * def.reach_mult * def.indoor_mult);
        // Another loud sound close by strengthens the first instead of adding one.
        p.attract(Vec2::new(110.0, 100.0), 100.0, false, def.life_secs, &def, 200);
        assert_eq!(p.attractors.len(), 2);
        assert_eq!(p.attractors[0].reach, 100.0 * def.reach_mult);
        assert_eq!(p.attractors[0].born, 0, "the spread keeps going from the first");
        assert!(p.attractors[0].life_secs > def.life_secs);
        p.expire(end);
        assert_eq!(p.attractors.len(), 1, "the indoor one faded");
        // Full: the weakest gives way.
        for k in 0..def.max_attractors + 3 {
            p.attract(Vec2::new(1000.0 + k as f32 * 100.0, 0.0), 30.0 + k as f32, false, def.life_secs, &def, end);
        }
        assert_eq!(p.attractors.len(), def.max_attractors as usize);
        assert!(!p.attractors.iter().any(|a| a.id == 1), "the old, nearly faded one should give way");
    }
}
