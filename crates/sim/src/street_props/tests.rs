use glam::{IVec2, Vec2};

use super::*;
use crate::map::{Edge, Feature, Floor, Side, TileMap};
use crate::players::{PLAYER_RADIUS, PlayerInput, PlayerState, step_movement};
use crate::template::tests::shipped_templates;
use crate::world::{World, WorldConfig};
use crate::worldgen::{GenConfig, Generated, generate, generate_with};

/// The shipped street-prop file.
fn shipped() -> StreetPropsDef {
    parse_street_props(include_str!("../../../../assets/data/street_props.ron")).unwrap()
}

fn town(seed: u64) -> Generated {
    generate_with(&GenConfig { seed, ..Default::default() }, &shipped_templates(), &shipped())
}

/// Every tile a prop covers, by index: which prop.
fn covered(map: &TileMap) -> std::collections::BTreeMap<(i32, i32), usize> {
    let mut out = std::collections::BTreeMap::new();
    for (i, p) in map.street_props.iter().enumerate() {
        let (min, size) = map.street_prop_rect(p);
        for dy in 0..size.y {
            for dx in 0..size.x {
                assert!(out.insert((min.x + dx, min.y + dy), i).is_none(), "two props on {}", min + IVec2::new(dx, dy));
            }
        }
    }
    out
}

fn kind_id<'a>(map: &'a TileMap, p: &StreetProp) -> &'a str {
    &map.prop_kinds[p.kind as usize].id
}

/// Outdoor tiles reachable on foot from a point on the main street.
fn reachable(g: &Generated) -> Vec<bool> {
    let m = &g.map;
    let w = m.width();
    let mut seen = vec![false; (w * m.height()) as usize];
    let start = g.plan.streets[0].0 + IVec2::new(1, 4);
    let mut stack = vec![start];
    seen[(start.y * w + start.x) as usize] = true;
    while let Some(p) = stack.pop() {
        for s in Side::ALL {
            let q = p + s.offset();
            if !m.in_bounds(q) || m.tile(q).room != 0 || !m.can_step(p, s) || seen[(q.y * w + q.x) as usize] {
                continue;
            }
            seen[(q.y * w + q.x) as usize] = true;
            stack.push(q);
        }
    }
    seen
}

#[test]
fn the_shipped_file_parses_and_unknown_fields_are_refused() {
    let def = shipped();
    assert_eq!(def.kinds.len(), 32);
    assert!(def.kinds.iter().all(|k| k.blocks || k.id == "trash_bag" || k.id == "washing_line"));
    let car = def.kinds.iter().find(|k| k.id == "car").unwrap();
    assert!(car.big() && car.footprint(Side::S) == IVec2::new(2, 5) && car.footprint(Side::E) == IVec2::new(5, 2));
    let big = "(kinds: [(id: \"x\", size: (2.0, 4.0, 1.0), color: (0.0, 0.0, 0.0), tiles: (2, 5))], rules: [(kind: \"x\", place: PLACE, chance: 0.5)])";
    assert!(parse_street_props(&big.replace("PLACE", "Roadside")).is_ok());
    assert!(parse_street_props(&big.replace("PLACE", "HouseKerb")).unwrap_err().contains("more than one tile"));
    assert!(parse_street_props(&big.replace("(2, 5)", "(3, 5)").replace("PLACE", "Parking")).unwrap_err().contains("parking bay"));
    assert!(parse_street_props(&big.replace("(2, 5)", "(6, 5)").replace("PLACE", "Yard")).unwrap_err().contains("tiles"));
    assert!((def.kinds[0].radius() - 0.2).abs() < 1e-6 && (def.kinds[2].radius() - 0.33).abs() < 1e-6);
    assert!(parse_street_props("(kinds: [], extra: 1)").is_err());
    assert!(parse_street_props("(kinds: [(id: \"x\", size: (0.5, 0.5, 0.0), color: (0.0, 0.0, 0.0))])").unwrap_err().contains("size"));
    assert!(parse_street_props("(kinds: [(id: \"x\", size: (0.5, 0.5, 1.0), color: (0.0, 0.0, 0.0))], rules: [(kind: \"x\", place: GardenPath, spacing: Some((40.0, 60.0)))])")
        .unwrap_err()
        .contains("only Kerb"));
    assert_eq!(parse_street_props("()").unwrap(), StreetPropsDef::default());
}

