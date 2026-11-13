//! Hooks for mod scripts (issue #25, docs/modding.md).
//!
//! `sim` runs no scripts and knows no scripting language: it has two traits a script runtime
//! implements ([`ScriptEngine`], which makes a [`ScriptHost`] for each world; the Luau one is the
//! `zombye-mods` crate), the events a tick hands to it ([`ScriptEvent`]), the functions scripts
//! change the world through (the `script_*` ones on [`World`], which check everything they are
//! given), and the state scripts keep ([`ScriptState`]).
//!
//! **Determinism.** A world with a host runs it once at the end of every [`World::step`], whoever
//! steps the world (the game, a server, the bench, MCP), with that tick's events in the order they
//! happened. A script only reads the world through its host and only changes it through `World`'s
//! functions, takes its random numbers from the world ([`World::script_random`]) and keeps what
//! must outlast a call in the world too ([`World::script_set`]): the store, the random stream and
//! events not yet handed over are saved (the optional `MODS` section) and in the state hash. So a
//! world with scripts repeats exactly, and a loaded one carries on like the one that was saved, as
//! long as the scripts themselves keep nothing outside the store (docs/modding.md says so).
//!
//! A world without a host does none of it: no events are kept, nothing more is hashed or saved.

use std::collections::BTreeMap;
use std::fmt;

use glam::Vec2;
use serde::{Deserialize, Serialize};

use crate::rng::Rng;
use crate::run::DeathCause;
use crate::world::World;
use crate::zombies::ZombieState;

/// The longest key and string a script can store, in bytes.
pub const MAX_KEY: usize = 64;
pub const MAX_STRING: usize = 1024;
/// How deep a stored table can nest, and how big one mod's store can get ([`Value::weight`]).
pub const MAX_DEPTH: usize = 8;
pub const MAX_STORE: usize = 64 * 1024;
/// Events kept for the next tick's hooks (more are dropped), and script errors kept for whoever
/// asks ([`World::script_errors`]).
pub const MAX_EVENTS: usize = 1024;
pub const MAX_ERRORS: usize = 32;
/// Limits on what one call may ask for.
pub const MAX_NOISE: f32 = 150.0;
pub const MAX_SPAWN: u32 = 50;
pub const MAX_TELL: usize = 160;

/// Makes script hosts: the loaded mods' scripts, compiled or not, shared by every world made from
/// one [`crate::WorldConfig`] (a new run, a loaded save).
pub trait ScriptEngine: Send + Sync + fmt::Debug {
    /// A host for one world, its scripts loaded and nothing run yet.
    fn start(&self) -> Box<dyn ScriptHost>;
    /// The mods whose scripts these are: id and a hash of the script sources, in load order.
    fn mods(&self) -> Vec<(String, u64)>;
}

/// The scripts of one world.
pub trait ScriptHost: Send + Sync {
    /// The end of a tick: call the scripts' hooks for `events` (in order), then their tick hook.
    /// Errors go to [`World::script_error`]; a hook that fails must leave the host usable.
    fn run(&mut self, world: &mut World, events: &[ScriptEvent]);
}

/// What happened in a tick that scripts are told of.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum ScriptEvent {
    /// A player used one of an item (ate, drank, or used a `usable` one): the item's index.
    Used { player: u32, item: u16 },
    /// A player's swing, shove or stomp hit a zombie.
    Hit { player: u32, zombie: u32, damage: f32, killed: bool },
    /// A player finished a craft: the recipe's index.
    Crafted { player: u32, recipe: u16 },
    /// A player died.
    Died { player: u32 },
    /// A survivor began: a player joined, or a dead one started again.
    Spawned { player: u32 },
}

/// A value a script keeps in the world: plain data only.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Value {
    Bool(bool),
    Num(f64),
    Str(String),
    List(Vec<Value>),
    Map(BTreeMap<String, Value>),
}

impl Value {
    /// Roughly the bytes it takes, for the store's limit.
    pub fn weight(&self) -> usize {
        match self {
            Value::Bool(_) => 1,
            Value::Num(_) => 8,
            Value::Str(s) => 2 + s.len(),
            Value::List(l) => 2 + l.iter().map(Value::weight).sum::<usize>(),
            Value::Map(m) => 2 + m.iter().map(|(k, v)| 2 + k.len() + v.weight()).sum::<usize>(),
        }
    }

