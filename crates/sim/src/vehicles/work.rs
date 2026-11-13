//! Stage 4 of driving: keys and locks, hotwiring, the boot and the glovebox, and repairs, with the
//! Mechanics skill ([`crate::skills::Skill::Mechanics`]) behind the last two.
//!
//! **Keys and locks** ([`Access`]): a car is found locked or not, with its key in the ignition, in
//! its glovebox or nowhere (seeded by the world seed and where it was parked, `vehicles.ron`'s
//! `work`). A key is an item (`work.key_item`, `car_key`) whose stack's `born` is the id of the car
//! it opens (a key keeps no food fresh, so the field is free; keys to two cars never stack). The
//! engine starts with the key in the ignition, with the key on the driver, or once the wires are
//! crossed ([`VehicleOp::Hotwire`]: a job at the wheel with a tool tagged `work.hotwire_tag`, that
//! works by a chance Mechanics raises). A locked car is opened with its key, from inside, or by
//! smashing a window ([`VehicleOp::BreakIn`]: loud, and it costs the windows). Whether it starts is
//! sent with the car ([`VehicleView::start`]), so a client predicts it exactly.
//!
//! **The boot and the glovebox** are containers (kinds `work.boot` and `work.glovebox` in
//! `containers.ron`, loot from `loot.ron`'s tables for them): container ids with
//! [`VEHICLE_BIT`] set ([`vehicle_container`]). The boot is reached from behind the car, the
//! glovebox from the driver's seat or the open door; neither from outside while it's locked.
//! Things can be put in both, and go where the car goes.
//!
//! **Repairs** are data (`work.repairs`, [`RepairDef`]): a part, what it uses up, the tools it
//! needs, how much it mends and up to where, the seconds and the Mechanics level it takes. A repair
//! is a job ([`VehicleOp::Repair`]), checked again when it's due.
//!
//! **Saves:** an optional `VACC` section (each car's [`Access`]), after `VCND`; without it cars are
//! as found. Mechanics XP lives in the `MECH` section (`crate::save`).

use glam::{IVec2, Vec2};
use serde::{Deserialize, Serialize};

use super::{Vehicle, VehicleKind, VehicleView, enterable, seat_of};
use crate::containers::ContainerSite;
use crate::crafting::{RecipeItem, Tool};
use crate::inventory::JobKind;
use crate::items::ItemStack;
use crate::skills::Skill;
use crate::world::World;

/// Set in a container id that is a vehicle's boot or glovebox (bit 31 is a body's).
pub const VEHICLE_BIT: u32 = 0x4000_0000;
/// The two containers a vehicle has, as the lowest bit of their id.
pub const BOOT: u8 = 0;
pub const GLOVEBOX: u8 = 1;

/// The container id of vehicle `vid`'s boot ([`BOOT`]) or glovebox ([`GLOVEBOX`]).
pub fn vehicle_container(vid: u32, slot: u8) -> u32 {
    VEHICLE_BIT | (vid << 1) | (slot as u32 & 1)
}

/// The vehicle and which of its containers container `c` is, if it is one.
pub fn vehicle_of_container(c: u32) -> Option<(u32, u8)> {
    (c & (VEHICLE_BIT | crate::corpses::CORPSE_BIT) == VEHICLE_BIT).then(|| ((c & !VEHICLE_BIT) >> 1, (c & 1) as u8))
}

/// A car's lock and key: saved (`VACC`), in the state hash.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Access {
    pub locked: bool,
    /// Its key is in the ignition.
    pub ignition: bool,
    /// Its wires are crossed: it starts without a key.
    pub hotwired: bool,
    /// Its key lies in the glovebox, not yet found (it becomes an item there when the glovebox is
    /// first searched).
    pub glovebox_key: bool,
}

