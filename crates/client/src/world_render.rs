//! Streams the world around the player into meshes: ground and trees per 16 m chunk, and one set
//! of meshes per building (ground floor, upper floor, roof) plus an entity per door leaf and per
//! window pane, which follow the map's edge changes (doors swing, sash windows slide up, broken
//! glass leaves jagged shards, a broken door's leaf is gone). Battered doors and windows show
//! cracks (and splinters) in three stages as their hit points run down (`EdgeDamage` in snapshots).
//! The upper floor and roof of the building you're in are hidden so you can see inside.

use std::collections::HashMap;
use std::f32::consts::FRAC_PI_2;

use bevy::prelude::*;
use zombye_sim::map::{Building, CHUNK_SIZE, Edge, Feature, OwnedEdge, RoofStyle, TileMap};

use crate::Options;
use crate::camera::IsoCamera;
use crate::materials::{self, WorldMaterials};
use crate::meshgen::{self, CutKind, CutTag, MeshBuilder, add_blob, add_cone, add_furniture, add_prism, floor_color, hash01, rgb, shade};
use crate::player::Player;
use crate::server_link::{ClientMap, EdgeChangesThisFrame, EdgeDamageMap, EdgeDamageThisFrame};

/// Storey height and wall thickness, in metres.
pub const STOREY: f32 = 3.0;
const WALL_T: f32 = 0.08;
/// Height a cut-away wall drops to; must match `STUB_HEIGHT` in world.wesl.
const STUB_HEIGHT: f32 = 0.4;
/// Chunks within this many chunks of the player are loaded.
const LOAD_RADIUS: i32 = 6;
const MAX_LOADS_PER_FRAME: usize = 6;

#[derive(Default)]
struct BuildingRender {
    lower: Vec<Entity>,
    /// Upper floors and roof: hidden while the player is inside.
    upper: Vec<Entity>,
    /// Ground-floor doors and windows, keyed in `Streamer::edges`.
    edge_keys: Vec<(IVec2, OwnedEdge)>,
}

#[derive(Resource, Default)]
struct Streamer {
    chunks: HashMap<IVec2, Vec<Entity>>,
    buildings: HashMap<usize, BuildingRender>,
    /// Door leaf or window pane entity per ground-floor edge.
    edges: HashMap<(IVec2, OwnedEdge), Entity>,
}

#[derive(Resource)]
struct SharedMeshes {
    door: Handle<Mesh>,
    pane: Handle<Mesh>,
    shards: Handle<Mesh>,
    /// Damage overlays per stage (see [`crack_stage`]), in the leaf's and the pane's own frames.
    door_cracks: [Handle<Mesh>; CRACK_STAGES],
    pane_cracks: [Handle<Mesh>; CRACK_STAGES],
}

/// The crack overlay drawn on a door leaf or window pane (a child entity, so it swings and slides
/// with it).
#[derive(Component)]
struct Cracks {
    overlay: Entity,
    door: bool,
}

const CRACK_STAGES: usize = 3;

/// Which crack overlay to show at this fraction of hit points left (`None`: barely scratched).
fn crack_stage(hp: f32) -> Option<usize> {
    match hp {
        h if h >= 0.85 => None,
        h if h >= 0.55 => Some(0),
        h if h >= 0.3 => Some(1),
        _ => Some(2),
    }
}

/// A door leaf or window pane drawn as its own entity. Hidden when its wall is cut away (like the
/// walls in the shader) or when `shown` is off (a broken door's leaf).
#[derive(Component)]
struct EdgePiece {
    /// Midpoint of its edge, for the cutaway rule.
    mid: Vec2,
    shown: bool,
}

/// A door leaf, swinging toward `open`.
#[derive(Component)]
struct DoorLeaf {
    closed: Quat,
    open_rot: Quat,
    open: bool,
}

/// A sash window's glass: slides up when open, swaps to shards when broken.
#[derive(Component)]
struct WindowPane {
    base: f32,
    open: bool,
    broken: bool,
}

/// How far an open sash slides up.
const SASH_LIFT: f32 = 0.6;

pub struct WorldRenderPlugin;

impl Plugin for WorldRenderPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Streamer>()
            .add_systems(Startup, setup_shared)
            .add_systems(Update, (stream, apply_edge_changes, apply_edge_damage, animate_doors, animate_panes, hide_upper_floors, cut_edge_pieces).chain());
    }
}

