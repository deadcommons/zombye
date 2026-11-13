//! For looking at the sample mod in the game, and for measuring what scripts cost. Ignored: run by
//! hand.
//!
//! `cargo test -p zombye-mods --release --test review -- --ignored --nocapture`
//!
//! `alarm_clock_saves` writes (to `ZOMBYE_REVIEW_DIR`, else `target/review`) `alarm_clock.zsave`, a
//! survivor in the street with an alarm clock in the pocket and zombies up the road, and
//! `alarm_clock_ringing.zsave`, the same half a minute after using it. Play the first with
//! `zombye --mod mods/alarm_clock --load PATH --ignored` and right-click the clock in the inventory.

use std::path::PathBuf;
use std::time::Instant;

use glam::{IVec2, Vec2};
use zombye_mods::{Mod, apply, load_into};
use zombye_net::{content, save_file};
use zombye_sim::{Place, PlayerInput, World, WorldConfig};

fn sample_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../mods/alarm_clock")
}

fn config(seed: u64) -> WorldConfig {
    content::load_config(&content::default_data_dir(), seed).unwrap()
}

fn save(w: &World, name: &str) {
    let dir = std::env::var("ZOMBYE_REVIEW_DIR").map_or_else(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/review"), PathBuf::from);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{name}.zsave"));
    save_file::save(w, &path).unwrap();
    println!("wrote {}", path.display());
}

/// The nearest point to `from` with open ground outdoors for `clear` tiles all round.
fn open_ground(w: &World, from: Vec2, clear: i32) -> Vec2 {
    let c = from.floor().as_ivec2();
    let free = |t: IVec2| w.map.in_bounds(t) && !w.map.tile_blocked(t) && w.map.room_at(t.as_vec2() + Vec2::splat(0.5)) == 0 && w.map.building_at(t.as_vec2() + Vec2::splat(0.5)).is_none();
    let mut best: Option<(i32, IVec2)> = None;
    for dy in -40..=40 {
        for dx in -40..=40 {
            let t = c + IVec2::new(dx, dy);
            let d = dx * dx + dy * dy;
            if best.is_some_and(|(b, _)| d >= b) {
                continue;
            }
            if (-clear..=clear).all(|y| (-clear..=clear).all(|x| free(t + IVec2::new(x, y)))) {
                best = Some((d, t));
            }
        }
    }
    best.expect("open ground near the start").1.as_vec2() + Vec2::splat(0.5)
}

#[test]
#[ignore]
fn alarm_clock_saves() {
    let mut config = config(1);
    load_into(&mut config, &[sample_dir()]).unwrap();
    config.zombies = Some(0);
    config.start_hour = 15.0;
    let mut w = World::new(config);
    let at = open_ground(&w, w.player_spawn, 7);
    let id = w.add_player(at);
    w.set_dev(id, None, Some(true));
    assert!(w.give_item(id, "alarm_clock", 1) && w.give_item(id, "water_bottle", 1) && w.give_item(id, "apple", 2));
    // Two knots of zombies up and down the road, out of the first view.
    let placed = w.spawn_zombies(9, at + Vec2::new(13.0, -2.0), 3.0) + w.spawn_zombies(7, at + Vec2::new(-12.0, 4.0), 3.0);
    println!("survivor at {at}, {placed} zombies");
    w.step();
    save(&w, "alarm_clock");
    // Used: set down, ticking; the survivor steps back to watch.
    w.player_use(id, Place::Carried, 0).unwrap();
    for t in 0..620u32 {
        if t == 30 {
            w.set_player_input(id, 1, PlayerInput { move_dir: Vec2::new(0.6, -0.8), ..Default::default() });
        }
        if t == 90 {
            w.set_player_input(id, 2, PlayerInput { facing: 2.2, ..Default::default() });
        }
        w.step();
    }
    assert!(w.script_errors().is_empty(), "{:?}", w.script_errors());
    let near = (0..w.zombies.len()).filter(|&i| w.zombies.pos[i].distance(at) < 6.0).count();
    println!("after 31 s: {near} of {placed} zombies within 6 m of the clock");
    save(&w, "alarm_clock_ringing");
}

/// What scripts cost a tick: the default town with its ~2,950 zombies and a running player, with
/// no mods, with the sample mod idle, with a clock ringing, and with a script that works hard
/// every tick.
#[test]
#[ignore]
fn script_cost() {
    let run = |label: &str, mods: Vec<Mod>, use_clock: bool| {
        let mut config = config(1);
        apply(&mut config, &mods).unwrap();
        let mut w = World::new(config);
        let id = w.add_player(w.player_spawn);
        w.give_item(id, "alarm_clock", 1);
        w.set_player_input(id, 1, PlayerInput { move_dir: Vec2::new(1.0, 0.3), run: true, ..Default::default() });
        for _ in 0..100 {
            w.step();
        }
        if use_clock {
            w.player_use(id, Place::Carried, 0).unwrap();
        }
        let (mut total, mut worst) = (0.0f64, 0.0f64);
        let ticks = 1200;
        for _ in 0..ticks {
            let start = Instant::now();
            w.step();
            let ms = start.elapsed().as_secs_f64() * 1000.0;
            total += ms;
            worst = worst.max(ms);
        }
        assert!(w.script_errors().is_empty(), "{:?}", w.script_errors());
        println!("{label:<34} {:.3} ms/tick, worst {:.2} ms", total / ticks as f64, worst);
    };
    let sample = || Mod::load(&sample_dir()).unwrap();
    let busy = Mod::new("busy").with_script(
        "busy.luau",
        "function on_tick(game) local n = 0 for _, p in ipairs(game:players()) do local me = game:player(p) n += game:zombies_near(me.x, me.y, 20) end local t = game:get('t') or {} t[#t % 50 + 1] = n game:set('t', t) end",
    );
    for _ in 0..2 {
        run("no mods", vec![], false);
        run("alarm_clock, idle", vec![sample()], false);
        run("alarm_clock, a clock ringing", vec![sample()], true);
        run("a script busy every tick", vec![busy.clone()], false);
    }
}
