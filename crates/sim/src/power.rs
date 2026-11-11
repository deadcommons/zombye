//! Electricity and room lights.
//!
//! **Power sources** ([`Source`]) each cover part of the map ([`Coverage`]) and are on or off. A
//! building has power while any source that is on covers it ([`Power::powered`]). Only the town's
//! **grid** exists for now: it covers everywhere, starts on or off as `power.ron` says
//! ([`GridDef`]), and can fail for good some game hours after the world began. Generators and solar
//! panels are kinds already ([`SourceKind`]), meant to cover one building or an area round them;
//! nothing makes one yet.
//!
//! **Light switches:** every room has one ([`Power::switches`], on or off). A room is **lit** when
//! its switch is on and its building has power ([`Power::lit`]): so flipping a switch with the grid
//! down does nothing you can see, and the lights left on come back if power does. When the world
//! begins about one room in five has its light left on (`lights_on_at_start`), the same rooms the
//! client used to light by itself (never a hall, porch or garage).
//!
//! **Lit rooms are seen:** at night a player standing in a lit room is seen by zombies as if by
//! `lit_room_seen` of daylight (`senses::Target::lit`), so a light left on in a house gives you away
//! through its windows. Turning it off is the way to hide.
//!
//! The switches and the sources' state are saved (the optional `POWR` section) and in the state
//! hash; [`PowerDef`] is saved with the settings. Clients get the lit rooms in each snapshot.

use std::collections::BTreeSet;

use glam::{IVec2, Vec2};
use serde::{Deserialize, Serialize};

use crate::map::{Edge, Feature, Side, TileMap};

/// The grid as `power.ron` describes it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GridDef {
    /// On when the world begins.
    pub on: bool,
    /// Game hours after the world began when the grid fails for good (`None`: it never does).
    pub fails_after_hours: Option<f32>,
}

impl Default for GridDef {
    fn default() -> Self {
        Self { on: true, fails_after_hours: None }
    }
}

/// `assets/data/power.ron`: the grid and the lights. Every field is optional (these defaults).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PowerDef {
    pub grid: GridDef,
    /// The share of rooms (not halls, porches or garages) with a light left on when the world begins.
    pub lights_on_at_start: f32,
    /// How well zombies see a player standing in a lit room at night, as daylight (0 dark, 1 full day).
    pub lit_room_seen: f32,
}

impl Default for PowerDef {
    fn default() -> Self {
        Self { grid: GridDef::default(), lights_on_at_start: 0.22, lit_room_seen: 0.8 }
    }
}

impl PowerDef {
    /// Explains the first mistake, if any.
    pub fn validate(&self) -> Result<(), String> {
        let check = |name: &str, v: f32, lo: f32, hi: f32| if v.is_finite() && (lo..=hi).contains(&v) { Ok(()) } else { Err(format!("{name} must be in {lo}..{hi}, got {v}")) };
        check("lights_on_at_start", self.lights_on_at_start, 0.0, 1.0)?;
        check("lit_room_seen", self.lit_room_seen, 0.0, 1.0)?;
        if let Some(h) = self.grid.fails_after_hours {
            check("grid.fails_after_hours", h, 0.0, 1e6)?;
        }
        Ok(())
    }
}

/// Read and check `power.ron`.
pub fn parse_power(text: &str) -> Result<PowerDef, String> {
    let def: PowerDef = ron::from_str(text).map_err(|e| e.to_string())?;
    def.validate()?;
    Ok(def)
}

/// What makes the power.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SourceKind {
    /// The town's grid.
    Grid,
    /// A generator (not made by anything yet).
    Generator,
    /// Solar panels (not made by anything yet).
    Solar,
}

/// Where a source's power reaches.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Coverage {
    /// Every building on the map.
    Everywhere,
    /// These buildings (indices into `TileMap::buildings`).
    Buildings(Vec<u16>),
    /// Every building whose middle is within `radius` metres of `center`.
    Within { center: Vec2, radius: f32 },
}

impl Coverage {
    pub fn covers(&self, map: &TileMap, building: usize) -> bool {
        match self {
            Coverage::Everywhere => true,
            Coverage::Buildings(list) => list.iter().any(|&b| b as usize == building),
            Coverage::Within { center, radius } => map.buildings.get(building).is_some_and(|b| (b.min.as_vec2() + b.size.as_vec2() * 0.5).distance(*center) <= *radius),
        }
    }
}

