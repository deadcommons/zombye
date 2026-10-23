//! Items in the world. For now only melee weapons: their definitions are data
//! (`assets/data/items/weapons.ron`, parsed with [`parse_weapons`]) and a few lie on house floors
//! as [`GroundItem`]s that a player can pick up and swing. Inventory proper comes with M4.

use glam::{IVec2, Vec2};
use serde::{Deserialize, Serialize};

use crate::map::{Feature, TileMap};
use crate::rng::Rng;

/// What kind of weapon it is. Informational for now (skills and animations will key off it).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WeaponCategory {
    /// Bare hands. Exactly one weapon must be this; it's what a player without a weapon uses.
    Unarmed,
    ShortBlade,
    ShortBlunt,
    LongBlunt,
    Axe,
}

/// One melee weapon. Distances in metres (reach is measured to the target's body), times in
/// seconds, chances in 0..1. All numbers are ours.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WeaponDef {
    pub id: String,
    pub name: String,
    pub category: WeaponCategory,
    /// Damage per hit, rolled uniformly (zombies have about 100 hit points).
    pub damage: (f32, f32),
    /// Reach from the player's centre to the edge of a zombie's body.
    pub range: f32,
    /// Width of the swing in front of the player.
    pub arc_degrees: f32,
    /// From starting the swing to the moment it lands.
    pub windup_seconds: f32,
    /// From starting the swing until the next action can start (≥ windup).
    pub swing_seconds: f32,
    /// Zombies one swing can hit, nearest first.
    pub max_targets: u32,
    /// How far a hit pushes a zombie back.
    pub knockback: f32,
    pub stagger_chance: f32,
    pub knockdown_chance: f32,
    pub crit_chance: f32,
    pub crit_multiplier: f32,
    /// Endurance (0..1) one swing costs.
    pub endurance_cost: f32,
    /// Radius of the noise a swing makes.
    pub noise_radius: f32,
    /// Relative chance of being placed in a house (0 = never).
    #[serde(default)]
    pub spawn_weight: f32,
    /// Room types (`kitchen`, `bedroom`, ...) where it's three times as likely to turn up.
    #[serde(default)]
    pub found_in: Vec<String>,
    /// Grey-box look: length in metres and colour (linear-ish RGB 0..1).
    #[serde(default = "default_length")]
    pub length: f32,
    #[serde(default = "default_color")]
    pub color: (f32, f32, f32),
}

fn default_length() -> f32 {
    0.5
}

fn default_color() -> (f32, f32, f32) {
    (0.5, 0.5, 0.5)
}

impl WeaponDef {
    /// Built-in bare hands, used when the content has no `Unarmed` weapon (tests, bare configs).
    pub fn fists() -> Self {
        Self {
            id: "fists".into(),
            name: "Bare hands".into(),
            category: WeaponCategory::Unarmed,
            damage: (4.0, 7.0),
            range: 0.6,
            arc_degrees: 60.0,
            windup_seconds: 0.15,
            swing_seconds: 0.45,
            max_targets: 1,
            knockback: 0.15,
            stagger_chance: 0.25,
            knockdown_chance: 0.03,
            crit_chance: 0.05,
            crit_multiplier: 1.5,
            endurance_cost: 0.02,
            noise_radius: 3.0,
            spawn_weight: 0.0,
            found_in: Vec::new(),
            length: 0.0,
            color: (0.8, 0.65, 0.5),
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        let id = &self.id;
        let chance = |name: &str, v: f32| if (0.0..=1.0).contains(&v) { Ok(()) } else { Err(format!("{id}: {name} must be in 0..1, got {v}")) };
        if id.is_empty() || !id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_') {
            return Err(format!("weapon id {id:?} must be lowercase letters, digits and _"));
        }
        let (lo, hi) = self.damage;
        if !(lo >= 0.0 && hi >= lo && hi.is_finite()) {
            return Err(format!("{id}: damage must be (min, max) with 0 <= min <= max, got ({lo}, {hi})"));
        }
        if !(self.range > 0.0 && self.range <= 4.0) {
            return Err(format!("{id}: range must be in (0, 4] m, got {}", self.range));
        }
        if !(self.arc_degrees > 0.0 && self.arc_degrees <= 360.0) {
            return Err(format!("{id}: arc_degrees must be in (0, 360], got {}", self.arc_degrees));
        }
        if !(self.windup_seconds >= 0.0 && self.swing_seconds >= self.windup_seconds && self.swing_seconds >= 0.1 && self.swing_seconds <= 5.0) {
            return Err(format!("{id}: need 0 <= windup_seconds <= swing_seconds, swing_seconds in [0.1, 5], got {} and {}", self.windup_seconds, self.swing_seconds));
        }
        if self.max_targets == 0 || self.max_targets > 8 {
            return Err(format!("{id}: max_targets must be 1..8, got {}", self.max_targets));
        }
        if !(0.0..=1.5).contains(&self.knockback) {
            return Err(format!("{id}: knockback must be in 0..1.5 m, got {}", self.knockback));
        }
        chance("stagger_chance", self.stagger_chance)?;
        chance("knockdown_chance", self.knockdown_chance)?;
        chance("crit_chance", self.crit_chance)?;
        chance("endurance_cost", self.endurance_cost)?;
        if !(self.crit_multiplier >= 1.0 && self.crit_multiplier <= 10.0) {
            return Err(format!("{id}: crit_multiplier must be in 1..10, got {}", self.crit_multiplier));
        }
        if !(0.0..=100.0).contains(&self.noise_radius) {
            return Err(format!("{id}: noise_radius must be in 0..100 m, got {}", self.noise_radius));
        }
        if !(self.spawn_weight >= 0.0 && self.spawn_weight.is_finite()) {
            return Err(format!("{id}: spawn_weight must be >= 0"));
        }
        if self.category == WeaponCategory::Unarmed && self.spawn_weight > 0.0 {
            return Err(format!("{id}: an Unarmed weapon can't lie about (spawn_weight must be 0)"));
        }
        if !(0.0..=3.0).contains(&self.length) {
            return Err(format!("{id}: length must be in 0..3 m"));
        }
        Ok(())
    }
}