    /// Is it something a script may store: finite numbers, strings and keys within the limits,
    /// nested no deeper than [`MAX_DEPTH`]?
    pub fn check(&self, depth: usize) -> Result<(), String> {
        if depth > MAX_DEPTH {
            return Err(format!("nested deeper than {MAX_DEPTH} tables"));
        }
        match self {
            Value::Bool(_) => Ok(()),
            Value::Num(n) if n.is_finite() => Ok(()),
            Value::Num(n) => Err(format!("{n} is not a finite number")),
            Value::Str(s) if s.len() <= MAX_STRING => Ok(()),
            Value::Str(s) => Err(format!("a string of {} bytes (at most {MAX_STRING})", s.len())),
            Value::List(l) => l.iter().try_for_each(|v| v.check(depth + 1)),
            Value::Map(m) => m.iter().try_for_each(|(k, v)| if k.len() > MAX_KEY { Err(format!("key {k:?} is longer than {MAX_KEY} bytes")) } else { v.check(depth + 1) }),
        }
    }

    fn hash_into(&self, mix: &mut impl FnMut(u32)) {
        match self {
            Value::Bool(b) => mix(0xB0 | *b as u32),
            Value::Num(n) => {
                mix(0xA1);
                mix(n.to_bits() as u32);
                mix((n.to_bits() >> 32) as u32);
            }
            Value::Str(s) => {
                mix(0xA2);
                mix_str(mix, s);
            }
            Value::List(l) => {
                mix(0xA3 | (l.len() as u32) << 8);
                l.iter().for_each(|v| v.hash_into(mix));
            }
            Value::Map(m) => {
                mix(0xA4 | (m.len() as u32) << 8);
                for (k, v) in m {
                    mix_str(mix, k);
                    v.hash_into(mix);
                }
            }
        }
    }
}

fn mix_str(mix: &mut impl FnMut(u32), s: &str) {
    mix(s.len() as u32);
    for chunk in s.as_bytes().chunks(4) {
        mix(chunk.iter().fold(0u32, |a, &b| a << 8 | b as u32));
    }
}

/// What scripts keep in the world: saved, and in the state hash once there is any of it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ScriptState {
    /// Each mod's store, by mod id then key.
    pub store: BTreeMap<String, BTreeMap<String, Value>>,
    /// The scripts' random stream, once one has drawn from it.
    pub rng: Option<u64>,
    /// Events since the last tick's hooks ran (a player joining between ticks, say).
    pub pending: Vec<ScriptEvent>,
    /// The mods whose scripts ran this world when it was saved, with their scripts' hashes.
    pub mods: Vec<(String, u64)>,
}

impl ScriptState {
    /// Nothing a world without scripts wouldn't have.
    pub fn is_empty(&self) -> bool {
        self.store.is_empty() && self.rng.is_none() && self.pending.is_empty()
    }

    pub(crate) fn hash_into(&self, mix: &mut impl FnMut(u32)) {
        if self.is_empty() {
            return;
        }
        mix(0x4D4F_4453);
        for (id, store) in &self.store {
            mix_str(mix, id);
            mix(store.len() as u32);
            for (k, v) in store {
                mix_str(mix, k);
                v.hash_into(mix);
            }
        }
        if let Some(r) = self.rng {
            mix(r as u32);
            mix((r >> 32) as u32);
        }
        mix(self.pending.len() as u32);
    }

