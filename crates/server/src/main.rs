//! `zombye-server`: headless authoritative server.
//!
//! Modes:
//! - `zombye-server --mcp` — MCP server on stdio. Time only advances when an agent calls
//!   `step_ticks`, so agent runs are deterministic and reproducible.
//! - `zombye-server --bench [--ticks N]` — run N ticks and print timing.
//!
//! Common options: `--zombies N` (default 1000), `--seed S` (default 1), `--data DIR` (content
//! folder, default the repo's `assets/data`).

mod mcp;

use std::path::PathBuf;
use std::time::Instant;

use zombye_net::content;
use zombye_sim::{TICK_DT, World, WorldConfig};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |name: &str| args.iter().any(|a| a == name);
    let text = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let value = |name: &str| text(name).and_then(|v| v.parse::<u64>().ok());

    let data = text("--data").map(PathBuf::from).unwrap_or_else(content::default_data_dir);
    let templates = match content::load_building_templates(&data) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("failed to load content: {e}");
            std::process::exit(1);
        }
    };
    let weapons = match content::load_weapons(&data) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("failed to load content: {e}");
            std::process::exit(1);
        }
    };
    let config = WorldConfig::new(value("--seed").unwrap_or(1), value("--zombies").unwrap_or(1000) as u32, templates).with_weapons(weapons);

    if flag("--mcp") {
        mcp::serve(World::new(config));
    } else if flag("--bench") {
        bench(config, value("--ticks").unwrap_or(1000));
    } else {
        eprintln!("usage: zombye-server (--mcp | --bench [--ticks N]) [--zombies N] [--seed S] [--data DIR]");
        std::process::exit(2);
    }
}

fn bench(config: WorldConfig, ticks: u64) {
    let zombies = config.zombies;
    let start = Instant::now();
    let mut world = World::new(config);
    let gen_ms = start.elapsed().as_secs_f32() * 1000.0;
    world.set_goal(Some(world.player_spawn));
    let start = Instant::now();
    let mut worst: f32 = 0.0;
    for _ in 0..ticks {
        world.step();
        worst = worst.max(world.stats.tick_ms);
    }
    let total = start.elapsed().as_secs_f32();
    println!(
        "map {}x{} m, {} buildings, generated in {gen_ms:.0} ms; {zombies} zombies, {ticks} ticks ({:.0} s of game time) in {:.2} s: \
         avg {:.3} ms/tick, worst {:.3} ms, budget {:.0} ms, hash {:016x}",
        world.map.width(),
        world.map.height(),
        world.map.buildings.len(),
        ticks as f32 * TICK_DT,
        total,
        total * 1000.0 / ticks as f32,
        worst,
        TICK_DT * 1000.0,
        world.state_hash()
    );
}