fn setup_shared(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>) {
    // Door leaf with its hinge at the origin, lying along +x.
    let mut mb = MeshBuilder::default();
    mb.set_tag(CutTag { kind: CutKind::Structure, ..default() });
    let c = rgb(0.42, 0.28, 0.18);
    mb.add_box(Vec3::new(0.04, 0.0, -0.025), Vec3::new(0.96, 2.05, 0.025), meshgen::same(c));
    mb.add_box(Vec3::new(0.8, 1.0, -0.05), Vec3::new(0.86, 1.06, 0.05), meshgen::same(rgb(0.75, 0.7, 0.4)));
    let door = meshes.add(mb.build());

    // Window glass along +x from the edge's start, filling the frame between sill and header.
    // Structure, like door leaves: dimmed but readable in unseen rooms, cut away with its wall.
    let glass = LinearRgba::new(0.55, 0.68, 0.78, 0.32);
    let mut mb = MeshBuilder::default();
    mb.set_tag(CutTag { kind: CutKind::Structure, ..default() });
    mb.add_box(Vec3::new(0.05, WINDOW_SILL, -0.012), Vec3::new(0.95, WINDOW_HEAD, 0.012), meshgen::same(glass));
    let pane = meshes.add(mb.build());
    // What's left of a smashed pane: jagged teeth around the frame.
    let mut mb = MeshBuilder::default();
    mb.set_tag(CutTag { kind: CutKind::Structure, ..default() });
    let (lo, hi) = (WINDOW_SILL + 0.02, WINDOW_HEAD - 0.02);
    let teeth: [[(f32, f32); 3]; 7] = [
        [(0.06, lo), (0.32, lo), (0.06, lo + 0.38)],
        [(0.32, lo), (0.47, lo), (0.40, lo + 0.16)],
        [(0.94, lo), (0.94, lo + 0.52), (0.66, lo)],
        [(0.06, hi), (0.06, hi - 0.30), (0.44, hi)],
        [(0.94, hi), (0.58, hi), (0.86, hi - 0.42)],
        [(0.06, 1.35), (0.06, 1.62), (0.17, 1.47)],
        [(0.94, 1.30), (0.82, 1.42), (0.94, 1.58)],
    ];
    for t in teeth {
        mb.triangle(t.map(|(x, y)| Vec3::new(x, y, 0.0)), glass);
    }
    let shards = meshes.add(mb.build());
    let door_cracks = std::array::from_fn(|k| meshes.add(crack_mesh(true, k)));
    let pane_cracks = std::array::from_fn(|k| meshes.add(crack_mesh(false, k)));
    commands.insert_resource(SharedMeshes { door, pane, shards, door_cracks, pane_cracks });
}

/// Grey-box damage for a door leaf (dark cracks, and pale splintered wood when it's nearly gone)
/// or a window pane (white cracks radiating from where it was struck), drawn just off both faces.
/// Each stage adds to the one before.
fn crack_mesh(door: bool, stage: usize) -> Mesh {
    let mut mb = MeshBuilder::default();
    mb.set_tag(CutTag { kind: CutKind::Structure, ..default() });
    // A tiny fixed xorshift, so the cracks look the same every time.
    let mut seed = if door { 0x2545_F491u32 } else { 0x9E37_79B9u32 };
    let mut rand = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        (seed >> 8) as f32 / (1u32 << 24) as f32
    };
    let (lo, hi, face, width, color) = if door {
        (Vec2::new(0.1, 0.12), Vec2::new(0.9, 1.95), 0.03, 0.022, rgb(0.08, 0.05, 0.03))
    } else {
        (Vec2::new(0.07, WINDOW_SILL + 0.03), Vec2::new(0.93, WINDOW_HEAD - 0.03), 0.016, 0.013, rgb(0.88, 0.92, 0.95))
    };
    let line = |mb: &mut MeshBuilder, a: Vec2, b: Vec2, w: f32, color: LinearRgba| {
        let n = (b - a).perp().normalize_or_zero() * w * 0.5;
        for z in [face, -face] {
            let v = |p: Vec2| Vec3::new(p.x, p.y, z);
            mb.quad([v(a - n), v(b - n), v(b + n), v(a + n)], Vec3::Z * z.signum(), color);
        }
    };
    // Every stage draws the earlier stages' cracks first (same random sequence), then more.
    let per_stage = if door { [3, 4, 5] } else { [1, 1, 1] };
    for (k, &count) in per_stage.iter().enumerate().take(stage + 1) {
        for _ in 0..count {
            let start = lo + (hi - lo) * Vec2::new(rand(), rand());
            if door {
                // A jagged split along the grain (mostly up and down), a few segments long.
                let mut p = start;
                let mut dir = Vec2::new(rand() - 0.5, if rand() < 0.5 { 1.0 } else { -1.0 }).normalize();
                for _ in 0..5 {
                    let q = (p + dir * (0.12 + 0.16 * rand())).clamp(lo, hi);
                    line(&mut mb, p, q, width * (0.6 + 0.6 * rand()), color);
                    p = q;
                    dir = (dir + Vec2::new(rand() - 0.5, rand() - 0.5) * 0.8).normalize_or(Vec2::Y);
                }
            } else {
                // A star of cracks from the point of impact, and a ring around it.
                let rays = 6 + k * 2;
                let mut ring = Vec::new();
                for r in 0..rays {
                    let a = (r as f32 + rand() * 0.6) / rays as f32 * std::f32::consts::TAU;
                    let mut p = start;
                    let mut dir = Vec2::from_angle(a);
                    for seg in 0..3 {
                        let q = (p + dir * (0.08 + 0.12 * rand())).clamp(lo, hi);
                        line(&mut mb, p, q, width, color);
                        if seg == 0 {
                            ring.push(q);
                        }
                        p = q;
                        dir = (dir + Vec2::new(rand() - 0.5, rand() - 0.5) * 0.6).normalize_or(dir);
                    }
                }
                for r in 0..ring.len() {
                    line(&mut mb, ring[r], ring[(r + 1) % ring.len()], width * 0.8, color);
                }
            }
        }
        if door && k == 2 {
            // Nearly through: pale splintered wood showing where chunks came off.
            let pale = rgb(0.66, 0.5, 0.32);
            for _ in 0..6 {
                let c = lo + (hi - lo) * Vec2::new(0.2 + 0.6 * rand(), 0.25 + 0.5 * rand());
                let pts = [c + Vec2::new(-0.07, -0.03), c + Vec2::new(0.06 + 0.04 * rand(), -0.05), c + Vec2::new(0.01, 0.12 + 0.08 * rand())];
                for z in [face + 0.002, -face - 0.002] {
                    mb.triangle(pts.map(|p| Vec3::new(p.x, p.y, z)), pale);
                }
            }
        }
    }
    mb.build()
}