    /// A loaded save's script state: values a script could have stored, events naming things the
    /// content has.
    pub(crate) fn check(&self, items: usize, recipes: usize) -> Result<(), String> {
        for (id, store) in &self.store {
            if id.len() > MAX_KEY {
                return Err(format!("mod id {id:?} is too long"));
            }
            let mut weight = 0;
            for (k, v) in store {
                if k.len() > MAX_KEY {
                    return Err(format!("mod {id}: key {k:?} is too long"));
                }
                v.check(1).map_err(|why| format!("mod {id}: {k}: {why}"))?;
                weight += k.len() + v.weight();
            }
            if weight > MAX_STORE {
                return Err(format!("mod {id} stores {weight} bytes (at most {MAX_STORE})"));
            }
        }
        if self.pending.len() > MAX_EVENTS {
            return Err(format!("{} script events waiting (at most {MAX_EVENTS})", self.pending.len()));
        }
        for e in &self.pending {
            let ok = match *e {
                ScriptEvent::Used { item, .. } => (item as usize) < items,
                ScriptEvent::Crafted { recipe, .. } => (recipe as usize) < recipes,
                ScriptEvent::Hit { damage, .. } => damage.is_finite(),
                ScriptEvent::Died { .. } | ScriptEvent::Spawned { .. } => true,
            };
            if !ok {
                return Err(format!("a script event that can't be: {e:?}"));
            }
        }
        Ok(())
    }
}

/// What a script is told of a player ([`World::script_player`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlayerInfo {
    pub id: u32,
    pub pos: Vec2,
    pub facing: f32,
    pub hp: f32,
    pub max_hp: f32,
    pub dead: bool,
    pub asleep: bool,
    pub indoors: bool,
    pub endurance: f32,
    pub hunger: f32,
    pub thirst: f32,
    pub tiredness: f32,
}

/// A need a script can read and set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Need {
    Hunger,
    Thirst,
    Tiredness,
}

impl World {
    /// Does this world run scripts?
    pub fn has_scripts(&self) -> bool {
        self.config.scripts.is_some()
    }

    /// The mods whose scripts run this world (id, hash of the sources).
    pub fn script_mods(&self) -> Vec<(String, u64)> {
        self.config.scripts.as_ref().map(|e| e.mods()).unwrap_or_default()
    }

    /// What the scripts keep ([`ScriptState`]).
    pub fn script_state(&self) -> &ScriptState {
        &self.script
    }

    /// The latest script errors (at most [`MAX_ERRORS`]; not saved).
    pub fn script_errors(&self) -> &[String] {
        &self.script_errors
    }

    /// A script failed: kept for whoever asks, the oldest dropped.
    pub fn script_error(&mut self, text: String) {
        if self.script_errors.len() >= MAX_ERRORS {
            self.script_errors.remove(0);
        }
        self.script_errors.push(text);
    }

    /// Something scripts are told of happened. Nothing is kept in a world without scripts.
    pub(crate) fn script_event(&mut self, event: ScriptEvent) {
        // By the config, not the host: the host is out of the world while its hooks run, and what
        // they cause (a script killing a player) is an event too, for the next tick.
        if self.config.scripts.is_some() && self.script.pending.len() < MAX_EVENTS {
            self.script.pending.push(event);
        }
    }

    /// The end of a tick: the scripts' hooks for what happened, then their tick hooks.
    pub(crate) fn run_scripts(&mut self) {
        let Some(mut host) = self.scripts.take() else { return };
        let events = std::mem::take(&mut self.script.pending);
        host.run(self, &events);
        self.scripts = Some(host);
    }

    /// Uniform in `[0, 1)` from the scripts' own stream (saved; apart from the AI's and combat's,
    /// so a mod never shifts the base game's rolls).
    pub fn script_random(&mut self) -> f32 {
        let mut rng = Rng::new(self.script.rng.unwrap_or(self.config.seed() ^ 0x5C21_97D5_0000_4D4F));
        let x = rng.f32();
        self.script.rng = Some(rng.state());
        x
    }

    /// What mod `id` stored under `key`.
    pub fn script_get(&self, id: &str, key: &str) -> Option<&Value> {
        self.script.store.get(id)?.get(key)
    }

