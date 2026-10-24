//! What players carry, and moving things between them, furniture and the floor.
//!
//! **Two limits.** *Room* (litres): pockets ([`CarryTuning::pockets_l`]) plus the bags a player
//! carries (on the back, on a shoulder, in a hand) hold the `carried` items; held and worn items take
//! no room. *Weight* (kg): everything counts in full wherever it is (bags add room, not lightness).
//! The load (weight over the carry limit, [`CarryTuning::limit`]) puts a player in a [`Burden`]
//! stage: slower, more tiring, no running, no climbing; past [`CarryTuning::max_load`] nothing more
//! can be picked up. The stage's [`MoveMods`] go into the player's state, so client prediction
//! moves exactly as the server does.
//!
//! **Slots.** Two hands ([`Slot::Hand`] swings, a two-handed item takes both), a bag on the back and
//! one on the shoulder, and worn clothing (two worn items can't share a body part on one layer).
//!
//! **Moving things takes time.** Moves, searches, uses, treatments and crafts ([`JobKind`]; first
//! aid in [`crate::firstaid`], crafting in [`crate::crafting`]) queue on the player
//! ([`Player::jobs`]); the first counts down and happens when it's done (0.3 s a stack, more for a
//! heavy one; a search takes the container's search time). Each is checked when it happens: the
//! thing is still there, the container is still open and within reach, it fits. Swinging, shoving or
//! stomping drops what you were doing. Picking up the item you face (`PickUp`, E) is the one quick
//! grab. A player who dies drops everything. Asking again for what is already queued (a repeated
//! click, a request crossing a snapshot) queues nothing more and refuses nothing.
//!
//! **Containers** ([`crate::containers`]) are opened by searching them (`Search`) from within
//! [`CONTAINER_REACH`] of the furniture's nearest edge, in its room (E picks the one faced, nearest
//! first, else the nearest): filled on first open, stored sparsely in the world from then on
//! (`World::opened`), and open to the searcher until it is [`CONTAINER_KEEP_OPEN`] away or leaves
//! the room. E with no container and nothing on the floor gets one plain answer ([`NOTHING_HERE`]).
//! **The floor** is the world's ground items: those within
//! [`PICKUP_REACH`](crate::combat::PICKUP_REACH) of a player with no wall, shut window or door or
//! fence between (furniture doesn't stop a hand) are its floor; snapshots send each player its floor.

use std::collections::BTreeSet;

use glam::Vec2;
use serde::{Deserialize, Serialize};

use crate::TICK_DT;
use crate::combat::PICKUP_REACH;
use crate::containers::{ContainerDef, ContainerSite};
use crate::firstaid::{Treatment, treating};
use crate::items::{BagCarry, GroundItem, ItemCategory, ItemDef, ItemStack};
use crate::players::{MoveMods, Player, Stance};
use crate::senses::SoundSource;
use crate::vision::clear_reach;
use crate::world::World;

/// How far a container can be from a player opening it (to the nearest edge of the furniture's
/// footprint), in the same room.
pub const CONTAINER_REACH: f32 = 1.6;
/// An open container stays open (and usable) until its player is this far from it, or leaves the
/// room: some slack for shuffling about in front of it.
pub const CONTAINER_KEEP_OPEN: f32 = 2.2;
/// E prefers a container within this angle of where the player faces (to the nearest point of the
/// furniture), nearest first; with none there, the nearest in reach.
const FACING_DEGREES: f32 = 60.0;
/// What E says with nothing on the floor within reach and no container to search.
pub const NOTHING_HERE: &str = "nothing to pick up or search here";
/// Item notes kept if nobody drains them.
pub const MAX_ITEM_NOTES: usize = 64;

/// Somewhere one item at a time goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Slot {
    /// The hand that swings: a weapon here is the one used.
    Hand,
    OffHand,
    /// A bag worn on the back.
    Back,
    /// A bag on the shoulder.
    Shoulder,
}

impl Slot {
    pub const ALL: [Slot; 4] = [Slot::Hand, Slot::OffHand, Slot::Back, Slot::Shoulder];
}

/// Where a stack is, for moving it. With [`Place::Carried`], [`Place::Worn`] and
/// [`Place::Container`] an index says which stack; with [`Place::Ground`] it is the ground item's id.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Place {
    /// In pockets and bags.
    Carried,
    Slot(Slot),
    Worn,
    /// In the container with this id (it must be open to the player).
    Container(u32),
    /// On the floor within reach.
    Ground,
}

/// Everything one player has.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Inventory {
    pub hand: Option<ItemStack>,
    pub off_hand: Option<ItemStack>,
    pub back: Option<ItemStack>,
    pub shoulder: Option<ItemStack>,
    pub worn: Vec<ItemStack>,
    /// In pockets and bags: at most the room they give.
    pub carried: Vec<ItemStack>,
}

impl Inventory {
    pub fn slot(&self, s: Slot) -> Option<ItemStack> {
        match s {
            Slot::Hand => self.hand,
            Slot::OffHand => self.off_hand,
            Slot::Back => self.back,
            Slot::Shoulder => self.shoulder,
        }
    }

    pub fn slot_mut(&mut self, s: Slot) -> &mut Option<ItemStack> {
        match s {
            Slot::Hand => &mut self.hand,
            Slot::OffHand => &mut self.off_hand,
            Slot::Back => &mut self.back,
            Slot::Shoulder => &mut self.shoulder,
        }
    }

    /// Every stack, wherever it is.
    pub fn stacks(&self) -> impl Iterator<Item = &ItemStack> {
        Slot::ALL.into_iter().filter_map(|s| match s {
            Slot::Hand => self.hand.as_ref(),
            Slot::OffHand => self.off_hand.as_ref(),
            Slot::Back => self.back.as_ref(),
            Slot::Shoulder => self.shoulder.as_ref(),
        })
        .chain(&self.worn)
        .chain(&self.carried)
    }

    pub fn stacks_mut(&mut self) -> impl Iterator<Item = &mut ItemStack> {
        [&mut self.hand, &mut self.off_hand, &mut self.back, &mut self.shoulder].into_iter().filter_map(|s| s.as_mut()).chain(&mut self.worn).chain(&mut self.carried)
    }

    pub fn is_empty(&self) -> bool {
        self.stacks().next().is_none()
    }

    /// Everything out, in slot order then worn then carried.
    pub fn take_all(&mut self) -> Vec<ItemStack> {
        let all = self.stacks().copied().collect();
        *self = Self::default();
        all
    }

    /// Total weight (kg).
    pub fn weight(&self, items: &[ItemDef]) -> f32 {
        self.stacks().fold(0.0, |kg, s| kg + stack_weight(items, s))
    }

    /// Room for carried items (litres): pockets plus the bags in the slots.
    pub fn room(&self, items: &[ItemDef], pockets: f32) -> f32 {
        let bags: f32 = Slot::ALL.iter().filter_map(|&s| self.slot(s)).filter_map(|s| items.get(s.item as usize)?.bag.as_ref().map(|b| b.capacity_l)).sum();
        pockets + bags
    }

    /// Room the carried items take (litres).
    pub fn used(&self, items: &[ItemDef]) -> f32 {
        stacks_volume(items, &self.carried)
    }

    /// Speed lost to awkward bags (the sum of their `slow`).
    fn bag_slow(&self, items: &[ItemDef]) -> f32 {
        Slot::ALL.iter().filter_map(|&s| self.slot(s)).filter_map(|s| items.get(s.item as usize)?.bag.as_ref().map(|b| b.slow)).sum()
    }
}

pub fn stack_weight(items: &[ItemDef], s: &ItemStack) -> f32 {
    items.get(s.item as usize).map_or(0.0, |d| d.weight_kg * s.count as f32)
}

pub fn stacks_volume(items: &[ItemDef], stacks: &[ItemStack]) -> f32 {
    stacks.iter().fold(0.0, |l, s| l + items.get(s.item as usize).map_or(0.0, |d| d.volume_l * s.count as f32))
}

/// Add `count` of the kind of thing `like` is (its item, age and goes had; its own count doesn't
/// matter) to a list of stacks: topping up stacks of the same kind first, then new stacks.
pub fn merge_into(stacks: &mut Vec<ItemStack>, like: ItemStack, mut count: u16, stack_size: u16) {
    let size = stack_size.max(1);
    for s in stacks.iter_mut().filter(|s| s.same_kind(&like) && s.count < size) {
        let add = count.min(size - s.count);
        s.count += add;
        count -= add;
        if count == 0 {
            return;
        }
    }
    while count > 0 {
        let n = count.min(size);
        stacks.push(like.with_count(n));
        count -= n;
    }
}

/// How heavy a load is, as a share of the carry limit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Burden {
    /// Up to the limit: no effect.
    None,
    /// A little over: slower, running tires faster.
    Burdened,
    /// Well over: slower still, no running, swings cost more.
    Heavy,
    /// Far over: very slow, no running or climbing.
    Overloaded,
}

/// Carrying: limits, burden stages and how long handling things takes. Serde with defaults, saved
/// with the world like the other tunings. All numbers are ours.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CarryTuning {
    /// What a player carries without effect (kg)...
    pub limit_kg: f32,
    /// ...plus this per Strength level (#46).
    pub strength_kg: f32,
    /// Room in pockets (litres), always there.
    pub pockets_l: f32,
    /// Burden stages start at these shares of the limit.
    pub burdened_at: f32,
    pub heavy_at: f32,
    pub overloaded_at: f32,
    /// Nothing more can be picked up past this share of the limit.
    pub max_load: f32,
    pub burdened_speed: f32,
    pub burdened_run_drain: f32,
    pub heavy_speed: f32,
    /// Swings, shoves and stomps cost this much more endurance from Heavy on.
    pub heavy_action_cost: f32,
    pub overloaded_speed: f32,
    /// Seconds to move one stack, and one heavier than `heavy_stack_kg`.
    pub move_secs: f32,
    pub heavy_move_secs: f32,
    pub heavy_stack_kg: f32,
    /// Seconds to open a container that has been searched before.
    pub reopen_secs: f32,
    /// Seconds to eat or drink something.
    pub use_secs: f32,
    /// Searching while sneaking takes this much longer, and makes this share of the noise.
    pub sneak_search_mult: f32,
    pub sneak_noise_mult: f32,
    /// Jobs a player can have queued.
    pub max_jobs: u32,
}

impl Default for CarryTuning {
    fn default() -> Self {
        Self {
            limit_kg: 14.0,
            strength_kg: 1.5,
            pockets_l: 3.0,
            burdened_at: 1.0,
            heavy_at: 1.3,
            overloaded_at: 1.6,
            max_load: 2.0,
            burdened_speed: 0.9,
            burdened_run_drain: 1.3,
            heavy_speed: 0.75,
            heavy_action_cost: 1.2,
            overloaded_speed: 0.5,
            move_secs: 0.3,
            heavy_move_secs: 0.6,
            heavy_stack_kg: 3.0,
            reopen_secs: 0.3,
            use_secs: 1.0,
            sneak_search_mult: 1.5,
            sneak_noise_mult: 0.5,
            max_jobs: 16,
        }
    }
}