impl Access {
    /// A car as it is found: locked, and where its key is, by the seed and where it was parked.
    pub fn found(seed: u64, from: IVec2, w: &WorkTuning) -> Self {
        let roll = |k: u64| crate::rng::hash01(seed ^ 0x4b45_5953, from.x as u32 as u64, from.y as u32 as u64, k);
        let key = roll(1);
        let ignition = key < w.ignition;
        let glovebox_key = !ignition && key < w.ignition + w.glovebox_key;
        // A car with its key in the ignition was left in a hurry: never locked.
        Self { locked: !ignition && roll(2) < w.locked, ignition, hotwired: false, glovebox_key }
    }

    /// It starts without a key on the driver.
    pub fn starts(&self) -> bool {
        self.ignition || self.hotwired
    }
}

/// One of a car's four parts ([`super::PARTS`], in that order).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Part {
    Engine,
    Tyres,
    Windows,
    Body,
}

impl Part {
    pub const ALL: [Part; 4] = [Part::Engine, Part::Tyres, Part::Windows, Part::Body];

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn from_index(i: u8) -> Option<Self> {
        Self::ALL.get(i as usize).copied()
    }

    /// What it is called: "engine", "tyres", ...
    pub fn label(self) -> &'static str {
        super::PARTS[self.index()]
    }
}

/// One way to mend a part (`vehicles.ron`'s `work.repairs`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepairDef {
    pub part: Part,
    /// What it's called ("Change a tyre").
    pub name: String,
    /// Used up, and needed to hand but kept (as a recipe's).
    #[serde(default)]
    pub inputs: Vec<RecipeItem>,
    #[serde(default)]
    pub tools: Vec<Tool>,
    /// How much of the part it mends (0..1), at Mechanics 0, and how far it can bring it (a taped
    /// window is never as good as new).
    pub mends: f32,
    #[serde(default = "whole")]
    pub up_to: f32,
    /// Seconds it takes at Mechanics 0.
    pub secs: f32,
    /// The Mechanics level it takes.
    #[serde(default)]
    pub level: u32,
}

fn whole() -> f32 {
    1.0
}

/// Keys, hotwiring, the boot and glovebox and repairs (`vehicles.ron`'s `work`; ours).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WorkTuning {
    /// The item a car key is.
    pub key_item: String,
    /// How a car is found: locked, its key in the ignition, its key in the glovebox (chances; the
    /// rest have no key to find).
    pub locked: f32,
    pub ignition: f32,
    pub glovebox_key: f32,
    /// What crosses a car's wires (a tag), how long a try takes (s, at Mechanics 0), the chance a
    /// try works (Mechanics adds `SkillTuning::mechanics_hotwire` a level) and its noise (m).
    pub hotwire_tag: String,
    pub hotwire_secs: f32,
    pub hotwire_chance: f32,
    pub hotwire_noise: f32,
    /// Smashing a window to get into a locked car: what it takes off the windows, and its noise.
    pub break_in_windows: f32,
    pub break_in_noise: f32,
    /// Locking or unlocking a car: its noise (the click and the lights).
    pub lock_noise: f32,
    /// The container kinds (`containers.ron`) a car's boot and glovebox are.
    pub boot: String,
    pub glovebox: String,
    /// How far from a car's box a survivor can work on it (repairs, the lock).
    pub reach: f32,
    pub repairs: Vec<RepairDef>,
}