/// One power source.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Source {
    pub kind: SourceKind,
    pub covers: Coverage,
    pub on: bool,
}

/// The world's power: sources, switches and what is lit. See the module docs.
#[derive(Clone, Debug, Default)]
pub struct Power {
    /// The sources, the grid first.
    pub sources: Vec<Source>,
    /// The grid's failure (`GridDef::fails_after_hours`) has happened (it happens once).
    pub grid_failed: bool,
    /// Rooms whose light switch is on.
    pub switches: BTreeSet<u16>,
    /// Per building: has power (derived from the sources).
    powered: Vec<bool>,
    /// Rooms lit now, in order (derived: switch on and powered).
    lit: Vec<u16>,
    /// Where each room's switch is on its wall, by room (derived from the map: [`switch_spots`]).
    spots: Vec<Option<SwitchSpot>>,
}

/// Where a room's light switch is: on the inside face of a wall, beside a doorway into the room
/// (else on any of its walls). The same on every machine: worked out from the map alone.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SwitchSpot {
    /// The point on the wall's face, on the ground plane (the client draws it at
    /// [`SWITCH_HEIGHT`]).
    pub pos: Vec2,
    /// The way the switch faces: out of the wall, into the room.
    pub normal: Vec2,
    /// The tile the switch is in, and the side of it the wall is on.
    pub tile: IVec2,
    pub side: Side,
}

/// How high a switch is on its wall, in metres.
pub const SWITCH_HEIGHT: f32 = 1.2;
/// How far from a switch a hand reaches it (the server's rule; the client asks the same).
pub const SWITCH_REACH: f32 = 1.3;
/// How far a switch sits out from the line of its wall: half a wall (0.08) and a little.
pub const SWITCH_INSET: f32 = 0.1;
/// How far along the wall from the middle of its tile edge, toward the doorway beside it.
const SWITCH_TOWARD_DOOR: f32 = 0.3;