/// The same seed gives the same props, another seed others; the rest of the town is the same with
/// or without them, but for the props' tiles.
#[test]
fn placement_is_deterministic_and_only_adds_props() {
    let (a, b, c) = (town(1), town(1), town(2));
    assert!(!a.map.street_props.is_empty());
    assert_eq!(a.map.street_props, b.map.street_props);
    assert_ne!(a.map.street_props, c.map.street_props);
    assert_eq!(crate::save::map_hash(&a.map), crate::save::map_hash(&b.map));
    let bare = generate(&GenConfig::default(), &shipped_templates());
    assert!(bare.map.street_props.is_empty());
    assert_eq!(bare.player_spawn, a.player_spawn);
    assert_eq!(bare.map.buildings.len(), a.map.buildings.len());
    let under = covered(&a.map);
    for y in 0..a.map.height() {
        for x in 0..a.map.width() {
            let t = IVec2::new(x, y);
            let (p, q) = (a.map.tile(t), bare.map.tile(t));
            if under.contains_key(&(x, y)) {
                assert_eq!((p.floor, p.room, q.feature), (q.floor, q.room, Feature::None), "{t}");
            } else {
                assert_eq!(p, q, "{t}");
            }
        }
    }
    // Sorted by tile, one a tile, every tile a blocking one covers a StreetProp.
    for w in a.map.street_props.windows(2) {
        assert!((w[0].tile.y, w[0].tile.x) < (w[1].tile.y, w[1].tile.x));
    }
    for p in &a.map.street_props {
        let (min, size) = a.map.street_prop_rect(p);
        for q in (0..size.y).flat_map(|dy| (0..size.x).map(move |dx| min + IVec2::new(dx, dy))) {
            let want = if a.map.prop_kinds[p.kind as usize].blocks { Feature::StreetProp } else { Feature::None };
            assert_eq!(a.map.tile(q).feature, want, "{} at {q}", kind_id(&a.map, p));
        }
        assert_eq!(a.map.street_prop(p.tile), Some(p));
    }
}

