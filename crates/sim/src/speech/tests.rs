//! Who hears what is said (#67): across a room, through a wall, out of range; the noise it makes
//! for zombies; gestures.

use super::*;
use crate::items;
use crate::map::{CHUNK_SIZE, Edge, Side, TileMap};
use crate::template::BuildingTemplate;
use crate::world::WorldConfig;
use crate::zombies::ZombieState;
use glam::IVec2;

/// An empty field at midnight with a speaker at (60.5, 60.5) and listeners at `others`.
fn field(others: &[Vec2]) -> (World, u32, Vec<u32>) {
    let templates = vec![BuildingTemplate::from_ron(include_str!("../../../../assets/data/buildings/bungalow.ron")).unwrap()];
    let weapons = items::parse_weapons(include_str!("../../../../assets/data/items/weapons.ron")).unwrap();
    let mut c = WorldConfig::new(7, 0, templates).with_weapons(weapons).with_map_chunks(10);
    c.start_hour = 0.0;
    c.needs = crate::needs::NeedsTuning::off();
    let mut w = World::new(c);
    w.map = TileMap::new(w.map.width() / CHUNK_SIZE, w.map.height() / CHUNK_SIZE);
    w.map_changed();
    w.items.clear();
    w.sites.clear();
    let a = w.add_player(me());
    let ids = others.iter().map(|&at| w.add_player(at)).collect();
    (w, a, ids)
}

fn me() -> Vec2 {
    Vec2::new(60.5, 60.5)
}

/// A wall along the west side of column `x`, the whole height of the field.
fn wall(w: &mut World, x: i32) {
    for y in 0..w.map.height() {
        w.map.set_edge(IVec2::new(x, y), Side::W, Edge::Wall);
    }
    w.map_changed();
}

fn heard_by(lines: &[Said], id: u32) -> Option<&Said> {
    lines.iter().find(|s| s.listener == id)
}

/// The issue's "done when": two players talk across a room and through a wall, and a third out
/// of range hears nothing.
#[test]
fn talk_is_heard_across_a_room_and_through_a_wall_but_not_out_of_range() {
    // b across the room (8 m, in the open), c through a wall (3 m), d 20 m off.
    let (mut w, a, ids) = field(&[me() + Vec2::new(0.0, 8.0), me() + Vec2::new(3.0, 0.0), me() + Vec2::new(0.0, -20.0)]);
    let (b, c, d) = (ids[0], ids[1], ids[2]);
    wall(&mut w, 62);
    assert_eq!(w.player_say(a, "Anyone there?", Volume::Talk), Some(2));
    let lines = w.take_speech();
    let own = heard_by(&lines, a).expect("the speaker's own line");
    assert!(own.speaker == a && !own.muffled && own.text == "Anyone there?" && own.volume == Volume::Talk);
    let across = heard_by(&lines, b).expect("heard across the room");
    assert!(!across.muffled && across.speaker == a && across.pos == me() && across.text == "Anyone there?");
    let through = heard_by(&lines, c).expect("heard through the wall");
    assert!(through.muffled, "through a wall it is muffled");
    assert!(own.seen && !through.seen, "nobody sees through a wall");
    assert!(heard_by(&lines, d).is_none(), "20 m off, talk doesn't carry");
    assert_eq!(lines.len(), 3);
    assert!(w.take_speech().is_empty(), "taken once");

    // And back: the one behind the wall answers, and is heard muffled; the far one still hears nothing.
    w.step();
    assert_eq!(w.player_say(c, "Here. Behind the wall.", Volume::Talk), Some(1));
    let lines = w.take_speech();
    assert!(heard_by(&lines, a).is_some_and(|s| s.muffled && s.speaker == c));
    assert!(heard_by(&lines, b).is_none(), "8.5 m and a wall away: talk only gets 4.8 m through one");
    assert!(heard_by(&lines, d).is_none());
}

