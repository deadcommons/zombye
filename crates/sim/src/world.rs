//! The world: generated map + players + the zombies hunting them, advanced one fixed tick at a time.
//!
//! A tick: players move (making footstep noise), their swings, shoves and stomps that are due land
//! ([`crate::combat`]), zombies look and listen ([`senses`]), the flow field cache spends its build
//! budget, then every zombie runs its state machine and moves ([`ai`]), then the doors and windows
//! they battered take damage (and maybe break), then their strikes on players roll for damage.
//! Sounds made during a tick (groans, thumps, doors, debug noise) are heard on the next one;
//! footsteps and swings are heard in the same tick.
//!
//! Shut doors and intact windows have hit points, stored sparsely: only damaged edges have an
//! entry, in a sorted map so anything that walks it is deterministic.

use std::collections::{BTreeMap, BTreeSet};
use std::f32::consts::TAU;
use std::time::Instant;

use glam::{IVec2, Vec2};

use crate::TICK_DT;
use crate::ai::{self, Ctx, GOAL_RANGE, ZombieAttack, ZombieThump, ZombieTuning};
use crate::combat::{ActionEvent, CombatEvent, CombatTuning, HitEvent};
use crate::containers::{self, ContainerData, ContainerSite};
use crate::health::{HealthTuning, InfectionTuning};
use crate::inventory::{CarryTuning, ItemNote};
use crate::items::{self, GroundItem, ItemDef, ItemStack, WeaponCategory, WeaponDef};
use crate::players::Action;
use crate::flow::{FlowCache, FlowField, WalkGrid};
use crate::map::{Edge, EdgeChange, EdgeRef, OwnedEdge, TileMap};
use crate::nav::Nav;
use crate::players::{self, PLAYER_RADIUS, Player, PlayerInput, Stance};
use crate::population::{NEVER, Population, PopulationDef, Zone};
use crate::rng::{Rng, hash01};
use crate::senses::{self, Sound, SoundSource, Target};
use crate::spatial::SpatialHash;
use crate::template::BuildingTemplate;
use crate::tiers::{self, ChunkDistances, NearCtx, RegionIndex, TierTuning};
use crate::worldgen::{self, GenConfig};
use crate::zombies::{Tier, ZombieState, Zombies};

pub use crate::ai::ZOMBIE_RADIUS;

/// Minimum ticks between rebuilds of a moving player's flow field.
const FLOW_REBUILD_TICKS: u64 = 4;
/// Flow fields kept at once, and how many may be built per tick.
const FLOW_CACHE_SIZE: usize = 40;
const FLOW_BUILDS_PER_TICK: u32 = 3;
/// Path range of a player's field and of a noise / last-known-position field.
const PLAYER_FLOW_RANGE: f32 = 64.0;
const POINT_FLOW_RANGE: f32 = 40.0;
/// Sounds kept per tick (debug spam beyond this is dropped).
pub(crate) const MAX_SOUNDS: usize = 512;
/// No zombies spawn this close to the player's starting point.
const SPAWN_CLEAR_RADIUS: f32 = 14.0;
/// How far a door or window can be from the player using it (to the middle of the edge).
pub const INTERACT_REACH: f32 = 1.6;
/// A player aims interactions this far ahead, so the door you face wins over one behind you.
const INTERACT_AIM_AHEAD: f32 = 0.6;

#[derive(Clone, Debug)]
pub struct WorldConfig {
    pub generation: GenConfig,
    /// How many zombies to place at the start. `None`: as many as the population densities give
    /// for this map (`population`); a number keeps their proportions but sets the total.
    pub zombies: Option<u32>,
    pub templates: Vec<BuildingTemplate>,
    /// Hour of day at tick 0.
    pub start_hour: f32,
    /// Real seconds per in-game day.
    pub day_length_secs: f32,
    /// Zombie senses and behaviour.
    pub zombie: ZombieTuning,
    /// Melee weapons (`assets/data/items/weapons.ron`). Bare hands are added if missing.
    pub weapons: Vec<WeaponDef>,
    /// Every item (`assets/data/items/items.ron`). A weapon without an item gets a plain one
    /// ([`items::complete_items`]).
    pub items: Vec<ItemDef>,
    /// Which furniture holds things (`assets/data/containers.ron`).
    pub containers: ContainerData,
    /// Carry limits, burden and how long handling items takes.
    pub carry: CarryTuning,
    /// Player health and every combat number that isn't a weapon's.
    pub combat: CombatTuning,
    /// Wounds, bleeding, festering, food poisoning ([`crate::health`]).
    pub health: HealthTuning,
    /// The zombie infection's sandbox settings (fatal by default).
    pub infection: InfectionTuning,
    /// Simulation tiers: ranges and update rates (see [`crate::tiers`]).
    pub tiers: TierTuning,
    /// Where zombies live, respawn and migration (`assets/data/population.ron`, see
    /// [`crate::population`]).
    pub population: PopulationDef,
}

impl WorldConfig {
    pub fn new(seed: u64, zombies: u32, templates: Vec<BuildingTemplate>) -> Self {
        Self {
            generation: GenConfig { seed, ..Default::default() },
            zombies: Some(zombies),
            templates,
            start_hour: 9.0,
            day_length_secs: 3600.0,
            zombie: ZombieTuning::default(),
            weapons: Vec::new(),
            items: Vec::new(),
            containers: ContainerData::default(),
            carry: CarryTuning::default(),
            combat: CombatTuning::default(),
            health: HealthTuning::default(),
            infection: InfectionTuning::default(),
            tiers: TierTuning::default(),
            population: PopulationDef::default(),
        }
    }

    /// A square map `chunks` × `chunks` chunks (16 m each) instead of the default; the town stays
    /// the same size in the middle, with more forest round it.
    pub fn with_map_chunks(mut self, chunks: i32) -> Self {
        self.generation.chunks = IVec2::splat(chunks.max(1));
        self
    }

    pub fn with_weapons(mut self, weapons: Vec<WeaponDef>) -> Self {
        self.weapons = weapons;
        self
    }

    pub fn with_items(mut self, items: Vec<ItemDef>) -> Self {
        self.items = items;
        self
    }

    pub fn with_containers(mut self, containers: ContainerData) -> Self {
        self.containers = containers;
        self
    }

    pub fn with_population(mut self, population: PopulationDef) -> Self {
        self.population = population;
        self
    }

    pub fn seed(&self) -> u64 {
        self.generation.seed
    }
}

/// Timings of the last tick, for profiling via MCP/HUD.
#[derive(Clone, Copy, Debug, Default)]
pub struct TickStats {
    pub tick_ms: f32,
    /// Senses plus flow-field builds.
    pub senses_ms: f32,
    pub flow_ms: f32,
    /// Time making plans over the navigation graph, plans made and nodes expanded (`crate::nav`).
    pub nav_ms: f32,
    pub nav_plans: u32,
    pub nav_expanded: u32,
    /// Zombies chasing, lunging at or attacking a player.
    pub chasing: u32,
    pub investigating: u32,
    pub sight_checks: u32,
    pub sounds: u32,
    pub flow_builds: u32,
    /// Zombies battering a door or window, thumps landed, and edges broken this tick.
    pub thumping: u32,
    pub thumps: u32,
    pub breaks: u32,
    /// Zombies staggered or down.
    pub reeling: u32,
    /// Zombies on each tier after this tick's tier pass (corpses included), and how many changed
    /// tier in it.
    pub active: u32,
    pub near: u32,
    pub virtual_: u32,
    pub promoted: u32,
    pub demoted: u32,
    /// Near zombies updated this tick, and virtual zombies moved.
    pub near_updated: u32,
    pub virtual_updated: u32,
    /// Zombies added by respawn this tick, and attractors alive after it.
    pub respawned: u32,
    pub attractors: u32,
    /// Time for the tier pass plus rebuilding the spatial hash and the region index; the active
    /// tier's state machine and movement (senses and flow fields are separate, above); the near
    /// tier's update; the virtual tier's hearing and bulk moves.
    pub tier_ms: f32,
    pub active_ms: f32,
    pub near_ms: f32,
    pub virtual_ms: f32,
}

pub struct World {
    pub config: WorldConfig,
    pub map: TileMap,
    pub town_min: IVec2,
    pub town_max: IVec2,
    pub player_spawn: Vec2,
    pub spawn_building: usize,
    pub zombies: Zombies,
    /// Players in join order.
    pub players: Vec<Player>,
    pub tick: u64,
    pub stats: TickStats,
    /// Map edits since the last snapshot (door toggles), for replication.
    pub edge_changes: Vec<EdgeChange>,
    /// Hours added by time skips.
    pub(crate) clock_offset_hours: f32,
    /// Zombie strikes this tick (cleared at the start of each tick).
    pub attack_events: Vec<ZombieAttack>,
    /// Zombie thumps on doors and windows this tick, in the order they were applied.
    pub thump_events: Vec<ZombieThump>,
    /// Doors and windows broken this tick (by zombies or smashed by players).
    pub break_events: Vec<EdgeChange>,
    /// Player swings, shoves and stomps that landed this tick, and what each hit.
    pub action_events: Vec<ActionEvent>,
    pub hit_events: Vec<HitEvent>,
    /// Hits, kills, shoves, stomps and wounds since the last snapshot, for clients' hit feedback
    /// (blood, flashes, screen shake). Drained by whoever replicates the world, like
    /// `edge_changes`; at most [`crate::combat::MAX_COMBAT_EVENTS`] are kept if nobody does. Not simulation
    /// state (not in the state hash).
    pub combat_events: Vec<CombatEvent>,
    /// Items lying about, in id order.
    pub(crate) items: Vec<GroundItem>,
    pub(crate) next_item_id: u32,
    /// Per item in `config.items`, the weapon it swings as (derived from the content).
    pub(crate) item_weapons: Vec<Option<u16>>,
    /// Every container in the map, in id order (derived from the map and `config.containers`).
    pub(crate) sites: Vec<ContainerSite>,
    /// What opened containers hold, by container id; unopened ones aren't here (they fill on first
    /// open, from the seed). Sorted, so walking it is deterministic.
    pub(crate) opened: BTreeMap<u32, Vec<ItemStack>>,
    /// What players asked of their items and how it went, for their clients (not simulation state;
    /// drained by whoever replicates the world, at most [`crate::inventory::MAX_ITEM_NOTES`] kept).
    pub item_notes: Vec<ItemNote>,
    /// Items eaten or drunk this tick: (player, item). For the needs (#11).
    pub used_items: Vec<(u32, u16)>,
    /// Players whose inventory, open container or job queue changed since the last snapshot took
    /// them (replication, not simulation state).
    pub(crate) private_dirty: BTreeSet<u32>,
    /// Index of bare hands in `config.weapons`.
    pub(crate) unarmed: u16,
    /// Hit points of damaged doors and windows, by [`EdgeRef::key`]. Missing = undamaged.
    pub(crate) edge_hp: BTreeMap<(i32, i32, u8), f32>,
    /// Edges whose hit points changed since the last [`take_edge_damage`](Self::take_edge_damage),
    /// for replication (not simulation state).
    pub(crate) edge_hp_dirty: BTreeSet<(i32, i32, u8)>,
    /// Corpses removed by the last tick, by id (cleared each tick).
    pub removed_corpses: Vec<u32>,
    /// Sounds made since the last tick; heard (and moved to `heard`) by the next one.
    pub(crate) sounds: Vec<Sound>,
    /// Sounds zombies listened to in the last tick.
    pub(crate) heard: Vec<Sound>,
    /// Debug override (MCP `set_goal`): when set, zombies head for this tile and ignore the players.
    pub(crate) goal: Option<IVec2>,
    pub(crate) goal_flow: Option<FlowField>,
    pub(crate) goal_dirty: bool,
    /// Walkable steps per tile, for flow-field searches. Kept in step with `map`.
    pub(crate) walk: WalkGrid,
    pub(crate) flow_cache: FlowCache,
    /// The navigation graph (derived from `walk`) and the shared long-range plans (saved).
    pub(crate) nav: Nav,
    /// Players as the senses see them this tick.
    pub(crate) targets: Vec<Target>,
    pub(crate) rng: Rng,
    /// Combat rolls (hits, crits, staggers, zombie strikes), apart from the AI's stream.
    pub(crate) combat_rng: Rng,
    pub(crate) hash: SpatialHash,
    /// Per chunk, how far the nearest player is (for tiers); virtual zombies by region.
    pub(crate) chunk_dist: ChunkDistances,
    pub(crate) regions: RegionIndex,
    /// Per-region population targets and sightings, and the attractors drawing the far horde.
    pub(crate) population: Population,
    /// Map edits made through the world (doors and windows opened, shut, broken), by
    /// [`EdgeRef::key`]: a save stores these edges on top of the map regenerated from the seed.
    pub(crate) map_edits: BTreeSet<(i32, i32, u8)>,
    /// The map was edited directly (tests, editors; see [`map_changed`](Self::map_changed)), so a
    /// save stores all of it instead of the edits.
    pub(crate) map_custom: bool,
    /// Hash of the map as generated, so loading can tell the generator (or its data) changed.
    pub(crate) map_base_hash: u64,
    pub(crate) scratch_tiles: Vec<IVec2>,
    pub(crate) next_id: u32,
    pub(crate) next_player_id: u32,
    pub(crate) scratch_pos: Vec<Vec2>,
    pub(crate) scratch_actions: Vec<(usize, Action)>,
    pub(crate) scratch_hits: Vec<(f32, usize)>,
}

/// Bare hands are added to weapons that have none (as the world does on creation).
pub(crate) fn add_fists(weapons: &mut Vec<WeaponDef>) {
    if !weapons.iter().any(|w| w.category == WeaponCategory::Unarmed) {
        weapons.push(WeaponDef::fists());
    }
}

/// The edge an [`EdgeRef::key`] names.
pub(crate) fn edge_of_key((x, y, w): (i32, i32, u8)) -> EdgeRef {
    EdgeRef::new(IVec2::new(x, y), if w == 0 { OwnedEdge::North } else { OwnedEdge::West })
}

impl World {
    /// Generate the map from the config's seed and templates and scatter the zombies over it.
    pub fn new(config: WorldConfig) -> Self {
        let mut world = Self::generate(config);
        world.nav = Nav::new(&world.walk);
        world.populate();
        world
    }

    /// The generated world before any zombies, and without its navigation graph (loading a save
    /// starts from this, and builds the graph once the map edits are in).
    pub(crate) fn generate(mut config: WorldConfig) -> Self {
        add_fists(&mut config.weapons);
        let weapon_items = items::complete_items(&mut config.items, &config.weapons);
        let item_weapons = (0..config.items.len()).map(|i| weapon_items.iter().position(|w| *w == Some(i as u16)).map(|w| w as u16)).collect();
        let unarmed = config.weapons.iter().position(|w| w.category == WeaponCategory::Unarmed).unwrap_or(0) as u16;
        let generated = worldgen::generate(&config.generation, &config.templates);
        let items = items::place_weapons(&generated.map, &config.weapons, &weapon_items, generated.spawn_building, config.generation.seed, 1);
        let sites = containers::find_sites(&generated.map, &config.containers);
        let map = generated.map;
        let hash = SpatialHash::new(map.width() as f32, map.height() as f32, 1.0);
        let regions = RegionIndex::new(&map, config.tiers.region_size);
        let walk = WalkGrid::new(&map);
        let nav = Nav::default();
        let mut world = Self {
            rng: Rng::new(config.generation.seed),
            combat_rng: Rng::new(config.generation.seed ^ 0xC0_4BA7),
            next_item_id: items.len() as u32 + 1,
            items,
            item_weapons,
            sites,
            opened: BTreeMap::new(),
            item_notes: Vec::new(),
            used_items: Vec::new(),
            private_dirty: BTreeSet::new(),
            unarmed,
            action_events: Vec::new(),
            hit_events: Vec::new(),
            combat_events: Vec::new(),
            scratch_actions: Vec::new(),
            scratch_hits: Vec::new(),
            town_min: generated.town_min,
            town_max: generated.town_max,
            player_spawn: generated.player_spawn,
            spawn_building: generated.spawn_building,
            map,
            hash,
            chunk_dist: ChunkDistances::default(),
            population: Population::default(),
            regions,
            scratch_tiles: Vec::new(),
            zombies: Zombies::default(),
            players: Vec::new(),
            tick: 0,
            stats: TickStats::default(),
            edge_changes: Vec::new(),
            clock_offset_hours: 0.0,
            attack_events: Vec::new(),
            thump_events: Vec::new(),
            break_events: Vec::new(),
            edge_hp: BTreeMap::new(),
            edge_hp_dirty: BTreeSet::new(),
            removed_corpses: Vec::new(),
            sounds: Vec::new(),
            heard: Vec::new(),
            goal: None,
            goal_flow: None,
            goal_dirty: false,
            walk,
            nav,
            flow_cache: FlowCache::new(FLOW_CACHE_SIZE, FLOW_BUILDS_PER_TICK, PLAYER_FLOW_RANGE, POINT_FLOW_RANGE, FLOW_REBUILD_TICKS),
            targets: Vec::new(),
            next_id: 1,
            next_player_id: 1,
            scratch_pos: Vec::new(),
            map_edits: BTreeSet::new(),
            map_custom: false,
            map_base_hash: 0,
            config,
        };
        world.map_base_hash = crate::save::map_hash(&world.map);
        world
    }