/// Mailboxes on the lawn beside a garden path's street end, kerb props at a straight kerb facing
/// the road; none on a road, path or driveway, beside a door, or next to another prop. Prints the
/// counts.
#[test]
fn props_stand_where_they_belong() {
    for seed in 1..=3 {
        let g = town(seed);
        let m = &g.map;
        let mut counts = std::collections::BTreeMap::new();
        // Driveways: straight out from every garage opening to a sidewalk and across it, a tile
        // either side.
        let mut drive = std::collections::BTreeSet::new();
        for (bi, b) in m.buildings.iter().enumerate() {
            for t in (b.min.y..b.min.y + b.size.y).flat_map(|y| (b.min.x..b.min.x + b.size.x).map(move |x| IVec2::new(x, y))) {
                let r = m.tile(t).room;
                if r == 0 || m.rooms[r as usize].building as usize != bi || m.rooms[r as usize].room_type != "garage" {
                    continue;
                }
                for s in Side::ALL {
                    let e = m.edge(t, s);
                    if m.tile(t + s.offset()).room == 0 && (e.is_door() || e == Edge::Arch) {
                        // Out to the first sidewalk and across it, as the client draws them.
                        let (mut q, mut on_walk) = (t + s.offset(), false);
                        for _ in 0..40 {
                            let tile = m.tile(q);
                            if !m.in_bounds(q) || tile.floor.is_road() || tile.room != 0 || (on_walk && tile.floor != Floor::Sidewalk) {
                                break;
                            }
                            on_walk |= tile.floor == Floor::Sidewalk;
                            for k in -1..=1 {
                                let d = q + along(s) * k;
                                drive.insert((d.x, d.y));
                            }
                            q += s.offset();
                        }
                    }
                }
            }
        }
        let under = covered(m);
        for (pi, p) in m.street_props.iter().enumerate() {
            let id = kind_id(m, p);
            *counts.entry(id).or_insert(0) += 1;
            let kind = &m.prop_kinds[p.kind as usize];
            let (min, size) = m.street_prop_rect(p);
            let parked = ["car", "van", "pickup", "police_car", "ambulance", "car_wreck", "school_bus"].contains(&id);
            assert_eq!(parked, kind.tiles.1 >= 5 && id != "washing_line", "{id}");
            for q in (0..size.y).flat_map(|dy| (0..size.x).map(move |dx| min + IVec2::new(dx, dy))) {
                let tile = m.tile(q);
                assert_eq!(tile.room, 0, "seed {seed}: {id} at {q} indoors");
                assert_eq!(tile.floor.is_road(), parked, "seed {seed}: {id} at {q} on {:?}", tile.floor);
                assert!(tile.floor != Floor::Path, "seed {seed}: {id} at {q} on a path");
                assert!(!drive.contains(&(q.x, q.y)), "seed {seed}: {id} at {q} on a driveway");
                for s in Side::ALL {
                    let e = m.edge(q, s);
                    let inside = (q + s.offset() - min).cmpge(IVec2::ZERO).all() && (q + s.offset() - min).cmplt(size).all();
                    assert!(inside && e == Edge::None || !inside && !(e.is_door() || e == Edge::Arch || e == Edge::DoorBroken), "seed {seed}: {id} at {q}: {e:?} to the {s:?}");
                }
                // Nothing else on it or next to it.
                for dy in -1..=1 {
                    for dx in -1..=1 {
                        assert!(under.get(&(q.x + dx, q.y + dy)).is_none_or(|&o| o == pi), "seed {seed}: {id} at {q} beside another prop");
                    }
                }
            }
            if parked {
                // Along a kerb or in a lot, and the road beside it stays open: two tiles of it free
                // on one long side at least.
                let long = if size.x > size.y { IVec2::X } else { IVec2::Y };
                let across = long.perp().abs();
                let wide = size.dot(across);
                let open = [-1, 1].into_iter().any(|way| {
                    (0..size.dot(long)).all(|j| {
                        (1..=2).all(|w| {
                            let q = min + long * j + across * if way > 0 { wide - 1 + w } else { -w };
                            m.tile(q).floor.is_road() && !m.tile_blocked(q)
                        })
                    })
                });
                // In a lot's bay the aisle is at one end.
                let len = size.dot(long);
                let aisle = [-1, 1].into_iter().any(|way| {
                    (0..wide).all(|j| {
                        (1..=2).all(|w| {
                            let q = min + across * j + long * if way > 0 { len - 1 + w } else { -w };
                            m.tile(q).floor.is_road() && !m.tile_blocked(q)
                        })
                    })
                });
                assert!(open || aisle, "seed {seed}: {id} at {} shuts its street", p.tile);
                continue;
            }
            if matches!(m.tile(p.tile).floor, Floor::Grass) && id != "mailbox" {
                // A yard: on lawn, a tile of open lawn all round it.
                for dy in -1..=size.y {
                    for dx in -1..=size.x {
                        let q = min + IVec2::new(dx, dy);
                        let tile = m.tile(q);
                        let own = under.get(&(q.x, q.y)) == Some(&pi);
                        assert!(tile.floor == Floor::Grass && tile.room == 0 && (own || tile.feature == Feature::None), "seed {seed}: {id} at {} has {:?} {:?} at {q}", p.tile, tile.floor, tile.feature);
                    }
                }
                assert!(!["fire_hydrant", "street_lamp", "trash_can", "park_bench"].contains(&id), "seed {seed}: {id} in a yard");
                continue;
            }
            let tile = m.tile(p.tile);
            let ahead = p.tile + p.facing.offset();
            match id {
                "mailbox" => {
                    assert_eq!(tile.floor, Floor::Grass, "seed {seed}: mailbox at {}", p.tile);
                    assert_eq!(m.tile(ahead).floor, Floor::Sidewalk, "seed {seed}: mailbox at {} not facing the sidewalk", p.tile);
                    let a = along(p.facing);
                    assert!([p.tile + a, p.tile - a].iter().any(|&q| m.tile(q).floor == Floor::Path), "seed {seed}: mailbox at {} not by a path", p.tile);
                }
                _ => {
                    assert_eq!(tile.floor, Floor::Sidewalk, "seed {seed}: {id} at {}", p.tile);
                    assert!(m.tile(ahead).floor.is_road(), "seed {seed}: {id} at {} not at the kerb", p.tile);
                    let a = along(p.facing);
                    for k in -STRAIGHT..=STRAIGHT {
                        assert!(m.tile(p.tile + a * k).floor == Floor::Sidewalk && m.tile(ahead + a * k).floor.is_road(), "seed {seed}: {id} at {} not on a straight kerb", p.tile);
                    }
                }
            }
        }
        eprintln!("seed {seed}: {counts:?} ({} houses with paths)", m.buildings.iter().filter(|b| b.template.starts_with("house")).count());
        assert!(counts.get("mailbox").copied().unwrap_or(0) >= 40, "seed {seed}: {counts:?}");
        assert!(counts.get("fire_hydrant").copied().unwrap_or(0) >= 40, "seed {seed}: {counts:?}");
        assert!(counts.get("trash_can").copied().unwrap_or(0) >= 10, "seed {seed}: {counts:?}");
        assert!(counts.get("car").copied().unwrap_or(0) >= 12, "seed {seed}: {counts:?}");
        assert!(counts.get("garden_shed").copied().unwrap_or(0) >= 5, "seed {seed}: {counts:?}");
        assert!(counts.get("barbecue").copied().unwrap_or(0) >= 5, "seed {seed}: {counts:?}");
    }
}