#[test]
fn volume_sets_how_far_a_voice_carries() {
    // 2 m, 8 m and 30 m off in the open; 10 m off behind a wall.
    let (mut w, a, ids) = field(&[me() + Vec2::new(0.0, 2.0), me() + Vec2::new(0.0, 8.0), me() + Vec2::new(0.0, 30.0), me() + Vec2::new(-10.0, 0.0)]);
    wall(&mut w, 55);
    let who =|w: &mut World, volume| {
        w.step();
        w.player_say(a, "hey", volume).unwrap();
        let lines = w.take_speech();
        ids.iter().map(|&id| heard_by(&lines, id).map(|s| s.muffled)).collect::<Vec<_>>()
    };
    assert_eq!(who(&mut w, Volume::Whisper), [Some(false), None, None, None]);
    assert_eq!(who(&mut w, Volume::Talk), [Some(false), Some(false), None, None]);
    assert_eq!(who(&mut w, Volume::Shout), [Some(false), Some(false), Some(false), Some(true)], "a shout gets 16 m through a wall");
    // The rule itself: the whole reach with a line of sight, `muffle` of it without.
    let t = &w.config.speech;
    let muffle = w.config.zombie.wall_muffle;
    assert_eq!(hears(&w.map, muffle, me(), me() + Vec2::new(0.0, t.talk_m - 0.1), t.talk_m), Some(false));
    assert_eq!(hears(&w.map, muffle, me(), me() + Vec2::new(0.0, t.talk_m + 0.1), t.talk_m), None);
    assert_eq!(hears(&w.map, muffle, me(), me() + Vec2::new(-(t.talk_m * muffle + 1.5), 0.0), t.talk_m), None, "through the wall, past the muffled reach");
}

#[test]
fn the_dead_the_sleeping_and_the_gone_neither_speak_nor_hear() {
    let (mut w, a, ids) = field(&[me() + Vec2::new(0.0, 2.0), me() + Vec2::new(2.0, 0.0), me() + Vec2::new(-2.0, 0.0)]);
    let (dead, asleep, away) = (ids[0], ids[1], ids[2]);
    w.players.iter_mut().find(|p| p.id == dead).unwrap().dead = true;
    w.players.iter_mut().find(|p| p.id == asleep).unwrap().needs.sleep = Some(crate::needs::Sleep { spot: crate::needs::SleepSpot::Floor, hours: 0.0 });
    w.players.iter_mut().find(|p| p.id == away).unwrap().away = Some(10.0);
    assert_eq!(w.player_say(a, "hello?", Volume::Talk), Some(0));
    assert_eq!(w.take_speech().len(), 1, "only the speaker's own line");
    for id in ids {
        assert_eq!(w.player_say(id, "hello", Volume::Talk), None);
    }
    assert_eq!(w.player_say(99, "hello", Volume::Talk), None, "no such player");
    assert!(w.take_speech().is_empty());
}

#[test]
fn text_is_cleaned_and_one_line_a_tick() {
    assert_eq!(clean_text("  hello\n\tthere  \u{7}friend  ", 120).as_deref(), Some("hello there friend"));
    assert_eq!(clean_text(" \n\t ", 120), None);
    assert_eq!(clean_text("", 120), None);
    assert_eq!(clean_text("héllo wörld", 7).as_deref(), Some("héllo w"));
    assert_eq!(clean_text("abc   ", 4).as_deref(), Some("abc"));

    let (mut w, a, ids) = field(&[me() + Vec2::new(0.0, 2.0)]);
    let long = "x".repeat(500);
    assert_eq!(w.player_say(a, &long, Volume::Talk), Some(1));
    assert_eq!(w.player_say(a, "again, the same tick", Volume::Talk), None);
    assert_eq!(w.player_say(a, "   ", Volume::Talk), None);
    assert_eq!(w.player_say(ids[0], "the other may answer at once", Volume::Talk), Some(1));
    let lines = w.take_speech();
    assert_eq!(lines.len(), 4);
    assert!(lines.iter().filter(|s| s.speaker == a).all(|s| s.text.chars().count() == w.config.speech.max_chars));
    w.step();
    assert_eq!(w.player_say(a, "the next tick", Volume::Talk), Some(1));
    // Nobody taking them: the queue stays bounded.
    for _ in 0..400 {
        w.step();
        w.player_say(a, "spam", Volume::Whisper);
    }
    assert!(w.speech.len() <= MAX_SPEECH);
    assert!(w.speech.last().is_some_and(|s| s.text == "spam"));
}

/// A zombie 22 m off with its back to the speaker, in the dark.
fn zombie_hears(volume: Volume) -> ZombieState {
    let (mut w, a, _) = field(&[]);
    let z = w.add_zombie(me() + Vec2::new(22.0, 0.0), 0.0, 1.0);
    w.step();
    assert_eq!(w.zombies.state[z], ZombieState::Wander);
    w.player_say(a, "HELP!", volume).unwrap();
    for _ in 0..3 {
        w.step();
    }
    if w.zombies.state[z] == ZombieState::Investigate {
        assert!(w.zombies.target_pos[z].distance(me()) < 0.01, "it heads for the voice");
    }
    w.zombies.state[z]
}

