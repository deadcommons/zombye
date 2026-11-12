//! Chopping trees down (`ChopTuning`, `World::chop_tree`).
//!
//! A tree (`Feature::Tree` or `Feature::PineTree`) has hit points while it stands. A swing of an
//! item carrying the chop tag (`ChopTuning::tag`, "chops": the fire axe) that hits no body takes
//! them off the trunk it faces; at none the tree comes down, leaving a `Feature::Stump` (low:
//! nothing walks round it any more) and logs on the ground round it. Each chop is noisy (the
//! weapon's noise), wears the axe like a hit, trains Strength a little, and is a
//! `CombatEventKind::Chop` for the client's chips and thunk; the fall is a louder noise.
//!
//! **State:** the hit points of trees chopped at (`Trees::hp`, sparse by tile; full hit points
//! when missing) and the tiles felled (`Trees::felled`). Both are saved (the optional `TREE`
//! section, read straight after the map so the stumps are on it before the walk grid, the nav
//! graph and the flow fields are rebuilt) and in `state_hash` only while there are any, so worlds
//! nobody has chopped in hash as before. **Replication:** each felled tree is a
//! [`TreeChange`] (`World::take_tree_changes`), and a joining client gets every stump
//! (`World::stumps`).
//!
//! Every number is in [`ChopTuning`] (`WorldConfig::chop`, serde defaults, saved with the
//! settings; ours).

use std::collections::BTreeMap;

use glam::{IVec2, Vec2};
use serde::{Deserialize, Serialize};

use crate::combat::{CombatEvent, CombatEventKind};
use crate::map::{Feature, TREE_RADIUS};
use crate::skills::Skill;
use crate::senses::SoundSource;
use crate::world::World;

/// How trees are chopped down, and what they give.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ChopTuning {
    /// The item tag that chops (`items.ron`): a swing of an item with it hits trees.
    pub tag: String,
    /// Hit points of a broadleaf or birch (`Feature::Tree`).
    pub tree_hp: f32,
    /// Hit points of a pine or spruce (`Feature::PineTree`).
    pub pine_hp: f32,
    /// A chop takes the weapon's middle damage (skill and wear included) times this.
    pub damage_mult: f32,
    /// The item a felled tree leaves (`items.ron`).
    pub log_item: String,
    /// Logs a felled broadleaf leaves.
    pub tree_logs: u16,
    /// Logs a felled pine leaves.
    pub pine_logs: u16,
    /// Radius of the noise a tree makes coming down (metres).
    pub fall_noise: f32,
    /// A chop costs this share of the swing's endurance (steady work, not a fight's swing): the
    /// rest is given back when it lands on a tree.
    pub endurance_mult: f32,
    /// Strength XP a chop gives.
    pub chop_xp: f32,
    /// Strength XP for felling a tree.
    pub fell_xp: f32,
}

impl Default for ChopTuning {
    fn default() -> Self {
        Self {
            tag: "chops".into(),
            tree_hp: 480.0,
            pine_hp: 400.0,
            damage_mult: 1.0,
            log_item: "log".into(),
            tree_logs: 2,
            pine_logs: 3,
            fall_noise: 40.0,
            endurance_mult: 0.5,
            chop_xp: 0.5,
            fell_xp: 4.0,
        }
    }
}

impl ChopTuning {
    /// Values a save or a settings file may hold.
    pub fn check(&self) -> Result<(), String> {
        let ok = |v: f32, lo: f32, hi: f32| v.is_finite() && v >= lo && v <= hi;
        if !ok(self.tree_hp, 1.0, 1e5) || !ok(self.pine_hp, 1.0, 1e5) {
            return Err("chop settings: tree hit points must be 1 to 100000".into());
        }
        if !ok(self.damage_mult, 0.0, 100.0) || !ok(self.fall_noise, 0.0, 1000.0) || !ok(self.chop_xp, 0.0, 1000.0) || !ok(self.fell_xp, 0.0, 1000.0) || !ok(self.endurance_mult, 0.0, 10.0) {
            return Err("chop settings: damage_mult, fall_noise, endurance_mult or XP out of range".into());
        }
        if self.tree_logs > 20 || self.pine_logs > 20 {
            return Err("chop settings: at most 20 logs a tree".into());
        }
        Ok(())
    }

