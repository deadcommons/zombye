//! Weapons wear out. A weapon with a `durability` in `weapons.ron` takes knocks as it is used: each
//! swing that hits something may add one ([`WeaponDef::wear_chance`], less in practised hands), and
//! at `durability` knocks it breaks: it stays where it is (in the hand, the bags, the hotbar) as a
//! broken thing, no use as a weapon ([`crate::CombatTuning::broken_damage`]) and never wearing
//! further, until it can be mended or taken apart (later). How many it has taken rides on its stack
//! ([`ItemStack::used`], which for such a weapon counts knocks: no save format change), so a
//! weapon keeps its wear on the floor, in a cupboard and in another survivor's hands. Bare hands
//! and guns don't wear. All numbers are ours.

use crate::items::{ItemDef, ItemStack, WeaponDef};
use crate::players::{Action, ActionKind};
use crate::rng::hash01;
use crate::world::World;

const WEAR_SALT: u64 = 0x3EA2_0B1D_77C4_19A5;
const FOUND_SALT: u64 = 0x51FE_A3C0_0D2B_6E11;

/// The knocks `item` takes before it breaks; 0: it isn't a weapon that wears.
pub fn durability(items: &[ItemDef], weapons: &[WeaponDef], item: u16) -> u8 {
    let Some(id) = items.get(item as usize).and_then(|d| d.weapon.as_deref()) else { return 0 };
    weapons.iter().find(|w| w.id == id).map_or(0, |w| w.durability)
}

/// How a weapon that wears is holding up, for showing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Condition {
    Good,
    Worn,
    /// Weaker ([`crate::CombatTuning::battered_damage`]), and not long for this world.
    Battered,
    /// Every knock taken: still an item, but next to no use as a weapon
    /// ([`crate::CombatTuning::broken_damage`], no crits or knockdowns).
    Broken,
}

impl Condition {
    pub fn label(self) -> &'static str {
        match self {
            Self::Good => "good",
            Self::Worn => "worn",
            Self::Battered => "battered",
            Self::Broken => "broken",
        }
    }
}

/// The state of stack `s`: knocks it can still take, out of how many, and what to call that.
/// `None`: it doesn't wear. `battered_below` is [`crate::CombatTuning::battered_below`].
pub fn condition(items: &[ItemDef], weapons: &[WeaponDef], s: &ItemStack, battered_below: f32) -> Option<(u8, u8, Condition)> {
    let of = durability(items, weapons, s.item);
    if of == 0 {
        return None;
    }
    let left = of.saturating_sub(s.used);
    let share = left as f32 / of as f32;
    let c = if left == 0 {
        Condition::Broken
    } else if share <= battered_below {
        Condition::Battered
    } else if share <= 0.6 {
        Condition::Worn
    } else {
        Condition::Good
    };
    Some((left, of, c))
}

impl World {
    /// The state of a stack ([`condition`]) in this world.
    pub fn condition_of(&self, s: &ItemStack) -> Option<(u8, u8, Condition)> {
        condition(&self.config.items, &self.config.weapons, s, self.config.combat.battered_below)
    }

    /// The condition of the weapon in player `pi`'s hand (`None`: nothing there that wears).
    pub(crate) fn hand_condition(&self, pi: usize) -> Option<Condition> {
        self.players[pi].inventory.hand.and_then(|s| self.condition_of(&s)).map(|c| c.2)
    }

    /// What is left of the damage of the weapon in player `pi`'s hand: less once it is battered,
    /// next to nothing once broken.
    pub(crate) fn wear_damage_mult(&self, pi: usize) -> f32 {
        match self.hand_condition(pi) {
            Some(Condition::Battered) => self.config.combat.battered_damage,
            Some(Condition::Broken) => self.config.combat.broken_damage,
            _ => 1.0,
        }
    }

    /// Player `pi`'s swing `a` hit `hits` bodies: the weapon in its hand may take a knock, and
    /// breaks at its last (and stays in the hand, broken). Rolled from the seed, the tick and the player, not a stream, so worlds
    /// whose weapons don't wear run exactly as before.
    pub(crate) fn wear_weapon(&mut self, pi: usize, a: Action, hits: u32) {
        if a.kind != ActionKind::Swing || hits == 0 {
            return;
        }
        let w = &self.config.weapons[a.weapon as usize];
        let Some(held) = self.players[pi].inventory.hand else { return };
        // A broken one has nothing more to lose.
        if w.durability == 0 || held.used >= w.durability || self.item_weapon(held.item) != Some(a.weapon) {
            return;
        }
        let level = self.skill_level(pi, w.skill());
        let chance = w.wear_chance * (1.0 - self.config.combat.wear_skill_less * level as f32).max(0.0);
        let id = self.players[pi].id;
        if hash01(self.config.seed() ^ WEAR_SALT, self.tick, id as u64, held.item as u64) >= chance {
            return;
        }
        let (of, name) = (w.durability, self.item_def(held.item).name.to_lowercase());
        let knocks = held.used.saturating_add(1);
        if knocks >= of {
            self.players[pi].inventory.hand = Some(ItemStack { used: of, ..held });
            self.note(id, false, format!("your {name} broke: it's no use as a weapon now"));
        } else {
            let before = self.condition_of(&held).map(|c| c.2);
            self.players[pi].inventory.hand = Some(ItemStack { used: knocks, ..held });
            let now = self.condition_of(&ItemStack { used: knocks, ..held }).map(|c| c.2);
            if now != before && now == Some(Condition::Battered) {
                self.note(id, false, format!("your {name} is battered: it won't last much longer"));
            }
        }
        self.refresh_carry(pi);
    }

    /// Weapons just found in container `c` have seen some use: a seeded number of knocks, more
    /// often few than many.
    pub(crate) fn wear_found(&self, c: u32, stacks: &mut [ItemStack]) {
        let seed = self.config.loot_seed() ^ FOUND_SALT;
        for (k, s) in stacks.iter_mut().enumerate() {
            let of = durability(&self.config.items, &self.config.weapons, s.item);
            if of == 0 {
                continue;
            }
            let share = hash01(seed, c as u64, k as u64, s.item as u64);
            s.used = ((share * share * self.config.combat.found_wear_max.clamp(0.0, 1.0) * of as f32) as u8).min(of - 1);
        }
    }
}

#[cfg(test)]
mod tests;
