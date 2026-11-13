//! Optional groups (#73, docs/PLAN.md §4.8: "an optional group (shared map markers, seeing your
//! group's names); groups never switch damage off").
//!
//! A group is a list of players who agreed to it: one **invites** another it can see near by
//! ([`World::group_invite`]), the other **accepts** ([`World::group_accept`]) within
//! `invite_secs`, and anyone **leaves** when they like ([`World::group_leave`]; a group of one
//! is no group). What it gives is information and nothing else: members are told each other's
//! names ([`GroupView::members`]; a client still only draws a member its survivor sees, so a
//! name is a label on someone in view, not a position), and share **markers**, points on the
//! map any member puts down or takes away ([`World::group_mark`], at most `max_markers`, the
//! oldest going first). **Nothing in combat reads a group**: blows land on members as on anyone.
//!
//! Membership is the player's (it outlasts a death and a new survivor). Names come from whoever
//! runs the world ([`World::set_player_name`]: a server's join names; without one, "Survivor N").
//! Groups, markers and invitations are saved (the optional `GRPS` section) and in the state hash
//! only while there are any.

use glam::Vec2;
use serde::{Deserialize, Serialize};

use crate::TICK_RATE;
use crate::world::World;

/// Group sizes, invitations and markers. Ours.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GroupTuning {
    pub max_members: usize,
    pub max_markers: usize,
    /// How near the invited survivor has to be (metres; it must be in sight too).
    pub invite_m: f32,
    /// How long an invitation stands (seconds).
    pub invite_secs: f32,
}

impl Default for GroupTuning {
    fn default() -> Self {
        Self { max_members: 8, max_markers: 8, invite_m: 12.0, invite_secs: 60.0 }
    }
}

/// A point on the map a group shares.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Marker {
    pub id: u32,
    pub pos: Vec2,
    /// The member who put it down.
    pub by: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Group {
    pub id: u32,
    /// Player ids, in the order they joined.
    pub members: Vec<u32>,
    /// Oldest first.
    pub markers: Vec<Marker>,
}

/// An invitation waiting for its answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Invite {
    pub from: u32,
    pub to: u32,
    /// The tick it lapses at.
    pub until: u64,
}

/// Every group in the world, and the invitations out.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Groups {
    pub list: Vec<Group>,
    pub invites: Vec<Invite>,
    pub next_group: u32,
    pub next_marker: u32,
}

impl Groups {
    pub fn is_empty(&self) -> bool {
        self.list.is_empty() && self.invites.is_empty()
    }

    /// The group `player` is in.
    pub fn of(&self, player: u32) -> Option<&Group> {
        self.list.iter().find(|g| g.members.contains(&player))
    }

    fn index_of(&self, player: u32) -> Option<usize> {
        self.list.iter().position(|g| g.members.contains(&player))
    }

    /// Are the two in one group?
    pub fn together(&self, a: u32, b: u32) -> bool {
        self.of(a).is_some_and(|g| g.members.contains(&b))
    }

    /// Whether this could have come out of play: for loading. `known` says which player ids exist.
    pub fn check(&self, known: impl Fn(u32) -> bool, in_bounds: impl Fn(Vec2) -> bool, t: &GroupTuning) -> Result<(), String> {
        let mut seen: Vec<u32> = Vec::new();
        for g in &self.list {
            if g.id == 0 || g.id >= self.next_group.max(1) || g.members.len() < 2 || g.members.len() > t.max_members.max(64) {
                return Err(format!("group {}: {} members", g.id, g.members.len()));
            }
            for &m in &g.members {
                if !known(m) || seen.contains(&m) {
                    return Err(format!("group {}: member {m} is unknown or in two groups", g.id));
                }
                seen.push(m);
            }
            if g.markers.len() > 64 || g.markers.iter().any(|m| !m.pos.is_finite() || !in_bounds(m.pos) || m.id >= self.next_marker.max(1)) {
                return Err(format!("group {}: a marker off the map", g.id));
            }
        }
        if self.invites.len() > 4096 || self.invites.iter().any(|i| !known(i.from) || !known(i.to) || i.from == i.to) {
            return Err("an invitation between unknown players".into());
        }
        Ok(())
    }
}

