//! Guns (#72, PLAN §4.8): a weapon with `ranged` numbers in `weapons.ron` shoots instead of
//! swinging. Loud, and the reason to carry one is another survivor or a last resort.
//!
//! **A gun is a weapon like any other** (`WeaponCategory::Firearm`): the attack intent pulls the
//! trigger (`ActionKind::Swing`), `windup_seconds` later the shot goes and `swing_seconds` after
//! the pull the next can. Of the weapon's own numbers a shot uses `damage` (each pellet's),
//! `max_targets` (bodies one bullet goes through, each after the first taking
//! `multi_hit_falloff` less), `crit_multiplier` (a hit to the head), `knockback`,
//! `stagger_chance`, `knockdown_chance`, `endurance_cost` (the recoil) and `noise_radius`; the
//! rest is [`RangedDef`]. `range`, `arc_degrees` and `crit_chance` aren't read.
//!
//! **Where a shot goes:** from the shooter, 1.4 m up, along its facing and its **pitch**
//! (`Player::pitch`, `World::set_pitch`: radians up from level, the client's word like the facing;
//! the isometric client works it out from what the cursor is on, a first-person one would send its
//! camera's), each pellet off by up to the gun's spread either way, wider on the move and when
//! tired. The server follows it over the tile map ([`flight`]): walls and shut doors stop it; a
//! window lets it through between sill and head (a shut one's glass breaks), a doorway under its
//! head, a fence stops what flies under a metre, a tree trunk stops it, and so does the ground.
//! Furniture and street props don't. Bodies are upright cylinders 1.8 m tall (0.4 m lying down):
//! the first one the line passes through is hit, at the height it was hit at: from 1.5 m up it is
//! the head. Zombies are struck as by a blow (`World::strike`); with PvP on other players are in
//! the same list (`World::strike_player`: the wound on the part at that height, a gash that
//! clothing doesn't stop, damage times `CombatTuning::pvp_shot_mult`).
//!
//! **Lag compensation (#23):** as for a blow, bodies are tested where the shooter's client showed
//! them (`World::rewind_tick`, at most `lag_comp_max_secs` back); the map is as it is now.
//!
//! **Ammunition is items.** A gun holds up to `capacity` rounds of its `ammo` item; how many is
//! on its stack ([`ItemStack::used`], which for a gun or a magazine counts the rounds in it, not
//! goes had). A gun with a `magazine` item is reloaded by swapping in the fullest one carried
//! (`reload_seconds`; the one taken out goes back in the pocket with what it still held); with
//! none to swap, or no magazine at all, loose rounds go in one at a time (`round_seconds` each)
//! until it is full, they run out or the queue is dropped (shooting does that). Using a carried
//! magazine fills it the same way. All of it is the item job [`JobKind::Reload`], asked for with
//! the ordinary use of the gun in hand (`World::player_use`, so `Intent::Use`), or by pulling the
//! trigger on an empty gun.
//!
//! **Noise:** a shot is a player's sound of the weapon's `noise_radius` (up to
//! [`MAX_SHOT_NOISE`]), heard that tick like a swing: every zombie in earshot comes, sleepers wake,
//! and being over the migration's `min_radius` it leaves an attractor that draws the far horde in
//! over the following minutes (`crate::population`).
//!
//! Deterministic: spread comes from the combat RNG, found guns' loads from a hash of the seed.

use glam::{IVec2, Vec2};
use serde::{Deserialize, Serialize};

use crate::ai::ZOMBIE_RADIUS;
use crate::combat::{ActionEvent, CombatEvent, CombatEventKind, PLAYER_TARGET, Strike};
use crate::inventory::{JobKind, Place, Slot};
use crate::items::{ItemDef, ItemStack, WeaponDef};
use crate::lagcomp::Frame;
use crate::map::{Edge, EdgeRef, Feature, Side, TREE_RADIUS, TileMap};
use crate::players::{Action, PLAYER_RADIUS};
use crate::rng::hash01;
use crate::senses::{Sound, SoundSource};
use crate::world::World;
use crate::zombies::ZombieState;
use crate::TICK_DT;