    /// Full hit points of a tree of this kind (none: not a tree).
    pub fn hp_of(&self, f: Feature) -> Option<f32> {
        match f {
            Feature::Tree => Some(self.tree_hp),
            Feature::PineTree => Some(self.pine_hp),
            _ => None,
        }
    }

    pub fn logs_of(&self, f: Feature) -> u16 {
        if f == Feature::PineTree { self.pine_logs } else { self.tree_logs }
    }
}

/// A tile whose feature changed in play (a tree felled to a stump), for clients' map copies.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreeChange {
    pub tile: IVec2,
    pub feature: Feature,
}

/// A chop landed this tick (cleared each tick): which tree, what's left of it, and whether it fell.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChopEvent {
    pub player: u32,
    pub tile: IVec2,
    /// Share of its hit points left (0 once felled).
    pub left: f32,
    pub felled: bool,
}

/// The trees chopped at and felled.
#[derive(Clone, Debug, Default)]
pub struct Trees {
    /// Hit points of trees chopped at but standing, by tile. Saved.
    pub(crate) hp: BTreeMap<(i32, i32), f32>,
    /// Tiles whose tree was felled (each a stump now), with what tree it was. Saved.
    pub(crate) felled: BTreeMap<(i32, i32), Feature>,
    /// Replication: felled since the last snapshot took them.
    pub(crate) changes: Vec<TreeChange>,
    /// Chops this tick.
    pub(crate) events: Vec<ChopEvent>,
}

/// What a save keeps of the trees (`TREE`).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct TreeSave {
    pub hp: Vec<((i32, i32), f32)>,
    pub felled: Vec<((i32, i32), Feature)>,
}

impl Trees {
    pub(crate) fn is_empty(&self) -> bool {
        self.hp.is_empty() && self.felled.is_empty()
    }

    pub(crate) fn to_save(&self) -> TreeSave {
        TreeSave { hp: self.hp.iter().map(|(&k, &v)| (k, v)).collect(), felled: self.felled.iter().map(|(&k, &v)| (k, v)).collect() }
    }

    pub(crate) fn hash(&self, mix: &mut impl FnMut(u32)) {
        for (&(x, y), hp) in &self.hp {
            mix(0x5452_0000);
            mix(x as u32);
            mix(y as u32);
            mix(hp.to_bits());
        }
        for (&(x, y), &f) in &self.felled {
            mix(0x5446_0000 | f as u32);
            mix(x as u32);
            mix(y as u32);
        }
    }
}

/// The tree nearest `origin` whose trunk a blow facing `facing` with reach `range` (to the bark)
/// and half arc `half_arc` reaches: its tile. Ground floor only; nothing that blocks a hand may
/// stand between (`vision::clear_reach`). Pure, so a client can ask it too.
pub fn tree_in_reach(map: &crate::map::TileMap, origin: Vec2, facing: f32, range: f32, half_arc: f32) -> Option<IVec2> {
    let forward = Vec2::from_angle(facing);
    let reach = range + TREE_RADIUS;
    let t = origin.floor().as_ivec2();
    let r = reach.ceil() as i32 + 1;
    let mut best: Option<(f32, IVec2)> = None;
    for dy in -r..=r {
        for dx in -r..=r {
            let n = t + IVec2::new(dx, dy);
            if !map.in_bounds(n) || !matches!(map.tile(n).feature, Feature::Tree | Feature::PineTree) {
                continue;
            }
            let centre = n.as_vec2() + Vec2::splat(0.5);
            let to = centre - origin;
            let d = to.length();
            if d > reach || d < 1e-4 {
                continue;
            }
            let half_width = (TREE_RADIUS / d).min(1.0).asin();
            if forward.dot(to / d).clamp(-1.0, 1.0).acos() > half_arc + half_width {
                continue;
            }
            // To the near side of the bark.
            let bark = centre - to / d * (TREE_RADIUS + 0.05);
            if !crate::vision::clear_reach(map, origin, bark) {
                continue;
            }
            if best.is_none_or(|(bd, bt)| d < bd || (d == bd && (n.y, n.x) < (bt.y, bt.x))) {
                best = Some((d, n));
            }
        }
    }
    best.map(|(_, n)| n)
}