/// A member as its group is told of it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MemberView {
    pub id: u32,
    pub name: String,
}

/// What one player's client is told of its group and the invitations to it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct GroupView {
    pub player: u32,
    /// Its group's members, itself included, in joining order; empty with no group.
    pub members: Vec<MemberView>,
    pub markers: Vec<Marker>,
    /// Who has invited it (ids and names), oldest first.
    pub invites: Vec<MemberView>,
}

impl GroupView {
    pub fn name_of(&self, id: u32) -> Option<&str> {
        self.members.iter().find(|m| m.id == id).map(|m| m.name.as_str())
    }
}

impl World {
    pub fn groups(&self) -> &Groups {
        &self.groups
    }

    /// The name player `id` goes by (a server's join name); "Survivor N" when none was given.
    pub fn player_name(&self, id: u32) -> String {
        self.names.get(&id).cloned().unwrap_or_else(|| format!("Survivor {id}"))
    }

    /// Whoever runs the world says what player `id` is called (not saved: said again on start).
    pub fn set_player_name(&mut self, id: u32, name: &str) {
        self.names.insert(id, name.to_string());
    }

    /// What player `id`'s client is told: its group and the invitations to it. `None` with neither.
    pub fn group_view(&self, id: u32) -> Option<GroupView> {
        let member = |id: u32| MemberView { id, name: self.player_name(id) };
        let g = self.groups.of(id);
        let invites: Vec<MemberView> = self.groups.invites.iter().filter(|i| i.to == id).map(|i| member(i.from)).collect();
        if g.is_none() && invites.is_empty() {
            return None;
        }
        Some(GroupView { player: id, members: g.map(|g| g.members.iter().map(|&m| member(m)).collect()).unwrap_or_default(), markers: g.map(|g| g.markers.clone()).unwrap_or_default(), invites })
    }

    fn living(&self, id: u32) -> bool {
        self.player(id).is_some_and(|p| !p.dead && p.away.is_none())
    }

    /// Player `id` invites `target` (`None`: the nearest survivor it sees within `invite_m`) into
    /// its group (a new one, made when the other accepts, if it has none).
    pub fn group_invite(&mut self, id: u32, target: Option<u32>) -> Result<(), String> {
        let t = self.config.groups.clone();
        if !self.living(id) {
            return Err(format!("no living player {id}"));
        }
        // By the record of who sees whom, or by the rule itself (someone who has only just come).
        let sees = |w: &World, o: u32| w.awareness.sees(id, o) || w.sees_now(id, o);
        let me = self.player(id).map(|p| p.state.pos).unwrap_or_default();
        let target = target.or_else(|| {
            self.players
                .iter()
                .filter(|o| o.id != id && !o.dead && o.away.is_none() && o.state.pos.distance(me) <= t.invite_m && sees(self, o.id) && !self.groups.together(id, o.id))
                .min_by(|a, b| a.state.pos.distance(me).total_cmp(&b.state.pos.distance(me)).then(a.id.cmp(&b.id)))
                .map(|o| o.id)
        });
        let Some(target) = target.filter(|&o| o != id && self.living(o)) else { return self.refuse(id, "nobody near enough to invite".into()) };
        let there = self.player(target).map(|p| p.state.pos).unwrap_or_default();
        if there.distance(me) > t.invite_m || !sees(self, target) {
            return self.refuse(id, "they have to be near and in sight to be invited".into());
        }
        if self.groups.together(id, target) {
            return self.refuse(id, "they are in your group already".into());
        }
        if self.groups.of(target).is_some() {
            return self.refuse(id, "they are in another group".into());
        }
        if self.groups.of(id).is_some_and(|g| g.members.len() >= t.max_members) {
            return self.refuse(id, "your group is full".into());
        }
        let until = self.tick + (t.invite_secs * TICK_RATE as f32) as u64;
        match self.groups.invites.iter_mut().find(|i| i.from == id && i.to == target) {
            Some(i) => i.until = until,
            None => {
                self.groups.invites.push(Invite { from: id, to: target, until });
                let name = self.player_name(id);
                self.note(target, true, format!("{name} invites you into a group"));
            }
        }
        self.note(id, true, "Invited them into your group".into());
        Ok(())
    }

