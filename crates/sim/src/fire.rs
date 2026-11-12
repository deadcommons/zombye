//! Fire and cooking.
//!
//! **Campfires** ([`Fire`]) are built outdoors by a recipe that `places: Some(Campfire)`
//! (`recipes.ron`): its inputs are the first fuel, and the fire is put down lit about a metre in
//! front of the survivor. A fire burns its fuel in game hours (`FireTuning::hours_per_kg` of every
//! kilogram of fuel put on it, at most `max_fuel_hours`), then goes out and stays as a cold fire pit
//! that can be fed and lit again. Tending it is [`World::player_fire`] (`Intent::Fire`): put on one
//! carried item tagged `fuel` (a plank, firewood, a newspaper), light it with something tagged `fire`
//! (matches, a lighter; kept), or put it out. Fires are kept in the world, saved (the optional `FIRE`
//! section) and in the state hash.
//!
//! **Cooking** is crafting: a recipe with `heat: true` is only made within `reach` of a lit fire or a
//! working stove (furniture of the kind `stove_furniture`, while its building has power: the town's
//! grid, `crate::power`). [`heat_near`] is the one rule, pure, so the inventory's Make list offers
//! only what the server makes. Cooked food is made fresh; raw meat and eggs carry a chance of food
//! poisoning when eaten as they are (`items::Consumable::poison`).
//!
//! **A fire is a hazard and gives you away.** Standing in one burns (`WoundKind::Burn` on a leg or a
//! foot, `burn_chance_per_sec`; clothing there may stop it); zombies walking through one take
//! `zombie_burn_per_sec` and can burn to death. It crackles (a small noise now and then), and at night
//! whoever stands within `light_radius` of a lit fire is seen by zombies as if by `seen_as_daylight`.
//! No campfires indoors (`indoors: false`): fire doesn't spread, so the rule keeps houses standing.
//!
//! Every number is in [`FireTuning`] (`assets/data/fire.ron`, `WorldConfig::fire`, saved with the
//! settings). All ours.

use glam::{IVec2, Vec2};
use serde::{Deserialize, Serialize};

use crate::health::{Part, WoundKind};
use crate::map::TileMap;
use crate::world::World;

/// `assets/data/fire.ron`. Every field is optional (these defaults).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FireTuning {
    /// Metres from a fire's middle, or a stove's footprint, that a survivor can cook at and tend it from.
    pub reach: f32,
    /// Game hours of burning each kilogram of fuel gives.
    pub hours_per_kg: f32,
    /// Most game hours of fuel a campfire holds.
    pub max_fuel_hours: f32,
    /// How far in front of the survivor a new campfire goes (metres).
    pub build_ahead: f32,
    /// Fires kept at least this far apart (metres).
    pub spacing: f32,
    /// Most fires in a world.
    pub max_fires: u32,
    /// Campfires may be built indoors (fire doesn't spread; off keeps houses standing).
    pub indoors: bool,
    /// Standing within this of a lit fire's middle burns (metres).
    pub burn_radius: f32,
    /// Chance a second of a burn while standing in a lit fire.
    pub burn_chance_per_sec: f32,
    /// Hit points a second a zombie standing in a lit fire loses.
    pub zombie_burn_per_sec: f32,
    /// A lit fire crackles: a noise this loud (metres)...
    pub crackle_noise: f32,
    /// ...every this many seconds.
    pub crackle_every_secs: f32,
    /// At night a survivor within this of a lit fire is in its light...
    pub light_radius: f32,
    /// ...and zombies see them as if by this much daylight (0 dark, 1 full day).
    pub seen_as_daylight: f32,
    /// The furniture kind that cooks while its building has power.
    pub stove_furniture: String,
    /// Stoves need power (off: they always cook, like gas).
    pub stoves_need_power: bool,
}

impl Default for FireTuning {
    fn default() -> Self {
        Self {
            reach: 1.6,
            hours_per_kg: 0.75,
            max_fuel_hours: 8.0,
            build_ahead: 1.0,
            spacing: 2.0,
            max_fires: 200,
            indoors: false,
            burn_radius: 0.5,
            burn_chance_per_sec: 0.6,
            zombie_burn_per_sec: 6.0,
            crackle_noise: 6.0,
            crackle_every_secs: 4.0,
            light_radius: 6.0,
            seen_as_daylight: 0.6,
            stove_furniture: "stove".into(),
            stoves_need_power: true,
        }
    }
}

