//! What one survivor knows of another (#66, docs/PLAN.md §4.8 "fair information"): whom it sees,
//! and whom it only heard.
//!
//! With open PvP a client must learn about another player only through its own survivor's senses,
//! or a modified client is a wallhack. So the world keeps, for every pair of players, whether one
//! **sees** the other, by the rule zombies see players by ([`senses::eyes_range`]: a view cone,
//! daylight, the other's stance, whether it moves, its Stealth, then a line of sight) with a
//! survivor's eyes ([`PlayerSightTuning`], which follow the client's fog of war: what your screen
//! shows lit is where you'd see someone), and for a moment after (`grace_secs`), so nobody
//! flickers at the edge of sight. A player that isn't seen but makes a noise in earshot (the
//! zombies' rule again: the sound's radius, `wall_muffle` of it with no line of sight) is
//! **heard**: a sound at a position ([`Heard`]), not the player.
//!
//! Servers cut each client's snapshots by this (`net`), and MCP `observe` an agent's view.
//!
//! Nothing here feeds back into the simulation: it is derived every tick from the players and the
//! tick's sounds, isn't saved and isn't in the state hash. With fewer than two players it does
//! nothing at all.

use std::collections::BTreeMap;

use glam::Vec2;
use serde::{Deserialize, Serialize};

use crate::TICK_RATE;
use crate::map::TileMap;
use crate::players::Stance;
use crate::senses::{self, Cone, Eyes, SoundSource, Target};
use crate::vision::line_of_sight;
use crate::world::World;

/// A survivor's eyes and ears for other survivors. Ours. The ranges and the cone are the client's
/// fog of war's (70 m by day, 10 m at night, nothing behind you past arm's length).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PlayerSightTuning {
    /// How far a walking survivor is seen ahead in full daylight, and at night (metres).
    pub sight_day: f32,
    pub sight_night: f32,
    /// Half-angle of the cone seen at full range, and of the wider one seen at
    /// `peripheral_mult` of it (degrees). Beyond that only `rear_range`.
    pub cone_deg: f32,
    pub peripheral_deg: f32,
    pub peripheral_mult: f32,
    /// Anyone this close is noticed whichever way you face (still not through a wall).
    pub rear_range: f32,
    /// Range multipliers for whoever is looked at: sneaking, running, standing still.
    pub sneak_mult: f32,
    pub run_mult: f32,
    pub still_mult: f32,
    /// Someone holding a light is seen at least this far at night.
    pub lit_range: f32,
    /// Half-angle of a held flashlight's beam (degrees): whoever stands in yours is seen out to
    /// its reach (the light's radius + 2 m) whatever the hour.
    pub beam_deg: f32,
    /// Seen for this long after sight is lost, so players don't flicker at the edge.
    pub grace_secs: f32,
    /// A player heard and not seen is told as a sound at most this often (a louder one at once).
    pub heard_every_secs: f32,
    /// How long a heard sound is remembered ([`Awareness::heard_lately`]).
    pub heard_secs: f32,
}

impl Default for PlayerSightTuning {
    fn default() -> Self {
        Self {
            sight_day: 70.0,
            sight_night: 10.0,
            cone_deg: 105.0,
            peripheral_deg: 125.0,
            peripheral_mult: 0.5,
            rear_range: 2.5,
            sneak_mult: 0.6,
            run_mult: 1.2,
            still_mult: 0.85,
            lit_range: 35.0,
            beam_deg: 33.0,
            grace_secs: 1.0,
            heard_every_secs: 0.4,
            heard_secs: 4.0,
        }
    }
}

impl PlayerSightTuning {
    fn eyes(&self) -> Eyes {
        Eyes {
            day: self.sight_day,
            night: self.sight_night,
            cone: Cone::from_degrees(self.cone_deg, self.peripheral_deg),
            peripheral_mult: self.peripheral_mult,
            rear_range: self.rear_range,
            sneak_mult: self.sneak_mult,
            run_mult: self.run_mult,
            still_mult: self.still_mult,
        }
    }
}

