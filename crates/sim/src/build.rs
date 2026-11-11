//! Building: the build catalog (`assets/data/build.ron`) and the things a survivor builds with it.
//!
//! The catalog is data, a list of [`BuildDef`]s: what a build is called, which part of the build
//! menu it sits in ([`BuildCategory`]), what it's built on ([`BuildOn`]), the materials it uses
//! up and the tools it needs to hand (as a recipe's: [`crate::crafting::RecipeItem`],
//! [`crate::crafting::Tool`]), how long a piece takes, its hit points, how many pieces it stacks
//! up to, how loud it is, and how it comes down again ([`RemoveDef`]). The client's build menu
//! lists it; `Intent::Build` names a build by id.
//!
//! **What there is now: barricades.** A window (shut, open or broken) or a door (shut or broken)
//! boarded up with planks ([`BuildOn::Window`], [`BuildOn::Door`]). Each piece is one plank
//! nailed across from the side the survivor stands on, a job of the build's `secs` (faster with
//! Handiwork). The edge becomes [`Edge::WindowBoarded`] / [`Edge::DoorBoarded`]: it blocks
//! walking and sight, nobody opens or climbs it, and zombies batter it like a shut door
//! (`World::apply_thumps`), through the planks' hit points first: each plank's worth of damage
//! takes one down, and when the last goes the edge is what it was underneath again (a shut door
//! still to batter through, a window still to smash). A survivor with the remove tools prises a
//! plank off ([`RemoveDef`]: the plank back, the nails lost). What's on an edge is a
//! [`Barricade`] in [`TileMap::barricades`] (saved in the `BARR` section, in `state_hash` while
//! any, replicated as [`BarricadeChange`]s).
//!
//! **Later** (the build menu is made for it): walls, floors, doors and frames, furniture, house
//! upgrades. Each is a new [`BuildOn`] (what a build stands on: a tile, a wall edge, a room) and a
//! new [`BuildCategory`]; the materials, tools, time, hit points and stacking already come from the
//! data.

use glam::Vec2;
use serde::{Deserialize, Serialize};

use crate::crafting::{RecipeItem, Tool, amount, has_tool, have, tool_text};
use crate::inventory::Inventory;
use crate::inventory::JobKind;
use crate::items::ItemDef;
use crate::map::{Barricade, BarricadeChange, Edge, EdgeChange, EdgeRef, OwnedEdge, TileMap};
use crate::senses::SoundSource;
use crate::world::{INTERACT_REACH, World};
use crate::zombies::ZombieState;

/// Barricade changes kept for replication if nobody drains them.
const MAX_BARRICADE_CHANGES: usize = 1024;

/// Which part of the build menu a build is listed in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum BuildCategory {
    /// Boarding up windows and doors.
    Barricades,
}

impl BuildCategory {
    pub fn label(self) -> &'static str {
        match self {
            BuildCategory::Barricades => "Barricades",
        }
    }
}

/// What a build is put on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BuildOn {
    /// A ground-floor window, shut, open or broken.
    Window,
    /// A ground-floor door, shut or broken (an open one is shut first).
    Door,
}

impl BuildOn {
    pub fn label(self) -> &'static str {
        match self {
            BuildOn::Window => "window",
            BuildOn::Door => "door",
        }
    }

    /// Can a first piece go on an edge of this kind?
    pub fn takes(self, kind: Edge) -> bool {
        match self {
            BuildOn::Window => matches!(kind, Edge::Window | Edge::WindowOpen | Edge::WindowBroken),
            BuildOn::Door => matches!(kind, Edge::DoorClosed | Edge::DoorBroken),
        }
    }

    /// The edge it becomes with a piece on.
    pub fn boarded(self) -> Edge {
        match self {
            BuildOn::Window => Edge::WindowBoarded,
            BuildOn::Door => Edge::DoorBoarded,
        }
    }
}

/// How a build comes down again, a piece at a time.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoveDef {
    pub secs: f32,
    #[serde(default)]
    pub tools: Vec<Tool>,
    /// What each piece taken down gives back.
    #[serde(default)]
    pub returns: Vec<RecipeItem>,
    /// How far it's heard (metres).
    #[serde(default)]
    pub noise: f32,
}

