//! Saving and loading the world: [`World::save`] turns it into bytes and [`World::load`] back. No
//! I/O here (`net::save_file` reads and writes the files).
//!
//! **What's stored.** Everything that isn't derived and that the future depends on: the world
//! settings (seed, map size, tunings), map edits, door and window damage, ground items, players
//! (with their queued inputs and actions, inventories, open container, item jobs, wounds and
//! illnesses, needs and sleep, skills), the contents
//! of every container opened so far (unopened ones fill from the seed when first opened, so they
//! aren't stored), every zombie on every tier with all its AI columns, the
//! population state, the clock and tick, both RNG streams, the sounds waiting to be heard, the
//! flow-field cache and the long-range plans (`crate::nav`). Derived state is rebuilt on load: the
//! walk grid, the navigation graph and the plans' index, spatial hash, region index, chunk
//! distances, the debug goal's field, the container sites (from the map and `containers.ron`) and
//! each player's weapon in hand and load modifiers (from its inventory). Events for replication (edge changes, edge damage,
//! combat events) and per-tick outputs are not saved: a client (re)joining gets the full picture.
//! So save, load and step gives the same [`World::state_hash`] as stepping the original.
//!
//! **The map** is not stored: it is regenerated from the seed and the building templates, and the
//! save carries the edges changed since (doors and windows opened, shut or broken). A 1 km map is
//! a million tiles (several MB even packed) while a session's edits are a few hundred bytes; the
//! price is regenerating on load (about 0.2 s for 1 km). A hash of the map as generated
//! guards against the generator having changed. A map edited by hand ([`World::map_changed`]:
//! tests, editors) is stored whole instead.
//!
//! **Content** (building templates, weapons, items, containers, loot tables, population file, street props,
//! recipes) comes from the caller, as for a new world; the save records a fingerprint of each.
//! Loading against changed content goes on with a note when the change is compatible (weapon and
//! item numbers, weapons and items added, `containers.ron` changes that leave every opened container
//! a container, `loot.ron` (it only decides what unopened containers will hold), `population.ron`,
//! `recipes.ron` (queued crafts of a recipe that's gone are called off); saved weapon, item and
//! recipe indices are matched up by id) and fails with
//! [`LoadError::ContentChanged`] when it isn't (templates that change the map, a weapon or item the
//! save uses removed, an opened container that no longer is one), unless
//! [`LoadOptions::force`] says to load anyway (see [`crate::save_check`]). Tunings that are not
//! in data files yet (`ZombieTuning`, `CombatTuning`, `TierTuning`, `CarryTuning`, `HealthTuning`,
//! `InfectionTuning`, `NeedsTuning`, `SkillTuning`) are saved with the world, as RON
//! text, so a save keeps its own numbers and an added field just takes its default.
//!
//! **Validation.** Values read from a save are checked before the world is used (finite numbers,
//! positions on the map, ids, indices, states that go together): a crafted file with good checksums
//! fails with [`LoadError::Invalid`] naming the field rather than breaking a tick.
//!
//! **Format** (all integers little-endian):
//!
//! ```text
//! magic "ZOMBYSAV" | format version: u32 | sections... | "END\0" section
//! section = tag: [u8; 4] | length: u32 | payload | checksum: u64 (FNV-1a of tag + payload)
//! ```
//!
//! Sections, in order: `INFO` ([`SaveInfo`]: enough to list saves without reading the rest),
//! `CONF` (settings, RON), `MAPD` (map edits), `WRLD` (players, items, clock, RNGs, population,
//! flow cache...), then one `ZOMB` per 64 m region holding zombies (the tiers' region grid), so a
//! later streaming save (#3, #22) can rewrite only the regions that changed, then `RUNS` (each
//! player's run stats and cause of death, `crate::run`; optional, so format 4 saves from before
//! #13 still load, with empty records; written as `RUN2` since PvP added the players killed, and
//! `RUNS` is still read), then `KNOC` (players reeling or down from another player's blow; only when
//! there are any), then `CLMB` (climbs under way over a fence or through a window, started with E;
//! only when there are any), then `AWAY` (#68: which survivors are out of the world, logged out and their
//! time run out, the seconds left to those still lying in it, and spawn protection left; only when
//! there is any of it. Survivors out of the world are stored with the players in `WRLD`, so every
//! check and content change reaches them, and set aside again on load), then `LOOK` (what bodies
//! that rise wore, #63; optional too),
//! then `FARC` (how far the far clock is ahead of the tick, #57; optional, only written when it is),
//! then `SHOV` (zombies shoved lately, whose shoves add up toward a knockdown; only when there are any),
//! then `LEVL` (who and what is upstairs, `crate::stairs`; only when anyone or anything is),
//! then `BAGS` (which pocket or bag each carried stack is in; only when anything is in a bag),
//! then `MODS` (what mod scripts keep, #25, `crate::script`; optional, only for a world with mods),
//! and `XPLR` (what each survivor has seen of the map, `crate::explore`, written after the first
//! nights; only players with anything marked).
//! Payloads are
//! [postcard](https://docs.rs/postcard), and zombies are stored column by column. A file with
//! another format version fails with [`LoadError::Version`] (format 3 added items, inventories and
//! containers, format 4 wounds and illness on players, format 5 first aid: dressings, treatments,
//! painkillers and antibiotics, format 6 needs and sleep on players and each stack's age and goes
//! had, format 7 crafting: craft jobs in players' queues and the recipes in the content
//! fingerprint, format 8 skills: each player's XP in every skill; older saves are refused);
//! truncated or damaged files fail with [`LoadError::Truncated`] or
//! [`LoadError::Corrupt`], never a panic.

use std::fmt;

use glam::{IVec2, Vec2};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::ai::ZombieTuning;
use crate::combat::CombatTuning;
use crate::flow::{SavedFlowCache, WalkGrid};
use crate::health::{HealthTuning, InfectionTuning};
use crate::inventory::CarryTuning;
use crate::items::{GroundItem, ItemStack, complete_items};
use crate::map::{EdgeChange, EdgeRef, TileMap};
use crate::nav::SavedNav;
use crate::needs::NeedsTuning;
use crate::skills::SkillTuning;
use crate::players::Player;
use crate::population::{Attractor, Population, Zone};
use crate::rng::Rng;
use crate::save_check;
use crate::senses::Sound;
use crate::spatial::SpatialHash;
use crate::tiers::{RegionIndex, TierTuning};
use crate::world::{World, WorldConfig, add_fists};
use crate::worldgen::GenConfig;
use crate::zombies::{Tier, ZombieAnim, ZombieState, Zombies};

/// The first eight bytes of every save.
pub const MAGIC: [u8; 8] = *b"ZOMBYSAV";
/// Bumped whenever the layout changes; other versions are refused with a clear error.
pub const FORMAT_VERSION: u32 = 8;
/// File extension `net::save_file` uses.
pub const EXTENSION: &str = "zsave";

const INFO: [u8; 4] = *b"INFO";
const CONF: [u8; 4] = *b"CONF";
const MAPD: [u8; 4] = *b"MAPD";
const WRLD: [u8; 4] = *b"WRLD";
const ZOMB: [u8; 4] = *b"ZOMB";
/// Players' run stats (#13), after the zombies. Optional: a save without it (format 4 from before
/// #13) loads with empty records, so adding it didn't change the format version.
const RUNS: [u8; 4] = *b"RUNS";
/// Run stats as written since PvP (with the players killed); `RUNS` is still read.
const RUN2: [u8; 4] = *b"RUN2";
/// Players reeling or down from another player's blow (PvP; optional, only when there are any).
const KNOC: [u8; 4] = *b"KNOC";
/// Climbs under way (E over a fence or through a window), by player id; only when there are any.
const CLMB: [u8; 4] = *b"CLMB";
/// Blinds on windows and glazed doors and whether each is down (`crate::blinds`), every one, by
/// edge key, and the doors with glass in them ([`BlindSave`]). Optional: a save without it (from before blinds) keeps the ones its map was
/// generated with, so adding it didn't change the format version.
const BLND: [u8; 4] = *b"BLND";

/// The `BLND` section: every blind (edge key, down) and every glazed door (edge key).
type BlindSave = (Vec<((i32, i32, u8), bool)>, Vec<(i32, i32, u8)>);

/// [`crate::run::RunStats`] as the `RUNS` section held it, before players killed were counted.
#[derive(Deserialize)]
struct RunsV1 {
    secs: f64,
    hours: f64,
    kills: u32,
    distance_m: f64,
    items_taken: u32,
    searched: u32,
    wounds: u32,
    hp_lost: f32,
    death: Option<crate::run::Death>,
}

impl From<RunsV1> for crate::run::RunStats {
    fn from(r: RunsV1) -> Self {
        let RunsV1 { secs, hours, kills, distance_m, items_taken, searched, wounds, hp_lost, death } = r;
        Self { secs, hours, kills, distance_m, items_taken, searched, wounds, hp_lost, death, player_kills: 0 }
    }
}

/// Logged-out survivors and spawn protection (#68; optional, only when there is any).
const AWAY: [u8; 4] = *b"AWAY";

/// The `AWAY` section, by player id.
#[derive(Default, PartialEq, Serialize, Deserialize)]
struct AwaySave {
    /// Out of the world (`World::parked`).
    parked: Vec<u32>,
    /// Lying logged out in the world, and the seconds left.
    leaving: Vec<(u32, f32)>,
    /// Spawn protection, and the seconds left.
    protected: Vec<(u32, f32)>,
}

/// Tied hands (#73, `crate::bonds`; optional, only when there are any): the player, the item it
/// is tied with by id (so a changed item list needs no remapping), who tied it, the seconds
/// struggled and whether it is struggling.
const BOND: [u8; 4] = *b"BOND";
type BondSave = Vec<(u32, String, u32, f32, bool)>;
/// Groups, their markers and the invitations out (#73, `crate::groups`; optional, only when there
/// are any).
const GRPS: [u8; 4] = *b"GRPS";