impl World {
    /// The chop tuning in force.
    pub fn chop_tuning(&self) -> &ChopTuning {
        &self.config.chop
    }

    /// Hit points left on the tree at `tile` and its full hit points (none: no tree there).
    pub fn tree_hp(&self, tile: IVec2) -> Option<(f32, f32)> {
        if !self.map.in_bounds(tile) {
            return None;
        }
        let max = self.config.chop.hp_of(self.map.tile(tile).feature)?;
        Some((self.trees.hp.get(&(tile.x, tile.y)).copied().unwrap_or(max), max))
    }

    /// Trees chopped at but standing: tile, hit points left.
    pub fn damaged_trees(&self) -> impl Iterator<Item = (IVec2, f32)> + '_ {
        self.trees.hp.iter().map(|(&(x, y), &hp)| (IVec2::new(x, y), hp))
    }

    /// Every tree felled so far, as stumps: what a joining client applies to its map copy.
    pub fn stumps(&self) -> Vec<TreeChange> {
        self.trees.felled.keys().map(|&(x, y)| TreeChange { tile: IVec2::new(x, y), feature: Feature::Stump }).collect()
    }

    /// Trees felled since the last call (replication).
    pub fn take_tree_changes(&mut self) -> Vec<TreeChange> {
        std::mem::take(&mut self.trees.changes)
    }

    /// Chops that landed this tick.
    pub fn chop_events(&self) -> &[ChopEvent] {
        &self.trees.events
    }

    /// Does player `pi` hold something that chops?
    pub(crate) fn holds_chopper(&self, pi: usize) -> bool {
        let tag = &self.config.chop.tag;
        self.players[pi].inventory.hand.is_some_and(|s| self.config.items.get(s.item as usize).is_some_and(|d| d.has_tag(tag)))
    }

    /// A swing of player `pi` that hit no body lands on the tree it faces, if it holds something
    /// that chops. `damage` is the swing's middle damage. Returns whether it hit a tree.
    pub(crate) fn chop_swing(&mut self, pi: usize, origin: Vec2, facing: f32, range: f32, half_arc: f32, damage: f32) -> bool {
        if !self.holds_chopper(pi) || self.players[pi].state.level > 0 {
            return false;
        }
        let Some(tile) = tree_in_reach(&self.map, origin, facing, range, half_arc) else { return false };
        let (hp, max) = self.tree_hp(tile).expect("a tree");
        let left = (hp - damage * self.config.chop.damage_mult).max(0.0);
        let player = self.players[pi].id;
        let centre = tile.as_vec2() + Vec2::splat(0.5);
        let dir = (centre - origin).normalize_or_zero();
        self.push_combat_event(CombatEvent { kind: CombatEventKind::Chop, pos: centre - dir * TREE_RADIUS, dir, zombie: 0, player, damage: hp - left, crit: left <= 0.0, part: None, wound: None, by: 0 });
        let xp = self.config.chop.chop_xp;
        self.award_xp(pi, Skill::Strength, xp);
        // Steady chopping tires less than a fight's swings.
        let weapon = self.players[pi].weapon.unwrap_or(self.unarmed);
        let back = self.config.weapons[weapon as usize].endurance_cost * (1.0 - self.config.chop.endurance_mult);
        let e = &mut self.players[pi].state.endurance;
        *e = (*e + back).clamp(0.0, 1.0);
        let felled = left <= 0.0;
        self.trees.events.push(ChopEvent { player, tile, left: if felled { 0.0 } else { left / max }, felled });
        if felled {
            self.fell_tree(tile, dir);
            let xp = self.config.chop.fell_xp;
            self.award_xp(pi, Skill::Strength, xp);
            let logs = self.config.chop.logs_of(*self.trees.felled.get(&(tile.x, tile.y)).expect("just felled"));
            let name = self.config.items.iter().find(|d| d.id == self.config.chop.log_item).map_or("logs".to_string(), |d| d.name.to_lowercase());
            self.note(player, false, format!("the tree came down: {logs} x {name} on the ground"));
        } else {
            self.trees.hp.insert((tile.x, tile.y), left);
        }
        true
    }

    /// Bring the tree at `tile` down, falling along `dir`: a stump where it stood, logs along where
    /// it fell, a crash. Keeps the walk grid, the nav graph and the flow fields in step. (Public for
    /// tools and tests.)
    pub fn fell_tree(&mut self, tile: IVec2, dir: Vec2) {
        let Some(kind) = self.map.in_bounds(tile).then(|| self.map.tile(tile).feature).filter(|f| matches!(f, Feature::Tree | Feature::PineTree)) else { return };
        self.trees.hp.remove(&(tile.x, tile.y));
        self.trees.felled.insert((tile.x, tile.y), kind);
        self.map.tile_mut(tile).expect("in bounds").feature = Feature::Stump;
        self.trees.changes.push(TreeChange { tile, feature: Feature::Stump });
        self.walk.update_around(&self.map, tile);
        self.nav.edge_changed(&self.walk, tile);
        self.goal_dirty = true;
        self.flow_cache.invalidate_near(tile);
        let centre = tile.as_vec2() + Vec2::splat(0.5);
        let noise = self.config.chop.fall_noise;
        self.emit_sound_from(centre, noise, SoundSource::World);
        // Logs where the trunk fell, a metre and more out, cut to lengths.
        let Some(log) = self.item_index(&self.config.chop.log_item.clone()) else { return };
        let dir = if dir.length_squared() > 0.5 { dir } else { Vec2::X };
        let angle = dir.to_angle();
        for k in 0..self.config.chop.logs_of(kind) {
            let at = self.map.resolve_circle(centre + dir * (1.0 + 1.1 * k as f32), 0.2);
            let at = if self.map.in_bounds(at.floor().as_ivec2()) { at } else { centre };
            let s = self.fresh_stack(log, 1);
            self.put_on_floor(s, at, angle + std::f32::consts::FRAC_PI_2, 0);
        }
    }

    /// Put the saved trees on a map regenerated from the seed (load): stumps where trees were
    /// felled, hit points on trees chopped at. On a map that changed (`fit`), what no longer fits
    /// is dropped and counted; else it is an error naming the tile.
    pub(crate) fn restore_trees(&mut self, save: TreeSave, fit: bool) -> Result<usize, String> {
        let mut dropped = 0;
        let mut prev = None;
        for (k, kind) in save.felled {
            if prev.is_some_and(|p| p >= k) {
                return Err("felled trees out of order".into());
            }
            prev = Some(k);
            let tile = IVec2::new(k.0, k.1);
            if !matches!(kind, Feature::Tree | Feature::PineTree) {
                return Err(format!("felled tree at {tile} was a {kind:?}"));
            }
            let here = self.map.in_bounds(tile).then(|| self.map.tile(tile).feature);
            match here {
                // A map stored whole already has its stumps.
                Some(Feature::Stump) => {}
                Some(f) if f == kind => self.map.tile_mut(tile).expect("in bounds").feature = Feature::Stump,
                _ if fit => {
                    dropped += 1;
                    continue;
                }
                _ => return Err(format!("felled tree at {tile} where the map has no such tree")),
            }
            self.trees.felled.insert(k, kind);
        }
        let mut prev = None;
        for (k, hp) in save.hp {
            if prev.is_some_and(|p| p >= k) {
                return Err("chopped trees out of order".into());
            }
            prev = Some(k);
            let tile = IVec2::new(k.0, k.1);
            match self.tree_hp(tile) {
                Some((_, max)) if hp.is_finite() && hp > 0.0 && hp < max => {
                    self.trees.hp.insert(k, hp);
                }
                Some(_) if !hp.is_finite() || hp <= 0.0 => return Err(format!("tree at {tile} has {hp} hit points")),
                // Full or over (the tuning changed): as good as untouched.
                Some(_) => {}
                None if fit => dropped += 1,
                None => return Err(format!("hit points on a tree at {tile}, where the map has none")),
            }
        }
        Ok(dropped)
    }
}

#[cfg(test)]
mod tests;
