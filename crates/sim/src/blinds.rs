//! Blinds on windows and glazed doors: pull one down and nobody sees in (or out).
//!
//! A blind hangs on the inside of a window (shut, open or broken: it hangs inside the frame) or of
//! a door with glass in it (a *glazed* door). Down, sight doesn't pass that glass
//! ([`TileMap::blocks_view`]): zombies outside don't see you through it, and you don't see out.
//! Sound is untouched. Some windows and glazed doors have one from the start
//! ([`place`], with the map); the rest can take one: a blind is an item (any item with the
//! [`BlindTuning::tag`] tag, the roller blind in `items.ron`), found in houses and hardware shops
//! and fitted from inside within reach ([`BlindOp::Fit`], a job of [`BlindTuning::fit_secs`]). A
//! fitted blind can be rolled up and down ([`BlindOp::Toggle`], at once, a little noise) or taken
//! down again into the bags ([`BlindOp::TakeDown`]).
//!
//! State: [`TileMap::blinds`] (saved in the `BLND` section, in `state_hash`, replicated as
//! [`BlindChange`]s); which doors are glazed is [`TileMap::glazed`], derived with the map.
//! Every number is in [`BlindTuning`] (`WorldConfig::blinds`, saved with the settings). Ours.

use glam::{IVec2, Vec2};
use serde::{Deserialize, Serialize};

use crate::inventory::JobKind;
use crate::map::{BlindChange, Edge, EdgeRef, OwnedEdge, TileMap};
use crate::rng::hash01;
use crate::senses::SoundSource;
use crate::template::BuildingKind;
use crate::world::{INTERACT_REACH, World};

/// Blind changes kept for replication if nobody drains them.
const MAX_BLIND_CHANGES: usize = 1024;

/// Blinds' numbers: where the town has them from the start, and how long handling one takes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BlindTuning {
    /// Chance a ground-floor window has a blind from the start, by the building's kind.
    pub house_window: f32,
    pub shop_window: f32,
    pub office_window: f32,
    pub civic_window: f32,
    /// Chance an outside door is glazed: a house's front door, its other outside doors, and the
    /// outside doors of shops, offices and landmarks.
    pub glazed_front_door: f32,
    pub glazed_back_door: f32,
    pub glazed_shop_door: f32,
    /// Chance a glazed door has a blind from the start.
    pub door_blind: f32,
    /// Chance a blind there from the start is down.
    pub down: f32,
    /// The item tag that makes an item a blind to fit.
    pub tag: String,
    /// Seconds to fit a blind, and to take one down.
    pub fit_secs: f32,
    pub take_down_secs: f32,
    /// How far rolling one up or down is heard (metres; 0 silent).
    pub noise: f32,
}

impl Default for BlindTuning {
    fn default() -> Self {
        Self {
            house_window: 0.45,
            shop_window: 0.1,
            office_window: 0.6,
            civic_window: 0.3,
            glazed_front_door: 0.25,
            glazed_back_door: 0.5,
            glazed_shop_door: 0.85,
            door_blind: 0.35,
            down: 0.25,
            tag: "blind".to_string(),
            fit_secs: 6.0,
            take_down_secs: 3.0,
            noise: 2.5,
        }
    }
}

impl BlindTuning {
    /// No blinds anywhere and no glazed doors (tests that want the map as it was).
    pub fn none() -> Self {
        Self { house_window: 0.0, shop_window: 0.0, office_window: 0.0, civic_window: 0.0, glazed_front_door: 0.0, glazed_back_door: 0.0, glazed_shop_door: 0.0, door_blind: 0.0, ..Self::default() }
    }

    /// Values a save may hold.
    pub fn check(&self) -> Result<(), String> {
        let chances = [self.house_window, self.shop_window, self.office_window, self.civic_window, self.glazed_front_door, self.glazed_back_door, self.glazed_shop_door, self.door_blind, self.down];
        if chances.iter().any(|c| !(0.0..=1.0).contains(c)) {
            return Err("blinds: a chance outside 0..1".to_string());
        }
        if [self.fit_secs, self.take_down_secs, self.noise].iter().any(|v| !v.is_finite() || !(0.0..=600.0).contains(v)) {
            return Err("blinds: a time or noise outside 0..600".to_string());
        }
        Ok(())
    }
}