/// Bottom and top of a window opening above its storey's floor.
const WINDOW_SILL: f32 = 0.9;
const WINDOW_HEAD: f32 = 2.1;

#[allow(clippy::too_many_arguments)]
fn stream(
    mut commands: Commands,
    map: Res<ClientMap>,
    mats: Option<Res<WorldMaterials>>,
    shared: Option<Res<SharedMeshes>>,
    mut streamer: ResMut<Streamer>,
    mut meshes: ResMut<Assets<Mesh>>,
    damage: Res<EdgeDamageMap>,
    options: Res<Options>,
    player: Query<&Transform, With<Player>>,
    cams: Query<&IsoCamera>,
) {
    let (Some(mats), Some(shared), Ok(p)) = (mats, shared, player.single()) else { return };
    let map = &map.0;
    // Around the player; an offscreen render centred elsewhere (`--center`) loads around that.
    let at = match cams.single() {
        Ok(cam) if options.offscreen() => cam.focus,
        _ => p.translation,
    };
    let here = IVec2::new(at.x as i32, at.z as i32).div_euclid(IVec2::splat(CHUNK_SIZE));
    let first_load = streamer.chunks.is_empty();
    let budget = if first_load { usize::MAX } else { MAX_LOADS_PER_FRAME };

    // Chunks: load the nearest missing ones first.
    let mut wanted: Vec<IVec2> = Vec::new();
    for cy in here.y - LOAD_RADIUS..=here.y + LOAD_RADIUS {
        for cx in here.x - LOAD_RADIUS..=here.x + LOAD_RADIUS {
            let c = IVec2::new(cx, cy);
            if map.chunk(cx, cy).is_some() && !streamer.chunks.contains_key(&c) {
                wanted.push(c);
            }
        }
    }
    wanted.sort_by_key(|c| (*c - here).length_squared());
    for c in wanted.into_iter().take(budget) {
        let entities = spawn_chunk(&mut commands, &mut meshes, &mats, map, c);
        streamer.chunks.insert(c, entities);
    }
    let far: Vec<IVec2> = streamer.chunks.keys().copied().filter(|c| (*c - here).abs().max_element() > LOAD_RADIUS + 1).collect();
    for c in far {
        for e in streamer.chunks.remove(&c).unwrap() {
            commands.entity(e).despawn();
        }
    }

    // Buildings, anchored to the chunk of their centre.
    let mut loads = 0;
    for (i, b) in map.buildings.iter().enumerate() {
        let anchor = (b.min + b.size / 2).div_euclid(IVec2::splat(CHUNK_SIZE));
        let dist = (anchor - here).abs().max_element();
        let loaded = streamer.buildings.contains_key(&i);
        if dist <= LOAD_RADIUS && !loaded && loads < budget {
            loads += 1;
            let render = spawn_building(&mut commands, &mut meshes, &mats, &shared, map, i, b, &mut streamer.edges, &damage);
            streamer.buildings.insert(i, render);
        } else if dist > LOAD_RADIUS + 1 && loaded {
            let render = streamer.buildings.remove(&i).unwrap();
            for e in render.lower.into_iter().chain(render.upper) {
                commands.entity(e).despawn();
            }
            for key in render.edge_keys {
                streamer.edges.remove(&key);
            }
        }
    }
}