/// Every room's switch, by room (room 0, outdoors, has none). Only ground-floor rooms are on the
/// tiles, so upper rooms have none either (nobody is up there yet).
pub fn switch_spots(map: &TileMap) -> Vec<Option<SwitchSpot>> {
    // The best spot found for each room, with its score: lower is better.
    let mut best: Vec<Option<(u32, SwitchSpot)>> = vec![None; map.rooms.len()];
    let spot = |t: IVec2, side: Side, along: Vec2| {
        let out = side.offset().as_vec2();
        let pos = t.as_vec2() + Vec2::splat(0.5) + out * (0.5 - SWITCH_INSET) + along;
        SwitchSpot { pos, normal: -out, tile: t, side }
    };
    // Furniture against the wall would hide the switch (a fridge, a wardrobe): keep clear of it.
    let blocked = |t: IVec2| map.tile(t).feature == Feature::Furniture;
    // The room's north and west walls face the camera from inside at its usual angle; its south
    // and east walls are the ones the cutaways drop, where a switch can only lie on the stub.
    let near_side = |side: Side| matches!(side, Side::S | Side::E);
    // A tile in a recess of the room one tile wide (an alcove, a short passage): walled in on both
    // sides along the wall, so a switch there would be tucked out of the way.
    let recess = |t: IVec2, side: Side| {
        let r = map.tile(t).room;
        let perp = IVec2::new(side.offset().y, side.offset().x);
        let open = |d: IVec2| Side::ALL.into_iter().find(|s| s.offset() == d).is_some_and(|s| map.edge(t, s) == Edge::None && map.tile(t + d).room == r);
        !open(perp) && !open(-perp)
    };
    // How much of the room a switch on this tile's `side` wall looks out over: the room's tiles in
    // a band 7 wide and 4 deep in front of it. A switch tucked in a corner of an L or a nook sees
    // little of the room; one on its main wall sees a lot. Scored as what it misses.
    let hidden = |t: IVec2, side: Side| {
        let r = map.tile(t).room;
        let inward = -side.offset();
        let perp = IVec2::new(inward.y, inward.x);
        let mut seen = 0u32;
        for d in 0..4 {
            for l in -3..=3 {
                let q = t + inward * d + perp * l;
                if map.in_bounds(q) && map.tile(q).room == r {
                    seen += 1;
                }
            }
        }
        28 - seen
    };
    // Each room's doorways (the tile inside), for how far a wall is from the nearest.
    let mut doors: Vec<Vec<IVec2>> = vec![Vec::new(); map.rooms.len()];
    let tiles = || map.buildings.iter().flat_map(|b| (b.min.y..b.min.y + b.size.y).flat_map(move |y| (b.min.x..b.min.x + b.size.x).map(move |x| IVec2::new(x, y))));
    let room_of = |t: IVec2| {
        let r = map.tile(t).room;
        (r != 0 && (r as usize) < map.rooms.len()).then_some(r)
    };
    let way_in = |t: IVec2, side: Side, r: u16| matches!(map.edge(t, side), Edge::DoorClosed | Edge::DoorOpen | Edge::DoorBroken | Edge::Arch) && map.tile(t + side.offset()).room != r;
    for t in tiles() {
        let Some(r) = room_of(t) else { continue };
        if Side::ALL.into_iter().any(|side| way_in(t, side, r)) {
            doors[r as usize].push(t);
        }
    }
    let mut offer = |r: u16, score: u32, s: SwitchSpot| {
        let slot = &mut best[r as usize];
        if slot.map_or(true, |(b, _)| score < b) {
            *slot = Some((score, s));
        }
    };
    for t in tiles() {
        let Some(r) = room_of(t) else { continue };
        for side in Side::ALL {
            if map.edge(t, side) == Edge::Wall {
                // Any wall of the room: on the camera's side, near a doorway and looking out over
                // the room, before the wall beside a doorway that the cutaways drop.
                let to_door = doors[r as usize].iter().map(|d| (*d - t).abs().element_sum() as u32).min().unwrap_or(20).min(20);
                let score = if near_side(side) { 300 } else { 4 } + to_door + if blocked(t) { 1000 } else { 0 } + if recess(t, side) { 30 } else { 0 } + hidden(t, side) * 2;
                offer(r, score, spot(t, side, Vec2::ZERO));
            }
            if !way_in(t, side, r) {
                continue;
            }
            // The wall beside the doorway, inside this room: a tile along the wall from the
            // doorway's, nearest first, whose same side is a wall, toward the doorway.
            let perp = IVec2::new(side.offset().y, side.offset().x);
            for step in [perp, -perp] {
                for k in 1..=4 {
                    let n = t + step * k;
                    if !map.in_bounds(n) || map.tile(n).room != r || map.edge(n, side) != Edge::Wall {
                        break;
                    }
                    let score = k as u32 * 2 + if near_side(side) { 60 } else { 0 } + if blocked(n) { 1000 } else { 0 } + if recess(n, side) { 30 } else { 0 } + hidden(n, side) * 2;
                    offer(r, score, spot(n, side, -step.as_vec2() * SWITCH_TOWARD_DOOR));
                }
            }
        }
    }
    best.into_iter().enumerate().map(|(r, b)| if r == 0 { None } else { b.map(|(_, s)| s) }).collect()
}

/// What is saved of the power (the `POWR` section).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PowerSave {
    pub sources: Vec<Source>,
    pub grid_failed: bool,
    pub switches: Vec<u16>,
}

/// Does this room start with its light on? About `share` of the rooms, by a hash of the room,
/// never a hall, porch or garage (the hash the client lit windows by before there was power).
pub fn starts_lit(map: &TileMap, room: u16, share: f32) -> bool {
    if room == 0 {
        return false;
    }
    let Some(r) = map.rooms.get(room as usize) else { return false };
    !matches!(r.room_type.as_str(), "hall" | "porch" | "garage" | "outdoors") && hash01(room as i32, r.building as i32, 71) < share
}

/// The client's old room hash (`meshgen::hash01`), so the same rooms are lit as before.
fn hash01(x: i32, y: i32, salt: u32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x8da6_b343) ^ (y as u32).wrapping_mul(0xd816_3841) ^ salt.wrapping_mul(0xcb1a_b31f);
    h ^= h >> 13;
    h = h.wrapping_mul(0x5bd1_e995);
    h ^= h >> 15;
    (h & 0xffff) as f32 / 65536.0
}