impl Default for WorkTuning {
    fn default() -> Self {
        let tag = |t: &str| Tool::Tag(t.into());
        let item = |i: &str, n: u16| RecipeItem { item: i.into(), count: n };
        Self {
            key_item: "car_key".into(),
            locked: 0.6,
            ignition: 0.1,
            glovebox_key: 0.2,
            hotwire_tag: "hotwires".into(),
            hotwire_secs: 10.0,
            hotwire_chance: 0.25,
            hotwire_noise: 4.0,
            break_in_windows: 0.3,
            break_in_noise: 35.0,
            lock_noise: 6.0,
            boot: "boot".into(),
            glovebox: "glovebox".into(),
            reach: 1.4,
            repairs: vec![
                RepairDef { part: Part::Engine, name: "Fix the engine".into(), inputs: vec![item("engine_parts", 1)], tools: vec![tag("wrenches")], mends: 0.4, up_to: 1.0, secs: 30.0, level: 1 },
                RepairDef { part: Part::Tyres, name: "Change a tyre".into(), inputs: vec![item("spare_tyre", 1)], tools: vec![tag("wrenches")], mends: 0.5, up_to: 1.0, secs: 20.0, level: 0 },
                RepairDef { part: Part::Windows, name: "Tape plastic over a window".into(), inputs: vec![item("plastic_bag", 1), item("duct_tape", 1)], tools: vec![], mends: 0.25, up_to: 0.6, secs: 8.0, level: 0 },
                RepairDef { part: Part::Body, name: "Beat out the dents".into(), inputs: vec![], tools: vec![tag("hammers")], mends: 0.15, up_to: 0.7, secs: 15.0, level: 0 },
                RepairDef { part: Part::Body, name: "Patch the body".into(), inputs: vec![item("duct_tape", 1), item("wooden_plank", 1)], tools: vec![tag("hammers")], mends: 0.3, up_to: 0.9, secs: 25.0, level: 2 },
            ],
        }
    }
}

impl WorkTuning {
    /// Every number in range (errors name the field or the repair).
    pub fn check(&self) -> Result<(), String> {
        for (what, v, lo, hi) in [
            ("locked", self.locked, 0.0, 1.0),
            ("ignition", self.ignition, 0.0, 1.0),
            ("glovebox_key", self.glovebox_key, 0.0, 1.0 - self.ignition),
            ("hotwire_secs", self.hotwire_secs, 0.5, 600.0),
            ("hotwire_chance", self.hotwire_chance, 0.0, 1.0),
            ("hotwire_noise", self.hotwire_noise, 0.0, 300.0),
            ("break_in_windows", self.break_in_windows, 0.0, 1.0),
            ("break_in_noise", self.break_in_noise, 0.0, 300.0),
            ("lock_noise", self.lock_noise, 0.0, 300.0),
            ("reach", self.reach, 0.1, 5.0),
        ] {
            if !v.is_finite() || !(lo..=hi).contains(&v) {
                return Err(format!("work: {what} {v} must be between {lo} and {hi}"));
            }
        }
        if self.repairs.len() > 64 {
            return Err(format!("work: at most 64 repairs, got {}", self.repairs.len()));
        }
        for r in &self.repairs {
            let bad = |why: String| Err(format!("work: repair {:?}: {why}", r.name));
            if r.name.trim().is_empty() {
                return Err("work: a repair needs a name".into());
            }
            if !(r.mends.is_finite() && r.mends > 0.0 && r.mends <= 1.0) {
                return bad(format!("mends {} must be above 0 and at most 1", r.mends));
            }
            if !(r.up_to.is_finite() && r.up_to > 0.0 && r.up_to <= 1.0) {
                return bad(format!("up_to {} must be above 0 and at most 1", r.up_to));
            }
            if !(r.secs.is_finite() && (0.5..=600.0).contains(&r.secs)) {
                return bad(format!("secs {} must be between 0.5 and 600", r.secs));
            }
            if r.level > crate::skills::MAX_LEVEL {
                return bad(format!("level {} is over the top level {}", r.level, crate::skills::MAX_LEVEL));
            }
            if r.inputs.iter().any(|x| x.count == 0 || x.count > 100) {
                return bad("counts are 1 to 100".into());
            }
        }
        Ok(())
    }

