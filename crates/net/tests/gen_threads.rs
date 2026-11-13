//! Making a world on several threads (#19, `zombye_sim::par`) makes the same world as on one: the
//! map, its hash, the walk grid, the nav graph, the state hash, the save, and the same future.

use glam::Vec2;
use zombye_net::content;
use zombye_sim::flow::WalkGrid;
use zombye_sim::par::with_threads;
use zombye_sim::{PlayerInput, World, WorldConfig};

fn config() -> WorldConfig {
    let mut config = content::load_config(&content::default_data_dir(), 7).expect("the shipped content").with_map_chunks(64);
    config.loot_seed = Some(11);
    config
}

/// Step a world 100 ticks with two players walking about.
fn play(w: &mut World) {
    let ids = [w.add_player(w.player_spawn), w.add_player(w.player_spawn + Vec2::new(6.0, 0.0))];
    for t in 0..100u32 {
        for (k, &id) in ids.iter().enumerate() {
            let dir = Vec2::from_angle(t as f32 * 0.07 + k as f32 * 2.0);
            w.set_player_input(id, t + 1, PlayerInput { move_dir: dir, facing: dir.to_angle(), run: k == 0, sneak: false, aim: false });
        }
        w.step();
    }
}

#[test]
fn a_world_made_on_threads_is_the_world_made_on_one() {
    let one = with_threads(1, || World::new(config()));
    // The walk grid after the vehicles took their street props off the map (updated round each,
    // not built again) is the grid built afresh from the finished map.
    assert!(*one.walk() == WalkGrid::new(&one.map), "the walk grid updated round the vehicles is the whole grid built again");
    assert!(!one.vehicles().is_empty(), "the shipped content parks vehicles, so that was tested");
    let bytes = one.save();
    for n in [2, 4, 7] {
        let many = with_threads(n, || World::new(config()));
        assert_eq!(many.map_base_hash(), one.map_base_hash(), "{n} threads: map hash");
        assert!(many.walk() == one.walk(), "{n} threads: walk grid");
        assert!(many.nav().graph() == one.nav().graph(), "{n} threads: nav graph");
        assert_eq!(many.state_hash(), one.state_hash(), "{n} threads: state hash");
        assert!(many.save() == bytes, "{n} threads: save");
    }
    // The same future, and loading the save on any number of threads gives the same world.
    let (mut a, mut b) = (with_threads(1, || World::new(config())), with_threads(4, || World::new(config())));
    play(&mut a);
    play(&mut b);
    assert_eq!(a.state_hash(), b.state_hash(), "after 100 ticks");
    let saved = a.save();
    let content = config();
    let la = with_threads(1, || World::load(&saved, &content).expect("loads"));
    let lb = with_threads(4, || World::load(&saved, &content).expect("loads"));
    assert_eq!(la.state_hash(), a.state_hash(), "loaded on one thread");
    assert_eq!(lb.state_hash(), a.state_hash(), "loaded on four");
    assert!(la.walk() == lb.walk() && la.nav().graph() == lb.nav().graph(), "the loaded walk grid and nav graph");
    assert!(*la.walk() == WalkGrid::new(&la.map), "the loaded walk grid, updated round the vehicles, is the whole grid built again");
    assert!(la.save() == lb.save(), "saved again");
}
