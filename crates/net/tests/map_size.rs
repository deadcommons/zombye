//! How long a big map takes to build and how much memory it holds (Linear DCM-94). Ignored: run
//! with `ZOMBYE_MAP_CHUNKS=128,256 cargo test -p zombye-net --release --test map_size -- --ignored --nocapture`.
//! A joining client builds its map the same way (`remote::build_map`: `World::new` with no zombies).

use std::time::Instant;

use zombye_net::content;
use zombye_sim::map::{Feature, Tile};
use zombye_sim::{World, worldgen};

/// This process's resident memory in MB (Linux), or 0.
fn rss_mb() -> f64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| s.lines().find(|l| l.starts_with("VmRSS:")).and_then(|l| l.split_whitespace().nth(1)?.parse::<f64>().ok()))
        .map_or(0.0, |kb| kb / 1024.0)
}

#[test]
#[ignore]
fn map_size_report() {
    let sizes = std::env::var("ZOMBYE_MAP_CHUNKS").unwrap_or_else(|_| "128,256".into());
    let base = content::load_config(&content::default_data_dir(), 1).expect("content");
    for n in sizes.split(',').filter_map(|s| s.trim().parse::<i32>().ok()) {
        let mut config = base.clone().with_map_chunks(n);
        config.zombies = Some(0);
        let before = rss_mb();
        let t = Instant::now();
        let generated = worldgen::generate_full(&config.generation, &config.templates, &config.street_props, &config.biomes);
        let gen_s = t.elapsed().as_secs_f64();
        let map = &generated.map;
        let tiles = map.width() as u64 * map.height() as u64;
        let gen_rss = rss_mb() - before;
        let town = (generated.town_max - generated.town_min).max(glam::IVec2::ZERO);
        let town_share = (town.x as f64 * town.y as f64) / tiles as f64;
        let all = || (0..map.height()).flat_map(move |y| (0..map.width()).map(move |x| map.tile(glam::IVec2::new(x, y))));
        let trees = all().filter(|t| matches!(t.feature, Feature::Tree | Feature::PineTree)).count();
        let indoors = all().filter(|t| t.room != 0).count();
        println!(
            "{n} chunks ({} m a side, {tiles} tiles): worldgen {gen_s:.2} s, +{gen_rss:.0} MB ({:.1} bytes a tile; tiles alone {} MB at {} bytes); \
             town {}x{} m = {:.2}% of the map, the rest forest and wild ground; {trees} tree tiles, {indoors} indoor tiles, {} rooms, {} buildings",
            map.width(),
            gen_rss * 1048576.0 / tiles as f64,
            tiles * std::mem::size_of::<Tile>() as u64 / 1048576,
            std::mem::size_of::<Tile>(),
            town.x,
            town.y,
            town_share * 100.0,
            map.rooms.len(),
            map.buildings.len(),
        );
        drop(generated);
        let before = rss_mb();
        let t = Instant::now();
        let world = World::new(config);
        println!("{n} chunks: World::new (what a joining client's build_map does) {:.2} s, +{:.0} MB", t.elapsed().as_secs_f64(), rss_mb() - before);
        drop(world);
    }
}