/// The loudest a gun may be (m).
pub const MAX_SHOT_NOISE: f32 = 300.0;
/// How high the muzzle is held.
pub const MUZZLE_HEIGHT: f32 = 1.4;
/// How tall a standing body is, where its head starts, and how tall one lying down is.
pub const BODY_HEIGHT: f32 = 1.8;
pub const HEAD_FROM: f32 = 1.5;
pub const DOWN_HEIGHT: f32 = 0.4;
/// A window's opening (docs/asset-pipeline.md), a doorway's head and a fence's top.
pub const SILL_HEIGHT: f32 = 0.9;
pub const HEAD_HEIGHT: f32 = 2.1;
pub const FENCE_HEIGHT: f32 = 1.0;
/// The steepest aim, up or down (radians).
pub const MAX_PITCH: f32 = 1.3;
const FOUND_SALT: u64 = 0x6E5_0A44_0;
/// The tag a gun's magazine item carries, so a client's menu can offer to fill it without the
/// weapon table (`RangedDef::check_items` holds the items to it).
pub const TAG_MAGAZINE: &str = "magazine";

/// What makes a weapon a gun. Distances in metres, times in seconds, angles in degrees. Ours.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RangedDef {
    /// The item one round of it is (an id in `items.ron`).
    pub ammo: String,
    /// Rounds it holds.
    pub capacity: u8,
    /// The item its detachable magazine is, if it has one: a carried one is swapped in whole.
    #[serde(default)]
    pub magazine: Option<String>,
    /// Swapping a magazine.
    #[serde(default = "one")]
    pub reload_seconds: f32,
    /// Pushing one loose round in.
    pub round_seconds: f32,
    /// How far a bullet flies.
    pub range: f32,
    /// Each pellet goes up to this far off the aim, sideways and up or down, standing still.
    pub spread_degrees: f32,
    /// Added to it while moving.
    #[serde(default)]
    pub move_spread_degrees: f32,
    /// Pellets a shot (a shotgun's; one round either way).
    #[serde(default = "one_pellet")]
    pub pellets: u8,
    /// The share of its damage a pellet still does at the end of its range (falling off evenly).
    #[serde(default = "one")]
    pub far_damage: f32,
}

fn one() -> f32 {
    1.0
}

fn one_pellet() -> u8 {
    1
}

impl RangedDef {
    /// Check the numbers (`id`: the weapon's, for the error).
    pub fn validate(&self, id: &str) -> Result<(), String> {
        let item_id = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
        if !item_id(&self.ammo) {
            return Err(format!("{id}: ranged ammo {:?} must be an item id", self.ammo));
        }
        if self.magazine.as_deref().is_some_and(|m| !item_id(m) || m == self.ammo) {
            return Err(format!("{id}: ranged magazine must be an item id other than the ammo's"));
        }
        if self.capacity == 0 || self.capacity > 100 {
            return Err(format!("{id}: ranged capacity must be 1..100, got {}", self.capacity));
        }
        if !(self.reload_seconds >= 0.1 && self.reload_seconds <= 30.0 && self.round_seconds >= 0.1 && self.round_seconds <= 30.0) {
            return Err(format!("{id}: ranged reload_seconds and round_seconds must be in [0.1, 30], got {} and {}", self.reload_seconds, self.round_seconds));
        }
        if !(self.range >= 1.0 && self.range <= 300.0) {
            return Err(format!("{id}: ranged range must be in [1, 300] m, got {}", self.range));
        }
        if !(self.spread_degrees >= 0.0 && self.move_spread_degrees >= 0.0 && self.spread_degrees + self.move_spread_degrees <= 30.0) {
            return Err(format!("{id}: ranged spread_degrees and move_spread_degrees must be 0 or more and at most 30 together"));
        }
        if self.pellets == 0 || self.pellets > 16 {
            return Err(format!("{id}: ranged pellets must be 1..16, got {}", self.pellets));
        }
        if !(0.0..=1.0).contains(&self.far_damage) {
            return Err(format!("{id}: ranged far_damage must be in 0..1, got {}", self.far_damage));
        }
        Ok(())
    }