    /// Can a zombie be placed on tile `t` at the start or by respawn? Not near where the player
    /// wakes up, nor in that house.
    fn spawnable(&self, t: IVec2) -> bool {
        let c = t.as_vec2() + Vec2::splat(0.5);
        c.distance(self.player_spawn) >= SPAWN_CLEAR_RADIUS && self.map.building_at(c) != Some(self.spawn_building)
    }

    /// Work out every region's target from the population densities, then scatter zombies over
    /// the map by density (busy streets, quiet forest): `config.zombies` of them, or the natural
    /// number for this map.
    fn populate(&mut self) {
        let def = &self.config.population;
        let town = (self.town_min, self.town_max);
        let mut pop = Population::new(&self.map, &self.regions, def, town, |t| self.spawnable(t));
        let natural = pop.total_target();
        let n = self.config.zombies.unwrap_or(natural.round() as u32);
        if natural > 0.0 {
            pop.scale_targets(n as f32 / natural);
        }
        self.population = pop;
        let max = def.max_density();
        if max <= 0.0 {
            return;
        }
        let (w, h) = (self.map.width() as u32, self.map.height() as u32);
        let mut spawned = 0;
        let mut attempts = 0u64;
        while spawned < n && attempts < n as u64 * 2000 {
            attempts += 1;
            let t = IVec2::new(self.rng.below(w) as i32, self.rng.below(h) as i32);
            let weight = self.config.population.tile_density(&self.map, town, t) / max;
            if self.rng.f32() >= weight {
                continue;
            }
            let p = t.as_vec2() + Vec2::new(self.rng.range(0.2, 0.8), self.rng.range(0.2, 0.8));
            if !self.spawnable(t) {
                continue;
            }
            self.push_zombie(p);
            spawned += 1;
        }
    }

    fn push_zombie(&mut self, p: Vec2) -> usize {
        // Clear of trunks and walls from the start (it may not get a full update for a while).
        let p = self.map.resolve_circle(p, ZOMBIE_RADIUS);
        let facing = self.rng.range(0.0, TAU);
        // Mostly shamblers, a few faster ones.
        let (share, roll) = (self.rng.f32(), self.rng.f32());
        let speed = self.config.population.speed(share, roll);
        self.add_zombie(p, facing, speed)
    }

    /// Add one zombie exactly here (tests, debug tools). Its hit points come from a hash of its id,
    /// so they don't touch the spawn RNG stream. Returns its index.
    pub fn add_zombie(&mut self, pos: Vec2, facing: f32, speed: f32) -> usize {
        let (lo, hi) = self.config.combat.zombie_hp;
        let hp = lo + (hi - lo) * hash01(self.config.seed(), self.next_id as u64, 0x4850, 0);
        self.zombies.push(self.next_id, pos, facing, speed, hp);
        self.next_id += 1;
        self.zombies.len() - 1
    }

    /// Spawn `n` zombies on open tiles within `radius` of `centre`.
    pub fn spawn_zombies(&mut self, n: u32, centre: Vec2, radius: f32) -> u32 {
        let mut spawned = 0;
        let mut attempts = 0;
        while spawned < n && attempts < n * 20 {
            attempts += 1;
            let a = self.rng.range(0.0, TAU);
            let r = radius * self.rng.f32().sqrt();
            let p = centre + Vec2::from_angle(a) * r;
            if self.map.tile_blocked(p.floor().as_ivec2()) {
                continue;
            }
            self.push_zombie(p);
            spawned += 1;
        }
        spawned
    }

    /// Add a player at `pos` (pushed out of walls and furniture). Returns its id.
    pub fn add_player(&mut self, pos: Vec2) -> u32 {
        let id = self.next_player_id;
        self.next_player_id += 1;
        let pos = self.map.resolve_circle(pos, PLAYER_RADIUS);
        self.players.push(Player::new(id, pos, self.config.combat.player_hp));
        self.private_dirty.insert(id);
        id
    }

    pub fn player(&self, id: u32) -> Option<&Player> {
        self.players.iter().find(|p| p.id == id)
    }

    /// Store a player's latest input; it is applied on every tick until a newer one arrives.
    /// Sequence numbers start at 1; stale or repeated ones are ignored. Returns whether it was taken.
    pub fn set_player_input(&mut self, id: u32, seq: u32, input: PlayerInput) -> bool {
        let Some(p) = self.players.iter_mut().find(|p| p.id == id) else { return false };
        if seq <= p.last_input_seq {
            return false;
        }
        p.input = input.sanitized();
        p.last_input_seq = seq;
        true
    }

    /// The debug goal override, if set.
    pub fn goal(&self) -> Option<IVec2> {
        self.goal
    }

    /// Debug override: every zombie within [`GOAL_RANGE`] (path distance) of this point heads for
    /// it, ignoring sight and sound. `None` goes back to normal senses.
    pub fn set_goal(&mut self, goal: Option<Vec2>) {
        self.goal = goal.map(|p| p.floor().as_ivec2()).filter(|t| self.map.in_bounds(*t));
        self.goal_dirty = true;
    }

    /// The debug goal's flow field, while one is set.
    pub fn flow(&self) -> Option<&FlowField> {
        self.goal_flow.as_ref()
    }

    /// Call after editing `map` directly (tests, editors): refreshes pathfinding data. Doors toggled
    /// through the world's own methods don't need this. From then on a save stores the whole map
    /// rather than the edits to the generated one.
    pub fn map_changed(&mut self) {
        self.map_custom = true;
        self.walk = WalkGrid::new(&self.map);
        self.nav.rebuild_all(&self.walk);
        self.flow_cache.invalidate();
        self.goal_dirty = true;
    }

    /// The navigation graph and the long-range plans zombies share.
    pub fn nav(&self) -> &Nav {
        &self.nav
    }

    /// Flow fields cached for zombies' targets right now.
    pub fn flow_cache_len(&self) -> usize {
        self.flow_cache.len()
    }

    /// Make a noise: zombies within `radius` (less through walls) come to investigate. It is heard
    /// on the next tick.
    pub fn emit_sound(&mut self, pos: Vec2, radius: f32) {
        self.emit_sound_from(pos, radius, SoundSource::World);
    }

    /// Debug noise (MCP `make_noise`): a sound, and with `lasting` an attractor that keeps
    /// drawing the far horde in for `migration.lasting_secs` (like an alarm left ringing).
    pub fn make_noise(&mut self, pos: Vec2, radius: f32, lasting: bool) {
        self.emit_sound(pos, radius);
        let m = &self.config.population.migration;
        if lasting && m.enabled && pos.is_finite() && radius > 0.0 {
            let indoors = self.map.room_at(pos) != 0;
            self.population.attract(pos, radius, indoors, m.lasting_secs, m, self.tick);
        }
    }

    pub fn emit_sound_from(&mut self, pos: Vec2, radius: f32, source: SoundSource) {
        self.emit_sound_luring(pos, radius, source, pos);
    }

    /// A sound heard around `pos` that sends investigating zombies to `lure`.
    pub fn emit_sound_luring(&mut self, pos: Vec2, radius: f32, source: SoundSource, lure: Vec2) {
        if self.sounds.len() < MAX_SOUNDS && pos.is_finite() && lure.is_finite() && radius.is_finite() && radius > 0.0 {
            self.sounds.push(Sound { pos, radius, source, lure });
        }
    }

    /// Sounds zombies listened to in the last tick (footsteps, groans, doors, debug noise).
    pub fn sounds_heard(&self) -> &[Sound] {
        &self.heard
    }

    /// 0 at night, 1 in full daylight.
    pub fn daylight(&self) -> f32 {
        senses::daylight(self.time_of_day())
    }

    /// Hour of day in [0, 24).
    pub fn time_of_day(&self) -> f32 {
        let hours = self.config.start_hour + self.tick as f32 * TICK_DT / self.config.day_length_secs * 24.0 + self.clock_offset_hours;
        hours.rem_euclid(24.0)
    }

    /// Move the clock forward; wounds and illness live through the hours skipped (bleeding,
    /// healing, festering, the infection), in small steps.
    pub fn skip_time(&mut self, hours: f32) {
        self.clock_offset_hours += hours;
        self.skip_health(hours);
    }

    /// Full hit points of an edge of this kind (0 for anything that can't be battered).
    pub fn max_hp(&self, kind: Edge) -> f32 {
        match kind {
            Edge::DoorClosed | Edge::DoorOpen => self.config.zombie.door_hp,
            Edge::Window | Edge::WindowOpen => self.config.zombie.window_hp,
            _ => 0.0,
        }
    }

    /// Hit points left and full hit points of a door or window that isn't broken, else `None`.
    pub fn edge_hp(&self, edge: EdgeRef) -> Option<(f32, f32)> {
        let max = self.max_hp(self.map.owned_edge(edge.tile, edge.edge));
        (max > 0.0).then(|| (self.edge_hp.get(&edge.key()).copied().unwrap_or(max), max))
    }