/// Parse and validate a weapons file: a RON list of [`WeaponDef`]. Ids must be unique and exactly
/// one weapon must be `Unarmed`.
pub fn parse_weapons(text: &str) -> Result<Vec<WeaponDef>, String> {
    let defs: Vec<WeaponDef> = ron::from_str(text).map_err(|e| e.to_string())?;
    validate_weapons(&defs)?;
    Ok(defs)
}

pub fn validate_weapons(defs: &[WeaponDef]) -> Result<(), String> {
    for (i, d) in defs.iter().enumerate() {
        d.validate()?;
        if defs[..i].iter().any(|o| o.id == d.id) {
            return Err(format!("weapon id {:?} is used twice", d.id));
        }
    }
    match defs.iter().filter(|d| d.category == WeaponCategory::Unarmed).count() {
        1 => Ok(()),
        n => Err(format!("need exactly one Unarmed weapon (bare hands), found {n}")),
    }
}

/// A weapon lying on the ground. `weapon` indexes `WorldConfig::weapons`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct GroundItem {
    /// Stable id, never reused within a world.
    pub id: u32,
    pub weapon: u16,
    pub pos: Vec2,
    /// Facing angle it lies at (radians), for the renderer.
    pub angle: f32,
}

/// Scatter weapons over house floors: 0–2 per building on free ground-floor tiles, at least one in
/// `spawn_building`, chosen by `spawn_weight` (three times as likely in a room of a `found_in`
/// type). Deterministic for the same map, weapons and seed; doesn't touch the world RNG.
pub fn place_weapons(map: &TileMap, weapons: &[WeaponDef], spawn_building: usize, seed: u64, first_id: u32) -> Vec<GroundItem> {
    let mut out = Vec::new();
    if weapons.iter().all(|w| w.spawn_weight <= 0.0) {
        return out;
    }
    let mut rng = Rng::new(seed ^ 0x17E5_CA77_E2ED);
    let mut tiles: Vec<IVec2> = Vec::new();
    for (bi, b) in map.buildings.iter().enumerate() {
        tiles.clear();
        for y in b.min.y..b.min.y + b.size.y {
            for x in b.min.x..b.min.x + b.size.x {
                let t = IVec2::new(x, y);
                let tile = map.tile(t);
                if tile.room != 0 && map.rooms[tile.room as usize].building as usize == bi && tile.feature == Feature::None {
                    tiles.push(t);
                }
            }
        }
        let mut count = rng.below(3);
        if bi == spawn_building {
            count = count.max(1);
        }
        for _ in 0..count.min(tiles.len() as u32) {
            let k = rng.below(tiles.len() as u32) as usize;
            let t = tiles.swap_remove(k);
            let room_type = &map.rooms[map.tile(t).room as usize].room_type;
            let weight = |w: &WeaponDef| w.spawn_weight * if w.found_in.iter().any(|r| r == room_type) { 3.0 } else { 1.0 };
            let total: f32 = weapons.iter().map(weight).sum();
            let mut roll = rng.f32() * total;
            let mut pick = None;
            for (wi, w) in weapons.iter().enumerate() {
                let wt = weight(w);
                if wt <= 0.0 {
                    continue;
                }
                pick = Some(wi);
                if roll < wt {
                    break;
                }
                roll -= wt;
            }
            let Some(wi) = pick else { continue };
            let pos = t.as_vec2() + Vec2::new(rng.range(0.3, 0.7), rng.range(0.3, 0.7));
            out.push(GroundItem { id: first_id + out.len() as u32, weapon: wi as u16, pos, angle: rng.range(0.0, std::f32::consts::TAU) });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn weapons() -> Vec<WeaponDef> {
        parse_weapons(include_str!("../../../assets/data/items/weapons.ron")).unwrap()
    }

    #[test]
    fn weapons_file_is_valid() {
        let w = weapons();
        assert!(w.len() >= 9, "only {} weapons", w.len());
        assert_eq!(w.iter().filter(|d| d.category == WeaponCategory::Unarmed).count(), 1);
        assert!(w.iter().filter(|d| d.spawn_weight > 0.0).count() >= 8);
    }

    #[test]
    fn validation_explains_mistakes() {
        let mut w = weapons();
        w[1].damage = (10.0, 5.0);
        assert!(validate_weapons(&w).unwrap_err().contains("damage"));
        let mut w = weapons();
        w[2].id = w[1].id.clone();
        assert!(validate_weapons(&w).unwrap_err().contains("twice"));
        let mut w = weapons();
        w[1].stagger_chance = 1.5;
        assert!(validate_weapons(&w).unwrap_err().contains("stagger_chance"));
        let w: Vec<WeaponDef> = weapons().into_iter().filter(|d| d.category != WeaponCategory::Unarmed).collect();
        assert!(validate_weapons(&w).unwrap_err().contains("Unarmed"));
        assert!(parse_weapons("[(id: \"x\")]").is_err());
    }
}