fn spawn_chunk(commands: &mut Commands, meshes: &mut Assets<Mesh>, mats: &WorldMaterials, map: &TileMap, c: IVec2) -> Vec<Entity> {
    let mut ground = MeshBuilder::default();
    let mut props = MeshBuilder::default();
    let min = c * CHUNK_SIZE;
    for y in min.y..min.y + CHUNK_SIZE {
        for x in min.x..min.x + CHUNK_SIZE {
            let t = IVec2::new(x, y);
            let tile = map.tile(t);
            let (x0, z0) = (x as f32, y as f32);
            ground.quad(
                [Vec3::new(x0, 0.0, z0), Vec3::new(x0, 0.0, z0 + 1.0), Vec3::new(x0 + 1.0, 0.0, z0 + 1.0), Vec3::new(x0 + 1.0, 0.0, z0)],
                Vec3::Y,
                floor_color(tile.floor, x, y),
            );
            let jitter = Vec3::new(hash01(x, y, 3) - 0.5, 0.0, hash01(x, y, 4) - 0.5) * 0.3;
            let centre = Vec3::new(x0 + 0.5, 0.0, z0 + 0.5) + jitter;
            let s = 0.8 + 0.5 * hash01(x, y, 5);
            match tile.feature {
                Feature::Tree => {
                    props.set_tag(CutTag::default());
                    add_prism(&mut props, centre, 0.16 * s, 2.4 * s, 6, rgb(0.30, 0.22, 0.16));
                    props.set_tag(CutTag { kind: CutKind::Canopy, anchor: centre.xz(), radius: 1.6 * s });
                    let leaf = [rgb(0.22, 0.36, 0.16), rgb(0.30, 0.40, 0.18), rgb(0.36, 0.38, 0.16)][(hash01(x, y, 6) * 3.0) as usize % 3];
                    add_blob(&mut props, centre + Vec3::Y * 2.9 * s, Vec3::new(1.5, 1.25, 1.5) * s, leaf, (x * 31 + y) as u32);
                }
                Feature::PineTree => {
                    props.set_tag(CutTag::default());
                    add_prism(&mut props, centre, 0.14 * s, 1.2 * s, 5, rgb(0.28, 0.20, 0.15));
                    props.set_tag(CutTag { kind: CutKind::Canopy, anchor: centre.xz(), radius: 1.3 * s });
                    let green = rgb(0.14, 0.26, 0.16);
                    add_cone(&mut props, centre + Vec3::Y * 0.9 * s, 1.3 * s, 2.4 * s, 7, green);
                    add_cone(&mut props, centre + Vec3::Y * 2.2 * s, 1.0 * s, 2.0 * s, 7, shade(green, 1.1));
                    add_cone(&mut props, centre + Vec3::Y * 3.4 * s, 0.7 * s, 1.6 * s, 7, shade(green, 1.2));
                }
                Feature::Bush => {
                    props.set_tag(CutTag::default());
                    add_blob(&mut props, centre + Vec3::Y * 0.4, Vec3::new(0.6, 0.45, 0.6) * s, rgb(0.24, 0.34, 0.17), (x * 17 + y * 5) as u32);
                }
                _ => {}
            }
            // Fences on the edges this tile owns.
            for (which, kind) in [(OwnedEdge::North, tile.north), (OwnedEdge::West, tile.west)] {
                if kind == Edge::Fence {
                    props.set_tag(CutTag::default());
                    add_fence(&mut props, t, which);
                }
            }
        }
    }
    let mut out = vec![commands.spawn((Mesh3d(meshes.add(ground.build())), MeshMaterial3d(mats.ground.clone()), Transform::IDENTITY)).id()];
    if !props.is_empty() {
        out.push(commands.spawn((Mesh3d(meshes.add(props.build())), MeshMaterial3d(mats.building.clone()), Transform::IDENTITY)).id());
    }
    out
}

fn add_fence(mb: &mut MeshBuilder, t: IVec2, which: OwnedEdge) {
    let (a, b) = TileMap::edge_segment(t, which);
    let wood = rgb(0.55, 0.48, 0.38);
    let along = (b - a).normalize();
    let half = Vec2::new(along.y.abs(), along.x.abs()) * 0.03;
    for (h0, h1) in [(0.35, 0.45), (0.8, 0.9)] {
        let lo = a.min(b) - half;
        let hi = a.max(b) + half;
        mb.add_box(Vec3::new(lo.x, h0, lo.y), Vec3::new(hi.x, h1, hi.y), meshgen::same(wood));
    }
    mb.add_box(Vec3::new(a.x - 0.05, 0.0, a.y - 0.05), Vec3::new(a.x + 0.05, 1.05, a.y + 0.05), meshgen::same(shade(wood, 0.8)));
}

/// Colour of one side of a wall: the room's paint inside, the building's siding outside.
fn wall_side_color(map: &TileMap, room: u16, siding: LinearRgba) -> LinearRgba {
    if room == 0 { siding } else { meshgen::interior_color(&map.rooms[room as usize].room_type) }
}