#[test]
fn shouting_is_a_noise_zombies_hear() {
    assert_eq!(zombie_hears(Volume::Shout), ZombieState::Investigate, "a shout carries 30 m to a zombie");
    assert_eq!(zombie_hears(Volume::Talk), ZombieState::Wander, "talk only 7 m");
    assert_eq!(zombie_hears(Volume::Whisper), ZombieState::Wander);
    // The noises are in order, a whisper quieter than sneaking past and a shout over the
    // migration's threshold (it leaves an attractor for the far horde).
    let (mut w, a, _) = field(&[]);
    let (s, z) = (w.config.speech.clone(), w.config.zombie.clone());
    assert!(s.whisper_noise < z.walk_noise && s.talk_noise > z.walk_noise && s.talk_noise < z.run_noise && s.shout_noise > z.glass_break_noise);
    assert!(s.shout_noise >= w.config.population.migration.min_radius);
    let before = w.population().attractors.len();
    w.player_say(a, "OVER HERE!", Volume::Shout).unwrap();
    w.step();
    w.step();
    assert_eq!(w.population().attractors.len(), before + 1, "a shout draws the far horde");
}

/// A sleeper wakes to a voice near by, as to any other noise.
#[test]
fn a_voice_wakes_a_sleeper() {
    let (mut w, a, ids) = field(&[me() + Vec2::new(0.0, 4.0)]);
    let b = ids[0];
    w.players.iter_mut().find(|p| p.id == b).unwrap().needs.tiredness = 0.8;
    assert!(w.player_sleep(b).is_ok());
    w.step();
    assert!(w.player(b).unwrap().needs.asleep());
    assert_eq!(w.player_say(a, "Wake up.", Volume::Talk), Some(0), "asleep, it doesn't take the words in");
    w.step();
    w.step();
    assert!(!w.player(b).unwrap().needs.asleep(), "but the voice wakes it");
}

#[test]
fn gestures_last_a_moment_and_hands_stay_up() {
    let (mut w, a, _) = field(&[]);
    let kind = |w: &World| w.player(a).unwrap().gesture.map(|g| g.kind);
    let hash = w.state_hash();
    assert!(w.player_gesture(a, Some(Gesture::Wave)));
    assert_eq!(kind(&w), Some(Gesture::Wave));
    assert_eq!(w.state_hash(), hash, "a pose, not simulation state");
    let ticks = (w.config.speech.gesture_secs * crate::TICK_RATE as f32) as u32;
    for _ in 0..ticks - 2 {
        w.step();
    }
    assert_eq!(kind(&w), Some(Gesture::Wave));
    for _ in 0..4 {
        w.step();
    }
    assert_eq!(kind(&w), None, "a wave runs out");

    // Hands stay up until lowered: by the same gesture again, by another, or by `None`.
    assert!(w.player_gesture(a, Some(Gesture::HandsUp)));
    for _ in 0..400 {
        w.step();
    }
    assert_eq!(kind(&w), Some(Gesture::HandsUp));
    assert!(w.player_gesture(a, Some(Gesture::HandsUp)));
    assert_eq!(kind(&w), None);
    assert!(w.player_gesture(a, Some(Gesture::HandsUp)) && w.player_gesture(a, Some(Gesture::Point)));
    assert_eq!(kind(&w), Some(Gesture::Point));
    assert!(w.player_gesture(a, None) && !w.player_gesture(a, None));

    // A swing ends it, and none starts mid-swing.
    assert!(w.player_gesture(a, Some(Gesture::HandsUp)));
    assert!(w.player_attack(a));
    w.step();
    assert_eq!(kind(&w), None, "you can't swing with your hands up");
    assert!(!w.player_gesture(a, Some(Gesture::Wave)), "mid-swing");
    assert!(!w.player_gesture(99, Some(Gesture::Wave)));
}

#[test]
fn names_parse() {
    for v in Volume::ALL {
        assert_eq!(Volume::parse(v.name()), Some(v));
    }
    for g in Gesture::ALL {
        assert_eq!(Gesture::parse(g.name()), Some(g));
    }
    assert_eq!(Gesture::parse("Hands-Up"), Some(Gesture::HandsUp));
    assert_eq!(Volume::parse("YELL"), Some(Volume::Shout));
    assert!(Volume::parse("sing").is_none() && Gesture::parse("dance").is_none());
}