impl CarryTuning {
    /// The carry limit (kg) at a Strength level (#46).
    pub fn limit(&self, strength: u32) -> f32 {
        self.limit_kg + self.strength_kg * strength as f32
    }

    /// The burden stage of a load (`ratio` = weight / limit).
    pub fn burden(&self, ratio: f32) -> Burden {
        if ratio > self.overloaded_at {
            Burden::Overloaded
        } else if ratio > self.heavy_at {
            Burden::Heavy
        } else if ratio > self.burdened_at {
            Burden::Burdened
        } else {
            Burden::None
        }
    }

    /// What a load does to movement; `bag_slow` is the speed awkward bags take off.
    pub fn mods(&self, ratio: f32, bag_slow: f32) -> MoveMods {
        let base = MoveMods::default();
        let mut m = match self.burden(ratio) {
            Burden::None => base,
            Burden::Burdened => MoveMods { speed: self.burdened_speed, run_drain: self.burdened_run_drain, ..base },
            Burden::Heavy => MoveMods { speed: self.heavy_speed, run_drain: self.burdened_run_drain, can_run: false, ..base },
            Burden::Overloaded => MoveMods { speed: self.overloaded_speed, run_drain: self.burdened_run_drain, can_run: false, can_climb: false, ..base },
        };
        m.speed *= (1.0 - bag_slow).clamp(0.1, 1.0);
        m
    }
}

/// What a player's load comes to.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Carry {
    pub weight_kg: f32,
    pub limit_kg: f32,
    /// Room for carried items, and how much of it is used (litres).
    pub room_l: f32,
    pub used_l: f32,
    pub burden: Burden,
}

/// Something a player does with items that takes time.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum JobKind {
    /// Move `count` (0 = all) of the stack at `from`/`index`, which held `item` when asked, to `to`.
    /// `spill`: if it doesn't fit there, it goes on the floor (taking something off).
    Move { from: Place, index: u32, item: u16, to: Place, count: u16, spill: bool },
    /// Open a container (searching it the first time, which fills it).
    Search { container: u32 },
    /// Eat or drink one of the stack at `from`/`index`.
    Use { from: Place, index: u32, item: u16 },
    /// First aid (#10): `treatment` on wound `wound` (`None` for pills) with one use of `item`
    /// (`None`: taking a dressing off). The player stands still while it's under way.
    Treat { wound: Option<u32>, treatment: Treatment, item: Option<u16> },
    /// Crafting (#12): make recipe `recipe` (an index into `WorldConfig::recipes`) from what the
    /// player carries and holds (see [`crate::crafting`]).
    Craft { recipe: u16 },
}

/// A queued job and the seconds it has left (only the first counts down).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ItemJob {
    pub kind: JobKind,
    pub left: f32,
}

/// What happened to a player's request (done, or why not), for its client and agents. An event,
/// not simulation state: drained into snapshots like combat events.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ItemNote {
    pub player: u32,
    pub ok: bool,
    pub text: String,
}

/// The stack at a place, resolved for a job: it must still hold `item`; if the index moved, the
/// first stack of that item there.
fn resolve(stacks: &[ItemStack], index: u32, item: u16) -> Option<usize> {
    match stacks.get(index as usize) {
        Some(s) if s.item == item => Some(index as usize),
        _ => stacks.iter().position(|s| s.item == item),
    }
}

fn lower(name: &str) -> String {
    name.to_lowercase()
}

impl World {
    /// An item's index in `config.items`, by id.
    pub fn item_index(&self, id: &str) -> Option<u16> {
        self.config.items.iter().position(|d| d.id == id).map(|i| i as u16)
    }

    pub fn item_def(&self, item: u16) -> &ItemDef {
        &self.config.items[item as usize]
    }

    /// The weapon an item swings as (index into `config.weapons`).
    pub fn item_weapon(&self, item: u16) -> Option<u16> {
        self.item_weapons.get(item as usize).copied().flatten()
    }

    /// Every container in the map, in id order.
    pub fn containers(&self) -> &[ContainerSite] {
        &self.sites
    }

    pub fn container(&self, id: u32) -> Option<&ContainerSite> {
        self.sites.binary_search_by_key(&id, |s| s.id).ok().map(|k| &self.sites[k])
    }

    pub fn container_def(&self, site: &ContainerSite) -> &ContainerDef {
        &self.config.containers.containers[site.def as usize]
    }

    /// What a container holds, if it has been opened (filled); `None` before anyone searched it.
    pub fn container_contents(&self, id: u32) -> Option<&[ItemStack]> {
        self.opened.get(&id).map(Vec::as_slice)
    }