    /// Mod `id` stores `value` under `key` (`None` forgets it). Refused, with the reason, if the
    /// key or value is something the store doesn't take or the mod's store would grow past
    /// [`MAX_STORE`].
    pub fn script_set(&mut self, id: &str, key: &str, value: Option<Value>) -> Result<(), String> {
        if key.is_empty() || key.len() > MAX_KEY {
            return Err(format!("a key is 1 to {MAX_KEY} bytes, got {}", key.len()));
        }
        let Some(value) = value else {
            if let Some(store) = self.script.store.get_mut(id) {
                store.remove(key);
                if store.is_empty() {
                    self.script.store.remove(id);
                }
            }
            return Ok(());
        };
        value.check(1)?;
        let others: usize = self.script.store.get(id).map_or(0, |s| s.iter().filter(|(k, _)| k.as_str() != key).map(|(k, v)| k.len() + v.weight()).sum());
        let weight = others + key.len() + value.weight();
        if weight > MAX_STORE {
            return Err(format!("the store would hold {weight} bytes (at most {MAX_STORE})"));
        }
        self.script.store.entry(id.to_string()).or_default().insert(key.to_string(), value);
        Ok(())
    }

    /// What a script is told of player `id`.
    pub fn script_player(&self, id: u32) -> Option<PlayerInfo> {
        let p = self.player(id)?;
        Some(PlayerInfo {
            id,
            pos: p.state.pos,
            facing: p.state.facing,
            hp: p.hp,
            max_hp: self.config.combat.player_hp,
            dead: p.dead,
            asleep: p.needs.sleep.is_some(),
            indoors: self.map.room_at(p.state.pos) != 0,
            endurance: p.state.endurance,
            hunger: p.needs.hunger,
            thirst: p.needs.thirst,
            tiredness: p.needs.tiredness,
        })
    }

    /// Living zombies within `radius` of `pos`.
    pub fn script_zombies_near(&self, pos: Vec2, radius: f32) -> u32 {
        if !pos.is_finite() || !radius.is_finite() || radius <= 0.0 {
            return 0;
        }
        let z = &self.zombies;
        let r2 = radius * radius;
        (0..z.len()).filter(|&i| z.state[i] != ZombieState::Dead && z.pos[i].distance_squared(pos) <= r2).count() as u32
    }

    /// A noise at `pos` zombies hear from `radius` metres (at most [`MAX_NOISE`]; less through
    /// walls), on the next tick; `lasting` leaves an attractor as an alarm would. Says whether it
    /// was made.
    pub fn script_noise(&mut self, pos: Vec2, radius: f32, lasting: bool) -> bool {
        if !pos.is_finite() || !radius.is_finite() || radius <= 0.0 || !self.map.in_bounds(pos.floor().as_ivec2()) {
            return false;
        }
        self.make_noise(pos, radius.min(MAX_NOISE), lasting);
        true
    }

    /// Up to [`MAX_SPAWN`] zombies within `radius` of `pos` (placed from the world's own stream, as
    /// the debug spawn is). Returns how many were placed.
    pub fn script_spawn_zombies(&mut self, pos: Vec2, count: u32, radius: f32) -> u32 {
        if !pos.is_finite() || !radius.is_finite() || !self.map.in_bounds(pos.floor().as_ivec2()) {
            return 0;
        }
        self.spawn_zombies(count.min(MAX_SPAWN), pos, radius.clamp(1.0, 50.0))
    }

    /// How many of item `item` player `id` carries in its pockets and bags.
    pub fn script_count(&self, id: u32, item: &str) -> u32 {
        let (Some(p), Some(i)) = (self.player(id), self.item_index(item)) else { return 0 };
        p.inventory.carried.iter().filter(|s| s.item == i).map(|s| s.count as u32).sum()
    }

    /// Take up to `count` of item `item` out of player `id`'s pockets and bags. Returns how many.
    pub fn script_take(&mut self, id: u32, item: &str, count: u32) -> u32 {
        let (Some(pi), Some(i)) = (self.players.iter().position(|p| p.id == id), self.item_index(item)) else { return 0 };
        let mut left = count;
        let carried = &mut self.players[pi].inventory.carried;
        for s in carried.iter_mut().rev().filter(|s| s.item == i) {
            let n = left.min(s.count as u32);
            s.count -= n as u16;
            left -= n;
        }
        carried.retain(|s| s.count > 0);
        if left != count {
            self.refresh_carry(pi);
            self.private_dirty.insert(id);
        }
        count - left
    }

    /// Put `count` of item `item` into player `id`'s pockets (as a start kit is given).
    pub fn script_give(&mut self, id: u32, item: &str, count: u32) -> bool {
        let given = count > 0 && self.give_item(id, item, count.min(100) as u16);
        if given {
            self.private_dirty.insert(id);
        }
        given
    }