/// One thing a survivor can build. Seconds of play; counts of whole items; ours.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildDef {
    pub id: String,
    /// What it's called in the build menu and in notes ("Board up a window").
    pub name: String,
    /// A line for the build menu.
    #[serde(default)]
    pub description: String,
    pub category: BuildCategory,
    pub on: BuildOn,
    /// Seconds a piece takes (Handiwork makes it faster, as crafting).
    pub secs: f32,
    /// Used up by each piece.
    pub inputs: Vec<RecipeItem>,
    /// Needed to hand, kept.
    #[serde(default)]
    pub tools: Vec<Tool>,
    /// Hit points of each piece, against zombies battering it.
    pub hp: f32,
    /// Pieces it stacks up to (planks on one window).
    pub max: u8,
    /// How far putting a piece up is heard (metres).
    #[serde(default)]
    pub noise: f32,
    /// How far a piece battered down is heard.
    #[serde(default)]
    pub break_noise: f32,
    /// Handiwork XP for each piece.
    #[serde(default)]
    pub xp: f32,
    pub remove: RemoveDef,
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

impl BuildDef {
    /// Check one build on its own: id, name, numbers, counts.
    pub fn validate(&self) -> Result<(), String> {
        let id = &self.id;
        if !valid_id(id) {
            return Err(format!("build id {id:?} must be lowercase letters, digits and _"));
        }
        let what = format!("build {id}");
        if self.name.trim().is_empty() {
            return Err(format!("{what}: name is empty"));
        }
        let secs_ok = |s: f32| s.is_finite() && s > 0.0 && s <= 600.0;
        if !secs_ok(self.secs) || !secs_ok(self.remove.secs) {
            return Err(format!("{what}: secs must be over 0 and at most 600"));
        }
        if !(self.hp.is_finite() && self.hp > 0.0 && self.hp <= 10_000.0) {
            return Err(format!("{what}: hp must be over 0 and at most 10000"));
        }
        if self.max == 0 || self.max > 16 {
            return Err(format!("{what}: max must be 1 to 16"));
        }
        for v in [self.noise, self.break_noise, self.remove.noise, self.xp] {
            if !(v.is_finite() && (0.0..=1000.0).contains(&v)) {
                return Err(format!("{what}: noise and xp must be 0 to 1000"));
            }
        }
        if self.inputs.is_empty() {
            return Err(format!("{what}: no inputs"));
        }
        for x in self.inputs.iter().chain(&self.remove.returns) {
            if x.count == 0 || x.count > 100 {
                return Err(format!("{what}: {} count must be 1 to 100", x.item));
            }
        }
        Ok(())
    }
}

/// Check every build and that ids are unique.
pub fn validate_builds(defs: &[BuildDef]) -> Result<(), String> {
    for (k, d) in defs.iter().enumerate() {
        d.validate()?;
        if defs[..k].iter().any(|e| e.id == d.id) {
            return Err(format!("build {} is listed twice", d.id));
        }
    }
    Ok(())
}

/// Check every build's items and tools against the items (`items.ron`): every item id there,
/// some item with each tool tag. Errors name the build.
pub fn check_builds(defs: &[BuildDef], items: &[ItemDef]) -> Result<(), String> {
    for d in defs {
        for x in d.inputs.iter().chain(&d.remove.returns) {
            if !items.iter().any(|i| i.id == x.item) {
                return Err(format!("build {}: no item {:?} in items.ron", d.id, x.item));
            }
        }
        for t in d.tools.iter().chain(&d.remove.tools) {
            let ok = match t {
                Tool::Item(id) => items.iter().any(|i| &i.id == id),
                Tool::Tag(tag) => items.iter().any(|i| i.has_tag(tag)),
            };
            if !ok {
                return Err(format!("build {}: no item for tool {t:?} in items.ron", d.id));
            }
        }
    }
    Ok(())
}