    /// Every item and tool it names is in `items` (errors name what's missing).
    pub fn check_items(&self, items: &[crate::items::ItemDef]) -> Result<(), String> {
        let has = |id: &str| items.iter().any(|d| d.id == id);
        if !has(&self.key_item) {
            return Err(format!("work: no key item {:?} in items.ron", self.key_item));
        }
        if !items.iter().any(|d| d.has_tag(&self.hotwire_tag)) {
            return Err(format!("work: no item tagged {:?} to hotwire with", self.hotwire_tag));
        }
        for r in &self.repairs {
            for x in &r.inputs {
                if !has(&x.item) {
                    return Err(format!("work: repair {:?}: no item {:?} in items.ron", r.name, x.item));
                }
            }
            for t in &r.tools {
                let ok = match t {
                    Tool::Item(id) => has(id),
                    Tool::Tag(tag) => items.iter().any(|d| d.has_tag(tag)),
                };
                if !ok {
                    return Err(format!("work: repair {:?}: no item for tool {t:?}", r.name));
                }
            }
        }
        Ok(())
    }

    /// The repairs of part `part`, with their index in [`Self::repairs`].
    pub fn repairs_of(&self, part: Part) -> impl Iterator<Item = (usize, &RepairDef)> {
        self.repairs.iter().enumerate().filter(move |(_, r)| r.part == part)
    }
}

/// The chance a try at hotwiring works at Mechanics `level` (at most 0.95).
pub fn hotwire_chance(w: &WorkTuning, s: &crate::skills::SkillTuning, level: u32) -> f32 {
    (w.hotwire_chance + s.mechanics_hotwire * level as f32).clamp(0.0, 0.95)
}

/// Where a vehicle's container is reached from: the boot behind it, the glovebox at the driver's
/// seat.
pub fn container_point(kind: &VehicleKind, v: &super::VehicleState, slot: u8) -> Vec2 {
    if slot == BOOT { v.pos - v.forward() * (kind.length * 0.5) } else { seat_of(kind, v) }
}

/// Does `inv` hold the key to vehicle `vid` (the key item `key`)? Anywhere: hands, worn, carried.
pub fn holds_key(inv: &crate::inventory::Inventory, key: u16, vid: u32) -> bool {
    inv.stacks().any(|s| s.item == key && s.born == vid as i32)
}

impl World {
    /// The vehicles as clients see them: whether each starts goes by its driver too (a key on them).
    pub fn vehicle_views(&self) -> Vec<VehicleView> {
        (0..self.vehicles.list.len()).map(|vi| VehicleView::of(&self.vehicles.list[vi], self.startable(vi))).collect()
    }

    fn key_item(&self) -> Option<u16> {
        self.item_index(&self.config.vehicles.work.key_item)
    }

    /// Does player `pi` carry the key to vehicle `vid`?
    pub(crate) fn carries_key(&self, pi: usize, vid: u32) -> bool {
        self.key_item().is_some_and(|k| self.players.get(pi).is_some_and(|p| holds_key(&p.inventory, k, vid)))
    }

    /// Will vehicle `vi`'s engine start (or keep running): its key in the ignition, its wires
    /// crossed, or its key on its driver.
    pub(crate) fn startable(&self, vi: usize) -> bool {
        let v = &self.vehicles.list[vi];
        v.access.starts() || v.driver.and_then(|d| self.players.iter().position(|p| p.id == d)).is_some_and(|pi| self.carries_key(pi, v.id))
    }

    /// What the save keeps of the locks and keys (`VACC`).
    pub(crate) fn access_save(&self) -> Vec<(u32, Access)> {
        self.vehicles.list.iter().map(|v| (v.id, v.access)).collect()
    }

    /// The locks and keys from a save (after [`World::restore_vehicles`]): by id; one for a
    /// vehicle that isn't there is skipped.
    pub(crate) fn restore_access(&mut self, list: Vec<(u32, Access)>) -> Result<(), String> {
        for (id, a) in list {
            if a.ignition && a.glovebox_key {
                return Err(format!("vehicle {id}: its key can't be in the ignition and the glovebox at once"));
            }
            if let Some(v) = self.vehicles.list.iter_mut().find(|v| v.id == id) {
                v.access = a;
            }
        }
        Ok(())
    }