    /// Take the ground item `id` away. Says whether it was there.
    pub fn script_remove_ground(&mut self, id: u32) -> bool {
        let before = self.items.len();
        self.items.retain(|it| it.id != id);
        self.items.len() != before
    }

    /// Give player `id` `hp` hit points back (never above full; nothing for the dead).
    pub fn script_heal(&mut self, id: u32, hp: f32) -> bool {
        let max = self.config.combat.player_hp;
        match self.players.iter_mut().find(|p| p.id == id && !p.dead) {
            Some(p) if hp.is_finite() && hp > 0.0 => {
                p.hp = (p.hp + hp).min(max.max(p.hp));
                true
            }
            _ => false,
        }
    }

    /// Take `hp` hit points off player `id` (god mode holds; it can kill). Says whether it died.
    pub fn script_hurt(&mut self, id: u32, hp: f32) -> bool {
        let Some(pi) = self.players.iter().position(|p| p.id == id && !p.dead) else { return false };
        hp.is_finite() && hp > 0.0 && self.hurt_player_by(pi, hp, DeathCause::Other)
    }

    /// Set one of player `id`'s needs (0 fine to 1 worst).
    pub fn script_set_need(&mut self, id: u32, need: Need, value: f32) -> bool {
        let Some(pi) = self.players.iter().position(|p| p.id == id && !p.dead) else { return false };
        if !value.is_finite() {
            return false;
        }
        let n = &mut self.players[pi].needs;
        let v = value.clamp(0.0, 1.0);
        match need {
            Need::Hunger => n.hunger = v,
            Need::Thirst => n.thirst = v,
            Need::Tiredness => n.tiredness = v,
        }
        // A level may have changed, and with it how the player moves: the snapshot carries it.
        self.refresh_carry(pi);
        self.private_dirty.insert(id);
        true
    }

    /// Set player `id`'s endurance (0 to 1).
    pub fn script_set_endurance(&mut self, id: u32, value: f32) -> bool {
        match self.players.iter_mut().find(|p| p.id == id && !p.dead) {
            Some(p) if value.is_finite() => {
                p.state.endurance = value.clamp(0.0, 1.0);
                true
            }
            _ => false,
        }
    }

    /// A line for player `id`'s screen (the yellow line and the inventory panel's foot), at most
    /// [`MAX_TELL`] characters of one line.
    pub fn script_tell(&mut self, id: u32, text: &str) -> bool {
        if self.player(id).is_none() {
            return false;
        }
        let line: String = text.chars().map(|c| if c.is_control() { ' ' } else { c }).take(MAX_TELL).collect();
        let line = line.trim();
        if line.is_empty() {
            return false;
        }
        self.note(id, true, line.to_string());
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inventory::Place;
    use crate::items::parse_items;
    use crate::save::LoadError;
    use crate::template::BuildingTemplate;
    use crate::world::WorldConfig;
    use std::sync::{Arc, Mutex};

    type Seen = Arc<Mutex<Vec<(u64, ScriptEvent)>>>;

    /// A host in Rust: writes down what it is handed, and counts its ticks in the store.
    #[derive(Debug, Default)]
    struct Recorder(Seen);

    struct RecorderHost(Seen);

    impl ScriptEngine for Recorder {
        fn start(&self) -> Box<dyn ScriptHost> {
            Box::new(RecorderHost(self.0.clone()))
        }
        fn mods(&self) -> Vec<(String, u64)> {
            vec![("recorder".into(), 7)]
        }
    }

    impl ScriptHost for RecorderHost {
        fn run(&mut self, world: &mut World, events: &[ScriptEvent]) {
            self.0.lock().unwrap().extend(events.iter().map(|e| (world.tick, *e)));
            let n = match world.script_get("recorder", "ticks") {
                Some(Value::Num(n)) => *n,
                _ => 0.0,
            };
            world.script_set("recorder", "ticks", Some(Value::Num(n + 1.0))).unwrap();
        }
    }

    const ITEMS: &str = r#"[
        (id: "apple", name: "Apple", category: Food, weight_kg: 0.2, volume_l: 0.2, food: Some((hunger: 0.1, thirst: 0.0, uses: 1, spoils_hours: None))),
        (id: "bell", name: "Bell", category: Junk, weight_kg: 0.2, volume_l: 0.2, tags: ["usable"]),
        (id: "horn", name: "Horn", category: Junk, weight_kg: 0.2, volume_l: 0.2, tags: ["reusable"]),
        (id: "rock", name: "Rock", category: Junk, weight_kg: 0.2, volume_l: 0.2),
    ]"#;

