//! Waking up and walking out (a playtest bug: "the player can't leave the starting room, he
//! doesn't fit through the door"). The default single-player world as the client makes it (the
//! shipped data, seed 1, 14:00, one player in the bedroom), and a player driven only by the
//! intents a client sends: it walks tile by tile to the nearest ground outdoors, opening each
//! shut door on the way with `Interact`, and must get there. The client's prediction
//! (`step_movement` on the snapshot's state) has to agree with the server all the way.
//!
//! These hold the simulation's side of it: a body fits through every doorway in town, walked at
//! straight or on a diagonal (one WASD key is a diagonal under the isometric camera), with at
//! most a few ticks of sliding round a jamb. They passed when the bug was reported: what kept
//! the player in was the client hiding a shut door's leaf with its cut-away wall, so the doorway
//! looked open (`world_render::piece_look` and its test).

use std::collections::VecDeque;

use glam::{IVec2, Vec2};
use zombye_net::{Intent, LocalServer, content};
use zombye_sim::map::{Edge, Side, TileMap};
use zombye_sim::players::{PLAYER_RADIUS, RUN_SPEED, WALK_SPEED, step_movement};
use zombye_sim::ai::ZOMBIE_RADIUS;
use zombye_sim::{PlayerInput, TICK_DT};

const SIDES: [Side; 4] = [Side::N, Side::E, Side::S, Side::W];

/// The client's own start (`server_link.rs`): the shipped data, this seed, waking at 14:00.
fn single_player(seed: u64) -> (LocalServer, u32) {
    let mut config = content::load_config(&content::default_data_dir(), seed).expect("the shipped content");
    config.start_hour = 14.0;
    let mut server = LocalServer::new(config);
    let id = server.join(None);
    // The town's zombies are not what this is about.
    server.send(Intent::Dev { player: id, god: Some(true), ignored: Some(true) });
    (server, id)
}

fn walkable(map: &TileMap, a: IVec2, side: Side) -> bool {
    let b = a + side.offset();
    !map.tile_blocked(b) && (!map.edge(a, side).blocks_movement() || map.edge(a, side) == Edge::DoorClosed)
}

/// The tiles from `from` to the nearest tile outdoors, through doors (shut ones too).
fn way_out(map: &TileMap, from: IVec2) -> Vec<IVec2> {
    let mut came: std::collections::BTreeMap<(i32, i32), IVec2> = std::collections::BTreeMap::new();
    let mut queue = VecDeque::from([from]);
    came.insert((from.x, from.y), from);
    while let Some(t) = queue.pop_front() {
        if map.tile(t).room == 0 {
            let mut path = vec![t];
            let mut at = t;
            while at != from {
                at = came[&(at.x, at.y)];
                path.push(at);
            }
            path.reverse();
            return path;
        }
        for side in SIDES {
            let n = t + side.offset();
            if walkable(map, t, side) && !came.contains_key(&(n.x, n.y)) {
                came.insert((n.x, n.y), t);
                queue.push_back(n);
            }
        }
    }
    panic!("no way out of the house from {from}, even through its doors");
}

fn side_to(a: IVec2, b: IVec2) -> Side {
    SIDES.into_iter().find(|s| a + s.offset() == b).expect("neighbouring tiles")
}

/// Walk the player along `path` with a client's intents. Returns where it stopped if it got stuck.
fn walk(server: &mut LocalServer, id: u32, path: &[IVec2], run: bool) -> Result<u32, String> {
    let mut seq = 0u32;
    let mut ticks = 0u32;
    let mut predicted = server.world.player(id).unwrap().state;
    for pair in path.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let side = side_to(a, b);
        let target = b.as_vec2() + Vec2::splat(0.5);
        let mut spent = 0;
        loop {
            let pos = server.world.player(id).unwrap().state.pos;
            if pos.distance(target) < 0.15 {
                break;
            }
            let to = target - pos;
            let facing = to.y.atan2(to.x);
            let here = pos.floor().as_ivec2();
            if here == a && server.world.map.edge(a, side) == Edge::DoorClosed {
                // Face the door and open it, as F does.
                seq += 1;
                server.send(Intent::PlayerInput { player: id, seq, move_dir: [0.0, 0.0], facing, run: false, sneak: false, aim: false });
                step_movement(&server.world.map, &mut predicted, &PlayerInput { move_dir: Vec2::ZERO, facing, run: false, sneak: false, aim: false }, TICK_DT);
                server.tick();
                server.send(Intent::Interact { player: id });
            }
            seq += 1;
            let dir = to.normalize_or_zero();
            server.send(Intent::PlayerInput { player: id, seq, move_dir: dir.into(), facing, run, sneak: false, aim: false });
            let snap = server.tick();
            // Prediction: the map as the client has it after this snapshot's edge changes is the
            // server's, so predicting on the server's map is what the client does.
            step_movement(&server.world.map, &mut predicted, &PlayerInput { move_dir: dir, facing, run, sneak: false, aim: false }, TICK_DT);
            let view = snap.players.iter().find(|p| p.id == id).unwrap();
            if view.state() != predicted {
                // A door opening changes the map between the client's step and the server's; the
                // snapshot puts it right, as in the game.
                predicted = view.state();
            }
            ticks += 1;
            spent += 1;
            if spent > 20 * 6 {
                let p = server.world.player(id).unwrap().state.pos;
                return Err(format!("stuck at {p} going from tile {a} to {b} ({side:?} edge: {:?}) after {spent} ticks", server.world.map.edge(a, side)));
            }
        }
    }
    Ok(ticks)
}