/// What bodies that rise wore (#63; optional, like `RUNS`).
const LOOK: [u8; 4] = *b"LOOK";
/// Each player's first-night record (#19, `crate::firstnight`; optional, like `RUNS`: a save
/// without it has none, and players with no record aren't listed).
const NITE: [u8; 4] = *b"NITE";
/// What each player has seen of the map (`crate::explore`; optional, like `RUNS`: a save without
/// it loads with blank maps).
const XPLR: [u8; 4] = *b"XPLR";
/// How far the far clock has run ahead of the tick (#57; optional, like `RUNS`: a save without it
/// has it at 0, and it is only written when it isn't).
const FARC: [u8; 4] = *b"FARC";
/// What mod scripts keep in the world (#25, `crate::script::ScriptState`: their store, random
/// stream, events not yet handed to them, and which mods they were; optional, like `RUNS`: only
/// written for a world with mods or with something of theirs in it).
const MODS: [u8; 4] = *b"MODS";
/// The power sources and light switches ([`crate::power`]); optional (a save without it gets a new
/// world's: the grid as the settings say, the lights left on).
const POWR: [u8; 4] = *b"POWR";
/// Zombies shoved lately ([`crate::combat::Shoved`]), adding up toward a knockdown; only when
/// there are any, so adding it didn't change the format version.
const SHOV: [u8; 4] = *b"SHOV";
/// The players upstairs (id and floor) and the ground items lying there (`crate::stairs`); only
/// when there are any, so adding it didn't change the format version.
const LEVL: [u8; 4] = *b"LEVL";
/// Which pouch (pockets, or which bag) each carried stack is in, per player; only players with
/// anything in a bag, so adding it didn't change the format version. Without it (a save from
/// before bags had their own room) everything is put away afresh on load.
const BAGS: [u8; 4] = *b"BAGS";
/// Who each survivor was ([`crate::origin::Origin`]: Background, Strengths and Flaws, and what
/// they come to); only survivors that have one, so adding it didn't change the format version.
const ORIG: [u8; 4] = *b"ORIG";
const END: [u8; 4] = *b"END\0";
/// Bytes before the first section: magic and version.
const HEADER: usize = 12;

/// What a save holds, in brief: the first section, readable on its own (see [`read_info`]).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SaveInfo {
    /// Version of the game that wrote it.
    pub game_version: String,
    pub tick: u64,
    /// Hour of day in [0, 24).
    pub time_of_day: f32,
    pub seed: u64,
    /// Map size in tiles.
    pub map_size: IVec2,
    /// Zombies stored (corpses included) and how many of them are alive.
    pub zombies: u32,
    pub alive: u32,
    pub players: u32,
    /// Fingerprint of the content the world was made from.
    pub content: ContentPrint,
}

/// Fingerprints of the data a world was built from: hashes of building templates, weapons and the
/// population file, plus the ids, to say what changed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ContentPrint {
    /// The building templates and `street_props.ron` (#61) together: both decide the map.
    pub templates: u64,
    pub template_ids: Vec<String>,
    pub weapons: u64,
    pub weapon_ids: Vec<String>,
    /// Items (with the plain ones made for weapons that have none) and containers.
    pub items: u64,
    pub item_ids: Vec<String>,
    /// `containers.ron` and `loot.ron` together: both only say how containers fill (one hash, so
    /// the save layout didn't change when loot tables came in).
    pub containers: u64,
    pub population: u64,
    /// `recipes.ron` (#12, format 7): queued crafts name recipes by index, matched up by id.
    pub recipes: u64,
    pub recipe_ids: Vec<String>,
}

impl ContentPrint {
    /// The fingerprint of a world's config (bare hands added as a world would).
    pub fn of(config: &WorldConfig) -> Self {
        let mut weapons = config.weapons.clone();
        add_fists(&mut weapons);
        let mut items = config.items.clone();
        complete_items(&mut items, &weapons);
        Self {
            // The street props decide the map too: hashed with the templates (no change to the save
            // layout; with none, the hash is the templates' alone, as before them).
            templates: if config.street_props.is_empty() { fnv(&encode(&config.templates)) } else { fnv(&encode(&(&config.templates, &config.street_props))) },
            template_ids: config.templates.iter().map(|t| t.id.clone()).collect(),
            weapons: fnv(&encode(&weapons)),
            weapon_ids: weapons.iter().map(|w| w.id.clone()).collect(),
            items: fnv(&encode(&items)),
            item_ids: items.iter().map(|d| d.id.clone()).collect(),
            containers: fnv(&encode(&(&config.containers, &config.loot))),
            population: fnv(&encode(&config.population)),
            recipes: fnv(&encode(&config.recipes)),
            recipe_ids: config.recipes.iter().map(|r| r.id.clone()).collect(),
        }
    }
}

/// Why a save couldn't be loaded.
#[derive(Clone, Debug, PartialEq)]
pub enum LoadError {
    /// Doesn't start with [`MAGIC`].
    NotASave,
    /// Written with another format version.
    Version { found: u32, supported: u32 },
    /// Cut short (a partial write or download).
    Truncated,
    /// Damaged or inconsistent: a bad checksum, undecodable data, or values that don't fit.
    Corrupt(String),
    /// Made with content changed in ways it can't take without [`LoadOptions::force`]; each entry
    /// says what changed.
    ContentChanged(Vec<String>),
    /// The map generated from the saved seed and templates isn't the one the save was made on
    /// (the generator changed). [`LoadOptions::force`] loads it onto the new map.
    MapChanged,
    /// Intact, but holding a value no tick could have made (NaN, off the map, a repeated id, an index
    /// past the end of what it indexes...): says which field.
    Invalid(String),
}

impl LoadError {
    /// Would loading anyway ([`LoadOptions::force`]) get past this?
    pub fn forceable(&self) -> bool {
        matches!(self, Self::ContentChanged(_) | Self::MapChanged)
    }
}

/// How to load a save.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LoadOptions {
    /// Load anyway when the content or map changed in a way the save can't simply take: drop what
    /// no longer fits (see [`crate::save_check`]) and say so in [`Loaded::notes`].
    pub force: bool,
}

/// A loaded world, and what loading it had to say (`notes`): compatible content changes, and what a
/// forced load changed. No notes for a save loaded against the content it was made with.
pub struct Loaded {
    pub world: World,
    pub notes: Vec<String>,
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotASave => write!(f, "not a Zombye save file"),
            Self::Version { found, supported } => {
                write!(f, "save format version {found} is not supported (this build reads version {supported}); it was written by {} build", if found > supported { "a newer" } else { "an older" })
            }
            Self::Truncated => write!(f, "save file is truncated (cut short while writing or copying?)"),
            Self::Corrupt(why) => write!(f, "save file is damaged: {why}"),
            Self::ContentChanged(what) => write!(f, "save was made with different game data: {}", what.join("; ")),
            Self::MapChanged => write!(f, "the map generator has changed since this save was made, so its map can't be rebuilt"),
            Self::Invalid(why) => write!(f, "save file holds impossible values: {why}"),
        }
    }
}

impl std::error::Error for LoadError {}

/// World settings that aren't content: stored as RON in the `CONF` section.
#[derive(Serialize, Deserialize)]
struct Settings {
    generation: GenConfig,
    zombies: Option<u32>,
    start_hour: f32,
    day_length_secs: f32,
    zombie: ZombieTuning,
    combat: CombatTuning,
    tiers: TierTuning,
    #[serde(default)]
    carry: CarryTuning,
    #[serde(default)]
    health: HealthTuning,
    #[serde(default)]
    infection: InfectionTuning,
    #[serde(default)]
    needs: NeedsTuning,
    #[serde(default)]
    skills: SkillTuning,
    #[serde(default)]
    power: crate::power::PowerDef,
    /// The seed loot rolls from (`WorldConfig::loot_seed`); none in older saves: the world seed.
    #[serde(default)]
    loot_seed: Option<u64>,
    #[serde(default)]
    blinds: crate::blinds::BlindTuning,
    /// A difficulty's multipliers on the content (`crate::difficulty`); none in older saves.
    #[serde(default)]
    sandbox: crate::difficulty::Sandbox,
}

#[derive(Serialize, Deserialize)]
enum MapLayout {
    /// Edges changed since generation, in edge order, with what they are now.
    Edits(Vec<EdgeChange>),
    /// The whole map (it was edited by hand).
    Full(TileMap),
}

#[derive(Serialize, Deserialize)]
struct MapSave {
    /// Hash of the map as generated ([`map_hash`]).
    base_hash: u64,
    town_min: IVec2,
    town_max: IVec2,
    player_spawn: Vec2,
    spawn_building: u32,
    layout: MapLayout,
}

#[derive(Serialize, Deserialize)]
struct PopulationSave {
    target: Vec<f32>,
    zone: Vec<Zone>,
    last_seen: Vec<u64>,
    attractors: Vec<Attractor>,
    next_attractor: u32,
    respawned: u32,
}

#[derive(Serialize, Deserialize)]
struct WorldSave {
    tick: u64,
    clock_offset_hours: f32,
    rng: u64,
    combat_rng: u64,
    next_id: u32,
    next_player_id: u32,
    next_item_id: u32,
    players: Vec<Player>,
    items: Vec<GroundItem>,
    /// Opened containers' contents, by container id, in id order.
    opened: Vec<(u32, Vec<ItemStack>)>,
    /// Damaged doors and windows by [`EdgeRef::key`], in key order.
    edge_hp: Vec<((i32, i32, u8), f32)>,
    /// Sounds made since the last tick (heard by the next), and those heard by the last.
    sounds: Vec<Sound>,
    heard: Vec<Sound>,
    goal: Option<IVec2>,
    population: PopulationSave,
    flow: SavedFlowCache,
    /// Long-range plans and queued plan requests.
    nav: SavedNav,
    /// Zombies in all the `ZOMB` sections together.
    zombies: u32,
}

/// The zombie columns a save stores, each of `Zombies`' columns but `id` (stored as deltas). The
/// struct patterns below name every column, so adding one to `Zombies` without saving it doesn't
/// compile.
macro_rules! zombie_block {
    ($($col:ident: $ty:ty),* $(,)?) => {
        /// The zombies of one region, column by column, in id order.
        #[derive(Default, Serialize, Deserialize)]
        struct ZombieBlock {
            region: u32,
            /// The first id, then the gap to each next one.
            id_gaps: Vec<u32>,
            $($col: Vec<$ty>,)*
        }

        impl ZombieBlock {
            /// The zombies at `indices` (ascending).
            fn gather(z: &Zombies, region: u32, indices: &[u32]) -> Self {
                let Zombies { id, $($col,)* } = z;
                let mut last = 0u32;
                let id_gaps = indices
                    .iter()
                    .map(|&i| {
                        let gap = id[i as usize].wrapping_sub(last);
                        last = id[i as usize];
                        gap
                    })
                    .collect();
                Self { region, id_gaps, $($col: indices.iter().map(|&i| $col[i as usize]).collect(),)* }
            }

            fn columns_agree(&self) -> bool {
                let n = self.id_gaps.len();
                true $(&& self.$col.len() == n)*
            }

            fn append_to(self, out: &mut Zombies) {
                let mut id = 0u32;
                for gap in self.id_gaps {
                    id = id.wrapping_add(gap);
                    out.id.push(id);
                }
                $(out.$col.extend(self.$col);)*
            }
        }

        /// `z` reordered so row `k` is old row `order[k]`.
        fn reorder(z: Zombies, order: &[usize]) -> Zombies {
            let Zombies { id, $($col,)* } = z;
            Zombies { id: order.iter().map(|&i| id[i]).collect(), $($col: order.iter().map(|&i| $col[i]).collect(),)* }
        }
    };
}

