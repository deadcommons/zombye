//! Stairs and the upper floor (players only, for now).
//!
//! A two-storey building's `stairs` furniture is a [`Stair`](crate::map::Stair): a ramp from the
//! ground floor to the first upper floor that a player walks up and down
//! ([`TileMap::resolve_walker`](crate::map::TileMap::resolve_walker), from
//! [`step_movement`](crate::players::step_movement), so the client's prediction is exact). Which
//! floor a player is on is `PlayerState::level`; how high it stands follows from that and where it
//! is (`PlayerState::height`).
//!
//! What holds upstairs: the upper floor's own walls, windows and furniture stop the body; its
//! furniture are containers like any other ([`ContainerSite::level`](crate::ContainerSite)); things
//! dropped lie on that floor ([`World::item_level`](crate::World::item_level): the world keeps the
//! ids of the ground items upstairs, so the items themselves are as they were) and are reached
//! only from it; sleeping takes a bed up there; players on different floors don't see each other.
//! Saved in an optional `LEVL` section and in the state hash only while anyone or anything is
//! upstairs, so other worlds save and hash as before. What doesn't yet: zombies stay on the ground floor (they
//! neither see nor strike a player upstairs, or higher up a flight than
//! [`StairTuning::out_of_reach`](crate::players::StairTuning), but hear it as ever and gather
//! below); blows, shots and hands don't pass between floors; no gun is fired from upstairs; upstairs
//! doors are open doorways, and its windows can't be opened, climbed, smashed or given blinds.

#[cfg(test)]
mod tests {
    use crate::TICK_DT;
    use crate::map::{STOREY, Side, TileMap};
    use crate::players::{PLAYER_RADIUS, PlayerInput, PlayerState, step_movement};
    use crate::template::{BuildingTemplate, Placement};
    use glam::{IVec2, Vec2};

    const TWO_STOREY: [&str; 3] = [
        include_str!("../../../assets/data/buildings/family_two_storey.ron"),
        include_str!("../../../assets/data/buildings/townhouse.ron"),
        include_str!("../../../assets/data/buildings/office_block.ron"),
    ];

    fn placed(text: &str, turns: u8, mirror: bool) -> TileMap {
        let t = BuildingTemplate::from_ron(text).unwrap();
        let mut map = TileMap::new(4, 4);
        t.place(&mut map, Placement { origin: IVec2::new(20, 20), quarter_turns: turns, mirror }, 0, 0);
        map.index_storeys();
        map.stair_speed = 0.6;
        map
    }

    fn walk(map: &TileMap, s: &mut PlayerState, to: Vec2, secs: f32) -> bool {
        for _ in 0..(secs / TICK_DT) as u32 {
            let d = to - s.pos;
            if d.length() < 0.05 {
                return true;
            }
            let input = PlayerInput { move_dir: d.normalize(), facing: d.to_angle(), run: false, sneak: false, aim: false };
            step_movement(map, s, &input, TICK_DT);
        }
        false
    }

    /// Every shipped two-storey building, any way round: its stairs are a flight a player walks
    /// up from the tile at its foot onto the upper floor, rising all the way, and back down.
    #[test]
    fn every_shipped_staircase_is_walked_up_and_down() {
        for text in TWO_STOREY {
            for turns in 0..4 {
                for mirror in [false, true] {
                    let map = placed(text, turns, mirror);
                    let what = format!("{} turned {turns} mirrored {mirror}", map.buildings[0].template);
                    assert_eq!(map.stairs.len(), 1, "{what}: its stairs aren't a flight");
                    let st = map.stairs[0];
                    let (foot, top) = st.ends();
                    let dir = st.up.offset().as_vec2();
                    let mut s = PlayerState::new(st.foot().as_vec2() + Vec2::splat(0.5));
                    assert!(walk(&map, &mut s, foot - dir * 0.5, 3.0), "{what}: can't stand at the foot");
                    let mut last = 0.0;
                    let landing = st.landing().as_vec2() + Vec2::splat(0.5);
                    for _ in 0..400 {
                        let d = landing - s.pos;
                        if d.length() < 0.05 {
                            break;
                        }
                        step_movement(&map, &mut s, &PlayerInput { move_dir: d.normalize(), facing: 0.0, run: false, sneak: false, aim: false }, TICK_DT);
                        let h = s.height(&map);
                        assert!(h >= last - 1e-4, "{what}: went down on the way up ({last} to {h} at {})", s.pos);
                        last = h;
                    }
                    assert_eq!((s.level, s.height(&map)), (1, STOREY), "{what}: not upstairs at {}", s.pos);
                    assert!(s.pos.distance(landing) < 0.06 && st.progress(top) == 1.0, "{what}: stopped at {}", s.pos);
                    // And down again.
                    assert!(walk(&map, &mut s, foot - dir * 0.6, 12.0), "{what}: can't walk back down (at {} level {})", s.pos, s.level);
                    assert_eq!((s.level, s.height(&map)), (0, 0.0), "{what}");
                }
            }
        }
    }