impl Power {
    /// A new world's power: the grid as `def` says, and the lights left on.
    pub fn new(map: &TileMap, def: &PowerDef) -> Self {
        let switches = (1..map.rooms.len() as u16).filter(|&r| starts_lit(map, r, def.lights_on_at_start)).collect();
        let mut p = Self { sources: vec![Source { kind: SourceKind::Grid, covers: Coverage::Everywhere, on: def.grid.on }], grid_failed: false, switches, spots: switch_spots(map), ..Default::default() };
        p.refresh(map);
        p
    }

    /// Work out again which buildings have power and which rooms are lit. Returns whether the lit
    /// rooms changed.
    pub fn refresh(&mut self, map: &TileMap) -> bool {
        self.powered = (0..map.buildings.len()).map(|b| self.sources.iter().any(|s| s.on && s.covers.covers(map, b))).collect();
        let lit: Vec<u16> = self.switches.iter().copied().filter(|&r| map.rooms.get(r as usize).is_some_and(|room| self.powered.get(room.building as usize).copied().unwrap_or(false))).collect();
        let changed = lit != self.lit;
        self.lit = lit;
        changed
    }

    /// Does building `b` have power?
    pub fn powered(&self, b: usize) -> bool {
        self.powered.get(b).copied().unwrap_or(false)
    }

    /// The rooms lit now, in order.
    pub fn lit(&self) -> &[u16] {
        &self.lit
    }

    pub fn is_lit(&self, room: u16) -> bool {
        self.lit.binary_search(&room).is_ok()
    }

    /// The grid (the first source) is on.
    pub fn grid_on(&self) -> bool {
        self.sources.iter().find(|s| s.kind == SourceKind::Grid).is_some_and(|s| s.on)
    }

    pub fn to_save(&self) -> PowerSave {
        PowerSave { sources: self.sources.clone(), grid_failed: self.grid_failed, switches: self.switches.iter().copied().collect() }
    }

    /// Back from a save, checked against the map.
    pub fn from_save(save: PowerSave, map: &TileMap) -> Result<Self, String> {
        if let Some(&r) = save.switches.iter().find(|&&r| r == 0 || r as usize >= map.rooms.len()) {
            return Err(format!("a light switch in room {r}, which the map hasn't"));
        }
        for s in &save.sources {
            match &s.covers {
                Coverage::Buildings(list) if list.iter().any(|&b| b as usize >= map.buildings.len()) => return Err("a power source covers a building the map hasn't".into()),
                Coverage::Within { center, radius } if !(center.is_finite() && radius.is_finite() && *radius >= 0.0) => return Err("a power source covers an impossible area".into()),
                _ => {}
            }
        }
        let mut p = Self { sources: save.sources, grid_failed: save.grid_failed, switches: save.switches.into_iter().collect(), spots: switch_spots(map), ..Default::default() };
        p.refresh(map);
        Ok(p)
    }

    /// Where a room's switch is, if it has one.
    pub fn spot(&self, room: u16) -> Option<SwitchSpot> {
        self.spots.get(room as usize).copied().flatten()
    }

    /// Every room's switch, by room.
    pub fn spots(&self) -> &[Option<SwitchSpot>] {
        &self.spots
    }

    /// The map was replaced (`World::map_changed`): forget what it hasn't, and find the switches again.
    pub fn map_changed(&mut self, map: &TileMap) {
        let (rooms, buildings) = (map.rooms.len(), map.buildings.len());
        self.switches.retain(|&r| r != 0 && (r as usize) < rooms);
        for s in &mut self.sources {
            if let Coverage::Buildings(list) = &mut s.covers {
                list.retain(|&b| (b as usize) < buildings);
            }
        }
        self.spots = switch_spots(map);
        self.refresh(map);
    }

    /// Fold into the state hash.
    pub fn hash(&self, mix: &mut impl FnMut(u32)) {
        mix(0x504F_5752 ^ self.grid_failed as u32);
        for s in &self.sources {
            mix(s.kind as u32 | (s.on as u32) << 8);
        }
        mix(self.switches.len() as u32);
        for &r in &self.switches {
            mix(r as u32);
        }
    }
}

