//! The stress test's script (`--bench-live N`, `--crowd N`): a crowd round the player at the start,
//! and the player walking a fixed loop (a 24 m square from where they start, round and round) so the
//! camera, the streaming and the crowd keep moving. Everything goes through the normal intents and
//! input, single-player or on a server (which must allow dev intents for the crowd: `--allow-dev`).

use bevy::prelude::*;
use zombye_net::Intent;

use crate::Options;
use crate::autoplay::AutoInput;
use crate::player::{LocalPlayer, PlayerInputSet};
use crate::server_link::ServerLink;

/// The loop's side (metres) and how close counts as reaching a corner.
const SIDE: f32 = 24.0;
const REACHED: f32 = 1.5;

pub struct BenchLivePlugin;

impl Plugin for BenchLivePlugin {
    fn build(&self, app: &mut App) {
        let options = app.world().resource::<Options>().clone();
        if options.crowd.is_some() {
            app.add_systems(Update, spawn_crowd);
        }
        if options.bench_live.is_some() {
            app.add_systems(FixedUpdate, walk.before(PlayerInputSet)).add_systems(Last, count_zombies);
        }
    }
}

/// A crowd round the player, once: spread over a disc big enough for them (about a zombie a square
/// metre), and a noise so they come in.
fn spawn_crowd(options: Res<Options>, mut link: ResMut<ServerLink>, me: Option<Res<LocalPlayer>>, mut done: Local<bool>) {
    let (Some(n), Some(me)) = (options.crowd, me) else { return };
    if *done || n == 0 {
        return;
    }
    *done = true;
    let at = me.curr.pos;
    let radius = (n as f32 / std::f32::consts::PI).sqrt() + 6.0;
    link.send(Intent::SpawnZombies { count: n, center: at.to_array(), radius });
    link.send(Intent::MakeNoise { pos: at.to_array(), radius: radius + 20.0, lasting: false });
    println!("--crowd: {n} zombies within {radius:.0} m");
}

/// Walk the loop: toward the next corner of a square from where the player started.
fn walk(me: Res<LocalPlayer>, mut auto: ResMut<AutoInput>, mut state: Local<Option<(Vec2, usize)>>) {
    let pos = me.curr.pos;
    let (start, corner) = state.get_or_insert((pos, 1));
    let corners = [Vec2::ZERO, Vec2::new(SIDE, 0.0), Vec2::new(SIDE, SIDE), Vec2::new(0.0, SIDE)];
    let target = *start + corners[*corner % 4];
    if pos.distance(target) < REACHED {
        *corner += 1;
    }
    auto.active = true;
    auto.run = false;
    auto.move_dir = (*start + corners[*corner % 4] - pos).normalize_or_zero();
}

/// At the end of the bench (and every 150 frames): how many zombies the client was sent and how many
/// the world has in each tier (single-player only), so a run says what it measured.
#[allow(clippy::type_complexity)]
fn count_zombies(
    link: Res<ServerLink>,
    mut exit: MessageReader<AppExit>,
    mut frame: Local<u32>,
    moved: Query<(), (With<Mesh3d>, Changed<GlobalTransform>)>,
    meshes: Query<(), Changed<Mesh3d>>,
    tags: Query<(), Changed<bevy::mesh::MeshTag>>,
    seen: Query<&ViewVisibility, (With<Mesh3d>, Changed<ViewVisibility>)>,
    all: Query<&ViewVisibility, With<Mesh3d>>,
    ui: (Query<(), Changed<Node>>, Query<(), Changed<Text>>, Query<(), Changed<ComputedNode>>, Query<(), With<Node>>),
) {
    *frame += 1;
    let leaving = exit.read().next().is_some();
    if !leaving && *frame % 150 != 0 {
        return;
    }
    // What the engine has to take up again this frame: each of these is extracted and uploaded anew.
    println!(
        "churn this frame: {} meshes moved, {} changed mesh or material, {} changed tag, {} came into or went out of view; {} of {} mesh entities in view",
        moved.iter().count(),
        meshes.iter().count(),
        tags.iter().count(),
        seen.iter().count(),
        all.iter().filter(|v| v.get()).count(),
        all.iter().count()
    );
    println!("churn this frame, UI: {} nodes changed, {} texts, {} laid out anew, of {} nodes", ui.0.iter().count(), ui.1.iter().count(), ui.2.iter().count(), ui.3.iter().count());
    let sent = link.curr.zombies.len();
    match link.world() {
        Some(w) => {
            let [active, near, far] = w.tier_counts();
            println!("zombies: {sent} sent to the client; world {} (active {active}, near {near}, virtual {far})", w.zombies.len());
        }
        None => println!("zombies: {sent} sent to the client"),
    }
}