    /// The flight is a ramp between walls: nobody gets on or off it over its sides, under it from
    /// the far end on the ground floor, or into the stairwell upstairs but through its top end.
    #[test]
    fn a_flight_is_entered_only_at_its_ends() {
        let map = placed(TWO_STOREY[0], 0, false);
        let st = map.stairs[0];
        assert_eq!(st.up, Side::N);
        let (lo, hi) = (st.min.as_vec2(), (st.min + st.size).as_vec2());
        let mid = (lo + hi) * 0.5;
        // From the side on the ground floor.
        let mut s = PlayerState::new(Vec2::new(lo.x - 0.6, mid.y));
        walk(&map, &mut s, mid, 3.0);
        assert!(s.pos.x < lo.x - PLAYER_RADIUS + 0.01 && s.level == 0, "walked into the side of the stairs: {}", s.pos);
        // From beyond the top end on the ground floor (under the upper floor).
        let mut s = PlayerState::new(Vec2::new(mid.x, lo.y - 0.6));
        walk(&map, &mut s, mid, 3.0);
        assert!(s.pos.y < lo.y - PLAYER_RADIUS + 0.01 && s.level == 0, "walked into the back of the stairs: {}", s.pos);
        // Upstairs, over the side of the well and over its foot end.
        for from in [Vec2::new(lo.x - 0.6, mid.y), Vec2::new(mid.x, hi.y + 0.6)] {
            let mut s = PlayerState { level: 1, ..PlayerState::new(from) };
            walk(&map, &mut s, mid, 3.0);
            assert!(s.level == 1 && !st.on(s.pos), "fell into the stairwell from {from}: {}", s.pos);
        }
        // On the flight, the sides hold too.
        let mut s = PlayerState::new(Vec2::new(mid.x, hi.y - 1.0));
        walk(&map, &mut s, Vec2::new(lo.x - 2.0, hi.y - 1.0), 3.0);
        assert!(st.on(s.pos), "left the flight over its side: {}", s.pos);
    }

    /// Upstairs the body stays on the floor and inside the upper walls, and walks through the
    /// upper floor's doorways.
    #[test]
    fn the_upper_floor_keeps_a_body_in_its_rooms() {
        let map = placed(TWO_STOREY[0], 0, false);
        let b = &map.buildings[0];
        let st = map.stairs[0];
        let start = st.landing().as_vec2() + Vec2::splat(0.5);
        // Out through every outside wall: stopped inside the footprint.
        // (Not back the way of the stairs: that is the way down.)
        for dir in [Vec2::X, Vec2::NEG_X, Vec2::Y, Vec2::NEG_Y].into_iter().filter(|d| *d != -st.up.offset().as_vec2()) {
            let mut s = PlayerState { level: 1, ..PlayerState::new(start) };
            for _ in 0..400 {
                step_movement(&map, &mut s, &PlayerInput { move_dir: dir, facing: 0.0, run: true, sneak: false, aim: false }, TICK_DT);
            }
            let (lo, hi) = (b.min.as_vec2(), (b.min + b.size).as_vec2());
            assert!(s.level == 1 && s.pos.cmpgt(lo + 0.3).all() && s.pos.cmplt(hi - 0.3).all(), "left the upper floor going {dir}: {}", s.pos);
            assert_ne!(map.room_on(1, s.pos), 0);
        }
        // Every upstairs room can be walked into from the landing (doors are open doorways).
        let mut rooms: Vec<crate::map::RoomId> = map.upper_tiles().filter(|(_, u)| u.floor).map(|(_, u)| u.room).collect();
        rooms.sort_unstable();
        rooms.dedup();
        assert!(rooms.len() >= 4, "{rooms:?}");
        let mut seen = std::collections::HashSet::from([st.landing()]);
        let mut queue = std::collections::VecDeque::from([st.landing()]);
        while let Some(t) = queue.pop_front() {
            for side in Side::ALL {
                let n = t + side.offset();
                let (owner, which) = side.owner(t);
                let edge = map.upper_tile(owner).map(|u| if matches!(which, crate::map::OwnedEdge::North) { u.north } else { u.west }).unwrap_or_default();
                let free = map.upper_tile(n).is_some_and(|u| u.floor && !u.blocked) && map.stair_on_tile(n).is_none();
                if free && !crate::map::upper_edge_blocks(edge) && seen.insert(n) {
                    queue.push_back(n);
                }
            }
        }
        for r in rooms {
            assert!(seen.iter().any(|t| map.upper_tile(*t).unwrap().room == r), "upstairs room {} can't be reached", map.rooms[r as usize].name);
        }
    }