/// Has `inv` (carried and held, as crafting counts) `times` lots of `inputs` and every one of
/// `tools`? If not, everything missing, in words. The build menu offers only what passes; the
/// server checks again when a build is asked for and when it's done.
pub fn check_have(inputs: &[RecipeItem], tools: &[Tool], inv: &Inventory, items: &[ItemDef], times: u32) -> Result<(), String> {
    let mut missing = Vec::new();
    for x in inputs {
        let need = x.count as u32 * times.max(1);
        let got = items.iter().position(|d| d.id == x.item).map_or(0, |i| have(inv, i as u16));
        if got < need {
            missing.push(format!("needs {} (you have {got})", amount(items, &x.item, need.min(u16::MAX as u32) as u16)));
        }
    }
    for t in tools.iter().filter(|t| !has_tool(inv, t, items)) {
        missing.push(format!("needs {} to hand (kept)", tool_text(items, t)));
    }
    if missing.is_empty() { Ok(()) } else { Err(missing.join("; ")) }
}

/// Can `inv` put up a piece of `d` (`times` pieces)?
pub fn check_build(d: &BuildDef, inv: &Inventory, items: &[ItemDef], times: u32) -> Result<(), String> {
    check_have(&d.inputs, &d.tools, inv, items, times)
}

/// Can `inv` take a piece of `d` down (its remove tools)?
pub fn check_removal(d: &BuildDef, inv: &Inventory, items: &[ItemDef]) -> Result<(), String> {
    check_have(&[], &d.remove.tools, inv, items, 1)
}

/// Read a build catalog (a RON list of [`BuildDef`]) and check it on its own.
pub fn parse_builds(text: &str) -> Result<Vec<BuildDef>, String> {
    let defs: Vec<BuildDef> = ron::from_str(text).map_err(|e| e.to_string())?;
    validate_builds(&defs)?;
    Ok(defs)
}

/// Read a build catalog and check it against the items too.
pub fn parse_builds_for(text: &str, items: &[ItemDef]) -> Result<Vec<BuildDef>, String> {
    let defs = parse_builds(text)?;
    check_builds(&defs, items)?;
    Ok(defs)
}

/// The build in brief: "wooden plank + nail x4, with claw hammer (5 s a plank, 4 at most)".
pub fn describe(d: &BuildDef, items: &[ItemDef]) -> String {
    let list = d.inputs.iter().map(|x| amount(items, &x.item, x.count)).collect::<Vec<_>>().join(" + ");
    let tools = if d.tools.is_empty() { String::new() } else { format!(", with {}", d.tools.iter().map(|t| tool_text(items, t)).collect::<Vec<_>>().join(" and ")) };
    format!("{list}{tools} ({:.0} s a piece, {} at most)", d.secs, d.max)
}

/// Can build `d` go on edge `e` of `map` now: a first piece on the kind it's for, or one more on a
/// barricade of the same build that isn't full?
pub fn fits(map: &TileMap, d: &BuildDef, build: u16, e: EdgeRef) -> bool {
    let kind = map.owned_edge(e.tile, e.edge);
    if kind.is_boarded() {
        map.barricade(e).is_some_and(|b| b.build == build && b.planks < d.max)
    } else {
        d.on.takes(kind)
    }
}

/// The edge a body at `pos` facing `facing` would put build `d` on (the doors' reach rule,
/// [`crate::blinds::faced`]). Pure, so the client asks it of its own map copy.
pub fn faced_for_build(map: &TileMap, pos: Vec2, facing: f32, d: &BuildDef, build: u16) -> Option<EdgeRef> {
    crate::blinds::faced(map, pos, facing, |e| fits(map, d, build, e))
}

/// The boarded-up edge a body at `pos` facing `facing` would prise a plank off.
pub fn faced_barricade(map: &TileMap, pos: Vec2, facing: f32) -> Option<EdgeRef> {
    crate::blinds::faced(map, pos, facing, |e| map.barricade(e).is_some())
}

/// Is `pos` on the owner tile's side of edge `e` (south of a north edge, east of a west one)?
pub fn on_owner_side(e: EdgeRef, pos: Vec2) -> bool {
    match e.edge {
        OwnedEdge::North => pos.y >= e.tile.y as f32,
        OwnedEdge::West => pos.x >= e.tile.x as f32,
    }
}