    /// The vehicle player `pi` works on: `vehicle` if within reach, else the one it faces (or the
    /// one it sits in).
    fn worked_on(&self, pi: usize, vehicle: Option<u32>) -> Result<usize, String> {
        let p = &self.players[pi];
        if let Some(vid) = p.vehicle
            && vehicle.is_none_or(|v| v == vid)
        {
            return self.vehicles.list.iter().position(|v| v.id == vid).ok_or_else(|| "no such car".into());
        }
        let reach = self.config.vehicles.work.reach;
        let boxes = self.vehicle_boxes();
        let (pos, facing) = (p.state.pos, p.state.facing);
        match vehicle {
            Some(vid) => {
                let i = self.vehicles.list.iter().position(|v| v.id == vid).ok_or("no such car")?;
                if boxes.get(i).is_none_or(|b| b.distance(pos) > reach) {
                    return Err("too far from the car".into());
                }
                Ok(i)
            }
            None => enterable(&boxes, pos, facing, reach).ok_or_else(|| "no car here".into()),
        }
    }

    /// Hands free and awake: who can work on a car.
    fn can_work(&self, pi: usize) -> Result<(), String> {
        let p = &self.players[pi];
        let why = if p.dead {
            "dead"
        } else if p.needs.asleep() || p.away.is_some() {
            "asleep"
        } else if p.knock.is_some() {
            "still reeling"
        } else if p.restraint.is_some() {
            "your hands are tied"
        } else if p.state.level > 0 || p.state.climb.is_some() {
            "not from here"
        } else if p.action.is_some() {
            "busy fighting"
        } else {
            return Ok(());
        };
        Err(format!("not now: {why}"))
    }

    fn label_of(&self, vi: usize) -> String {
        self.vehicle_kind(&self.vehicles.list[vi]).map_or_else(|| "car".into(), |k| k.label())
    }

    /// Cross the wires of the car player `pi` sits at the wheel of: a job.
    pub(super) fn start_hotwire(&mut self, pi: usize) -> Result<String, String> {
        self.can_work(pi)?;
        let id = self.players[pi].id;
        let Some(vid) = self.players[pi].vehicle else { return Err("get in at the wheel to hotwire a car".into()) };
        let vi = self.vehicles.list.iter().position(|v| v.id == vid).ok_or("no such car")?;
        if self.startable(vi) {
            return Err(format!("the {} starts already", self.label_of(vi)));
        }
        let w = self.config.vehicles.work.clone();
        let inv = &self.players[pi].inventory;
        if !inv.stacks().any(|s| self.config.items[s.item as usize].has_tag(&w.hotwire_tag)) {
            return Err("you need something to strip and cross the wires with (a screwdriver)".into());
        }
        if self.players[pi].jobs.iter().any(|j| matches!(j.kind, JobKind::Hotwire { .. })) {
            return Ok("already at it".into());
        }
        let level = self.skill_level(pi, Skill::Mechanics);
        let secs = w.hotwire_secs * self.config.skills.mechanics_time_mult(level);
        self.queue_job(id, JobKind::Hotwire { vehicle: vid }, secs)?;
        Ok(format!("you work at the wires under the {}'s wheel", self.label_of(vi)))
    }

    /// A try at hotwiring is due.
    pub(crate) fn do_hotwire(&mut self, pi: usize, vid: u32) -> Result<String, String> {
        let vi = self.vehicles.list.iter().position(|v| v.id == vid).ok_or("the car is gone")?;
        if self.players[pi].vehicle != Some(vid) {
            return Err("you left the wheel".into());
        }
        let w = self.config.vehicles.work.clone();
        if !self.players[pi].inventory.stacks().any(|s| self.config.items[s.item as usize].has_tag(&w.hotwire_tag)) {
            return Err("you need something to strip and cross the wires with".into());
        }
        let id = self.players[pi].id;
        let pos = self.vehicles.list[vi].state.pos;
        self.emit_sound_from(pos, w.hotwire_noise, crate::senses::SoundSource::Player(id));
        let level = self.skill_level(pi, Skill::Mechanics);
        let chance = hotwire_chance(&w, &self.config.skills, level);
        let roll = crate::rng::hash01(self.config.seed() ^ 0x4857_4952, vid as u64, self.tick, id as u64);
        let name = self.label_of(vi);
        if roll < chance {
            self.vehicles.list[vi].access.hotwired = true;
            let xp = self.config.skills.hotwire_xp;
            self.award_xp(pi, Skill::Mechanics, xp);
            Ok(format!("sparks: the {name}'s wires are crossed, it will start"))
        } else {
            let xp = self.config.skills.hotwire_fail_xp;
            self.award_xp(pi, Skill::Mechanics, xp);
            Err(format!("the {name}'s wires spark and nothing: try again ({:.0}% a try)", chance * 100.0))
        }
    }