impl crate::world::World {
    /// The power: sources, switches, what has power and what is lit.
    pub fn power(&self) -> &Power {
        &self.power
    }

    /// Admin and tests: turn the grid on or off.
    pub fn set_grid(&mut self, on: bool) {
        for s in self.power.sources.iter_mut().filter(|s| s.kind == SourceKind::Grid) {
            s.on = on;
        }
        self.power.refresh(&self.map);
    }

    /// Add a power source (a generator or solar panels, later made by the player). Returns its index.
    pub fn add_power_source(&mut self, source: Source) -> usize {
        self.power.sources.push(source);
        self.power.refresh(&self.map);
        self.power.sources.len() - 1
    }

    /// Turn a source on or off.
    pub fn set_power_source(&mut self, index: usize, on: bool) -> bool {
        let Some(s) = self.power.sources.get_mut(index) else { return false };
        s.on = on;
        self.power.refresh(&self.map);
        true
    }

    /// Is the room at `pos` lit (switch on, power on)?
    pub fn lit_at(&self, pos: Vec2) -> bool {
        let room = self.map.room_at(pos);
        room != 0 && self.power.is_lit(room)
    }

    /// Flip the light switch of the room player `id` stands in, if it is within [`SWITCH_REACH`]
    /// (`on`: set it; `None`: the other way). With no power in the building it clicks and does
    /// nothing. Says how it went in the player's notes. Returns whether the switch moved.
    pub fn player_light(&mut self, id: u32, on: Option<bool>) -> bool {
        let Some(p) = self.players.iter().find(|p| p.id == id) else { return false };
        if p.dead || p.away.is_some() {
            return false;
        }
        let pos = p.state.pos;
        let room = self.map.room_at(pos);
        if !self.power.spot(room).is_some_and(|s| s.pos.distance(pos) <= SWITCH_REACH) {
            self.note(id, false, "no light switch in reach".into());
            return false;
        }
        let building = self.map.rooms[room as usize].building as usize;
        let name = self.map.rooms[room as usize].name.clone();
        // No power: the switch clicks and nothing changes (lights left on come back with it).
        if !self.power.powered(building) {
            self.note(id, false, "click. Nothing: no power".into());
            return false;
        }
        let was = self.power.switches.contains(&room);
        let want = on.unwrap_or(!was);
        if want == was {
            return false;
        }
        if want {
            self.power.switches.insert(room);
        } else {
            self.power.switches.remove(&room);
        }
        self.power.refresh(&self.map);
        let text = if want { format!("light on in the {name}") } else { format!("light off in the {name}") };
        self.note(id, true, text);
        true
    }

