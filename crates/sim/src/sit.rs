//! Sitting down: on a sofa, a bed's edge, a toilet, a bench or a deckchair beside you, else on
//! the ground (Q in the game, `Intent::Sit`, MCP `sit`).
//!
//! A seated survivor doesn't move, can't run or climb, gets its breath back faster
//! ([`SitTuning::endurance_regen`]) and slowly loses tiredness ([`SitTuning::rest_per_h`]) where
//! standing it would gain it. Moving (the client sends `Sit` off on WASD, and any movement input
//! stands it up on the server too), swinging, shoving, stomping, climbing, lying down, being
//! knocked about, tied up or killed stands it up. Eating, drinking, crafting, cooking and talking
//! don't: sit by the fire and cook.
//!
//! What it sits on is worked out from the map alone ([`seat_at`]), so a client draws the body on
//! the same seat the server means. `Player::seated` is saved in an optional `SEAT` section and is in
//! the state hash only while set, so other worlds save and hash as before.

use glam::Vec2;
use serde::{Deserialize, Serialize};

use crate::map::TileMap;
use crate::world::World;

/// What a survivor sits on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum SeatKind {
    Ground = 1,
    Sofa,
    Bed,
    Toilet,
    Bench,
}

impl SeatKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Ground => "ground",
            Self::Sofa => "sofa",
            Self::Bed => "edge of the bed",
            Self::Toilet => "toilet",
            Self::Bench => "bench",
        }
    }

    /// How high the seat is (m): where the body's hips go.
    pub fn height(self) -> f32 {
        match self {
            Self::Ground => 0.0,
            Self::Sofa => 0.42,
            Self::Bed => 0.5,
            Self::Toilet => 0.42,
            Self::Bench => 0.45,
        }
    }

    /// 0 none, else the kind: three bits of `PlayerPacked::bits`.
    pub fn bits(s: Option<Self>) -> u32 {
        s.map_or(0, |s| s as u32)
    }

    pub fn from_bits(b: u32) -> Option<Self> {
        Some(match b {
            1 => Self::Ground,
            2 => Self::Sofa,
            3 => Self::Bed,
            4 => Self::Toilet,
            5 => Self::Bench,
            _ => return None,
        })
    }
}

/// Where a survivor at `pos` on floor `level` sits: the seat, where its hips go and which way it
/// faces then (`None`: the way it already faces).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Seat {
    pub kind: SeatKind,
    pub at: Vec2,
    pub facing: Option<f32>,
}

/// Furniture and street props this close (m, to the nearest edge of their footprint) are sat on.
pub const SEAT_REACH: f32 = 0.9;

fn seat_of_furniture(kind: &str) -> Option<SeatKind> {
    Some(match kind {
        "sofa" => SeatKind::Sofa,
        "bed" => SeatKind::Bed,
        "toilet" => SeatKind::Toilet,
        _ => return None,
    })
}

fn seat_of_prop(id: &str) -> Option<SeatKind> {
    Some(match id {
        "park_bench" | "bench" | "deckchair" | "sun_lounger" => SeatKind::Bench,
        _ => return None,
    })
}

/// Facing (radians, 0 east) of the way a side points.
fn side_angle(s: crate::map::Side) -> f32 {
    let o = s.offset().as_vec2();
    o.y.atan2(o.x)
}

/// How far in from a seat's front edge the hips go (m): the thighs' length over it, the knees at
/// the edge and the feet on the floor in front.
const HIPS_IN: f32 = 0.3;

/// Where the hips go on the footprint `lo..hi` facing `front`: `HIPS_IN` back from its front edge,
/// and along it as near `pos` as keeps `inset` from its ends.
fn on_seat(lo: Vec2, hi: Vec2, pos: Vec2, front: crate::map::Side, inset: f32) -> Vec2 {
    let mid = (lo + hi) * 0.5;
    let a = (lo + Vec2::splat(inset)).min(mid);
    let b = (hi - Vec2::splat(inset)).max(mid);
    let mut at = pos.clamp(a, b);
    let f = front.offset().as_vec2();
    let back = HIPS_IN.min((hi - lo).dot(f.abs()) * 0.5);
    if f.x != 0.0 {
        at.x = if f.x > 0.0 { hi.x - back } else { lo.x + back };
    } else {
        at.y = if f.y > 0.0 { hi.y - back } else { lo.y + back };
    }
    at
}

/// Which side of a bed someone at `pos` sits on: the edge nearest it, never the head end (the
/// furniture's facing is its foot end).
fn bed_side(lo: Vec2, hi: Vec2, pos: Vec2, foot: crate::map::Side) -> crate::map::Side {
    use crate::map::Side;
    let gap = |s: Side| match s {
        Side::N => lo.y - pos.y,
        Side::S => pos.y - hi.y,
        Side::W => lo.x - pos.x,
        Side::E => pos.x - hi.x,
    };
    Side::ALL.into_iter().filter(|&s| s != foot.opposite()).max_by(|&a, &b| gap(a).total_cmp(&gap(b))).unwrap_or(foot)
}

