//! Logging out and spawn protection on a server (#68, docs/PLAN.md §4.8).
//!
//! **Logging out.** A survivor whose player leaves ([`World::player_logout`]) doesn't vanish: it
//! lies where it was left, asleep and defenceless ([`Player::away`]), for a time the server sets,
//! so leaving can't end a fight. Zombies still see, hear and strike it, other players can hit,
//! kill and loot it. Somewhere safe ([`World::logout_is_safe`]: no zombie near or after it, no
//! other player about) the time is the server's shorter one. When it runs out the survivor leaves
//! the world whole ([`World::parked`]: wounds, needs, skills and everything it carries), and
//! [`World::player_return`] puts it back where it lay when its player comes back; one still lying
//! there just gets up. A survivor killed while its player was away stays a dead player, as any
//! other.
//!
//! **Spawn protection.** [`World::protect_player`] gives a survivor some seconds
//! ([`Player::protected`]) in which other players' swings, shoves and stomps pass it by
//! (`combat::land` leaves it out of the targets). Zombies are not held off. Starting a swing,
//! shove or stomp of its own ends it at once (`combat::begin_action`).
//!
//! Single-player uses none of this: nothing here happens unless a server asks for it.

use glam::Vec2;
use serde::Deserialize;

use crate::TICK_DT;
use crate::items::ItemDef;
use crate::players::{PLAYER_RADIUS, Player, PlayerInput};
use crate::world::World;
use crate::zombies::ZombieState;

/// Logging out is safe with no living zombie this near (metres)...
pub const SAFE_FROM_ZOMBIES: f32 = 25.0;
/// ...and no other living player this near.
pub const SAFE_FROM_PLAYERS: f32 = 40.0;
/// The longest a server may keep a logged-out survivor in the world, or protect a new one (seconds).
pub const MAX_LOGOUT_SECS: f32 = 3600.0;

