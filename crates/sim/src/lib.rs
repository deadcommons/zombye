//! Zombye game simulation.
//!
//! Pure Rust, no rendering, no engine, no I/O. Everything advances in fixed ticks of [`TICK_DT`]
//! seconds and all randomness comes from a seeded [`rng::Rng`], so the same seed and inputs always
//! give the same result. Both the headless server and the client's local server run this crate.
//!
//! Units: 1 tile = 1 metre. World position `(x, y)` is on the ground plane; `+y` is "south" on the
//! map and maps to the renderer's `+z`.

pub mod ai;
pub mod combat;
pub mod containers;
pub mod crafting;
pub mod firstaid;
pub mod flow;
pub mod health;
pub mod inventory;
pub mod items;
pub mod loot;
pub mod map;
pub mod nav;
pub mod needs;
pub mod players;
pub mod population;
pub mod rng;
pub mod run;
pub mod save;
pub mod save_check;
pub mod senses;
pub mod spatial;
pub mod template;
pub mod tiers;
pub mod vision;
pub mod world;
pub mod worldgen;
pub mod zombies;

pub use ai::{ZombieAttack, ZombieThump, ZombieTuning};
pub use combat::{ActionEvent, CombatEvent, CombatEventKind, CombatTuning, HitEffect, HitEvent};
pub use containers::{ContainerData, ContainerDef, ContainerSite};
pub use crafting::{RecipeDef, RecipeItem, Tool};
pub use firstaid::{Aid, AidView, CourseView, FirstAidTuning, Treatment};
pub use health::{Dressing, Health, HealthTags, HealthTuning, HealthView, InfectionMode, InfectionStage, InfectionTuning, Part, Wound, WoundKind, WoundView};
pub use inventory::{Burden, Carry, CarryTuning, Inventory, ItemJob, ItemNote, JobKind, Place, Slot};
pub use items::{GroundItem, ItemCategory, ItemDef, ItemStack, WeaponCategory, WeaponDef};
pub use loot::{LootData, LootTable};
pub use flow::FlowField;
pub use map::TileMap;
pub use needs::{Freshness, Needs, NeedsTuning, NeedsView, SleepSpot, StatusInput, StatusTag, TagKind, WakeReason};
pub use players::{Action, ActionKind, DevFlags, MoveMods, Player, PlayerInput, PlayerState, Stance};
pub use population::{Attractor, PopulationDef, Zone};
pub use template::BuildingTemplate;
pub use run::{Death, DeathCause, RunStats};
pub use save::{LoadError, LoadOptions, Loaded, SaveInfo};
pub use world::{World, WorldConfig};
pub use senses::{Sound, SoundSource};
pub use tiers::TierTuning;
pub use zombies::{Tier, ZombieAnim, ZombieState, Zombies};

/// Simulation ticks per second.
pub const TICK_RATE: u32 = 20;
/// Seconds per simulation tick.
pub const TICK_DT: f32 = 1.0 / TICK_RATE as f32;