    /// The gun's item `gun` among `items`: its rounds and its magazine must be items too, and a
    /// gun and a magazine don't stack (each has its own rounds in it).
    pub fn check_items(&self, gun: &str, items: &[ItemDef]) -> Result<(), String> {
        if !items.iter().any(|d| d.id == self.ammo) {
            return Err(format!("{gun}: its ammo {:?} isn't in the items", self.ammo));
        }
        if let Some(m) = &self.magazine {
            match items.iter().find(|d| &d.id == m) {
                None => return Err(format!("{gun}: its magazine {m:?} isn't in the items")),
                Some(d) if d.stack != 1 => return Err(format!("{gun}: its magazine {m:?} must not stack (stack: 1)")),
                Some(d) if d.weapon.is_some() || d.consumable().is_some() => return Err(format!("{gun}: its magazine {m:?} can't be a weapon, food or drink")),
                Some(d) if !d.has_tag(TAG_MAGAZINE) => return Err(format!("{gun}: its magazine {m:?} needs the tag {TAG_MAGAZINE:?}")),
                _ => {}
            }
        }
        Ok(())
    }
}

/// What an item is loaded with: a gun, or a gun's magazine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Holds {
    /// The item one round is.
    pub ammo: u16,
    /// Rounds it takes; how many are in a stack of it is [`ItemStack::used`].
    pub capacity: u8,
    /// A magazine (not the gun itself).
    pub magazine: bool,
}

/// What `item` is loaded with, if it is a gun or a magazine whose rounds are among `items`.
pub fn holds(items: &[ItemDef], weapons: &[WeaponDef], item: u16) -> Option<Holds> {
    let def = items.get(item as usize)?;
    let ammo = |r: &RangedDef| items.iter().position(|d| d.id == r.ammo).map(|i| i as u16);
    if let Some(w) = &def.weapon {
        let r = weapons.iter().find(|x| &x.id == w)?.ranged.as_ref()?;
        return Some(Holds { ammo: ammo(r)?, capacity: r.capacity, magazine: false });
    }
    let r = weapons.iter().filter_map(|w| w.ranged.as_ref()).find(|r| r.magazine.as_deref() == Some(def.id.as_str()))?;
    Some(Holds { ammo: ammo(r)?, capacity: r.capacity, magazine: true })
}

/// The most [`ItemStack::used`] can be for `item`: a gun's or magazine's capacity, one under
/// the knocks a weapon that wears takes ([`crate::wear`]: all of them, broken), else one under the goes a thing to
/// eat or drink has.
pub fn max_used(items: &[ItemDef], weapons: &[WeaponDef], item: u16) -> u8 {
    match holds(items, weapons, item) {
        Some(h) => h.capacity,
        None if crate::wear::durability(items, weapons, item) > 0 => crate::wear::durability(items, weapons, item),
        None => items.get(item as usize).and_then(|d| d.consumable()).map_or(1, |c| c.uses.max(1)) - 1,
    }
}

/// How a shot's line ends on the map, and the shut windows it went through on the way.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Flight {
    /// Metres along the ground to where it stopped (or ran out).
    pub end: f32,
    glass: [(f32, EdgeRef); 4],
    panes: usize,
}

impl Flight {
    /// The shut windows it passed, nearest first, with how far along each was (at most 4).
    pub fn glass(&self) -> &[(f32, EdgeRef)] {
        &self.glass[..self.panes]
    }
}

