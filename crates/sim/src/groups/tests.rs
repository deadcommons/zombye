//! Groups (#73): inviting, accepting, leaving, shared markers, names, that damage stays on, saves.

use super::*;
use crate::map::{CHUNK_SIZE, Edge, Side, TileMap};
use crate::template::BuildingTemplate;
use crate::world::WorldConfig;
use glam::IVec2;

/// An empty field at noon with players at the given places.
fn field(at: &[Vec2]) -> (World, Vec<u32>) {
    let templates = vec![BuildingTemplate::from_ron(include_str!("../../../../assets/data/buildings/bungalow.ron")).unwrap()];
    let weapons = crate::items::parse_weapons(include_str!("../../../../assets/data/items/weapons.ron")).unwrap();
    let mut c = WorldConfig::new(7, 0, templates).with_weapons(weapons).with_map_chunks(10).with_pvp(true);
    c.start_hour = 12.0;
    c.needs = crate::needs::NeedsTuning::off();
    let mut w = World::new(c);
    w.map = TileMap::new(w.map.width() / CHUNK_SIZE, w.map.height() / CHUNK_SIZE);
    w.map_changed();
    w.items.clear();
    w.sites.clear();
    w.spawn_building = 0;
    let ids = at.iter().map(|&p| w.add_player(p)).collect();
    (w, ids)
}

fn p(x: f32, y: f32) -> Vec2 {
    Vec2::new(60.5 + x, 60.5 + y)
}

fn names(v: &GroupView) -> Vec<&str> {
    v.members.iter().map(|m| m.name.as_str()).collect()
}

#[test]
fn invite_accept_names_and_leaving() {
    // b 4 m in front of a (who faces east), c beside them, d 40 m off.
    let (mut w, ids) = field(&[p(0.0, 0.0), p(4.0, 0.0), p(3.0, 2.0), p(40.0, 0.0)]);
    let (a, b, c, d) = (ids[0], ids[1], ids[2], ids[3]);
    w.set_player_name(a, "ada");
    w.set_player_name(b, "bob");
    assert!(w.group_view(a).is_none() && w.groups().is_empty());
    assert!(w.group_accept(b, None).is_err(), "nobody asked");
    assert!(w.group_invite(a, Some(d)).is_err(), "40 m off is too far");
    assert!(w.group_invite(a, Some(a)).is_err());
    // The nearest survivor in sight.
    w.group_invite(a, None).unwrap();
    let v = w.group_view(c).expect("an invitation is something to tell");
    assert!(v.members.is_empty() && v.invites.len() == 1 && v.invites[0].name == "ada");
    assert!(w.group_view(a).is_none(), "no group until someone accepts");
    w.group_invite(a, Some(b)).unwrap();
    w.group_accept(b, Some(a)).unwrap();
    let v = w.group_view(b).unwrap();
    assert_eq!(names(&v), ["ada", "bob"]);
    assert!(v.invites.is_empty());
    assert_eq!(v.name_of(a), Some("ada"));
    assert!(w.groups().together(a, b) && !w.groups().together(a, c));
    // A member invites too; one in a group can't be asked into another or accept a second.
    w.group_accept(c, None).unwrap();
    assert_eq!(names(&w.group_view(a).unwrap()), ["ada", "bob", "Survivor 3"]);
    assert!(w.group_invite(c, Some(b)).unwrap_err().contains("already"));
    assert!(w.item_notes.iter().any(|n| n.player == a && n.text == "Survivor 3 joined your group"));
    // Leaving: two stay a group, one alone is none.
    w.group_leave(a).unwrap();
    assert!(w.group_view(a).is_none());
    assert_eq!(names(&w.group_view(b).unwrap()), ["bob", "Survivor 3"]);
    w.group_leave(c).unwrap();
    assert!(w.group_view(b).is_none() && w.groups().list.is_empty());
    assert!(w.group_leave(b).is_err());
}