zombie_block! {
    pos: Vec2,
    vel: Vec2,
    facing: f32,
    speed: f32,
    anim: ZombieAnim,
    wander: Vec2,
    wander_timer: f32,
    state: ZombieState,
    target_player: u32,
    target_pos: Vec2,
    timer: f32,
    cooldown: f32,
    groan_timer: f32,
    sees: u8,
    direct: bool,
    thump_edge: EdgeRef,
    hp: f32,
    anchor: Vec2,
    stuck: f32,
    bored: f32,
    tier: Tier,
}

fn encode<T: Serialize + ?Sized>(value: &T) -> Vec<u8> {
    postcard::to_allocvec(value).expect("encoding to memory doesn't fail")
}

fn decode<T: DeserializeOwned>(tag: [u8; 4], bytes: &[u8]) -> Result<T, LoadError> {
    postcard::from_bytes(bytes).map_err(|e| LoadError::Corrupt(format!("{} section: {e}", tag_name(tag))))
}

fn tag_name(tag: [u8; 4]) -> String {
    String::from_utf8_lossy(&tag).trim_end_matches('\0').to_string()
}

/// 64-bit FNV-1a.
fn fnv(bytes: &[u8]) -> u64 {
    fnv_from(0xcbf2_9ce4_8422_2325, bytes)
}

fn fnv_from(mut h: u64, bytes: &[u8]) -> u64 {
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

fn checksum(tag: [u8; 4], payload: &[u8]) -> u64 {
    fnv_from(fnv(&tag), payload)
}

/// A hash of every tile (floor, edges, feature, room) and the room and building counts.
pub fn map_hash(map: &TileMap) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut mix = |v: u64| h = (h ^ v).wrapping_mul(0x0100_0000_01b3).rotate_left(29);
    mix(map.chunks_x as u64);
    mix(map.chunks_y as u64);
    mix(map.rooms.len() as u64);
    mix(map.buildings.len() as u64);
    for cy in 0..map.chunks_y {
        for cx in 0..map.chunks_x {
            for t in &map.chunk(cx, cy).expect("in range").tiles {
                mix(t.floor as u64 | (t.north as u64) << 8 | (t.west as u64) << 16 | (t.feature as u64) << 24 | (t.room as u64) << 32);
            }
        }
    }
    // Street props (#61), the ones that don't block too.
    for p in &map.street_props {
        mix((p.tile.x as u32 as u64) | (p.tile.y as u32 as u64) << 32);
        mix(p.kind as u64 | (p.facing as u64) << 16);
    }
    h
}

struct Writer(Vec<u8>);

impl Writer {
    fn new() -> Self {
        let mut out = Vec::with_capacity(1 << 20);
        out.extend_from_slice(&MAGIC);
        out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        Self(out)
    }

    fn section(&mut self, tag: [u8; 4], payload: &[u8]) {
        let len = u32::try_from(payload.len()).expect("a save section under 4 GB");
        self.0.extend_from_slice(&tag);
        self.0.extend_from_slice(&len.to_le_bytes());
        self.0.extend_from_slice(payload);
        self.0.extend_from_slice(&checksum(tag, payload).to_le_bytes());
    }
}

/// Reads sections one at a time, checking lengths and checksums.
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Result<Self, LoadError> {
        if bytes.len() < MAGIC.len() || bytes[..MAGIC.len()] != MAGIC {
            // A file cut off inside the magic still looks like a save that was cut short.
            return Err(if !bytes.is_empty() && bytes.len() < MAGIC.len() && MAGIC.starts_with(bytes) { LoadError::Truncated } else { LoadError::NotASave });
        }
        let version = bytes.get(8..HEADER).ok_or(LoadError::Truncated)?;
        let found = u32::from_le_bytes(version.try_into().expect("four bytes"));
        if found != FORMAT_VERSION {
            return Err(LoadError::Version { found, supported: FORMAT_VERSION });
        }
        Ok(Self { bytes, at: HEADER })
    }

    /// The next section's tag and payload.
    fn next(&mut self) -> Result<([u8; 4], &'a [u8]), LoadError> {
        let head = self.bytes.get(self.at..self.at + 8).ok_or(LoadError::Truncated)?;
        let tag: [u8; 4] = head[..4].try_into().expect("four bytes");
        let len = u32::from_le_bytes(head[4..].try_into().expect("four bytes")) as usize;
        let start = self.at + 8;
        let end = start.checked_add(len).ok_or(LoadError::Truncated)?;
        let payload = self.bytes.get(start..end).ok_or(LoadError::Truncated)?;
        let sum = self.bytes.get(end..end + 8).ok_or(LoadError::Truncated)?;
        if u64::from_le_bytes(sum.try_into().expect("eight bytes")) != checksum(tag, payload) {
            return Err(LoadError::Corrupt(format!("{} section fails its checksum", tag_name(tag))));
        }
        self.at = end + 8;
        Ok((tag, payload))
    }

    /// The next section, which must be `tag`.
    fn expect(&mut self, tag: [u8; 4]) -> Result<&'a [u8], LoadError> {
        let (found, payload) = self.next()?;
        if found != tag {
            return Err(LoadError::Corrupt(format!("expected the {} section, found {}", tag_name(tag), tag_name(found))));
        }
        Ok(payload)
    }
}

/// Read just the [`SaveInfo`] at the start of a save. `bytes` may be only the beginning of the
/// file (the info section is small; 64 KiB is plenty).
pub fn read_info(bytes: &[u8]) -> Result<SaveInfo, LoadError> {
    let mut r = Reader::new(bytes)?;
    decode(INFO, r.expect(INFO)?)
}

fn corrupt(why: impl Into<String>) -> LoadError {
    LoadError::Corrupt(why.into())
}

