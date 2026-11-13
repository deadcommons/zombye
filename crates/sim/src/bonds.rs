//! Hold-ups: tying another survivor's hands, freeing them, and searching one who is tied (#73,
//! docs/PLAN.md §4.8: "restraints for hold-ups").
//!
//! **Tying** ([`World::player_restrain`]): with a restraint in your pockets or hands (an item
//! tagged `restrains`: cable ties, rope) and another survivor within reach who has their **hands
//! up**, is **knocked down**, or (with player against player on) lies **asleep**, a few seconds
//! of work ties their hands ([`Player::restraint`]). It is called off if they lower their hands,
//! get up or step away, or if you swing, are knocked about or fall asleep. With PvP off only
//! hands held up will do: nobody is tied against their will in co-op.
//!
//! **Tied**, a survivor walks slowly and can't run, climb, fight, gesture, smash a window or do
//! anything with their things (it can still talk, and push a door open or shut). It gets out by
//! **struggling** ([`World::player_struggle`]): standing still and working at it for
//! `escape_secs` (rope) or `escape_tie_secs` (a cable tie), which makes a little noise; or by
//! someone **freeing** it ([`World::player_free`]): rope is untied and falls at its feet, a
//! cable tie (`ties_once`) has to be cut, with something tagged `cuts`.
//!
//! **Searching** ([`World::player_frisk`]): a tied survivor's things can be gone through like a
//! container's: after `frisk_secs` the searcher's client is shown everything it has
//! ([`Player::frisking`]), for as long as it stays tied and within `keep_m`, and
//! [`World::player_take_from`] takes a stack at a time, into the searcher's pockets and bags or,
//! with no room, to the floor at its feet.
//!
//! Everything a player does here is one [`HandsJob`] at a time, checked when asked and again when
//! due, like the item jobs. Restraints are saved (the `BOND` section) and in the state hash while
//! there are any; a job under way and an open search are not (a load calls them off, as it
//! lowers raised hands). Every number is in [`BondTuning`]. Ours.

use glam::Vec2;
use serde::{Deserialize, Serialize};

use crate::TICK_DT;
use crate::inventory::{Place, merge_into, stack_weight};
use crate::items::ItemStack;
use crate::players::Player;
use crate::senses::SoundSource;
use crate::speech::Gesture;
use crate::vision::clear_reach;
use crate::world::World;

/// An item with this tag ties hands.
pub const TAG_RESTRAINS: &str = "restrains";
/// A restraint with this tag is used up: it can't be untied, only cut, and is gone after.
pub const TAG_ONCE: &str = "ties_once";
/// An item with this tag cuts a `ties_once` restraint off.
pub const TAG_CUTS: &str = "cuts";

/// Tying, freeing and searching: reach and times. Ours.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BondTuning {
    /// How near the other survivor has to be to start (metres between the two).
    pub reach_m: f32,
    /// How far they may get before a job under way or an open search is called off.
    pub keep_m: f32,
    pub tie_secs: f32,
    pub free_secs: f32,
    pub frisk_secs: f32,
    /// Taking one stack from a searched survivor; something worn or held takes `take_worn_secs`.
    pub take_secs: f32,
    pub take_worn_secs: f32,
    /// Seconds of struggling to get out of rope, and out of a cable tie.
    pub escape_secs: f32,
    pub escape_tie_secs: f32,
    /// Walking speed with tied hands, as a share of normal (no running, no climbing).
    pub speed: f32,
    /// The noise struggling makes (metres), once every `struggle_noise_secs`.
    pub struggle_noise: f32,
    pub struggle_noise_secs: f32,
}

impl Default for BondTuning {
    fn default() -> Self {
        Self {
            reach_m: 1.6,
            keep_m: 2.2,
            tie_secs: 3.0,
            free_secs: 2.5,
            frisk_secs: 2.0,
            take_secs: 0.6,
            take_worn_secs: 1.5,
            escape_secs: 60.0,
            escape_tie_secs: 90.0,
            speed: 0.6,
            struggle_noise: 4.0,
            struggle_noise_secs: 2.0,
        }
    }
}