impl FireTuning {
    /// Settings a tick can't work with, by name (checked on load and when the file is read).
    pub fn check(&self) -> Result<(), String> {
        let ok = |v: f32, lo: f32, hi: f32| v.is_finite() && v >= lo && v <= hi;
        for (name, v, lo, hi) in [
            ("reach", self.reach, 0.1, 10.0),
            ("hours_per_kg", self.hours_per_kg, 0.0, 100.0),
            ("max_fuel_hours", self.max_fuel_hours, 0.0, 1000.0),
            ("build_ahead", self.build_ahead, 0.3, 3.0),
            ("spacing", self.spacing, 0.0, 50.0),
            ("burn_radius", self.burn_radius, 0.0, 3.0),
            ("burn_chance_per_sec", self.burn_chance_per_sec, 0.0, 20.0),
            ("zombie_burn_per_sec", self.zombie_burn_per_sec, 0.0, 1000.0),
            ("crackle_noise", self.crackle_noise, 0.0, 100.0),
            ("crackle_every_secs", self.crackle_every_secs, 0.0, 600.0),
            ("light_radius", self.light_radius, 0.0, 50.0),
            ("seen_as_daylight", self.seen_as_daylight, 0.0, 1.0),
        ] {
            if !ok(v, lo, hi) {
                return Err(format!("fire.{name} must be from {lo} to {hi}, got {v}"));
            }
        }
        if self.max_fires > 10_000 {
            return Err(format!("fire.max_fires must be at most 10000, got {}", self.max_fires));
        }
        Ok(())
    }
}

/// Parse and check `fire.ron`.
pub fn parse_fire(text: &str) -> Result<FireTuning, String> {
    let t: FireTuning = ron::from_str(text).map_err(|e| e.to_string())?;
    t.check()?;
    Ok(t)
}

/// What a recipe can put down instead of (or as well as) items.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Placed {
    /// A campfire, lit, fed with the recipe's inputs that are fuel.
    Campfire,
}

/// A campfire on the ground floor.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Fire {
    pub id: u32,
    pub pos: Vec2,
    /// Game hours of fuel left.
    pub fuel_h: f32,
    pub lit: bool,
}

/// A fire as clients see it: the fuel to the nearest game minute, so it changes now and then.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FireView {
    pub id: u32,
    pub pos: [f32; 2],
    pub lit: bool,
    /// Game minutes of fuel left.
    pub fuel_min: u16,
}

impl Fire {
    pub fn view(&self) -> FireView {
        FireView { id: self.id, pos: self.pos.into(), lit: self.lit, fuel_min: (self.fuel_h.max(0.0) * 60.0).ceil().min(u16::MAX as f32) as u16 }
    }
}

/// The fires in a world, in id order.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Fires {
    pub list: Vec<Fire>,
    pub next_id: u32,
}

impl Fires {
    pub fn get(&self, id: u32) -> Option<&Fire> {
        self.list.iter().find(|f| f.id == id)
    }

    /// Checked on load: ids in order and below the next, numbers a tick can work with, on the map.
    pub fn check(&self, map: &TileMap) -> Result<(), String> {
        if self.list.windows(2).any(|w| w[0].id >= w[1].id) || self.list.iter().any(|f| f.id == 0 || f.id >= self.next_id.max(1)) {
            return Err("fires out of order or with impossible ids".into());
        }
        for f in &self.list {
            if !(f.pos.is_finite() && f.fuel_h.is_finite() && f.fuel_h >= 0.0 && f.fuel_h <= 1000.0) || !map.in_bounds(f.pos.floor().as_ivec2()) {
                return Err(format!("fire {} holds impossible values", f.id));
            }
            if f.lit && f.fuel_h <= 0.0 {
                return Err(format!("fire {} burns with no fuel", f.id));
            }
        }
        Ok(())
    }

    /// Fold into the state hash (only while there are any, so worlds without hash as before).
    pub fn hash(&self, mix: &mut impl FnMut(u32)) {
        for f in &self.list {
            mix(0x4649_0000 ^ f.id ^ (f.lit as u32) << 31);
            mix(f.pos.x.to_bits());
            mix(f.pos.y.to_bits());
            mix(f.fuel_h.to_bits());
        }
    }
}