/// Follow a shot from `origin` along unit `dir` over the ground, starting [`MUZZLE_HEIGHT`] up
/// and gaining `slope` metres a metre, for at most `max` metres along the ground: what on the
/// tile map stops it (see the module docs), and the glass it breaks before that.
pub fn flight(map: &TileMap, origin: Vec2, dir: Vec2, max: f32, slope: f32) -> Flight {
    let mut out = Flight::default();
    let height = |t: f32| MUZZLE_HEIGHT + t * slope;
    // The ground (or, steeply up, nothing within reach of anything): where the line leaves the
    // heights anything stands at.
    let mut max = max;
    if slope < -1e-6 {
        max = max.min(MUZZLE_HEIGHT / -slope);
    }
    // A trunk in a tile: where the line meets it, if it does.
    let trunk = |tile: IVec2| -> Option<f32> {
        if !matches!(map.tile(tile).feature, Feature::Tree | Feature::PineTree) {
            return None;
        }
        let rel = tile.as_vec2() + Vec2::splat(0.5) - origin;
        let along = rel.dot(dir);
        let off2 = rel.length_squared() - along * along;
        let t = along - (TREE_RADIUS * TREE_RADIUS - off2).max(0.0).sqrt();
        (off2 < TREE_RADIUS * TREE_RADIUS && t >= 0.0).then_some(t)
    };
    let mut tile = origin.floor().as_ivec2();
    if !map.in_bounds(tile) {
        return out;
    }
    let step = IVec2::new(if dir.x >= 0.0 { 1 } else { -1 }, if dir.y >= 0.0 { 1 } else { -1 });
    let next_boundary = |o: f32, t: i32, s: i32| if s > 0 { (t + 1) as f32 - o } else { o - t as f32 };
    let inv = Vec2::new(if dir.x.abs() > 1e-9 { 1.0 / dir.x.abs() } else { f32::INFINITY }, if dir.y.abs() > 1e-9 { 1.0 / dir.y.abs() } else { f32::INFINITY });
    let mut t_max = Vec2::new(next_boundary(origin.x, tile.x, step.x) * inv.x, next_boundary(origin.y, tile.y, step.y) * inv.y);
    loop {
        let (t, side) = if t_max.x < t_max.y { (t_max.x, if step.x > 0 { Side::E } else { Side::W }) } else { (t_max.y, if step.y > 0 { Side::S } else { Side::N }) };
        // Anything standing in this tile comes before its far edge.
        if let Some(hit) = trunk(tile)
            && hit <= t.min(max)
        {
            out.end = hit;
            return out;
        }
        if t >= max {
            out.end = max;
            return out;
        }
        let h = height(t);
        let edge = map.edge(tile, side);
        let through = match edge {
            Edge::None => true,
            Edge::Wall | Edge::DoorClosed => false,
            Edge::Window | Edge::WindowOpen | Edge::WindowBroken => (SILL_HEIGHT..=HEAD_HEIGHT).contains(&h),
            Edge::DoorOpen | Edge::DoorBroken | Edge::Arch => h <= HEAD_HEIGHT,
            Edge::Fence => h > FENCE_HEIGHT,
        };
        if !through || !map.in_bounds(tile + side.offset()) {
            out.end = t;
            return out;
        }
        if edge == Edge::Window && out.panes < out.glass.len() {
            out.glass[out.panes] = (t, EdgeRef::of(tile, side));
            out.panes += 1;
        }
        tile += side.offset();
        match side {
            Side::E | Side::W => t_max.x += inv.x,
            _ => t_max.y += inv.y,
        }
    }
}

/// Where along a shot's line (metres over the ground from `origin`, at most `end`) it enters an
/// upright cylinder of `radius` and height `top` standing at `centre`, if it passes through it.
fn through(origin: Vec2, dir: Vec2, slope: f32, end: f32, centre: Vec2, radius: f32, top: f32) -> Option<f32> {
    let rel = centre - origin;
    let along = rel.dot(dir);
    let off2 = rel.length_squared() - along * along;
    if off2 >= radius * radius {
        return None;
    }
    let half = (radius * radius - off2).sqrt();
    let (a, b) = ((along - half).max(0.0), (along + half).min(end));
    if a > b {
        return None;
    }
    // Its height while over the body: some of that must be between the feet and the top.
    let (ha, hb) = (MUZZLE_HEIGHT + a * slope, MUZZLE_HEIGHT + b * slope);
    (ha.min(hb) <= top && ha.max(hb) >= 0.0).then_some(a)
}

/// The body part at `height` on a standing player (`roll` in 0..1 picks a side, and between the
/// torso and an arm).
fn part_at(height: f32, roll: f32) -> crate::health::Part {
    use crate::health::Part::*;
    let left = roll < 0.5;
    let side = (roll * 2.0).fract();
    if height >= HEAD_FROM {
        Head
    } else if height >= 0.95 {
        match (side < 0.7, left) {
            (true, _) => Torso,
            (false, true) => LeftArm,
            (false, false) => RightArm,
        }
    } else if height >= 0.8 && side >= 0.7 {
        if left { LeftHand } else { RightHand }
    } else if height >= 0.12 {
        if left { LeftLeg } else { RightLeg }
    } else if left {
        LeftFoot
    } else {
        RightFoot
    }
}

impl World {
    /// Player `id` aims `pitch` radians up (+) or down (-) of level, at most [`MAX_PITCH`]. With
    /// its facing that is the way its shots go.
    pub fn set_pitch(&mut self, id: u32, pitch: f32) {
        if let Some(p) = self.players.iter_mut().find(|p| p.id == id) {
            p.pitch = if pitch.is_finite() { pitch.clamp(-MAX_PITCH, MAX_PITCH) } else { 0.0 };
        }
    }