/// A survivor as another's senses take it, and as a pair of eyes itself.
#[derive(Clone, Copy, Debug)]
pub struct Body {
    pub id: u32,
    pub pos: Vec2,
    pub facing: f32,
    pub stance: Stance,
    /// Moved this tick.
    pub moving: bool,
    /// Stealth (#46): multiplies every sight range against it.
    pub sight_mult: f32,
    /// The radius of the light it holds (0: none).
    pub light: f32,
    /// Eyes shut: sees nobody.
    pub asleep: bool,
}

impl Body {
    /// Standing at `pos` facing `facing`, walking, with no light.
    pub fn standing(id: u32, pos: Vec2, facing: f32) -> Self {
        Self { id, pos, facing, stance: Stance::Walk, moving: true, sight_mult: 1.0, light: 0.0, asleep: false }
    }
}

/// How far `viewer` can make `target` out right now (before the line of sight), in metres.
pub fn sight_range(t: &PlayerSightTuning, daylight: f32, viewer: &Body, target: &Body) -> f32 {
    let to = target.pos - viewer.pos;
    let d = to.length();
    let dir = if d > 1e-4 { to / d } else { Vec2::from_angle(viewer.facing) };
    let mut eyes = t.eyes();
    let dark = 1.0 - daylight;
    if target.light > 0.0 {
        // A light in the dark shows from far off, whoever holds it and however quietly.
        eyes.night = eyes.night.max(t.lit_range);
    }
    let seen = Target { id: target.id, pos: target.pos, stance: target.stance, moving: target.moving, ignored: false, sight_mult: target.sight_mult };
    let mut range = senses::eyes_range(&eyes, daylight, &seen, viewer.facing, dir);
    // In our own beam.
    if viewer.light > 0.0 && dark > 0.0 && Vec2::from_angle(viewer.facing).dot(dir) >= t.beam_deg.to_radians().cos() {
        range = range.max(viewer.light + 2.0);
    }
    range
}

/// Does `viewer` see `target`: within [`sight_range`] with a line of sight (windows don't block
/// it; walls and shut doors do)?
pub fn sees(map: &TileMap, t: &PlayerSightTuning, daylight: f32, viewer: &Body, target: &Body) -> bool {
    !viewer.asleep && viewer.pos.distance(target.pos) <= sight_range(t, daylight, viewer, target) && line_of_sight(map, viewer.pos, target.pos)
}

/// A player `listener` heard without seeing it: a sound at a position, as loud as its radius.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Heard {
    pub listener: u32,
    pub pos: Vec2,
    /// How far the sound carries in the open (metres): footsteps 1-14, a swing, a door, glass.
    pub radius: f32,
    /// [`Awareness::now`] when it was heard.
    pub at: u64,
}

/// Who sees whom and who heard whom, kept by the world ([`World::awareness`]).
#[derive(Clone, Debug, Default)]
pub struct Awareness {
    /// Updates so far (one a tick with two or more players).
    now: u64,
    /// (viewer, seen) to the update it was last in sight.
    seen: BTreeMap<(u32, u32), u64>,
    /// (listener, heard) to when the listener was last told and how loud that was.
    told: BTreeMap<(u32, u32), (u64, f32)>,
    heard: Vec<Heard>,
}

fn ticks(secs: f32) -> u64 {
    (secs.max(0.0) * TICK_RATE as f32).round() as u64
}

impl Awareness {
    pub fn now(&self) -> u64 {
        self.now
    }

    /// Does `viewer` see `target` in this tick, by sight now or within the grace period after?
    /// A player always sees itself.
    pub fn sees(&self, viewer: u32, target: u32) -> bool {
        viewer == target || self.seen.contains_key(&(viewer, target))
    }

    /// The sounds of unseen players `listener` heard in the last tick.
    pub fn heard_now(&self, listener: u32) -> impl Iterator<Item = &Heard> {
        self.heard.iter().filter(move |h| h.listener == listener && h.at == self.now)
    }

    /// Every unseen player's sound heard in the last tick, whoever heard it.
    pub fn all_heard_now(&self) -> impl Iterator<Item = &Heard> {
        self.heard.iter().filter(|h| h.at == self.now)
    }