impl World {
    /// A build's index in `config.builds`, by id.
    pub fn build_index(&self, id: &str) -> Option<u16> {
        self.config.builds.iter().position(|d| d.id == id).map(|i| i as u16)
    }

    /// Put up, change or (`None`) take away the barricade on `edge`, and send the change out.
    fn set_barricade(&mut self, edge: EdgeRef, b: Option<Barricade>) {
        match b {
            Some(b) => {
                self.map.barricades.insert(edge.key(), b);
            }
            None => {
                self.map.barricades.remove(&edge.key());
            }
        }
        if self.barricade_changes.len() >= MAX_BARRICADE_CHANGES {
            self.barricade_changes.remove(0);
        }
        self.barricade_changes.push(BarricadeChange { tile: edge.tile, edge: edge.edge, barricade: b });
        self.edge_hp_dirty.insert(edge.key());
    }

    /// Barricades that went up, changed or came down since the last call (whoever replicates the
    /// world drains this, like `edge_changes`).
    pub fn take_barricade_changes(&mut self) -> Vec<BarricadeChange> {
        std::mem::take(&mut self.barricade_changes)
    }

    /// Every barricade as it is now, in edge order: what a joining client sets its map copy's to.
    pub fn barricades_now(&self) -> Vec<BarricadeChange> {
        self.map
            .barricades
            .iter()
            .map(|(&key, &b)| {
                let e = crate::world::edge_of_key(key);
                BarricadeChange { tile: e.tile, edge: e.edge, barricade: Some(b) }
            })
            .collect()
    }

    /// Hit points left and full hit points of the planks on `edge` (each plank up is worth a
    /// build's `hp`), if it's boarded up.
    pub fn barricade_hp(&self, edge: EdgeRef) -> Option<(f32, f32)> {
        let b = self.map.barricade(edge)?;
        let per = self.config.builds.get(b.build as usize).map_or(1.0, |d| d.hp);
        Some((b.hp, per * b.planks as f32))
    }

    /// Take `damage` off the planks on `edge` (a zombie's thump, a test): each plank's worth takes
    /// a plank down, loudly; when the last goes the edge is what it was underneath again.
    /// Returns whether the barricade is gone.
    pub(crate) fn damage_barricade(&mut self, edge: EdgeRef, damage: f32, source: SoundSource) -> bool {
        let Some(mut b) = self.map.barricade(edge).copied() else { return false };
        let (per, noise) = self.config.builds.get(b.build as usize).map_or((1.0, 0.0), |d| (d.hp, d.break_noise));
        b.hp -= damage.max(0.0);
        if b.hp <= 0.0 {
            self.set_barricade(edge, None);
            self.set_edge(edge, b.under);
            self.break_events.push(EdgeChange { tile: edge.tile, edge: edge.edge, kind: b.under });
            if noise > 0.0 {
                self.emit_sound_from(edge.mid(), noise, source);
            }
            return true;
        }
        let planks = ((b.hp / per).ceil() as u8).clamp(1, b.planks);
        let lost = planks < b.planks;
        b.planks = planks;
        self.set_barricade(edge, Some(b));
        if lost && noise > 0.0 {
            self.emit_sound_from(edge.mid(), noise, source);
        }
        false
    }