    /// What item `item` is loaded with, if it is a gun or a magazine ([`holds`]).
    pub fn holds(&self, item: u16) -> Option<Holds> {
        holds(&self.config.items, &self.config.weapons, item)
    }

    /// The gun in player `id`'s hand: (rounds in it, rounds it takes).
    pub fn loaded(&self, id: u32) -> Option<(u8, u8)> {
        let gun = self.player(id)?.inventory.hand?;
        let h = self.holds(gun.item).filter(|h| !h.magazine)?;
        Some((gun.used, h.capacity))
    }

    /// Does player `pi` hold a gun with nothing in it?
    pub(crate) fn gun_is_empty(&self, pi: usize) -> bool {
        let p = &self.players[pi];
        p.weapon.is_some_and(|w| self.config.weapons[w as usize].ranged.is_some()) && p.inventory.hand.is_some_and(|g| g.used == 0)
    }

    /// The shot of action `a` goes, from `origin` facing `facing` (see the module docs): one
    /// round, its pellets, the noise. With nothing in the gun (emptied, or put away, since the
    /// trigger was pulled) nothing happens.
    pub(crate) fn fire(&mut self, pi: usize, a: Action, origin: Vec2, facing: f32, ahead: u64) {
        let w = &self.config.weapons[a.weapon as usize];
        let Some(r) = &w.ranged else { return };
        let (range, spread, move_spread, pellets, far, noise) = (r.range, r.spread_degrees, r.move_spread_degrees, r.pellets, r.far_damage, w.noise_radius);
        let s = Strike {
            range,
            half_arc: 0.0,
            max_targets: w.max_targets,
            damage: w.damage,
            crit_chance: 0.0,
            crit_multiplier: w.crit_multiplier,
            knockback: w.knockback,
            stagger_chance: w.stagger_chance,
            knockdown_chance: w.knockdown_chance,
            noise,
            shot: None,
        };
        let player = self.players[pi].id;
        let held = self.players[pi].inventory.hand.is_some_and(|g| g.used > 0 && self.item_weapon(g.item) == Some(a.weapon));
        if !held {
            self.action_events.push(ActionEvent { player, kind: a.kind, weapon: a.weapon, hits: 0 });
            return;
        }
        let p = &mut self.players[pi];
        if let Some(gun) = &mut p.inventory.hand {
            gun.used -= 1;
        }
        let moving = p.input.move_dir.length_squared() > 1e-4 && p.state.mods.speed > 0.0;
        let pitch = p.pitch;
        self.private_dirty.insert(player);
        // Tired or hurt arms (the action's power) hold it less steady.
        let spread = ((spread + if moving { move_spread } else { 0.0 }) / a.power.clamp(0.4, 1.0)).to_radians();
        // Where the shooter's client showed everybody (none: where they are).
        let history = std::mem::take(&mut self.lag_history);
        let frame = self.rewind_tick(pi, ahead).and_then(|t| history.frame(t));
        let mut hits = 0;
        for _ in 0..pellets {
            let yaw = facing + (self.combat_rng.f32() * 2.0 - 1.0) * spread;
            let up = (pitch + (self.combat_rng.f32() * 2.0 - 1.0) * spread).clamp(-MAX_PITCH, MAX_PITCH);
            hits += self.pellet(pi, a, &s, origin, yaw, up, far, frame);
        }
        self.lag_history = history;
        self.action_events.push(ActionEvent { player, kind: a.kind, weapon: a.weapon, hits });
        if noise > 0.0 && self.heard.len() < crate::world::MAX_SOUNDS {
            // Heard this tick, like a swing; loud enough, it draws the far horde too.
            self.heard.push(Sound::new(origin, noise, SoundSource::Player(player)));
        }
    }