    /// The sounds of unseen players `listener` heard lately (`heard_secs`), oldest first.
    pub fn heard_lately(&self, listener: u32) -> impl Iterator<Item = &Heard> {
        self.heard.iter().filter(move |h| h.listener == listener)
    }

    fn clear(&mut self) {
        self.seen.clear();
        self.told.clear();
        self.heard.clear();
    }
}

impl World {
    /// Who sees and heard whom among the players (#66).
    pub fn awareness(&self) -> &Awareness {
        &self.awareness
    }

    /// Does player `viewer` see player `target` (now, or a moment ago)? With one player, or
    /// before the first tick, only itself.
    pub fn player_sees(&self, viewer: u32, target: u32) -> bool {
        self.awareness.sees(viewer, target)
    }

    /// The players as each other's senses take them this tick.
    pub fn bodies(&self) -> Vec<Body> {
        self.players
            .iter()
            .map(|p| {
                let target = self.targets.iter().find(|t| t.id == p.id);
                let inv = &p.inventory;
                let light = [inv.hand, inv.off_hand].into_iter().flatten().filter_map(|s| self.config.items.get(s.item as usize)?.light.as_ref()).map(|l| l.radius_m).fold(0.0, f32::max);
                Body {
                    id: p.id,
                    pos: p.state.pos,
                    facing: p.state.facing,
                    stance: p.state.stance,
                    moving: target.is_some_and(|t| t.moving),
                    sight_mult: target.map_or(1.0, |t| t.sight_mult),
                    light,
                    asleep: p.needs.asleep(),
                }
            })
            .collect()
    }

    /// Does `viewer` see `target` right now, by the rule alone (no grace period)? For tools and tests.
    pub fn sees_now(&self, viewer: u32, target: u32) -> bool {
        let bodies = self.bodies();
        let find = |id| bodies.iter().find(|b| b.id == id);
        let (Some(v), Some(t)) = (find(viewer), find(target)) else { return false };
        sees(&self.map, &self.config.player_sight, senses::daylight(self.time_of_day()), v, t)
    }

    /// Once a tick, after everything has moved and with the tick's sounds still in `heard`.
    pub(crate) fn step_awareness(&mut self) {
        if self.players.len() < 2 {
            if self.awareness.now != 0 {
                self.awareness.clear();
            }
            return;
        }
        let t = &self.config.player_sight;
        let daylight = senses::daylight(self.time_of_day());
        let bodies = self.bodies();
        let a = &mut self.awareness;
        a.now += 1;
        let now = a.now;
        // Nobody is told of anything beyond the snapshots' reach.
        let reach = self.config.tiers.interest_radius;
        for v in &bodies {
            for o in &bodies {
                if v.id != o.id && v.pos.distance_squared(o.pos) <= reach * reach && sees(&self.map, t, daylight, v, o) {
                    a.seen.insert((v.id, o.id), now);
                }
            }
        }
        let grace = ticks(t.grace_secs);
        a.seen.retain(|_, at| now - *at <= grace);

        // Players heard and not seen. Ears stay open asleep (a noise is what wakes a sleeper).
        let (every, keep) = (ticks(t.heard_every_secs), ticks(t.heard_secs));
        let muffle = self.config.zombie.wall_muffle;
        for s in &self.heard {
            let SoundSource::Player(by) = s.source else { continue };
            for (v, p) in bodies.iter().zip(&self.players) {
                if v.id == by || p.dead || a.seen.contains_key(&(v.id, by)) {
                    continue;
                }
                let d = v.pos.distance(s.pos);
                if d > s.radius || (d > s.radius * muffle && !line_of_sight(&self.map, v.pos, s.pos)) {
                    continue;
                }
                let last = a.told.get(&(v.id, by)).copied();
                if last.is_none_or(|(at, radius)| now - at >= every || s.radius > radius * 1.5) {
                    a.told.insert((v.id, by), (now, s.radius));
                    a.heard.push(Heard { listener: v.id, pos: s.pos, radius: s.radius, at: now });
                }
            }
        }
        a.told.retain(|_, (at, _)| now - *at <= every.max(1) * 4);
        a.heard.retain(|h| now - h.at <= keep);
    }
}

#[cfg(test)]
mod tests;