/// A stove's footprint and building.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stove {
    pub min: IVec2,
    pub size: IVec2,
    pub building: usize,
}

impl Stove {
    pub fn nearest(&self, p: Vec2) -> Vec2 {
        p.clamp(self.min.as_vec2(), (self.min + self.size).as_vec2())
    }
}

/// Every ground-floor stove in the map (furniture of the kind `t.stove_furniture`).
pub fn stoves<'a>(map: &'a TileMap, t: &'a FireTuning) -> impl Iterator<Item = Stove> + 'a {
    map.buildings.iter().enumerate().flat_map(move |(b, bld)| bld.furniture.iter().filter(move |f| f.level == 0 && f.kind == t.stove_furniture).map(move |f| Stove { min: f.min, size: f.size, building: b }))
}

/// Is there heat to cook with for a survivor at `pos` on floor `level`: a lit fire within reach with
/// nothing in between, or a stove in reach in the same room (with power, if stoves need it)? The one
/// rule, for the server and the client's Make list. `fires` are the fires' places and whether each
/// burns; `powered(b)` says whether building `b` has power.
pub fn heat_near(map: &TileMap, t: &FireTuning, fires: impl IntoIterator<Item = (Vec2, bool)>, pos: Vec2, level: u8, powered: impl Fn(usize) -> bool) -> bool {
    if level != 0 {
        return false;
    }
    if fires.into_iter().any(|(at, lit)| lit && at.distance(pos) <= t.reach && crate::vision::clear_reach(map, pos, at)) {
        return true;
    }
    let room = map.room_at(pos);
    room != 0
        && stoves(map, t).any(|s| {
            let near = s.nearest(pos);
            near.distance(pos) <= t.reach && map.room_at(s.min.as_vec2() + s.size.as_vec2() * 0.5) == room && (!t.stoves_need_power || powered(s.building))
        })
}

/// How lit a survivor at `pos` is by the fires, as daylight for zombies' sight (0: not in any
/// lit fire's light).
pub fn firelight(fires: &Fires, t: &FireTuning, pos: Vec2) -> f32 {
    if fires.list.iter().any(|f| f.lit && f.pos.distance_squared(pos) <= t.light_radius * t.light_radius) { t.seen_as_daylight } else { 0.0 }
}

/// What [`World::player_fire`] does to a fire.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FireOp {
    /// Put one of the carried stack `index` (tagged `fuel`) on it.
    AddFuel { index: u32 },
    /// Light it (needs fuel and something tagged `fire` to hand).
    Light,
    /// Put it out (the fuel left stays for next time).
    PutOut,
}

/// The parts a fire burns: what stands in it.
const BURN_PARTS: [Part; 4] = [Part::LeftLeg, Part::RightLeg, Part::LeftFoot, Part::RightFoot];

impl World {
    /// The fires, in id order.
    pub fn fires(&self) -> &[Fire] {
        &self.fires.list
    }

    /// Is there heat to cook with where player `pi` stands ([`heat_near`])?
    pub fn heat_for(&self, pi: usize) -> bool {
        let p = &self.players[pi];
        heat_near(&self.map, &self.config.fire, self.fires.list.iter().map(|f| (f.pos, f.lit)), p.state.pos, p.state.level, |b| self.power.powered(b))
    }

    /// Where player `pi` would build a campfire now, or why not.
    pub(crate) fn campfire_spot(&self, pi: usize) -> Result<Vec2, String> {
        let t = &self.config.fire;
        let p = &self.players[pi];
        if p.state.level != 0 {
            return Err("not upstairs".into());
        }
        let at = p.state.pos + Vec2::from_angle(p.state.facing) * t.build_ahead;
        let tile = at.floor().as_ivec2();
        if !t.indoors && (self.map.room_at(at) != 0 || self.map.room_at(p.state.pos) != 0) {
            return Err("not indoors: build it outside".into());
        }
        if self.map.tile_blocked(tile) || !crate::vision::clear_reach(&self.map, p.state.pos, at) {
            return Err("something is in the way in front of you".into());
        }
        if self.fires.list.len() >= t.max_fires as usize {
            return Err("there are too many fires already".into());
        }
        if self.fires.list.iter().any(|f| f.pos.distance(at) < t.spacing) {
            return Err("there's a fire right there already".into());
        }
        Ok(at)
    }