    /// The driver takes the key out of the ignition (the engine stops unless it's hotwired).
    pub(super) fn take_key(&mut self, pi: usize) -> Result<String, String> {
        self.can_work(pi)?;
        let Some(vid) = self.players[pi].vehicle else { return Err("not in a car".into()) };
        let vi = self.vehicles.list.iter().position(|v| v.id == vid).ok_or("no such car")?;
        if !self.vehicles.list[vi].access.ignition {
            return Err("there's no key in the ignition".into());
        }
        let key = self.key_item().ok_or("there are no car keys in this game")?;
        self.vehicles.list[vi].access.ignition = false;
        let stack = ItemStack { born: vid as i32, ..ItemStack::one(key) };
        self.players[pi].inventory.carried.push(stack);
        self.refresh_carry(pi);
        Ok(format!("you take the {}'s key", self.label_of(vi)))
    }

    /// Lock or unlock a car: with its key on you, or from inside.
    pub(super) fn toggle_lock(&mut self, pi: usize, vehicle: Option<u32>) -> Result<String, String> {
        self.can_work(pi)?;
        let vi = self.worked_on(pi, vehicle)?;
        let vid = self.vehicles.list[vi].id;
        let inside = self.players[pi].vehicle == Some(vid);
        if !inside && !self.carries_key(pi, vid) {
            return Err(format!("you need the {}'s key to lock or unlock it", self.label_of(vi)));
        }
        let a = &mut self.vehicles.list[vi].access;
        a.locked = !a.locked;
        let locked = a.locked;
        let (id, pos) = (self.players[pi].id, self.vehicles.list[vi].state.pos);
        self.emit_sound_from(pos, self.config.vehicles.work.lock_noise, crate::senses::SoundSource::Player(id));
        Ok(format!("you {} the {}", if locked { "lock" } else { "unlock" }, self.label_of(vi)))
    }

    /// Smash a window of a locked car to open it.
    pub(super) fn break_in(&mut self, pi: usize, vehicle: Option<u32>) -> Result<String, String> {
        self.can_work(pi)?;
        if self.players[pi].vehicle.is_some() {
            return Err("get out first".into());
        }
        let vi = self.worked_on(pi, vehicle)?;
        if !self.vehicles.list[vi].access.locked {
            return Err(format!("the {} isn't locked", self.label_of(vi)));
        }
        let w = self.config.vehicles.work.clone();
        let v = &mut self.vehicles.list[vi];
        v.access.locked = false;
        v.cond.parts[2] = (v.cond.parts[2] - w.break_in_windows).max(0.0);
        let (id, pos) = (self.players[pi].id, v.state.pos);
        self.emit_sound_from(pos, w.break_in_noise, crate::senses::SoundSource::Player(id));
        Ok(format!("you smash the {}'s window and open it", self.label_of(vi)))
    }