impl World {
    /// The whole world as bytes (see the [module docs](self) for what's in it). Load it with
    /// [`World::load`].
    pub fn save(&self) -> Vec<u8> {
        // Every field is named, so a new one has to be sorted into saved or derived here.
        let World {
            config,
            map,
            town_min,
            town_max,
            player_spawn,
            spawn_building,
            zombies,
            players,
            tick,
            clock_offset_hours,
            far_ticks,
            items,
            next_item_id,
            opened,
            edge_hp,
            sounds,
            heard,
            goal,
            walk,
            flow_cache,
            nav,
            rng,
            combat_rng,
            regions,
            population,
            next_id,
            next_player_id,
            map_edits,
            map_custom,
            map_base_hash,
            // Rebuilt on load: from the content, the map, the zombies or the players.
            town_core: _,
            unarmed: _,
            item_weapons: _,
            sites: _,
            goal_flow: _,
            goal_dirty: _,
            hash: _,
            chunk_dist: _,
            // Replication queues and last tick's outputs: a (re)joining client gets everything.
            stats: _,
            edge_changes: _,
            blind_changes: _,
            edge_hp_dirty: _,
            combat_events: _,
            attack_events: _,
            thump_events: _,
            break_events: _,
            action_events: _,
            hit_events: _,
            player_hit_events: _,
            removed_corpses: _,
            left_world: _,
            item_notes: _,
            used_items: _,
            woke: _,
            level_ups: _,
            speech: _,
            groups,
            // Said again by whoever runs the world (a server's join names).
            names: _,
            skill_mods_due: _,
            private_dirty: _,
            rise_looks,
            shoved,
            upstairs,
            parked,
            // Scratch, refilled every tick.
            targets: _,
            // Derived from the players and each tick's sounds (#66); nothing reads it back.
            awareness: _,
            scratch_tiles: _,
            scratch_marks: _,
            scratch_pos: _,
            scratch_actions: _,
            scratch_hits: _,
            scratch_tops: _,
            // Lag compensation (#23): a few ticks of positions, kept only while remote clients
            // report what they see, and they report again every tick.
            lag_history: _,
            scratch_trades: _,
            // The script host is made again from the content's scripts; what scripts keep is saved.
            scripts: _,
            script,
            script_errors: _,
            power,
        } = self;
        // Survivors out of the world are saved among the players (in id order); `AWAY` says which.
        let away = AwaySave {
            parked: parked.iter().map(|p| p.id).collect(),
            leaving: players.iter().filter_map(|p| p.away.map(|s| (p.id, s))).collect(),
            protected: players.iter().filter(|p| p.protected > 0.0).map(|p| (p.id, p.protected)).collect(),
        };
        let all_players: Vec<Player>;
        let players = if parked.is_empty() {
            players
        } else {
            let mut all: Vec<Player> = players.iter().chain(parked).cloned().collect();
            all.sort_by_key(|p| p.id);
            all_players = all;
            &all_players
        };
        let mut w = Writer::new();
        let alive = zombies.state.iter().filter(|s| **s != ZombieState::Dead).count() as u32;
        let info = SaveInfo {
            game_version: env!("CARGO_PKG_VERSION").to_string(),
            tick: *tick,
            time_of_day: self.time_of_day(),
            seed: config.seed(),
            map_size: IVec2::new(map.width(), map.height()),
            zombies: zombies.len() as u32,
            alive,
            players: players.len() as u32,
            content: ContentPrint::of(config),
        };
        w.section(INFO, &encode(&info));
        let settings = Settings {
            generation: config.generation.clone(),
            zombies: config.zombies,
            start_hour: config.start_hour,
            day_length_secs: config.day_length_secs,
            zombie: config.zombie.clone(),
            combat: config.combat.clone(),
            tiers: config.tiers.clone(),
            carry: config.carry.clone(),
            health: config.health.clone(),
            infection: config.infection.clone(),
            needs: config.needs,
            skills: config.skills.clone(),
            power: config.power.clone(),
            loot_seed: config.loot_seed,
            blinds: config.blinds.clone(),
            sandbox: config.sandbox.clone(),
        };
        w.section(CONF, ron::to_string(&settings).expect("settings serialize").as_bytes());
        let layout = if *map_custom {
            MapLayout::Full(map.clone())
        } else {
            MapLayout::Edits(
                map_edits
                    .iter()
                    .map(|&key| {
                        let e = crate::world::edge_of_key(key);
                        EdgeChange { tile: e.tile, edge: e.edge, kind: map.owned_edge(e.tile, e.edge) }
                    })
                    .collect(),
            )
        };
        let map_save =
            MapSave { base_hash: *map_base_hash, town_min: *town_min, town_max: *town_max, player_spawn: *player_spawn, spawn_building: *spawn_building as u32, layout };
        w.section(MAPD, &encode(&map_save));
        let world = WorldSave {
            tick: *tick,
            clock_offset_hours: *clock_offset_hours,
            rng: rng.state(),
            combat_rng: combat_rng.state(),
            next_id: *next_id,
            next_player_id: *next_player_id,
            next_item_id: *next_item_id,
            players: players.clone(),
            items: items.clone(),
            opened: opened.iter().map(|(k, v)| (*k, v.clone())).collect(),
            edge_hp: edge_hp.iter().map(|(k, v)| (*k, *v)).collect(),
            sounds: sounds.clone(),
            heard: heard.clone(),
            goal: *goal,
            population: PopulationSave {
                target: population.target.clone(),
                zone: population.zone.clone(),
                last_seen: population.last_seen.clone(),
                attractors: population.attractors.clone(),
                next_attractor: population.next_attractor,
                respawned: population.respawned,
            },
            flow: flow_cache.to_saved(walk),
            nav: nav.to_saved(),
            zombies: zombies.len() as u32,
        };
        w.section(WRLD, &encode(&world));
        // Zombies by region (where they stand now), each region in id order.
        let mut by_region: Vec<Vec<u32>> = vec![Vec::new(); regions.regions()];
        for (i, p) in zombies.pos.iter().enumerate() {
            by_region[regions.region_of(*p)].push(i as u32);
        }
        for (r, indices) in by_region.iter().enumerate().filter(|(_, v)| !v.is_empty()) {
            w.section(ZOMB, &encode(&ZombieBlock::gather(zombies, r as u32, indices)));
        }
        let runs: Vec<(u32, crate::run::RunStats)> = players.iter().map(|p| (p.id, p.run.clone())).collect();
        w.section(RUN2, &encode(&runs));
        // Players reeling or down from another's blow (PvP); none, no section.
        let knocks: Vec<(u32, crate::players::Knock)> = players.iter().filter_map(|p| p.knock.map(|k| (p.id, k))).collect();
        if !knocks.is_empty() {
            w.section(KNOC, &encode(&knocks));
        }
        let climbs: Vec<(u32, crate::players::Climb)> = players.iter().filter_map(|p| p.state.climb.map(|c| (p.id, c))).collect();
        if !climbs.is_empty() {
            w.section(CLMB, &encode(&climbs));
        }
        if away != AwaySave::default() {
            w.section(AWAY, &encode(&away));
        }
        // Every blind (the map's own `blinds` aren't stored with its tiles), even none: the save's
        // blinds replace those the map is generated with.
        // The glazed doors too: a map stored whole (or regenerated anyway) needn't have them where
        // the generated one does.
        let blinds: BlindSave = (map.blinds.iter().map(|(k, v)| (*k, *v)).collect(), map.glazed.iter().copied().collect());
        w.section(BLND, &encode(&blinds));
        // Tied hands and groups (#73); none, no section.
        let bonds: BondSave =
            players.iter().filter_map(|p| p.restraint.map(|r| (p.id, config.items.get(r.item as usize).map(|d| d.id.clone()).unwrap_or_default(), r.by, r.struggled, r.struggling))).collect();
        if !bonds.is_empty() {
            w.section(BOND, &encode(&bonds));
        }
        if !groups.is_empty() {
            w.section(GRPS, &encode(groups));
        }
        w.section(LOOK, &encode(rise_looks));
        let nights: Vec<(u32, crate::firstnight::FirstNight)> = players.iter().filter_map(|p| Some((p.id, p.first_night.clone()?))).collect();
        if !nights.is_empty() {
            w.section(NITE, &encode(&nights));
        }
        let origins: Vec<(u32, crate::origin::Origin)> = players.iter().filter(|p| !p.origin.is_plain()).map(|p| (p.id, p.origin.clone())).collect();
        if !origins.is_empty() {
            w.section(ORIG, &encode(&origins));
        }
        let maps: Vec<(u32, &crate::explore::Explored)> = players.iter().filter(|p| p.explored.count() > 0).map(|p| (p.id, &p.explored)).collect();
        if !maps.is_empty() {
            w.section(XPLR, &encode(&maps));
        }
        if *far_ticks != 0 {
            w.section(FARC, &encode(far_ticks));
        }
        w.section(POWR, &encode(&power.to_save()));
        if !shoved.is_empty() {
            w.section(SHOV, &encode(shoved));
        }
        let levels: Vec<(u32, u8)> = players.iter().filter(|p| p.state.level > 0).map(|p| (p.id, p.state.level)).collect();
        let up_items: Vec<u32> = upstairs.iter().copied().filter(|id| items.binary_search_by_key(id, |it| it.id).is_ok()).collect();
        if !levels.is_empty() || !up_items.is_empty() {
            w.section(LEVL, &encode(&(levels, up_items)));
        }
        let bags: Vec<(u32, Vec<u8>)> = players.iter().filter(|p| p.inventory.carried.iter().any(|s| s.home != crate::items::Pouch::Pockets)).map(|p| (p.id, p.inventory.homes())).collect();
        if !bags.is_empty() {
            w.section(BAGS, &encode(&bags));
        }
        // Mod scripts (#25): what they keep, and whose they are. A world without mods has none.
        let mods = config.scripts.as_ref().map(|e| e.mods()).unwrap_or_default();
        if !script.is_empty() || !mods.is_empty() {
            w.section(MODS, &encode(&crate::script::ScriptState { mods, ..script.clone() }));
        }
        w.section(END, &[]);
        w.0
    }

    /// Rebuild a world from [`World::save`]'s bytes. `content` supplies the building templates,
    /// weapons and population settings (as for [`World::new`]); everything else in it is replaced
    /// by what was saved. Compatible content changes load (see [`World::load_with`] for what they
    /// were); anything that isn't an intact save of this format, holding values a tick could have
    /// made, fails without panicking.
    pub fn load(bytes: &[u8], content: &WorldConfig) -> Result<World, LoadError> {
        Self::load_with(bytes, content, LoadOptions::default()).map(|l| l.world)
    }