/// What a new survivor on a server starts with (`assets/data/start_kit.ron`): item ids and counts.
pub type StartKit = Vec<(String, u16)>;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct KitFile {
    items: Vec<KitEntry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct KitEntry {
    item: String,
    #[serde(default = "one")]
    count: u16,
}

fn one() -> u16 {
    1
}

/// Read a start kit, `(items: [(item: "water_bottle", count: 1), ...])`, against the items it
/// names: every id must be an item, counts 1 to 20, nothing listed twice, at most 12 entries (a
/// little, not a loadout). Errors name the entry.
pub fn parse_start_kit(text: &str, items: &[ItemDef]) -> Result<StartKit, String> {
    let file: KitFile = ron::from_str(text).map_err(|e| e.to_string())?;
    if file.items.len() > 12 {
        return Err(format!("{} entries: a start kit has at most 12", file.items.len()));
    }
    let mut kit = StartKit::new();
    for e in file.items {
        if !items.iter().any(|i| i.id == e.item) {
            return Err(format!("{}: no such item", e.item));
        }
        if !(1..=20).contains(&e.count) {
            return Err(format!("{}: count {} (1 to 20)", e.item, e.count));
        }
        if kit.iter().any(|(id, _)| *id == e.item) {
            return Err(format!("{}: listed twice", e.item));
        }
        kit.push((e.item, e.count));
    }
    Ok(kit)
}

impl World {
    /// Put a start kit into player `id`'s pockets (a server's new survivors; ids the world's items
    /// don't have are skipped). Returns how many kinds it got.
    pub fn give_kit(&mut self, id: u32, kit: &[(String, u16)]) -> usize {
        kit.iter().filter(|(item, count)| self.give_item(id, item, *count)).count()
    }

    /// Is player `id` somewhere safe to leave from: nothing after it, no living zombie within
    /// [`SAFE_FROM_ZOMBIES`], no other living player (one who is playing) within
    /// [`SAFE_FROM_PLAYERS`], and not reeling from a blow?
    pub fn logout_is_safe(&self, id: u32) -> bool {
        let Some(p) = self.player(id) else { return false };
        if p.knock.is_some() {
            return false;
        }
        let at = p.state.pos;
        let z = &self.zombies;
        let zombies = (0..z.len()).any(|i| z.state[i] != ZombieState::Dead && (z.pos[i].distance(at) < SAFE_FROM_ZOMBIES || (z.target_player[i] == id && z.state[i].is_after(id))));
        let players = self.players.iter().any(|o| o.id != id && !o.dead && o.away.is_none() && o.state.pos.distance(at) < SAFE_FROM_PLAYERS);
        !zombies && !players
    }

    /// Player `id`'s player left the server: the survivor stays in the world, lying asleep, for
    /// `secs` (`safe_secs` if [`logout_is_safe`](Self::logout_is_safe)), then leaves it. Returns
    /// the seconds it stays, or `None` if there is no such living player in the world (a dead one
    /// stays as it is) or it was logged out already.
    pub fn player_logout(&mut self, id: u32, secs: f32, safe_secs: f32) -> Option<f32> {
        let pi = self.players.iter().position(|p| p.id == id && !p.dead && p.away.is_none())?;
        let clamp = |s: f32| if s.is_finite() { s.clamp(0.0, MAX_LOGOUT_SECS) } else { 0.0 };
        let left = if self.logout_is_safe(id) { clamp(safe_secs).min(clamp(secs)) } else { clamp(secs) };
        // It stops what it was doing and stands (then lies) still.
        self.cancel_jobs(pi);
        let p = &mut self.players[pi];
        p.input = PlayerInput { facing: p.state.facing, ..PlayerInput::default() };
        p.last_input_seq = p.last_input_seq.wrapping_add(1);
        p.queued = None;
        p.open = None;
        p.protected = 0.0;
        p.away = Some(left);
        self.private_dirty.insert(id);
        Some(left)
    }

    /// Player `id`'s player is back. A survivor lying logged out gets up; one that had left the
    /// world is put back where it lay (pushed out of anything that now stands there). Returns
    /// whether it is in the world now (a dead one too: its player sees how it ended).
    pub fn player_return(&mut self, id: u32) -> bool {
        if let Some(p) = self.players.iter_mut().find(|p| p.id == id) {
            p.away = None;
            self.private_dirty.insert(id);
            return true;
        }
        let Some(k) = self.parked.iter().position(|p| p.id == id) else { return false };
        let mut p = self.parked.remove(k);
        p.state.pos = self.map.resolve_circle(p.state.pos, PLAYER_RADIUS);
        let pi = self.players.partition_point(|o| o.id < id);
        self.players.insert(pi, p);
        self.refresh_carry(pi);
        self.private_dirty.insert(id);
        true
    }

    /// Give player `id` `secs` of spawn protection from other players (0 ends it).
    pub fn protect_player(&mut self, id: u32, secs: f32) -> bool {
        let Some(p) = self.players.iter_mut().find(|p| p.id == id && !p.dead) else { return false };
        p.protected = if secs.is_finite() { secs.clamp(0.0, MAX_LOGOUT_SECS) } else { 0.0 };
        true
    }

    /// The survivors out of the world (logged out, their time in it run out), in id order.
    pub fn parked(&self) -> &[Player] {
        &self.parked
    }

    /// Player `id`'s survivor, in the world or out of it.
    pub fn survivor(&self, id: u32) -> Option<&Player> {
        self.player(id).or_else(|| self.parked.iter().find(|p| p.id == id))
    }

    /// Where survivor `id` is or was left.
    pub fn survivor_pos(&self, id: u32) -> Option<Vec2> {
        self.survivor(id).map(|p| p.state.pos)
    }

    /// Once a tick, last among what touches players: spawn protection runs down, and logged-out
    /// survivors whose time is up leave the world (their ids go to `left_world`). One that died
    /// lying there is a dead player like any other.
    pub(crate) fn step_logout(&mut self) {
        let mut pi = 0;
        while pi < self.players.len() {
            let p = &mut self.players[pi];
            p.protected = if p.dead { 0.0 } else { (p.protected - TICK_DT).max(0.0) };
            match p.away {
                Some(_) if p.dead => p.away = None,
                Some(left) if left - TICK_DT <= 0.0 => {
                    self.park(pi);
                    continue;
                }
                Some(left) => p.away = Some(left - TICK_DT),
                None => {}
            }
            pi += 1;
        }
    }

    /// Take player `pi` out of the world, whole.
    fn park(&mut self, pi: usize) {
        let mut p = self.players.remove(pi);
        let id = p.id;
        (p.away, p.protected, p.action, p.queued, p.knock, p.open) = (None, 0.0, None, None, None, None);
        p.jobs.clear();
        self.private_dirty.remove(&id);
        self.skill_mods_due.retain(|&d| d != id);
        self.left_world.push(id);
        let k = self.parked.partition_point(|o| o.id < id);
        self.parked.insert(k, p);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::template::BuildingTemplate;
    use crate::world::WorldConfig;

    fn world(pvp: bool) -> World {
        let templates = [include_str!("../../../assets/data/buildings/bungalow.ron")].iter().map(|t| BuildingTemplate::from_ron(t).unwrap()).collect();
        let mut config = WorldConfig::new(7, 0, templates).with_pvp(pvp);
        config.needs = crate::needs::NeedsTuning::off();
        World::new(config)
    }

    /// Open ground west of town, where nothing is in the way.
    fn field(w: &World) -> Vec2 {
        Vec2::new(w.town_min.x as f32 - 12.0, (w.town_min.y + w.town_max.y) as f32 * 0.5)
    }

    fn secs(w: &mut World, s: f32) {
        for _ in 0..(s / TICK_DT).round() as u32 {
            w.step();
        }
    }

    #[test]
    fn a_logged_out_survivor_stays_then_leaves_and_comes_back_whole() {
        let mut w = world(true);
        let at = field(&w);
        let a = w.add_player(at);
        let b = w.add_player(at + Vec2::new(200.0, 0.0));
        assert!(w.give_item(a, "water_bottle", 1) || w.config.items.is_empty());
        w.step();
        let before = w.player(a).unwrap().clone();
        assert_eq!(w.player_logout(a, 10.0, 2.0), Some(2.0), "nobody about: the safe time");
        assert_eq!(w.player_logout(a, 10.0, 2.0), None, "already logged out");
        // It takes no input while it lies there.
        assert!(!w.set_player_input(a, 1000, PlayerInput { move_dir: Vec2::X, ..Default::default() }));
        secs(&mut w, 1.5);
        let lying = w.player(a).expect("still in the world");
        assert!(lying.away.is_some() && lying.state.pos == before.state.pos);
        secs(&mut w, 1.0);
        assert!(w.player(a).is_none() && w.parked().len() == 1, "gone after its time");
        assert!(w.left_world.is_empty(), "only the tick it left");
        assert!(w.player(b).is_some());
        let hash_away = w.state_hash();
        // A save keeps it, in the world or out of it.
        let content = w.config.clone();
        let loaded = World::load(&w.save(), &content).unwrap();
        assert_eq!(loaded.state_hash(), hash_away);
        assert_eq!(loaded.parked(), w.parked());
        assert_eq!(loaded.players.len(), 1);
        // Back: the same survivor where it lay, with what it had.
        assert!(w.player_return(a));
        let back = w.player(a).unwrap();
        assert_eq!((back.state.pos, back.hp, &back.inventory, back.skills), (before.state.pos, before.hp, &before.inventory, before.skills));
        assert!(back.away.is_none() && w.parked().is_empty());
        assert_eq!(w.players.iter().map(|p| p.id).collect::<Vec<_>>(), vec![a, b], "id order kept");
        assert!(w.set_player_input(a, 2000, PlayerInput { move_dir: Vec2::X, ..Default::default() }));
        w.step();
        assert!(w.player(a).unwrap().state.pos.x > before.state.pos.x, "it moves again");
        assert!(!w.player_return(99));
    }

    #[test]
    fn the_start_kit_is_checked_against_the_items() {
        let weapons = crate::items::parse_weapons(include_str!("../../../assets/data/items/weapons.ron")).unwrap();
        let items = crate::items::parse_items(include_str!("../../../assets/data/items/items.ron"), &weapons).unwrap();
        let kit = parse_start_kit(include_str!("../../../assets/data/start_kit.ron"), &items).expect("the shipped kit");
        assert!(!kit.is_empty() && kit.len() <= 6, "a little: {kit:?}");
        // It fits a new survivor's pockets and doesn't burden it.
        let (litres, kg) = kit.iter().fold((0.0, 0.0), |(l, k), (id, n)| {
            let def = items.iter().find(|i| i.id == *id).unwrap();
            (l + def.volume_l * *n as f32, k + def.weight_kg * *n as f32)
        });
        assert!(litres <= 3.0 && kg < 5.0, "{litres} L, {kg} kg");
        for (bad, why) in [
            ("(items: [(item: \"unobtainium\")])", "no such item"),
            ("(items: [(item: \"rag\", count: 0)])", "count 0"),
            ("(items: [(item: \"rag\"), (item: \"rag\")])", "listed twice"),
            ("(items: [], loadout: 3)", "loadout"),
        ] {
            let e = parse_start_kit(bad, &items).unwrap_err();
            assert!(e.contains(why), "{bad}: {e}");
        }
        assert_eq!(parse_start_kit("(items: [(item: \"rag\")])", &items).unwrap(), vec![("rag".to_string(), 1)]);
    }

    #[test]
    fn coming_back_early_gets_the_survivor_up() {
        let mut w = world(true);
        let a = w.add_player(field(&w));
        w.player_logout(a, 30.0, 30.0);
        secs(&mut w, 5.0);
        assert!(w.player(a).unwrap().away.is_some());
        // A save keeps the seconds it has left, and the loaded world goes on the same.
        let mut loaded = World::load(&w.save(), &w.config.clone()).unwrap();
        assert_eq!(loaded.player(a).unwrap().away, w.player(a).unwrap().away);
        assert_eq!(loaded.state_hash(), w.state_hash());
        let mut twin = world(true);
        let b = twin.add_player(field(&twin));
        twin.player_logout(b, 30.0, 30.0);
        secs(&mut twin, 5.0);
        assert_eq!(twin.state_hash(), w.state_hash());
        secs(&mut loaded, 26.0);
        secs(&mut twin, 26.0);
        assert_eq!(loaded.state_hash(), twin.state_hash());
        assert!(loaded.player(a).is_none() && loaded.parked().len() == 1);
        assert!(w.player_return(a));
        secs(&mut w, 40.0);
        assert!(w.player(a).is_some_and(|p| p.away.is_none()) && w.parked().is_empty());
    }

    #[test]
    fn leaving_a_fight_takes_the_long_time_and_the_body_can_be_killed() {
        let mut w = world(true);
        let at = field(&w);
        let a = w.add_player(at);
        let b = w.add_player(at + Vec2::new(0.9, 0.0));
        w.step();
        assert!(!w.logout_is_safe(a), "another player is right there");
        assert_eq!(w.player_logout(a, 60.0, 5.0), Some(60.0));
        w.give_item(a, "water_bottle", 2);
        let carried = w.player(a).unwrap().inventory.carried.clone();
        // B beats it to death where it lies; nothing protects it.
        let mut seq = 0;
        for _ in 0..20 * 120 {
            if w.player(a).is_none_or(|p| p.dead) {
                break;
            }
            seq += 1;
            // Blows push the body about: B stays on it.
            let to = w.player(a).unwrap().state.pos - w.player(b).unwrap().state.pos;
            let move_dir = if to.length() > 0.8 { to.normalize() } else { Vec2::ZERO };
            w.set_player_input(b, seq, PlayerInput { move_dir, facing: to.to_angle(), aim: true, ..Default::default() });
            // Swings pass over a body on the ground; stamp on it then.
            if w.player(a).unwrap().knock.is_some_and(|k| k.down) {
                w.player_stomp(b);
            } else {
                w.player_attack(b);
            }
            w.step();
        }
        let dead = w.player(a).expect("killed before its time ran out: the body stays");
        assert!(dead.dead, "hp {}", dead.hp);
        w.step();
        assert!(w.player(a).unwrap().away.is_none() && w.parked().is_empty());
        // What it carried lies where it fell, for the taking.
        if !carried.is_empty() {
            assert!(w.items().iter().any(|it| it.pos.distance(at) < 3.0), "its things are on the floor");
        }
        // Its player comes back to a dead survivor, and no time brings it back to life.
        assert!(w.player_return(a));
        secs(&mut w, 70.0);
        assert!(w.player(a).unwrap().dead);
    }

    #[test]
    fn zombies_near_make_logging_out_unsafe_and_they_get_the_sleeper() {
        let mut w = world(false);
        let at = field(&w);
        let a = w.add_player(at);
        w.step();
        assert!(w.logout_is_safe(a));
        w.spawn_zombies(4, at + Vec2::new(6.0, 0.0), 2.0);
        w.step();
        assert!(!w.logout_is_safe(a));
        assert_eq!(w.player_logout(a, 120.0, 5.0), Some(120.0));
        let hp = w.player(a).unwrap().hp;
        secs(&mut w, 60.0);
        assert!(w.player(a).is_some_and(|p| p.hp < hp), "defenceless: the zombies got to it");
    }

    #[test]
    fn spawn_protection_holds_off_players_until_it_ends_or_you_attack() {
        let mut w = world(true);
        let at = field(&w);
        let a = w.add_player(at);
        let b = w.add_player(at + Vec2::new(0.9, 0.0));
        w.step();
        // `who` swings at the other from 0.7 m, both stood up where they began.
        let swing = |w: &mut World, who: u32, facing: f32, seq: &mut u32| {
            for (k, p) in w.players.iter_mut().enumerate() {
                (p.state.pos, p.knock, p.ready_in) = (at + Vec2::new(0.7 * k as f32, 0.0), None, 0.0);
            }
            *seq += 1;
            assert!(w.set_player_input(who, *seq, PlayerInput { facing, aim: true, ..Default::default() }));
            assert!(w.player_attack(who));
            let mut hits = 0;
            for _ in 0..40 {
                w.step();
                hits += w.player_hit_events.len();
            }
            hits
        };
        let (mut seq, mut aseq) = (0, 0);
        let (east, west) = (0.0, std::f32::consts::PI);
        assert_eq!(swing(&mut w, b, west, &mut seq), 1, "unprotected, the blow lands");
        let hp = w.player(a).unwrap().hp;
        assert!(w.protect_player(a, 30.0));
        let hash = w.state_hash();
        assert_eq!(swing(&mut w, b, west, &mut seq), 0, "the blow passes a protected survivor by");
        assert!(w.player(a).unwrap().hp >= hp && w.player(a).unwrap().knock.is_none());
        assert_ne!(hash, w.state_hash());
        // A save keeps the seconds left.
        let loaded = World::load(&w.save(), &w.config.clone()).unwrap();
        assert_eq!(loaded.player(a).unwrap().protected, w.player(a).unwrap().protected);
        assert_eq!(loaded.state_hash(), w.state_hash());
        // A attacks: its protection is over at once, and B's next blow lands.
        assert_eq!(swing(&mut w, a, east, &mut aseq), 1, "a protected survivor can hit");
        assert_eq!(w.player(a).unwrap().protected, 0.0);
        assert_eq!(swing(&mut w, b, west, &mut seq), 1);
        // And it runs out by itself.
        w.protect_player(b, 1.0);
        secs(&mut w, 1.1);
        assert_eq!(w.player(b).unwrap().protected, 0.0);
    }

    #[test]
    fn a_world_nobody_logs_out_of_hashes_and_saves_as_before() {
        let mut a = world(true);
        let mut b = world(true);
        let at = field(&a);
        for w in [&mut a, &mut b] {
            w.add_player(at);
            w.add_player(at + Vec2::new(30.0, 0.0));
        }
        // Protection given and gone, a logout taken back: nothing of it is left.
        b.protect_player(1, 0.2);
        secs(&mut a, 1.0);
        secs(&mut b, 1.0);
        assert_eq!(a.state_hash(), b.state_hash());
        assert_eq!(a.save(), b.save());
    }
}