#[test]
fn the_player_walks_out_of_the_bedroom_and_the_house() {
    for seed in 1..=4u64 {
        for run in [false, true] {
            let (mut server, id) = single_player(seed);
            server.tick();
            let start = server.world.player(id).unwrap().state.pos;
            assert_ne!(server.world.map.room_at(start), 0, "seed {seed}: wakes up indoors");
            let path = way_out(&server.world.map, start.floor().as_ivec2());
            let doors = path.windows(2).filter(|p| server.world.map.edge(p[0], side_to(p[0], p[1])).is_door()).count();
            assert!(doors >= 1, "seed {seed}: a way out goes through a door");
            match walk(&mut server, id, &path, run) {
                Ok(_) => {}
                Err(e) => panic!("seed {seed}, run {run}: {e}\npath {path:?}"),
            }
            let end = server.world.player(id).unwrap().state.pos;
            assert_eq!(server.world.map.room_at(end), 0, "seed {seed}: outdoors at {end}");
        }
    }
}

/// Every doorway of the default town (each door, opened, and each arch; from both sides) lets a
/// body through without sticking: a player walking or running, and a body of a zombie's size at
/// a zombie's pace, held on one heading (straight at it, or on either diagonal, as one WASD key
/// is) along a line that crosses the doorway up to 0.4 m off its middle. It is through within a
/// few ticks of what open ground would take: all it may lose is the slide round a jamb.
#[test]
fn every_doorway_in_town_lets_a_player_and_a_zombie_through() {
    /// Ticks lost to a jamb at most (measured: 6 walking, on a diagonal 0.4 m off the middle).
    const SLACK: u32 = 8;
    let (mut server, id) = single_player(1);
    server.tick();
    let w = &mut server.world;
    let mut doorways = vec![];
    for y in 1..w.map.height() - 1 {
        for x in 1..w.map.width() - 1 {
            for side in [Side::N, Side::W] {
                let a = IVec2::new(x, y);
                let e = w.map.edge(a, side);
                if (e.is_door() || e == Edge::Arch) && !w.map.tile_blocked(a) && !w.map.tile_blocked(a + side.offset()) {
                    if e.is_door() {
                        w.map.set_edge(a, side, Edge::DoorOpen);
                    }
                    doorways.push((a, side));
                }
            }
        }
    }
    w.map_changed();
    assert!(doorways.len() > 500, "the town's doorways: {}", doorways.len());
    let template = w.player(id).unwrap().state;
    let map = &w.map;
    let mut walked = 0u32;
    for &(a, side) in &doorways {
        for (from, n) in [(a, side.offset().as_vec2()), (a + side.offset(), -side.offset().as_vec2())] {
            let lat = n.perp();
            let mid = from.as_vec2() + Vec2::splat(0.5) + n * 0.5;
            for d in [n, (n + lat).normalize(), (n - lat).normalize()] {
                for ci in -8..=8i32 {
                    let cross = mid + lat * (ci as f32 * 0.05);
                    let start = cross - d * (0.7 / d.dot(n));
                    // Only lines with a clear run up to the jambs (no furniture or wall in the way).
                    let clear = (0..20).all(|k| {
                        let p = start.lerp(cross, k as f32 / 20.0);
                        (cross - p).dot(n) < 0.4 || map.resolve_circle(p, PLAYER_RADIUS.max(ZOMBIE_RADIUS)).distance(p) < 1e-4
                    });
                    if !clear {
                        continue;
                    }
                    walked += 1;
                    let through = |p: Vec2| (p - mid).dot(n) > 0.35;
                    let ideal = |speed: f32| ((0.7 + 0.35) / d.dot(n) / (speed * TICK_DT)).ceil() as u32;
                    for run in [false, true] {
                        let mut s = template;
                        s.pos = start;
                        let input = PlayerInput { move_dir: d, facing: 0.0, run, sneak: false, aim: false };
                        let limit = ideal(if run { RUN_SPEED } else { WALK_SPEED }) + SLACK;
                        let took = (1..=limit).find(|_| {
                            step_movement(map, &mut s, &input, TICK_DT);
                            through(s.pos)
                        });
                        assert!(took.is_some(), "a player (run {run}) heading {d} from {start} isn't through the doorway {a} {side:?} in {limit} ticks: at {}", s.pos);
                    }
                    // A zombie's body at a slow zombie's pace, moved as the zombie update moves one.
                    let speed = 1.2;
                    let mut p = start;
                    let limit = ideal(speed) + 2 * SLACK;
                    let took = (1..=limit).find(|_| {
                        p = map.resolve_circle(p + d * speed * TICK_DT, ZOMBIE_RADIUS);
                        through(p)
                    });
                    assert!(took.is_some(), "a zombie heading {d} from {start} isn't through the doorway {a} {side:?} in {limit} ticks: at {p}");
                }
            }
        }
    }
    assert!(walked > 50_000, "lines walked: {walked}");
}