/// What a player does with the blind it faces.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BlindOp {
    /// Roll it up or down (at once).
    Toggle,
    /// Fit a blind it carries to the window or glazed door it faces (a job).
    Fit,
    /// Take the blind down into its bags (a job).
    TakeDown,
    /// The key's likely thing: roll the blind it faces up or down, or with none there fit one.
    Use,
}

/// The tiles either side of an edge: (north or west, south or east).
pub fn sides(e: EdgeRef) -> (IVec2, IVec2) {
    match e.edge {
        OwnedEdge::North => (e.tile - IVec2::Y, e.tile),
        OwnedEdge::West => (e.tile - IVec2::X, e.tile),
    }
}

/// The tile on the inside of edge `e`, where its blind hangs: the indoor one (the owner tile if
/// both are indoors, or neither).
pub fn inside(map: &TileMap, e: EdgeRef) -> IVec2 {
    let (a, b) = sides(e);
    if map.tile(b).room == 0 && map.tile(a).room != 0 { a } else { b }
}

/// Is `pos` on the inside of edge `e`?
pub fn on_inside(map: &TileMap, e: EdgeRef, pos: Vec2) -> bool {
    let (a, _) = sides(e);
    let near = if e.far_tile(pos) == a { e.tile } else { a };
    near == inside(map, e)
}

/// The edge passing `want` that a body at `pos` facing `facing` reaches, by the same rule as a
/// door (`World::player_interact`): within [`INTERACT_REACH`] of the edge's middle, the one
/// nearest a point just ahead. Pure, so a client can ask it of its own map copy.
pub fn faced(map: &TileMap, pos: Vec2, facing: f32, want: impl Fn(EdgeRef) -> bool) -> Option<EdgeRef> {
    let aim = pos + Vec2::from_angle(facing) * crate::world::INTERACT_AIM_AHEAD;
    let t = pos.floor().as_ivec2();
    let mut best: Option<(f32, EdgeRef)> = None;
    for dy in -2..=2 {
        for dx in -2..=2 {
            for which in [OwnedEdge::North, OwnedEdge::West] {
                let e = EdgeRef::new(t + IVec2::new(dx, dy), which);
                if !map.in_bounds(e.tile) || !want(e) {
                    continue;
                }
                let d = aim.distance(e.mid());
                if pos.distance(e.mid()) <= INTERACT_REACH && best.is_none_or(|(bd, _)| d < bd) {
                    best = Some((d, e));
                }
            }
        }
    }
    best.map(|(_, e)| e)
}

/// The window or glazed door for a blind that a body at `pos` facing `facing` would handle, from
/// inside: with `has` one with a blind on it, else one that takes a blind and has none.
pub fn faced_for_blind(map: &TileMap, pos: Vec2, facing: f32, has: bool) -> Option<EdgeRef> {
    faced(map, pos, facing, |e| map.takes_blind(e) && map.blind(e).is_some() == has && on_inside(map, e, pos))
}