    /// A world's player walks up the first staircase of the town. Returns the world, the player
    /// and the flight.
    fn upstairs_world() -> (crate::World, u32, crate::map::Stair) {
        let mut config = crate::save::tests::content();
        config.zombies = Some(0);
        let mut w = crate::World::new(config);
        let st = w.map.stairs.first().copied().expect("a staircase in town");
        let foot = st.foot().as_vec2() + Vec2::splat(0.5);
        let id = w.add_player(foot);
        let pi = w.players.iter().position(|p| p.id == id).unwrap();
        w.players[pi].state.pos = foot;
        let top = st.landing().as_vec2() + Vec2::splat(0.5);
        for t in 0..400 {
            let d = top - w.players[pi].state.pos;
            if w.players[pi].state.level == 1 && d.length() < 0.1 {
                break;
            }
            w.set_player_input(id, t + 1, PlayerInput { move_dir: d.normalize_or_zero(), facing: d.to_angle(), run: false, sneak: false, aim: false });
            w.step();
        }
        w.set_player_input(id, 1000, PlayerInput::default());
        assert_eq!(w.players[pi].state.level, 1, "never got upstairs: {:?}", w.players[pi].state.pos);
        (w, id, st)
    }

    #[test]
    fn zombies_lose_a_player_upstairs_and_blows_stay_on_their_floor() {
        let (mut w, id, st) = upstairs_world();
        w.step();
        assert!(w.targets.iter().find(|t| t.id == id).unwrap().ignored, "zombies still see a player upstairs");
        // Part-way up the flight it is in their sight again below `out_of_reach`.
        let pi = 0;
        let low = st.foot().as_vec2() + Vec2::splat(0.5) + st.up.offset().as_vec2() * 0.7;
        w.players[pi].state.level = 0;
        w.players[pi].state.pos = low;
        w.step();
        let h = w.players[pi].state.height(&w.map);
        assert!(h > 0.0 && h < w.config.stairs.out_of_reach, "height {h}");
        assert!(!w.targets[0].ignored);
        // Nothing on the ground floor is worked from the stairs or upstairs.
        assert!(w.player_interact(id).is_none() && !w.player_climb(id));
    }

    #[test]
    fn things_dropped_upstairs_stay_upstairs_and_survive_a_save() {
        use crate::inventory::Place;
        let (mut w, id, _) = upstairs_world();
        let rag = w.item_index("rag").unwrap();
        w.players[0].inventory.carried.push(crate::items::ItemStack::new(rag, 1));
        w.player_drop(id, Place::Carried, 0, 0).unwrap();
        for _ in 0..40 {
            w.step();
        }
        let it = *w.items.iter().find(|it| it.stack.item == rag).expect("the rag on the floor");
        assert_eq!(w.item_level(it.id), 1);
        assert!(w.floor_of(id).iter().any(|f| f.id == it.id), "not in reach of who dropped it");
        // Someone standing right under it doesn't reach it, nor see who is upstairs.
        let other = w.add_player(it.pos);
        let oi = w.players.iter().position(|p| p.id == other).unwrap();
        w.players[oi].state.pos = it.pos;
        assert!(w.floor_of(other).iter().all(|f| f.id != it.id));
        assert!(!w.sees_now(other, id) && !w.sees_now(id, other));
        w.players.remove(oi);

        let hash = w.state_hash();
        let mut loaded = crate::World::load(&w.save(), &crate::save::tests::content()).unwrap();
        assert_eq!(loaded.state_hash(), hash);
        assert_eq!(loaded.players[0].state.level, 1);
        assert_eq!(loaded.item_level(it.id), 1);
        for t in 0..20 {
            for x in [&mut w, &mut loaded] {
                x.set_player_input(id, 2000 + t, PlayerInput { move_dir: Vec2::X, facing: 0.0, run: false, sneak: false, aim: false });
                x.step();
            }
            assert_eq!(loaded.state_hash(), w.state_hash(), "tick {t}");
        }
        // Taken up again, nothing is left upstairs.
        w.players[0].state.pos = it.pos;
        assert!(w.player_pick_up(id).is_some());
        w.step();
        assert!(w.upstairs.is_empty());
        // A world with nobody upstairs hashes as it always did.
        w.players[0].state.level = 0;
        let flat = w.state_hash();
        w.players[0].state.level = 1;
        assert_ne!(w.state_hash(), flat);
    }

    #[test]
    fn upstairs_furniture_is_searched_from_upstairs_only() {
        let (mut w, id, st) = upstairs_world();
        let site = w.containers().iter().find(|s| s.level == 1 && s.building == st.building).cloned().expect("a container upstairs");
        assert!(w.containers().iter().any(|s| s.level == 0));
        // Beside it on its floor.
        let beside = (-1..=site.size.y)
            .flat_map(|y| (-1..=site.size.x).map(move |x| IVec2::new(x, y)))
            .filter(|o| (o.x < 0 || o.x >= site.size.x) != (o.y < 0 || o.y >= site.size.y))
            .map(|o| site.min + o)
            .find(|t| w.map.upper_tile(*t).is_some_and(|u| u.floor && !u.blocked && u.room == site.room))
            .expect("a free tile beside it");
        w.players[0].state.pos = beside.as_vec2() + Vec2::splat(0.5);
        assert!(w.containers_in_reach(id).contains(&site.id), "not reached from {beside} (site {:?})", site);
        w.players[0].state.level = 0;
        assert!(!w.containers_in_reach(id).contains(&site.id));
    }
}
