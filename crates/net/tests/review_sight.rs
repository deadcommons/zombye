//! Saves for reviewing fair information (#66) in offscreen pictures: two survivors on a PvP world,
//! the second where the first does or doesn't see it. Ignored: run it by hand (the folder as for
//! `review_saves.rs`: `ZOMBYE_REVIEW_DIR`, else `target/review`):
//!
//! `cargo test -p zombye-net --release --test review_sight -- --ignored`
//!
//! then `zombye --render-to shot.png --load target/review/sight_seen.zsave --zoom 16` (player 1's
//! view; `--player 2` for the other's). `sight_seen`: the other in plain view 7 m ahead in the
//! street; `sight_behind`: the same two, ours facing the other way; `sight_wall`: ours in the
//! bedroom, the other outside the nearest wall, faced; `sight_dark`: the street at 1 am, the other
//! 16 m ahead; `sight_dark_lit`: the same with a flashlight in the other's hand.

use std::path::PathBuf;

use glam::{IVec2, Vec2};
use zombye_net::{content, save_file};
use zombye_sim::vision::line_of_sight;
use zombye_sim::{Slot, World};

fn world(hour: f32) -> (World, u32) {
    let mut config = content::load_config(&content::default_data_dir(), 1).unwrap();
    config.zombies = Some(0);
    config.combat.pvp = true;
    config.start_hour = hour;
    let mut w = World::new(config);
    let id = w.add_player(w.player_spawn);
    (w, id)
}

fn save(w: &World, name: &str) {
    let dir = std::env::var("ZOMBYE_REVIEW_DIR").map_or_else(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/review"), PathBuf::from);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{name}.zsave"));
    save_file::save(w, &path).unwrap();
    println!("wrote {}", path.display());
}

fn open(w: &World, t: IVec2) -> bool {
    w.map.in_bounds(t) && w.map.tile(t).room == 0 && !w.map.tile_blocked(t)
}

/// The nearest spot to the spawn house with `len` metres of open ground straight east of it and
/// 15 m west of it, a few metres wide (a street).
fn street(w: &World, len: i32) -> Vec2 {
    let c = w.player_spawn.floor().as_ivec2();
    let mut best: Option<(i32, IVec2)> = None;
    for dy in -40..=40 {
        for dx in -40..=40 {
            let t = c + IVec2::new(dx, dy);
            let d = dx * dx + dy * dy;
            if best.is_none_or(|(b, _)| d < b) && (-15..=len).all(|k| (-3..=1).all(|j| open(w, t + IVec2::new(k, j)))) {
                best = Some((d, t));
            }
        }
    }
    best.expect("a stretch of open ground near the spawn house").1.as_vec2() + Vec2::splat(0.5)
}

fn place(w: &mut World, id: u32, at: Vec2, facing: f32) {
    let p = w.players.iter_mut().find(|p| p.id == id).unwrap();
    p.state.pos = at;
    p.state.facing = facing;
    p.input.facing = facing;
}

#[test]
#[ignore]
fn review_sight() {
    // In plain view, and the same with our back turned.
    let (mut w, me) = world(13.0);
    let at = street(&w, 18);
    let other = w.add_player(at + Vec2::new(7.0, 0.0));
    place(&mut w, me, at, 0.0);
    place(&mut w, other, at + Vec2::new(7.0, 0.0), std::f32::consts::PI);
    assert!(w.hold_item(other, Slot::Hand, "crowbar"));
    assert!(w.sees_now(me, other));
    save(&w, "sight_seen");
    place(&mut w, me, at, std::f32::consts::PI);
    assert!(!w.sees_now(me, other), "behind us");
    save(&w, "sight_behind");

    // To watch live (`--load sight_walker`, no `--render-to`): the other starts 13 m behind us and
    // keeps walking east past us, 2 m to our left (a save keeps a player's last input). Behind
    // our back it is only heard, from 6 m: rings on the ground; abreast of us it comes into view.
    place(&mut w, me, at, 0.0);
    place(&mut w, other, at + Vec2::new(-13.0, -2.0), 0.0);
    w.players.iter_mut().find(|p| p.id == other).unwrap().input.move_dir = Vec2::X;
    assert!(!w.sees_now(me, other));
    save(&w, "sight_walker");

    // In the dark, 16 m off; then with a light in its hand.
    let (mut w, me) = world(1.0);
    let other = w.add_player(at + Vec2::new(16.0, 0.0));
    place(&mut w, me, at, 0.0);
    place(&mut w, other, at + Vec2::new(16.0, 0.0), std::f32::consts::PI);
    assert!(!w.sees_now(me, other), "dark");
    save(&w, "sight_dark");
    assert!(w.hold_item(other, Slot::Hand, "flashlight"));
    assert!(w.sees_now(me, other), "its light");
    save(&w, "sight_dark_lit");

    // Behind a wall: we stand in the bedroom, the other outside the nearest wall, and we face it.
    let (mut w, me) = world(13.0);
    let home = w.player_spawn;
    let spot = (0..64)
        .flat_map(|k| (6..16).map(move |r| (r, k)))
        .map(|(r, k)| home + Vec2::from_angle(k as f32 / 64.0 * std::f32::consts::TAU) * (r as f32 * 0.5))
        .filter(|p| open(&w, p.floor().as_ivec2()) && !line_of_sight(&w.map, home, *p))
        .min_by(|a, b| a.distance(home).total_cmp(&b.distance(home)))
        .expect("somewhere outside the bedroom wall");
    let spot = spot.floor() + Vec2::splat(0.5);
    let other = w.add_player(spot);
    place(&mut w, me, home, (spot - home).to_angle());
    place(&mut w, other, spot, (home - spot).to_angle());
    assert!(w.hold_item(other, Slot::Hand, "crowbar"));
    assert!(!w.sees_now(me, other), "a wall between");
    println!("sight_wall: us at {home}, the other at {spot} ({:.1} m)", spot.distance(home));
    save(&w, "sight_wall");
}