/// Tied hands.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Restraint {
    /// What they are tied with (an index into the items).
    pub item: u16,
    /// Who tied them (a player id).
    pub by: u32,
    /// Seconds struggled so far.
    pub struggled: f32,
    /// Working at it now (only counts while standing still).
    pub struggling: bool,
}

/// What a [`HandsJob`] does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum HandsKind {
    /// Tie the target's hands with one of `item`.
    Tie { item: u16 },
    Free,
    /// Start searching the target.
    Frisk,
    /// Take `count` (0: all) of the searched target's stack at `from`/`index`, which is of `item`.
    Take { from: Place, index: u32, item: u16, count: u16 },
}

/// The kinds, for clients (what the bar says, the pose).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HandsAct {
    Tie,
    Free,
    Frisk,
    Take,
}

impl HandsKind {
    pub fn act(self) -> HandsAct {
        match self {
            HandsKind::Tie { .. } => HandsAct::Tie,
            HandsKind::Free => HandsAct::Free,
            HandsKind::Frisk => HandsAct::Frisk,
            HandsKind::Take { .. } => HandsAct::Take,
        }
    }
}

impl HandsAct {
    /// "Tying their hands", ...
    pub fn doing(self) -> &'static str {
        match self {
            HandsAct::Tie => "Tying their hands",
            HandsAct::Free => "Freeing their hands",
            HandsAct::Frisk => "Searching them",
            HandsAct::Take => "Taking it",
        }
    }
}

/// Something a player is doing to another with its hands, until it is done.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct HandsJob {
    pub kind: HandsKind,
    /// The other player's id.
    pub target: u32,
    pub left: f32,
    pub total: f32,
}

/// Take one of `item` out of what `p` carries or holds. Returns whether there was one.
fn take_one(p: &mut Player, item: u16) -> bool {
    let inv = &mut p.inventory;
    if let Some(k) = inv.carried.iter().position(|s| s.item == item) {
        inv.carried[k].count -= 1;
        inv.carried[k].used = 0;
        if inv.carried[k].count == 0 {
            inv.carried.remove(k);
        }
        return true;
    }
    for slot in [&mut inv.hand, &mut inv.off_hand] {
        if let Some(s) = slot.as_mut().filter(|s| s.item == item) {
            s.count -= 1;
            if s.count == 0 {
                *slot = None;
            }
            return true;
        }
    }
    false
}

impl World {
    fn has_tagged(&self, p: &Player, tag: &str) -> Option<u16> {
        crate::firstaid::usable(&p.inventory).map(|s| s.item).find(|&i| self.config.items.get(i as usize).is_some_and(|d| d.has_tag(tag)))
    }

    /// Is the restraint item `item` one that is used up (cut off, not untied)?
    pub fn restraint_is_cut(&self, item: u16) -> bool {
        self.config.items.get(item as usize).is_some_and(|d| d.has_tag(TAG_ONCE))
    }

    /// How long player `p` has to struggle to get out of what ties it (seconds), if it is tied.
    pub fn escape_secs(&self, p: &Player) -> Option<f32> {
        let r = p.restraint?;
        let t = &self.config.bonds;
        Some(if self.restraint_is_cut(r.item) { t.escape_tie_secs } else { t.escape_secs })
    }

