//! Zombie storage: struct-of-arrays so the per-tick loops stay cache-friendly at thousands of zombies.

use glam::Vec2;
use serde::{Deserialize, Serialize};

use crate::map::EdgeRef;

/// Animation the renderer should play. Derived by the sim; replicated to clients.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum ZombieAnim {
    #[default]
    Idle = 0,
    Walk = 1,
    Attack = 2,
    /// Reeling from a hit or a shove.
    Stagger = 3,
    /// Knocked down, on the ground (will get up).
    Down = 4,
    /// A corpse.
    Dead = 5,
    /// Getting back up: the last [`GET_UP_SECS`] of being knocked down (still `Down` to the AI).
    GetUp = 6,
    /// Battering a door or window.
    Thump = 7,
}

/// A knocked-down zombie spends the last this-many seconds on the ground getting up
/// ([`ZombieAnim::GetUp`]); it can still be stomped meanwhile.
pub const GET_UP_SECS: f32 = 1.1;

/// What a zombie is doing (see `ai`). Server-only: clients see [`ZombieAnim`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum ZombieState {
    /// Shuffling about with no idea anyone is near.
    #[default]
    Wander = 0,
    /// Heading for something it heard (`target_pos`), then milling about there.
    Investigate = 1,
    /// After `target_player`: toward the player while it sees them, else to where it last saw them.
    Chase = 2,
    /// A short burst of speed at a player within reach of a lunge.
    Lunge = 3,
    /// In reach: striking on a cooldown.
    Attack = 4,
    /// Battering a shut door or window (`thump_edge`) that stands between it and its target (a
    /// player if `target_player` is set, else the point it investigates). Goes back to chasing or
    /// investigating once the way is open.
    Thump = 5,
    /// Reeling from a hit or a shove for `timer` seconds: can't attack or move with purpose.
    Staggered = 6,
    /// On the ground for `timer` seconds: can't attack, then gets up. Stomp it.
    Down = 7,
    /// A corpse. Skipped by the AI, senses, crowd separation and the spatial hash. `timer` counts
    /// the seconds since it died; old corpses are removed (`ZombieTuning::max_corpses`, `corpse_secs`).
    Dead = 8,
}

impl ZombieState {
    /// Chasing, lunging or attacking: locked on to a player.
    pub fn is_hunting(self) -> bool {
        matches!(self, Self::Chase | Self::Lunge | Self::Attack)
    }

    /// Hunting, or battering its way toward the player it hunts.
    pub fn is_after(self, player_target: u32) -> bool {
        self.is_hunting() || (self == Self::Thump && player_target != 0)
    }

    /// Staggered or down: out of action for a moment (senses and the state machine wait).
    pub fn is_reeling(self) -> bool {
        matches!(self, Self::Staggered | Self::Down)
    }
}

/// How much of the simulation a zombie gets, by how far it is from the nearest player (see
/// [`crate::tiers`]). Server-only: clients are only sent active zombies.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[repr(u8)]
pub enum Tier {
    /// Everything, every tick: senses, the full state machine, flow fields, crowd separation, combat.
    #[default]
    Active = 0,
    /// A few times a second (staggered): hearing, heading for what it heard or remembers, simple
    /// collision. No sight, combat or separation. Not sent to clients.
    Near = 1,
    /// Moved in bulk per region every few seconds: slow wander, drifting toward loud noises.
    Virtual = 2,
}