/// Place the glazed doors and the blinds the town starts with, by `t`, from a hash of the seed
/// and each edge (so in any order, and the same for the same map). Ground-floor windows and
/// outside doors of every building.
pub fn place(map: &mut TileMap, seed: u64, t: &BlindTuning) {
    map.glazed.clear();
    map.blinds.clear();
    let roll = |e: EdgeRef, salt: u64| hash01(seed ^ 0xB11D_5000 ^ salt, e.tile.x as u64, e.tile.y as u64, e.edge as u64);
    for b in 0..map.buildings.len() {
        let (min, size) = (map.buildings[b].min, map.buildings[b].size);
        let look = map.buildings[b].look;
        let kind = look.map_or(BuildingKind::House, |l| l.kind);
        for y in min.y..=min.y + size.y {
            for x in min.x..=min.x + size.x {
                for which in [OwnedEdge::North, OwnedEdge::West] {
                    let e = EdgeRef::new(IVec2::new(x, y), which);
                    let (sa, sb) = sides(e);
                    let (ra, rb) = (map.tile(sa).room, map.tile(sb).room);
                    let ours = |r: u16| r != 0 && map.rooms.get(r as usize).is_some_and(|room| room.building as usize == b);
                    if !(ours(ra) || ours(rb)) {
                        continue;
                    }
                    match map.owned_edge(e.tile, e.edge) {
                        Edge::Window => {
                            let chance = match kind {
                                BuildingKind::House => t.house_window,
                                BuildingKind::Shop => t.shop_window,
                                BuildingKind::Office => t.office_window,
                                BuildingKind::Civic => t.civic_window,
                            };
                            if roll(e, 1) < chance {
                                map.blinds.insert(e.key(), roll(e, 2) < t.down);
                            }
                        }
                        Edge::DoorClosed | Edge::DoorOpen if (ra == 0) != (rb == 0) => {
                            // Which way it opens onto the outside.
                            let out = match (which, ra == 0) {
                                (OwnedEdge::North, true) => crate::map::Side::N,
                                (OwnedEdge::North, false) => crate::map::Side::S,
                                (OwnedEdge::West, true) => crate::map::Side::W,
                                (OwnedEdge::West, false) => crate::map::Side::E,
                            };
                            let chance = match kind {
                                BuildingKind::House if look.is_some_and(|l| l.front == out) => t.glazed_front_door,
                                BuildingKind::House => t.glazed_back_door,
                                _ => t.glazed_shop_door,
                            };
                            if roll(e, 3) < chance {
                                map.glazed.insert(e.key());
                                if roll(e, 4) < t.door_blind {
                                    map.blinds.insert(e.key(), roll(e, 5) < t.down);
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }
}

impl World {
    /// Put up, roll or (`None`) take down the blind on `edge`, and send the change out.
    pub fn set_blind(&mut self, edge: EdgeRef, blind: Option<bool>) {
        if self.map.blind(edge) == blind {
            return;
        }
        self.map.set_blind(edge, blind);
        // Kept until a snapshot drains them; with nobody draining (an agent's world), the oldest go.
        if self.blind_changes.len() >= MAX_BLIND_CHANGES {
            self.blind_changes.remove(0);
        }
        self.blind_changes.push(BlindChange { tile: edge.tile, edge: edge.edge, blind });
    }

    /// Blinds that appeared, went or were rolled since the last call (whoever replicates the world
    /// drains this, like `edge_changes`).
    pub fn take_blind_changes(&mut self) -> Vec<BlindChange> {
        std::mem::take(&mut self.blind_changes)
    }

    /// Every blind as it is now, in edge order: what a joining client sets its map copy's to.
    pub fn blinds_now(&self) -> Vec<BlindChange> {
        self.map.blinds.iter().map(|(&key, &down)| {
            let e = crate::world::edge_of_key(key);
            BlindChange { tile: e.tile, edge: e.edge, blind: Some(down) }
        }).collect()
    }

    /// Every glazed door, in edge order (a joining network client's map copy needs them).
    pub fn glazed_doors(&self) -> Vec<EdgeRef> {
        self.map.glazed.iter().map(|&key| crate::world::edge_of_key(key)).collect()
    }

    /// Does player `pi` have a blind on it to fit (carried or in a hand)? Its item index.
    fn blind_carried(&self, pi: usize) -> Option<u16> {
        let tag = &self.config.blinds.tag;
        let inv = &self.players[pi].inventory;
        inv.carried.iter().chain(inv.hand.iter()).chain(inv.off_hand.iter()).map(|s| s.item).find(|&i| self.config.items.get(i as usize).is_some_and(|d| d.has_tag(tag)))
    }

    /// Player `id` rolls, fits or takes down the blind of the window or glazed door it faces from
    /// inside (see [`BlindOp`]). Refusals are noted for its client and returned.
    pub fn player_blind(&mut self, id: u32, op: BlindOp) -> Result<(), String> {
        let Some(pi) = self.players.iter().position(|p| p.id == id) else { return Err(format!("no player {id}")) };
        let p = &self.players[pi];
        if p.dead || p.needs.asleep() || p.knock.is_some() || p.away.is_some() || p.state.climb.is_some() {
            return Err("can't now".to_string());
        }
        if p.restraint.is_some() {
            return self.refuse(id, "your hands are tied".to_string());
        }
        let (pos, facing) = (p.state.pos, p.state.facing);
        let with = faced_for_blind(&self.map, pos, facing, true);
        let op = match op {
            BlindOp::Use if with.is_some() => BlindOp::Toggle,
            BlindOp::Use => BlindOp::Fit,
            op => op,
        };
        match op {
            BlindOp::Toggle | BlindOp::Use => {
                let Some(edge) = with else { return self.refuse(id, "no blind here: face a window or a glass door with one, from inside".to_string()) };
                let down = self.map.blind(edge) != Some(true);
                self.set_blind(edge, Some(down));
                if self.config.blinds.noise > 0.0 {
                    self.emit_sound_from(edge.mid(), self.config.blinds.noise, SoundSource::Player(id));
                }
                Ok(())
            }
            BlindOp::TakeDown => {
                let Some(edge) = with else { return self.refuse(id, "no blind here to take down".to_string()) };
                if self.players[pi].jobs.iter().any(|j| matches!(j.kind, JobKind::Blind { edge: e, fit: false } if e == edge)) {
                    return Ok(());
                }
                let secs = self.config.blinds.take_down_secs;
                self.queue_job(id, JobKind::Blind { edge, fit: false }, secs)
            }
            BlindOp::Fit => {
                let Some(edge) = faced_for_blind(&self.map, pos, facing, false) else {
                    let why = if with.is_some() { "there's a blind here already" } else { "face a window or a glass door from inside to fit a blind" };
                    return self.refuse(id, why.to_string());
                };
                if self.blind_carried(pi).is_none() {
                    return self.refuse(id, "you have no blind to fit: find one in a house or a hardware shop".to_string());
                }
                if self.players[pi].jobs.iter().any(|j| matches!(j.kind, JobKind::Blind { fit: true, .. })) {
                    return Ok(());
                }
                let secs = self.config.blinds.fit_secs;
                self.queue_job(id, JobKind::Blind { edge, fit: true }, secs)
            }
        }
    }

    /// A blind job is due: fit one to `edge`, or take its blind down, if that still can be.
    pub(crate) fn do_blind(&mut self, pi: usize, edge: EdgeRef, fit: bool) -> Result<String, String> {
        let pos = self.players[pi].state.pos;
        if !self.map.in_bounds(edge.tile) || !self.map.takes_blind(edge) {
            return Err("that window or door is gone".to_string());
        }
        if pos.distance(edge.mid()) > INTERACT_REACH + 0.3 || !on_inside(&self.map, edge, pos) {
            return Err("moved away from the window".to_string());
        }
        let what = if self.map.owned_edge(edge.tile, edge.edge).is_window() { "window" } else { "door" };
        if fit {
            if self.map.blind(edge).is_some() {
                return Err(format!("there's a blind on that {what} already"));
            }
            let Some(item) = self.blind_carried(pi) else { return Err("you have no blind to fit".to_string()) };
            crate::firstaid::take_one(&mut self.players[pi].inventory, item);
            self.refresh_carry(pi);
            self.set_blind(edge, Some(true));
            Ok(format!("fitted a blind to the {what}: it's down (Shift+F rolls it up)"))
        } else {
            if self.map.blind(edge).is_none() {
                return Err(format!("no blind on that {what}"));
            }
            self.set_blind(edge, None);
            let id = self.items_with_tag(&self.config.blinds.tag.clone());
            if let Some(i) = id {
                self.give_one(pi, i);
            }
            Ok(format!("took the blind down from the {what}"))
        }
    }

    /// The first item with `tag`, if any.
    fn items_with_tag(&self, tag: &str) -> Option<u16> {
        self.config.items.iter().position(|d| d.has_tag(tag)).map(|i| i as u16)
    }
}

#[cfg(test)]
mod tests;