    /// The other survivor player `id` would tie, free or search: the nearest one within reach it
    /// faces (within 80 degrees; one right at its feet whichever way), with nothing that stops a
    /// hand between them.
    pub fn faced_player(&self, id: u32) -> Option<u32> {
        let p = self.player(id)?;
        let reach = self.config.bonds.reach_m;
        let aim = Vec2::from_angle(p.state.facing);
        self.players
            .iter()
            .filter(|o| o.id != id && !o.dead && o.state.level == p.state.level)
            .filter_map(|o| {
                let d = o.state.pos - p.state.pos;
                let len = d.length();
                (len <= reach && (len < 0.5 || d.dot(aim) >= len * 0.17) && if p.state.level > 0 { self.map.upper_clear(p.state.pos, o.state.pos) } else { clear_reach(&self.map, p.state.pos, o.state.pos) }).then_some((len, o.id))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)))
            .map(|(_, id)| id)
    }

    /// Can player `pi` use its hands on someone: why not, if not.
    fn hands_free(&self, pi: usize) -> Result<(), String> {
        let p = &self.players[pi];
        Err(if p.dead {
            "dead"
        } else if p.needs.asleep() || p.away.is_some() {
            "asleep"
        } else if p.restraint.is_some() {
            "your hands are tied"
        } else if p.knock.is_some() {
            "knocked about"
        } else if p.action.is_some() {
            "in the middle of a swing"
        } else {
            return Ok(());
        }
        .to_string())
    }

    fn in_hands_reach(&self, pi: usize, ti: usize, reach: f32) -> bool {
        let (a, b) = (self.players[pi].state.pos, self.players[ti].state.pos);
        a.distance(b) <= reach && clear_reach(&self.map, a, b)
    }

    /// Can `t` be tied now: hands up, knocked down, or (PvP) asleep.
    fn tieable(&self, t: &Player) -> Result<(), String> {
        let pvp = self.config.combat.pvp;
        Err(if t.dead {
            "they are dead"
        } else if t.restraint.is_some() {
            "their hands are tied already"
        } else if t.protected > 0.0 {
            "they have only just arrived"
        } else if t.gesture.is_some_and(|g| g.kind == Gesture::HandsUp) || (pvp && (t.knock.is_some_and(|k| k.down) || t.needs.asleep() || t.away.is_some())) {
            return Ok(());
        } else if pvp {
            "they have to have their hands up, or be on the ground"
        } else {
            "they have to put their hands up"
        }
        .to_string())
    }

    /// Whether the job `kind` of player `pi` on player `ti` can go on (or start, within `reach`).
    fn hands_check(&self, pi: usize, ti: usize, kind: HandsKind, reach: f32) -> Result<(), String> {
        self.hands_free(pi)?;
        let (p, t) = (&self.players[pi], &self.players[ti]);
        if t.dead {
            return Err("they are dead".into());
        }
        if !self.in_hands_reach(pi, ti, reach) {
            return Err("too far away".into());
        }
        match kind {
            HandsKind::Tie { item } => {
                self.tieable(t)?;
                if !crate::firstaid::carries(&p.inventory, item) {
                    return Err("nothing to tie them with".into());
                }
            }
            HandsKind::Free => {
                let Some(r) = t.restraint else { return Err("their hands aren't tied".into()) };
                if self.restraint_is_cut(r.item) && self.has_tagged(p, TAG_CUTS).is_none() {
                    return Err(format!("you need something that cuts to get a {} off", self.item_def(r.item).name.to_lowercase()));
                }
            }
            HandsKind::Frisk | HandsKind::Take { .. } => {
                if t.restraint.is_none() {
                    return Err("only someone whose hands are tied can be searched".into());
                }
            }
        }
        Ok(())
    }

    fn start_hands(&mut self, id: u32, target: Option<u32>, kind: impl Fn(&World, usize) -> Result<HandsKind, String>, secs: f32) -> Result<(), String> {
        let Some(pi) = self.players.iter().position(|p| p.id == id) else { return Err(format!("no player {id}")) };
        if let Err(why) = self.hands_free(pi) {
            return self.refuse(id, why);
        }
        let Some(target) = target.or_else(|| self.faced_player(id)) else { return self.refuse(id, "nobody within reach".into()) };
        let Some(ti) = self.players.iter().position(|p| p.id == target && p.id != id) else { return self.refuse(id, "nobody there".into()) };
        let kind = match kind(self, pi) {
            Ok(k) => k,
            Err(why) => return self.refuse(id, why),
        };
        // Asked again for what is under way (a repeated key press): nothing to do or complain of.
        if self.players[pi].hands.is_some_and(|h| h.target == target && h.kind.act() == kind.act()) {
            return Ok(());
        }
        if self.players[pi].hands.is_some() {
            return self.refuse(id, "busy".into());
        }
        if let Err(why) = self.hands_check(pi, ti, kind, self.config.bonds.reach_m) {
            return self.refuse(id, why);
        }
        // Its own things can wait.
        self.cancel_jobs(pi);
        let p = &mut self.players[pi];
        p.gesture = None;
        p.hands = Some(HandsJob { kind, target, left: secs.max(0.0), total: secs.max(0.0) });
        Ok(())
    }

    /// Player `id` starts tying the hands of `target` (`None`: the survivor it faces within
    /// reach) with a restraint it carries. See the [module docs](self) for who can be tied.
    pub fn player_restrain(&mut self, id: u32, target: Option<u32>) -> Result<(), String> {
        let secs = self.config.bonds.tie_secs;
        self.start_hands(id, target, |w, pi| w.has_tagged(&w.players[pi], TAG_RESTRAINS).map(|item| HandsKind::Tie { item }).ok_or_else(|| "you have nothing to tie hands with (cable ties, rope)".to_string()), secs)
    }

    /// Player `id` starts freeing `target`'s (`None`: the faced survivor's) tied hands.
    pub fn player_free(&mut self, id: u32, target: Option<u32>) -> Result<(), String> {
        let secs = self.config.bonds.free_secs;
        self.start_hands(id, target, |_, _| Ok(HandsKind::Free), secs)
    }

    /// Player `id` starts searching `target` (`None`: the faced survivor), whose hands are tied.
    pub fn player_frisk(&mut self, id: u32, target: Option<u32>) -> Result<(), String> {
        let secs = self.config.bonds.frisk_secs;
        if let (Some(p), Some(t)) = (self.player(id), target.or_else(|| self.faced_player(id)))
            && p.frisking == Some(t)
        {
            return Ok(());
        }
        self.start_hands(id, target, |_, _| Ok(HandsKind::Frisk), secs)
    }

    /// Player `id` takes `count` (0: all) of the stack at `from`/`index` (carried, worn or a slot)
    /// from the survivor it is searching; `item` is what the client saw there.
    pub fn player_take_from(&mut self, id: u32, from: Place, index: u32, item: u16, count: u16) -> Result<(), String> {
        let Some(target) = self.player(id).and_then(|p| p.frisking) else { return self.refuse(id, "you aren't searching anyone".into()) };
        if !matches!(from, Place::Carried | Place::Worn | Place::Slot(_)) {
            return self.refuse(id, "that isn't theirs".into());
        }
        let t = &self.config.bonds;
        let secs = if from == Place::Carried { t.take_secs } else { t.take_worn_secs };
        self.start_hands(id, Some(target), |_, _| Ok(HandsKind::Take { from, index, item, count }), secs)
    }

    /// Player `id` stops what its hands are doing to another survivor and shuts its search.
    pub fn player_stop_hands(&mut self, id: u32) -> bool {
        let Some(p) = self.players.iter_mut().find(|p| p.id == id) else { return false };
        let had = p.hands.take().is_some() | p.frisking.take().is_some();
        if had {
            self.private_dirty.insert(id);
        }
        had
    }

    /// Player `id`, tied, starts (`on`) or stops working at getting out. Returns whether that
    /// changed anything.
    pub fn player_struggle(&mut self, id: u32, on: bool) -> bool {
        let Some(p) = self.players.iter_mut().find(|p| p.id == id && !p.dead) else { return false };
        let Some(r) = &mut p.restraint else { return false };
        let changed = r.struggling != on;
        r.struggling = on;
        changed
    }

    /// Debug and tests: tie player `id`'s hands with `item` at once, as if `by` had.
    pub fn restrain_now(&mut self, id: u32, item: &str, by: u32) -> bool {
        let Some(item) = self.item_index(item) else { return false };
        let Some(ti) = self.players.iter().position(|p| p.id == id && !p.dead) else { return false };
        self.tie(ti, item, by);
        true
    }

    fn tie(&mut self, ti: usize, item: u16, by: u32) {
        self.cancel_jobs(ti);
        let t = &mut self.players[ti];
        (t.gesture, t.action, t.queued, t.hands, t.frisking, t.open) = (None, None, None, None, None, None);
        t.restraint = Some(Restraint { item, by, struggled: 0.0, struggling: false });
        let id = t.id;
        // A sleeper wakes with its hands tied.
        self.wake_player(id);
        self.refresh_carry(ti);
    }

    /// Player `ti`'s hands come free; rope falls at its feet, a cut or snapped tie is gone.
    fn untie(&mut self, ti: usize) {
        let Some(r) = self.players[ti].restraint.take() else { return };
        if !self.restraint_is_cut(r.item) && (r.item as usize) < self.config.items.len() {
            let (pos, facing) = (self.players[ti].state.pos, self.players[ti].state.facing);
            let level = self.players[ti].state.level;
            self.put_on_floor(ItemStack::one(r.item), pos, facing, level);
        }
        self.refresh_carry(ti);
    }

    /// Tied hands worked loose, jobs on other survivors counted down and done, searches shut
    /// when the two part. After movement and the tick's blows, so the movement modifiers a
    /// snapshot carries are the ones the next tick moves with.
    pub(crate) fn step_bonds(&mut self) {
        if !self.players.iter().any(|p| p.restraint.is_some() || p.hands.is_some() || p.frisking.is_some()) {
            return;
        }
        for pi in 0..self.players.len() {
            let p = &mut self.players[pi];
            let id = p.id;
            if p.dead {
                if p.restraint.take().is_some() | p.hands.take().is_some() | p.frisking.take().is_some() {
                    self.private_dirty.insert(id);
                }
                continue;
            }
            // Struggling: only standing still, awake and on its feet.
            if let Some(r) = p.restraint
                && r.struggling
                && p.input.move_dir == Vec2::ZERO
                && !p.needs.asleep()
                && p.away.is_none()
                && p.knock.is_none()
            {
                let every = self.config.bonds.struggle_noise_secs.max(TICK_DT);
                let before = (r.struggled / every) as u32;
                let struggled = r.struggled + TICK_DT;
                p.restraint = Some(Restraint { struggled, ..r });
                let pos = p.state.pos;
                if (struggled / every) as u32 != before {
                    self.emit_sound_from(pos, self.config.bonds.struggle_noise, SoundSource::Player(id));
                }
                if struggled >= self.escape_secs(&self.players[pi]).unwrap_or(0.0) {
                    let name = self.item_def(r.item).name.to_lowercase();
                    let cut = self.restraint_is_cut(r.item);
                    self.untie(pi);
                    self.note(id, true, if cut { format!("You snapped the {name}: your hands are free") } else { format!("You worked the {name} loose: your hands are free") });
                }
            }
            // An open search shuts when they part or the other is tied no longer.
            if let Some(t) = self.players[pi].frisking {
                let keep = self.config.bonds.keep_m;
                let ok = self.players.iter().position(|o| o.id == t).is_some_and(|ti| self.hands_check(pi, ti, HandsKind::Frisk, keep).is_ok());
                if !ok {
                    self.players[pi].frisking = None;
                    self.private_dirty.insert(id);
                }
            }
            let Some(mut job) = self.players[pi].hands else { continue };
            let keep = self.config.bonds.keep_m;
            let ti = self.players.iter().position(|o| o.id == job.target);
            let check = match ti {
                Some(ti) => self.hands_check(pi, ti, job.kind, keep),
                None => Err("they are gone".to_string()),
            };
            let (Some(ti), Ok(())) = (ti, &check) else {
                self.players[pi].hands = None;
                let what = match job.kind.act() {
                    HandsAct::Tie => "Couldn't tie their hands",
                    HandsAct::Free => "Couldn't free them",
                    HandsAct::Frisk => "Couldn't search them",
                    HandsAct::Take => "Couldn't take it",
                };
                self.note(id, false, format!("{what}: {}", check.err().unwrap_or_default()));
                continue;
            };
            job.left -= TICK_DT;
            if job.left > 0.0 {
                self.players[pi].hands = Some(job);
                continue;
            }
            self.players[pi].hands = None;
            let other = self.players[ti].id;
            match job.kind {
                HandsKind::Tie { item } => {
                    if take_one(&mut self.players[pi], item) {
                        self.refresh_carry(pi);
                        self.tie(ti, item, id);
                        let name = self.item_def(item).name.to_lowercase();
                        self.note(id, true, format!("You tied their hands ({name})"));
                        self.note(other, true, format!("Your hands are tied ({name}): stand still and struggle to get out"));
                    }
                }
                HandsKind::Free => {
                    self.untie(ti);
                    self.note(id, true, "You freed their hands".into());
                    self.note(other, true, "Your hands are free".into());
                }
                HandsKind::Frisk => {
                    self.players[pi].frisking = Some(other);
                    self.private_dirty.insert(id);
                    self.note(id, true, "Searching them".into());
                    self.note(other, true, "A survivor is going through your things".into());
                }
                HandsKind::Take { from, index, item, count } => match self.do_take_from(pi, ti, from, index, item, count) {
                    Ok((mine, theirs)) => {
                        self.note(id, true, mine);
                        self.note(other, true, theirs);
                    }
                    Err(why) => self.note(id, false, why),
                },
            }
        }
    }

    /// The take itself: `n` of the stack out of `ti`'s things into `pi`'s pockets and bags, or to
    /// the floor at `pi`'s feet when they don't fit or would be too heavy.
    fn do_take_from(&mut self, pi: usize, ti: usize, from: Place, index: u32, item: u16, count: u16) -> Result<(String, String), String> {
        let gone = || "it isn't there any more".to_string();
        let inv = &self.players[ti].inventory;
        // By index if that is still the item, else the first stack of it there.
        let (stack, index) = match from {
            Place::Carried | Place::Worn => {
                let list = if from == Place::Carried { &inv.carried } else { &inv.worn };
                let k = list.get(index as usize).filter(|s| s.item == item).map(|_| index as usize).or_else(|| list.iter().position(|s| s.item == item)).ok_or_else(gone)?;
                (list[k], k)
            }
            Place::Slot(s) => (inv.slot(s).filter(|s| s.item == item).ok_or_else(gone)?, 0),
            _ => return Err("that isn't theirs".into()),
        };
        let n = if count == 0 { stack.count } else { count.min(stack.count) };
        let items = &self.config.items;
        let def = &items[item as usize];
        // A bag only comes off when what's left still fits what's left of the room.
        if let (Place::Slot(_), Some(bag)) = (from, &def.bag)
            && inv.used(items) > inv.room(items, self.config.carry.pockets_l) - bag.capacity_l + 1e-3
        {
            return Err(format!("their {} has to be emptied first", def.name.to_lowercase()));
        }
        let name = if n > 1 { format!("{n} x {}", def.name.to_lowercase()) } else { def.name.to_lowercase() };
        let (size, taken) = (def.stack, stack.with_count(n));
        // Out of theirs.
        let inv = &mut self.players[ti].inventory;
        match from {
            Place::Carried | Place::Worn => {
                let list = if from == Place::Carried { &mut inv.carried } else { &mut inv.worn };
                list[index].count -= n;
                if list[index].count == 0 {
                    list.remove(index);
                } else {
                    list[index].used = 0;
                }
            }
            Place::Slot(s) => {
                let slot = inv.slot_mut(s);
                *slot = (stack.count > n).then(|| ItemStack { used: 0, ..stack.with_count(stack.count - n) });
            }
            _ => unreachable!("checked above"),
        }
        self.refresh_carry(ti);
        // Into ours, or onto the floor.
        let c = self.carry(&self.players[pi]);
        let fits = c.used_l + self.config.items[item as usize].volume_l * n as f32 <= c.room_l + 1e-3 && c.weight_kg + stack_weight(&self.config.items, &taken) <= c.limit_kg * self.config.carry.max_load;
        let p = &mut self.players[pi];
        p.run.items_taken += n as u32;
        if fits {
            merge_into(&mut p.inventory.carried, taken, n, size);
            self.refresh_carry(pi);
        } else {
            let (pos, facing, level) = (p.state.pos, p.state.facing, p.state.level);
            self.put_on_floor(taken, pos, facing, level);
        }
        let mine = if fits { format!("Took their {name}") } else { format!("Took their {name}: no room, it is at your feet") };
        Ok((mine, format!("A survivor took your {name}")))
    }
}

#[cfg(test)]
mod tests;