#[derive(Clone, Debug, Default)]
pub struct Zombies {
    /// Stable network/entity id. Never reused within a world.
    pub id: Vec<u32>,
    pub pos: Vec<Vec2>,
    pub vel: Vec<Vec2>,
    /// Facing angle in radians, 0 = +x, counter-clockwise toward +y.
    pub facing: Vec<f32>,
    /// Top walking speed in m/s.
    pub speed: Vec<f32>,
    pub anim: Vec<ZombieAnim>,
    /// Current idle wander direction and seconds until it changes.
    pub wander: Vec<Vec2>,
    pub wander_timer: Vec<f32>,
    pub state: Vec<ZombieState>,
    /// Player being hunted (Chase/Lunge/Attack), by id; 0 = none.
    pub target_player: Vec<u32>,
    /// Where it's heading: the sound it investigates, or where it last saw its player.
    pub target_pos: Vec<Vec2>,
    /// Seconds left of the current state: memory of the player (Chase), interest in the sound
    /// (Investigate) or in getting through (Thump), the lunge itself (Lunge). For a corpse, the
    /// seconds since it died.
    pub timer: Vec<f32>,
    /// Seconds until it can lunge or strike again.
    pub cooldown: Vec<f32>,
    /// Seconds until its next groan while hunting.
    pub groan_timer: Vec<f32>,
    /// Ticks left of "sees its player" since the last sight check that saw them.
    pub sees: Vec<u8>,
    /// The way to its target is a clear straight line (rechecked every few ticks), so it walks
    /// straight there instead of asking for a flow field.
    pub direct: Vec<bool>,
    /// The door or window it is battering (Thump).
    pub thump_edge: Vec<EdgeRef>,
    /// Hit points left; dead at 0.
    pub hp: Vec<f32>,
    /// Giving up: where it was when it last made headway, and seconds since then spent pushing
    /// against walls (see `ZombieTuning::give_up_secs`).
    pub anchor: Vec<Vec2>,
    pub stuck: Vec<f32>,
    /// Seconds left of having lost interest (`ZombieTuning::bored_secs`).
    pub bored: Vec<f32>,
    /// Level of detail it is simulated at (see [`Tier`]). Every other column is kept as is across
    /// tier changes, so a zombie keeps what it was doing.
    pub tier: Vec<Tier>,
}

impl Zombies {
    pub fn len(&self) -> usize {
        self.pos.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pos.is_empty()
    }

    pub fn push(&mut self, id: u32, pos: Vec2, facing: f32, speed: f32, hp: f32) {
        self.id.push(id);
        self.pos.push(pos);
        self.vel.push(Vec2::ZERO);
        self.facing.push(facing);
        self.speed.push(speed);
        self.anim.push(ZombieAnim::Idle);
        self.wander.push(Vec2::ZERO);
        self.wander_timer.push(0.0);
        self.state.push(ZombieState::Wander);
        self.target_player.push(0);
        self.target_pos.push(pos);
        self.timer.push(0.0);
        self.cooldown.push(0.0);
        self.groan_timer.push(0.0);
        self.sees.push(0);
        self.direct.push(false);
        self.thump_edge.push(EdgeRef::default());
        self.hp.push(hp);
        self.anchor.push(pos);
        self.stuck.push(0.0);
        self.bored.push(0.0);
        self.tier.push(Tier::Active);
    }

    /// Remove every zombie `remove` picks (by index), keeping the rest in order, so ids stay
    /// sorted. Ids are never reused.
    pub fn remove_where(&mut self, remove: impl Fn(usize) -> bool) {
        let keep: Vec<bool> = (0..self.len()).map(|i| !remove(i)).collect();
        if keep.iter().all(|k| *k) {
            return;
        }
        fn retain<T>(v: &mut Vec<T>, keep: &[bool]) {
            let mut k = keep.iter();
            v.retain(|_| *k.next().unwrap_or(&true));
        }
        retain(&mut self.id, &keep);
        retain(&mut self.pos, &keep);
        retain(&mut self.vel, &keep);
        retain(&mut self.facing, &keep);
        retain(&mut self.speed, &keep);
        retain(&mut self.anim, &keep);
        retain(&mut self.wander, &keep);
        retain(&mut self.wander_timer, &keep);
        retain(&mut self.state, &keep);
        retain(&mut self.target_player, &keep);
        retain(&mut self.target_pos, &keep);
        retain(&mut self.timer, &keep);
        retain(&mut self.cooldown, &keep);
        retain(&mut self.groan_timer, &keep);
        retain(&mut self.sees, &keep);
        retain(&mut self.direct, &keep);
        retain(&mut self.thump_edge, &keep);
        retain(&mut self.hp, &keep);
        retain(&mut self.anchor, &keep);
        retain(&mut self.stuck, &keep);
        retain(&mut self.bored, &keep);
        retain(&mut self.tier, &keep);
    }

    pub fn is_dead(&self, i: usize) -> bool {
        self.state[i] == ZombieState::Dead
    }

    /// Index of the zombie with this id.
    pub fn index_of(&self, id: u32) -> Option<usize> {
        // Ids are handed out in increasing order and never reused, and removal keeps the order, so
        // the list is sorted.
        self.id.binary_search(&id).ok()
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }
}