    /// Damaged doors and windows with their hit points left, in a stable order.
    pub fn damaged_edges(&self) -> impl Iterator<Item = (EdgeRef, f32)> + '_ {
        self.edge_hp.iter().map(|(&key, &hp)| (edge_of_key(key), hp))
    }

    /// Doors and windows whose hit points changed since the last call, with the fraction of their
    /// full hit points left (1 once broken: a broken edge has none, and shows as broken). In a
    /// stable order, each edge once. Whoever replicates the world drains this, like `edge_changes`.
    pub fn take_edge_damage(&mut self) -> Vec<(EdgeRef, f32)> {
        let dirty = std::mem::take(&mut self.edge_hp_dirty);
        dirty
            .into_iter()
            .map(|key| {
                let edge = edge_of_key(key);
                (edge, self.edge_hp(edge).map_or(1.0, |(hp, max)| (hp / max).clamp(0.0, 1.0)))
            })
            .collect()
    }

    /// Open or close the door or window nearest `p` (within reach). Returns the new edge.
    pub fn interact_near(&mut self, p: Vec2) -> Option<Edge> {
        let edge = self.edge_in_reach(p, p, Self::can_interact)?;
        Some(self.toggle_edge(edge, SoundSource::World))
    }

    /// Player `id` opens or closes the door or window it faces, if one is within reach of where
    /// the server has it. Returns the new edge, or `None` (request ignored) if there is no such
    /// edge or player. Broken doors and windows can't be shut again.
    pub fn player_interact(&mut self, id: u32) -> Option<Edge> {
        let s = self.player(id)?.state;
        let aim = s.pos + Vec2::from_angle(s.facing) * INTERACT_AIM_AHEAD;
        let edge = self.edge_in_reach(s.pos, aim, Self::can_interact)?;
        Some(self.toggle_edge(edge, SoundSource::Player(id)))
    }

    /// Player `id` smashes the window it faces (shut or open), if one is within reach. Loud.
    /// Returns the broken edge, or `None` if there is no such window or player.
    pub fn player_smash_window(&mut self, id: u32) -> Option<EdgeRef> {
        let s = self.player(id)?.state;
        let aim = s.pos + Vec2::from_angle(s.facing) * INTERACT_AIM_AHEAD;
        let edge = self.edge_in_reach(s.pos, aim, |e| matches!(e, Edge::Window | Edge::WindowOpen))?;
        self.break_edge(edge, SoundSource::Player(id));
        // Bare-handed, the glass may cut.
        self.smash_cut(id);
        Some(edge)
    }

    fn can_interact(e: Edge) -> bool {
        e.is_door() || matches!(e, Edge::Window | Edge::WindowOpen)
    }

    /// The edge accepted by `want` within [`INTERACT_REACH`] of `from` whose middle is closest to `aim`.
    fn edge_in_reach(&self, from: Vec2, aim: Vec2, want: impl Fn(Edge) -> bool) -> Option<EdgeRef> {
        let t = from.floor().as_ivec2();
        let mut best: Option<(f32, EdgeRef)> = None;
        for dy in -2..=2 {
            for dx in -2..=2 {
                let owner = t + IVec2::new(dx, dy);
                for which in [OwnedEdge::North, OwnedEdge::West] {
                    if !want(self.map.owned_edge(owner, which)) {
                        continue;
                    }
                    let edge = EdgeRef::new(owner, which);
                    let mid = edge.mid();
                    let d = aim.distance(mid);
                    if from.distance(mid) <= INTERACT_REACH && best.is_none_or(|(bd, _)| d < bd) {
                        best = Some((d, edge));
                    }
                }
            }
        }
        best.map(|(_, edge)| edge)
    }

    /// Set an edge and keep everything that depends on the map in step: replication, the walk
    /// grid, the flow fields that cover it.
    fn set_edge(&mut self, edge: EdgeRef, kind: Edge) {
        self.map.set_owned_edge(edge.tile, edge.edge, kind);
        self.map_edits.insert(edge.key());
        self.edge_changes.push(EdgeChange { tile: edge.tile, edge: edge.edge, kind });
        self.walk.update_around(&self.map, edge.tile);
        self.nav.edge_changed(&self.walk, edge.tile);
        self.goal_dirty = true;
        self.flow_cache.invalidate_near(edge.tile);
    }

    /// Open a shut door or window, or shut an open one (keeping any damage). Doors are noisier.
    pub(crate) fn toggle_edge(&mut self, edge: EdgeRef, source: SoundSource) -> Edge {
        let kind = match self.map.owned_edge(edge.tile, edge.edge) {
            Edge::DoorClosed => Edge::DoorOpen,
            Edge::DoorOpen => Edge::DoorClosed,
            Edge::Window => Edge::WindowOpen,
            Edge::WindowOpen => Edge::Window,
            other => return other,
        };
        self.set_edge(edge, kind);
        let noise = if kind.is_door() { self.config.zombie.door_noise } else { self.config.zombie.window_noise };
        self.emit_sound_from(edge.mid(), noise, source);
        kind
    }

    /// A door or window gives way for good: loud.
    pub(crate) fn break_edge(&mut self, edge: EdgeRef, source: SoundSource) {
        let kind = self.map.owned_edge(edge.tile, edge.edge);
        let broken = kind.broken();
        if broken == kind {
            return;
        }
        if self.edge_hp.remove(&edge.key()).is_some() {
            self.edge_hp_dirty.insert(edge.key());
        }
        self.set_edge(edge, broken);
        self.break_events.push(EdgeChange { tile: edge.tile, edge: edge.edge, kind: broken });
        let t = &self.config.zombie;
        let noise = if broken == Edge::DoorBroken { t.door_break_noise } else { t.glass_break_noise };
        self.emit_sound_from(edge.mid(), noise, source);
    }

    /// Damage from this tick's thumps, in order. Each is a noise at the edge, heard on both sides,
    /// that lures the zombies it draws to the far side (where the thumper is trying to get to), so
    /// they head inside too and join in.
    fn apply_thumps(&mut self) {
        let thumps = std::mem::take(&mut self.thump_events);
        for th in &thumps {
            let kind = self.map.owned_edge(th.edge.tile, th.edge.edge);
            if !kind.is_breakable() {
                // Broken (or opened) earlier this tick.
                continue;
            }
            let max = self.max_hp(kind);
            let hp = self.edge_hp.entry(th.edge.key()).or_insert(max);
            *hp -= th.damage;
            let gone = *hp <= 0.0;
            self.edge_hp_dirty.insert(th.edge.key());
            let far = th.edge.far_tile(th.from).as_vec2() + Vec2::splat(0.5);
            self.emit_sound_luring(th.edge.mid(), self.config.zombie.thump_noise, SoundSource::Zombie(th.zombie), far);
            if gone {
                self.break_edge(th.edge, SoundSource::Zombie(th.zombie));
            }
        }
        self.thump_events = thumps;
    }

    /// Clear away corpses that have lain longer than `corpse_secs`, and the oldest beyond
    /// `max_corpses`. Their ids go to `removed_corpses`; snapshots simply stop listing them.
    fn remove_old_corpses(&mut self) {
        let t = &self.config.zombie;
        let z = &self.zombies;
        let (mut corpses, mut expired) = (0usize, false);
        for i in 0..z.len() {
            if z.state[i] == ZombieState::Dead {
                corpses += 1;
                expired |= z.timer[i] >= t.corpse_secs;
            }
        }
        let excess = corpses.saturating_sub(t.max_corpses as usize);
        if excess == 0 && !expired {
            return;
        }
        // Oldest first; among those that died on the same tick, the lowest id.
        let mut dead: Vec<(f32, u32)> = (0..z.len()).filter(|&i| z.state[i] == ZombieState::Dead).map(|i| (z.timer[i], z.id[i])).collect();
        dead.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        let removed = &mut self.removed_corpses;
        removed.extend(dead.iter().enumerate().filter(|(k, (age, _))| *k < excess || *age >= t.corpse_secs).map(|(_, (_, id))| *id));
        removed.sort_unstable();
        let gone: Vec<bool> = z.id.iter().map(|id| removed.binary_search(id).is_ok()).collect();
        self.zombies.remove_where(|i| gone[i]);
    }

    pub fn step(&mut self) {
        let start = Instant::now();
        self.attack_events.clear();
        self.thump_events.clear();
        self.break_events.clear();
        self.removed_corpses.clear();
        self.action_events.clear();
        self.hit_events.clear();
        self.used_items.clear();
        // Sounds made since the last tick are heard now; groans made during this one, next tick.
        std::mem::swap(&mut self.sounds, &mut self.heard);
        self.sounds.clear();

        // Players move first, so zombies react to where they are now. Moving makes noise.
        self.targets.clear();
        let t = &self.config.zombie;
        let mut climbed_glass = Vec::new();
        for (pi, p) in self.players.iter_mut().enumerate() {
            if p.dead {
                continue;
            }
            let before = p.state.pos;
            players::step_movement(&self.map, &mut p.state, &p.input, TICK_DT);
            // This life's time played and distance walked (`crate::run`).
            p.run.secs += TICK_DT as f64;
            p.run.distance_m += p.state.pos.distance(before) as f64;
            // Starting to climb through a broken window: the glass left in the frame may cut.
            if !self.map.in_window_opening(before) && self.map.window_opening(p.state.pos) == Some(Edge::WindowBroken) {
                climbed_glass.push(pi);
            }
            let moving = p.state.pos.distance_squared(before) > 1e-8;
            self.targets.push(Target { id: p.id, pos: p.state.pos, stance: p.state.stance, moving, ignored: p.dev.ignored });
            if moving && self.heard.len() < MAX_SOUNDS {
                let radius = match p.state.stance {
                    Stance::Sneak => t.sneak_noise,
                    Stance::Walk => t.walk_noise,
                    Stance::Run => t.run_noise,
                };
                self.heard.push(Sound::new(p.state.pos, radius, SoundSource::Player(p.id)));
            }
        }

        for pi in climbed_glass {
            self.climb_cut(pi);
        }

        // Item moves, searches and uses that are due; containers left behind close.
        self.step_item_jobs();

        // Which regions players are near (respawn keeps away from those for a while).
        if !self.players.is_empty() {
            let seen = self.config.population.respawn.seen_distance;
            self.population.update_seen(&self.regions, self.players.iter().map(|p| p.state.pos), seen, self.tick);
        }

        // Tiers: who gets how much of the simulation this tick, by distance to the players.
        let tier_start = Instant::now();
        let changes = self.assign_tiers();
        // Corpses stay out of the hash: nothing senses, avoids or hits them. Virtual zombies are
        // indexed by region instead.
        let (states, tier) = (&self.zombies.state, &self.zombies.tier);
        self.hash.rebuild_where(&self.zombies.pos, |i| states[i] != ZombieState::Dead && tier[i] != Tier::Virtual);
        self.regions.rebuild_where(&self.zombies.pos, |i| states[i] != ZombieState::Dead && tier[i] == Tier::Virtual);
        let tier_ms = tier_start.elapsed().as_secs_f32() * 1000.0;
        self.step_player_actions();
        // Ignored players (dev) aren't heard: their footsteps, swings, doors and searches are
        // dropped here, before anything listens (senses, the far tiers, migration).
        if self.players.iter().any(|p| p.dev.ignored) {
            let players = &self.players;
            self.heard.retain(|s| !matches!(s.source, SoundSource::Player(id) if players.iter().any(|p| p.id == id && p.dev.ignored)));
        }
        let t = &self.config.zombie;
        let mut sight_checks = 0;
        if self.goal.is_some() {
            if self.goal_dirty || self.goal_flow.is_none() {
                self.goal_flow = self.goal.map(|g| FlowField::build(&self.walk, g, GOAL_RANGE + 2.0));
                self.goal_dirty = false;
            }
        } else {
            self.goal_flow = None;
            let daylight = senses::daylight(self.time_of_day());
            let seed = self.config.seed();
            sight_checks = senses::sight_pass(&mut self.zombies, &self.hash, &self.map, &self.targets, t, daylight, self.tick, seed);
            senses::hearing_pass(&mut self.zombies, &self.hash, &self.map, &self.heard, t);
        }
        let senses_ms = start.elapsed().as_secs_f32() * 1000.0 - tier_ms;

        // Build the fields zombies asked for last tick, and follow moving players.
        let targets = &self.targets;
        self.flow_cache.update(&self.walk, self.tick, |id| targets.iter().find(|p| p.id == id).map(|p| p.pos.floor().as_ivec2()));
        let flow_ms = start.elapsed().as_secs_f32() * 1000.0 - senses_ms - tier_ms;
        // Plans the zombies asked for last tick.
        let nav_start = Instant::now();
        self.nav.update(self.tick);
        let nav_ms = nav_start.elapsed().as_secs_f32() * 1000.0;
        let long_paths = self.config.tiers.long_paths;

        self.scratch_pos.clear();
        self.scratch_pos.extend_from_slice(&self.zombies.pos);
        let mut cx = Ctx {
            map: &self.map,
            walk: &self.walk,
            nav: long_paths.then_some(&mut self.nav),
            hash: &self.hash,
            targets: &self.targets,
            tuning: &self.config.zombie,
            flow: &mut self.flow_cache,
            goal: self.goal_flow.as_ref(),
            rng: &mut self.rng,
            sounds: &mut self.sounds,
            attacks: &mut self.attack_events,
            thumps: &mut self.thump_events,
            tick: self.tick,
        };
        let ai_start = Instant::now();
        let counts = ai::step_zombies(&mut self.zombies, &mut self.scratch_pos, &mut cx);
        let active_ms = ai_start.elapsed().as_secs_f32() * 1000.0;
        let near_start = Instant::now();
        let mut ncx = NearCtx {
            map: &self.map,
            walk: &self.walk,
            nav: long_paths.then_some(&mut self.nav),
            flow: &self.flow_cache,
            goal: self.goal_flow.as_ref(),
            tuning: &self.config.zombie,
            tiers: &self.config.tiers,
            rng: &mut self.rng,
            tick: self.tick,
        };
        let near_updated = tiers::step_near(&mut self.zombies, &mut self.scratch_pos, &mut ncx);
        let near_ms = near_start.elapsed().as_secs_f32() * 1000.0;
        std::mem::swap(&mut self.zombies.pos, &mut self.scratch_pos);
        let virtual_start = Instant::now();
        let (t, tt) = (&self.config.zombie, &self.config.tiers);
        let migration = &self.config.population.migration;
        if self.goal.is_none() {
            tiers::virtual_hearing(&mut self.zombies, &self.regions, &self.map, &self.heard, t, tt);
            // Loud sounds leave attractors that draw the far horde in over the next minutes.
            self.population.hear(&self.heard, &self.map, migration, self.tick);
        }
        self.population.expire(self.tick);
        let seed = self.config.generation.seed;
        let nav = long_paths.then_some(&mut self.nav);
        let virtual_updated = tiers::step_virtual(&mut self.zombies, &self.regions, &self.map, &self.walk, nav, t, tt, &self.population, migration, self.tick, seed);
        let virtual_ms = virtual_start.elapsed().as_secs_f32() * 1000.0;
        self.apply_thumps();
        self.apply_zombie_attacks();
        // Wounds bleed and heal, illness runs its course, an infected body rises.
        let hours = TICK_DT * self.game_hours_per_sec();
        self.step_health(hours, TICK_DT);
        // Last: indices into the zombie columns (the spatial hash) aren't needed after this.
        self.remove_old_corpses();
        let respawned = self.respawn();

        self.tick += 1;
        let tier_counts = self.tier_counts();
        self.stats = TickStats {
            tick_ms: start.elapsed().as_secs_f32() * 1000.0,
            senses_ms,
            flow_ms,
            nav_ms,
            nav_plans: self.nav.made,
            nav_expanded: self.nav.expanded,
            chasing: counts.hunting,
            investigating: counts.investigating,
            sight_checks,
            sounds: self.heard.len() as u32,
            flow_builds: self.flow_cache.builds,
            thumping: counts.thumping,
            thumps: self.thump_events.len() as u32,
            breaks: self.break_events.len() as u32,
            reeling: counts.reeling,
            active: tier_counts[0],
            near: tier_counts[1],
            virtual_: tier_counts[2],
            promoted: changes.promoted,
            demoted: changes.demoted,
            near_updated,
            virtual_updated,
            respawned,
            attractors: self.population.attractors.len() as u32,
            tier_ms,
            active_ms,
            near_ms,
            virtual_ms,
        };
    }

    /// The tier pass: with at least one player (and tiers enabled), every zombie moves to the tier
    /// its chunk's distance to the nearest player calls for. With none, nothing changes tier.
    fn assign_tiers(&mut self) -> tiers::TierChanges {
        if !self.config.tiers.enabled || self.players.is_empty() {
            if !self.config.tiers.enabled {
                self.zombies.tier.iter_mut().for_each(|t| *t = Tier::Active);
            }
            return tiers::TierChanges::default();
        }
        self.chunk_dist.update(&self.map, self.players.iter().map(|p| p.state.pos));
        tiers::assign(&mut self.zombies, &self.chunk_dist, &self.config.tiers, &self.config.zombie, &self.map, &mut self.scratch_tiles)
    }

    /// Respawn (if on): every `interval_secs`, each region far from every player and unseen for
    /// `unseen_secs` that has fewer than `top_up_below` of its target gets topped up, on open
    /// ground picked by density, never near where the player woke up or in that house. New
    /// zombies start on the virtual tier. Returns how many were added.
    fn respawn(&mut self) -> u32 {
        let r = self.config.population.respawn.clone();
        let every = ((r.interval_secs / TICK_DT).round() as u64).max(1);
        if !r.enabled || self.players.is_empty() || self.tick == 0 || !self.tick.is_multiple_of(every) {
            return 0;
        }
        self.population.count_into(&self.zombies, &self.regions);
        let (town, max, seed, tick) = ((self.town_min, self.town_max), self.config.population.max_density(), self.config.seed(), self.tick);
        let (unseen, far2) = ((r.unseen_secs / TICK_DT) as u64, r.min_distance * r.min_distance);
        let mut budget = r.max_per_check;
        let mut added = 0;
        for reg in 0..self.regions.regions() {
            if budget == 0 || max <= 0.0 {
                break;
            }
            let (target, count, seen) = (self.population.target[reg], self.population.counts[reg], self.population.last_seen[reg]);
            if target < 1.0 || count as f32 >= target * r.top_up_below || (seen != NEVER && tick - seen < unseen) {
                continue;
            }
            let (lo, hi) = self.regions.rect(reg);
            if self.players.iter().any(|p| p.state.pos.clamp(lo, hi).distance_squared(p.state.pos) < far2) {
                continue;
            }
            let want = (target.round() as u32).saturating_sub(count).min(r.max_per_region).min(budget);
            let size = self.regions.size();
            let mut placed = 0;
            for k in 0..want as u64 * 400 {
                if placed == want {
                    break;
                }
                let roll = |j: u64| hash01(seed ^ 0x5245_5350, tick, reg as u64, k * 4 + j);
                let p = lo + Vec2::new(roll(0), roll(1)) * size;
                let t = p.floor().as_ivec2();
                if !self.map.in_bounds(t) || roll(2) >= self.config.population.tile_density(&self.map, town, t) / max || !self.spawnable(t) {
                    continue;
                }
                let i = self.push_zombie(p);
                self.zombies.tier[i] = Tier::Virtual;
                placed += 1;
            }
            budget -= placed;
            added += placed;
        }
        self.population.respawned += added;
        added
    }

    /// The population state: per-region targets, zones and sightings, and the live attractors.
    pub fn population(&self) -> &Population {
        &self.population
    }

    /// The population zone of tile `t` (from the generator and the population file).
    pub fn zone_at(&self, t: IVec2) -> Zone {
        self.config.population.zone_at(&self.map, (self.town_min, self.town_max), t)
    }

    /// Zombies on each tier: [active, near, virtual] (corpses included).
    pub fn tier_counts(&self) -> [u32; 3] {
        let mut n = [0; 3];
        for t in &self.zombies.tier {
            n[*t as usize] += 1;
        }
        n
    }

    /// Should zombie `i` go to clients? Active, and within `interest_radius` of some player (any
    /// active zombie when there are no players).
    pub fn in_interest(&self, i: usize) -> bool {
        let r = self.config.tiers.interest_radius;
        self.zombies.tier[i] == Tier::Active && (self.players.is_empty() || self.players.iter().any(|p| p.state.pos.distance_squared(self.zombies.pos[i]) <= r * r))
    }

    /// Virtual zombies indexed by region, as of the start of the last tick.
    pub fn regions(&self) -> &RegionIndex {
        &self.regions
    }

    /// Hash of all zombie and player state, for determinism checks and replays.
    pub fn state_hash(&self) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        let mut mix = |v: u32| {
            h ^= v as u64;
            h = h.wrapping_mul(0x0100_0000_01b3);
        };
        for i in 0..self.zombies.len() {
            mix(self.zombies.id[i]);
            mix(self.zombies.pos[i].x.to_bits());
            mix(self.zombies.pos[i].y.to_bits());
            mix(self.zombies.facing[i].to_bits());
            mix(self.zombies.state[i] as u32 | (self.zombies.sees[i] as u32) << 8 | (self.zombies.tier[i] as u32) << 16);
            mix(self.zombies.target_player[i]);
            mix(self.zombies.target_pos[i].x.to_bits());
            mix(self.zombies.target_pos[i].y.to_bits());
            mix(self.zombies.timer[i].to_bits());
            mix(self.zombies.hp[i].to_bits());
            mix(self.zombies.stuck[i].to_bits());
            mix(self.zombies.bored[i].to_bits());
        }
        for p in &self.players {
            mix(p.id);
            mix(p.state.pos.x.to_bits());
            mix(p.state.pos.y.to_bits());
            mix(p.state.facing.to_bits());
            mix(p.state.endurance.to_bits());
            // Dev toggles add bits only when on, so a world without them hashes as it always did.
            mix(p.state.stance as u32 | (p.state.winded as u32) << 8 | (p.dead as u32) << 9 | (p.dev.god as u32) << 10 | (p.dev.ignored as u32) << 11);
            mix(p.hp.to_bits());
            Self::hash_health(&p.health, &mut mix);
            mix(p.weapon.map_or(u32::MAX, u32::from));
            mix(p.ready_in.to_bits());
            mix(p.action.map_or(u32::MAX, |a| a.kind as u32 | a.timer.to_bits() << 2));
            for s in p.inventory.stacks() {
                mix(s.item as u32 | (s.count as u32) << 16);
            }
            let inv = &p.inventory;
            mix(inv.worn.len() as u32 | (inv.carried.len() as u32) << 8 | (inv.hand.is_some() as u32) << 16 | (inv.off_hand.is_some() as u32) << 17 | (inv.back.is_some() as u32) << 18 | (inv.shoulder.is_some() as u32) << 19);
            mix(p.open.unwrap_or(u32::MAX));
            for j in &p.jobs {
                mix(j.left.to_bits());
                mix(match j.kind {
                    crate::inventory::JobKind::Move { index, item, count, .. } => index ^ (item as u32) << 8 ^ (count as u32) << 20,
                    crate::inventory::JobKind::Search { container } => container ^ 0x5EA4_0000,
                    crate::inventory::JobKind::Use { index, item, .. } => index ^ (item as u32) << 8 ^ 0x0E00_0000,
                });
            }
        }
        for it in &self.items {
            mix(it.id);
            mix(it.stack.item as u32 | (it.stack.count as u32) << 16);
            mix(it.pos.x.to_bits());
            mix(it.pos.y.to_bits());
        }
        for (&c, stacks) in &self.opened {
            mix(c);
            mix(stacks.len() as u32);
            for s in stacks {
                mix(s.item as u32 | (s.count as u32) << 16);
            }
        }
        for (&(x, y, w), hp) in &self.edge_hp {
            mix(x as u32);
            mix(y as u32);
            mix(w as u32);
            mix(hp.to_bits());
        }
        self.population.hash_into(&mut mix);
        h
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map::Side;
    use crate::template::BuildingTemplate;
    use crate::zombies::{ZombieAnim, ZombieState};

    fn templates() -> Vec<BuildingTemplate> {
        [include_str!("../../../assets/data/buildings/bungalow.ron"), include_str!("../../../assets/data/buildings/family_two_storey.ron")]
            .iter()
            .map(|t| BuildingTemplate::from_ron(t).unwrap())
            .collect()
    }

    fn world(seed: u64, zombies: u32) -> World {
        World::new(WorldConfig::new(seed, zombies, templates()))
    }

    /// A point on the main street, outside any building.
    fn street_point(w: &World) -> Vec2 {
        for y in w.town_min.y..w.town_max.y {
            let p = IVec2::new((w.town_min.x + w.town_max.x) / 2, y);
            if w.map.tile(p).floor.is_road() && !w.map.tile_blocked(p) {
                return p.as_vec2() + Vec2::splat(0.5);
            }
        }
        panic!("no street");
    }

    #[test]
    fn same_seed_same_result() {
        let mut a = world(7, 300);
        let mut b = world(7, 300);
        let goal = street_point(&a);
        a.set_goal(Some(goal));
        b.set_goal(Some(goal));
        for _ in 0..200 {
            a.step();
            b.step();
        }
        assert_eq!(a.state_hash(), b.state_hash());
        assert_ne!(a.state_hash(), world(8, 300).state_hash());
    }

    #[test]
    fn zombies_converge_on_goal_and_never_cross_walls() {
        let mut w = world(3, 300);
        let goal = street_point(&w);
        w.set_goal(Some(goal));
        let rooms_before: Vec<u16> = w.zombies.pos.iter().map(|p| w.map.room_at(*p)).collect();
        let near = |w: &World| w.zombies.pos.iter().filter(|p| p.distance(goal) < 15.0).count();
        let before = near(&w);
        for _ in 0..20 * 40 {
            w.step();
        }
        assert!(near(&w) > before + 30, "zombies should gather: {before} -> {}", near(&w));
        for p in &w.zombies.pos {
            // A resolved position doesn't overlap walls, trunks or furniture (resolving again barely moves it).
            let again = w.map.resolve_circle(*p, ZOMBIE_RADIUS);
            assert!(again.distance(*p) < 0.05, "zombie overlapping an obstacle at {p}");
        }
        // Zombies shut inside a building (no open way out) are still inside that building.
        for (i, p) in w.zombies.pos.iter().enumerate() {
            let room = rooms_before[i];
            if room != 0 && w.flow().and_then(|f| f.distance(*p)).is_none() {
                let building = w.map.rooms[room as usize].building as usize;
                assert_eq!(w.map.building_at(*p), Some(building), "zombie {i} escaped a sealed building");
            }
        }
    }

    #[test]
    fn doors_toggle_and_replicate() {
        let mut w = world(1, 0);
        let b = &w.map.buildings[w.spawn_building];
        let centre = (b.min + b.size / 2).as_vec2();
        // Find some door near the spawn house and toggle it twice.
        let mut found = None;
        'outer: for y in b.min.y - 1..b.min.y + b.size.y + 1 {
            for x in b.min.x - 1..b.min.x + b.size.x + 1 {
                for which in [OwnedEdge::North, OwnedEdge::West] {
                    if w.map.owned_edge(IVec2::new(x, y), which).is_door() {
                        found = Some(TileMap::edge_segment(IVec2::new(x, y), which));
                        break 'outer;
                    }
                }
            }
        }
        let (a, bb) = found.expect("a door");
        let mid = (a + bb) * 0.5;
        let first = w.interact_near(mid).unwrap();
        let second = w.interact_near(mid).unwrap();
        assert_ne!(first, second);
        assert_eq!(w.edge_changes.len(), 2);
        let _ = centre;
    }

    /// Every open tile on a road.
    fn road_points(w: &World) -> Vec<Vec2> {
        let mut out = Vec::new();
        for y in w.town_min.y..w.town_max.y {
            for x in w.town_min.x..w.town_max.x {
                let t = IVec2::new(x, y);
                if w.map.tile(t).floor.is_road() && !w.map.tile_blocked(t) {
                    out.push(t.as_vec2() + Vec2::splat(0.5));
                }
            }
        }
        out
    }

    fn close_doors_of(w: &mut World, building: usize) {
        let b = &w.map.buildings[building];
        let (min, max) = (b.min - IVec2::ONE, b.min + b.size + IVec2::ONE);
        for y in min.y..max.y {
            for x in min.x..max.x {
                for which in [OwnedEdge::North, OwnedEdge::West] {
                    if w.map.owned_edge(IVec2::new(x, y), which) == Edge::DoorOpen {
                        w.map.set_owned_edge(IVec2::new(x, y), which, Edge::DoorClosed);
                    }
                }
            }
        }
        w.map_changed();
    }

    #[test]
    fn players_stay_inside_a_shut_house() {
        for seed in 1..=3 {
            let mut w = world(seed, 0);
            let house = w.spawn_building;
            close_doors_of(&mut w, house);
            let id = w.add_player(w.player_spawn);
            let mut seq = 0;
            // Walk and run into every wall of the house for a while.
            for k in 0..8 {
                let dir = Vec2::from_angle(k as f32 * TAU / 8.0 + 0.3);
                for _ in 0..100 {
                    seq += 1;
                    w.set_player_input(id, seq, PlayerInput { move_dir: dir, facing: 0.0, run: k % 2 == 0, sneak: false });
                    w.step();
                    let p = w.player(id).unwrap().state.pos;
                    assert_eq!(w.map.building_at(p), Some(w.spawn_building), "seed {seed}: left the house at {p}");
                    assert!(w.map.resolve_circle(p, PLAYER_RADIUS).distance(p) < 0.05, "seed {seed}: inside an obstacle at {p}");
                }
            }
        }
    }

    #[test]
    fn same_seed_same_inputs_same_hash() {
        let run = |seed: u64, wiggle: f32| {
            let mut w = world(seed, 300);
            let id = w.add_player(street_point(&w));
            for t in 0..400u32 {
                let a = t as f32 * 0.05 + wiggle;
                let input = PlayerInput { move_dir: Vec2::from_angle(a), facing: a, run: t % 100 < 50, sneak: t % 100 > 80 };
                assert!(w.set_player_input(id, t + 1, input));
                w.step();
            }
            // Stale and repeated sequence numbers are ignored.
            assert!(!w.set_player_input(id, 400, PlayerInput::default()));
            assert!(!w.set_player_input(id, 3, PlayerInput::default()));
            w.state_hash()
        };
        assert_eq!(run(5, 0.0), run(5, 0.0));
        assert_ne!(run(5, 0.0), run(5, 0.5));
    }

    /// A loud noise (an alarm going off every 10 s) draws the horde in; once close they see the
    /// player and attack.
    #[test]
    fn a_loud_noise_pulls_a_horde_onto_a_player() {
        let mut w = world(3, 300);
        let id = w.add_player(street_point(&w));
        // Tough enough to outlast the horde, so we can watch it keep coming.
        w.players[0].hp = 1e6;
        let p = w.player(id).unwrap().state.pos;
        let near = |w: &World| w.zombies.pos.iter().filter(|z| z.distance(p) < 15.0).count();
        let before = near(&w);
        let (mut attacks, mut landed) = (0, 0);
        for t in 0..20 * 60 {
            if t % 200 == 0 {
                w.emit_sound(p, 80.0);
            }
            w.step();
            attacks += w.attack_events.len();
            landed += w.attack_events.iter().filter(|a| a.landed).count();
        }
        assert!(near(&w) > before + 30, "zombies should close in: {before} -> {}", near(&w));
        assert!(w.zombies.anim.contains(&ZombieAnim::Attack), "someone should be attacking");
        assert!(attacks > 0 && w.attack_events.iter().all(|a| a.player == id));
        assert!(landed > 0 && landed < attacks, "{landed} of {attacks} strikes landed");
        assert!(w.player(id).unwrap().hp < 1e6);
    }

    #[test]
    fn noise_splits_zombies_between_two_players() {
        let mut w = world(3, 400);
        let roads = road_points(&w);
        let a = roads[0];
        let b = *roads.iter().max_by(|p, q| p.distance(a).total_cmp(&q.distance(a))).unwrap();
        assert!(a.distance(b) > 60.0);
        w.add_player(a);
        w.add_player(b);
        let near = |w: &World, p: Vec2| w.zombies.pos.iter().filter(|z| z.distance(p) < 15.0).count();
        let (before_a, before_b) = (near(&w, a), near(&w, b));
        for t in 0..20 * 60 {
            if t % 200 == 0 {
                w.emit_sound(a, 80.0);
                w.emit_sound(b, 80.0);
            }
            w.step();
        }
        assert!(near(&w, a) > before_a + 10, "player 1: {before_a} -> {}", near(&w, a));
        assert!(near(&w, b) > before_b + 10, "player 2: {before_b} -> {}", near(&w, b));
    }

    /// An empty grass field at noon with no zombies, for hand-placed sense tests.
    fn arena() -> World {
        let mut config = WorldConfig::new(11, 0, templates());
        config.start_hour = 12.0;
        let mut w = World::new(config);
        w.map = TileMap::new(w.map.width() / crate::map::CHUNK_SIZE, w.map.height() / crate::map::CHUNK_SIZE);
        w.map_changed();
        w
    }

    /// A zombie that stands still facing `facing` (so it can't turn round by chance).
    fn add_zombie(w: &mut World, pos: Vec2, facing: f32) -> usize {
        let i = w.add_zombie(pos, facing, 1.0);
        w.zombies.wander_timer[i] = 1e9;
        i
    }

    fn wall_x(w: &mut World, x: i32, ys: std::ops::Range<i32>) {
        for y in ys {
            w.map.set_edge(IVec2::new(x, y), crate::map::Side::W, Edge::Wall);
        }
        w.map_changed();
    }

    #[test]
    fn zombie_sees_a_player_in_front_but_not_through_a_wall() {
        for walled in [false, true] {
            let mut w = arena();
            if walled {
                wall_x(&mut w, 100, 80..120);
            }
            let z = add_zombie(&mut w, Vec2::new(95.5, 100.5), 0.0);
            w.add_player(Vec2::new(101.5, 100.5));
            for _ in 0..40 {
                w.step();
            }
            let state = w.zombies.state[z];
            if walled {
                assert_eq!(state, ZombieState::Wander, "saw through a wall");
            } else {
                assert!(state.is_hunting(), "didn't see a player 6 m ahead: {state:?}");
                assert_eq!(w.zombies.target_player[z], 1);
            }
        }
    }

    /// A player moving back and forth 3 m behind a zombie: sneaking goes unnoticed, running is heard.
    #[test]
    fn sneaking_behind_a_zombie_is_quiet_running_is_heard() {
        for run in [false, true] {
            let mut w = arena();
            let z = add_zombie(&mut w, Vec2::new(100.5, 100.5), 0.0);
            let id = w.add_player(Vec2::new(97.5, 100.5));
            let mut noticed = None;
            for t in 0..200u32 {
                let dir = if (t / 20) % 2 == 0 { Vec2::Y } else { -Vec2::Y };
                w.set_player_input(id, t + 1, PlayerInput { move_dir: dir, facing: 0.0, run, sneak: !run });
                w.step();
                if noticed.is_none() && w.zombies.state[z] != ZombieState::Wander {
                    noticed = Some(w.zombies.state[z]);
                }
            }
            if run {
                assert_eq!(noticed, Some(ZombieState::Investigate), "a running player should be heard first");
                assert!(w.zombies.state[z].is_hunting(), "and then seen once it turns: {:?}", w.zombies.state[z]);
            } else {
                assert_eq!(noticed, None, "a sneaking player behind it was noticed");
            }
        }
    }

    #[test]
    fn a_noise_draws_a_wandering_zombie() {
        let mut w = arena();
        let z = add_zombie(&mut w, Vec2::new(100.5, 100.5), 0.0);
        // Through a wall it carries less far: 20 m muffled to 8 m doesn't reach 15 m away.
        wall_x(&mut w, 108, 80..120);
        let noise = Vec2::new(115.5, 104.5);
        w.emit_sound(noise, 20.0);
        w.step();
        assert_eq!(w.zombies.state[z], ZombieState::Wander, "heard through the wall");
        // Out in the open it does.
        w.map = TileMap::new(w.map.width() / crate::map::CHUNK_SIZE, w.map.height() / crate::map::CHUNK_SIZE);
        w.map_changed();
        w.emit_sound(noise, 20.0);
        w.step();
        assert_eq!(w.zombies.state[z], ZombieState::Investigate);
        assert_eq!(w.zombies.target_pos[z], noise);
        for _ in 0..20 * 25 {
            w.step();
        }
        assert!(w.zombies.pos[z].distance(noise) < w.config.zombie.search_radius + 1.0, "went to look: {}", w.zombies.pos[z]);
        assert_eq!(w.zombies.state[z], ZombieState::Investigate);
    }

    #[test]
    fn a_chaser_that_loses_sight_searches_then_gives_up() {
        let mut w = arena();
        let z = add_zombie(&mut w, Vec2::new(95.5, 100.5), 0.0);
        let id = w.add_player(Vec2::new(100.5, 100.5));
        for _ in 0..8 {
            w.step();
        }
        assert_eq!(w.zombies.state[z], ZombieState::Chase);
        let seen_at = w.player(id).unwrap().state.pos;
        // The player vanishes far out of sight and earshot (standing still makes no noise).
        w.players[0].state.pos = Vec2::new(200.5, 200.5);
        let t = w.config.zombie.clone();
        let ticks = |secs: f32| (secs / TICK_DT) as u32;
        for _ in 0..ticks(t.memory_secs - 1.0) {
            w.step();
        }
        assert_eq!(w.zombies.state[z], ZombieState::Chase, "gave up too early");
        assert!(w.zombies.pos[z].distance(seen_at) < t.search_radius + 1.0, "searching where it last saw them");
        for _ in 0..ticks(2.0) {
            w.step();
        }
        assert_eq!(w.zombies.state[z], ZombieState::Investigate, "memory should have run out");
        for _ in 0..ticks(t.investigate_secs + 1.0) {
            w.step();
        }
        assert_eq!(w.zombies.state[z], ZombieState::Wander);
    }

    #[test]
    fn a_chaser_lunges_then_attacks_on_a_cooldown() {
        let mut w = arena();
        let z = add_zombie(&mut w, Vec2::new(95.5, 100.5), 0.0);
        let id = w.add_player(Vec2::new(100.5, 100.5));
        let (mut lunged, mut attacks) = (false, 0);
        for _ in 0..20 * 10 {
            w.step();
            lunged |= w.zombies.state[z] == ZombieState::Lunge;
            attacks += w.attack_events.iter().filter(|a| a.player == id && a.zombie == w.zombies.id[z]).count();
        }
        assert!(lunged, "should lunge when close");
        assert_eq!(w.zombies.state[z], ZombieState::Attack);
        let t = &w.config.zombie;
        // About one strike per cooldown once in reach.
        assert!(attacks >= 4 && attacks as f32 <= 10.0 / t.attack_cooldown + 1.0, "{attacks} attacks");
        // Shutting it out (a wall between) stops the attacks.
        wall_x(&mut w, 100, 90..110);
        w.players[0].state.pos = Vec2::new(100.7, 100.5);
        w.zombies.pos[z] = Vec2::new(99.6, 100.5);
        for _ in 0..40 {
            w.step();
            assert!(w.attack_events.is_empty(), "attacked through a wall");
        }
    }

    #[test]
    fn senses_are_deterministic() {
        let run = |seed: u64| {
            let mut w = world(seed, 600);
            let id = w.add_player(street_point(&w));
            for t in 0..600u32 {
                let a = t as f32 * 0.03;
                w.set_player_input(id, t + 1, PlayerInput { move_dir: Vec2::from_angle(a), facing: a, run: t % 120 < 40, sneak: t % 120 > 90 });
                if t % 150 == 0 {
                    w.emit_sound(w.player(id).unwrap().state.pos, 40.0);
                }
                w.step();
            }
            assert!(w.stats.chasing + w.stats.investigating > 0, "nothing reacted");
            w.state_hash()
        };
        assert_eq!(run(9), run(9));
    }

    #[test]
    fn players_only_toggle_doors_in_reach() {
        let mut w = world(1, 0);
        let b = &w.map.buildings[w.spawn_building];
        let mut found = None;
        'outer: for y in b.min.y - 1..b.min.y + b.size.y + 1 {
            for x in b.min.x - 1..b.min.x + b.size.x + 1 {
                for which in [OwnedEdge::North, OwnedEdge::West] {
                    if w.map.owned_edge(IVec2::new(x, y), which).is_door() {
                        found = Some(TileMap::edge_segment(IVec2::new(x, y), which));
                        break 'outer;
                    }
                }
            }
        }
        let (a, bb) = found.expect("a door");
        let mid = (a + bb) * 0.5;
        let normal = (bb - a).perp().normalize();
        let far = w.add_player(mid + normal * 4.0);
        assert!(w.player_interact(far).is_none(), "door toggled from 4 m away");
        assert!(w.edge_changes.is_empty());
        let near = w.add_player(mid + normal * 0.7);
        assert!(w.player_interact(near).is_some(), "door in reach should toggle");
        assert_eq!(w.edge_changes.len(), 1);
        assert!(w.player_interact(99).is_none(), "unknown player");
    }

    /// Doors and windows in a building's outer wall: (edge, tile inside, tile outside).
    fn exterior_edges(w: &World, building: usize, want: impl Fn(Edge) -> bool) -> Vec<(EdgeRef, IVec2, IVec2)> {
        let b = &w.map.buildings[building];
        let mut out = Vec::new();
        for y in b.min.y..b.min.y + b.size.y {
            for x in b.min.x..b.min.x + b.size.x {
                let t = IVec2::new(x, y);
                let room = w.map.tile(t).room;
                if room == 0 || w.map.rooms[room as usize].building as usize != building {
                    continue;
                }
                for side in Side::ALL {
                    let n = t + side.offset();
                    if w.map.tile(n).room == 0 && want(w.map.edge(t, side)) {
                        out.push((EdgeRef::of(t, side), t, n));
                    }
                }
            }
        }
        out
    }

    /// Zombies on open ground 3-7 m outside a building, every `every`-th such tile.
    fn ring_of_zombies(w: &mut World, building: usize, every: usize) -> usize {
        let b = &w.map.buildings[building];
        let (min, max) = (b.min, b.min + b.size);
        let mut spots = Vec::new();
        for y in min.y - 7..max.y + 7 {
            for x in min.x - 7..max.x + 7 {
                let t = IVec2::new(x, y);
                let gap = (min.x - x).max(x - max.x + 1).max(min.y - y).max(y - max.y + 1);
                if gap >= 3 && w.map.in_bounds(t) && w.map.tile(t).room == 0 && !w.map.tile_blocked(t) {
                    spots.push(t.as_vec2() + Vec2::splat(0.5));
                }
            }
        }
        let mut n = 0;
        for p in spots.into_iter().step_by(every) {
            add_zombie(w, p, 0.0);
            n += 1;
        }
        n
    }

    /// A player shut in their house, banging about now and then (a noise every 15 s). Returns
    /// (ticks until something broke, until a zombie got inside, until the first strike), and checks
    /// on the way that zombies only ever break doors and windows, never open them.
    fn break_in(seed: u64, secs: u32) -> (World, Option<u32>, Option<u32>, Option<u32>) {
        let mut w = world(seed, 0);
        let house = w.spawn_building;
        close_doors_of(&mut w, house);
        let id = w.add_player(w.player_spawn);
        let n = ring_of_zombies(&mut w, house, 6);
        assert!(n >= 10, "only {n} zombies placed");
        let (mut broke, mut inside, mut struck) = (None, None, None);
        for t in 0..secs * 20 {
            if t % 300 == 0 {
                let p = w.player(id).unwrap().state.pos;
                w.emit_sound_from(p, 30.0, SoundSource::Player(id));
            }
            w.step();
            if broke.is_none() && w.stats.breaks > 0 {
                broke = Some(t);
            }
            if inside.is_none() && w.zombies.pos.iter().any(|p| w.map.building_at(*p) == Some(house)) {
                inside = Some(t);
            }
            if struck.is_none() && w.attack_events.iter().any(|a| a.player == id) {
                struck = Some(t);
            }
            for c in &w.edge_changes {
                assert!(matches!(c.kind, Edge::DoorBroken | Edge::WindowBroken), "a zombie changed an edge to {:?}", c.kind);
            }
        }
        (w, broke, inside, struck)
    }

    #[test]
    fn zombies_break_into_a_shut_house_and_reach_the_player() {
        for seed in [1, 2] {
            let (w, broke, inside, struck) = break_in(seed, 150);
            let broke = broke.unwrap_or_else(|| panic!("seed {seed}: nothing broke"));
            let inside = inside.unwrap_or_else(|| panic!("seed {seed}: no zombie got in"));
            let struck = struck.unwrap_or_else(|| panic!("seed {seed}: nobody reached the player"));
            assert!(broke <= inside && inside <= struck, "seed {seed}: broke {broke}, inside {inside}, struck {struck}");
            // Nobody walked through a wall: every zombie inside came through a broken edge.
            assert!(w.edge_changes.iter().any(|c| c.kind == Edge::WindowBroken || c.kind == Edge::DoorBroken));
            eprintln!("seed {seed}: first break after {:.1} s, first zombie inside {:.1} s, first strike {:.1} s", broke as f32 * TICK_DT, inside as f32 * TICK_DT, struck as f32 * TICK_DT);
        }
    }

    #[test]
    fn break_ins_are_deterministic() {
        let run = || {
            let (w, broke, inside, _) = break_in(3, 40);
            (w.state_hash(), broke, inside, w.edge_changes.len())
        };
        let a = run();
        assert_eq!(a, run());
        assert!(a.1.is_some(), "expected a break within 40 s");
    }

    /// An open field split by a long wall at x = 100 with one way through at (100, 100), and a
    /// player standing just behind it.
    fn walled_arena(opening: Edge) -> (World, Vec2) {
        let mut w = arena();
        let h = w.map.height();
        wall_x(&mut w, 100, 0..h);
        w.map.set_edge(IVec2::new(100, 100), Side::W, opening);
        w.map_changed();
        let player = Vec2::new(103.5, 100.5);
        w.add_player(player);
        (w, player)
    }

    /// Three zombies on the near side, alerted by the player's noise: seconds until the way breaks.
    fn secs_to_break(opening: Edge) -> f32 {
        let (mut w, player) = walled_arena(opening);
        for k in 0..3 {
            add_zombie(&mut w, Vec2::new(96.5, 99.5 + k as f32), 0.0);
        }
        for t in 0..20 * 300 {
            if t % 200 == 0 {
                w.emit_sound_from(player, 20.0, SoundSource::Player(1));
            }
            w.step();
            if let Some(c) = w.break_events.first() {
                assert_eq!((c.tile, c.edge, c.kind), (IVec2::new(100, 100), OwnedEdge::West, opening.broken()));
                assert!(w.edge_hp(EdgeRef::new(c.tile, c.edge)).is_none(), "a broken edge has no hit points");
                return t as f32 * TICK_DT;
            }
            if t == 20 * 20 {
                let (hp, max) = w.edge_hp(EdgeRef::new(IVec2::new(100, 100), OwnedEdge::West)).expect("still standing");
                assert!(hp < max, "{opening:?} not damaged after 20 s");
            }
        }
        panic!("{opening:?} never broke");
    }

    #[test]
    fn windows_break_faster_than_doors() {
        let window = secs_to_break(Edge::Window);
        let door = secs_to_break(Edge::DoorClosed);
        eprintln!("three zombies: window broke after {window:.1} s, door after {door:.1} s");
        assert!(window * 2.5 < door, "window {window:.1} s, door {door:.1} s");
    }

    #[test]
    fn thump_noise_draws_more_zombies() {
        let (mut w, player) = walled_arena(Edge::DoorClosed);
        w.players.clear();
        // Nobody inside to keep them at it: let them keep going anyway for this test.
        w.config.zombie.thump_interest_secs = 1e4;
        // One zombie set on getting through the door, and one 11 m back facing away.
        let thumper = add_zombie(&mut w, Vec2::new(99.5, 100.5), 0.0);
        w.zombies.state[thumper] = ZombieState::Investigate;
        w.zombies.target_pos[thumper] = player;
        w.zombies.timer[thumper] = 1e4;
        let far = add_zombie(&mut w, Vec2::new(88.5, 101.5), std::f32::consts::PI);
        let mut first = None;
        for t in 0..20 * 30 {
            w.step();
            if w.stats.thumps > 0 && first.is_none() {
                first = Some(t);
                assert_eq!(w.zombies.state[far], ZombieState::Wander, "noticed something before the first thump");
            }
        }
        assert!(first.is_some(), "no thumps");
        assert_eq!(w.zombies.state[thumper], ZombieState::Thump);
        // The far one heard it, came over and joined in.
        let door = EdgeRef::new(IVec2::new(100, 100), OwnedEdge::West);
        assert!(w.zombies.pos[far].distance(door.mid()) < 2.0, "didn't come: {}", w.zombies.pos[far]);
        assert_eq!(w.zombies.state[far], ZombieState::Thump);
        assert_eq!(w.zombies.target_pos[far], Vec2::new(100.5, 100.5), "lured to the far side of the door");
    }

    /// Zombies climb through an open window, slowly, and walk through a broken door.
    #[test]
    fn zombies_climb_through_open_windows_slowly() {
        let cross = |opening: Edge| {
            let (mut w, player) = walled_arena(opening);
            w.players.clear();
            let z = add_zombie(&mut w, Vec2::new(98.5, 100.5), 0.0);
            w.zombies.state[z] = ZombieState::Investigate;
            w.zombies.target_pos[z] = player;
            w.zombies.timer[z] = 1e4;
            (0..20 * 60).find(|_| {
                w.step();
                w.zombies.pos[z].x > 101.0
            })
        };
        let door = cross(Edge::DoorBroken).expect("through the broken door");
        let window = cross(Edge::WindowOpen).expect("through the open window");
        // The 0.7 m opening at climb speed instead of shambling pace: most of a second longer.
        assert!(window > door + 15, "window {window} ticks, door {door}");
    }

    /// Battering a door for minutes only ever breaks it; nothing a zombie does opens it.
    #[test]
    fn zombies_never_open_doors() {
        let (mut w, player) = walled_arena(Edge::DoorClosed);
        w.config.zombie.door_hp = 1e6;
        w.players.clear();
        for k in 0..4 {
            let z = add_zombie(&mut w, Vec2::new(98.5, 99.0 + k as f32 * 0.6), 0.0);
            w.zombies.state[z] = ZombieState::Investigate;
            w.zombies.target_pos[z] = player;
            w.zombies.timer[z] = 1e5;
        }
        let mut thumps = 0;
        for t in 0..20 * 180 {
            if t % 200 == 0 {
                // Someone moving about inside keeps them at it.
                w.emit_sound_from(player, 15.0, SoundSource::Player(1));
            }
            w.step();
            thumps += w.stats.thumps;
            assert_eq!(w.map.edge(IVec2::new(100, 100), Side::W), Edge::DoorClosed);
            assert!(w.zombies.pos.iter().all(|p| p.x < 100.0), "a zombie got through a shut door");
        }
        assert!(thumps > 100, "only {thumps} thumps in 3 minutes");
        assert!(w.edge_changes.is_empty());
    }

    #[test]
    fn players_open_climb_through_and_close_windows() {
        let mut w = world(1, 0);
        let house = w.spawn_building;
        let windows = exterior_edges(&w, house, |e| e == Edge::Window);
        let &(edge, inside, outside) = windows
            .iter()
            .find(|(_, a, b)| !w.map.tile_blocked(*a) && !w.map.tile_blocked(*b) && !w.map.tile_blocked(*b + (*b - *a)))
            .expect("a window with room on both sides");
        let (from, to) = (inside.as_vec2() + Vec2::splat(0.5), outside.as_vec2() + Vec2::splat(0.5));
        let dir = (to - from).normalize();
        let id = w.add_player(from);
        w.players[0].state.facing = dir.y.atan2(dir.x);
        // Shut: a wall to walk into.
        let mut seq = 0;
        let mut walk = |w: &mut World, d: Vec2, ticks: u32| {
            for _ in 0..ticks {
                seq += 1;
                w.set_player_input(id, seq, PlayerInput { move_dir: d, facing: d.y.atan2(d.x), run: false, sneak: false });
                w.step();
            }
        };
        walk(&mut w, dir, 20);
        assert_eq!(w.map.building_at(w.player(id).unwrap().state.pos), Some(house), "walked through a shut window");
        // Open it: quiet.
        assert_eq!(w.player_interact(id), Some(Edge::WindowOpen));
        assert_eq!(w.map.owned_edge(edge.tile, edge.edge), Edge::WindowOpen);
        w.step();
        assert!(w.sounds_heard().iter().any(|s| s.radius == w.config.zombie.window_noise && s.radius < w.config.zombie.door_noise));
        // Climb out: much slower than walking the same metre.
        let start = w.player(id).unwrap().state.pos;
        let mut ticks = 0;
        while w.player(id).unwrap().state.pos.distance(start) < 1.2 {
            walk(&mut w, dir, 1);
            ticks += 1;
            assert!(ticks < 200, "stuck in the window at {}", w.player(id).unwrap().state.pos);
        }
        let walking = 1.2 / crate::players::WALK_SPEED / TICK_DT;
        assert!(ticks as f32 > walking * 2.0, "climbed 1.2 m in {ticks} ticks (walking takes {walking:.0})");
        assert_eq!(w.map.room_at(w.player(id).unwrap().state.pos), 0, "should be outside now");
        // Turn round and shut it from outside.
        walk(&mut w, -dir, 1);
        walk(&mut w, Vec2::ZERO, 1);
        assert_eq!(w.player_interact(id), Some(Edge::Window));
        // Smash it: loud, and it stays broken.
        assert_eq!(w.player_smash_window(id), Some(edge));
        assert_eq!(w.map.owned_edge(edge.tile, edge.edge), Edge::WindowBroken);
        w.step();
        assert!(w.sounds_heard().iter().any(|s| s.radius == w.config.zombie.glass_break_noise));
        assert_ne!(w.player_interact(id), Some(Edge::Window), "a broken window can't be shut");
        assert!(w.player_smash_window(id).is_none() || w.map.owned_edge(edge.tile, edge.edge) == Edge::WindowBroken);
        let kinds: Vec<Edge> = w.edge_changes.iter().map(|c| c.kind).collect();
        assert_eq!(&kinds[..3], &[Edge::WindowOpen, Edge::Window, Edge::WindowBroken]);
    }

    /// `n` zombies on the near side of the walled arena's shut door, set on getting to the player
    /// behind it (who stays out of sight).
    fn siege(n: usize) -> (World, Vec<usize>, Vec2) {
        let (mut w, player) = walled_arena(Edge::DoorClosed);
        let mut zs = Vec::new();
        for k in 0..n {
            let z = add_zombie(&mut w, Vec2::new(96.5 - (k / 4) as f32, 99.0 + (k % 4) as f32 * 0.7), 0.0);
            w.zombies.state[z] = ZombieState::Investigate;
            w.zombies.target_pos[z] = player;
            w.zombies.timer[z] = 60.0;
            zs.push(z);
        }
        (w, zs, player)
    }

    /// Step, the player making a noise every `noise_every` ticks (0 = never).
    fn step_siege(w: &mut World, player: Vec2, ticks: u32, noise_every: u32) {
        for t in 0..ticks {
            if noise_every > 0 && t % noise_every == 0 {
                w.emit_sound_from(player, 15.0, SoundSource::Player(1));
            }
            w.step();
        }
    }

    /// Battering a door, they lose interest once the player inside goes quiet (and out of sight):
    /// they stop, mill about, then wander off. A player still making noise keeps them at it.
    #[test]
    fn thumpers_lose_interest_once_the_player_goes_quiet() {
        let t = ZombieTuning::default();
        let ticks = |secs: f32| (secs / TICK_DT) as u32;
        for keep_noisy in [true, false] {
            let (mut w, zs, player) = siege(4);
            w.config.zombie.door_hp = 1e6;
            step_siege(&mut w, player, ticks(15.0), 200);
            let thumping = |w: &World| zs.iter().filter(|&&z| w.zombies.state[z] == ZombieState::Thump).count();
            assert!(thumping(&w) >= 3, "only {} battering", thumping(&w));
            step_siege(&mut w, player, ticks(t.thump_interest_secs + 5.0), if keep_noisy { 240 } else { 0 });
            if keep_noisy {
                assert_eq!(thumping(&w), 4, "a noisy player should keep them all at it");
                continue;
            }
            assert_eq!(thumping(&w), 0, "still battering a door nobody is behind");
            for &z in &zs {
                assert_eq!(w.zombies.state[z], ZombieState::Investigate);
                assert!(w.zombies.bored[z] > 0.0);
                assert!(w.zombies.target_pos[z].x < 100.0, "should look about on its own side");
            }
            // The racket they made doesn't bring them back; after a while they wander.
            step_siege(&mut w, player, ticks(t.investigate_secs + 1.0), 0);
            for &z in &zs {
                assert_eq!(w.zombies.state[z], ZombieState::Wander);
            }
            assert!(w.stats.thumps == 0);
        }
    }

    /// More zombies than fit in front of a door still add pressure: the ones close behind push
    /// in, each of their thumps doing a fraction of a front-rank one.
    #[test]
    fn a_crowd_breaks_a_door_faster() {
        let secs = |n: usize| {
            let (mut w, _, player) = siege(n);
            let (mut crowd, mut full) = (0, 0);
            for t in 0..20 * 400 {
                if t % 200 == 0 {
                    w.emit_sound_from(player, 15.0, SoundSource::Player(1));
                }
                w.step();
                for th in &w.thump_events {
                    if th.damage < w.config.zombie.thump_damage { crowd += 1 } else { full += 1 }
                }
                if !w.break_events.is_empty() {
                    return (t as f32 * TICK_DT, crowd, full);
                }
            }
            panic!("{n} zombies never broke the door");
        };
        let (few, few_crowd, _) = secs(2);
        let (many, many_crowd, many_full) = secs(12);
        eprintln!("door broke after {few:.1} s with 2 zombies, {many:.1} s with 12 ({many_full} full thumps, {many_crowd} from the crowd)");
        assert!(few_crowd <= 2, "two fit at the door ({few_crowd} crowd thumps)");
        assert!(many_crowd > many_full / 2, "the crowd should be pushing: {many_crowd} vs {many_full}");
        assert!(many < few * 0.6, "a crowd of 12 took {many:.1} s, two took {few:.1} s");
    }

    /// A zombie heading for a noise inside a box with no way in (walls only) pushes against the
    /// wall for a while, then gives up: it looks about where it is and then wanders.
    #[test]
    fn a_zombie_stuck_against_walls_gives_up() {
        let mut w = arena();
        let t = w.config.zombie.clone();
        for k in 0..4 {
            w.map.set_edge(IVec2::new(110 + k, 98), Side::N, Edge::Wall);
            w.map.set_edge(IVec2::new(110 + k, 101), Side::S, Edge::Wall);
            w.map.set_edge(IVec2::new(110, 98 + k), Side::W, Edge::Wall);
            w.map.set_edge(IVec2::new(113, 98 + k), Side::E, Edge::Wall);
        }
        w.map_changed();
        let z = add_zombie(&mut w, Vec2::new(103.5, 99.5), 0.0);
        let inside = Vec2::new(111.5, 99.5);
        w.emit_sound(inside, 30.0);
        let ticks = |secs: f32| (secs / TICK_DT) as u32;
        for _ in 0..ticks(t.give_up_secs) {
            w.step();
        }
        assert_eq!(w.zombies.state[z], ZombieState::Investigate);
        assert_eq!(w.zombies.target_pos[z], inside, "still trying");
        assert!(w.zombies.pos[z].distance(Vec2::new(109.7, 99.5)) < 1.5, "should be at the wall: {}", w.zombies.pos[z]);
        for _ in 0..ticks(8.0) {
            w.step();
        }
        assert!(w.zombies.bored[z] > 0.0, "gave up");
        assert_ne!(w.zombies.target_pos[z], inside);
        // The same noise again (not a player's) doesn't bring it back while it's bored.
        w.emit_sound(inside, 30.0);
        for _ in 0..ticks(t.investigate_secs + 1.0) {
            w.step();
        }
        assert_eq!(w.zombies.state[z], ZombieState::Wander);
    }

    /// A player in a fenced pen (fences can't be battered down) can be seen but not reached: a
    /// zombie gives up at once instead of pushing at the fence.
    #[test]
    fn a_chaser_gives_up_on_a_player_it_cannot_reach() {
        let mut w = arena();
        for k in 0..3 {
            w.map.set_edge(IVec2::new(110 + k, 99), Side::N, Edge::Fence);
            w.map.set_edge(IVec2::new(110 + k, 101), Side::S, Edge::Fence);
            w.map.set_edge(IVec2::new(110, 99 + k), Side::W, Edge::Fence);
            w.map.set_edge(IVec2::new(112, 99 + k), Side::E, Edge::Fence);
        }
        w.map_changed();
        let z = add_zombie(&mut w, Vec2::new(101.5, 100.5), 0.0);
        w.add_player(Vec2::new(111.5, 100.5));
        let (mut chased, mut gave_up) = (false, false);
        for _ in 0..20 * 10 {
            w.step();
            chased |= w.zombies.state[z].is_hunting();
            gave_up |= chased && w.zombies.bored[z] > 0.0;
        }
        assert!(chased, "never saw the player");
        assert!(gave_up, "kept after a player it can't reach");
        assert!(w.zombies.pos[z].x < 108.0, "pressed up to the fence: {}", w.zombies.pos[z]);
    }

    /// Corpses are cleared away: beyond `max_corpses` the oldest go first, and any that have lain
    /// `corpse_secs` go. Ids of the rest still resolve, and it's deterministic.
    #[test]
    fn old_corpses_are_cleared_away() {
        let run = || {
            let mut w = arena();
            w.config.zombie.max_corpses = 3;
            w.config.zombie.corpse_secs = 5.0;
            let mut ids = Vec::new();
            for k in 0..8 {
                let z = add_zombie(&mut w, Vec2::new(90.5 + k as f32, 90.5), 0.0);
                ids.push(w.zombies.id[z]);
            }
            let alive = add_zombie(&mut w, Vec2::new(90.5, 95.5), 0.0);
            let alive_id = w.zombies.id[alive];
            // Kill them one tick apart: the first ones are the oldest.
            for &id in &ids {
                let i = w.zombies.index_of(id).unwrap();
                (w.zombies.state[i], w.zombies.hp[i], w.zombies.timer[i]) = (ZombieState::Dead, 0.0, 0.0);
                w.step();
            }
            let left: Vec<u32> = (0..w.zombies.len()).filter(|&i| w.zombies.is_dead(i)).map(|i| w.zombies.id[i]).collect();
            assert_eq!(left, ids[5..], "the three newest corpses stay");
            assert!(w.zombies.index_of(alive_id).is_some_and(|i| w.zombies.pos[i] == Vec2::new(90.5, 95.5)));
            for _ in 0..(5.0 / TICK_DT) as u32 {
                w.step();
            }
            assert!(w.zombies.state.iter().all(|s| *s != ZombieState::Dead), "corpses older than corpse_secs remain");
            assert_eq!(w.zombies.len(), 1);
            w.state_hash()
        };
        assert_eq!(run(), run());
    }

    #[test]
    fn edge_damage_is_reported_once_per_change() {
        let (mut w, player) = walled_arena(Edge::Window);
        let z = add_zombie(&mut w, Vec2::new(99.4, 100.5), 0.0);
        w.zombies.state[z] = ZombieState::Investigate;
        w.zombies.target_pos[z] = player;
        w.zombies.timer[z] = 60.0;
        let window = EdgeRef::new(IVec2::new(100, 100), OwnedEdge::West);
        let mut seen = Vec::new();
        for _ in 0..20 * 30 {
            w.step();
            for (e, frac) in w.take_edge_damage() {
                assert_eq!(e, window);
                seen.push(frac);
            }
            if !w.break_events.is_empty() {
                break;
            }
        }
        let max = w.config.zombie.window_hp;
        let expected: Vec<f32> = (1..max as u32).map(|k| 1.0 - k as f32 / max).chain([1.0]).collect();
        assert_eq!(seen.len(), expected.len(), "{seen:?}");
        for (a, b) in seen.iter().zip(&expected) {
            assert!((a - b).abs() < 1e-4, "{seen:?}");
        }
        assert!(w.take_edge_damage().is_empty());
    }

    #[test]
    fn thousands_of_zombies_tick_fast() {
        let mut w = world(1, 5000);
        w.set_goal(Some(street_point(&w)));
        w.step(); // includes the first flow build
        let start = Instant::now();
        for _ in 0..100 {
            w.step();
        }
        let per_tick = start.elapsed().as_secs_f32() * 10.0;
        eprintln!("5000 zombies, debug goal: {per_tick:.3} ms/tick");
        // Budget is 50 ms per tick at 20 Hz; debug builds are slower, so only fail on something egregious.
        assert!(per_tick < 50.0);

        // With senses: two players running about the street and an alarm pulling a horde.
        let mut w = world(1, 5000);
        let a = w.add_player(street_point(&w));
        let b = w.add_player(street_point(&w) + Vec2::new(0.0, 6.0));
        let mut seq = 0;
        let mut drive = |w: &mut World, t: u32| {
            seq += 1;
            let d = Vec2::from_angle(t as f32 * 0.02);
            w.set_player_input(a, seq, PlayerInput { move_dir: d, facing: 0.0, run: true, sneak: false });
            w.set_player_input(b, seq, PlayerInput { move_dir: -d, facing: 0.0, run: false, sneak: false });
            if t.is_multiple_of(200) {
                w.emit_sound(w.player(a).unwrap().state.pos, 60.0);
            }
        };
        for t in 0..200 {
            drive(&mut w, t);
            w.step();
        }
        let start = Instant::now();
        let (mut senses, mut flow, mut builds) = (0.0, 0.0, 0);
        for t in 200..300 {
            drive(&mut w, t);
            w.step();
            (senses, flow, builds) = (senses + w.stats.senses_ms, flow + w.stats.flow_ms, builds + w.stats.flow_builds);
        }
        let per_tick = start.elapsed().as_secs_f32() * 10.0;
        eprintln!(
            "5000 zombies, senses ({} hunting, {} investigating): {per_tick:.3} ms/tick (senses {:.3} ms, flow {:.3} ms, {builds} field builds in 100 ticks)",
            w.stats.chasing,
            w.stats.investigating,
            senses / 100.0,
            flow / 100.0
        );
        assert!(per_tick < 50.0);
    }

    // --- Simulation tiers (crate::tiers) ---

    use crate::zombies::Tier;

    /// The nearest open tile to `p`.
    fn open_tile_near(w: &World, p: Vec2) -> Vec2 {
        let c = p.floor().as_ivec2();
        for r in 0..40 {
            for dy in -r..=r {
                for dx in -r..=r {
                    let t = c + IVec2::new(dx, dy);
                    if (dx as i32).abs().max((dy as i32).abs()) == r && w.map.in_bounds(t) && !w.map.tile_blocked(t) && w.map.tile(t).room == 0 {
                        return t.as_vec2() + Vec2::splat(0.5);
                    }
                }
            }
        }
        panic!("no open tile near {p}");
    }

    /// A zombie standing in an open field changes tier as the player moves away and back: promoted
    /// at the range, demoted only once its chunk is `hysteresis` beyond it.
    #[test]
    fn tiers_follow_the_player_with_hysteresis() {
        let mut w = arena();
        let tt = w.config.tiers.clone();
        // Chunk (2, 10) spans x 32..48; it looks about the middle of it.
        let z = add_zombie(&mut w, Vec2::new(40.5, 168.5), 0.0);
        (w.zombies.state[z], w.zombies.target_pos[z], w.zombies.timer[z]) = (ZombieState::Investigate, Vec2::new(40.5, 168.5), 1e6);
        w.add_player(Vec2::new(48.0, 168.0));
        // Player `d` metres east of the chunk: step a tick and see the tier.
        let at = |w: &mut World, d: f32| {
            w.players[0].state.pos = Vec2::new(48.0 + d, 168.0);
            w.step();
            w.zombies.tier[z]
        };
        let a = tt.active_range;
        let n = tt.near_range;
        let h = tt.hysteresis;
        let path = [
            (a - 10.0, Tier::Active),
            (a + h - 2.0, Tier::Active),
            (a + h + 2.0, Tier::Near),
            (a + 5.0, Tier::Near),
            (a - 1.0, Tier::Active),
            (n + 10.0, Tier::Near),
            (n + h + 2.0, Tier::Virtual),
            (n + 5.0, Tier::Virtual),
            (n - 1.0, Tier::Near),
            (n + h - 2.0, Tier::Near),
            (500.0, Tier::Virtual),
            (a - 1.0, Tier::Active),
        ];
        for (d, want) in path {
            assert_eq!(at(&mut w, d), want, "player {d} m from the zombie's chunk");
        }
        // Lingering in the band doesn't flip it either way.
        for k in 0..200 {
            assert_eq!(at(&mut w, a + 2.0 + (k % 10) as f32), Tier::Active);
        }
        assert_eq!(w.tier_counts(), [1, 0, 0]);
        assert!(w.zombies.pos[z].distance(Vec2::new(40.5, 168.5)) < w.config.zombie.search_radius + 1.0);
    }

    /// A virtual zombie that hears a loud noise heads there in bulk moves, is still heading there
    /// when it becomes near, and when the player comes close enough to make it active.
    #[test]
    fn a_far_zombie_that_heard_a_noise_is_still_going_there_when_promoted() {
        let mut w = arena();
        let z = add_zombie(&mut w, Vec2::new(250.5, 160.5), 0.0);
        let id = w.zombies.id[z];
        w.add_player(Vec2::new(20.5, 160.5));
        w.step();
        assert_eq!(w.zombies.tier[z], Tier::Virtual);
        let noise = Vec2::new(100.5, 160.5);
        w.emit_sound(noise, 200.0);
        w.step();
        assert_eq!((w.zombies.state[z], w.zombies.target_pos[z]), (ZombieState::Investigate, noise), "heard it");
        // It drifts there while virtual, every few seconds, and turns near on the way.
        let mut last = w.zombies.pos[z].x;
        let mut ticks = 0;
        while w.zombies.tier[z] == Tier::Virtual {
            w.step();
            let x = w.zombies.pos[z].x;
            // (Settling it when it's promoted may nudge it a few centimetres.)
            assert!(x <= last + 0.1, "virtual zombie went the wrong way: {last} -> {x}");
            last = x;
            ticks += 1;
            assert!(ticks < 20 * 120, "never became near (at {})", w.zombies.pos[z]);
        }
        assert_eq!(w.zombies.tier[z], Tier::Near);
        assert!(w.zombies.pos[z].x < 200.0, "should have come a long way: {}", w.zombies.pos[z]);
        assert_eq!((w.zombies.state[z], w.zombies.target_pos[z]), (ZombieState::Investigate, noise), "still going there when near");
        // A few near updates: still on its way, closer.
        let before = w.zombies.pos[z].distance(noise);
        for _ in 0..40 {
            w.step();
        }
        assert_eq!(w.zombies.tier[z], Tier::Near);
        assert!(w.zombies.pos[z].distance(noise) < before - 1.0, "near zombie stopped going there");
        // The player comes over: active, and it carries on to the noise with the full AI.
        let zp = w.zombies.pos[z];
        w.players[0].state.pos = Vec2::new(zp.x, zp.y - 40.0);
        w.step();
        let i = w.zombies.index_of(id).unwrap();
        assert_eq!(w.zombies.tier[i], Tier::Active);
        assert_eq!((w.zombies.state[i], w.zombies.target_pos[i]), (ZombieState::Investigate, noise), "kept its intent when promoted");
        let before = w.zombies.pos[i].distance(noise);
        for _ in 0..20 * 5 {
            w.step();
        }
        assert!(w.zombies.pos[i].distance(noise) < before - 2.0, "active zombie stopped going there");
    }

    /// A chaser the player suddenly leaves far behind drops to the virtual tier still chasing, goes
    /// on to where it last saw them, and is found there (searching) when the player comes back.
    #[test]
    fn a_chaser_left_behind_keeps_going_where_it_last_saw_you() {
        let mut w = arena();
        let z = add_zombie(&mut w, Vec2::new(92.5, 100.5), 0.0);
        w.add_player(Vec2::new(100.5, 100.5));
        for _ in 0..8 {
            w.step();
        }
        assert_eq!(w.zombies.state[z], ZombieState::Chase);
        let seen = w.zombies.target_pos[z];
        w.players[0].state.pos = Vec2::new(300.5, 100.5);
        w.step();
        assert_eq!(w.zombies.tier[z], Tier::Virtual);
        assert!(w.zombies.state[z].is_hunting() && w.zombies.target_pos[z].distance(seen) < 1.0, "{:?} {}", w.zombies.state[z], w.zombies.target_pos[z]);
        for _ in 0..20 * 12 {
            w.step();
        }
        w.players[0].state.pos = Vec2::new(100.5, 145.5);
        w.step();
        assert_eq!(w.zombies.tier[z], Tier::Active);
        assert!(w.zombies.pos[z].distance(seen) < w.config.zombie.search_radius + 1.0, "not where it last saw the player: {}", w.zombies.pos[z]);
        assert!(matches!(w.zombies.state[z], ZombieState::Chase | ZombieState::Investigate), "{:?}", w.zombies.state[z]);
    }

    /// Walking far away and coming back finds the same zombies (by id), roughly where they were,
    /// after they spent the time away on the virtual tier.
    #[test]
    fn walking_away_and_back_finds_the_same_zombies() {
        let mut config = WorldConfig::new(5, 1500, templates()).with_map_chunks(40);
        // At night, so they don't spot the player and follow.
        config.start_hour = 1.0;
        let mut w = World::new(config);
        let home = street_point(&w);
        w.add_player(home);
        w.step();
        let watched: Vec<(u32, Vec2)> = (0..w.zombies.len()).filter(|&i| w.zombies.pos[i].distance(home) < 40.0).map(|i| (w.zombies.id[i], w.zombies.pos[i])).collect();
        assert!(watched.len() >= 20, "only {} zombies around", watched.len());
        assert!(watched.iter().all(|(id, _)| w.zombies.tier[w.zombies.index_of(*id).unwrap()] == Tier::Active));
        // Walk (fast, without footsteps) 250 m along the main street, wait, and walk back.
        let away = home + Vec2::new(250.0, 0.0);
        let walk = |w: &mut World, from: Vec2, to: Vec2| {
            let steps = (from.distance(to) / 0.5) as u32;
            for k in 0..=steps {
                w.players[0].state.pos = from.lerp(to, k as f32 / steps as f32);
                w.step();
            }
        };
        walk(&mut w, home, away);
        for _ in 0..20 * 40 {
            w.step();
        }
        for (id, _) in &watched {
            assert_eq!(w.zombies.tier[w.zombies.index_of(*id).unwrap()], Tier::Virtual, "zombie {id} should be virtual while we're away");
        }
        walk(&mut w, away, home);
        let mut moved: Vec<f32> = Vec::new();
        for (id, was) in &watched {
            let i = w.zombies.index_of(*id).unwrap_or_else(|| panic!("zombie {id} is gone"));
            assert_eq!(w.zombies.tier[i], Tier::Active);
            moved.push(w.zombies.pos[i].distance(*was));
        }
        moved.sort_by(f32::total_cmp);
        let median = moved[moved.len() / 2];
        eprintln!("walked away and back: {} zombies, moved median {median:.1} m, max {:.1} m", moved.len(), moved.last().unwrap());
        assert!(median < 20.0, "they wandered off: median {median:.1} m");
    }

    /// The player sweeps about a shut town (every door and window shut and unbreakable), so
    /// thousands of zombies are promoted and demoted over and over. Nobody ever ends up in a blocked
    /// tile or overlapping a wall, nobody gets into or out of a sealed building, and it's
    /// deterministic.
    fn tier_churn(seed: u64, check: bool) -> (u64, u32, u32) {
        let mut config = WorldConfig::new(seed, 2500, templates()).with_map_chunks(36);
        config.zombie.door_hp = 1e9;
        config.zombie.window_hp = 1e9;
        let mut w = World::new(config);
        for y in 0..w.map.height() {
            for x in 0..w.map.width() {
                for which in [OwnedEdge::North, OwnedEdge::West] {
                    let shut = match w.map.owned_edge(IVec2::new(x, y), which) {
                        Edge::WindowOpen => Edge::Window,
                        Edge::DoorOpen => Edge::DoorClosed,
                        _ => continue,
                    };
                    w.map.set_owned_edge(IVec2::new(x, y), which, shut);
                }
            }
        }
        w.map_changed();
        let size = Vec2::new(w.map.width() as f32, w.map.height() as f32);
        let stops: Vec<Vec2> = [Vec2::new(0.5, 0.5), Vec2::new(0.15, 0.2), Vec2::new(0.85, 0.5), Vec2::new(0.5, 0.85), Vec2::new(0.2, 0.8)].iter().map(|f| open_tile_near(&w, *f * size)).collect();
        w.add_player(stops[0]);
        // Which tiles can be walked to from outdoors (walls, shut doors and windows block; trees
        // don't matter here): the rest are sealed buildings.
        let (mw, mh) = (w.map.width(), w.map.height());
        let mut open = vec![false; (mw * mh) as usize];
        let mut queue = vec![stops[0].floor().as_ivec2()];
        open[(queue[0].y * mw + queue[0].x) as usize] = true;
        while let Some(t) = queue.pop() {
            for side in Side::ALL {
                let n = t + side.offset();
                if w.map.in_bounds(n) && !w.map.edge(t, side).blocks_movement() && !open[(n.y * mw + n.x) as usize] {
                    open[(n.y * mw + n.x) as usize] = true;
                    queue.push(n);
                }
            }
        }
        let sealed_in = |w: &World, p: Vec2| {
            let t = p.floor().as_ivec2();
            (!open[(t.y * mw + t.x) as usize]).then(|| w.map.building_at(p))
        };
        let buildings: Vec<Option<Option<usize>>> = w.zombies.pos.iter().map(|p| sealed_in(&w, *p)).collect();
        let ids = w.zombies.id.clone();
        let mut tiers_before = w.zombies.tier.clone();
        let (mut promoted, mut demoted) = (0, 0);
        for t in 0..1200usize {
            let leg = t / 150;
            let (a, b) = (stops[leg % stops.len()], stops[(leg + 1) % stops.len()]);
            w.players[0].state.pos = a.lerp(b, (t % 150) as f32 / 150.0);
            w.step();
            promoted += w.stats.promoted;
            demoted += w.stats.demoted;
            if !check || t % 3 != 0 {
                continue;
            }
            for i in 0..w.zombies.len() {
                let p = w.zombies.pos[i];
                let (zid, tier) = (w.zombies.id[i], w.zombies.tier[i]);
                let k = ids.binary_search(&zid).unwrap();
                assert_eq!(sealed_in(&w, p), buildings[k], "zombie {zid} ({tier:?}) got into or out of a sealed building at {p}");
                // Virtual zombies stand on free tiles; the others collide like bodies (a centre may
                // be in a tree's tile, clear of the trunk), including the ones promoted this tick.
                if tier == Tier::Virtual {
                    assert!(!w.map.tile_blocked(p.floor().as_ivec2()), "zombie {zid} ({tier:?}) in a blocked tile at {p}");
                } else {
                    let was = tiers_before[k];
                    assert!(w.map.resolve_circle(p, ZOMBIE_RADIUS).distance(p) < 0.05, "zombie {zid} ({tier:?}, was {was:?}) overlapping an obstacle at {p}");
                }
                tiers_before[k] = tier;
            }
        }
        (w.state_hash(), promoted, demoted)
    }

    #[test]
    fn promotion_never_lands_in_walls_and_tiers_are_deterministic() {
        let a = tier_churn(2, true);
        eprintln!("tier churn: {} promotions, {} demotions", a.1, a.2);
        assert!(a.1 > 2000 && a.2 > 2000, "not much churn: {a:?}");
        assert_eq!(a, tier_churn(2, false));
        assert_ne!(a.0, tier_churn(3, false).0);
    }

    /// Issue #2's target: 1,000 active + 20,000 virtual zombies within the 50 ms tick, on a 1 km map
    /// with the town in the middle. One player in the forest west of town with a horde round them,
    /// running in circles and making a racket now and then; the rest of the world populated as usual.
    #[test]
    fn tiers_hold_a_thousand_active_and_twenty_thousand_virtual() {
        let start = Instant::now();
        let mut w = World::new(WorldConfig::new(1, 21_500, templates()).with_map_chunks(64));
        let gen_ms = start.elapsed().as_secs_f32() * 1000.0;
        let s = Instant::now();
        let graph = crate::nav::NavGraph::new(&w.walk);
        let nav_build_ms = s.elapsed().as_secs_f32() * 1000.0;
        assert_eq!(&graph, w.nav().graph());
        let main_y = (w.town_min.y + w.town_max.y) as f32 / 2.0;
        let camp = open_tile_near(&w, Vec2::new(w.town_min.x as f32 * 0.5, main_y));
        w.spawn_zombies(1000, camp, 45.0);
        let id = w.add_player(camp);
        w.players[0].hp = 1e9;
        let mut seq = 0;
        let mut drive = |w: &mut World, t: u32| {
            seq += 1;
            let d = Vec2::from_angle(t as f32 * 0.01);
            w.set_player_input(id, seq, PlayerInput { move_dir: d, facing: 0.0, run: true, sneak: false });
            if t.is_multiple_of(200) {
                w.emit_sound(w.player(id).unwrap().state.pos, 60.0);
            }
        };
        for t in 0..200 {
            drive(&mut w, t);
            w.step();
        }
        let ticks = if cfg!(debug_assertions) { 20 } else { 400 };
        let (mut total, mut worst) = (0.0f32, 0.0f32);
        let mut sum = TickStats::default();
        let mut tiers = [0u32; 3];
        let mut worst_nav = 0.0f32;
        for t in 200..200 + ticks {
            drive(&mut w, t);
            let s = Instant::now();
            w.step();
            let ms = s.elapsed().as_secs_f32() * 1000.0;
            (total, worst) = (total + ms, worst.max(ms));
            let st = w.stats;
            sum.senses_ms += st.senses_ms;
            sum.flow_ms += st.flow_ms;
            sum.tier_ms += st.tier_ms;
            sum.active_ms += st.active_ms;
            sum.near_ms += st.near_ms;
            sum.virtual_ms += st.virtual_ms;
            sum.near_updated += st.near_updated;
            sum.virtual_updated += st.virtual_updated;
            sum.nav_ms += st.nav_ms;
            sum.nav_plans += st.nav_plans;
            sum.nav_expanded += st.nav_expanded;
            worst_nav = worst_nav.max(st.nav_ms);
            tiers = [tiers[0].max(st.active), tiers[1].max(st.near), tiers[2].max(st.virtual_)];
        }
        let n = ticks as f32;
        let avg = total / n;
        eprintln!(
            "1 km map (generated in {gen_ms:.0} ms), {} zombies: {} active / {} near / {} virtual (now; peaks {:?}), {} hunting\n  \
             {ticks} ticks: avg {avg:.3} ms/tick, worst {worst:.3} ms (budget 50 ms)\n  \
             per tick: tiers+index {:.3} ms, senses {:.3} ms, flow {:.3} ms, active AI {:.3} ms, near {:.3} ms ({:.0} updated), virtual {:.3} ms ({:.0} moved)\n  \
             nav: graph of {} nodes / {} links built in {nav_build_ms:.1} ms; {} plans in {ticks} ticks ({} nodes expanded), {:.3} ms/tick, worst {worst_nav:.3} ms; {} cached ({} bytes saved)",
            w.zombies.len(),
            w.stats.active,
            w.stats.near,
            w.stats.virtual_,
            tiers,
            w.stats.chasing,
            sum.tier_ms / n,
            sum.senses_ms / n,
            sum.flow_ms / n,
            sum.active_ms / n,
            sum.near_ms / n,
            sum.near_updated as f32 / n,
            sum.virtual_ms / n,
            sum.virtual_updated as f32 / n,
            graph.len(),
            graph.link_count(),
            sum.nav_plans,
            sum.nav_expanded,
            sum.nav_ms / n,
            w.nav().plan_count(),
            postcard::to_allocvec(&w.nav.to_saved()).unwrap().len(),
        );
        assert!(w.stats.active >= 1000, "only {} active", w.stats.active);
        assert!(w.stats.virtual_ >= 20_000, "only {} virtual", w.stats.virtual_);
        if !cfg!(debug_assertions) {
            assert!(avg < 10.0 && worst < 50.0, "avg {avg:.2} ms, worst {worst:.2} ms");
        }
    }

    // --- Population: density from data, respawn, migration toward noise (crate::population) ---

    use crate::population::{PopulationDef, Zone, parse_population};

    /// Living zombies per open tile in each zone, and the open tiles per zone.
    fn zone_densities(w: &World) -> ([f64; 7], [u32; 7]) {
        let mut area = [0u32; 7];
        for y in 0..w.map.height() {
            for x in 0..w.map.width() {
                let t = IVec2::new(x, y);
                if !w.map.tile_blocked(t) {
                    area[w.zone_at(t) as usize] += 1;
                }
            }
        }
        let mut count = [0u32; 7];
        for p in &w.zombies.pos {
            count[w.zone_at(p.floor().as_ivec2()) as usize] += 1;
        }
        let mut per = [0.0; 7];
        for k in 0..7 {
            per[k] = count[k] as f64 / area[k].max(1) as f64;
        }
        (per, area)
    }

    fn populated(population: PopulationDef, zombies: Option<u32>) -> World {
        let mut config = WorldConfig::new(1, 0, templates()).with_map_chunks(64).with_population(population);
        config.zombies = zombies;
        World::new(config)
    }

    /// The world is populated by the data file: the town core is denser than the deep forest by the
    /// ratio of their densities (within sampling noise), and changing the file changes that.
    #[test]
    fn population_follows_the_data_file() {
        let file = include_str!("../../../assets/data/population.ron");
        let def = parse_population(file).unwrap();
        let w = populated(def.clone(), Some(20_000));
        assert_eq!(w.zombies.len(), 20_000);
        assert!((w.population().total_target() - 20_000.0).abs() < 1.0, "targets add up to {}", w.population().total_target());
        let (per, _) = zone_densities(&w);
        let ratio = |per: &[f64; 7], a: Zone, b: Zone| per[a as usize] / per[b as usize];
        let want = (def.density.town_core / def.density.deep_forest) as f64;
        let got = ratio(&per, Zone::TownCore, Zone::DeepForest);
        eprintln!("town core / deep forest: {got:.1} (file says {want:.1}); street / yard {:.2} (file {:.2})", ratio(&per, Zone::Street, Zone::Yard), def.density.street / def.density.yard);
        assert!((got / want - 1.0).abs() < 0.15, "core/deep forest {got:.1}, want {want:.1}");
        assert!((ratio(&per, Zone::Street, Zone::Yard) / (def.density.street / def.density.yard) as f64 - 1.0).abs() < 0.15);
        // Nobody near where the player wakes up, or in that house.
        assert!(w.zombies.pos.iter().all(|p| p.distance(w.player_spawn) >= SPAWN_CLEAR_RADIUS - 1.0 && w.map.building_at(*p) != Some(w.spawn_building)));

        // Ten times the deep forest in the file: ten times as dense there relative to the core.
        let edited = parse_population(&file.replace("deep_forest: 650.0", "deep_forest: 6500.0")).unwrap();
        let w2 = populated(edited.clone(), Some(20_000));
        let (per2, _) = zone_densities(&w2);
        let got2 = ratio(&per2, Zone::TownCore, Zone::DeepForest);
        let want2 = (edited.density.town_core / edited.density.deep_forest) as f64;
        assert!((got2 / want2 - 1.0).abs() < 0.15, "edited: core/deep forest {got2:.1}, want {want2:.1}");
        assert_ne!(w.state_hash(), w2.state_hash());
        // Without a count the file's densities set the total; `scale` multiplies it.
        let natural = populated(def.clone(), None).zombies.len();
        let doubled = populated(PopulationDef { scale: 2.0, ..def.clone() }, None).zombies.len();
        let busier = populated(edited, None).zombies.len();
        eprintln!("natural population of a 1 km map: {natural}; scale 2: {doubled}; ten times the deep forest: {busier}");
        assert!((doubled as f32 / natural as f32 - 2.0).abs() < 0.01, "{natural} -> {doubled}");
        assert!(busier > natural * 3, "{natural} -> {busier}");
        // The default map comes to about a thousand.
        let mut config = WorldConfig::new(1, 0, templates()).with_population(def);
        config.zombies = None;
        let n = World::new(config).zombies.len();
        assert!((800..1200).contains(&n), "default map: {n} zombies");
    }

    /// An empty grass field `chunks` chunks a side, at noon, with no zombies.
    fn big_arena(chunks: i32, population: PopulationDef) -> World {
        let mut config = WorldConfig::new(11, 0, templates()).with_map_chunks(chunks).with_population(population);
        config.start_hour = 12.0;
        let mut w = World::new(config);
        w.map = TileMap::new(chunks, chunks);
        w.map_changed();
        w
    }

    /// Respawn tops up regions far from the player that nobody has been near lately, and nowhere
    /// else: not near the player, not where the player just was, never in the house they woke up in.
    #[test]
    #[allow(clippy::needless_range_loop)]
    fn respawn_tops_up_far_unseen_regions_only() {
        let respawn = crate::population::RespawnDef {
            enabled: true,
            interval_secs: 10.0,
            min_distance: 200.0,
            seen_distance: 100.0,
            unseen_secs: 120.0,
            top_up_below: 0.75,
            max_per_region: 1000,
            max_per_check: 100_000,
        };
        let def = PopulationDef { respawn, ..Default::default() };
        let mut config = WorldConfig::new(5, 1500, templates()).with_map_chunks(40).with_population(def.clone());
        config.start_hour = 1.0;
        let mut w = World::new(config);
        let home = open_tile_near(&w, Vec2::new(40.0, 40.0));
        let far_corner = open_tile_near(&w, Vec2::new(610.0, 610.0));
        w.add_player(far_corner);
        w.step();
        w.players[0].state.pos = home;
        w.step();
        let seen = w.regions().region_of(far_corner);
        assert!(w.population().target[seen] >= 1.0, "pick a corner with zombies");
        // Everyone gone.
        w.zombies.remove_where(|_| true);
        for _ in 0..20 * 25 {
            w.step();
            if w.stats.respawned > 0 && w.stats.respawned as usize == w.zombies.len() {
                // Just placed: on open ground, never by or in the house the player woke up in, far.
                for (i, p) in w.zombies.pos.iter().enumerate() {
                    assert_ne!(w.map.building_at(*p), Some(w.spawn_building), "respawned in the spawn house");
                    assert!(p.distance(w.player_spawn) >= SPAWN_CLEAR_RADIUS - 1.0, "respawned right by the spawn");
                    assert!(!w.map.tile_blocked(p.floor().as_ivec2()));
                    assert!(p.distance(home) >= def.respawn.min_distance);
                    assert_eq!(w.zombies.tier[i], Tier::Virtual);
                }
            }
        }
        let regions = w.regions().clone();
        let mut counts = vec![0u32; regions.regions()];
        for p in &w.zombies.pos {
            counts[regions.region_of(*p)] += 1;
        }
        let (mut eligible, mut refilled) = (0.0, 0);
        for r in 0..regions.regions() {
            let (lo, hi) = regions.rect(r);
            let d = home.clamp(lo, hi).distance(home);
            if d < def.respawn.min_distance {
                assert_eq!(counts[r], 0, "region {r} near the player was topped up");
            } else if r == seen {
                assert_eq!(counts[r], 0, "region {r} was seen just now");
            } else if w.population().target[r] >= 1.0 {
                eligible += w.population().target[r];
                refilled += counts[r];
                // (Wandering about can carry one or two over a region border.)
                assert!(counts[r] as f32 <= w.population().target[r] * 1.1 + 3.0, "region {r} overfilled: {} for {}", counts[r], w.population().target[r]);
            }
        }
        eprintln!("respawn: {refilled} zombies back (eligible targets {eligible:.0})");
        assert!(refilled as f32 > eligible * 0.9, "only {refilled} of {eligible:.0}");
        assert_eq!(w.population().respawned as usize, w.zombies.len());
        assert!(w.zombies.tier.iter().all(|t| *t == Tier::Virtual));
        // Long after the player left the corner, it fills up too; still nothing near the player.
        for _ in 0..20 * 120 {
            w.step();
        }
        let count_in = |w: &World, r: usize| w.zombies.pos.iter().filter(|p| w.regions().region_of(**p) == r).count();
        assert!(count_in(&w, seen) > 0, "the corner never filled up");
        assert!(w.zombies.pos.iter().all(|p| p.distance(home) > def.respawn.min_distance - 64.0));
        // Off (the default), nothing comes back.
        let mut config = WorldConfig::new(5, 1500, templates()).with_map_chunks(40);
        config.start_hour = 1.0;
        let mut w = World::new(config);
        w.add_player(home);
        w.zombies.remove_where(|_| true);
        for _ in 0..20 * 30 {
            w.step();
        }
        assert_eq!(w.zombies.len(), 0);
    }

    /// Zombies in a cluster `from` (all virtual: the player is far off), a noise of `radius` at
    /// `noise`, maybe in a closed room; steps `minutes` and returns, per minute, how many had come
    /// within 40 m of the noise, plus the world. `prepare` may change the world first.
    #[allow(clippy::too_many_arguments)]
    fn migration_run(def: PopulationDef, noise: Vec2, from: Vec2, radius: f32, room: bool, lasting: bool, minutes: u32, prepare: impl FnOnce(&mut World)) -> (Vec<usize>, World) {
        let mut w = big_arena(40, def);
        prepare(&mut w);
        if room {
            // A shut 4 x 4 room round the noise.
            let c = noise.floor().as_ivec2();
            w.map.rooms.push(crate::map::Room { building: 999, name: "Shed".into(), room_type: "shed".into(), floor: crate::map::Floor::Concrete });
            let id = (w.map.rooms.len() - 1) as u16;
            for dy in -2..2 {
                for dx in -2..2 {
                    w.map.tile_mut(c + IVec2::new(dx, dy)).unwrap().room = id;
                }
            }
            for k in -2..2 {
                w.map.set_edge(c + IVec2::new(k, -2), Side::N, Edge::Wall);
                w.map.set_edge(c + IVec2::new(k, 1), Side::S, Edge::Wall);
                w.map.set_edge(c + IVec2::new(-2, k), Side::W, Edge::Wall);
                w.map.set_edge(c + IVec2::new(1, k), Side::E, Edge::Wall);
            }
            w.map_changed();
            assert_ne!(w.map.room_at(noise), 0);
        }
        w.add_player(Vec2::new(20.5, 20.5));
        let n = w.spawn_zombies(300, from, 30.0) as usize;
        w.step();
        w.step();
        assert_eq!(w.tier_counts(), [0, 0, n as u32], "all far off");
        w.make_noise(noise, radius, lasting);
        let mut arrived = vec![false; w.zombies.len()];
        let mut per_minute = Vec::new();
        for t in 1..=minutes * 60 * 20 {
            w.step();
            if t % 20 == 0 {
                for (k, p) in w.zombies.pos.iter().enumerate() {
                    arrived[k] |= p.distance(noise) < 40.0;
                }
            }
            if t % (60 * 20) == 0 {
                per_minute.push(arrived.iter().filter(|a| **a).count());
            }
            if t == 20 * 20 {
                // Twenty seconds on: nobody far away has reacted yet.
                assert!(w.zombies.state.iter().all(|s| *s == ZombieState::Wander), "the far horde reacted at once");
                assert!(w.zombies.pos.iter().all(|p| p.distance(noise) > 200.0));
            }
        }
        assert_eq!(w.tier_counts(), [0, 0, n as u32], "the test should stay on the virtual tier");
        (per_minute, w)
    }

    /// A loud noise draws virtual zombies in from 400 m away over minutes (and does nothing at
    /// once); with migration off they stay put.
    #[test]
    fn a_loud_noise_draws_the_far_horde_in_over_minutes() {
        let (noise, from) = (Vec2::new(320.5, 600.5), Vec2::new(600.5, 300.5));
        assert!(noise.distance(from) > 400.0);
        let (arrived, _) = migration_run(PopulationDef::default(), noise, from, 120.0, false, false, 20, |_| {});
        eprintln!("outdoor noise 410 m away, arrivals by minute: {arrived:?}");
        assert!(arrived[1] == 0, "arrivals within two minutes: {arrived:?}");
        assert!(*arrived.last().unwrap() >= 300 * 6 / 10, "only {} of 300 came: {arrived:?}", arrived.last().unwrap());
        let mut off = PopulationDef::default();
        off.migration.enabled = false;
        let (arrived, _) = migration_run(off, noise, from, 120.0, false, false, 20, |_| {});
        assert_eq!(*arrived.last().unwrap(), 0, "without migration: {arrived:?}");
    }

    /// A really loud noise inside a shut room still pulls the far horde in (muffled: from less far
    /// than outdoors), where hearing alone only carries `wall_muffle` of it.
    #[test]
    fn an_indoor_loud_noise_still_draws_the_far_horde() {
        let (noise, from) = (Vec2::new(320.5, 500.5), Vec2::new(500.5, 350.5));
        let d = noise.distance(from);
        let def = PopulationDef::default();
        assert!(d > 120.0 * ZombieTuning::default().wall_muffle * 4.0 && d < 120.0 * def.migration.reach_mult * def.migration.indoor_mult);
        let (arrived, _) = migration_run(def.clone(), noise, from, 120.0, true, false, 20, |_| {});
        eprintln!("indoor noise {d:.0} m away, arrivals by minute: {arrived:?}");
        assert!(*arrived.last().unwrap() >= 300 / 2, "only {} of 300 came: {arrived:?}", arrived.last().unwrap());
        let mut off = def;
        off.migration.enabled = false;
        let (arrived, _) = migration_run(off, noise, from, 120.0, true, false, 20, |_| {});
        assert_eq!(*arrived.last().unwrap(), 0, "hearing alone shouldn't reach them: {arrived:?}");
    }

    /// Migration and respawn are deterministic, and the attractors are part of the state hash.
    #[test]
    fn migration_and_respawn_are_deterministic() {
        let run = |lasting: bool| {
            let mut def = PopulationDef::default();
            def.respawn.enabled = true;
            def.respawn.interval_secs = 15.0;
            def.respawn.unseen_secs = 0.0;
            let mut config = WorldConfig::new(9, 3000, templates()).with_map_chunks(48).with_population(def);
            config.start_hour = 1.0;
            let mut w = World::new(config);
            let me = open_tile_near(&w, Vec2::new(60.0, 384.0));
            w.add_player(me);
            // Thin everyone out so respawn has work to do.
            w.zombies.remove_where(|i| i % 2 == 0);
            w.step();
            w.make_noise(open_tile_near(&w, Vec2::new(384.0, 384.0)), 150.0, lasting);
            let after_noise = {
                w.step();
                w.state_hash()
            };
            for _ in 0..20 * 90 {
                w.step();
            }
            (after_noise, w.state_hash(), w.population().respawned, w.zombies.len())
        };
        let a = run(true);
        assert_eq!(a, run(true));
        let b = run(false);
        assert_ne!(a.0, b.0, "a lasting attractor should show in the state hash");
        assert!(a.2 > 0, "respawn never ran");
        eprintln!("determinism run: {a:?}");
    }

    // --- Long-range paths (crate::nav) ---

    /// A row of shut houses (a front and a back door and a window each) along x 380..392 from
    /// `y0` to `y1`, joined by garden fences, with back-garden fences running `wing` metres east at
    /// both ends: a pocket a horde coming from the east walks into. Returns each house's tile box.
    fn house_row(w: &mut World, y0: i32, y1: i32, wing: i32) -> Vec<(IVec2, IVec2)> {
        let (x0, x1) = (380, 392);
        let mut houses = Vec::new();
        let mut y = y0;
        while y + 14 <= y1 {
            let (lo, hi) = (IVec2::new(x0, y), IVec2::new(x1, y + 14));
            for x in lo.x..hi.x {
                w.map.set_edge(IVec2::new(x, lo.y), Side::N, Edge::Wall);
                w.map.set_edge(IVec2::new(x, hi.y - 1), Side::S, Edge::Wall);
            }
            for yy in lo.y..hi.y {
                w.map.set_edge(IVec2::new(lo.x, yy), Side::W, Edge::Wall);
                w.map.set_edge(IVec2::new(hi.x - 1, yy), Side::E, Edge::Wall);
            }
            w.map.set_edge(IVec2::new(lo.x, lo.y + 7), Side::W, Edge::DoorClosed);
            w.map.set_edge(IVec2::new(hi.x - 1, lo.y + 7), Side::E, Edge::DoorClosed);
            w.map.set_edge(IVec2::new(hi.x - 1, lo.y + 10), Side::E, Edge::Window);
            houses.push((lo, hi));
            if y + 15 + 14 <= y1 {
                // The garden fence across the one-metre gap to the next house.
                w.map.set_edge(IVec2::new(x0 + 6, y + 14), Side::W, Edge::Fence);
            }
            y += 15;
        }
        let south = houses.last().unwrap().1.y - 1;
        for x in x1..x1 + wing {
            w.map.set_edge(IVec2::new(x, y0), Side::N, Edge::Fence);
            w.map.set_edge(IVec2::new(x, south), Side::S, Edge::Fence);
        }
        w.map_changed();
        houses
    }

    /// Issue #44's target: a far (virtual) horde drawn to a noise on the far side of a row of shut
    /// houses goes round through the streets instead of piling up against the back gardens, which
    /// is where straight-line movement leaves it; nobody walks through a wall or into a house, and
    /// a few shared plans serve the whole horde.
    #[test]
    fn a_far_horde_finds_the_way_round_a_row_of_houses() {
        let (noise, from) = (Vec2::new(290.5, 320.5), Vec2::new(540.5, 320.5));
        let mut houses = Vec::new();
        let run = |long_paths: bool, houses: &mut Vec<(IVec2, IVec2)>| {
            migration_run(PopulationDef::default(), noise, from, 120.0, false, false, 20, |w| {
                w.config.tiers.long_paths = long_paths;
                *houses = house_row(w, 230, 412, 30);
            })
        };
        let (with, w) = run(true, &mut houses);
        let (without, w_off) = run(false, &mut houses);
        eprintln!("horde 250 m off behind a row of houses, arrivals by minute:\n  long paths: {with:?}\n  straight:   {without:?}");
        eprintln!("  {} plan searches for {} zombies ({} plans cached now)", w.nav().searches, w.zombies.len(), w.nav().plan_count());
        let n = w.zombies.len();
        assert!(*with.last().unwrap() >= n * 7 / 10, "only {} of {n} came round: {with:?}", with.last().unwrap());
        assert!(*without.last().unwrap() <= n / 5, "straight-line movement got round anyway: {without:?}; make the pocket deeper");
        assert!(w.nav().searches <= 80, "{} searches: plans aren't being shared", w.nav().searches);
        for w in [&w, &w_off] {
            for p in &w.zombies.pos {
                let t = p.floor().as_ivec2();
                assert!(!houses.iter().any(|(lo, hi)| t.x >= lo.x && t.y >= lo.y && t.x < hi.x && t.y < hi.y), "a zombie got into a shut house at {p}");
                assert!(!w.map.tile_blocked(t));
            }
        }
    }

    /// The navigation graph follows the map: a door shut between two areas is a costly portal, one
    /// broken open joins them (inside a chunk, into one node; on a chunk border, as an open
    /// portal), shutting an open door makes it a portal again, plans through a changed door are
    /// dropped, and the graph is always the one a fresh build would give.
    #[test]
    fn the_graph_follows_doors_breaking_and_shutting() {
        use crate::nav::{NavGraph, PortalKind};
        let mut w = big_arena(4, PopulationDef::default());
        // A 6 x 6 shed at x 26..32, y 20..26: its east wall is on the border of chunks 1 and 2.
        for k in 0..6 {
            w.map.set_edge(IVec2::new(26 + k, 20), Side::N, Edge::Wall);
            w.map.set_edge(IVec2::new(26 + k, 25), Side::S, Edge::Wall);
            w.map.set_edge(IVec2::new(26, 20 + k), Side::W, Edge::Wall);
            w.map.set_edge(IVec2::new(31, 20 + k), Side::E, Edge::Wall);
        }
        w.map.set_edge(IVec2::new(26, 22), Side::W, Edge::DoorClosed);
        w.map.set_edge(IVec2::new(31, 23), Side::E, Edge::DoorClosed);
        w.map_changed();
        let (inside, west, east) = (IVec2::new(28, 22), IVec2::new(25, 22), IVec2::new(32, 23));
        let link = |w: &World, a: IVec2, b: IVec2| -> Option<PortalKind> {
            let g = w.nav().graph();
            let (na, nb) = (g.node_at(a).unwrap(), g.node_at(b).unwrap());
            if na == nb {
                return Some(PortalKind::Open);
            }
            g.node(na).unwrap().links.iter().filter(|l| l.to == nb).map(|l| l.kind).min()
        };
        // Is there a portal of `kind` across the crossing from `a` to `b`?
        let portal = |w: &World, a: IVec2, b: IVec2, kind: PortalKind| {
            let g = w.nav().graph();
            g.node(g.node_at(a).unwrap()).unwrap().links.iter().any(|l| l.via == a && l.to_tile == b && l.kind == kind)
        };
        let fresh = |w: &World| assert_eq!(w.nav().graph(), &NavGraph::new(&w.walk), "local update differs from a fresh build");
        assert_eq!(link(&w, inside, west), Some(PortalKind::Door));
        assert_eq!(link(&w, inside, east), Some(PortalKind::Door));
        assert_ne!(w.nav().graph().node_at(inside), w.nav().graph().node_at(west));
        // A plan from inside out through the west door.
        let far = Vec2::new(10.5, 22.5);
        let (from, goal) = (w.nav().graph().node_at(inside).unwrap(), w.nav().goal_of(far).unwrap().0);
        assert_eq!(w.nav.route(from, goal, w.tick), crate::nav::Route::Pending);
        w.nav.update(w.tick);
        assert_eq!(w.nav().plan_count(), 1);
        // The west door is battered in: inside and out are one area of the chunk now.
        w.break_edge(EdgeRef::of(IVec2::new(26, 22), Side::W), SoundSource::World);
        assert_eq!(w.nav().graph().node_at(inside), w.nav().graph().node_at(west));
        assert_eq!(w.nav().plan_count(), 0, "the plan through the old door is dropped");
        fresh(&w);
        // The east door, on the chunk border: opened it's an open portal, shut again a door.
        let east_door = EdgeRef::of(IVec2::new(31, 23), Side::E);
        let (door_in, door_out) = (IVec2::new(31, 23), IVec2::new(32, 23));
        assert!(portal(&w, door_in, door_out, PortalKind::Door));
        w.toggle_edge(east_door, SoundSource::World);
        assert_eq!(w.map.owned_edge(east_door.tile, east_door.edge), Edge::DoorOpen);
        assert!(!portal(&w, door_in, door_out, PortalKind::Door));
        assert_eq!(link(&w, door_in, door_out), Some(PortalKind::Open));
        fresh(&w);
        w.toggle_edge(east_door, SoundSource::World);
        assert!(portal(&w, door_in, door_out, PortalKind::Door), "shut again: a door portal");
        fresh(&w);
    }
}
