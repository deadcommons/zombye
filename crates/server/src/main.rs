//! `zombye-server`: headless authoritative server.
//!
//! Modes:
//! - `zombye-server --mcp` — MCP server on stdio. Time only advances when an agent calls
//!   `step_ticks`, so agent runs are deterministic and reproducible.
//! - `zombye-server --bench [--ticks N] [--players N] [--horde N]` — run N ticks and print timing.
//!   With no players every zombie heads for a debug goal (all of them stay on the active tier);
//!   with players they start on the road west of town (in the forest on a big map), run in
//!   circles and make a 60 m noise every 10 s, `--horde N` spawns N more zombies round the first,
//!   and the report breaks the tick down by tier.
//!
//! Common options: `--zombies N` (default: as many as `population.ron` gives for the map, about a
//! thousand on the default map), `--seed S` (default 1), `--map-chunks N` (a square
//! map N × N chunks of 16 m; default 20, so 320 m; 64 is about 1 km), `--data DIR` (content
//! folder, default the repo's `assets/data`).

mod mcp;

use std::path::PathBuf;
use std::time::Instant;

use zombye_net::content;
use glam::Vec2;
use zombye_sim::world::TickStats;
use zombye_sim::{PlayerInput, TICK_DT, World, WorldConfig};

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
    let population = match content::load_population(&data) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("failed to load content: {e}");
            std::process::exit(1);
        }
    };
    let mut config = WorldConfig::new(value("--seed").unwrap_or(1), 0, templates).with_weapons(weapons).with_population(population);
    // Without --zombies, as many as the population file's densities give for the map.
    config.zombies = value("--zombies").map(|n| n as u32);
    if let Some(chunks) = value("--map-chunks") {
        config = config.with_map_chunks(chunks.clamp(1, 256) as i32);
    }

    if flag("--mcp") {
        mcp::serve(World::new(config));
    } else if flag("--bench") {
        bench(config, value("--ticks").unwrap_or(1000), value("--players").unwrap_or(0) as u32, value("--horde").unwrap_or(0) as u32);
    } else {
        eprintln!("usage: zombye-server (--mcp | --bench [--ticks N] [--players N] [--horde N]) [--zombies N] [--seed S] [--map-chunks N] [--data DIR]");
        std::process::exit(2);
    }
}

fn bench(config: WorldConfig, ticks: u64, players: u32, horde: u32) {
    let zombies = config.zombies.map_or("natural".to_string(), |n| n.to_string());
    let start = Instant::now();
    let mut world = World::new(config);
    let gen_ms = start.elapsed().as_secs_f32() * 1000.0;
    let mut ids = Vec::new();
    if players == 0 {
        world.set_goal(Some(world.player_spawn));
    } else {
        let road_y = (world.town_min.y + world.town_max.y) as f32 / 2.0;
        for k in 0..players {
            let p = open_ground_near(&world, Vec2::new(world.town_min.x as f32 * 0.5 + k as f32 * 40.0, road_y));
            if k == 0 && horde > 0 {
                world.spawn_zombies(horde, p, 45.0);
            }
            let id = world.add_player(p);
            // Unkillable, so the run measures a horde on someone rather than a corpse.
            world.players.last_mut().expect("just added").hp = 1e9;
            ids.push(id);
        }
    }
    let start = Instant::now();
    let mut worst: f32 = 0.0;
    let mut sum = TickStats::default();
    for t in 0..ticks {
        for (k, &id) in ids.iter().enumerate() {
            let dir = Vec2::from_angle(t as f32 * 0.01 + k as f32);
            world.set_player_input(id, t as u32 + 1, PlayerInput { move_dir: dir, facing: 0.0, run: true, sneak: false });
            if t % 200 == 0 {
                world.emit_sound(world.player(id).expect("joined").state.pos, 60.0);
            }
        }
        world.step();
        let s = world.stats;
        worst = worst.max(s.tick_ms);
        sum.senses_ms += s.senses_ms;
        sum.flow_ms += s.flow_ms;
        sum.tier_ms += s.tier_ms;
        sum.active_ms += s.active_ms;
        sum.near_ms += s.near_ms;
        sum.virtual_ms += s.virtual_ms;
        sum.promoted += s.promoted;
        sum.demoted += s.demoted;
    }
    let total = start.elapsed().as_secs_f32();
    let n = ticks.max(1) as f32;
    let s = world.stats;
    println!(
        "map {}x{} m, {} buildings, generated in {gen_ms:.0} ms; {} zombies ({zombies} + horde {horde}), {} players, {ticks} ticks ({:.0} s of game time) in {:.2} s: \
         avg {:.3} ms/tick, worst {:.3} ms, budget {:.0} ms, hash {:016x}\n\
         tiers now: {} active, {} near, {} virtual ({} hunting); {} promotions, {} demotions\n\
         per tick: tiers+index {:.3} ms, senses {:.3} ms, flow {:.3} ms, active AI {:.3} ms, near {:.3} ms, virtual {:.3} ms\n\
         population: target {:.0} in {} regions, {} attractors now, {} respawned",
        world.map.width(),
        world.map.height(),
        world.map.buildings.len(),
        world.zombies.len(),
        players,
        ticks as f32 * TICK_DT,
        total,
        total * 1000.0 / n,
        worst,
        TICK_DT * 1000.0,
        world.state_hash(),
        s.active,
        s.near,
        s.virtual_,
        s.chasing,
        sum.promoted,
        sum.demoted,
        sum.tier_ms / n,
        sum.senses_ms / n,
        sum.flow_ms / n,
        sum.active_ms / n,
        sum.near_ms / n,
        sum.virtual_ms / n,
        world.population().total_target(),
        world.regions().regions(),
        world.population().attractors.len(),
        world.population().respawned,
    );
}

/// The nearest open outdoor tile to `p`.
fn open_ground_near(world: &World, p: Vec2) -> Vec2 {
    let c = p.floor().as_ivec2();
    for r in 0..60 {
        for dy in -r..=r {
            for dx in -r..=r {
                let t = c + glam::IVec2::new(dx, dy);
                if dx.abs().max(dy.abs()) == r && world.map.in_bounds(t) && !world.map.tile_blocked(t) && world.map.tile(t).room == 0 {
                    return t.as_vec2() + Vec2::splat(0.5);
                }
            }
        }
    }
    world.player_spawn
}