    /// One bullet or pellet: follow it, hit the bodies it passes through (at most
    /// `s.max_targets`), break the glass on the way. Returns the bodies hit.
    #[allow(clippy::too_many_arguments)]
    fn pellet(&mut self, pi: usize, a: Action, s: &Strike, origin: Vec2, yaw: f32, pitch: f32, far: f32, frame: Option<&Frame>) -> u32 {
        let dir = Vec2::from_angle(yaw);
        let slope = pitch.tan();
        let reach = s.range * pitch.cos();
        let path = flight(&self.map, origin, dir, reach, slope);
        let player = self.players[pi].id;
        // They may have moved this far since the shooter saw them.
        let moved = frame.map_or(0.0, |f| self.tick.saturating_sub(f.tick) as f32 * TICK_DT * crate::lagcomp::MAX_BODY_SPEED);

        // How tall a body stands. One floored earlier this tick (by another pellet of the same
        // shot, say) is still on its feet as far as this one goes: the pellets fly together.
        let (hit_events, player_hit_events) = (&self.hit_events, &self.player_hit_events);
        let zombie_top = |z: &crate::zombies::Zombies, j: usize| {
            let fell_now = || hit_events.iter().any(|h| h.zombie == z.id[j] && h.effect == crate::combat::HitEffect::KnockedDown);
            if z.state[j] == ZombieState::Down && !fell_now() { DOWN_HEIGHT } else { BODY_HEIGHT }
        };
        let player_top = |o: &crate::players::Player| {
            let fell_now = || player_hit_events.iter().any(|h| h.victim == o.id && h.effect == crate::combat::HitEffect::KnockedDown);
            if (o.knock.is_some_and(|k| k.down) && !fell_now()) || o.needs.asleep() { DOWN_HEIGHT } else { BODY_HEIGHT }
        };

        // Bodies on the line, nearest first (ties by index, so the order is stable), with how
        // tall each stood.
        let mut found = std::mem::take(&mut self.scratch_hits);
        let mut tops = std::mem::take(&mut self.scratch_tops);
        found.clear();
        tops.clear();
        let z = &self.zombies;
        self.hash.for_each_within(origin + dir * (path.end * 0.5), path.end * 0.5 + ZOMBIE_RADIUS + moved, |j| {
            if z.state[j] == ZombieState::Dead {
                return;
            }
            let at = frame.and_then(|f| f.zombie(z.id[j])).unwrap_or(z.pos[j]);
            let top = zombie_top(z, j);
            if let Some(t) = through(origin, dir, slope, path.end, at, ZOMBIE_RADIUS, top) {
                found.push((t, j));
                tops.push((j, top));
            }
        });
        if self.config.combat.pvp {
            for (k, o) in self.players.iter().enumerate() {
                // A fresh survivor's spawn protection (#68): shots pass it by too.
                if k == pi || o.dead || o.protected > 0.0 {
                    continue;
                }
                let at = frame.and_then(|f| f.player(o.id)).unwrap_or(o.state.pos);
                let top = player_top(o);
                if let Some(t) = through(origin, dir, slope, path.end, at, PLAYER_RADIUS, top) {
                    found.push((t, PLAYER_TARGET | k));
                    tops.push((PLAYER_TARGET | k, top));
                }
            }
        }
        found.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));

        let forward = dir;
        let mut hits = 0;
        let mut end = path.end;
        for &(t, j) in &found {
            if hits >= s.max_targets {
                break;
            }
            let victim = (j & PLAYER_TARGET != 0).then_some(j & !PLAYER_TARGET);
            let top = tops.iter().find(|x| x.0 == j).map_or(BODY_HEIGHT, |x| x.1);
            let height = (MUZZLE_HEIGHT + t * slope).clamp(0.0, top);
            // The head: what a blow's crit is. Lying down there is no telling.
            let head = top == BODY_HEIGHT && height >= HEAD_FROM;
            let strike = Strike { crit_chance: if head { 1.0 } else { 0.0 }, shot: Some(height), ..*s };
            let falloff = self.config.combat.multi_hit_falloff.powi(hits as i32) * (1.0 + (far - 1.0) * (t / reach.max(1e-3)).clamp(0.0, 1.0));
            // A shot does what the gun does, however tired the arm.
            let full = Action { power: 1.0, ..a };
            // Where the body was hit from: straight back along the line.
            let at = origin + dir * t.max(0.05);
            match victim {
                Some(k) => self.strike_player(pi, full, &strike, origin, forward, k, at, falloff),
                None => self.strike(player, full, &strike, origin, forward, j, at, falloff),
            };
            hits += 1;
            if hits >= s.max_targets {
                end = t;
            }
        }
        self.scratch_hits = found;
        self.scratch_tops = tops;
        for &(t, edge) in path.glass() {
            if t <= end {
                self.break_edge(edge, SoundSource::Player(player));
            }
        }
        // For the client: the line it flew (`damage` is its length over the ground).
        self.push_combat_event(CombatEvent { kind: CombatEventKind::Shot, pos: origin + dir * end, dir, zombie: 0, player, damage: end, crit: false, part: None, wound: None, by: 0 });
        hits
    }

    /// The body part a shot at `height` hits on a player (the combat RNG picks the side).
    pub(crate) fn shot_part(&mut self, height: f32) -> crate::health::Part {
        part_at(height, self.combat_rng.f32())
    }

    /// How player `pi` would reload now: the carried magazine to swap in (`Ok(Some(index))`), or a
    /// loose round to push in (`Ok(None)`); `Err` says why not. `spare`: fill a carried magazine
    /// instead of the gun in hand (then `Ok(Some(index))` is the magazine to fill).
    fn reload_plan(&self, pi: usize, spare: bool) -> Result<Option<usize>, String> {
        let inv = &self.players[pi].inventory;
        let has = |item: u16| inv.carried.iter().any(|s| s.item == item);
        let name = |item: u16| self.item_def(item).name.to_lowercase();
        if spare {
            let mut any = None;
            for (k, s) in inv.carried.iter().enumerate() {
                let Some(h) = self.holds(s.item).filter(|h| h.magazine) else { continue };
                any = Some(h);
                if s.used < h.capacity && has(h.ammo) {
                    return Ok(Some(k));
                }
            }
            return Err(match any {
                Some(h) if !has(h.ammo) => format!("no {} to fill it with", name(h.ammo)),
                Some(_) => "every magazine is full".into(),
                None => "no magazine to fill".into(),
            });
        }
        let Some(gun) = inv.hand else { return Err("nothing in hand to reload".into()) };
        let Some(h) = self.holds(gun.item).filter(|h| !h.magazine) else { return Err(format!("the {} isn't something to reload", name(gun.item))) };
        if gun.used >= h.capacity {
            return Err(format!("the {} is full", name(gun.item)));
        }
        // The fullest magazine carried, if it holds more than the one in the gun.
        let magazine = self.item_weapon(gun.item).and_then(|w| self.config.weapons[w as usize].ranged.as_ref()).and_then(|r| r.magazine.as_deref());
        if let Some(m) = magazine
            && let Some((k, s)) = inv.carried.iter().enumerate().filter(|(_, s)| self.item_def(s.item).id == m).max_by_key(|(k, s)| (s.used, std::cmp::Reverse(*k)))
            && s.used > gun.used
        {
            return Ok(Some(k));
        }
        if has(h.ammo) { Ok(None) } else { Err(format!("no {} for the {}", name(h.ammo), name(gun.item))) }
    }

    /// Seconds the next step of a reload takes.
    fn reload_secs(&self, pi: usize, spare: bool, swap: bool) -> f32 {
        let inv = &self.players[pi].inventory;
        let ranged = |item: u16| {
            let def = self.item_def(item);
            self.config.weapons.iter().find(|w| def.weapon.as_deref() == Some(w.id.as_str()) || w.ranged.as_ref().is_some_and(|r| r.magazine.as_deref() == Some(def.id.as_str()))).and_then(|w| w.ranged.as_ref())
        };
        let r = if spare { inv.carried.iter().find_map(|s| ranged(s.item)) } else { inv.hand.and_then(|g| ranged(g.item)) };
        r.map_or(1.0, |r| if swap && !spare { r.reload_seconds } else { r.round_seconds })
    }

    /// Player `id` reloads the gun in its hand: the fullest magazine it carries swapped in, else
    /// loose rounds pushed in one by one until it is full or they run out. A job like eating:
    /// it doesn't stop you walking, and swinging or shooting drops it.
    pub fn player_reload(&mut self, id: u32) -> Result<(), String> {
        self.queue_reload(id, false)
    }

    /// Player `id` fills a magazine it carries with loose rounds, one by one.
    pub fn player_fill_magazine(&mut self, id: u32) -> Result<(), String> {
        self.queue_reload(id, true)
    }

    fn queue_reload(&mut self, id: u32, spare: bool) -> Result<(), String> {
        let Some(pi) = self.players.iter().position(|p| p.id == id) else { return Err(format!("no player {id}")) };
        // Already at it (a repeated key, a click on an empty gun): nothing more to do.
        if self.players[pi].jobs.iter().any(|j| j.kind == JobKind::Reload { spare }) {
            return Ok(());
        }
        match self.reload_plan(pi, spare) {
            Ok(plan) => {
                let secs = self.reload_secs(pi, spare, plan.is_some());
                self.queue_job(id, JobKind::Reload { spare }, secs)
            }
            Err(why) => self.refuse(id, why),
        }
    }

    /// A reload's step is due: swap the magazine, or push one round in and carry on with the next.
    /// An empty `Ok` is a step with more to come (nothing to tell the player yet).
    pub(crate) fn do_reload(&mut self, pi: usize, spare: bool) -> Result<String, String> {
        let plan = self.reload_plan(pi, spare)?;
        let inv = &mut self.players[pi].inventory;
        let text = if spare {
            let k = plan.expect("a magazine to fill");
            let h = holds(&self.config.items, &self.config.weapons, inv.carried[k].item).expect("a magazine");
            inv.carried[k].used += 1;
            let (now, name) = (inv.carried[k].used, self.config.items[inv.carried[k].item as usize].name.to_lowercase());
            take_round(&mut inv.carried, h.ammo);
            format!("filled the {name}: {now} of {}", h.capacity)
        } else {
            let gun = inv.hand.as_mut().expect("a gun in hand");
            let h = holds(&self.config.items, &self.config.weapons, gun.item).expect("a gun");
            let name = self.config.items[gun.item as usize].name.to_lowercase();
            match plan {
                Some(k) => {
                    std::mem::swap(&mut gun.used, &mut inv.carried[k].used);
                    format!("reloaded the {name}: {} of {}", gun.used, h.capacity)
                }
                None => {
                    gun.used += 1;
                    let now = gun.used;
                    take_round(&mut inv.carried, h.ammo);
                    format!("loaded the {name}: {now} of {}", h.capacity)
                }
            }
        };
        self.refresh_carry(pi);
        // More rounds to push in: the next one goes straight on, ahead of whatever else is queued.
        if let Ok(next) = self.reload_plan(pi, spare)
            && (spare || next.is_none())
        {
            let secs = self.reload_secs(pi, spare, false);
            self.players[pi].jobs.insert(0, crate::inventory::ItemJob { kind: JobKind::Reload { spare }, left: secs });
            return Ok(String::new());
        }
        Ok(text)
    }

    /// `Use` on a gun in hand reloads it, on a carried magazine fills it. `None`: `item` at
    /// `from` is neither, and the use is something else's.
    pub(crate) fn use_reloads(&mut self, id: u32, from: Place, item: u16) -> Option<Result<(), String>> {
        let h = self.holds(item)?;
        Some(match (h.magazine, from) {
            (false, Place::Slot(Slot::Hand)) => self.player_reload(id),
            (false, _) => self.refuse(id, "take it in your hand to reload it".into()),
            (true, Place::Carried) => self.player_fill_magazine(id),
            (true, _) => self.refuse(id, "put the magazine in a pocket to fill it".into()),
        })
    }

    /// Guns and magazines just found in container `c` hold what they were left with: a seeded
    /// number of rounds, more often few than many.
    pub(crate) fn load_found(&self, c: u32, stacks: &mut [ItemStack]) {
        let seed = self.config.loot_seed() ^ FOUND_SALT;
        for (k, s) in stacks.iter_mut().enumerate() {
            let Some(h) = self.holds(s.item) else { continue };
            let share = hash01(seed, c as u64, k as u64, s.item as u64);
            s.used = (share * share * (h.capacity as f32 + 1.0)) as u8;
            s.used = s.used.min(h.capacity);
        }
    }
}

/// Take one round of `ammo` out of `carried` (from the last stack of it, so the first stays whole).
fn take_round(carried: &mut Vec<ItemStack>, ammo: u16) {
    if let Some(k) = carried.iter().rposition(|s| s.item == ammo) {
        carried[k].count -= 1;
        if carried[k].count == 0 {
            carried.remove(k);
        }
    }
}

#[cfg(test)]
mod tests;