/// Props never close a way: every outdoor tile reachable from the main street without them still
/// is with them (but the props' own tiles), so every lot and building can still be walked to.
#[test]
fn props_never_close_a_way() {
    for seed in 1..=3 {
        let with = town(seed);
        let without = generate(&GenConfig { seed, ..Default::default() }, &shipped_templates());
        let (a, b) = (reachable(&with), reachable(&without));
        let w = with.map.width();
        for (i, (&yes, &before)) in a.iter().zip(&b).enumerate() {
            let t = IVec2::new(i as i32 % w, i as i32 / w);
            if before && with.map.tile(t).feature != Feature::StreetProp {
                assert!(yes, "seed {seed}: {t} can't be reached with the street props");
            }
        }
    }
}

/// A blocking prop blocks: a body walking into it stops its radius away (the one movement function,
/// so the client's prediction too), the walk grid and straight-line checks say no.
#[test]
fn blocking_props_block_movement() {
    let g = town(1);
    let m = &g.map;
    let hydrant = m.street_props.iter().find(|p| kind_id(m, p) == "fire_hydrant").unwrap();
    let centre = hydrant.tile.as_vec2() + Vec2::splat(0.5);
    let a = along(hydrant.facing).as_vec2();
    // Walk along the kerb into it from 3 m away.
    let mut state = PlayerState::new(centre - a * 3.0);
    let input = PlayerInput { move_dir: a, facing: 0.0, run: false, sneak: false, aim: false };
    for _ in 0..60 {
        step_movement(m, &mut state, &input, crate::TICK_DT);
    }
    let r = m.street_prop_radius(hydrant.tile);
    assert!((r - 0.22).abs() < 1e-3, "radius {r}");
    assert!(state.pos.distance(centre) >= r + PLAYER_RADIUS - 1e-3, "walked into the hydrant: {} from it", state.pos.distance(centre));
    assert!(state.pos.distance(centre) < r + PLAYER_RADIUS + 0.6, "stopped short or went round: {}", state.pos.distance(centre));
    // Paths go round it.
    assert!(m.tile_blocked(hydrant.tile));
    assert!(!crate::vision::clear_path(m, centre - a * 3.0, centre + a * 3.0));
    let walk = crate::flow::WalkGrid::new(m);
    let field = crate::flow::FlowField::build(&walk, (centre + a * 3.0).floor().as_ivec2(), 40.0);
    assert!(field.distance(centre).is_none(), "a path through the hydrant");
    assert!(field.distance(centre - a * 3.0).is_some_and(|d| d > 6.0 - 1e-3), "no way round");
}