/// Wall pieces for one edge on one level. `rooms` = (north/west side, south/east side).
fn add_wall(mb: &mut MeshBuilder, map: &TileMap, owner: IVec2, which: OwnedEdge, kind: Edge, base: f32, rooms: (u16, u16), siding: LinearRgba) {
    let (a, b) = TileMap::edge_segment(owner, which);
    mb.set_tag(CutTag { kind: CutKind::Wall, anchor: (a + b) * 0.5, radius: 0.0 });
    let (c0, c1) = (wall_side_color(map, rooms.0, siding), wall_side_color(map, rooms.1, siding));
    let cap = shade(siding, 0.75);
    let piece = |mb: &mut MeshBuilder, h0: f32, h1: f32| {
        let (min, max, faces) = match which {
            // Faces: +x, -x, +y, -y, +z, -z. North edges run along x; their -z face looks north.
            OwnedEdge::North => (Vec3::new(a.x - WALL_T, base + h0, a.y - WALL_T), Vec3::new(b.x + WALL_T, base + h1, a.y + WALL_T), [cap, cap, cap, cap, c1, c0]),
            OwnedEdge::West => (Vec3::new(a.x - WALL_T, base + h0, a.y - WALL_T), Vec3::new(a.x + WALL_T, base + h1, b.y + WALL_T), [c1, c0, cap, cap, cap, cap]),
        };
        mb.add_box(min, max, faces);
        // A cap at stub height, shown only when the wall is cut, so the stub looks solid.
        if h0 < STUB_HEIGHT && h1 > STUB_HEIGHT {
            let y = base + STUB_HEIGHT;
            mb.set_tag(CutTag { kind: CutKind::WallCap, anchor: (a + b) * 0.5, radius: 0.0 });
            mb.quad([Vec3::new(min.x, y, min.z), Vec3::new(min.x, y, max.z), Vec3::new(max.x, y, max.z), Vec3::new(max.x, y, min.z)], Vec3::Y, shade(siding, 0.6));
            mb.set_tag(CutTag { kind: CutKind::Wall, anchor: (a + b) * 0.5, radius: 0.0 });
        }
    };
    match kind {
        Edge::Wall => piece(mb, 0.0, STOREY),
        Edge::Window | Edge::WindowOpen | Edge::WindowBroken => {
            piece(mb, 0.0, WINDOW_SILL);
            piece(mb, WINDOW_HEAD, STOREY);
            // Frame and a centre mullion.
            let mid = (a + b) * 0.5;
            let frame = rgb(0.85, 0.84, 0.80);
            let (lo, hi) = (mid - Vec2::splat(0.03), mid + Vec2::splat(0.03));
            mb.add_box(Vec3::new(lo.x, base + WINDOW_SILL, lo.y), Vec3::new(hi.x, base + WINDOW_HEAD, hi.y), meshgen::same(frame));
        }
        Edge::DoorClosed | Edge::DoorOpen | Edge::DoorBroken => piece(mb, 2.1, STOREY),
        Edge::Arch => piece(mb, 2.4, STOREY),
        Edge::None | Edge::Fence => {}
    }
    mb.set_tag(CutTag::default());
}