    fn config(scripts: bool) -> (WorldConfig, Seen) {
        let templates: Vec<BuildingTemplate> = vec![ron::from_str(include_str!("../../../assets/data/buildings/bungalow.ron")).unwrap()];
        let mut config = WorldConfig::new(5, 0, templates).with_items(parse_items(ITEMS, &[]).unwrap()).with_map_chunks(8);
        let seen = Seen::default();
        if scripts {
            config.scripts = Some(Arc::new(Recorder(seen.clone())));
        }
        (config, seen)
    }

    #[test]
    fn the_host_runs_once_a_tick_with_the_ticks_events_in_order() {
        let (config, seen) = config(true);
        let mut w = World::new(config);
        let id = w.add_player(w.player_spawn);
        for item in ["apple", "bell", "horn", "rock"] {
            assert!(w.give_item(id, item, 1));
        }
        // A rock has no use; the rest do: food, a thing used up, a thing kept.
        assert!(w.player_use(id, Place::Carried, 3).is_err());
        for _ in 0..3 {
            w.player_use(id, Place::Carried, 0).unwrap();
            for _ in 0..80 {
                w.step();
            }
        }
        let item = |name: &str| w.item_index(name).unwrap();
        let events: Vec<ScriptEvent> = seen.lock().unwrap().iter().map(|(_, e)| *e).collect();
        assert_eq!(
            events,
            [ScriptEvent::Spawned { player: id }, ScriptEvent::Used { player: id, item: item("apple") }, ScriptEvent::Used { player: id, item: item("bell") }, ScriptEvent::Used { player: id, item: item("horn") }]
        );
        // Joining happened before the first tick: told at its end.
        assert_eq!(seen.lock().unwrap()[0].0, 0);
        assert_eq!(w.script_get("recorder", "ticks"), Some(&Value::Num(240.0)));
        assert_eq!((w.script_count(id, "apple"), w.script_count(id, "bell"), w.script_count(id, "horn"), w.script_count(id, "rock")), (0, 0, 1, 1));
        assert!(w.script.pending.is_empty());
        // Dying is an event too, whatever killed.
        assert!(w.script_hurt(id, 1000.0));
        w.step();
        assert_eq!(seen.lock().unwrap().last().map(|(_, e)| *e), Some(ScriptEvent::Died { player: id }));
    }

    #[test]
    fn a_world_without_scripts_keeps_nothing_of_them() {
        let (config, _) = config(false);
        let mut w = World::new(config);
        let id = w.add_player(w.player_spawn);
        assert!(w.give_item(id, "apple", 1));
        w.player_use(id, Place::Carried, 0).unwrap();
        for _ in 0..80 {
            w.step();
        }
        assert!(!w.has_scripts() && w.script.is_empty() && w.script_mods().is_empty());
        // No section in its save; and its hash takes the script state in only once there is any.
        assert!(!w.save().windows(4).any(|x| x == b"MODS"));
        let hash = w.state_hash();
        w.script_set("m", "k", Some(Value::Bool(true))).unwrap();
        assert_ne!(w.state_hash(), hash);
        w.script_set("m", "k", None).unwrap();
        assert_eq!(w.state_hash(), hash);
    }