    /// Each tick: the grid fails when its time comes (`GridDef::fails_after_hours`).
    pub(crate) fn step_power(&mut self) {
        if self.power.grid_failed {
            return;
        }
        if let Some(h) = self.config.power.grid.fails_after_hours
            && self.game_hours() >= h as f64
        {
            self.power.grid_failed = true;
            self.set_grid(false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::{World, WorldConfig};

    fn world() -> World {
        World::new(WorldConfig::new(7, 0, crate::worldgen::tests::templates()))
    }

    #[test]
    fn the_shipped_file_parses_and_mistakes_are_named() {
        let def = parse_power(include_str!("../../../assets/data/power.ron")).unwrap();
        assert!(def.grid.on);
        assert!(parse_power("(lights_on_at_start: 2.0)").unwrap_err().contains("lights_on_at_start"));
        assert!(parse_power("(grid: (on: true, fails_after_hours: Some(-1.0)))").unwrap_err().contains("fails_after_hours"));
        assert!(parse_power("(colour: 1)").is_err(), "unknown fields refused");
        assert_eq!(parse_power("()").unwrap(), PowerDef::default());
    }

    #[test]
    fn some_rooms_start_lit_and_none_with_the_grid_off() {
        let w = world();
        let rooms = w.map.rooms.len() - 1;
        let lit = w.power().lit().len();
        assert!(lit > rooms / 10 && lit < rooms / 3, "{lit} of {rooms}");
        assert!(w.power().lit().iter().all(|&r| !matches!(w.map.rooms[r as usize].room_type.as_str(), "hall" | "porch" | "garage")));
        let mut config = WorldConfig::new(7, 0, crate::worldgen::tests::templates());
        config.power.grid.on = false;
        let dark = World::new(config);
        assert!(dark.power().lit().is_empty());
        assert_eq!(dark.power().switches, w.power().switches, "the same lights are left on, with nothing to light them");
    }

    #[test]
    fn coverage_reaches_what_it_says() {
        let w = world();
        let b = &w.map.buildings[0];
        let mid = b.min.as_vec2() + b.size.as_vec2() * 0.5;
        assert!(Coverage::Everywhere.covers(&w.map, 3));
        assert!(Coverage::Buildings(vec![0]).covers(&w.map, 0) && !Coverage::Buildings(vec![0]).covers(&w.map, 1));
        assert!(Coverage::Within { center: mid, radius: 1.0 }.covers(&w.map, 0));
        let far = Coverage::Within { center: mid + Vec2::splat(1000.0), radius: 5.0 };
        assert!(!far.covers(&w.map, 0));
    }

    #[test]
    fn a_generator_lights_its_own_building_with_the_grid_down() {
        let mut w = world();
        let room = *w.power().switches.iter().next().expect("a light on");
        let b = w.map.rooms[room as usize].building;
        w.set_grid(false);
        assert!(!w.power().is_lit(room));
        w.add_power_source(Source { kind: SourceKind::Generator, covers: Coverage::Buildings(vec![b]), on: true });
        assert!(w.power().is_lit(room) && w.power().powered(b as usize));
        assert!(w.power().lit().iter().all(|&r| w.map.rooms[r as usize].building == b), "only its own building");
    }

    #[test]
    fn a_switch_lights_the_room_only_with_power() {
        let mut w = world();
        let id = w.add_player(w.player_spawn);
        let room = w.map.room_at(w.player_spawn);
        assert_ne!(room, 0, "wakes up indoors");
        let spot = w.power().spot(room).expect("the bedroom has a switch");
        // Out of reach of it: nothing.
        let far = (w.map.buildings.iter().flat_map(|b| (b.min.y..b.min.y + b.size.y).flat_map(move |y| (b.min.x..b.min.x + b.size.x).map(move |x| IVec2::new(x, y)))))
            .map(|t| t.as_vec2() + Vec2::splat(0.5))
            .find(|p| w.map.room_at(*p) == room && p.distance(spot.pos) > SWITCH_REACH + 0.5);
        if let Some(far) = far {
            w.players[0].state.pos = far;
            assert!(!w.player_light(id, None));
            assert!(w.item_notes.last().unwrap().text.contains("in reach"));
        }
        w.players[0].state.pos = spot.pos + spot.normal * 0.4;
        assert_eq!(w.map.room_at(w.players[0].state.pos), room);
        w.player_light(id, Some(false));
        assert!(!w.power().is_lit(room));
        assert!(w.player_light(id, None), "flipped on");
        assert!(w.power().is_lit(room) && w.lit_at(w.players[0].state.pos));
        assert!(w.item_notes.last().unwrap().text.starts_with("light on"));
        assert!(!w.player_light(id, Some(true)), "already on");
        w.set_grid(false);
        assert!(!w.power().is_lit(room), "the grid down: dark");
        assert!(w.power().switches.contains(&room), "the switch stays on");
        assert!(!w.player_light(id, Some(false)), "no power: the switch does nothing");
        assert!(w.power().switches.contains(&room));
        assert!(w.item_notes.last().unwrap().text.contains("no power"));
        w.set_grid(true);
        assert!(w.power().is_lit(room), "power back, light back");
        // Outdoors there is no switch.
        let out = (0..w.map.width()).map(|x| Vec2::new(x as f32 + 0.5, 0.5)).find(|p| w.map.room_at(*p) == 0).unwrap();
        w.players[0].state.pos = out;
        assert!(!w.player_light(id, None));
    }

    #[test]
    fn every_room_has_a_switch_on_its_own_wall_by_a_door() {
        let w = world();
        let spots = w.power().spots();
        assert!(spots[0].is_none(), "outdoors has none");
        let (mut rooms, mut by_door, mut behind, mut far) = (0, 0, 0, 0);
        for b in &w.map.buildings {
            for y in b.min.y..b.min.y + b.size.y {
                for x in b.min.x..b.min.x + b.size.x {
                    let t = IVec2::new(x, y);
                    let r = w.map.tile(t).room;
                    if r == 0 || spots[r as usize].is_some_and(|s| s.tile != t) {
                        continue;
                    }
                    let Some(s) = spots[r as usize] else { continue };
                    rooms += 1;
                    assert_eq!(w.map.edge(s.tile, s.side), Edge::Wall, "room {r}: on a wall");
                    assert_eq!(w.map.room_at(s.pos), r, "room {r}: inside its own room");
                    assert_eq!(w.map.room_at(s.pos + s.normal * 0.3), r, "room {r}: faces into it");
                    let perp = IVec2::new(s.side.offset().y, s.side.offset().x);
                    if [perp, -perp].iter().any(|d| (1..=4).any(|k| matches!(w.map.edge(t + *d * k, s.side), Edge::DoorClosed | Edge::DoorOpen | Edge::DoorBroken | Edge::Arch))) {
                        by_door += 1;
                    }
                    if w.map.tile(t).feature == crate::map::Feature::Furniture {
                        behind += 1;
                    }
                    if matches!(s.side, Side::N | Side::W) {
                        far += 1;
                    }
                }
            }
        }
        let with_tiles = (1..w.map.rooms.len()).filter(|&r| spots[r].is_some()).count();
        assert_eq!(rooms, with_tiles);
        assert!(by_door * 10 >= rooms * 6, "{by_door} of {rooms} beside a doorway");
        assert!(far * 10 >= rooms * 9, "{far} of {rooms} on a wall facing the camera");
        assert!(behind * 20 <= rooms, "{behind} of {rooms} behind furniture");
        eprintln!("{rooms} switches, {by_door} by a doorway, {behind} behind furniture, {far} on north or west walls");
        assert_eq!(switch_spots(&w.map), spots, "the same from the map alone");
    }

    #[test]
    fn the_grid_fails_when_its_time_comes() {
        let mut config = WorldConfig::new(7, 0, crate::worldgen::tests::templates());
        config.power.grid.fails_after_hours = Some(2.0);
        let mut w = World::new(config);
        assert!(w.power().grid_on() && !w.power().lit().is_empty());
        w.skip_time(1.0);
        assert!(w.power().grid_on());
        w.skip_time(1.5);
        assert!(!w.power().grid_on() && w.power().lit().is_empty() && w.power().grid_failed);
        w.set_grid(true);
        w.step();
        assert!(w.power().grid_on(), "once failed, an admin can turn it back on");
    }

    #[test]
    fn a_lit_room_is_seen_at_night() {
        let t = crate::ZombieTuning::default();
        let cone = crate::senses::Cone::new(&t);
        let dark = crate::senses::Target { id: 1, pos: Vec2::ZERO, stance: crate::Stance::Walk, moving: true, ignored: false, sight_mult: 1.0, lit: 0.0 };
        let lit = crate::senses::Target { lit: 0.8, ..dark };
        let (a, b) = (crate::senses::sight_range(&t, cone, 0.0, &dark, 0.0, Vec2::X), crate::senses::sight_range(&t, cone, 0.0, &lit, 0.0, Vec2::X));
        assert!(b > a * 1.5, "{a} in the dark, {b} in a lit room");
        let day = crate::senses::sight_range(&t, cone, 1.0, &dark, 0.0, Vec2::X);
        assert_eq!(crate::senses::sight_range(&t, cone, 1.0, &lit, 0.0, Vec2::X), day, "by day it makes no difference");
    }

    #[test]
    fn switches_and_the_grid_save_and_load() {
        let mut w = world();
        let id = w.add_player(w.player_spawn);
        let room = w.map.room_at(w.player_spawn);
        let spot = w.power().spot(room).unwrap();
        w.players[0].state.pos = spot.pos + spot.normal * 0.4;
        w.player_light(id, Some(true));
        w.set_grid(false);
        let back = World::load(&w.save(), &w.config).unwrap();
        assert_eq!(back.state_hash(), w.state_hash());
        assert!(!back.power().grid_on() && back.power().switches.contains(&room) && back.power().lit().is_empty());
        assert_eq!(back.power().switches, w.power().switches);
    }
}