    /// Is anyone (a zombie, another player) standing in the opening of `edge`, where a plank would
    /// shut them in the wall?
    fn opening_taken(&self, edge: EdgeRef, me: usize) -> bool {
        let (a, b) = TileMap::edge_segment(edge.tile, edge.edge);
        let near = |p: Vec2| {
            let ab = b - a;
            let t = ((p - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0);
            p.distance(a + ab * t) < 0.4
        };
        let z = &self.zombies;
        (0..z.id.len()).any(|i| z.state[i] != ZombieState::Dead && near(z.pos[i])) || self.players.iter().enumerate().any(|(k, p)| k != me && !p.dead && near(p.state.pos))
    }

    /// Player `id` puts a piece of build `build` (an index into `config.builds`) on the window or
    /// door it faces: a job of the build's time (with Handiwork), checked again when it's due.
    /// Asking again while one is queued queues another only if there's room on the edge and
    /// material for both (else nothing: a repeated click). Refusals are noted and returned.
    pub fn player_build(&mut self, id: u32, build: u16) -> Result<(), String> {
        let Some(pi) = self.players.iter().position(|p| p.id == id) else { return Err(format!("no player {id}")) };
        let Some(d) = self.config.builds.get(build as usize).cloned() else { return self.refuse(id, format!("no build {build}")) };
        let p = &self.players[pi];
        if p.dead || p.needs.asleep() || p.knock.is_some() || p.away.is_some() || p.state.climb.is_some() {
            return Err("can't now".to_string());
        }
        if p.state.off_ground(&self.map) {
            return self.refuse(id, format!("only the ground floor's {}s can be boarded up for now", d.on.label()));
        }
        if p.restraint.is_some() {
            return self.refuse(id, "your hands are tied".to_string());
        }
        let (pos, facing) = (p.state.pos, p.state.facing);
        let Some(edge) = faced_for_build(&self.map, pos, facing, &d, build) else {
            let full = faced_barricade(&self.map, pos, facing).and_then(|e| self.map.barricade(e).copied());
            let why = match full {
                Some(b) if b.build == build => format!("that {} is boarded up as far as it goes ({} planks)", d.on.label(), d.max),
                Some(_) => format!("that {} is boarded up another way", d.on.label()),
                None if d.on == BuildOn::Door && faced_open_door(&self.map, pos, facing) => "shut the door first".to_string(),
                None => format!("face a {} within reach to {}", d.on.label(), d.name.to_lowercase()),
            };
            return self.refuse(id, why);
        };
        let queued = self.players[pi].jobs.iter().filter(|j| matches!(j.kind, JobKind::Build { edge: e, build: b, add: true } if e == edge && b == build)).count() as u32;
        let up = self.map.barricade(edge).map_or(0, |b| b.planks as u32);
        if queued > 0 && up + queued >= d.max as u32 {
            return Ok(());
        }
        if let Err(why) = check_build(&d, &self.players[pi].inventory, &self.config.items, queued + 1) {
            if queued > 0 {
                return Ok(());
            }
            return self.refuse(id, format!("can't {}: {why}", d.name.to_lowercase()));
        }
        let secs = self.handiwork_secs(pi, d.secs);
        self.queue_job(id, JobKind::Build { edge, build, add: true }, secs)
    }

    /// [`World::player_build`] by the build's id (as `Intent::Build` names it).
    pub fn player_build_id(&mut self, id: u32, build: &str) -> Result<(), String> {
        match self.build_index(build) {
            Some(b) => self.player_build(id, b),
            None => self.refuse(id, format!("there's no build {build:?} in this game")),
        }
    }

    /// Player `id` prises a plank off the boarded-up window or door it faces (the build's
    /// [`RemoveDef`]): a job, checked again when it's due.
    pub fn player_unbuild(&mut self, id: u32) -> Result<(), String> {
        let Some(pi) = self.players.iter().position(|p| p.id == id) else { return Err(format!("no player {id}")) };
        let p = &self.players[pi];
        if p.dead || p.needs.asleep() || p.knock.is_some() || p.away.is_some() || p.state.climb.is_some() || p.state.off_ground(&self.map) {
            return Err("can't now".to_string());
        }
        if p.restraint.is_some() {
            return self.refuse(id, "your hands are tied".to_string());
        }
        let Some(edge) = faced_barricade(&self.map, p.state.pos, p.state.facing) else { return self.refuse(id, "nothing boarded up here: face a boarded window or door".to_string()) };
        let b = *self.map.barricade(edge).expect("faced");
        let Some(d) = self.config.builds.get(b.build as usize).cloned() else { return self.refuse(id, "those boards can't be taken down".to_string()) };
        let queued = self.players[pi].jobs.iter().filter(|j| matches!(j.kind, JobKind::Build { edge: e, add: false, .. } if e == edge)).count() as u32;
        if queued > 0 && queued >= b.planks as u32 {
            return Ok(());
        }
        if let Err(why) = check_removal(&d, &self.players[pi].inventory, &self.config.items) {
            return self.refuse(id, format!("can't take a plank down: {why}"));
        }
        let secs = self.handiwork_secs(pi, d.remove.secs);
        self.queue_job(id, JobKind::Build { edge, build: b.build, add: false }, secs)
    }

    /// A build job is due: a piece onto `edge` (`add`) or one off it, if that still can be.
    pub(crate) fn do_build(&mut self, pi: usize, edge: EdgeRef, build: u16, add: bool) -> Result<String, String> {
        let d = self.config.builds.get(build as usize).cloned().ok_or("that build is gone")?;
        let pos = self.players[pi].state.pos;
        let what = d.on.label();
        if !self.map.in_bounds(edge.tile) || pos.distance(edge.mid()) > INTERACT_REACH + 0.3 {
            return Err(format!("moved away from the {what}"));
        }
        let id = self.players[pi].id;
        if add {
            if !fits(&self.map, &d, build, edge) {
                return Err(format!("can't {} there any more", d.name.to_lowercase()));
            }
            let first = self.map.barricade(edge).is_none();
            if first && self.opening_taken(edge, pi) {
                return Err(format!("someone's in the way of the {what}"));
            }
            let mut inv = self.players[pi].inventory.clone();
            check_build(&d, &inv, &self.config.items, 1).map_err(|why| format!("can't {}: {why}", d.name.to_lowercase()))?;
            for x in &d.inputs {
                let i = self.item_index(&x.item).ok_or_else(|| format!("no item {}", x.item))?;
                if !crate::crafting::take_whole(&mut inv, i, x.count) {
                    return Err(format!("can't {}: the {} is gone", d.name.to_lowercase(), amount(&self.config.items, &x.item, 1)));
                }
            }
            self.players[pi].inventory = inv;
            self.refresh_carry(pi);
            let b = match self.map.barricade(edge).copied() {
                Some(mut b) => {
                    b.planks += 1;
                    b.hp += d.hp;
                    b
                }
                None => {
                    let under = self.map.owned_edge(edge.tile, edge.edge);
                    Barricade { build, planks: 1, hp: d.hp, under, owner_side: on_owner_side(edge, pos) }
                }
            };
            self.set_barricade(edge, Some(b));
            if first {
                self.set_edge(edge, d.on.boarded());
            }
            if d.noise > 0.0 {
                self.emit_sound_from(edge.mid(), d.noise, SoundSource::Player(id));
            }
            self.award_xp(pi, crate::skills::Skill::Handiwork, d.xp);
            let more = if b.planks >= d.max { " (as far as it goes)".to_string() } else { String::new() };
            Ok(format!("nailed a plank across the {what}: {} of {}{more}", b.planks, d.max))
        } else {
            let Some(mut b) = self.map.barricade(edge).copied() else { return Err(format!("no planks on that {what}")) };
            check_removal(&d, &self.players[pi].inventory, &self.config.items).map_err(|why| format!("can't take a plank down: {why}"))?;
            b.planks -= 1;
            if d.remove.noise > 0.0 {
                self.emit_sound_from(edge.mid(), d.remove.noise, SoundSource::Player(id));
            }
            for x in &d.remove.returns {
                if let Some(i) = self.item_index(&x.item) {
                    for _ in 0..x.count {
                        self.give_one(pi, i);
                    }
                }
            }
            self.refresh_carry(pi);
            let back = d.remove.returns.iter().map(|x| amount(&self.config.items, &x.item, x.count)).collect::<Vec<_>>().join(" and ");
            let back = if back.is_empty() { String::new() } else { format!(", got {back} back") };
            if b.planks == 0 {
                self.set_barricade(edge, None);
                self.set_edge(edge, b.under);
                Ok(format!("took the last plank off the {what}{back}"))
            } else {
                b.hp = b.hp.min(d.hp * b.planks as f32);
                self.set_barricade(edge, Some(b));
                Ok(format!("took a plank off the {what}{back}: {} left", b.planks))
            }
        }
    }
}

/// Does a body at `pos` facing `facing` face an open door (to say "shut it first")?
fn faced_open_door(map: &TileMap, pos: Vec2, facing: f32) -> bool {
    crate::blinds::faced(map, pos, facing, |e| map.owned_edge(e.tile, e.edge) == Edge::DoorOpen).is_some()
}

#[cfg(test)]
mod tests;