#[test]
fn invitations_need_sight_and_lapse() {
    let (mut w, ids) = field(&[p(0.0, 0.0), p(4.0, 0.0)]);
    let (a, b) = (ids[0], ids[1]);
    for y in 0..w.map.height() {
        w.map.set_edge(IVec2::new(62, y), Side::W, Edge::Wall);
    }
    w.map_changed();
    assert!(w.group_invite(a, Some(b)).unwrap_err().contains("in sight"));
    assert!(w.group_invite(a, None).is_err());
    for y in 0..w.map.height() {
        w.map.set_edge(IVec2::new(62, y), Side::W, Edge::None);
    }
    w.map_changed();
    w.group_invite(a, Some(b)).unwrap();
    let ticks = (w.config.groups.invite_secs * TICK_RATE as f32) as u32;
    for _ in 0..ticks - 1 {
        w.step();
    }
    assert!(w.group_view(b).is_some());
    w.step();
    w.step();
    assert!(w.group_view(b).is_none() && w.group_accept(b, None).is_err(), "it lapsed");
    // A full group takes no more.
    w.config.groups.max_members = 2;
    w.group_invite(a, Some(b)).unwrap();
    w.group_accept(b, None).unwrap();
    let c = w.add_player(p(1.0, 1.0));
    assert!(w.group_invite(a, Some(c)).unwrap_err().contains("full"));
}

#[test]
fn markers_are_shared_and_capped() {
    let (mut w, ids) = field(&[p(0.0, 0.0), p(3.0, 0.0), p(0.0, 3.0)]);
    let (a, b, c) = (ids[0], ids[1], ids[2]);
    assert!(w.group_mark(a, p(10.0, 10.0)).is_err(), "no group, nobody to share with");
    w.group_invite(a, Some(b)).unwrap();
    w.group_accept(b, None).unwrap();
    let m = w.group_mark(a, p(10.0, 10.0)).unwrap();
    assert!(w.group_mark(a, Vec2::new(-5.0, f32::NAN)).is_err() && w.group_mark(a, Vec2::new(-5.0, 3.0)).is_err());
    let v = w.group_view(b).unwrap();
    assert_eq!(v.markers, [Marker { id: m, pos: p(10.0, 10.0), by: a }]);
    assert!(w.group_view(c).is_none(), "not the outsider's business");
    // The other member takes it away by pointing near it.
    assert!(!w.group_unmark(c, Some(m), Vec2::ZERO));
    assert!(!w.group_unmark(b, None, p(20.0, 20.0)));
    assert!(w.group_unmark(b, None, p(11.0, 10.5)));
    assert!(w.group_view(a).unwrap().markers.is_empty());
    for k in 0..12 {
        w.group_mark(b, p(k as f32, 5.0)).unwrap();
    }
    let v = w.group_view(a).unwrap();
    assert_eq!(v.markers.len(), w.config.groups.max_markers);
    assert_eq!(v.markers[0].pos, p(4.0, 5.0), "the oldest went first");
}

/// "Damage between members stays on": a group changes nothing in a fight.
#[test]
fn members_still_hurt_each_other() {
    let (mut w, ids) = field(&[p(0.0, 0.0), p(0.7, 0.0)]);
    let (a, b) = (ids[0], ids[1]);
    w.group_invite(a, Some(b)).unwrap();
    w.group_accept(b, None).unwrap();
    let hp = w.player(b).unwrap().hp;
    for _ in 0..10 {
        w.set_aim(a, true);
        w.player_attack(a);
        for _ in 0..30 {
            w.step();
        }
    }
    assert!(w.player(b).unwrap().hp < hp, "a punch lands on a group member as on anyone");
    assert!(w.groups().together(a, b));
}

#[test]
fn groups_are_saved_and_hashed_only_when_there_are_any() {
    let (mut w, ids) = field(&[p(0.0, 0.0), p(3.0, 0.0), p(6.0, 1.0)]);
    let (a, b, c) = (ids[0], ids[1], ids[2]);
    let (plain_hash, plain_len) = (w.state_hash(), w.save().len());
    w.group_invite(a, Some(b)).unwrap();
    w.group_accept(b, None).unwrap();
    w.group_invite(b, Some(c)).unwrap();
    w.group_mark(b, p(7.0, 7.0)).unwrap();
    assert_ne!(w.state_hash(), plain_hash);
    let bytes = w.save();
    assert!(bytes.len() > plain_len);
    let mut l = World::load(&bytes, &w.config).unwrap();
    assert_eq!(l.state_hash(), w.state_hash());
    assert_eq!(l.groups(), w.groups());
    l.group_accept(c, None).unwrap();
    assert_eq!(l.group_view(c).unwrap().members.len(), 3);
    // A group naming a player who isn't there doesn't load.
    w.groups.list[0].members.push(99);
    assert!(matches!(World::load(&w.save(), &w.config), Err(crate::save::LoadError::Invalid(_))));
}
