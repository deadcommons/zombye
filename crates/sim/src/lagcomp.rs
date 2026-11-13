//! Lag compensation for melee (#23, PLAN §3): a short history of where zombies and players stood,
//! so a blow can land where its attacker saw the target.
//!
//! A client over the network shows the world a little late: the snapshot it draws left the server
//! a while ago, and the input it sends takes a while to arrive. Its own survivor is predicted, so
//! on its screen a swing lands with its survivor where the server will have it when the blow lands,
//! and everything else where the server had it some ticks before. Each tick such a client says
//! which snapshot it was showing ([`World::player_saw`], `net::Intent::Seen`); when its blow lands
//! the targets are tested where they stood at that tick (`combat::land`), at most
//! `CombatTuning::lag_comp_max_secs` back.
//!
//! Derived and short-lived: not saved, not in `state_hash`. Nothing is kept (and nothing changes)
//! until a player reports what it saw, so single-player and worlds without remote clients run
//! exactly as before. After a load the history is empty and blows land unrewound until clients
//! have reported again (a tick or so).

use glam::Vec2;

use crate::world::World;
use crate::zombies::{Tier, ZombieState};
use crate::TICK_DT;

/// Bodies never move faster than this (m/s; knockback is capped at 5, a runner does 5.5): how much
/// wider a rewound blow looks for candidates round where they stand now.
pub(crate) const MAX_BODY_SPEED: f32 = 6.0;

/// Where everybody stood at the end of one tick (what the snapshot of that tick showed).
#[derive(Clone, Debug, Default)]
pub struct Frame {
    /// `World::tick` when it was taken, the tick of the snapshot that showed it.
    pub tick: u64,
    /// Living zombies on the active tier (the only ones clients are sent), in id order.
    zombie_ids: Vec<u32>,
    zombie_pos: Vec<Vec2>,
    players: Vec<(u32, Vec2)>,
}

impl Frame {
    /// Where zombie `id` stood, if it was there (and active) then.
    pub fn zombie(&self, id: u32) -> Option<Vec2> {
        self.zombie_ids.binary_search(&id).ok().map(|i| self.zombie_pos[i])
    }

    /// Where player `id` stood, if it was in the world then.
    pub fn player(&self, id: u32) -> Option<Vec2> {
        self.players.iter().find(|p| p.0 == id).map(|p| p.1)
    }
}

/// The last few ticks' [`Frame`]s, oldest first.
#[derive(Clone, Debug, Default)]
pub struct History {
    frames: Vec<Frame>,
}

impl History {
    /// The frame of `tick`, if it is still kept.
    pub fn frame(&self, tick: u64) -> Option<&Frame> {
        self.frames.iter().find(|f| f.tick == tick)
    }

    pub fn len(&self) -> usize {
        self.frames.len()
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }
}

impl World {
    /// How many ticks back a blow may be rewound (`CombatTuning::lag_comp_max_secs`).
    pub fn lag_comp_ticks(&self) -> u64 {
        (self.config.combat.lag_comp_max_secs.max(0.0) / TICK_DT).round() as u64
    }

    /// Player `id`'s client says it is showing the snapshot of `tick` (as it sends the input that
    /// follows). Never ahead of the world, never back in time. From then on its blows land where
    /// it saw their targets, within the cap.
    pub fn player_saw(&mut self, id: u32, tick: u64) {
        let now = self.tick;
        if let Some(p) = self.players.iter_mut().find(|p| p.id == id) {
            p.seen_tick = tick.min(now).max(p.seen_tick);
        }
    }

    /// How far behind the world player `pi`'s view is, in ticks: 0 for one that never said (a
    /// local player), at most the cap.
    pub(crate) fn view_lag_ticks(&self, pi: usize) -> u64 {
        match self.players[pi].seen_tick {
            0 => 0,
            seen => self.tick.saturating_sub(seen).min(self.lag_comp_ticks()),
        }
    }

    /// Player `id`'s view lag as blows are compensated for it, in seconds (0: none).
    pub fn view_lag_secs(&self, id: u32) -> f32 {
        self.players.iter().position(|p| p.id == id).map_or(0.0, |pi| self.view_lag_ticks(pi) as f32 * TICK_DT)
    }

    /// The tick whose positions a blow of player `pi` landing now is tested against: what its
    /// client showed, `ahead` ticks on (a blow landing early in a trade), no further back than
    /// the cap. `None`: as things stand now.
    pub(crate) fn rewind_tick(&self, pi: usize, ahead: u64) -> Option<u64> {
        let seen = self.players[pi].seen_tick;
        if seen == 0 || self.config.combat.lag_comp_max_secs <= 0.0 {
            return None;
        }
        let tick = (seen + ahead).max(self.tick.saturating_sub(self.lag_comp_ticks()));
        (tick < self.tick).then_some(tick)
    }

    /// The positions kept for lag compensation (tests, tools).
    pub fn lag_history(&self) -> &History {
        &self.lag_history
    }

    /// End of a tick: keep where everybody stands, for blows landing in the next few. Nothing is
    /// kept while no player has reported what it sees.
    pub(crate) fn record_lag_frame(&mut self) {
        let keep = self.lag_comp_ticks() as usize + 1;
        let h = &mut self.lag_history;
        if keep <= 1 || !self.players.iter().any(|p| p.seen_tick != 0) {
            h.frames.clear();
            return;
        }
        // Reuse the oldest frame's buffers.
        let mut f = if h.frames.len() >= keep { h.frames.remove(0) } else { Frame::default() };
        f.tick = self.tick;
        f.zombie_ids.clear();
        f.zombie_pos.clear();
        let z = &self.zombies;
        for i in 0..z.len() {
            if z.tier[i] == Tier::Active && z.state[i] != ZombieState::Dead {
                f.zombie_ids.push(z.id[i]);
                f.zombie_pos.push(z.pos[i]);
            }
        }
        f.players.clear();
        f.players.extend(self.players.iter().filter(|p| !p.dead).map(|p| (p.id, p.state.pos)));
        h.frames.push(f);
    }
}