    /// Player `id` accepts the invitation from `from` (`None`: the newest one to it).
    pub fn group_accept(&mut self, id: u32, from: Option<u32>) -> Result<(), String> {
        if !self.living(id) {
            return Err(format!("no living player {id}"));
        }
        let Some(k) = self.groups.invites.iter().rposition(|i| i.to == id && from.is_none_or(|f| f == i.from)) else { return self.refuse(id, "nobody has invited you".into()) };
        if self.groups.of(id).is_some() {
            return self.refuse(id, "leave your group first".into());
        }
        let from = self.groups.invites.remove(k).from;
        let max = self.config.groups.max_members;
        let gi = match self.groups.index_of(from) {
            Some(gi) if self.groups.list[gi].members.len() >= max => return self.refuse(id, "that group is full".into()),
            Some(gi) => gi,
            None => {
                let gid = self.groups.next_group.max(1);
                self.groups.next_group = gid + 1;
                self.groups.list.push(Group { id: gid, members: vec![from], markers: Vec::new() });
                self.groups.list.len() - 1
            }
        };
        self.groups.list[gi].members.push(id);
        // Other invitations to it are answered by this.
        self.groups.invites.retain(|i| i.to != id);
        let name = self.player_name(id);
        for m in self.groups.list[gi].members.clone() {
            self.note(m, true, if m == id { "You joined the group".to_string() } else { format!("{name} joined your group") });
        }
        Ok(())
    }

    /// Player `id` leaves its group. One member left alone is no group: it goes, markers and all.
    pub fn group_leave(&mut self, id: u32) -> Result<(), String> {
        let Some(gi) = self.groups.index_of(id) else { return self.refuse(id, "you aren't in a group".into()) };
        let name = self.player_name(id);
        self.groups.list[gi].members.retain(|&m| m != id);
        self.groups.invites.retain(|i| i.from != id);
        self.note(id, true, "You left the group".into());
        let rest = self.groups.list[gi].members.clone();
        if rest.len() < 2 {
            self.groups.list.remove(gi);
        }
        for m in rest {
            self.note(m, true, format!("{name} left your group"));
        }
        Ok(())
    }

    /// Player `id` puts a marker its group shares at `pos`. Returns the marker's id.
    pub fn group_mark(&mut self, id: u32, pos: Vec2) -> Result<u32, String> {
        let max = self.config.groups.max_markers.max(1);
        let Some(gi) = self.groups.index_of(id) else { return self.refuse(id, "markers are shared with a group: you aren't in one".into()) };
        if !pos.is_finite() || !self.map.in_bounds(pos.floor().as_ivec2()) {
            return self.refuse(id, "that is off the map".into());
        }
        let mid = self.groups.next_marker.max(1);
        self.groups.next_marker = mid + 1;
        let g = &mut self.groups.list[gi];
        g.markers.push(Marker { id: mid, pos, by: id });
        let over = g.markers.len().saturating_sub(max);
        g.markers.drain(..over);
        Ok(mid)
    }

    /// Player `id` takes its group's marker `marker` away (`None`: the one nearest `near`, within
    /// 3 m of it). Returns whether one went.
    pub fn group_unmark(&mut self, id: u32, marker: Option<u32>, near: Vec2) -> bool {
        let Some(gi) = self.groups.index_of(id) else { return false };
        let g = &mut self.groups.list[gi];
        let k = match marker {
            Some(m) => g.markers.iter().position(|x| x.id == m),
            None => g.markers.iter().enumerate().filter(|(_, m)| m.pos.distance(near) <= 3.0).min_by(|a, b| a.1.pos.distance(near).total_cmp(&b.1.pos.distance(near))).map(|(k, _)| k),
        };
        k.map(|k| g.markers.remove(k)).is_some()
    }

    /// Invitations lapse.
    pub(crate) fn step_groups(&mut self) {
        if !self.groups.invites.is_empty() {
            let tick = self.tick;
            self.groups.invites.retain(|i| i.until > tick);
        }
    }
}

#[cfg(test)]
mod tests;