    /// Why repair `r` can't be done by player `pi` on vehicle `vi` now, if it can't.
    fn repair_blocked(&self, pi: usize, vi: usize, r: &RepairDef) -> Option<String> {
        let level = self.skill_level(pi, Skill::Mechanics);
        if level < r.level {
            return Some(format!("{} needs Mechanics {} (you have {level})", r.name, r.level));
        }
        let now = self.vehicles.list[vi].cond.parts[r.part.index()];
        if now >= r.up_to - 1e-3 {
            return Some(if r.up_to >= 1.0 { format!("the {} is as good as it gets", r.part.label()) } else { format!("the {} is better than {} can make it", r.part.label(), r.name.to_lowercase()) });
        }
        crate::build::check_have(&r.inputs, &r.tools, &self.players[pi].inventory, &self.config.items, 1).err().map(|why| format!("can't {}: {why}", r.name.to_lowercase()))
    }

    /// Repairs of `part`, best first, that player `id` could do on vehicle `vid` now: (index in
    /// `work.repairs`, why not). For the client's menu and agents.
    pub fn repair_options(&self, id: u32, vid: u32) -> Vec<(usize, Option<String>)> {
        let (Some(pi), Some(vi)) = (self.players.iter().position(|p| p.id == id), self.vehicles.list.iter().position(|v| v.id == vid)) else { return Vec::new() };
        self.config.vehicles.work.repairs.iter().enumerate().map(|(k, r)| (k, self.repair_blocked(pi, vi, r))).collect()
    }

    /// Start repair `repair` (an index into `work.repairs`) on the car faced (or `vehicle`): a job.
    pub(super) fn start_repair(&mut self, pi: usize, vehicle: Option<u32>, repair: u8) -> Result<String, String> {
        self.can_work(pi)?;
        let r = self.config.vehicles.work.repairs.get(repair as usize).cloned().ok_or("no such repair")?;
        let vi = self.worked_on(pi, vehicle)?;
        let vid = self.vehicles.list[vi].id;
        if r.part != Part::Windows && self.players[pi].vehicle == Some(vid) {
            return Err("get out to work on it".into());
        }
        if let Some(why) = self.repair_blocked(pi, vi, &r) {
            return Err(why);
        }
        if self.players[pi].jobs.iter().any(|j| matches!(j.kind, JobKind::Repair { vehicle, .. } if vehicle == vid)) {
            return Ok("already at it".into());
        }
        let id = self.players[pi].id;
        let secs = r.secs * self.config.skills.mechanics_time_mult(self.skill_level(pi, Skill::Mechanics));
        self.queue_job(id, JobKind::Repair { vehicle: vid, repair }, secs)?;
        Ok(format!("you start to {}", r.name.to_lowercase()))
    }

    /// A repair is due: checked again, its inputs used, the part mended.
    pub(crate) fn do_repair(&mut self, pi: usize, vid: u32, repair: u8) -> Result<String, String> {
        let r = self.config.vehicles.work.repairs.get(repair as usize).cloned().ok_or("that repair is gone")?;
        let vi = self.vehicles.list.iter().position(|v| v.id == vid).ok_or("the car is gone")?;
        let inside = self.players[pi].vehicle == Some(vid);
        let reach = self.config.vehicles.work.reach + 0.3;
        let b = self.vehicle_kind(&self.vehicles.list[vi]).map(|k| self.vehicles.list[vi].boxed(k));
        if !inside && b.is_none_or(|b| b.distance(self.players[pi].state.pos) > reach) {
            return Err(format!("moved away from the {}", self.label_of(vi)));
        }
        if let Some(why) = self.repair_blocked(pi, vi, &r) {
            return Err(why);
        }
        let mut inv = self.players[pi].inventory.clone();
        for x in &r.inputs {
            let i = self.item_index(&x.item).ok_or_else(|| format!("no item {}", x.item))?;
            if !crate::crafting::take_whole(&mut inv, i, x.count) {
                return Err(format!("can't {}: the {} is gone", r.name.to_lowercase(), x.item.replace('_', " ")));
            }
        }
        self.players[pi].inventory = inv;
        self.refresh_carry(pi);
        let level = self.skill_level(pi, Skill::Mechanics);
        let add = r.mends * self.config.skills.mechanics_amount_mult(level);
        let part = &mut self.vehicles.list[vi].cond.parts[r.part.index()];
        *part = (*part + add).min(r.up_to.max(*part)).min(1.0);
        let now = *part;
        let xp = self.config.skills.repair_xp;
        self.award_xp(pi, Skill::Mechanics, xp);
        Ok(format!("{}: the {}'s {} is at {:.0}%", r.name, self.label_of(vi), r.part.label(), now * 100.0))
    }