    /// The containers opened so far, by id.
    pub fn opened_containers(&self) -> impl Iterator<Item = (u32, &[ItemStack])> + '_ {
        self.opened.iter().map(|(&id, v)| (id, v.as_slice()))
    }

    /// What player `p`'s load comes to.
    pub fn carry(&self, p: &Player) -> Carry {
        let items = &self.config.items;
        let t = &self.config.carry;
        let weight_kg = p.inventory.weight(items);
        // Strength (#46) adds to the limit.
        let limit_kg = t.limit(p.skills.level(crate::skills::Skill::Strength, &self.config.skills));
        Carry { weight_kg, limit_kg, room_l: p.inventory.room(items, t.pockets_l), used_l: p.inventory.used(items), burden: t.burden(weight_kg / limit_kg.max(0.1)) }
    }

    /// After player `pi`'s inventory changed: the weapon in hand, the movement modifiers of its load,
    /// and its client to be told.
    pub(crate) fn refresh_carry(&mut self, pi: usize) {
        // Standing still for first aid stays as it is: only `step_item_jobs` changes it. (Asleep
        // is still too, but that comes from the needs, #11.)
        let p = &self.players[pi];
        let still = p.state.mods.speed == 0.0 && treating(&p.jobs);
        self.refresh_carry_as(pi, still);
    }

    /// [`World::refresh_carry`], standing still for first aid (#10) or not. Treating starts and
    /// ends only in [`World::step_item_jobs`], after movement, so the modifiers a snapshot carries
    /// are always the ones the next tick moves with and client prediction stays exact.
    fn refresh_carry_as(&mut self, pi: usize, still: bool) {
        let p = &self.players[pi];
        let c = self.carry(p);
        let slow = p.inventory.bag_slow(&self.config.items);
        let fitness = p.skills.level(crate::skills::Skill::Fitness, &self.config.skills);
        let load = self.config.skills.move_mods(fitness, self.config.carry.mods(c.weight_kg / c.limit_kg.max(0.1), slow));
        let mut mods = self.needs_mods(pi, self.health_mods(pi, load));
        if still && !p.dead {
            mods = MoveMods { speed: 0.0, run_speed: 0.0, can_run: false, can_climb: false, ..mods };
        }
        let weapon = p.inventory.hand.and_then(|s| self.item_weapon(s.item));
        let p = &mut self.players[pi];
        p.state.mods = mods;
        p.weapon = weapon;
        self.private_dirty.insert(p.id);
    }

    /// Every player's derived carry state (after loading).
    pub(crate) fn refresh_all_carry(&mut self) {
        for pi in 0..self.players.len() {
            let still = treating(&self.players[pi].jobs);
            self.refresh_carry_as(pi, still);
        }
    }

    /// Player ids whose inventory, open container or queue changed since the last
    /// [`take_private_dirty`](Self::take_private_dirty) (replication; not simulation state).
    pub fn take_private_dirty(&mut self) -> BTreeSet<u32> {
        std::mem::take(&mut self.private_dirty)
    }

    /// Ask for every player's private view to be sent again (a client (re)joining).
    pub fn mark_all_private_dirty(&mut self) {
        let ids: Vec<u32> = self.players.iter().map(|p| p.id).collect();
        self.private_dirty.extend(ids);
    }

    pub(crate) fn note(&mut self, player: u32, ok: bool, text: String) {
        if self.item_notes.len() >= MAX_ITEM_NOTES {
            self.item_notes.remove(0);
        }
        self.item_notes.push(ItemNote { player, ok, text });
    }

    /// Can player `p` open container `site`? Same room, within [`CONTAINER_REACH`] of the nearest
    /// edge of the furniture.
    fn reaches(&self, p: &Player, site: &ContainerSite) -> bool {
        self.within(p, site, CONTAINER_REACH)
    }

    /// Is container `site` still usable by player `p`, which has it open? Same room, within
    /// [`CONTAINER_KEEP_OPEN`] of the furniture.
    fn keeps_open(&self, p: &Player, site: &ContainerSite) -> bool {
        p.open == Some(site.id) && self.within(p, site, CONTAINER_KEEP_OPEN)
    }

    fn within(&self, p: &Player, site: &ContainerSite, reach: f32) -> bool {
        !p.dead && self.map.room_at(p.state.pos) == site.room && site.distance(p.state.pos) <= reach
    }

    /// The container player `id` faces within reach, if any: of those within [`FACING_DEGREES`]
    /// of its facing (toward the nearest point of the furniture; one it stands against counts as
    /// faced), the nearest; else the nearest in reach. Ties go to the lower id.
    pub fn container_in_reach(&self, id: u32) -> Option<u32> {
        let p = self.player(id)?;
        let (pos, ahead) = (p.state.pos, Vec2::from_angle(p.state.facing));
        let cone = FACING_DEGREES.to_radians().cos();
        let faced = |s: &ContainerSite| {
            let lo = s.min.as_vec2();
            let to = pos.clamp(lo, lo + s.size.as_vec2()) - pos;
            to.length() < 0.05 || ahead.dot(to.normalize()) >= cone
        };
        self.sites
            .iter()
            .filter(|s| self.reaches(p, s))
            .min_by(|a, b| (!faced(a)).cmp(&!faced(b)).then(a.distance(pos).total_cmp(&b.distance(pos))).then(a.id.cmp(&b.id)))
            .map(|s| s.id)
    }

    /// The containers player `id` can reach, nearest first.
    pub fn containers_in_reach(&self, id: u32) -> Vec<u32> {
        let Some(p) = self.player(id) else { return Vec::new() };
        let mut v: Vec<&ContainerSite> = self.sites.iter().filter(|s| self.reaches(p, s)).collect();
        v.sort_by(|a, b| a.distance(p.state.pos).total_cmp(&b.distance(p.state.pos)).then(a.id.cmp(&b.id)));
        v.into_iter().map(|s| s.id).collect()
    }

    /// The ground items player `id` can reach (its floor), nearest first.
    pub fn floor_of(&self, id: u32) -> Vec<GroundItem> {
        let Some(p) = self.player(id) else { return Vec::new() };
        let from = p.state.pos;
        let mut v: Vec<GroundItem> = self.items.iter().filter(|it| self.floor_reaches(p, it)).copied().collect();
        v.sort_by(|a, b| a.pos.distance(from).total_cmp(&b.pos.distance(from)).then(a.id.cmp(&b.id)));
        v
    }

    /// Can player `p` take ground item `it`? Within [`PICKUP_REACH`] of its feet with no wall,
    /// shut window or door or fence between (furniture doesn't stop a hand: [`clear_reach`]). The
    /// one rule for the floor: [`World::floor_of`], moves from the floor and `PickUp` all use it,
    /// and snapshots send each player its floor by it.
    fn floor_reaches(&self, p: &Player, it: &GroundItem) -> bool {
        !p.dead && it.pos.distance(p.state.pos) <= PICKUP_REACH && clear_reach(&self.map, p.state.pos, it.pos)
    }

    /// The ids of the ground items player `id` can reach, nearest first (its floor, for snapshots).
    pub fn floor_ids(&self, id: u32) -> Vec<u32> {
        self.floor_of(id).into_iter().map(|it| it.id).collect()
    }

    /// The stack at `from`/`index` for player `pi` right now (for queueing: what item it is).
    fn stack_at(&self, pi: usize, from: Place, index: u32) -> Option<ItemStack> {
        let p = &self.players[pi];
        match from {
            Place::Carried => p.inventory.carried.get(index as usize).copied(),
            Place::Worn => p.inventory.worn.get(index as usize).copied(),
            Place::Slot(s) => p.inventory.slot(s),
            Place::Container(c) => (p.open == Some(c)).then(|| self.opened.get(&c)?.get(index as usize).copied()).flatten(),
            Place::Ground => self.items.iter().find(|it| it.id == index && self.floor_reaches(p, it)).map(|it| it.stack),
        }
    }

    /// Seconds a move of `n` of `item` takes.
    fn move_secs(&self, item: u16, n: u16) -> f32 {
        let t = &self.config.carry;
        if self.item_def(item).weight_kg * n as f32 > t.heavy_stack_kg { t.heavy_move_secs } else { t.move_secs }
    }

    /// Queue a job for player `id`, checking what can be checked now. Refusals are noted for its
    /// client and returned.
    pub(crate) fn queue_job(&mut self, id: u32, kind: JobKind, secs: f32) -> Result<(), String> {
        let Some(pi) = self.players.iter().position(|p| p.id == id) else { return Err(format!("no player {id}")) };
        let max = self.config.carry.max_jobs as usize;
        let p = &mut self.players[pi];
        let refuse = if p.dead {
            Some("dead".to_string())
        } else if p.jobs.len() >= max {
            Some(format!("busy: {max} things queued already"))
        } else {
            None
        };
        if let Some(why) = refuse {
            self.note(id, false, why.clone());
            return Err(why);
        }
        p.jobs.push(ItemJob { kind, left: secs.max(0.0) });
        self.private_dirty.insert(id);
        Ok(())
    }

    pub(crate) fn refuse<T>(&mut self, id: u32, why: String) -> Result<T, String> {
        self.note(id, false, why.clone());
        Err(why)
    }

    /// Player `id` moves `count` (0 = all) of the stack at `from`/`index` to `to`. Queued: it happens
    /// after [`CarryTuning::move_secs`] (if it still can). Returns whether it was queued.
    pub fn player_move_item(&mut self, id: u32, from: Place, index: u32, to: Place, count: u16) -> Result<(), String> {
        self.queue_move(id, from, index, to, count, false)
    }

    fn queue_move(&mut self, id: u32, from: Place, index: u32, to: Place, count: u16, spill: bool) -> Result<(), String> {
        let Some(pi) = self.players.iter().position(|p| p.id == id) else { return Err(format!("no player {id}")) };
        if from == to && !matches!(from, Place::Ground) {
            return self.refuse(id, "it's already there".into());
        }
        let Some(stack) = self.stack_at(pi, from, index) else { return self.refuse(id, self.missing(pi, from, index)) };
        // Asked again for what is already on its way (a repeated click, a request crossing the
        // last snapshot): nothing more to do, and nothing to complain about.
        if self.queued_from(pi, from, index, stack.item) >= stack.count as u32 {
            return Ok(());
        }
        let n = if count == 0 { stack.count } else { count.min(stack.count) };
        let secs = self.move_secs(stack.item, n);
        self.queue_job(id, JobKind::Move { from, index, item: stack.item, to, count, spill }, secs)
    }

    /// How many of the stack at `from`/`index` (holding `item`) player `pi` has queued moves for
    /// already (a whole-stack move counts as all of it).
    fn queued_from(&self, pi: usize, from: Place, index: u32, item: u16) -> u32 {
        let whole = |count: u16| if count == 0 || matches!(from, Place::Slot(_) | Place::Worn) { u16::MAX as u32 } else { count as u32 };
        self.players[pi].jobs.iter().map(|j| match j.kind {
            JobKind::Move { from: f, index: i, item: it, count, .. } if f == from && i == index && it == item => whole(count),
            _ => 0,
        }).sum()
    }

    /// Why there's nothing to move at `from`/`index` for player `pi`, in words.
    fn missing(&self, pi: usize, from: Place, index: u32) -> String {
        let p = &self.players[pi];
        match from {
            Place::Ground => match self.items.iter().find(|it| it.id == index) {
                Some(it) => format!("the {} is out of reach", lower(&self.item_def(it.stack.item).name)),
                None => "that's no longer on the floor".into(),
            },
            Place::Container(c) if p.open != Some(c) => "that container isn't open (search it first)".into(),
            Place::Container(_) => "that's no longer in there".into(),
            Place::Slot(_) => "nothing there".into(),
            Place::Carried | Place::Worn => "you don't have that any more".into(),
        }
    }

    /// Player `id` puts the carried stack `index` where it belongs: a bag in its slot (a hand bag in
    /// the off hand if the hand is busy), clothing on, anything else in the hand.
    pub fn player_equip(&mut self, id: u32, index: u32) -> Result<(), String> {
        let Some(p) = self.player(id) else { return Err(format!("no player {id}")) };
        let Some(stack) = p.inventory.carried.get(index as usize).copied() else { return self.refuse(id, format!("no carried stack {index}")) };
        let def = self.item_def(stack.item);
        let to = match (&def.bag, &def.clothing) {
            (Some(b), _) => Place::Slot(match b.carried {
                BagCarry::Back => Slot::Back,
                BagCarry::Shoulder => Slot::Shoulder,
                BagCarry::Hand if p.inventory.hand.is_some() || def.two_handed => Slot::OffHand,
                BagCarry::Hand => Slot::Hand,
            }),
            (None, Some(_)) => Place::Worn,
            _ => Place::Slot(Slot::Hand),
        };
        self.queue_move(id, Place::Carried, index, to, 1, false)
    }

    /// Player `id` puts on the carried clothing `index`.
    pub fn player_wear(&mut self, id: u32, index: u32) -> Result<(), String> {
        let Some(p) = self.player(id) else { return Err(format!("no player {id}")) };
        match p.inventory.carried.get(index as usize) {
            Some(s) if self.item_def(s.item).clothing.is_some() => self.queue_move(id, Place::Carried, index, Place::Worn, 1, false),
            Some(s) => {
                let name = self.item_def(s.item).name.clone();
                self.refuse(id, format!("can't wear the {}", lower(&name)))
            }
            None => self.refuse(id, format!("no carried stack {index}")),
        }
    }

    /// Player `id` takes off or lets go of what's at `from` (a slot or a worn index) into its
    /// pockets and bags, or onto the floor if there's no room.
    pub fn player_unequip(&mut self, id: u32, from: Place, index: u32) -> Result<(), String> {
        if !matches!(from, Place::Slot(_) | Place::Worn) {
            return self.refuse(id, "only held, slung and worn things are taken off".into());
        }
        self.queue_move(id, from, index, Place::Carried, 0, true)
    }

    /// Player `id` drops `count` (0 = all) of the stack at `from`/`index` at its feet.
    pub fn player_drop(&mut self, id: u32, from: Place, index: u32, count: u16) -> Result<(), String> {
        if matches!(from, Place::Ground | Place::Container(_)) {
            return self.refuse(id, "drop only what you have".into());
        }
        self.queue_move(id, from, index, Place::Ground, count, false)
    }

    /// Player `id` eats or drinks one of the stack at `from`/`index` (carried or held).
    pub fn player_use(&mut self, id: u32, from: Place, index: u32) -> Result<(), String> {
        let Some(pi) = self.players.iter().position(|p| p.id == id) else { return Err(format!("no player {id}")) };
        if !matches!(from, Place::Carried | Place::Slot(Slot::Hand) | Place::Slot(Slot::OffHand)) {
            return self.refuse(id, "use what you carry or hold".into());
        }
        let Some(stack) = self.stack_at(pi, from, index) else { return self.refuse(id, format!("nothing at {from:?} {index}")) };
        let def = self.item_def(stack.item);
        if def.consumable().is_none() {
            let name = def.name.clone();
            return self.refuse(id, format!("can't use the {} (yet)", lower(&name)));
        }
        if let Err(why) = self.can_consume(pi, stack.item) {
            return self.refuse(id, why);
        }
        let secs = self.use_secs(stack.item);
        self.queue_job(id, JobKind::Use { from, index, item: stack.item }, secs)
    }

    /// Player `id` searches (opens) container `container`, or the one it faces within reach. Takes the
    /// container's search time the first time (longer sneaking), a moment after that.
    pub fn player_search(&mut self, id: u32, container: Option<u32>) -> Result<(), String> {
        let Some(pi) = self.players.iter().position(|p| p.id == id) else { return Err(format!("no player {id}")) };
        let Some(c) = container.or_else(|| self.container_in_reach(id)) else {
            // E with no container about: the floor, if anything lies within reach (the client's
            // view may be a tick behind), else one plain answer.
            if !self.floor_of(id).is_empty() {
                return self.player_pick_up(id).map(|_| ()).ok_or_else(|| "couldn't pick it up".to_string());
            }
            return self.refuse(id, NOTHING_HERE.into());
        };
        let Some(site) = self.container(c).cloned() else { return self.refuse(id, format!("no container {c}")) };
        let p = &self.players[pi];
        let name = lower(&self.container_def(&site).name);
        if p.jobs.iter().any(|j| j.kind == JobKind::Search { container: c }) {
            // Already on it.
            return Ok(());
        }
        // Open already, it reopens (quickly) from as far as it would stay open.
        if !(self.reaches(p, &site) || self.keeps_open(p, &site)) {
            return self.refuse(id, format!("the {name} is out of reach"));
        }
        let t = &self.config.carry;
        let secs = if self.opened.contains_key(&c) {
            t.reopen_secs
        } else {
            let sneak = if self.players[pi].state.stance == Stance::Sneak { t.sneak_search_mult } else { 1.0 };
            // Scavenging (#46) makes first searches faster.
            let skill = self.config.skills.search_time_mult(self.skill_level(pi, crate::skills::Skill::Scavenging));
            self.container_def(&site).search_secs * sneak * skill
        };
        self.queue_job(id, JobKind::Search { container: c }, secs)
    }

    /// Count down every player's first job, and do the ones that are due; close containers players
    /// have walked away from. Runs each tick after movement.
    pub(crate) fn step_item_jobs(&mut self) {
        for pi in 0..self.players.len() {
            self.step_jobs_of(pi);
            // Treating (first aid) means standing still from the next tick, until it's done.
            let p = &self.players[pi];
            let still = treating(&p.jobs) && !p.dead;
            if still != (p.state.mods.speed == 0.0) {
                self.refresh_carry_as(pi, still);
            }
        }
    }

    fn step_jobs_of(&mut self, pi: usize) {
        {
            let p = &self.players[pi];
            if let Some(c) = p.open
                && self.container(c).is_none_or(|s| !self.keeps_open(p, s))
            {
                let id = p.id;
                self.players[pi].open = None;
                self.private_dirty.insert(id);
            }
            let p = &mut self.players[pi];
            if p.dead || p.jobs.is_empty() {
                return;
            }
            p.jobs[0].left -= TICK_DT;
            if p.jobs[0].left > 0.0 {
                return;
            }
            let job = p.jobs.remove(0);
            let id = p.id;
            self.private_dirty.insert(id);
            // For the run stats (`crate::run`): things taken from a container or the floor, and
            // containers searched first.
            let held = p.inventory.stacks().map(|s| s.count as u32).sum::<u32>();
            let fresh = matches!(job.kind, JobKind::Search { container } if !self.opened.contains_key(&container));
            let result = match job.kind {
                JobKind::Move { from, index, item, to, count, spill } => self.do_move(pi, from, index, item, to, count, spill),
                JobKind::Search { container } => self.do_search(pi, container),
                JobKind::Use { from, index, item } => self.do_use(pi, from, index, item),
                JobKind::Treat { wound, treatment, item } => self.do_treat(pi, wound, treatment, item),
                JobKind::Craft { recipe } => self.do_craft(pi, recipe),
            };
            if result.is_ok() {
                let p = &mut self.players[pi];
                match job.kind {
                    JobKind::Move { from: Place::Container(_) | Place::Ground, .. } => {
                        p.run.items_taken += p.inventory.stacks().map(|s| s.count as u32).sum::<u32>().saturating_sub(held);
                    }
                    JobKind::Search { .. } if fresh => p.run.searched += 1,
                    _ => {}
                }
                if fresh {
                    // Scavenging (#46): a container searched for the first time.
                    self.award_xp(pi, crate::skills::Skill::Scavenging, self.config.skills.search_xp);
                }
            }
            match result {
                Ok(text) => self.note(id, true, text),
                Err(why) => self.note(id, false, why),
            }
        }
    }

    /// Drop player `pi`'s queue (it swung, shoved or stomped, or died).
    pub(crate) fn cancel_jobs(&mut self, pi: usize) {
        if !self.players[pi].jobs.is_empty() {
            self.players[pi].jobs.clear();
            self.private_dirty.insert(self.players[pi].id);
        }
    }

    fn do_search(&mut self, pi: usize, c: u32) -> Result<String, String> {
        let site = self.container(c).cloned().ok_or_else(|| format!("no container {c}"))?;
        let def = self.container_def(&site).clone();
        let p = &self.players[pi];
        if !(self.reaches(p, &site) || self.keeps_open(p, &site)) {
            return Err(format!("the {} is out of reach", lower(&def.name)));
        }
        let first = !self.opened.contains_key(&c);
        if first {
            let room_type = self.map.rooms.get(site.room as usize).map_or("", |r| r.room_type.as_str()).to_string();
            let mut contents = crate::loot::fill(self.config.seed(), c, &def, &room_type, &self.config.loot, &self.config.items);
            // Food found was made before the world began (#11), some of it a while before.
            self.age_found_food(c, &mut contents);
            self.opened.insert(c, contents);
        }
        let p = &self.players[pi];
        let (id, sneaking, at) = (p.id, p.state.stance == Stance::Sneak, site.centre());
        let quieter = if first { self.config.skills.search_noise_mult(self.skill_level(pi, crate::skills::Skill::Scavenging)) } else { 1.0 };
        let noise = def.noise_m * quieter * if sneaking { self.config.carry.sneak_noise_mult } else { 1.0 };
        if noise > 0.0 {
            self.emit_sound_from(at, noise, SoundSource::Player(id));
        }
        // The one container open to this player; anyone else with it open sees the same contents.
        self.players[pi].open = Some(c);
        let n = self.opened[&c].len();
        Ok(format!("{} the {}: {}", if first { "searched" } else { "opened" }, lower(&def.name), if n == 0 { "empty".to_string() } else { format!("{n} stacks") }))
    }

    fn do_use(&mut self, pi: usize, from: Place, index: u32, item: u16) -> Result<String, String> {
        let name = lower(&self.item_def(item).name);
        self.can_consume(pi, item)?;
        let uses = self.item_def(item).consumable().map_or(1, |c| c.uses.max(1));
        // One go: the top one of the stack, used up after its last go.
        let go = |s: &mut ItemStack| {
            s.used += 1;
            if s.used >= uses {
                s.used = 0;
                s.count -= 1;
            }
        };
        let mut inv = self.players[pi].inventory.clone();
        let had = match from {
            Place::Carried => {
                let k = resolve(&inv.carried, index, item).ok_or_else(|| format!("the {name} is gone"))?;
                let had = inv.carried[k];
                go(&mut inv.carried[k]);
                if inv.carried[k].count == 0 {
                    inv.carried.remove(k);
                }
                had
            }
            Place::Slot(s) => match inv.slot(s) {
                Some(mut st) if st.item == item => {
                    let had = st;
                    go(&mut st);
                    *inv.slot_mut(s) = (st.count > 0).then_some(st);
                    had
                }
                _ => return Err(format!("the {name} is gone")),
            },
            _ => return Err("use what you carry or hold".into()),
        };
        let p = &mut self.players[pi];
        p.inventory = inv;
        let id = p.id;
        self.used_items.push((id, item));
        let effect = self.consume(pi, had);
        self.refresh_carry(pi);
        let verb = if self.item_def(item).category == ItemCategory::Drink { "drank" } else { "ate" };
        let what = match uses - had.used - 1 {
            _ if uses == 1 => format!("the {name}"),
            0 => format!("the last of the {name}"),
            n => format!("some of the {name} ({n} of {uses} goes left)"),
        };
        Ok(format!("{verb} {what}: {effect}"))
    }

    /// Carry out a move. Atomic: works on copies and commits only if everything fits.
    #[allow(clippy::too_many_arguments)]
    fn do_move(&mut self, pi: usize, from: Place, index: u32, item: u16, to: Place, count: u16, spill: bool) -> Result<String, String> {
        let def = self.item_def(item).clone();
        let name = lower(&def.name);
        let p = &self.players[pi];
        let (pos, facing) = (p.state.pos, p.state.facing);
        let mut inv = p.inventory.clone();
        let items = &self.config.items;
        let before = (inv.weight(items), inv.used(items) - inv.room(items, self.config.carry.pockets_l));
        // A slot or a layer takes one; otherwise `count` (0 = all).
        let one = matches!(to, Place::Slot(_) | Place::Worn);
        let take = |have: u16| if one { 1 } else if count == 0 { have } else { count.min(have) };
        let mut src_box: Option<(u32, Vec<ItemStack>)> = None;
        let mut ground_take: Option<(usize, u16)> = None;
        // What moves: `n` of the stack's kind (a part-eaten top one stays behind unless all go).
        let part = |s: ItemStack, n: u16| ItemStack { used: if n >= s.count { s.used } else { 0 }, ..s.with_count(n) };
        let moved = match from {
            Place::Carried | Place::Worn => {
                let list = if from == Place::Carried { &mut inv.carried } else { &mut inv.worn };
                let k = resolve(list, index, item).ok_or_else(|| format!("the {name} is gone"))?;
                let n = take(list[k].count);
                let moved = part(list[k], n);
                list[k].count -= n;
                if list[k].count == 0 {
                    list.remove(k);
                }
                moved
            }
            Place::Slot(s) => match inv.slot(s) {
                Some(st) if st.item == item => {
                    *inv.slot_mut(s) = None;
                    st.with_count(1)
                }
                _ => return Err(format!("the {name} is gone")),
            },
            Place::Container(c) => {
                if !self.container(c).is_some_and(|s| self.keeps_open(p, s)) {
                    return Err("that container isn't open (or is out of reach)".into());
                }
                let mut list = self.opened.get(&c).cloned().unwrap_or_default();
                let k = resolve(&list, index, item).ok_or_else(|| format!("the {name} is gone"))?;
                let n = take(list[k].count);
                let moved = part(list[k], n);
                list[k].count -= n;
                if list[k].count == 0 {
                    list.remove(k);
                }
                src_box = Some((c, list));
                moved
            }
            Place::Ground => {
                let k = self.items.iter().position(|it| it.id == index).ok_or_else(|| format!("the {name} is gone"))?;
                if self.items[k].stack.item != item || !self.floor_reaches(p, &self.items[k]) {
                    return Err(format!("the {name} is out of reach"));
                }
                let n = take(self.items[k].stack.count);
                ground_take = Some((k, n));
                part(self.items[k].stack, n)
            }
        };
        self.finish_move(pi, inv, before, src_box, ground_take, def, moved, from, to, spill, pos, facing)
    }

    /// The second half of a move: put `moved` (`n` of a kind of item) at `to`, check the result and commit.
    #[allow(clippy::too_many_arguments)]
    fn finish_move(
        &mut self,
        pi: usize,
        mut inv: Inventory,
        before: (f32, f32),
        mut src_box: Option<(u32, Vec<ItemStack>)>,
        ground_take: Option<(usize, u16)>,
        def: ItemDef,
        moved: ItemStack,
        from: Place,
        to: Place,
        spill: bool,
        pos: Vec2,
        facing: f32,
    ) -> Result<String, String> {
        let n = moved.count;
        let items = &self.config.items;
        let name = lower(&def.name);
        let p = &self.players[pi];
        let open_ok = |c: u32| self.container(c).is_some_and(|s| self.keeps_open(p, s));
        let mut dst_box: Option<(u32, Vec<ItemStack>)> = None;
        let mut to_ground: Vec<ItemStack> = Vec::new();
        // Whatever a slot or a layer had in the way goes into pockets and bags.
        let mut displaced: Vec<ItemStack> = Vec::new();
        let mut to = to;
        match to {
            Place::Carried => merge_into(&mut inv.carried, moved, n, def.stack),
            Place::Slot(s) => {
                let ok = match s {
                    Slot::Hand | Slot::OffHand => true,
                    Slot::Back => def.bag.as_ref().is_some_and(|b| b.carried == BagCarry::Back),
                    Slot::Shoulder => def.bag.as_ref().is_some_and(|b| b.carried == BagCarry::Shoulder),
                };
                if !ok {
                    return Err(format!("the {name} doesn't go on your {}", if s == Slot::Back { "back" } else { "shoulder" }));
                }
                displaced.extend(inv.slot_mut(s).take());
                match s {
                    // Both hands for a two-handed item; one already in both hands lets go.
                    Slot::Hand if def.two_handed => displaced.extend(inv.off_hand.take()),
                    Slot::OffHand if inv.hand.is_some_and(|h| items[h.item as usize].two_handed) => displaced.extend(inv.hand.take()),
                    _ => {}
                }
                *inv.slot_mut(s) = Some(moved.with_count(1));
            }
            Place::Worn => {
                let Some(cl) = &def.clothing else { return Err(format!("can't wear the {name}")) };
                let clash = |s: &ItemStack| items[s.item as usize].clothing.as_ref().is_some_and(|o| o.layer == cl.layer && o.covers.iter().any(|b| cl.covers.contains(b)));
                displaced.extend(inv.worn.iter().copied().filter(|s| clash(s)));
                inv.worn.retain(|s| !clash(s));
                inv.worn.push(moved.with_count(1));
            }
            Place::Container(c) => {
                if !open_ok(c) {
                    return Err("that container isn't open (or is out of reach)".into());
                }
                let site = self.container(c).expect("open");
                let room = self.container_def(site).room_l;
                let mut list = match &src_box {
                    Some((sc, l)) if *sc == c => l.clone(),
                    _ => self.opened.get(&c).cloned().unwrap_or_default(),
                };
                if stacks_volume(items, &list) + def.volume_l * n as f32 > room + 1e-4 {
                    return Err(format!("no room for the {name} in the {}", lower(&self.container_def(site).name)));
                }
                merge_into(&mut list, moved, n, def.stack);
                if src_box.as_ref().is_some_and(|(sc, _)| *sc == c) {
                    src_box = None;
                }
                dst_box = Some((c, list));
            }
            Place::Ground => to_ground.push(moved),
        }
        for d in displaced {
            merge_into(&mut inv.carried, d, d.count, items[d.item as usize].stack);
        }
        // Does it all fit?
        let pockets = self.config.carry.pockets_l;
        let over = |inv: &Inventory| inv.used(items) - inv.room(items, pockets);
        let worse = |inv: &Inventory| {
            let o = over(inv);
            o > 1e-4 && o > before.1 + 1e-4
        };
        if worse(&inv) {
            if spill && to == Place::Carried {
                // Taking it off with no room for it: it goes on the floor instead.
                let k = inv.carried.iter().rposition(|s| s.same_kind(&moved)).expect("just added");
                inv.carried[k].count -= n.min(inv.carried[k].count);
                if inv.carried[k].count == 0 {
                    inv.carried.remove(k);
                }
                to_ground.push(moved);
                to = Place::Ground;
            }
            if worse(&inv) {
                return Err(match from {
                    Place::Slot(Slot::Back | Slot::Shoulder) => format!("empty the {name} first: what's in it won't fit without it"),
                    _ => format!("no room for the {name} (pockets and bags full)"),
                });
            }
        }
        let weight = inv.weight(items);
        let max = self.config.carry.max_load * self.carry(p).limit_kg;
        if weight > before.0 + 1e-4 && weight > max {
            return Err(format!("too heavy: can't carry the {name} as well ({weight:.1} of at most {max:.1} kg)"));
        }

        // Commit.
        self.players[pi].inventory = inv;
        for (c, list) in [src_box, dst_box].into_iter().flatten() {
            self.opened.insert(c, list);
            self.container_changed(c);
        }
        if let Some((k, take)) = ground_take {
            if take >= self.items[k].stack.count {
                self.items.remove(k);
            } else {
                self.items[k].stack.count -= take;
            }
        }
        for s in to_ground {
            self.put_on_floor(s, pos, facing);
        }
        self.refresh_carry(pi);
        let what = if n > 1 { format!("{n} x {name}") } else { name };
        Ok(match to {
            Place::Carried => format!("took the {what}"),
            Place::Slot(Slot::Hand) => format!("holding the {what}"),
            Place::Slot(Slot::OffHand) => format!("holding the {what} in the other hand"),
            Place::Slot(Slot::Back) => format!("put the {what} on your back"),
            Place::Slot(Slot::Shoulder) => format!("slung the {what} over your shoulder"),
            Place::Worn => format!("put on the {what}"),
            Place::Container(c) => format!("put the {what} in the {}", lower(&self.container_def(self.container(c).expect("exists")).name)),
            Place::Ground => format!("dropped the {what}"),
        })
    }

    /// Players with container `c` open hear about its new contents.
    fn container_changed(&mut self, c: u32) {
        let ids: Vec<u32> = self.players.iter().filter(|p| p.open == Some(c)).map(|p| p.id).collect();
        self.private_dirty.extend(ids);
    }

    /// Put a stack on the floor at `pos`: onto a pile of the same item lying right there if it has
    /// room, else as a new ground item. Returns the ground item's id.
    pub(crate) fn put_on_floor(&mut self, s: ItemStack, pos: Vec2, angle: f32) -> u32 {
        let size = self.config.items.get(s.item as usize).map_or(1, |d| d.stack);
        if let Some(it) = self.items.iter_mut().find(|it| it.stack.same_kind(&s) && it.pos.distance(pos) < 0.25 && it.stack.count + s.count <= size) {
            it.stack.count += s.count;
            return it.id;
        }
        let id = self.next_item_id;
        self.next_item_id += 1;
        self.items.push(GroundItem { id, stack: s, pos, angle });
        id
    }

    /// Player `pi` died: everything it had falls where it lay, in a small heap.
    pub(crate) fn drop_everything(&mut self, pi: usize) {
        let p = &mut self.players[pi];
        let (id, pos, facing) = (p.id, p.state.pos, p.state.facing);
        let all = p.inventory.take_all();
        p.open = None;
        p.jobs.clear();
        let seed = self.config.seed();
        for (k, s) in all.into_iter().enumerate() {
            let roll = |j: u64| crate::rng::hash01(seed ^ 0xD20F, id as u64, self.tick, k as u64 * 2 + j);
            let off = if k == 0 { Vec2::ZERO } else { Vec2::from_angle(roll(0) * std::f32::consts::TAU) * (0.15 + 0.3 * roll(1)) };
            let at = self.map.resolve_circle(pos + off, 0.05);
            let at = if self.map.in_bounds(at.floor().as_ivec2()) { at } else { pos };
            self.put_on_floor(s, at, facing + k as f32);
        }
        self.refresh_carry(pi);
    }

    /// Debug: put `count` of an item (by id) on the ground. Returns the new (or topped-up) ground
    /// item's id.
    pub fn spawn_item(&mut self, item: &str, count: u16, pos: Vec2) -> Option<u32> {
        let i = self.item_index(item)?;
        if count == 0 || !pos.is_finite() || !self.map.in_bounds(pos.floor().as_ivec2()) {
            return None;
        }
        let size = self.item_def(i).stack;
        let mut left = count.min(size.saturating_mul(10));
        let mut id = None;
        while left > 0 {
            let n = left.min(size);
            let gid = self.next_item_id;
            self.next_item_id += 1;
            self.items.push(GroundItem { id: gid, stack: self.fresh_stack(i, n), pos, angle: 0.0 });
            id.get_or_insert(gid);
            left -= n;
        }
        id
    }

    /// Debug and tests: put `count` of an item straight into player `id`'s pockets (no room or
    /// weight checks). Returns whether it did.
    pub fn give_item(&mut self, id: u32, item: &str, count: u16) -> bool {
        let Some(i) = self.item_index(item) else { return false };
        let Some(pi) = self.players.iter().position(|p| p.id == id) else { return false };
        let size = self.item_def(i).stack;
        let like = self.fresh_stack(i, 1);
        merge_into(&mut self.players[pi].inventory.carried, like, count.max(1), size);
        self.refresh_carry(pi);
        true
    }

    /// Debug and tests: put an item straight into a slot of player `id` (what was there is lost).
    pub fn hold_item(&mut self, id: u32, slot: Slot, item: &str) -> bool {
        let Some(i) = self.item_index(item) else { return false };
        let Some(pi) = self.players.iter().position(|p| p.id == id) else { return false };
        *self.players[pi].inventory.slot_mut(slot) = Some(ItemStack::one(i));
        self.refresh_carry(pi);
        true
    }

    /// Player `id` picks up the item nearest to where it faces within reach of its feet, at once.
    /// A weapon goes into its hand (what it held goes into its pockets and bags if it fits, else
    /// stays on the floor in its place); anything else into its pockets and bags, as many as fit.
    /// Returns the ground item id taken from.
    pub fn player_pick_up(&mut self, id: u32) -> Option<u32> {
        let pi = self.players.iter().position(|p| p.id == id)?;
        let p = &self.players[pi];
        if p.dead || p.action.is_some() {
            return None;
        }
        let aim = p.state.pos + Vec2::from_angle(p.state.facing) * 0.5;
        let k = self.items.iter().enumerate().filter(|(_, it)| self.floor_reaches(p, it)).min_by(|(_, a), (_, b)| a.pos.distance(aim).total_cmp(&b.pos.distance(aim))).map(|(k, _)| k)?;
        let it = self.items[k];
        let def = self.item_def(it.stack.item).clone();
        let (pos, facing) = (p.state.pos, p.state.facing);
        if self.item_weapon(it.stack.item).is_some() {
            let mut inv = p.inventory.clone();
            let mut out: Vec<ItemStack> = inv.hand.take().into_iter().collect();
            if def.two_handed {
                out.extend(inv.off_hand.take());
            }
            inv.hand = Some(it.stack.with_count(1));
            let items = &self.config.items;
            let max = self.config.carry.max_load * self.carry(p).limit_kg;
            let w0 = p.inventory.weight(items);
            if inv.weight(items) > w0 + 1e-4 && inv.weight(items) + out.iter().map(|s| stack_weight(items, s)).sum::<f32>() > max {
                self.note(id, false, format!("too heavy: can't carry the {} as well", lower(&def.name)));
                return None;
            }
            // What was held: into pockets and bags if there's room, else left where the new one lay.
            let mut left = Vec::new();
            for s in out {
                let mut trial = inv.clone();
                merge_into(&mut trial.carried, s, s.count, items[s.item as usize].stack);
                if trial.used(items) <= trial.room(items, self.config.carry.pockets_l) + 1e-4 && trial.weight(items) <= max {
                    inv = trial;
                } else {
                    left.push(s);
                }
            }
            if it.stack.count > 1 {
                self.items[k].stack.count -= 1;
            } else {
                self.items.remove(k);
            }
            for s in left {
                self.put_on_floor(s, pos, facing);
            }
            self.players[pi].inventory = inv;
            self.players[pi].run.items_taken += 1;
            self.refresh_carry(pi);
            self.note(id, true, format!("holding the {}", lower(&def.name)));
            return Some(it.id);
        }
        // As many as fit.
        let items = &self.config.items;
        let max = self.config.carry.max_load * self.carry(p).limit_kg;
        let mut inv = p.inventory.clone();
        let mut taken = 0;
        // Whole ones; a part-eaten top one only comes along with all the rest.
        let whole = ItemStack { used: 0, ..it.stack };
        for _ in 0..it.stack.count {
            let mut trial = inv.clone();
            merge_into(&mut trial.carried, whole, 1, def.stack);
            if trial.used(items) > trial.room(items, self.config.carry.pockets_l) + 1e-4 || trial.weight(items) > max {
                break;
            }
            inv = trial;
            taken += 1;
        }
        if taken == it.stack.count && it.stack.used > 0 {
            inv = p.inventory.clone();
            merge_into(&mut inv.carried, it.stack, taken, def.stack);
        }
        if taken == 0 {
            let full = inv.weight(items) + def.weight_kg > max;
            self.note(id, false, if full { format!("too heavy: can't carry the {} as well", lower(&def.name)) } else { format!("no room for the {} (pockets and bags full)", lower(&def.name)) });
            return None;
        }
        if taken >= it.stack.count {
            self.items.remove(k);
        } else {
            self.items[k].stack.count -= taken;
        }
        self.players[pi].inventory = inv;
        self.players[pi].run.items_taken += taken as u32;
        self.refresh_carry(pi);
        self.note(id, true, if taken > 1 { format!("took {taken} x {}", lower(&def.name)) } else { format!("took the {}", lower(&def.name)) });
        Some(it.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::items::{parse_items, parse_weapons};
    use crate::map::TileMap;
    use crate::players::PlayerInput;
    use crate::template::BuildingTemplate;
    use crate::world::WorldConfig;
    use glam::IVec2;

    fn content(seed: u64) -> WorldConfig {
        let templates = [include_str!("../../../assets/data/buildings/bungalow.ron"), include_str!("../../../assets/data/buildings/family_two_storey.ron")]
            .iter()
            .map(|t| BuildingTemplate::from_ron(t).unwrap())
            .collect();
        let weapons = parse_weapons(include_str!("../../../assets/data/items/weapons.ron")).unwrap();
        let items = parse_items(include_str!("../../../assets/data/items/items.ron"), &weapons).unwrap();
        let containers = crate::containers::parse_containers(include_str!("../../../assets/data/containers.ron")).unwrap();
        let mut c = WorldConfig::new(seed, 0, templates).with_weapons(weapons).with_items(items).with_containers(containers).with_loot(crate::loot::tests::shipped()).with_map_chunks(8);
        c.start_hour = 12.0;
        c
    }

    /// Open grass, one player at (60.5, 60.5) facing east, nothing on the floor.
    fn field() -> (World, u32) {
        let mut w = World::new(content(3));
        w.map = TileMap::new(w.map.width() / crate::map::CHUNK_SIZE, w.map.height() / crate::map::CHUNK_SIZE);
        w.map_changed();
        w.items.clear();
        w.sites.clear();
        let id = w.add_player(Vec2::new(60.5, 60.5));
        (w, id)
    }

    /// Step until player `id` has nothing queued (at most 10 s).
    fn settle(w: &mut World, id: u32) {
        for _ in 0..200 {
            if w.player(id).unwrap().jobs.is_empty() {
                return;
            }
            w.step();
        }
        panic!("jobs never finished");
    }

    fn me(w: &World, id: u32) -> &Player {
        w.player(id).unwrap()
    }

    fn last_note(w: &World, id: u32) -> String {
        w.item_notes.iter().rev().find(|n| n.player == id).map(|n| n.text.clone()).unwrap_or_default()
    }

    /// Picking up and dropping through the floor, with the time it takes and the room pockets have.
    #[test]
    fn floor_pockets_and_bags() {
        let (mut w, id) = field();
        let beans = w.item_index("tinned_beans").unwrap();
        let g = w.spawn_item("tinned_beans", 5, Vec2::new(61.0, 60.5)).unwrap();
        // Five tins are 2 L: they fit in 3 L of pockets. Moving takes 0.3 s.
        w.player_move_item(id, Place::Ground, g, Place::Carried, 0).unwrap();
        w.step();
        assert!(me(&w, id).inventory.carried.is_empty(), "not instant");
        settle(&mut w, id);
        assert_eq!(me(&w, id).inventory.carried, vec![ItemStack::new(beans, 5)]);
        assert!(w.items().is_empty());
        // A cushion (3 L) doesn't fit any more...
        let cushion = w.spawn_item("cushion", 1, Vec2::new(60.2, 60.5)).unwrap();
        w.player_move_item(id, Place::Ground, cushion, Place::Carried, 0).unwrap();
        settle(&mut w, id);
        assert!(last_note(&w, id).contains("no room"), "{}", last_note(&w, id));
        // ...until a backpack is on.
        let pack = w.spawn_item("school_backpack", 1, Vec2::new(60.6, 60.8)).unwrap();
        w.player_move_item(id, Place::Ground, pack, Place::Slot(Slot::Back), 0).unwrap();
        w.player_move_item(id, Place::Ground, cushion, Place::Carried, 0).unwrap();
        settle(&mut w, id);
        let c = w.carry(me(&w, id));
        assert_eq!((c.room_l, c.used_l), (23.0, 5.0), "{c:?}");
        // Taking the full backpack off would leave too much for the pockets.
        w.player_unequip(id, Place::Slot(Slot::Back), 0).unwrap();
        settle(&mut w, id);
        assert!(me(&w, id).inventory.back.is_some() && last_note(&w, id).contains("empty the"), "{}", last_note(&w, id));
        // Drop two tins: they land at your feet as one stack.
        w.player_drop(id, Place::Carried, 0, 2).unwrap();
        settle(&mut w, id);
        let floor = w.floor_of(id);
        assert_eq!(floor.len(), 1);
        assert_eq!(floor[0].stack, ItemStack::new(beans, 2));
        assert_eq!(me(&w, id).inventory.carried[0], ItemStack::new(beans, 3));
        // Out of reach: walk away and the floor is empty; moving from it fails.
        let far = floor[0].id;
        w.players[0].state.pos = Vec2::new(65.5, 60.5);
        assert!(w.floor_of(id).is_empty());
        assert!(w.player_move_item(id, Place::Ground, far, Place::Carried, 0).is_err());
    }

    /// The carry limit: over it you slow down and tire faster, much over it you can't run, and past
    /// the hard cap you can't take more.
    #[test]
    fn weight_slows_and_tires_and_has_a_cap() {
        let (mut w, id) = field();
        // No Strength (#46): the plain limit.
        w.players[0].skills.xp[crate::skills::Skill::Strength as usize] = 0.0;
        let t = w.config.carry.clone();
        let run = PlayerInput { move_dir: Vec2::Y, facing: 0.0, run: true, sneak: false };
        let trial = |w: &mut World, kg_bricks: u16| {
            let pi = w.players.iter().position(|p| p.id == id).unwrap();
            w.players[pi].inventory = Inventory::default();
            // Planks are 2 kg each; hold them straight in the carried list (room aside).
            let plank = w.item_index("wooden_plank").unwrap();
            w.players[pi].inventory.carried = (0..kg_bricks).map(|_| ItemStack::one(plank)).collect();
            w.refresh_carry(pi);
            w.players[pi].state.pos = Vec2::new(30.5, 20.5);
            w.players[pi].state.endurance = 1.0;
            for k in 0..20 {
                w.set_player_input(id, w.players[pi].last_input_seq + 1 + k, run);
                w.step();
            }
            let p = &w.players[pi];
            (p.state.pos.distance(Vec2::new(30.5, 20.5)), 1.0 - p.state.endurance, p.state.stance, w.carry(p).burden)
        };
        let (d0, e0, s0, b0) = trial(&mut w, 0);
        let (d1, e1, s1, b1) = trial(&mut w, 8); // 16 kg of 14: burdened
        let (d2, _, s2, b2) = trial(&mut w, 10); // 20 kg: heavy
        let (d3, _, _, b3) = trial(&mut w, 12); // 24 kg: overloaded
        assert_eq!((b0, b1, b2, b3), (Burden::None, Burden::Burdened, Burden::Heavy, Burden::Overloaded));
        assert_eq!((s0, s1, s2), (Stance::Run, Stance::Run, Stance::Walk));
        assert!((d1 / d0 - t.burdened_speed).abs() < 0.02, "burdened speed {d1} vs {d0}");
        assert!((e1 / e0 - t.burdened_run_drain).abs() < 0.05, "burdened drain {e1} vs {e0}");
        assert!(d2 < d1 && d3 < d2, "{d1} {d2} {d3}");
        // The hard cap: 28 kg. Holding 26 kg, a 2.5 kg axe is too much.
        trial(&mut w, 13);
        let axe = w.spawn_item("fire_axe", 1, w.players[0].state.pos).unwrap();
        assert_eq!(w.player_pick_up(id), None);
        assert!(last_note(&w, id).contains("too heavy"), "{}", last_note(&w, id));
        w.player_move_item(id, Place::Ground, axe, Place::Slot(Slot::Hand), 0).unwrap();
        settle(&mut w, id);
        assert!(me(&w, id).inventory.hand.is_none() && last_note(&w, id).contains("too heavy"));
        // Swings cost more endurance from Heavy on.
        let (mut light, light_id) = field();
        let (mut heavy, heavy_id) = field();
        for (w, id, n) in [(&mut light, light_id, 0), (&mut heavy, heavy_id, 10)] {
            w.give_item(id, "wooden_plank", 1);
            for _ in 0..n {
                w.give_item(id, "wooden_plank", 1);
            }
            w.player_attack(id);
        }
        let cost = |w: &World, id: u32| 1.0 - me(w, id).state.endurance;
        assert!((cost(&heavy, heavy_id) / cost(&light, light_id) - t.heavy_action_cost).abs() < 0.01);
    }

    /// Containers: searching takes time (longer sneaking) and makes a little noise, fills them once,
    /// and they stay open until you walk away. Items move in and out of them, room permitting.
    #[test]
    fn searching_and_using_containers() {
        let mut w = World::new(content(3));
        let id = w.add_player(w.player_spawn);
        assert!(w.containers().len() > w.map.buildings.len(), "{} containers in {} houses", w.containers().len(), w.map.buildings.len());
        assert!(w.opened_containers().next().is_none(), "nothing opened yet");
        let started = w.tick;
        let c = crate::save::tests::open_nearest_container(&mut w, id);
        let site = w.container(c).unwrap().clone();
        let def = w.container_def(&site).clone();
        let took = (w.tick - started) as f32 * TICK_DT;
        assert!((took - def.search_secs).abs() < 0.11, "searching the {} took {took} s, not {}", def.id, def.search_secs);
        let contents = w.container_contents(c).unwrap().to_vec();
        assert!(last_note(&w, id).starts_with("searched the"), "{}", last_note(&w, id));
        // Put something in and take something out.
        w.give_item(id, "towel", 1);
        w.player_move_item(id, Place::Carried, 0, Place::Container(c), 0).unwrap();
        settle(&mut w, id);
        let towel = w.item_index("towel").unwrap();
        assert!(w.container_contents(c).unwrap().iter().any(|s| s.item == towel), "{}", last_note(&w, id));
        assert!(me(&w, id).inventory.carried.is_empty());
        let k = w.container_contents(c).unwrap().iter().position(|s| s.item == towel).unwrap() as u32;
        w.player_move_item(id, Place::Container(c), k, Place::Carried, 0).unwrap();
        settle(&mut w, id);
        assert_eq!(w.container_contents(c).unwrap(), contents.as_slice());
        // Too big for it: a container has only so much room.
        let room = def.room_l;
        let big = w.config.items.iter().position(|d| d.volume_l > room).map(|i| w.config.items[i].id.clone());
        if let Some(big) = big {
            w.hold_item(id, Slot::Hand, &big);
            w.player_move_item(id, Place::Slot(Slot::Hand), 0, Place::Container(c), 0).unwrap();
            settle(&mut w, id);
            assert!(last_note(&w, id).contains("no room"), "{}", last_note(&w, id));
        }
        // Reopening is quick; walking away closes it, and then nothing moves in or out.
        let pos = me(&w, id).state.pos;
        w.players[0].state.pos = w.player_spawn + Vec2::new(40.0, 40.0);
        w.step();
        assert_eq!(me(&w, id).open, None);
        assert!(w.player_move_item(id, Place::Container(c), 0, Place::Carried, 0).is_err());
        w.players[0].state.pos = pos;
        let t0 = w.tick;
        w.player_search(id, Some(c)).unwrap();
        settle(&mut w, id);
        assert!((w.tick - t0) as f32 * TICK_DT <= w.config.carry.reopen_secs + 0.11);
        assert_eq!(me(&w, id).open, Some(c));
        // Sneaking makes the first search of another container slower.
        let other = w.containers().iter().find(|s| s.id != c && w.container_contents(s.id).is_none()).unwrap().id;
        w.players[0].state.stance = Stance::Sneak;
        w.players[0].input.sneak = true;
        let pi = 0;
        let site = w.container(other).unwrap().clone();
        let at = site.centre() + Vec2::new(0.0, site.size.y as f32 * 0.5 + 0.35);
        w.players[pi].state.pos = at;
        if w.player_search(id, Some(other)).is_ok() {
            let secs = w.players[pi].jobs[0].left;
            assert!((secs - w.container_def(&site).search_secs * w.config.carry.sneak_search_mult).abs() < 1e-4);
        }
    }

    /// Containers fill from the seed and the container alone: the same whatever order they're
    /// opened in, and not at all until opened.
    #[test]
    fn containers_fill_the_same_in_any_order() {
        let open_all = |order: &mut dyn Iterator<Item = u32>| {
            let mut w = World::new(content(9));
            let id = w.add_player(w.player_spawn);
            let pi = 0;
            for c in order {
                let site = w.container(c).unwrap().clone();
                // Teleport into its room, beside it, and search.
                let spots = (-1..=site.size.y).flat_map(|dy| (-1..=site.size.x).map(move |dx| site.min + glam::IVec2::new(dx, dy)));
                let Some(t) = spots.filter(|&t| w.map.in_bounds(t) && !w.map.tile_blocked(t) && w.map.tile(t).room == site.room).min_by_key(|t| (t.x, t.y)) else { continue };
                w.players[pi].state.pos = t.as_vec2() + Vec2::splat(0.5);
                if w.player_search(id, Some(c)).is_ok() {
                    settle(&mut w, id);
                }
            }
            w.opened_containers().map(|(c, s)| (c, s.to_vec())).collect::<Vec<_>>()
        };
        let w = World::new(content(9));
        let ids: Vec<u32> = w.containers().iter().filter(|s| s.building as usize == w.spawn_building).map(|s| s.id).collect();
        assert!(ids.len() >= 4, "{ids:?}");
        let a = open_all(&mut ids.clone().into_iter());
        let b = open_all(&mut ids.clone().into_iter().rev());
        assert_eq!(a, b);
        assert!(a.len() >= 3 && a.iter().any(|(_, s)| !s.is_empty()), "{a:?}");
    }

    /// Slots: a weapon from your pockets goes into your hand and swings; two-handed takes both
    /// hands; clothing layers; eating uses one up.
    #[test]
    fn equipping_wearing_and_eating() {
        let (mut w, id) = field();
        w.give_item(id, "kitchen_knife", 1);
        w.player_equip(id, 0).unwrap();
        settle(&mut w, id);
        assert_eq!(w.wielded(me(&w, id).weapon).id, "kitchen_knife");
        // It swings like one, and hits.
        let z = w.add_zombie(Vec2::new(61.0, 60.5), std::f32::consts::PI, 1.0);
        w.zombies.wander_timer[z] = 1e9;
        let hp = w.zombies.hp[z];
        w.player_attack(id);
        let mut swung = None;
        for _ in 0..20 {
            w.step();
            if let Some(a) = w.action_events.iter().find(|a| a.player == id) {
                swung = Some((a.weapon, a.hits));
            }
        }
        assert_eq!(swung, Some((w.weapon_index("kitchen_knife").unwrap(), 1)));
        assert!(w.zombies.index_of(1).is_none_or(|i| w.zombies.hp[i] < hp));
        // A plastic bag in the other hand, then a two-handed bat: the bag goes into the pockets.
        w.give_item(id, "plastic_bag", 1);
        let k = me(&w, id).inventory.carried.iter().position(|s| w.item_def(s.item).id == "plastic_bag").unwrap();
        w.player_equip(id, k as u32).unwrap();
        settle(&mut w, id);
        assert!(me(&w, id).inventory.off_hand.is_some());
        assert_eq!(w.carry(me(&w, id)).room_l, 11.0);
        w.give_item(id, "baseball_bat", 1);
        let k = me(&w, id).inventory.carried.iter().position(|s| w.item_def(s.item).id == "baseball_bat").unwrap();
        w.player_equip(id, k as u32).unwrap();
        settle(&mut w, id);
        let inv = &me(&w, id).inventory;
        assert_eq!(w.wielded(me(&w, id).weapon).id, "baseball_bat", "{}", last_note(&w, id));
        assert!(inv.off_hand.is_none());
        let names: Vec<&str> = inv.carried.iter().map(|s| w.item_def(s.item).id.as_str()).collect();
        assert!(names.contains(&"kitchen_knife") && names.contains(&"plastic_bag"), "{names:?}");
        // Clothing: a jumper over a T-shirt is fine; a hoodie replaces the jumper (same layer).
        for item in ["t_shirt", "jumper", "hoodie"] {
            w.give_item(id, item, 1);
            let k = me(&w, id).inventory.carried.iter().position(|s| w.item_def(s.item).id == item).unwrap();
            w.player_wear(id, k as u32).unwrap();
            settle(&mut w, id);
        }
        let worn: Vec<&str> = me(&w, id).inventory.worn.iter().map(|s| w.item_def(s.item).id.as_str()).collect();
        assert_eq!(worn, ["t_shirt", "hoodie"]);
        assert!(w.player_wear(id, 0).is_err(), "can't wear a knife");
        // Eating: one tin of three.
        w.give_item(id, "apple", 3);
        let k = me(&w, id).inventory.carried.iter().position(|s| w.item_def(s.item).id == "apple").unwrap() as u32;
        w.player_use(id, Place::Carried, k).unwrap();
        let mut ate = false;
        for _ in 0..80 {
            w.step();
            ate |= w.used_items.contains(&(id, w.item_index("apple").unwrap()));
        }
        assert!(ate && last_note(&w, id).starts_with("ate the apple: "), "{}", last_note(&w, id));
        assert_eq!(me(&w, id).inventory.carried.iter().find(|s| w.item_def(s.item).id == "apple").unwrap().count, 2);
        assert!(w.player_use(id, Place::Carried, 0).is_err(), "can't eat a knife");
    }

    /// A swing calls off what you were doing; dying drops everything where you fell.
    #[test]
    fn fighting_cancels_and_dying_drops_everything() {
        let (mut w, id) = field();
        w.give_item(id, "tinned_beans", 4);
        w.hold_item(id, Slot::Hand, "crowbar");
        w.hold_item(id, Slot::Back, "hiking_pack");
        w.player_drop(id, Place::Carried, 0, 0).unwrap();
        w.player_attack(id);
        assert!(me(&w, id).jobs.is_empty());
        let n = me(&w, id).inventory.stacks().count();
        let pi = 0;
        w.players[pi].hp = 0.0;
        w.kill_player(pi);
        let p = me(&w, id);
        assert!(p.inventory.is_empty() && p.weapon.is_none());
        assert_eq!(w.items().len(), n);
        assert!(w.items().iter().all(|it| it.pos.distance(p.state.pos) < 0.5));
    }

    /// Drain the notes; panic on any refusal (`what` says what was going on).
    fn no_refusals(w: &mut World, what: &str) -> Vec<String> {
        let notes = std::mem::take(&mut w.item_notes);
        let bad: Vec<&ItemNote> = notes.iter().filter(|n| !n.ok).collect();
        assert!(bad.is_empty(), "{what}: refused {bad:?}");
        notes.into_iter().map(|n| n.text).collect()
    }

    /// Step until player `id` has nothing queued, checking every note on the way.
    fn settle_clean(w: &mut World, id: u32, what: &str) -> Vec<String> {
        let mut said = Vec::new();
        for _ in 0..400 {
            said.extend(no_refusals(w, what));
            if w.player(id).unwrap().jobs.is_empty() {
                return said;
            }
            w.step();
        }
        panic!("{what}: jobs never finished");
    }

    /// The playtest of #48: standing at the kitchen counter, the counter, the fridge and the oven
    /// beside it can all be searched, emptied and filled again (with repeated clicks and a little
    /// shuffling about) without a single refusal.
    #[test]
    fn a_kitchen_from_the_counter() {
        let mut w = World::new(content(3));
        let id = w.add_player(w.player_spawn);
        assert!(w.hold_item(id, Slot::Back, "hiking_pack"));
        let kind = |w: &World, s: &ContainerSite| w.container_def(s).id.clone();
        let mut kitchens = 0;
        let rooms: Vec<u16> = w.containers().iter().filter(|s| kind(&w, s) == "kitchen_cupboard").map(|s| s.room).collect();
        for room in rooms {
            let here: Vec<ContainerSite> = w.containers().iter().filter(|s| s.room == room).cloned().collect();
            let find = |k: &str| here.iter().find(|s| kind(&w, s) == k).cloned();
            let (Some(counter), Some(fridge), Some(oven)) = (find("kitchen_cupboard"), find("fridge"), find("oven")) else { continue };
            kitchens += 1;
            // Somewhere a body fits at the counter (within 0.6 m of it), as near as it gets to all three.
            let (lo, hi) = (counter.min - IVec2::splat(2), counter.min + counter.size + IVec2::splat(2));
            let mut best: Option<(f32, Vec2)> = None;
            for y in (lo.y * 10)..(hi.y * 10) {
                for x in (lo.x * 10)..(hi.x * 10) {
                    let p = Vec2::new(x as f32, y as f32) * 0.1 + Vec2::splat(0.05);
                    if w.map.room_at(p) != room || w.map.resolve_circle(p, crate::players::PLAYER_RADIUS).distance(p) > 1e-3 || counter.distance(p) > 0.6 {
                        continue;
                    }
                    let worst = [&counter, &fridge, &oven].iter().map(|s| s.distance(p)).fold(0.0, f32::max);
                    if best.is_none_or(|(d, _)| worst < d) {
                        best = Some((worst, p));
                    }
                }
            }
            let (worst, at) = best.expect("room to stand at the counter");
            assert!(worst <= CONTAINER_REACH - 0.05, "room {room}: the furthest of counter, fridge and oven is {worst} m from the best spot at the counter");
            let pi = w.players.iter().position(|p| p.id == id).unwrap();
            w.players[pi].state.pos = at;
            w.players[pi].jobs.clear();
            // Facing the counter, E opens the counter.
            let lo = counter.min.as_vec2();
            let to = at.clamp(lo, lo + counter.size.as_vec2()) - at;
            w.players[pi].state.facing = to.y.atan2(to.x);
            assert_eq!(w.container_in_reach(id), Some(counter.id));
            for site in [&counter, &fridge, &oven] {
                let what = format!("room {room}, the {}", kind(&w, site));
                w.player_search(id, if site.id == counter.id { None } else { Some(site.id) }).unwrap();
                // A second press while it's under way changes nothing.
                w.player_search(id, Some(site.id)).unwrap();
                let said = settle_clean(&mut w, id, &what);
                assert!(said.iter().any(|t| t.starts_with("searched") || t.starts_with("opened")), "{what}: {said:?}");
                assert_eq!(me(&w, id).open, Some(site.id));
                // Shuffle half a step aside: still open.
                w.players[pi].state.pos = w.map.resolve_circle(at + Vec2::new(0.25, 0.25), crate::players::PLAYER_RADIUS);
                w.step();
                assert_eq!(me(&w, id).open, Some(site.id), "{what}: closed after a small step");
                // Take everything, each row clicked twice.
                let n = w.container_contents(site.id).unwrap().len();
                for _ in 0..n {
                    w.player_move_item(id, Place::Container(site.id), 0, Place::Carried, 0).unwrap();
                    w.player_move_item(id, Place::Container(site.id), 0, Place::Carried, 0).unwrap();
                    settle_clean(&mut w, id, &what);
                }
                assert!(w.container_contents(site.id).unwrap().is_empty(), "{what}: left {:?}", w.container_contents(site.id));
                w.players[pi].state.pos = at;
                // And put one thing back.
                if !me(&w, id).inventory.carried.is_empty() {
                    w.player_move_item(id, Place::Carried, 0, Place::Container(site.id), 1).unwrap();
                    settle_clean(&mut w, id, &what);
                    assert!(!w.container_contents(site.id).unwrap().is_empty(), "{what}");
                }
                // Empty pockets for the next one (room is another test's business).
                w.players[pi].inventory.carried.clear();
                w.refresh_carry(pi);
            }
        }
        assert!(kitchens >= 2, "both houses have kitchens: {kitchens}");
    }

    /// Reach is to the nearest edge of the furniture (1.6 m), and an open container stays open
    /// until 2.2 m or another room.
    #[test]
    fn container_reach_edges() {
        let (mut w, id) = field();
        let def = w.config.containers.lookup("fridge", "kitchen").unwrap();
        // A 2 x 1 fridge east of the player, its west edge at x = 62.
        w.sites.push(ContainerSite { id: 7, def, building: 0, room: 0, min: IVec2::new(62, 60), size: IVec2::new(2, 1) });
        let at = |w: &mut World, x: f32| {
            w.players[0].state.pos = Vec2::new(x, 60.5);
            w.players[0].jobs.clear();
        };
        at(&mut w, 62.0 - CONTAINER_REACH - 0.05);
        assert!(w.player_search(id, Some(7)).is_err());
        assert!(last_note(&w, id).contains("out of reach"), "{}", last_note(&w, id));
        assert_eq!(w.container_in_reach(id), None);
        at(&mut w, 62.0 - CONTAINER_REACH + 0.05);
        assert_eq!(w.container_in_reach(id), Some(7));
        w.player_search(id, None).unwrap();
        settle(&mut w, id);
        assert_eq!(me(&w, id).open, Some(7));
        // Stepping back a little keeps it open; usable too, and it can be searched again from there.
        at(&mut w, 62.0 - CONTAINER_KEEP_OPEN + 0.05);
        w.step();
        assert_eq!(me(&w, id).open, Some(7));
        w.item_notes.clear();
        w.player_search(id, Some(7)).unwrap();
        settle_clean(&mut w, id, "reopening from 2.1 m");
        w.give_item(id, "apple", 1);
        w.player_move_item(id, Place::Carried, 0, Place::Container(7), 0).unwrap();
        settle_clean(&mut w, id, "putting an apple in from 2.1 m");
        // Further: it closes.
        at(&mut w, 62.0 - CONTAINER_KEEP_OPEN - 0.05);
        w.step();
        assert_eq!(me(&w, id).open, None);
        // Another room never reaches, however near.
        at(&mut w, 61.5);
        w.sites[0].room = 3;
        assert_eq!(w.container_in_reach(id), None);
    }

    /// E prefers the container the player faces, nearest first; with none ahead, the nearest.
    #[test]
    fn e_prefers_the_container_you_face() {
        let (mut w, id) = field();
        let def = w.config.containers.lookup("counter", "kitchen").unwrap();
        // Player at (60.5, 60.5): one cupboard south-west (0.71 m to its corner), one east
        // (0.5 m), one further east behind it (1.5 m, in reach).
        w.sites.push(ContainerSite { id: 1, def, building: 0, room: 0, min: IVec2::new(59, 61), size: IVec2::ONE });
        w.sites.push(ContainerSite { id: 2, def, building: 0, room: 0, min: IVec2::new(61, 60), size: IVec2::ONE });
        w.sites.push(ContainerSite { id: 3, def, building: 0, room: 0, min: IVec2::new(62, 59), size: IVec2::new(1, 3) });
        w.players[0].state.facing = 0.0; // east
        assert_eq!(w.container_in_reach(id), Some(2), "the nearest of those ahead");
        w.players[0].state.facing = std::f32::consts::PI; // west: nothing ahead within 60 degrees but #1 (south-west, 45)
        assert_eq!(w.container_in_reach(id), Some(1));
        w.players[0].state.facing = -std::f32::consts::FRAC_PI_2; // north: none ahead, the nearest
        assert_eq!(w.container_in_reach(id), Some(2));
    }

    /// E with nothing about says so once per press, in plain words; repeated or crossing requests
    /// for one move queue it once and refuse nothing.
    #[test]
    fn e_with_nothing_here_and_repeated_requests() {
        let (mut w, id) = field();
        assert!(w.player_search(id, None).is_err());
        assert_eq!(w.item_notes.len(), 1);
        assert_eq!(last_note(&w, id), NOTHING_HERE);
        // With something on the floor, a search falls back to picking it up.
        w.item_notes.clear();
        w.spawn_item("apple", 1, Vec2::new(61.0, 60.5));
        w.player_search(id, None).unwrap();
        assert!(last_note(&w, id).contains("apple") && w.items().is_empty(), "{}", last_note(&w, id));
        // Three clicks on one stack on the floor: one job, the stack taken, no refusals.
        let g = w.spawn_item("tinned_beans", 3, Vec2::new(61.0, 60.5)).unwrap();
        for _ in 0..3 {
            w.player_move_item(id, Place::Ground, g, Place::Carried, 0).unwrap();
        }
        assert_eq!(me(&w, id).jobs.len(), 1);
        settle_clean(&mut w, id, "three clicks");
        // Shift-clicks one at a time: as many as there are, then nothing more.
        w.player_drop(id, Place::Carried, 1, 0).unwrap();
        settle_clean(&mut w, id, "dropping the beans");
        let g = w.floor_of(id)[0].id;
        for _ in 0..5 {
            w.player_move_item(id, Place::Ground, g, Place::Carried, 1).unwrap();
        }
        assert_eq!(me(&w, id).jobs.len(), 3);
        settle_clean(&mut w, id, "five shift-clicks on three");
        assert!(w.items().is_empty());
        // A request for something long gone says what's wrong in words.
        assert!(w.player_move_item(id, Place::Ground, 999, Place::Carried, 0).is_err());
        assert_eq!(last_note(&w, id), "that's no longer on the floor");
    }

    /// The floor is what a hand reaches: furniture in between doesn't stop it, a wall does.
    #[test]
    fn the_floor_reaches_over_furniture_not_through_walls() {
        let (mut w, id) = field();
        w.map.tile_mut(IVec2::new(61, 60)).unwrap().feature = crate::map::Feature::Furniture;
        let over = w.spawn_item("towel", 1, Vec2::new(61.5, 60.5)).unwrap();
        let beyond = w.spawn_item("towel", 1, Vec2::new(60.5, 61.6)).unwrap();
        w.map.set_edge(IVec2::new(60, 60), crate::map::Side::S, crate::map::Edge::Wall);
        w.map_changed();
        assert_eq!(w.floor_ids(id), vec![over]);
        w.player_move_item(id, Place::Ground, over, Place::Carried, 0).unwrap();
        assert!(w.player_move_item(id, Place::Ground, beyond, Place::Carried, 0).is_err());
        assert_eq!(last_note(&w, id), "the towel is out of reach");
        settle(&mut w, id);
        assert_eq!(me(&w, id).inventory.carried.len(), 1);
        // Something dropped lands at your feet and is on your floor at once.
        w.player_drop(id, Place::Carried, 0, 0).unwrap();
        settle(&mut w, id);
        assert_eq!(w.floor_ids(id).len(), 1);
    }
}