    /// Fuel (game hours) one item gives.
    fn fuel_hours(&self, item: u16) -> f32 {
        let d = self.item_def(item);
        if d.has_tag("fuel") { d.weight_kg.max(0.0) * self.config.fire.hours_per_kg } else { 0.0 }
    }

    /// Put a lit campfire down at `at`, fed with `fuel` (items by index, one each). Returns its id.
    pub(crate) fn place_campfire(&mut self, at: Vec2, fuel: &[(u16, u16)]) -> u32 {
        let hours: f32 = fuel.iter().map(|&(i, n)| self.fuel_hours(i) * n as f32).sum();
        let fuel_h = hours.min(self.config.fire.max_fuel_hours);
        let id = self.fires.next_id.max(1);
        self.fires.next_id = id + 1;
        self.fires.list.push(Fire { id, pos: at, fuel_h, lit: fuel_h > 0.0 });
        id
    }

    /// Debug and tests: a campfire at `pos` with `hours` of fuel, lit or not. Returns its id.
    pub fn add_fire(&mut self, pos: Vec2, hours: f32, lit: bool) -> u32 {
        let id = self.fires.next_id.max(1);
        self.fires.next_id = id + 1;
        let fuel_h = hours.clamp(0.0, self.config.fire.max_fuel_hours.max(0.0));
        self.fires.list.push(Fire { id, pos, fuel_h, lit: lit && fuel_h > 0.0 });
        id
    }

    /// Player `id` tends fire `fire`: puts fuel on, lights it or puts it out ([`FireOp`]), from
    /// within `FireTuning::reach` with nothing in between. Says how it went in the player's notes
    /// and returns it.
    pub fn player_fire(&mut self, id: u32, fire: u32, op: FireOp) -> Result<String, String> {
        let Some(pi) = self.players.iter().position(|p| p.id == id) else { return Err(format!("no player {id}")) };
        let p = &self.players[pi];
        if p.dead || p.away.is_some() {
            return Err("dead".into());
        }
        let (pos, level) = (p.state.pos, p.state.level);
        let t = self.config.fire.clone();
        let Some(k) = self.fires.list.iter().position(|f| f.id == fire) else { return self.refuse(id, "that fire is gone".into()) };
        let f = self.fires.list[k];
        if level != 0 || f.pos.distance(pos) > t.reach || !crate::vision::clear_reach(&self.map, pos, f.pos) {
            return self.refuse(id, "the fire is out of reach".into());
        }
        let text = match op {
            FireOp::Light => {
                if f.lit {
                    return self.refuse(id, "it's burning already".into());
                }
                if f.fuel_h <= 0.0 {
                    return self.refuse(id, "nothing to burn: put some fuel on it first".into());
                }
                let items = &self.config.items;
                if !crate::crafting::has_tool(&self.players[pi].inventory, &crate::crafting::Tool::Tag("fire".into()), items) {
                    let with = crate::crafting::tool_text(items, &crate::crafting::Tool::Tag("fire".into()));
                    return self.refuse(id, format!("needs {with} to light it"));
                }
                self.fires.list[k].lit = true;
                format!("lit the fire: about {} of fuel", hours_text(f.fuel_h))
            }
            FireOp::PutOut => {
                if !f.lit {
                    return self.refuse(id, "it isn't burning".into());
                }
                self.fires.list[k].lit = false;
                self.emit_sound_from(f.pos, 3.0, crate::senses::SoundSource::Player(id));
                "put the fire out".into()
            }
            FireOp::AddFuel { index } => {
                let Some(s) = self.players[pi].inventory.carried.get(index as usize).copied() else { return self.refuse(id, "that isn't in your bags".into()) };
                let add = self.fuel_hours(s.item);
                let name = self.item_def(s.item).name.to_lowercase();
                if add <= 0.0 {
                    return self.refuse(id, format!("the {name} won't burn"));
                }
                if f.fuel_h >= t.max_fuel_hours - 0.05 {
                    return self.refuse(id, "it can't take any more".into());
                }
                let inv = &mut self.players[pi].inventory;
                inv.carried[index as usize].count -= 1;
                if inv.carried[index as usize].count == 0 {
                    inv.carried.remove(index as usize);
                }
                let fuel = (f.fuel_h + add).min(t.max_fuel_hours);
                self.fires.list[k].fuel_h = fuel;
                self.private_dirty.insert(id);
                self.refresh_carry(pi);
                format!("put a {name} on the fire: about {} of fuel", hours_text(fuel))
            }
        };
        self.note(id, true, text.clone());
        Ok(text)
    }