fn add_roof(mb: &mut MeshBuilder, b: &Building, siding: LinearRgba) {
    let color = meshgen::roof_color(b.roof_color);
    let eave = STOREY * b.levels as f32;
    let o = 0.4;
    let (x0, z0) = (b.min.x as f32, b.min.y as f32);
    let (x1, z1) = (x0 + b.size.x as f32, z0 + b.size.y as f32);
    let pitch = 0.5;
    let centre = Vec2::new((x0 + x1) * 0.5, (z0 + z1) * 0.5);
    mb.set_tag(CutTag { kind: CutKind::Roof, anchor: centre, radius: (x1 - x0).max(z1 - z0) * 0.5 + o });
    match b.roof {
        RoofStyle::GableX => {
            let zm = (z0 + z1) * 0.5;
            let ridge = eave + (zm - z0 + o) * pitch;
            mb.quad([Vec3::new(x0 - o, eave, z0 - o), Vec3::new(x0 - o, ridge, zm), Vec3::new(x1 + o, ridge, zm), Vec3::new(x1 + o, eave, z0 - o)], Vec3::new(0.0, 1.0, -pitch).normalize(), color);
            mb.quad([Vec3::new(x1 + o, eave, z1 + o), Vec3::new(x1 + o, ridge, zm), Vec3::new(x0 - o, ridge, zm), Vec3::new(x0 - o, eave, z1 + o)], Vec3::new(0.0, 1.0, pitch).normalize(), shade(color, 0.85));
            let gable = eave + (zm - z0) * pitch;
            mb.triangle([Vec3::new(x0, eave, z0), Vec3::new(x0, eave, z1), Vec3::new(x0, gable, zm)], siding);
            mb.triangle([Vec3::new(x1, eave, z1), Vec3::new(x1, eave, z0), Vec3::new(x1, gable, zm)], siding);
        }
        RoofStyle::GableY => {
            let xm = (x0 + x1) * 0.5;
            let ridge = eave + (xm - x0 + o) * pitch;
            mb.quad([Vec3::new(x0 - o, eave, z1 + o), Vec3::new(xm, ridge, z1 + o), Vec3::new(xm, ridge, z0 - o), Vec3::new(x0 - o, eave, z0 - o)], Vec3::new(-pitch, 1.0, 0.0).normalize(), color);
            mb.quad([Vec3::new(x1 + o, eave, z0 - o), Vec3::new(xm, ridge, z0 - o), Vec3::new(xm, ridge, z1 + o), Vec3::new(x1 + o, eave, z1 + o)], Vec3::new(pitch, 1.0, 0.0).normalize(), shade(color, 0.85));
            let gable = eave + (xm - x0) * pitch;
            mb.triangle([Vec3::new(x1, eave, z0), Vec3::new(x0, eave, z0), Vec3::new(xm, gable, z0)], siding);
            mb.triangle([Vec3::new(x0, eave, z1), Vec3::new(x1, eave, z1), Vec3::new(xm, gable, z1)], siding);
        }
        RoofStyle::Flat => {
            mb.add_box(Vec3::new(x0 - 0.1, eave, z0 - 0.1), Vec3::new(x1 + 0.1, eave + 0.2, z1 + 0.1), meshgen::same(color));
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn_building(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    mats: &WorldMaterials,
    shared: &SharedMeshes,
    map: &TileMap,
    index: usize,
    b: &Building,
    edges: &mut HashMap<(IVec2, OwnedEdge), Entity>,
    damage: &EdgeDamageMap,
) -> BuildingRender {
    let siding = meshgen::siding_color(b.siding);
    let mut lower = MeshBuilder::default();
    let mut upper = MeshBuilder::default();
    let mut roof = MeshBuilder::default();
    let mut render = BuildingRender::default();
    let belongs = |room: u16| room != 0 && map.rooms[room as usize].building as usize == index;

    // Ground floor walls: every edge in (and just around) the footprint touching one of our rooms.
    for y in b.min.y..=b.min.y + b.size.y {
        for x in b.min.x..=b.min.x + b.size.x {
            let owner = IVec2::new(x, y);
            let tile = map.tile(owner);
            for (which, kind, other) in [(OwnedEdge::North, tile.north, owner - IVec2::Y), (OwnedEdge::West, tile.west, owner - IVec2::X)] {
                let rooms = (map.tile(other).room, tile.room);
                if kind == Edge::None || kind == Edge::Fence || !(belongs(rooms.0) || belongs(rooms.1)) {
                    continue;
                }
                add_wall(&mut lower, map, owner, which, kind, 0.0, rooms, siding);
                let hp = damage.0.get(&(owner, which)).copied().unwrap_or(1.0);
                if let Some(e) = spawn_edge_piece(commands, shared, mats, owner, which, 0.0, kind, hp) {
                    edges.insert((owner, which), e);
                    render.edge_keys.push((owner, which));
                    render.lower.push(e);
                }
            }
        }
    }
    for f in b.furniture.iter() {
        let target = if f.level == 0 { &mut lower } else { &mut upper };
        add_furniture(target, &f.kind, f.min.as_vec2(), f.size.as_vec2(), f.facing, STOREY * f.level as f32);
    }
    // Upper storeys: floor slabs and walls.
    for (level, t, room) in &b.upper_floors {
        let y = STOREY * *level as f32;
        let floor = map.rooms[*room as usize].floor;
        let (x0, z0) = (t.x as f32, t.y as f32);
        upper.quad([Vec3::new(x0, y, z0), Vec3::new(x0, y, z0 + 1.0), Vec3::new(x0 + 1.0, y, z0 + 1.0), Vec3::new(x0 + 1.0, y, z0)], Vec3::Y, floor_color(floor, t.x, t.y));
    }
    for (level, e) in &b.upper_edges {
        let base = STOREY * *level as f32;
        add_wall(&mut upper, map, e.tile, e.edge, e.kind, base, e.rooms, siding);
        if let Some(d) = spawn_edge_piece(commands, shared, mats, e.tile, e.edge, base, e.kind, 1.0) {
            render.upper.push(d);
        }
    }
    add_roof(&mut roof, b, siding);

    for (mb, group) in [(lower, 0), (upper, 1), (roof, 1)] {
        if mb.is_empty() {
            continue;
        }
        let e = commands.spawn((Mesh3d(meshes.add(mb.build())), MeshMaterial3d(mats.building.clone()), Transform::IDENTITY)).id();
        if group == 0 { render.lower.push(e) } else { render.upper.push(e) }
    }
    render
}

fn door_rotations(which: OwnedEdge) -> (Quat, Quat) {
    // The leaf mesh lies along +x from its hinge. North edges run along +x; west edges along +z.
    match which {
        OwnedEdge::North => (Quat::IDENTITY, Quat::from_rotation_y(-FRAC_PI_2)),
        OwnedEdge::West => (Quat::from_rotation_y(-FRAC_PI_2), Quat::IDENTITY),
    }
}

/// The door leaf or window pane for an edge, in the state the map has it now (`None` for edges
/// with neither), with cracks for `hp` (fraction of its hit points left).
#[allow(clippy::too_many_arguments)]
fn spawn_edge_piece(commands: &mut Commands, shared: &SharedMeshes, mats: &WorldMaterials, owner: IVec2, which: OwnedEdge, base: f32, kind: Edge, hp: f32) -> Option<Entity> {
    let (closed, open_rot) = door_rotations(which);
    let start = owner.as_vec2();
    let mid = start + if which == OwnedEdge::North { Vec2::new(0.5, 0.0) } else { Vec2::new(0.0, 0.5) };
    let piece = EdgePiece { mid, shown: kind != Edge::DoorBroken };
    match kind {
        Edge::DoorClosed | Edge::DoorOpen | Edge::DoorBroken => {
            let open = kind == Edge::DoorOpen;
            let rotation = if open { open_rot } else { closed };
            let tf = Transform { translation: Vec3::new(start.x, base, start.y), rotation, scale: Vec3::ONE };
            let e = commands.spawn((Mesh3d(shared.door.clone()), MeshMaterial3d(mats.building.clone()), tf, DoorLeaf { closed, open_rot, open }, piece)).id();
            add_cracks(commands, shared, mats, e, true, hp);
            Some(e)
        }
        Edge::Window | Edge::WindowOpen | Edge::WindowBroken => {
            let (open, broken) = (kind == Edge::WindowOpen, kind == Edge::WindowBroken);
            let lift = if open { SASH_LIFT } else { 0.0 };
            let tf = Transform { translation: Vec3::new(start.x, base + lift, start.y), rotation: closed, scale: Vec3::ONE };
            let mesh = if broken { shared.shards.clone() } else { shared.pane.clone() };
            let e = commands.spawn((Mesh3d(mesh), MeshMaterial3d(mats.glass.clone()), tf, WindowPane { base, open, broken }, piece)).id();
            add_cracks(commands, shared, mats, e, false, if broken { 1.0 } else { hp });
            Some(e)
        }
        _ => None,
    }
}

/// The crack overlay child of a door leaf or pane, showing the stage for `hp`.
fn add_cracks(commands: &mut Commands, shared: &SharedMeshes, mats: &WorldMaterials, piece: Entity, door: bool, hp: f32) {
    let stage = crack_stage(hp);
    let meshes = if door { &shared.door_cracks } else { &shared.pane_cracks };
    let vis = if stage.is_some() { Visibility::Inherited } else { Visibility::Hidden };
    let overlay = commands
        .spawn((Mesh3d(meshes[stage.unwrap_or(0)].clone()), MeshMaterial3d(mats.building.clone()), Transform::IDENTITY, vis, ChildOf(piece)))
        .id();
    commands.entity(piece).insert(Cracks { overlay, door });
}

/// Doors and windows the server says were battered (or are whole again, i.e. broken).
fn apply_edge_damage(
    damage: Res<EdgeDamageThisFrame>,
    streamer: Res<Streamer>,
    shared: Option<Res<SharedMeshes>>,
    pieces: Query<&Cracks>,
    mut overlays: Query<(&mut Mesh3d, &mut Visibility)>,
) {
    let Some(shared) = shared else { return };
    for d in &damage.0 {
        let Some(cracks) = streamer.edges.get(&(d.tile, d.edge)).and_then(|e| pieces.get(*e).ok()) else { continue };
        let Ok((mut mesh, mut vis)) = overlays.get_mut(cracks.overlay) else { continue };
        let meshes = if cracks.door { &shared.door_cracks } else { &shared.pane_cracks };
        match crack_stage(d.hp) {
            Some(k) => {
                if mesh.0 != meshes[k] {
                    mesh.0 = meshes[k].clone();
                }
                *vis = Visibility::Inherited;
            }
            None => *vis = Visibility::Hidden,
        }
    }
}

/// Doors and windows the server changed (opened, shut, broken).
fn apply_edge_changes(
    changes: Res<EdgeChangesThisFrame>,
    streamer: Res<Streamer>,
    shared: Option<Res<SharedMeshes>>,
    mut pieces: Query<(&mut EdgePiece, Option<&mut DoorLeaf>, Option<&mut WindowPane>, &mut Mesh3d)>,
) {
    let Some(shared) = shared else { return };
    for c in &changes.0 {
        let Some(e) = streamer.edges.get(&(c.tile, c.edge)) else { continue };
        let Ok((mut piece, leaf, pane, mut mesh)) = pieces.get_mut(*e) else { continue };
        if let Some(mut leaf) = leaf {
            leaf.open = c.kind == Edge::DoorOpen;
            // A battered-down door's leaf is gone.
            piece.shown = c.kind != Edge::DoorBroken;
        }
        if let Some(mut pane) = pane {
            pane.open = c.kind == Edge::WindowOpen;
            let broken = c.kind == Edge::WindowBroken;
            if broken != pane.broken {
                pane.broken = broken;
                mesh.0 = if broken { shared.shards.clone() } else { shared.pane.clone() };
            }
        }
    }
}

fn animate_panes(time: Res<Time>, mut panes: Query<(&WindowPane, &mut Transform)>) {
    let t = 1.0 - (-10.0 * time.delta_secs()).exp();
    for (pane, mut tf) in &mut panes {
        // Shards stay in the frame; a whole pane slides up when open.
        let target = pane.base + if pane.open && !pane.broken { SASH_LIFT } else { 0.0 };
        tf.translation.y += (target - tf.translation.y) * t;
    }
}

fn animate_doors(time: Res<Time>, mut leaves: Query<(&DoorLeaf, &mut Transform)>) {
    let t = 1.0 - (-12.0 * time.delta_secs()).exp();
    for (leaf, mut tf) in &mut leaves {
        let target = if leaf.open { leaf.open_rot } else { leaf.closed };
        tf.rotation = tf.rotation.slerp(target, t);
    }
}

/// Hide a building's upper storeys and roof while you're inside it, or while it stands between
/// you and the camera, so you always see your character and the floor you're on.
fn hide_upper_floors(
    map: Res<ClientMap>,
    streamer: Res<Streamer>,
    player: Query<&Transform, With<Player>>,
    cams: Query<&IsoCamera>,
    mut vis: Query<&mut Visibility>,
) {
    let (Ok(p), Ok(cam)) = (player.single(), cams.single()) else { return };
    let pos = p.translation.xz();
    let inside = map.0.building_at(pos);
    let toward_cam = cam.offset_dir();
    for (i, render) in &streamer.buildings {
        let b = &map.0.buildings[*i];
        let hide = Some(*i) == inside || hides_player(b, pos, toward_cam);
        let want = if hide { Visibility::Hidden } else { Visibility::Inherited };
        for e in &render.upper {
            if let Ok(mut v) = vis.get_mut(*e) {
                if *v != want {
                    *v = want;
                }
            }
        }
    }
}

/// Does any part of the building above the ground floor cover the player on screen? Walks the line
/// from the player's body toward the camera and checks whether it passes through the building's
/// footprint below its top.
fn hides_player(b: &Building, player: Vec2, toward_cam: Vec3) -> bool {
    let top = STOREY * b.levels as f32 + 2.5;
    let min = b.min.as_vec2() - Vec2::splat(0.4);
    let max = (b.min + b.size).as_vec2() + Vec2::splat(0.4);
    let ground = toward_cam.xz().normalize_or_zero();
    let rise = toward_cam.y / toward_cam.xz().length().max(1e-4);
    for body_height in [0.3, 1.6] {
        let mut s = 0.0;
        while s < 16.0 {
            let q = player + ground * s;
            let h = body_height + rise * s;
            if h > top {
                break;
            }
            // Only the part above the ground-floor walls: the ground floor has its own cutaway.
            if h > STOREY - 0.2 && q.x >= min.x && q.y >= min.y && q.x <= max.x && q.y <= max.y {
                return true;
            }
            s += 0.5;
        }
    }
    false
}

/// Door leaves and window panes drop with their wall (same rule as the shader's wall cutaway).
fn cut_edge_pieces(
    map: Res<ClientMap>,
    player: Query<&Transform, With<Player>>,
    cams: Query<&IsoCamera>,
    mut pieces: Query<(&EdgePiece, &Transform, &mut Visibility), Without<Player>>,
) {
    let (Ok(p), Ok(cam)) = (player.single(), cams.single()) else { return };
    let pos = p.translation.xz();
    let inside = materials::inside_rect(&map.0, pos);
    let dir = cam.offset_dir();
    for (piece, tf, mut vis) in &mut pieces {
        // Upper-floor pieces are handled by hiding the upper floor.
        if tf.translation.y > 1.0 {
            continue;
        }
        let cut = materials::wall_is_cut(piece.mid, pos, dir, inside);
        let want = if piece.shown && !cut { Visibility::Inherited } else { Visibility::Hidden };
        if *vis != want {
            *vis = want;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Cracks appear once a door or window is properly battered and get worse in stages, each
    /// overlay adding to the last and staying on the leaf or pane.
    #[test]
    fn cracks_grow_in_stages() {
        assert_eq!(crack_stage(1.0), None);
        assert_eq!(crack_stage(0.9), None);
        assert_eq!(crack_stage(0.7), Some(0));
        assert_eq!(crack_stage(0.4), Some(1));
        assert_eq!(crack_stage(0.05), Some(2));
        for door in [true, false] {
            let counts: Vec<usize> = (0..CRACK_STAGES).map(|k| crack_mesh(door, k).count_vertices()).collect();
            assert!(counts[0] > 0 && counts.windows(2).all(|w| w[1] > w[0]), "door {door}: {counts:?}");
            let mesh = crack_mesh(door, CRACK_STAGES - 1);
            let Some(bevy::mesh::VertexAttributeValues::Float32x3(pos)) = mesh.attribute(Mesh::ATTRIBUTE_POSITION) else { panic!() };
            let (top, depth) = if door { (2.05, 0.04) } else { (WINDOW_HEAD, 0.02) };
            for p in pos {
                assert!(p[0] >= 0.0 && p[0] <= 1.0 && p[1] >= 0.0 && p[1] <= top && p[2].abs() <= depth, "door {door}: {p:?} off the piece");
            }
        }
    }
}