    /// [`World::load`], with options (load anyway), returning what loading had to say about
    /// changed content and what a forced load changed.
    pub fn load_with(bytes: &[u8], content: &WorldConfig, options: LoadOptions) -> Result<Loaded, LoadError> {
        let force = options.force;
        let mut r = Reader::new(bytes)?;
        let info: SaveInfo = decode(INFO, r.expect(INFO)?)?;
        let text = std::str::from_utf8(r.expect(CONF)?).map_err(|_| corrupt("settings aren't text"))?;
        let settings: Settings = ron::from_str(text).map_err(|e| corrupt(format!("settings: {e}")))?;
        // The game's difficulty multiplies the content (`crate::difficulty`): put it on what we
        // were given first, so the content is compared as the save had it.
        let mut config = content.clone();
        settings.sandbox.check().map_err(|why| save_check::invalid(format!("settings: {why}")))?;
        settings.sandbox.apply(&mut config);
        let plan = save_check::plan_content(&info.content, &ContentPrint::of(&config));
        let mut notes = plan.notes.clone();
        config.generation = settings.generation;
        config.zombies = settings.zombies;
        config.start_hour = settings.start_hour;
        config.day_length_secs = settings.day_length_secs;
        config.zombie = settings.zombie;
        config.combat = settings.combat;
        config.tiers = settings.tiers;
        config.carry = settings.carry;
        config.health = settings.health;
        config.infection = settings.infection;
        config.needs = settings.needs;
        config.skills = settings.skills;
        config.power = settings.power;
        config.power.validate().map_err(|why| save_check::invalid(format!("power settings: {why}")))?;
        config.loot_seed = settings.loot_seed;
        settings.blinds.check().map_err(save_check::invalid)?;
        config.blinds = settings.blinds;
        save_check::check_settings(text, &config)?;
        let map_save: MapSave = decode(MAPD, r.expect(MAPD)?)?;
        let mut ws: WorldSave = decode(WRLD, r.expect(WRLD)?)?;
        // Weapons are named by index into the save's own list: move them to the content's.
        let mut blocking: Vec<String> = save_check::remap_weapons(&plan, &mut ws.players, force, &mut notes)?.into_iter().collect();
        // Items too, wherever they are.
        blocking.extend(save_check::remap_items(&plan, &mut ws.items, &mut ws.players, &mut ws.opened, force, &mut notes)?);
        // And recipes, in queued crafts.
        save_check::remap_recipes(&plan, &mut ws.players, &mut notes)?;

        let mut w = World::generate(config);
        let mut new_map = false;
        match map_save.layout {
            MapLayout::Edits(edits) => {
                if w.map_base_hash != map_save.base_hash {
                    match (&plan.templates, force) {
                        (_, true) => new_map = true,
                        (Some(why), false) => blocking.insert(0, why.clone()),
                        (None, false) => return Err(LoadError::MapChanged),
                    }
                } else if plan.templates.is_some() {
                    notes.push("building templates or street props changed, but the map they make is the same".to_string());
                }
                if !blocking.is_empty() {
                    return Err(LoadError::ContentChanged(blocking));
                }
                let mut dropped = 0;
                for c in edits {
                    if !w.map.in_bounds(c.tile) {
                        return Err(save_check::invalid(format!("map edit at {} is off the map", c.tile)));
                    }
                    let generated = w.map.owned_edge(c.tile, c.edge);
                    if !save_check::edit_fits(generated, c.kind) {
                        if new_map {
                            dropped += 1;
                            continue;
                        }
                        return Err(save_check::invalid(format!("map edit at {} {:?} turns a {generated:?} into a {:?}", c.tile, c.edge, c.kind)));
                    }
                    w.map.set_owned_edge(c.tile, c.edge, c.kind);
                    w.walk.update_around(&w.map, c.tile);
                    w.map_edits.insert(EdgeRef::new(c.tile, c.edge).key());
                }
                if new_map {
                    let why = plan.templates.as_deref().unwrap_or("the map generator changed, so the map differs");
                    notes.push(format!("loaded anyway: {why}; regenerated it and applied the door and window changes that still fit ({dropped} dropped)"));
                }
            }
            MapLayout::Full(map) => {
                if !blocking.is_empty() {
                    return Err(LoadError::ContentChanged(blocking));
                }
                if !map.is_consistent() {
                    return Err(corrupt("the stored map is malformed"));
                }
                if plan.templates.is_some() {
                    notes.push("building templates or street props changed; the save stores its map whole, so that map is kept".to_string());
                }
                // Street props aren't stored: the generated map's, where the stored tiles still have them.
                let mut map = map;
                let kinds = std::mem::take(&mut w.map.prop_kinds);
                map.street_props = std::mem::take(&mut w.map.street_props)
                    .into_iter()
                    .filter(|p| {
                        let tile = map.tile(p.tile);
                        let blocks = kinds.get(p.kind as usize).is_some_and(|k| k.blocks);
                        if blocks { tile.feature == crate::map::Feature::StreetProp } else { tile.room == 0 && tile.feature == crate::map::Feature::None }
                    })
                    .collect();
                map.prop_kinds = kinds;
                // Glazed doors and blinds aren't stored with the tiles: the generated map's (the
                // `BLND` section, if any, replaces the blinds below).
                map.glazed = std::mem::take(&mut w.map.glazed);
                map.blinds = std::mem::take(&mut w.map.blinds);
                map.stair_speed = w.map.stair_speed;
                map.index_storeys();
                w.map = map;
                // Containers are the stored map's furniture, not the generated one's.
                w.sites = crate::containers::find_sites(&w.map, &w.config.containers);
                w.map_custom = true;
                w.map_base_hash = map_save.base_hash;
                w.walk = WalkGrid::new(&w.map);
                w.regions = RegionIndex::new(&w.map, w.config.tiers.region_size);
                w.hash = SpatialHash::new(w.map.width() as f32, w.map.height() as f32, 1.0);
            }
        }
        // On a new map the town, the spawn and its house are the new map's.
        if !new_map {
            (w.town_min, w.town_max, w.player_spawn, w.spawn_building) = (map_save.town_min, map_save.town_max, map_save.player_spawn, map_save.spawn_building as usize);
        }
        // The navigation graph, from the map as it is now.
        w.nav = crate::nav::Nav::new(&w.walk);

        let regions = w.regions.regions();
        let pop = ws.population;
        if pop.target.len() != regions || pop.zone.len() != regions || pop.last_seen.len() != regions {
            return Err(save_check::invalid(format!("population has {} regions, the map {regions}", pop.target.len())));
        }
        w.population = Population {
            target: pop.target,
            zone: pop.zone,
            last_seen: pop.last_seen,
            attractors: pop.attractors,
            next_attractor: pop.next_attractor,
            respawned: pop.respawned,
            counts: Vec::new(),
        };
        w.tick = ws.tick;
        w.clock_offset_hours = ws.clock_offset_hours;
        w.rng = Rng::new(ws.rng);
        w.combat_rng = Rng::new(ws.combat_rng);
        (w.next_id, w.next_player_id, w.next_item_id) = (ws.next_id, ws.next_player_id, ws.next_item_id);
        w.players = ws.players;
        w.items = ws.items;
        w.opened = ws.opened.into_iter().collect();
        // Items whose number of goes changed: what was had of one can't be more than it has now.
        if plan.item_map.is_some() {
            save_check::fit_goes(&mut w);
        }
        w.edge_hp = ws.edge_hp.into_iter().collect();
        w.sounds = ws.sounds;
        w.heard = ws.heard;
        w.goal = ws.goal;
        w.goal_dirty = true;
        // Fields and plans saved for another map would lead zombies astray: on a new one they're
        // rebuilt as asked for.
        if !new_map {
            w.flow_cache.restore(ws.flow, &w.walk).map_err(LoadError::Corrupt)?;
            w.nav.restore(ws.nav).map_err(LoadError::Corrupt)?;
        }

        // Zombies: every region's block, then back into id order; then the run stats, if any.
        let mut z = Zombies::default();
        let mut runs: Option<Vec<(u32, crate::run::RunStats)>> = None;
        let mut looks: Option<crate::health::RiseLooks> = None;
        let mut knocks: Option<Vec<(u32, crate::players::Knock)>> = None;
        let mut climbs: Option<Vec<(u32, crate::players::Climb)>> = None;
        let mut far: Option<u64> = None;
        let mut script: Option<crate::script::ScriptState> = None;
        let mut power: Option<crate::power::PowerSave> = None;
        let mut away: Option<AwaySave> = None;
        let mut nights: Option<Vec<(u32, crate::firstnight::FirstNight)>> = None;
        let mut origins: Option<Vec<(u32, crate::origin::Origin)>> = None;
        let mut maps: Option<Vec<(u32, crate::explore::Explored)>> = None;
        let mut bonds: Option<BondSave> = None;
        let mut groups: Option<crate::groups::Groups> = None;
        let mut blinds: Option<BlindSave> = None;
        let mut shoved: Option<Vec<crate::combat::Shoved>> = None;
        let mut levels: Option<(Vec<(u32, u8)>, Vec<u32>)> = None;
        let mut bags: Option<Vec<(u32, Vec<u8>)>> = None;
        loop {
            let (tag, payload) = r.next()?;
            if tag == END {
                break;
            }
            if tag == RUNS && runs.is_none() {
                // As written before PvP: no count of players killed.
                let old: Vec<(u32, RunsV1)> = decode(RUNS, payload)?;
                runs = Some(old.into_iter().map(|(id, r)| (id, r.into())).collect());
                continue;
            }
            if tag == RUN2 && runs.is_none() {
                runs = Some(decode(RUN2, payload)?);
                continue;
            }
            if tag == KNOC && knocks.is_none() {
                knocks = Some(decode(KNOC, payload)?);
                continue;
            }
            if tag == CLMB && climbs.is_none() {
                climbs = Some(decode(CLMB, payload)?);
                continue;
            }
            if tag == AWAY && away.is_none() {
                away = Some(decode(AWAY, payload)?);
                continue;
            }
            if tag == LOOK && looks.is_none() {
                looks = Some(decode(LOOK, payload)?);
                continue;
            }
            if tag == FARC && far.is_none() {
                far = Some(decode(FARC, payload)?);
                continue;
            }
            if tag == NITE && nights.is_none() {
                nights = Some(decode(NITE, payload)?);
                continue;
            }
            if tag == ORIG && origins.is_none() {
                origins = Some(decode(ORIG, payload)?);
                continue;
            }
            if tag == XPLR && maps.is_none() {
                maps = Some(decode(XPLR, payload)?);
                continue;
            }
            if tag == BOND && bonds.is_none() {
                bonds = Some(decode(BOND, payload)?);
                continue;
            }
            if tag == GRPS && groups.is_none() {
                groups = Some(decode(GRPS, payload)?);
                continue;
            }
            if tag == POWR && power.is_none() {
                power = Some(decode(POWR, payload)?);
                continue;
            }
            if tag == BLND && blinds.is_none() {
                blinds = Some(decode(BLND, payload)?);
                continue;
            }
            if tag == BAGS && bags.is_none() {
                bags = Some(decode(BAGS, payload)?);
                continue;
            }
            if tag == LEVL && levels.is_none() {
                levels = Some(decode(LEVL, payload)?);
                continue;
            }
            if tag == SHOV && shoved.is_none() {
                shoved = Some(decode(SHOV, payload)?);
                continue;
            }
            if tag == MODS && script.is_none() {
                script = Some(decode(MODS, payload)?);
                continue;
            }
            if tag != ZOMB || runs.is_some() || looks.is_some() || knocks.is_some() || climbs.is_some() || far.is_some() || nights.is_some() || origins.is_some() || away.is_some() || bonds.is_some() || groups.is_some() || script.is_some() || power.is_some() || shoved.is_some() || blinds.is_some() || levels.is_some() || bags.is_some() {
                return Err(corrupt(format!("unexpected {} section", tag_name(tag))));
            }
            let block: ZombieBlock = decode(ZOMB, payload)?;
            if !block.columns_agree() {
                return Err(corrupt(format!("zombie columns of region {} have different lengths", block.region)));
            }
            save_check::zombie_region(block.region, &block.pos, block.id_gaps.first().copied().unwrap_or(0), &w.regions, &w.map)?;
            block.append_to(&mut z);
        }
        if z.len() != ws.zombies as usize {
            return Err(corrupt(format!("{} zombies stored, {} expected", z.len(), ws.zombies)));
        }
        for (id, mut run) in runs.unwrap_or_default() {
            let p = w.players.iter_mut().find(|p| p.id == id).ok_or_else(|| save_check::invalid(format!("run stats for player {id}, who isn't in the save")))?;
            run.check(p.dead).map_err(|why| save_check::invalid(format!("player {id}: {why}")))?;
            // The weapon another player killed with, in the content's list now (gone: unnamed).
            if let (Some(map), Some(crate::run::Death { cause: crate::run::DeathCause::Survivor { weapon, .. }, .. })) = (&plan.weapon_map, &mut run.death) {
                *weapon = map.get(*weapon as usize).copied().flatten().unwrap_or(u16::MAX);
            }
            p.run = run;
        }
        for (id, night) in nights.unwrap_or_default() {
            let p = w.players.iter_mut().find(|p| p.id == id).ok_or_else(|| save_check::invalid(format!("a first night for player {id}, who isn't in the save")))?;
            night.check().map_err(|why| save_check::invalid(format!("player {id}: {why}")))?;
            p.first_night = Some(night);
        }
        for (id, origin) in origins.unwrap_or_default() {
            let p = w.players.iter_mut().find(|p| p.id == id).ok_or_else(|| save_check::invalid(format!("an origin for player {id}, who isn't in the save")))?;
            origin.check().map_err(|why| save_check::invalid(format!("player {id}'s origin: {why}")))?;
            p.origin = origin;
        }
        for (id, seen) in maps.unwrap_or_default() {
            let (mw, mh, buildings) = (w.map.width(), w.map.height(), w.map.buildings.len());
            let p = w.players.iter_mut().find(|p| p.id == id).ok_or_else(|| save_check::invalid(format!("a map for player {id}, who isn't in the save")))?;
            match seen.check(mw, mh, buildings) {
                Ok(()) => p.explored = seen,
                // Loaded anyway onto another map: what was seen of the old one means nothing here.
                Err(_) if options.force => {}
                Err(why) => return Err(save_check::invalid(format!("player {id}: {why}"))),
            }
        }
        for (id, knock) in knocks.unwrap_or_default() {
            let p = w.players.iter_mut().find(|p| p.id == id).ok_or_else(|| save_check::invalid(format!("player {id} is knocked back, but isn't in the save")))?;
            if p.dead || p.knock.is_some() || !knock.timer.is_finite() || !(0.0..=60.0).contains(&knock.timer) {
                return Err(save_check::invalid(format!("player {id}: knocked back for {} s (dead: {})", knock.timer, p.dead)));
            }
            p.knock = Some(knock);
        }
        // Blinds: the save's replace the generated ones. One on an edge that takes none (a map
        // regenerated anyway) is dropped, else the save is refused.
        if let Some((list, glazed)) = blinds {
            // Glass in doors first: the save's, on what are doors on this map.
            let mut doors = std::collections::BTreeSet::new();
            for key in glazed {
                let e = crate::world::edge_of_key(key);
                let door = |k: crate::map::Edge| k.is_door() || k == crate::map::Edge::DoorBroken;
                if !w.map.in_bounds(e.tile) || key.2 > 1 || !door(w.map.owned_edge(e.tile, e.edge)) {
                    if new_map {
                        continue;
                    }
                    return Err(save_check::invalid(format!("glass in a door at {} {:?}, where there's no door", e.tile, e.edge)));
                }
                doors.insert(key);
            }
            w.map.glazed = doors;
            let mut kept = std::collections::BTreeMap::new();
            let mut dropped = 0;
            for (key, down) in list {
                let e = crate::world::edge_of_key(key);
                if !w.map.in_bounds(e.tile) || key.2 > 1 || !w.map.takes_blind(e) {
                    if new_map {
                        dropped += 1;
                        continue;
                    }
                    return Err(save_check::invalid(format!("a blind at {} {:?}, where there's no window or glass door ({:?})", e.tile, e.edge, w.map.owned_edge(e.tile, e.edge))));
                }
                kept.insert(key, down);
            }
            if dropped > 0 {
                notes.push(format!("{dropped} blinds dropped: their windows or doors aren't on the new map"));
            }
            w.map.blinds = kept;
        }
        // Which pouch each carried stack is in (a list that doesn't fit its player's stacks, after
        // a forced load dropped some, is left out: `settle` puts them away afresh).
        for (id, homes) in bags.unwrap_or_default() {
            if let Some(p) = w.players.iter_mut().find(|p| p.id == id) {
                p.inventory.set_homes(&homes);
            }
        }
        // Who and what is upstairs (`crate::stairs`). On a map that no longer has a floor there
        // (loaded anyway onto a changed one), they are on the ground floor.
        let (levels, up_items) = levels.unwrap_or_default();
        for (id, level) in levels {
            let map = &w.map;
            let p = w.players.iter_mut().find(|p| p.id == id).ok_or_else(|| save_check::invalid(format!("player {id} is upstairs, but isn't in the save")))?;
            if level != 1 || p.state.level != 0 {
                return Err(save_check::invalid(format!("player {id} on floor {level}")));
            }
            if map.upper_tile(p.state.pos.floor().as_ivec2()).is_some_and(|u| u.floor && !u.blocked) {
                p.state.level = 1;
            } else {
                notes.push(format!("player {id} was upstairs where there is no floor now: put on the ground floor"));
            }
        }
        if up_items.windows(2).any(|p| p[0] >= p[1]) {
            return Err(save_check::invalid("upstairs items out of order"));
        }
        for id in up_items {
            let it = w.items.iter().find(|it| it.id == id).ok_or_else(|| save_check::invalid(format!("ground item {id} is upstairs, but isn't in the save")))?;
            if w.map.upper_tile(it.pos.floor().as_ivec2()).is_some_and(|u| u.floor) {
                w.upstairs.insert(id);
            }
        }
        for (id, c) in climbs.unwrap_or_default() {
            let (map, give_up) = (&w.map, w.config.climb.give_up_secs);
            let p = w.players.iter_mut().find(|p| p.id == id).ok_or_else(|| save_check::invalid(format!("player {id} is climbing, but isn't in the save")))?;
            let finite = [c.at.x, c.at.y, c.dir.x, c.dir.y, c.left].iter().all(|v| v.is_finite());
            if p.dead || p.state.climb.is_some() || !finite || !map.in_bounds(c.at.floor().as_ivec2()) || (c.dir.length() - 1.0).abs() > 1e-3 || !(0.0..=give_up.max(60.0)).contains(&c.left) {
                return Err(save_check::invalid(format!("player {id}: climbing at {} toward {} for {} s (dead: {})", c.at, c.dir, c.left, p.dead)));
            }
            p.state.climb = Some(c);
        }
        // Tied hands (#73). An item that is gone from the content: the hands are free.
        for (id, item, by, struggled, struggling) in bonds.unwrap_or_default() {
            let found = w.config.items.iter().position(|d| d.id == item);
            let p = w.players.iter_mut().find(|p| p.id == id).ok_or_else(|| save_check::invalid(format!("player {id} is tied up, but isn't in the save")))?;
            if p.dead || p.restraint.is_some() || !struggled.is_finite() || !(0.0..=36_000.0).contains(&struggled) {
                return Err(save_check::invalid(format!("player {id}: tied up, struggled {struggled} s (dead: {})", p.dead)));
            }
            match found {
                Some(item) => p.restraint = Some(crate::bonds::Restraint { item: item as u16, by, struggled, struggling }),
                None => notes.push(format!("player {id} was tied with {item:?}, which is gone: its hands are free")),
            }
        }
        if let Some(g) = groups {
            let (players, map) = (&w.players, &w.map);
            g.check(|id| players.iter().any(|p| p.id == id), |pos| map.in_bounds(pos.floor().as_ivec2()), &w.config.groups).map_err(save_check::invalid)?;
            w.groups = g;
        }
        let mut order: Vec<usize> = (0..z.len()).collect();
        order.sort_by_key(|&i| z.id[i]);
        w.zombies = reorder(z, &order);
        w.rise_looks = looks.unwrap_or_default();
        let shoved = shoved.unwrap_or_default();
        if shoved.windows(2).any(|p| p[0].zombie >= p[1].zombie) || shoved.iter().any(|s| s.tick > w.tick || s.count == 0) {
            return Err(save_check::invalid("shoved zombies out of order, shoved after the save, or shoved no times"));
        }
        w.shoved = shoved;
        w.far_ticks = far.unwrap_or(0);
        if let Some(p) = power {
            match crate::power::Power::from_save(p, &w.map) {
                Ok(p) => w.power = p,
                // Loaded anyway onto a new map: its rooms aren't the old ones.
                Err(_) if new_map => notes.push("the map changed: the lights are as in a new world".into()),
                Err(why) => return Err(save_check::invalid(why)),
            }
        }
        if let Some(mut state) = script {
            state.check(w.config.items.len(), w.config.recipes.len()).map_err(|why| save_check::invalid(format!("script state: {why}")))?;
            // Whose scripts ran the world then, against whose run it now.
            let now = w.script_mods();
            for (id, hash) in &state.mods {
                match now.iter().find(|(n, _)| n == id) {
                    None => notes.push(format!("mod {id} isn't loaded: its scripts won't run (what it stored is kept)")),
                    Some((_, h)) if h != hash => notes.push(format!("mod {id}'s scripts changed: the new ones carry on from what the old ones stored")),
                    Some(_) => {}
                }
            }
            for (id, _) in now.iter().filter(|(n, _)| !state.mods.iter().any(|(s, _)| s == n)) {
                notes.push(format!("mod {id} is new to this save: its scripts start now"));
            }
            state.mods.clear();
            if w.scripts.is_none() {
                // Nothing would ever take them.
                state.pending.clear();
            }
            w.script = state;
        } else {
            for (id, _) in w.script_mods() {
                notes.push(format!("mod {id} is new to this save: its scripts start now"));
            }
        }
        save_check::check_far_clock(&w)?;
        save_check::check_looks(&w)?;
        if new_map {
            save_check::fit_to_new_map(&mut w, &mut notes);
        }
        // Opened containers must still be containers (on a new map, or with containers.ron changed).
        if let Some(what) = save_check::fit_containers(&mut w, &plan, new_map, force, &mut notes)? {
            return Err(LoadError::ContentChanged(vec![what]));
        }
        // What's in hand and what the load does to movement follow from the inventories.
        w.refresh_all_carry();
        save_check::validate(&w)?;
        // Logged-out survivors and spawn protection (#68); those out of the world are set aside.
        let away = away.unwrap_or_default();
        let limit = crate::logout::MAX_LOGOUT_SECS;
        for (what, list) in [("logged out", &away.leaving), ("protected", &away.protected)] {
            for &(id, secs) in list {
                let p = w.players.iter_mut().find(|p| p.id == id).ok_or_else(|| save_check::invalid(format!("player {id} is {what}, but isn't in the save")))?;
                if p.dead || !secs.is_finite() || !(0.0..=limit).contains(&secs) || away.parked.contains(&id) {
                    return Err(save_check::invalid(format!("player {id}: {what} for {secs} s (dead: {})", p.dead)));
                }
                if what == "protected" {
                    p.protected = secs;
                } else {
                    p.away = Some(secs);
                }
            }
        }
        for (k, &id) in away.parked.iter().enumerate() {
            let pi = w.players.iter().position(|p| p.id == id).ok_or_else(|| save_check::invalid(format!("player {id} is out of the world, but isn't in the save")))?;
            let p = &w.players[pi];
            if p.dead || away.parked[..k].contains(&id) || !p.jobs.is_empty() || p.action.is_some() || p.knock.is_some() {
                return Err(save_check::invalid(format!("player {id}: out of the world, but dead, busy or listed twice")));
            }
            let p = w.players.remove(pi);
            w.parked.push(p);
        }
        w.parked.sort_by_key(|p| p.id);

        // Derived state, as the end of a tick leaves it.
        let (states, tier) = (&w.zombies.state, &w.zombies.tier);
        w.hash.rebuild_where(&w.zombies.pos, |i| states[i] != ZombieState::Dead && tier[i] != Tier::Virtual);
        w.regions.rebuild_where(&w.zombies.pos, |i| states[i] != ZombieState::Dead && tier[i] == Tier::Virtual);
        if !w.players.is_empty() {
            w.chunk_dist.update(&w.map, w.players.iter().map(|p| p.state.pos), w.config.tiers.chunk_reach());
        }
        // No tick has run since: only the tier counts are known.
        [w.stats.active, w.stats.near, w.stats.virtual_] = w.tier_counts();
        Ok(Loaded { world: w, notes })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::items::parse_weapons;
    use crate::map::{Edge, Side};
    use crate::players::PlayerInput;
    use crate::template::BuildingTemplate;

    fn templates() -> Vec<BuildingTemplate> {
        [include_str!("../../../assets/data/buildings/bungalow.ron"), include_str!("../../../assets/data/buildings/family_two_storey.ron")]
            .iter()
            .map(|t| BuildingTemplate::from_ron(t).unwrap())
            .collect()
    }

    fn weapons() -> Vec<crate::items::WeaponDef> {
        parse_weapons(include_str!("../../../assets/data/items/weapons.ron")).unwrap()
    }

    pub(crate) fn content() -> WorldConfig {
        let items = crate::items::parse_items(include_str!("../../../assets/data/items/items.ron"), &weapons()).unwrap();
        let containers = crate::containers::parse_containers(include_str!("../../../assets/data/containers.ron")).unwrap();
        WorldConfig::new(5, 0, templates()).with_weapons(weapons()).with_items(items).with_containers(containers).with_loot(crate::loot::tests::shipped())
    }

    fn world(zombies: u32) -> World {
        let mut config = content();
        config.zombies = Some(zombies);
        World::new(config)
    }

    /// Player input for tick `t` of a scripted run: wander about, run and sneak now and then.
    fn input(t: u32) -> PlayerInput {
        let a = t as f32 * 0.031;
        PlayerInput { move_dir: Vec2::from_angle(a) * if t % 90 < 60 { 1.0 } else { 0.0 }, facing: a, run: t % 200 < 40, sneak: t % 200 > 170, aim: false }
    }

    /// Drive both worlds through the same inputs and intents for `ticks` ticks from tick `from`,
    /// checking they stay identical.
    fn step_both(a: &mut World, b: &mut World, from: u32, ticks: u32) {
        for t in from..from + ticks {
            for w in [&mut *a, &mut *b] {
                let ids: Vec<u32> = w.players.iter().map(|p| p.id).collect();
                for (k, id) in ids.into_iter().enumerate() {
                    let seq = w.player(id).unwrap().last_input_seq + 1;
                    w.set_player_input(id, seq, input(t + k as u32 * 37));
                    if t % 25 == 0 {
                        w.player_attack(id);
                    }
                    if t % 61 == 0 {
                        w.player_shove(id);
                    }
                    if t % 97 == 0 {
                        w.player_stomp(id);
                    }
                }
                if t % 150 == 0 {
                    let p = w.players[0].state.pos;
                    w.emit_sound(p, 40.0);
                }
                w.step();
            }
            assert_eq!(a.state_hash(), b.state_hash(), "diverged at tick {}", a.tick);
        }
    }

    /// Two players in a busy fight: a horde, combat on, a battered and a broken door, a corpse
    /// pile, attractors drawing the far horde, and zombies on all three tiers.
    fn busy_world() -> World {
        let mut config = content();
        config.zombies = Some(600);
        config.generation.chunks = IVec2::splat(24);
        config.population.respawn.enabled = true;
        config.population.respawn.interval_secs = 5.0;
        config.population.respawn.unseen_secs = 0.0;
        let mut w = World::new(config);
        let spawn = w.player_spawn;
        let a = w.add_player(spawn);
        let b = w.add_player(spawn + Vec2::new(1.0, 0.0));
        for p in &mut w.players {
            p.hp = 1e5;
        }
        w.spawn_item("baseball_bat", 1, spawn + Vec2::new(0.4, 0.0));
        w.player_pick_up(a);
        w.spawn_zombies(60, spawn + Vec2::new(12.0, 0.0), 8.0);
        // Break one door of the house, batter another.
        let doors: Vec<EdgeRef> = door_edges(&w);
        assert!(doors.len() >= 2, "need two doors");
        w.edge_hp.insert(doors[0].key(), 3.0);
        w.break_edge(doors[1], crate::senses::SoundSource::World);
        // Some corpses.
        for k in 0..5 {
            let i = w.add_zombie(spawn + Vec2::new(3.0 + k as f32, 6.0), 0.0, 1.0);
            (w.zombies.state[i], w.zombies.hp[i], w.zombies.timer[i]) = (ZombieState::Dead, 0.0, k as f32);
        }
        w.make_noise(spawn, 120.0, true);
        // Player b has things: a backpack on, food, a container of the spawn house searched (so it is
        // open and stored), and a move under way when the save is made.
        open_nearest_container(&mut w, b);
        assert!(w.hold_item(b, crate::inventory::Slot::Back, "school_backpack"));
        assert!(w.give_item(b, "tinned_beans", 3) && w.give_item(b, "jeans", 1));
        w.player_wear(b, 1).unwrap();
        w.player_drop(b, crate::inventory::Place::Carried, 0, 1).unwrap();
        // Player b is hurt too: a festering gash under a dirty dressing, an infected bite and food poisoning.
        w.inflict_wound(b, crate::health::Part::LeftLeg, crate::health::WoundKind::Gash, Some(false)).unwrap();
        w.inflict_wound(b, crate::health::Part::Torso, crate::health::WoundKind::Bite, Some(true)).unwrap();
        let pb = w.players.iter().position(|p| p.id == b).unwrap();
        (w.players[pb].health.wounds[0].dressing, w.players[pb].health.wounds[0].festering) = (crate::health::Dressing::Dirty, true);
        w.poison_player(b, 1.0);
        w
    }

    /// Put player `id` next to the spawn house's nearest container and search it to the end.
    /// Returns the container's id.
    pub(crate) fn open_nearest_container(w: &mut World, id: u32) -> u32 {
        let spawn = w.player_spawn;
        let site = w.containers().iter().filter(|s| s.building as usize == w.spawn_building).min_by(|a, b| a.distance(spawn).total_cmp(&b.distance(spawn))).expect("a container in the spawn house").clone();
        // Stand on a free tile of its room next to it.
        let mut best = None;
        for dy in -2..site.size.y + 2 {
            for dx in -2..site.size.x + 2 {
                let t = site.min + IVec2::new(dx, dy);
                let c = t.as_vec2() + Vec2::splat(0.5);
                if w.map.in_bounds(t) && !w.map.tile_blocked(t) && w.map.tile(t).room == site.room && site.distance(c) <= 1.0 && best.is_none_or(|(d, _)| site.distance(c) < d) {
                    best = Some((site.distance(c), c));
                }
            }
        }
        let (_, at) = best.expect("room to stand by the container");
        let pi = w.players.iter().position(|p| p.id == id).unwrap();
        // Standing still there (whatever it was doing) until the search is done.
        w.players[pi].state.pos = at;
        w.players[pi].input = crate::players::PlayerInput::default();
        w.player_search(id, Some(site.id)).unwrap();
        for _ in 0..200 {
            w.step();
            if w.player(id).unwrap().open == Some(site.id) {
                return site.id;
            }
        }
        panic!("the search never finished: {:?}", w.item_notes);
    }

    /// Shut doors of the spawn house, then of the houses nearest it, until there are at least two.
    fn door_edges(w: &World) -> Vec<EdgeRef> {
        let mut order: Vec<usize> = (0..w.map.buildings.len()).collect();
        let centre = |b: &crate::map::Building| (b.min.as_vec2() + b.size.as_vec2() * 0.5).distance(w.player_spawn);
        order.sort_by(|&a, &b| (a != w.spawn_building).cmp(&(b != w.spawn_building)).then(centre(&w.map.buildings[a]).total_cmp(&centre(&w.map.buildings[b]))));
        let mut out = Vec::new();
        for bi in order {
            let b = &w.map.buildings[bi];
            for y in b.min.y - 1..b.min.y + b.size.y + 1 {
                for x in b.min.x - 1..b.min.x + b.size.x + 1 {
                    for side in [Side::N, Side::W] {
                        let e = EdgeRef::of(IVec2::new(x, y), side);
                        if w.map.owned_edge(e.tile, e.edge) == Edge::DoorClosed && !out.contains(&e) {
                            out.push(e);
                        }
                    }
                }
            }
            if out.len() >= 2 {
                break;
            }
        }
        out
    }

    #[test]
    fn round_trip_keeps_the_state_hash() {
        let mut w = world(300);
        let id = w.add_player(w.player_spawn);
        for t in 0..100 {
            w.set_player_input(id, t + 1, input(t));
            w.step();
        }
        let bytes = w.save();
        let loaded = World::load(&bytes, &content()).unwrap();
        assert_eq!(loaded.state_hash(), w.state_hash());
        assert_eq!(loaded.tick, w.tick);
        assert_eq!(loaded.players, w.players);
        assert_eq!(loaded.time_of_day(), w.time_of_day());
        // Saving the loaded world gives the same bytes.
        assert_eq!(loaded.save(), bytes);
        let info = read_info(&bytes[..4096.min(bytes.len())]).unwrap();
        assert_eq!((info.tick, info.zombies, info.players), (100, w.zombies.len() as u32, 1));
    }

    /// A climb under way (E over a fence or through a window) is saved and carries on after a load.
    #[test]
    fn a_climb_under_way_survives_a_save() {
        let mut w = world(0);
        let id = w.add_player(w.player_spawn);
        let c = crate::players::Climb { at: w.player_spawn + Vec2::new(0.6, 0.0), dir: Vec2::X, left: 3.0 };
        w.players[0].state.climb = Some(c);
        let bytes = w.save();
        let mut loaded = World::load(&bytes, &content()).unwrap();
        assert_eq!(loaded.players[0].state.climb, Some(c));
        assert_eq!(loaded.state_hash(), w.state_hash());
        for t in 0..10 {
            for x in [&mut w, &mut loaded] {
                x.set_player_input(id, t + 1, PlayerInput::default());
                x.step();
            }
            assert_eq!(loaded.state_hash(), w.state_hash(), "tick {t}");
        }
        // Climbing nowhere: refused.
        w.players[0].state.climb = Some(crate::players::Climb { dir: Vec2::new(3.0, 0.0), ..c });
        assert!(matches!(World::load(&w.save(), &content()), Err(LoadError::Invalid(_))));
    }

    /// The real test: a saved and loaded world steps exactly like the original, through combat,
    /// broken doors, corpses, all three tiers, attractors and respawn.
    #[test]
    fn a_loaded_world_steps_like_the_original() {
        let mut a = busy_world();
        let mut b = busy_world();
        // Warm up: fights start, the far horde starts drifting, fields get cached.
        step_both(&mut a, &mut b, 0, 300);
        let [active, near, virt] = a.tier_counts();
        assert!(active > 0 && near > 0 && virt > 0, "all tiers in use: {active} {near} {virt}");
        assert!(a.zombies.state.contains(&ZombieState::Dead), "corpses");
        assert!(!a.population().attractors.is_empty(), "an attractor");
        assert!(a.flow_cache_len() > 0, "cached flow fields");
        assert!(a.nav().plan_count() > 0, "long-range plans");
        assert!(a.damaged_edges().count() > 0 || !a.map_edits.is_empty());
        let mut loaded = World::load(&a.save(), &a.config).unwrap();
        assert_eq!(loaded.state_hash(), a.state_hash());
        // b stays the unsaved reference; the loaded copy must keep up with it.
        let mut reference = b;
        let start = a.tick as u32;
        step_both(&mut loaded, &mut reference, start, 400);
        // And once more mid-fight, from the loaded world itself.
        let mut again = World::load(&loaded.save(), &a.config).unwrap();
        step_both(&mut again, &mut reference, start + 400, 200);
        let kills = again.zombies.state.iter().filter(|s| **s == ZombieState::Dead).count();
        assert!(kills > 0);
    }

    /// Stale flow fields (built before a door changed, not yet rebuilt) are stored whole, so the
    /// zombies using them after a load walk exactly the same paths.
    #[test]
    fn stale_flow_fields_survive_a_save() {
        let mut a = busy_world();
        let mut b = busy_world();
        step_both(&mut a, &mut b, 0, 120);
        // Open every shut door of the house: the fields covering them go stale.
        for e in door_edges(&a) {
            a.toggle_edge(e, crate::senses::SoundSource::World);
            b.toggle_edge(e, crate::senses::SoundSource::World);
        }
        assert!(a.flow_cache.to_saved(&a.walk).stored_fields() > 0, "some field went stale");
        let mut loaded = World::load(&a.save(), &a.config).unwrap();
        step_both(&mut loaded, &mut b, 120, 200);
    }

    /// A far horde on its way across town to a lasting noise, following long-range plans (some
    /// dropped by doors changing on the way, more being made): saved and loaded mid-trip, it keeps
    /// going exactly as the original does.
    #[test]
    fn long_range_plans_survive_a_save() {
        let make = || {
            let mut config = content();
            config.zombies = Some(500);
            config.generation.chunks = IVec2::splat(32);
            let mut w = World::new(config);
            let spawn = w.player_spawn;
            w.add_player(spawn);
            let far = Vec2::new(w.map.width() as f32, w.map.height() as f32) - spawn;
            w.make_noise(far.clamp(Vec2::splat(20.0), Vec2::splat(490.0)), 150.0, true);
            w
        };
        let (mut a, mut b) = (make(), make());
        step_both(&mut a, &mut b, 0, 1200);
        // Doors change on the way: plans through those chunks are dropped and made again.
        for e in door_edges(&a).into_iter().take(2) {
            a.toggle_edge(e, crate::senses::SoundSource::World);
            b.toggle_edge(e, crate::senses::SoundSource::World);
        }
        step_both(&mut a, &mut b, 1200, 5);
        assert!(a.nav().plan_count() > 0, "no long-range plans in use");
        let moving = a.zombies.state.iter().zip(&a.zombies.tier).filter(|(s, t)| **s == ZombieState::Investigate && **t != Tier::Active).count();
        assert!(moving > 20, "only {moving} far zombies heading somewhere");
        let mut loaded = World::load(&a.save(), &a.config).unwrap();
        assert_eq!(loaded.nav().plan_count(), a.nav().plan_count());
        assert_eq!(loaded.nav().pending(), a.nav().pending());
        assert_eq!(loaded.nav().graph(), a.nav().graph());
        step_both(&mut loaded, &mut b, 1205, 1500);
    }

    #[test]
    fn a_hand_edited_map_is_stored_whole() {
        let mut w = world(50);
        let t = IVec2::new(30, 30);
        w.map.set_edge(t, Side::N, Edge::Wall);
        w.map.tile_mut(t).unwrap().feature = crate::map::Feature::Bush;
        w.map_changed();
        let loaded = World::load(&w.save(), &content()).unwrap();
        assert_eq!(loaded.map.edge(t, Side::N), Edge::Wall);
        assert_eq!(loaded.map.tile(t).feature, crate::map::Feature::Bush);
        assert_eq!(map_hash(&loaded.map), map_hash(&w.map));
    }

    #[test]
    fn damaged_files_fail_cleanly() {
        let mut w = world(200);
        w.add_player(w.player_spawn);
        w.step();
        let bytes = w.save();
        let load = |b: &[u8]| World::load(b, &content()).map(|_| ());
        assert_eq!(load(&[]), Err(LoadError::NotASave));
        assert_eq!(load(b"hello, this is not a save at all"), Err(LoadError::NotASave));
        assert_eq!(load(&bytes[..5]), Err(LoadError::Truncated));
        // Cut anywhere: truncated, never a panic.
        for cut in [9, 12, 15, 40, bytes.len() / 3, bytes.len() / 2, bytes.len() - 9, bytes.len() - 1] {
            assert_eq!(load(&bytes[..cut]), Err(LoadError::Truncated), "cut at {cut}");
        }
        // Flip bytes all over: a checksum catches each.
        for at in (HEADER..bytes.len()).step_by(bytes.len() / 50) {
            let mut bad = bytes.clone();
            bad[at] ^= 0x5a;
            assert!(matches!(load(&bad), Err(LoadError::Corrupt(_) | LoadError::Truncated)), "flip at {at}: {:?}", load(&bad).err());
        }
        // Garbage with good checksums (a buggy writer) is reported, not a panic.
        let mut r = Reader::new(&bytes).unwrap();
        let info = r.expect(INFO).unwrap();
        let mut forged = Writer::new();
        forged.section(INFO, info);
        forged.section(CONF, b"(nonsense)");
        assert!(matches!(load(&forged.0), Err(LoadError::Corrupt(_))));
        let mut forged = Writer::new();
        forged.section(INFO, info);
        forged.section(CONF, r.expect(CONF).unwrap());
        forged.section(MAPD, &[0xff; 40]);
        forged.section(END, &[]);
        assert!(matches!(load(&forged.0), Err(LoadError::Corrupt(_))));
    }

    /// Run stats and the cause of death (#13) come back from a save; a save without them (format 4
    /// from before #13) loads with empty records; impossible ones are refused.
    #[test]
    fn run_stats_survive_a_save() {
        let mut w = world(50);
        let a = w.add_player(w.player_spawn);
        let b = w.add_player(w.player_spawn + Vec2::new(1.0, 0.0));
        for t in 0..120 {
            w.set_player_input(a, t + 1, input(t));
            w.step();
        }
        w.spawn_item("tinned_beans", 2, w.player(a).unwrap().state.pos + Vec2::new(0.4, 0.0));
        w.player_pick_up(a);
        open_nearest_container(&mut w, a);
        w.inflict_wound(b, crate::health::Part::LeftLeg, crate::health::WoundKind::Gash, Some(false)).unwrap();
        w.inflict_wound(b, crate::health::Part::RightLeg, crate::health::WoundKind::Gash, Some(false)).unwrap();
        w.skip_time(20.0);
        let (ra, rb) = (w.player(a).unwrap().run.clone(), w.player(b).unwrap().run.clone());
        assert!(ra.distance_m > 1.0 && ra.secs > 5.0 && ra.searched == 1 && ra.items_taken >= 1, "{ra:?}");
        assert!(rb.death.is_some_and(|d| matches!(d.cause, crate::run::DeathCause::BledOut { .. })), "{rb:?}");
        let bytes = w.save();
        let loaded = World::load(&bytes, &content()).unwrap();
        assert_eq!((&loaded.player(a).unwrap().run, &loaded.player(b).unwrap().run), (&ra, &rb));
        assert_eq!(loaded.state_hash(), w.state_hash());

        // The same save with every section but RUNS, as format 4 wrote it before #13.
        let rewrite = |runs: Option<([u8; 4], &[u8])>| {
            let mut r = Reader::new(&bytes).unwrap();
            let mut out = Writer::new();
            loop {
                let (tag, payload) = r.next().unwrap();
                if tag == RUN2 {
                    if let Some((tag, p)) = runs {
                        out.section(tag, p);
                    }
                    continue;
                }
                out.section(tag, payload);
                if tag == END {
                    break;
                }
            }
            out.0
        };
        let old = World::load(&rewrite(None), &content()).unwrap();
        assert_eq!(old.player(a).unwrap().run, crate::run::RunStats::default());
        assert!(old.player(b).unwrap().dead && old.player(b).unwrap().run.death.is_none());
        assert_eq!(old.state_hash(), w.state_hash());

        // A living player with a cause of death, a player who isn't there, a negative distance.
        // A `RUNS` section as written before PvP (no count of players killed) still loads.
        type Old = (f64, f64, u32, f64, u32, u32, u32, f32, Option<crate::run::Death>);
        let as_old = |r: &crate::run::RunStats| -> Old { (r.secs, r.hours, r.kills, r.distance_m, r.items_taken, r.searched, r.wounds, r.hp_lost, r.death) };
        let legacy: Vec<(u32, Old)> = vec![(a, as_old(&ra)), (b, as_old(&rb))];
        let old = World::load(&rewrite(Some((RUNS, &encode(&legacy)))), &content()).unwrap();
        assert_eq!((&old.player(a).unwrap().run, &old.player(b).unwrap().run), (&ra, &rb));

        let forged = |runs: Vec<(u32, crate::run::RunStats)>| World::load(&rewrite(Some((RUN2, &encode(&runs)))), &content()).map(|_| ());
        assert!(matches!(forged(vec![(a, rb.clone())]), Err(LoadError::Invalid(_))));
        assert!(matches!(forged(vec![(99, ra.clone())]), Err(LoadError::Invalid(_))));
        let bad = crate::run::RunStats { distance_m: -1.0, ..ra.clone() };
        assert!(matches!(forged(vec![(a, bad)]), Err(LoadError::Invalid(_))));
        let bad = crate::run::RunStats { hours: f64::NAN, ..ra };
        assert!(matches!(forged(vec![(a, bad)]), Err(LoadError::Invalid(_))));
    }

    #[test]
    fn other_versions_are_refused() {
        let w = world(10);
        let mut bytes = w.save();
        bytes[8..12].copy_from_slice(&(FORMAT_VERSION + 1).to_le_bytes());
        let err = World::load(&bytes, &content()).err().unwrap();
        assert_eq!(err, LoadError::Version { found: FORMAT_VERSION + 1, supported: FORMAT_VERSION });
        assert!(err.to_string().contains("newer"), "{err}");
    }

    #[test]
    fn changed_content_is_reported() {
        let w = world(10);
        let bytes = w.save();
        // A weapon's numbers changed: loads, with a note (more in `save_check`'s tests).
        let mut changed = content();
        changed.weapons[0].range += 0.1;
        let loaded = World::load_with(&bytes, &changed, LoadOptions::default()).unwrap();
        assert!(loaded.notes.len() == 1 && loaded.notes[0].contains("weapons"), "{:?}", loaded.notes);
        // Templates that change the map don't load (unless forced); population changes do.
        let mut other = content();
        other.templates.truncate(1);
        other.population.scale = 2.0;
        let err = World::load(&bytes, &other).err().unwrap();
        assert!(matches!(&err, LoadError::ContentChanged(what) if what.len() == 1 && what[0].contains("templates")), "{err}");
        assert!(err.forceable());
        // Settings that aren't content (seed, size, tunings) come from the save, whatever is passed.
        let mut settings = content();
        settings.generation.seed = 99;
        settings.zombie.door_hp = 1.0;
        let loaded = World::load(&bytes, &settings).unwrap();
        assert_eq!((loaded.config.seed(), loaded.config.zombie.door_hp), (5, w.config.zombie.door_hp));
        assert_eq!(loaded.state_hash(), w.state_hash());
    }

    /// The settings go through RON; every float must come back bit for bit.
    #[test]
    fn settings_round_trip_exactly() {
        let mut w = world(0);
        w.config.zombie.sight_day = 0.1 + 0.2;
        w.config.combat.zombie_hit_chance = 1.0 / 3.0;
        w.config.start_hour = 7.123_457;
        let loaded = World::load(&w.save(), &content()).unwrap();
        assert_eq!(loaded.config.zombie, w.config.zombie);
        assert_eq!(loaded.config.combat, w.config.combat);
        assert_eq!(loaded.config.tiers, w.config.tiers);
        assert_eq!(loaded.config.start_hour.to_bits(), w.config.start_hour.to_bits());
    }
}