    #[test]
    fn script_state_is_saved_checked_and_hashed() {
        let (config, _) = config(true);
        let mut w = World::new(config.clone());
        w.add_player(w.player_spawn);
        for _ in 0..5 {
            w.step();
        }
        let list = Value::List(vec![Value::Num(1.5), Value::Str("two".into()), Value::Map([("k".to_string(), Value::Bool(true))].into())]);
        w.script_set("recorder", "list", Some(list.clone())).unwrap();
        let (a, b) = (w.script_random(), w.script_random());
        assert!(a != b && (0.0..1.0).contains(&a));
        // An event waiting for the next tick (a player joined between ticks) is saved too.
        let other = w.add_player(w.player_spawn);
        assert_eq!(w.script.pending, [ScriptEvent::Spawned { player: other }]);
        let bytes = w.save();
        let loaded = World::load_with(&bytes, &config, Default::default()).unwrap();
        assert!(loaded.notes.is_empty(), "{:?}", loaded.notes);
        let mut copy = loaded.world;
        assert_eq!(copy.script_get("recorder", "list"), Some(&list));
        assert_eq!((copy.script.clone(), copy.state_hash()), (w.script.clone(), w.state_hash()));
        assert_eq!(copy.script_random(), w.script_random());
        for _ in 0..40 {
            w.step();
            copy.step();
            assert_eq!(copy.state_hash(), w.state_hash());
        }

        // Loaded by a game without the mod: a note, the store kept, nothing left waiting.
        let (plain, _) = self::config(false);
        let loaded = World::load_with(&bytes, &plain, Default::default()).unwrap();
        assert!(loaded.notes.iter().any(|n| n.contains("mod recorder isn't loaded")), "{:?}", loaded.notes);
        assert!(loaded.world.script.pending.is_empty() && loaded.world.script_get("recorder", "list").is_some());
        // A save from before the mod, loaded with it: a note.
        let old = World::new(plain).save();
        let loaded = World::load_with(&old, &config, Default::default()).unwrap();
        assert!(loaded.notes.iter().any(|n| n.contains("mod recorder is new to this save")), "{:?}", loaded.notes);

        // Values no script could have stored are refused, by name.
        let bad: [(&str, fn(&mut World)); 5] = [
            ("not a finite number", |w| drop(w.script.store.get_mut("recorder").unwrap().insert("x".into(), Value::Num(f64::NAN)))),
            ("is too long", |w| drop(w.script.store.get_mut("recorder").unwrap().insert("k".repeat(MAX_KEY + 1), Value::Bool(true)))),
            ("stores", |w| drop(w.script.store.get_mut("recorder").unwrap().insert("big".into(), Value::List(vec![Value::Str("x".repeat(MAX_STRING)); 80])))),
            ("nested deeper", |w| {
                let mut v = Value::Bool(true);
                for _ in 0..MAX_DEPTH + 2 {
                    v = Value::List(vec![v]);
                }
                w.script.store.get_mut("recorder").unwrap().insert("deep".into(), v);
            }),
            ("can't be", |w| w.script.pending.push(ScriptEvent::Used { player: 1, item: 999 })),
        ];
        for (what, spoil) in bad {
            let mut w = World::load(&bytes, &config).unwrap();
            spoil(&mut w);
            match World::load(&w.save(), &config) {
                Err(LoadError::Invalid(why)) => assert!(why.contains("script state") && why.contains(what), "{what}: {why}"),
                other => panic!("{what}: {:?}", other.map(|_| ())),
            }
        }
    }

    #[test]
    fn the_store_refuses_what_it_cannot_keep() {
        let (config, _) = config(false);
        let mut w = World::new(config);
        assert!(w.script_set("m", "", Some(Value::Bool(true))).is_err());
        assert!(w.script_set("m", &"k".repeat(MAX_KEY + 1), Some(Value::Bool(true))).is_err());
        assert!(w.script_set("m", "k", Some(Value::Num(f64::INFINITY))).is_err());
        assert!(w.script_set("m", "k", Some(Value::Str("x".repeat(MAX_STRING + 1)))).is_err());
        // Up to the limit and no further; replacing a key counts the new value, not both.
        let chunk = Value::Str("x".repeat(1000));
        let mut stored = 0;
        while w.script_set("m", &format!("k{stored}"), Some(chunk.clone())).is_ok() {
            stored += 1;
            assert!(stored < 100);
        }
        assert!((60..=65).contains(&stored), "{stored}");
        assert!(w.script_set("m", "k0", Some(chunk)).is_ok());
        assert!(w.script_set("other", "k", Some(Value::Bool(true))).is_ok(), "each mod has its own allowance");
    }
}