    /// Each tick: fires burn their fuel (`hours` of game time) and go out, crackle, and burn what
    /// stands in them (`dt` seconds of real time).
    pub(crate) fn step_fires(&mut self, hours: f32, dt: f32) {
        if self.fires.list.is_empty() {
            return;
        }
        let t = self.config.fire.clone();
        let every = ((t.crackle_every_secs / crate::TICK_DT).round() as u64).max(1);
        let crackle = t.crackle_noise > 0.0 && self.tick % every == 0;
        let mut lit = Vec::new();
        for f in &mut self.fires.list {
            if !f.lit {
                continue;
            }
            f.fuel_h -= hours;
            if f.fuel_h <= 0.0 {
                f.fuel_h = 0.0;
                f.lit = false;
                continue;
            }
            lit.push(f.pos);
        }
        if lit.is_empty() {
            return;
        }
        if crackle {
            for &at in &lit {
                self.emit_sound(at, t.crackle_noise);
            }
        }
        let r2 = t.burn_radius * t.burn_radius;
        // Survivors standing in one.
        for pi in 0..self.players.len() {
            let p = &self.players[pi];
            if p.dead || p.away.is_some() || p.state.level != 0 || !lit.iter().any(|at| at.distance_squared(p.state.pos) <= r2) {
                continue;
            }
            if self.combat_rng.f32() < t.burn_chance_per_sec * dt {
                self.burn_player(pi);
            }
        }
        // Zombies walking through one.
        if t.zombie_burn_per_sec > 0.0 {
            let mut killed = Vec::new();
            let z = &mut self.zombies;
            for j in 0..z.len() {
                if z.state[j] == crate::zombies::ZombieState::Dead || z.tier[j] != crate::zombies::Tier::Active || !lit.iter().any(|at| at.distance_squared(z.pos[j]) <= r2) {
                    continue;
                }
                z.hp[j] -= t.zombie_burn_per_sec * dt;
                if z.hp[j] <= 0.0 {
                    z.hp[j] = 0.0;
                    z.state[j] = crate::zombies::ZombieState::Dead;
                    z.anim[j] = crate::zombies::ZombieAnim::Dead;
                    z.target_player[j] = 0;
                    z.timer[j] = 0.0;
                    killed.push((z.id[j], z.pos[j]));
                }
            }
            for (zid, zpos) in killed {
                self.push_combat_event(crate::combat::CombatEvent { kind: crate::combat::CombatEventKind::ZombieKilled, pos: zpos, dir: Vec2::X, zombie: zid, player: 0, damage: 0.0, crit: false, part: None, wound: None, by: 0 });
            }
        }
    }

    /// Player `pi` stands in a fire: a burn on a leg or a foot, unless what it wears there stops it.
    fn burn_player(&mut self, pi: usize) {
        let part = BURN_PARTS[self.combat_rng.below(BURN_PARTS.len() as u32) as usize];
        let stop = self.protection(&self.players[pi], part.region()) * self.config.health.burn.protection;
        if self.combat_rng.f32() < stop {
            return;
        }
        let (lo, hi) = self.config.health.burn.damage;
        let damage = self.combat_rng.range(lo, hi);
        let p = &self.players[pi];
        let (pos, id, god) = (p.state.pos, p.id, p.dev.god);
        self.push_combat_event(crate::combat::CombatEvent { kind: crate::combat::CombatEventKind::PlayerHurt, pos, dir: Vec2::X, zombie: 0, player: id, damage, crit: false, part: Some(part), wound: Some(WoundKind::Burn), by: 0 });
        if !god {
            self.add_wound(pi, part, WoundKind::Burn, false, None);
        }
        self.interrupt_treatment(pi);
        self.note(id, false, format!("you're standing in the fire: a burn on the {}", part.name()));
        self.hurt_player_by(pi, damage, crate::run::DeathCause::Burned { part });
    }
}

/// "3 h 20 min", "40 min".
pub fn hours_text(h: f32) -> String {
    let m = (h.max(0.0) * 60.0).round() as u32;
    match (m / 60, m % 60) {
        (0, m) => format!("{m} min"),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m} min"),
    }
}

#[cfg(test)]
mod tests;