/// A prop of several tiles blocks all of them, whole: a body walking at a parked car stops at its
/// side wherever it meets it, and paths go round.
#[test]
fn big_props_block_every_tile_they_cover() {
    let g = town(1);
    let m = &g.map;
    let car = m.street_props.iter().find(|p| kind_id(m, p) == "car").unwrap();
    let (min, size) = m.street_prop_rect(car);
    assert_eq!(size.min_element(), 2);
    assert_eq!(size.max_element(), 5);
    let (lo, hi) = (min.as_vec2(), (min + size).as_vec2());
    for q in (0..size.y).flat_map(|dy| (0..size.x).map(move |dx| min + IVec2::new(dx, dy))) {
        assert!(m.tile_blocked(q) && m.street_prop_fills(q), "{q}");
    }
    // Walk at it across the road, toward each tile of its long side.
    let long = if size.x > size.y { Vec2::X } else { Vec2::Y };
    let across = long.perp().abs();
    for j in 0..5 {
        let aim = lo + long * (j as f32 + 0.5) + across * 1.0;
        for way in [-1.0, 1.0] {
            let from = aim + across * way * 3.0;
            if m.tile_blocked(from.floor().as_ivec2()) || m.tile(from.floor().as_ivec2()).room != 0 {
                continue;
            }
            let mut state = PlayerState::new(from);
            let input = PlayerInput { move_dir: -across * way, facing: 0.0, run: false, sneak: false, aim: false };
            for _ in 0..80 {
                step_movement(m, &mut state, &input, crate::TICK_DT);
            }
            let p = state.pos;
            let out = (lo - p).max(p - hi).max_element();
            assert!(out >= PLAYER_RADIUS - 1e-3, "walked into the car at {p}: {out} from its side");
        }
    }
    let walk = crate::flow::WalkGrid::new(m);
    let mid = (lo + hi) * 0.5;
    let field = crate::flow::FlowField::build(&walk, (mid + across * 3.0).floor().as_ivec2(), 40.0);
    assert!(field.distance(mid).is_none(), "a path through the car");
}

/// A world made with street props saves and loads with them (the map is regenerated), the same
/// state hash; one whose map is stored whole keeps them where its tiles still have them.
#[test]
fn props_come_back_with_a_loaded_world() {
    let content = WorldConfig::new(1, 50, shipped_templates()).with_street_props(shipped());
    let w = World::new(content.clone());
    assert!(!w.map.street_props.is_empty());
    let loaded = World::load(&w.save(), &content).unwrap();
    assert_eq!(loaded.map.street_props, w.map.street_props);
    assert_eq!(loaded.state_hash(), w.state_hash());
    // Without the props file, the save's templates no longer match: a changed map, refused unless forced.
    let bare = WorldConfig::new(1, 50, shipped_templates());
    assert!(matches!(World::load(&w.save(), &bare), Err(crate::save::LoadError::ContentChanged(_))));
    // A map stored whole: one prop's tile cleared by hand loses that prop, the rest stay.
    let mut w = World::new(content.clone());
    let gone = w.map.street_props[0].tile;
    w.map.tile_mut(gone).unwrap().feature = Feature::None;
    w.map_changed();
    let loaded = World::load(&w.save(), &content).unwrap();
    assert_eq!(loaded.map.street_props.len(), w.map.street_props.len() - 1);
    assert!(loaded.map.street_prop(gone).is_none());
    assert_eq!(loaded.map.street_props[..], w.map.street_props[1..]);
}

/// How long the town takes to generate with and without the street props
/// (`cargo test -p zombye-sim --release town_generation_time -- --ignored --nocapture`).
#[test]
#[ignore]
fn town_generation_time() {
    let templates = shipped_templates();
    let props = shipped();
    for (name, def) in [("without props", StreetPropsDef::default()), ("with props", props)] {
        let mut times = Vec::new();
        for seed in 1..=10 {
            let start = std::time::Instant::now();
            let g = generate_with(&GenConfig { seed, ..Default::default() }, &templates, &def);
            times.push(start.elapsed().as_secs_f64() * 1000.0);
            std::hint::black_box(g);
        }
        times.sort_by(f64::total_cmp);
        eprintln!("town generation {name}: mean {:.1} ms, median {:.1} ms", times.iter().sum::<f64>() / times.len() as f64, times[times.len() / 2]);
    }
    for (name, def) in [("without props", StreetPropsDef::default()), ("with props", shipped())] {
        let start = std::time::Instant::now();
        let w = World::new(WorldConfig::new(1, 0, templates.clone()).with_street_props(def));
        eprintln!("world (seed 1, no zombies) {name}: {:.1} ms", start.elapsed().as_secs_f64() * 1000.0);
        std::hint::black_box(w);
    }
}