/// What someone at `pos` on floor `level` sits on: the nearest sofa, bed or toilet in its room, or
/// outdoors the nearest bench, deckchair or sun lounger, within [`SEAT_REACH`]; else the ground
/// where it stands. The server's rule and the client's (which draws the body there).
pub fn seat_at(map: &TileMap, pos: Vec2, level: u8) -> Seat {
    let room = map.room_on(level, pos);
    let mut best: Option<(f32, Seat)> = None;
    let mut consider = |d: f32, seat: Seat| {
        if d <= SEAT_REACH && best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, seat));
        }
    };
    if room != 0 {
        for f in map.buildings.iter().flat_map(|b| &b.furniture).filter(|f| f.level == level as i8) {
            let Some(kind) = seat_of_furniture(&f.kind) else { continue };
            let lo = f.min.as_vec2();
            let hi = lo + f.size.as_vec2();
            if map.room_on(level, (lo + hi) * 0.5) != room {
                continue;
            }
            let d = pos.clamp(lo, hi).distance(pos);
            let front = if kind == SeatKind::Bed { bed_side(lo, hi, pos, f.facing) } else { f.facing };
            consider(d, Seat { kind, at: on_seat(lo, hi, pos, front, 0.35), facing: Some(side_angle(front)) });
        }
    } else if level == 0 {
        let t = pos.floor().as_ivec2();
        for p in map.street_props.iter().filter(|p| (p.tile - t).abs().max_element() <= 3) {
            let Some(kind) = map.prop_kinds.get(p.kind as usize).and_then(|k| seat_of_prop(&k.id)) else { continue };
            let (min, size) = map.street_prop_rect(p);
            let lo = min.as_vec2();
            let hi = lo + size.as_vec2();
            let d = pos.clamp(lo, hi).distance(pos);
            consider(d, Seat { kind, at: on_seat(lo, hi, pos, p.facing, 0.4), facing: Some(side_angle(p.facing)) });
        }
    }
    best.map_or(Seat { kind: SeatKind::Ground, at: pos, facing: None }, |(_, s)| s)
}

/// Sitting's numbers (`WorldConfig::sit`; serde defaults, a server's rule like `speech`: not saved).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SitTuning {
    /// Endurance comes back this many times as fast seated.
    pub endurance_regen: f32,
    /// Tiredness lost per game hour seated (awake, standing, it rises by `NeedsTuning::tired_per_h`).
    pub rest_per_h: f32,
}

impl Default for SitTuning {
    fn default() -> Self {
        Self { endurance_regen: 2.0, rest_per_h: 0.02 }
    }
}

impl World {
    /// Player `id` sits down (`Some(true)`), stands up (`Some(false)`) or the other (`None`).
    /// Refused (with a note) dead, asleep, knocked about, climbing, on a flight of stairs, logged
    /// out or in the middle of a swing. Returns what it did.
    pub fn player_sit(&mut self, id: u32, sit: Option<bool>) -> Result<String, String> {
        let Some(pi) = self.players.iter().position(|p| p.id == id) else { return Err(format!("no player {id}")) };
        let p = &self.players[pi];
        let want = sit.unwrap_or(p.seated.is_none());
        if !want {
            return Ok(if self.stand_up(pi) { "you get up".into() } else { "already standing".into() });
        }
        if p.seated.is_some() {
            return Ok("already sitting".into());
        }
        let why = if p.dead {
            Some("dead")
        } else if p.needs.asleep() || p.away.is_some() {
            Some("asleep")
        } else if p.knock.is_some() {
            Some("still reeling")
        } else if p.state.climb.is_some() {
            Some("climbing")
        } else if self.map.stair_at(p.state.pos).is_some() && p.state.level == 0 {
            Some("not on the stairs")
        } else if p.action.is_some() {
            Some("busy fighting")
        } else {
            None
        };
        if let Some(why) = why {
            self.note(id, false, format!("can't sit down: {why}"));
            return Err(why.into());
        }
        let seat = seat_at(&self.map, p.state.pos, p.state.level);
        let p = &mut self.players[pi];
        p.seated = Some(seat.kind);
        p.queued = None;
        self.refresh_carry(pi);
        let text = format!("you sit down on the {}", seat.kind.name());
        self.note(id, true, text.clone());
        Ok(text)
    }

    /// Player `pi` stands up, if seated. Returns whether it was.
    pub(crate) fn stand_up(&mut self, pi: usize) -> bool {
        if self.players[pi].seated.take().is_none() {
            return false;
        }
        self.refresh_carry(pi);
        true
    }

    /// Once a tick, after movement: a seated survivor that is dead, asleep, knocked about, tied,
    /// climbing, or asked to move gets up.
    pub(crate) fn step_sit(&mut self) {
        for pi in 0..self.players.len() {
            let p = &self.players[pi];
            if p.seated.is_some() && (p.dead || p.needs.asleep() || p.away.is_some() || p.knock.is_some() || p.restraint.is_some() || p.state.climb.is_some() || p.action.is_some() || p.input.move_dir.length_squared() > 0.01) {
                self.stand_up(pi);
            }
        }
    }
}

#[cfg(test)]
mod tests;