    /// The container kind index of a vehicle's boot or glovebox, if `containers.ron` has it.
    fn vehicle_container_def(&self, slot: u8) -> Option<u16> {
        let w = &self.config.vehicles.work;
        let id = if slot == BOOT { &w.boot } else { &w.glovebox };
        self.config.containers.containers.iter().position(|c| c.id == *id).map(|i| i as u16)
    }

    fn vehicle_site_of(&self, v: &Vehicle, slot: u8, def: u16) -> Option<ContainerSite> {
        let kind = self.vehicle_kind(v)?;
        let at = container_point(kind, &v.state, slot);
        let tile = at.floor().as_ivec2();
        let room = if self.map.in_bounds(tile) { self.map.room_at(at) } else { 0 };
        Some(ContainerSite { id: vehicle_container(v.id, slot), def, level: 0, building: u16::MAX, room, min: tile, size: IVec2::ONE, body: Some(at) })
    }

    /// Container `c` if it is a vehicle's boot or glovebox.
    pub(crate) fn vehicle_site(&self, c: u32) -> Option<ContainerSite> {
        let (vid, slot) = vehicle_of_container(c)?;
        let def = self.vehicle_container_def(slot)?;
        let v = self.vehicles.list.iter().find(|v| v.id == vid)?;
        self.vehicle_site_of(v, slot, def)
    }

    /// The boots and gloveboxes within `reach` of `pos`, by id.
    pub fn vehicle_sites_near(&self, pos: Vec2, reach: f32) -> Vec<ContainerSite> {
        let mut out = Vec::new();
        for slot in [BOOT, GLOVEBOX] {
            let Some(def) = self.vehicle_container_def(slot) else { continue };
            for v in &self.vehicles.list {
                if let Some(s) = self.vehicle_site_of(v, slot, def)
                    && s.distance(pos) <= reach
                {
                    out.push(s);
                }
            }
        }
        out.sort_by_key(|s| s.id);
        out
    }

    /// Can player `p` get at vehicle container `c` at all (the lock, and which side of the door
    /// it's on): the glovebox from the wheel or an unlocked door, the boot from outside an
    /// unlocked car. Not a vehicle's container: yes.
    pub(crate) fn vehicle_container_open_to(&self, p: &crate::players::Player, c: u32) -> bool {
        let Some((vid, slot)) = vehicle_of_container(c) else { return true };
        let Some(v) = self.vehicles.list.iter().find(|v| v.id == vid) else { return false };
        match p.vehicle {
            Some(at) if at == vid => slot == GLOVEBOX,
            Some(_) => false,
            None => !v.access.locked,
        }
    }

    /// What a boot or glovebox holds when first searched: its loot, and the car's key if it was
    /// left in the glovebox.
    pub(crate) fn fill_vehicle(&mut self, c: u32, site: &ContainerSite) -> Vec<ItemStack> {
        let def = self.container_def(site).clone();
        let mut out = crate::loot::fill(self.config.loot_seed(), c, &def, crate::containers::ANY_ROOM, &self.config.loot, &self.config.items);
        if let Some((vid, GLOVEBOX)) = vehicle_of_container(c)
            && let Some(key) = self.key_item()
            && let Some(v) = self.vehicles.list.iter_mut().find(|v| v.id == vid)
            && v.access.glovebox_key
        {
            v.access.glovebox_key = false;
            out.push(ItemStack { born: vid as i32, ..ItemStack::one(key) });
        }
        out
    }
}
