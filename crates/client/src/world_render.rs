//! Streams the world around the player into meshes: the ground and props (trees, bushes, fences
//! and hedges) per 16 m chunk (`meshgen::landscape`), and one set of meshes per building (ground
//! floor, upper floor, roof) plus an entity per door leaf and per window pane, which follow the
//! map's edge changes (doors swing, sash windows slide up, broken glass leaves jagged shards, a
//! broken door's leaf is gone). Battered doors and windows show
//! cracks (and splinters) in three stages as their hit points run down (`EdgeDamage` in snapshots).
//! The upper floors and roof of the building you're in fade out so you can see inside, and door
//! leaves and panes follow their wall when it is cut away (`cutaway.rs`): down to the stub, never hidden.

use std::collections::HashMap;
use std::f32::consts::FRAC_PI_2;

use bevy::light::NotShadowCaster;
use bevy::mesh::MeshTag;
use bevy::prelude::*;
use zombye_sim::map::{Building, CHUNK_SIZE, Edge, OwnedEdge, RoofStyle, TileMap};

use crate::Options;
use crate::camera::IsoCamera;
use crate::furniture_models::FurnitureModels;
use crate::materials::{self, WorldMaterials};
use crate::meshgen::palette::pal;
use crate::meshgen::{self, CutKind, CutTag, MeshBuilder, add_furniture, building_kit, floor_color, landscape, shade};
use crate::player::Player;
use crate::server_link::{ClientMap, EdgeChangesThisFrame, EdgeDamageMap, EdgeDamageThisFrame};

/// Storey height and wall thickness, in metres.
pub const STOREY: f32 = 3.0;
/// Half a wall's thickness.
pub(crate) const WALL_T: f32 = 0.08;
/// Height a cut-away wall drops to; must match `STUB_HEIGHT` in world.wesl.
pub(crate) const STUB_HEIGHT: f32 = 0.4;
/// Chunks within this many chunks of the player are loaded.
const LOAD_RADIUS: i32 = 6;
/// Chunks (and, separately, buildings) built per frame while streaming: a chunk of landscape
/// takes about 0.15-0.2 ms to build, so this keeps streaming under a millisecond a frame.
const MAX_LOADS_PER_FRAME: usize = 4;

#[derive(Default)]
struct BuildingRender {
    lower: Vec<Entity>,
    /// Upper floors and roof: faded out while the player is inside (each has a `MeshTag`).
    upper: Vec<Entity>,
    /// How far they are gone now (0 there, 1 gone), and the amount they were last drawn with
    /// (-1: not yet).
    gone: f32,
    drawn: f32,
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
    pane_lit: Handle<Mesh>,
    shards: Handle<Mesh>,
    /// A window on a cut-away wall, a framed lip of glass on the stub: plain, lit, smashed.
    lips: [Handle<Mesh>; 3],
    /// Damage overlays per stage (see [`crack_stage`]), in the leaf's and the pane's own frames.
    door_cracks: [Handle<Mesh>; CRACK_STAGES],
    pane_cracks: [Handle<Mesh>; CRACK_STAGES],
    /// Blender-built furniture, drawn instead of the grey-box shapes where there is a model.
    furniture: FurnitureModels,
    /// The blended black of contact shadows ([`contact_shadows`]); `None` with the setting off.
    contact: Option<Handle<StandardMaterial>>,
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

/// A door leaf or window pane drawn as its own entity. It follows its wall down to the stub when
/// the wall is cut away (`cut_edge_pieces`), and is hidden only when `shown` is off (a broken
/// door's leaf).
#[derive(Component)]
struct EdgePiece {
    /// Its edge, for the cutaway (`Cutaway::amount`).
    key: (IVec2, OwnedEdge),
    /// On the ground floor: upper-storey pieces go with their storey, never with a wall.
    ground: bool,
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
    /// How far the sash is up now (eased toward [`SASH_LIFT`] when open).
    lift: f32,
    open: bool,
    broken: bool,
    /// In a room with a light on (`building_kit::room_lit`).
    lit: bool,
}

impl WindowPane {
    fn mesh(&self, shared: &SharedMeshes) -> Handle<Mesh> {
        self.mesh_ref(shared, false).clone()
    }

    /// Its mesh: the pane in its frame, or (`lip`) the low lip it is on a cut-away wall.
    fn mesh_ref<'a>(&self, shared: &'a SharedMeshes, lip: bool) -> &'a Handle<Mesh> {
        match (lip, self.broken, self.lit) {
            (false, true, _) => &shared.shards,
            (false, false, true) => &shared.pane_lit,
            (false, false, false) => &shared.pane,
            (true, true, _) => &shared.lips[2],
            (true, false, true) => &shared.lips[1],
            (true, false, false) => &shared.lips[0],
        }
    }
}

/// How far an open sash slides up.
const SASH_LIFT: f32 = 0.6;

/// The props mesh (trees, bushes, fences, street props) of a chunk.
#[derive(Component)]
struct ChunkProps(IVec2);

pub struct WorldRenderPlugin;

impl Plugin for WorldRenderPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Streamer>()
            .add_systems(Startup, setup_shared)
            .add_systems(PostUpdate, fade_chunks.after(materials::update_params))
            .add_systems(Update, (stream, apply_edge_changes, apply_edge_damage, animate_doors, animate_panes, hide_upper_floors, cut_edge_pieces).chain().after(crate::cutaway::CutawaySystems));
    }
}

fn setup_shared(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>, mut materials: ResMut<Assets<StandardMaterial>>, options: Res<Options>, graphics: Res<crate::graphics::Graphics>) {
    // Door leaf with its hinge at the origin, lying along +x.
    let mut mb = MeshBuilder::default();
    mb.set_tag(CutTag { kind: CutKind::Structure, ..default() });
    let p = pal();
    mb.add_box(Vec3::new(0.04, 0.0, -0.025), Vec3::new(0.96, 2.05, 0.025), meshgen::same(p.door_leaf));
    mb.add_box(Vec3::new(0.8, 1.0, -0.05), Vec3::new(0.86, 1.06, 0.05), meshgen::same(p.door_handle));
    let door = meshes.add(mb.build());

    // Window glass along +x from the edge's start, filling the frame between sill and header.
    // Structure, like door leaves: dimmed but readable in unseen rooms, down to the stub with its wall.
    let glass = p.window_glass.with_alpha(0.32);
    let pane_mesh = |kind: CutKind| {
        let mut mb = MeshBuilder::default();
        mb.set_tag(CutTag { kind, ..default() });
        mb.add_box(Vec3::new(0.05, WINDOW_SILL, -0.012), Vec3::new(0.95, WINDOW_HEAD, 0.012), meshgen::same(glass));
        mb.build()
    };
    let pane = meshes.add(pane_mesh(CutKind::Structure));
    // The same in a room with a light on: it glows at night (world.wesl).
    let pane_lit = meshes.add(pane_mesh(CutKind::Lit));
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
    // A window on a cut-away wall: a low framed lip of glass standing on the stub (two short
    // posts, a rail, the glass between; jagged teeth for a smashed one), so you still see where
    // the windows are. From y = 0 (the stub's top) to `LIP_HEIGHT`.
    let lip_mesh = |kind: Option<CutKind>| {
        let mut mb = MeshBuilder::default();
        mb.set_tag(CutTag { kind: CutKind::Structure, ..default() });
        let frame = meshgen::same(p.trim_light);
        mb.add_box(Vec3::new(0.03, 0.0, -0.03), Vec3::new(0.09, LIP_HEIGHT, 0.03), frame);
        mb.add_box(Vec3::new(0.91, 0.0, -0.03), Vec3::new(0.97, LIP_HEIGHT, 0.03), frame);
        mb.add_box(Vec3::new(0.09, LIP_HEIGHT - 0.04, -0.03), Vec3::new(0.91, LIP_HEIGHT, 0.03), frame);
        let lip_glass = p.window_glass.with_alpha(0.5);
        match kind {
            Some(kind) => {
                mb.set_tag(CutTag { kind, ..default() });
                mb.add_box(Vec3::new(0.09, 0.0, -0.012), Vec3::new(0.91, LIP_HEIGHT - 0.04, 0.012), meshgen::same(lip_glass));
            }
            None => {
                for t in [[(0.09, 0.0), (0.3, 0.0), (0.09, 0.15)], [(0.91, 0.0), (0.91, 0.17), (0.7, 0.0)], [(0.42, 0.0), (0.56, 0.0), (0.5, 0.09)]] {
                    mb.triangle(t.map(|(x, y)| Vec3::new(x, y, 0.0)), lip_glass);
                }
            }
        }
        mb.build()
    };
    let lips = [meshes.add(lip_mesh(Some(CutKind::Structure))), meshes.add(lip_mesh(Some(CutKind::Lit))), meshes.add(lip_mesh(None))];
    let door_cracks = std::array::from_fn(|k| meshes.add(crack_mesh(true, k)));
    let pane_cracks = std::array::from_fn(|k| meshes.add(crack_mesh(false, k)));
    // Models live beside the game data: `<data>/../models/furniture` (assets/models/furniture) and
    // `<data>/../models/street` (street props).
    let mut furniture = FurnitureModels::load(&options.data.join("../models/furniture"));
    furniture.extend(FurnitureModels::load(&options.data.join("../models/street")));
    // Models still in the works, checked in game before they're promoted (see furniture_models.rs).
    for dir in std::env::var("ZOMBYE_EXTRA_MODELS").unwrap_or_default().split(':').filter(|d| !d.is_empty()) {
        furniture.extend(FurnitureModels::load(std::path::Path::new(dir)));
    }
    info!("{} furniture models", furniture.len());
    let contact = graphics.contact_shadows.then(|| {
        materials.add(StandardMaterial { base_color: Color::WHITE, unlit: true, alpha_mode: AlphaMode::Blend, cull_mode: None, double_sided: true, ..default() })
    });
    commands.insert_resource(SharedMeshes { door, pane, pane_lit, shards, lips, door_cracks, pane_cracks, furniture, contact });
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
        (Vec2::new(0.1, 0.12), Vec2::new(0.9, 1.95), 0.03, 0.022, pal().crack_door)
    } else {
        (Vec2::new(0.07, WINDOW_SILL + 0.03), Vec2::new(0.93, WINDOW_HEAD - 0.03), 0.016, 0.013, pal().crack_glass)
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
            let pale = pal().splinter;
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
pub(crate) const WINDOW_SILL: f32 = 0.9;
pub(crate) const WINDOW_HEAD: f32 = 2.1;

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
        let entities = spawn_chunk(&mut commands, &mut meshes, &mats, map, c, &shared.furniture);
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

fn spawn_chunk(commands: &mut Commands, meshes: &mut Assets<Mesh>, mats: &WorldMaterials, map: &TileMap, c: IVec2, models: &FurnitureModels) -> Vec<Entity> {
    let landscape::ChunkMeshes { ground, mut props, veg } = landscape::build_chunk(map, c);
    meshgen::street_props::add(&mut props, map, c, models);
    // The ground is flat but for ankle-high clutter (kerbs, tufts, ferns, rocks): it receives
    // shadows and casts none, which keeps it out of every shadow cascade. It also holds the chunk's
    // vegetation spots, whose instances `vegetation.rs` spawns as its children when it's near.
    let chunk_veg = crate::vegetation::ChunkVeg { chunk: c, spots: veg };
    let mut out = vec![commands.spawn((Mesh3d(meshes.add(ground.build())), MeshMaterial3d(mats.ground.clone()), Transform::IDENTITY, NotShadowCaster, chunk_veg)).id()];
    if !props.is_empty() {
        out.push(commands.spawn((Mesh3d(meshes.add(props.build())), MeshMaterial3d(mats.props.clone()), Transform::IDENTITY, ChunkProps(c))).id());
    }
    out
}

/// Colour of one side of a wall: the room's paint inside, the building's siding outside (and on a
/// porch).
fn wall_side_color(map: &TileMap, room: u16, siding: LinearRgba) -> LinearRgba {
    if building_kit::outside(map, room) { siding } else { meshgen::interior_color(&map.rooms[room as usize].room_type) }
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
        // Frames and muntins are the building kit's (`building_kit::Kit::dress_wall`).
        Edge::Window | Edge::WindowOpen | Edge::WindowBroken => {
            piece(mb, 0.0, WINDOW_SILL);
            piece(mb, WINDOW_HEAD, STOREY);
        }
        Edge::DoorClosed | Edge::DoorOpen | Edge::DoorBroken => piece(mb, 2.1, STOREY),
        Edge::Arch => piece(mb, 2.4, STOREY),
        Edge::None | Edge::Fence => {}
    }
    mb.set_tag(CutTag::default());
}

/// The roof's slopes (or slab); the gable walls, trim and the rest are the building kit's.
fn add_roof(mb: &mut MeshBuilder, b: &Building) {
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
        }
        RoofStyle::GableY => {
            let xm = (x0 + x1) * 0.5;
            let ridge = eave + (xm - x0 + o) * pitch;
            mb.quad([Vec3::new(x0 - o, eave, z1 + o), Vec3::new(xm, ridge, z1 + o), Vec3::new(xm, ridge, z0 - o), Vec3::new(x0 - o, eave, z0 - o)], Vec3::new(-pitch, 1.0, 0.0).normalize(), color);
            mb.quad([Vec3::new(x1 + o, eave, z0 - o), Vec3::new(xm, ridge, z0 - o), Vec3::new(xm, ridge, z1 + o), Vec3::new(x1 + o, eave, z1 + o)], Vec3::new(pitch, 1.0, 0.0).normalize(), shade(color, 0.85));
        }
        RoofStyle::Flat => {
            mb.add_box(Vec3::new(x0 - 0.1, eave, z0 - 0.1), Vec3::new(x1 + 0.1, eave + 0.2, z1 + 0.1), meshgen::same(color));
        }
    }
}

/// A building's merged meshes: the ground floor (walls and furniture), the upper storeys, and the
/// roof. Pure (map in, meshes out), so it can be timed (`building_build_time`).
pub struct BuildingMeshes {
    pub lower: MeshBuilder,
    pub upper: MeshBuilder,
    pub roof: MeshBuilder,
}

pub fn build_building(map: &TileMap, index: usize, b: &Building, furniture: &FurnitureModels) -> BuildingMeshes {
    let kit = building_kit::Kit::new(map, index, b);
    let siding = kit.wall;
    let mut lower = MeshBuilder::default();
    let mut upper = MeshBuilder::default();
    let mut roof = MeshBuilder::default();
    let ground = ground_edges(map, index, b);
    for &(owner, which, kind, rooms) in &ground {
        add_wall(&mut lower, map, owner, which, kind, 0.0, rooms, siding);
        kit.dress_wall(&mut lower, owner, which, kind, 0.0, rooms);
    }
    for f in b.furniture.iter() {
        let target = if f.level == 0 { &mut lower } else { &mut upper };
        let (min, size, base) = (f.min.as_vec2(), f.size.as_vec2(), STOREY * f.level as f32);
        if !furniture.add(target, &f.kind, min, size, f.facing, base) {
            add_furniture(target, &f.kind, min, size, f.facing, base);
        }
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
        kit.dress_wall(&mut upper, e.tile, e.edge, e.kind, base, e.rooms);
    }
    add_roof(&mut roof, b);
    kit.finish(&ground, &mut lower, &mut upper, &mut roof);
    BuildingMeshes { lower, upper, roof }
}

/// How far a contact shadow reaches from a wall's face, how dark it is against the wall, and how
/// far above the floor it lies.
const CONTACT_REACH: f32 = 0.6;
const CONTACT_DARK: f32 = 0.46;
const CONTACT_Y: f32 = 0.011;

/// Ground-contact shadows for a building (DCM-59): a soft dark strip on the ground and on the
/// floors along both faces of every ground-floor wall and window, black fading to clear, blended
/// over whatever is there. It stands in for ambient occlusion (the screen-space kind needs MSAA
/// off, which the window glass can't do without) at the cost of one small blended mesh a building.
pub fn contact_shadows(map: &TileMap, index: usize, b: &Building) -> MeshBuilder {
    let mut mb = MeshBuilder::default();
    let dark = LinearRgba::new(0.0, 0.0, 0.0, CONTACT_DARK);
    let clear = LinearRgba::new(0.0, 0.0, 0.0, 0.0);
    for (owner, which, kind, _) in ground_edges(map, index, b) {
        if !(kind == Edge::Wall || kind.is_window()) {
            continue;
        }
        // The edge runs from `a` along `along`; `out` points to one side of it.
        let a = owner.as_vec2();
        let (along, out) = if which == OwnedEdge::North { (Vec2::X, Vec2::Y) } else { (Vec2::Y, Vec2::X) };
        for side in [1.0, -1.0] {
            let near = out * side * WALL_T;
            let far = out * side * (WALL_T + CONTACT_REACH);
            let p = |q: Vec2| Vec3::new(q.x, CONTACT_Y, q.y);
            let (n0, n1, f0, f1) = (p(a + near), p(a + along + near), p(a + far), p(a + along + far));
            mb.triangle_shaded([n0, n1, f1], [dark, dark, clear]);
            mb.triangle_shaded([n0, f1, f0], [dark, clear, clear]);
        }
    }
    mb
}

/// The ground-floor edges of building `index` with something on them (walls, doors, windows,
/// arches): every edge in and just around its footprint touching one of its rooms, with the rooms
/// either side (north/west, south/east).
fn ground_edges(map: &TileMap, index: usize, b: &Building) -> Vec<(IVec2, OwnedEdge, Edge, (u16, u16))> {
    let belongs = |room: u16| room != 0 && map.rooms[room as usize].building as usize == index;
    let mut out = Vec::new();
    for y in b.min.y..=b.min.y + b.size.y {
        for x in b.min.x..=b.min.x + b.size.x {
            let owner = IVec2::new(x, y);
            let tile = map.tile(owner);
            for (which, kind, other) in [(OwnedEdge::North, tile.north, owner - IVec2::Y), (OwnedEdge::West, tile.west, owner - IVec2::X)] {
                let rooms = (map.tile(other).room, tile.room);
                if kind == Edge::None || kind == Edge::Fence || !(belongs(rooms.0) || belongs(rooms.1)) {
                    continue;
                }
                out.push((owner, which, kind, rooms));
            }
        }
    }
    out
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
    let mut render = BuildingRender { drawn: -1.0, ..default() };
    // Door leaves and window panes, one entity each.
    let lit = |rooms: (u16, u16)| building_kit::room_lit(map, rooms.0) || building_kit::room_lit(map, rooms.1);
    for (owner, which, kind, rooms) in ground_edges(map, index, b) {
        let hp = damage.0.get(&(owner, which)).copied().unwrap_or(1.0);
        if let Some(e) = spawn_edge_piece(commands, shared, mats, owner, which, 0.0, kind, hp, lit(rooms)) {
            edges.insert((owner, which), e);
            render.edge_keys.push((owner, which));
            render.lower.push(e);
        }
        // A ground-floor window with a light on behind it throws a warm pool at night (`lights.rs`).
        if kind.is_window() && lit(rooms) {
            let mid = owner.as_vec2() + if which == OwnedEdge::North { Vec2::new(0.5, 0.0) } else { Vec2::new(0.0, 0.5) };
            render.lower.push(crate::lights::spawn_window_light(commands, mid));
        }
    }
    for (level, e) in &b.upper_edges {
        let base = STOREY * *level as f32;
        if let Some(d) = spawn_edge_piece(commands, shared, mats, e.tile, e.edge, base, e.kind, 1.0, lit(e.rooms)) {
            commands.entity(d).insert(MeshTag::new(0));
            render.upper.push(d);
        }
    }
    let BuildingMeshes { lower, upper, roof } = build_building(map, index, b, &shared.furniture);
    for (mb, group) in [(lower, 0), (upper, 1), (roof, 1)] {
        if mb.is_empty() {
            continue;
        }
        let e = commands.spawn((Mesh3d(meshes.add(mb.build())), MeshMaterial3d(mats.building.clone()), Transform::IDENTITY)).id();
        if group == 0 {
            render.lower.push(e)
        } else {
            // The amount they're faded out by (`hide_upper_floors`).
            commands.entity(e).insert(MeshTag::new(0));
            render.upper.push(e)
        }
    }
    if let Some(material) = &shared.contact {
        let mb = contact_shadows(map, index, b);
        if !mb.is_empty() {
            render.lower.push(commands.spawn((Mesh3d(meshes.add(mb.build())), MeshMaterial3d(material.clone()), Transform::IDENTITY, NotShadowCaster)).id());
        }
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
/// with neither), with cracks for `hp` (fraction of its hit points left); a window `lit` glows at night.
#[allow(clippy::too_many_arguments)]
fn spawn_edge_piece(commands: &mut Commands, shared: &SharedMeshes, mats: &WorldMaterials, owner: IVec2, which: OwnedEdge, base: f32, kind: Edge, hp: f32, lit: bool) -> Option<Entity> {
    let (closed, open_rot) = door_rotations(which);
    let start = owner.as_vec2();
    let piece = EdgePiece { key: (owner, which), ground: base == 0.0, shown: kind != Edge::DoorBroken };
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
            let pane = WindowPane { base, lift, open, broken, lit };
            let e = commands.spawn((Mesh3d(pane.mesh(shared)), MeshMaterial3d(mats.glass.clone()), tf, pane, piece)).id();
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
                mesh.0 = pane.mesh(&shared);
            }
        }
    }
}

fn animate_panes(time: Res<Time>, mut panes: Query<(&mut WindowPane, &mut Transform, &EdgePiece)>) {
    let t = 1.0 - (-10.0 * time.delta_secs()).exp();
    for (mut pane, mut tf, piece) in &mut panes {
        // Shards stay in the frame; a whole pane slides up when open.
        let target = if pane.open && !pane.broken { SASH_LIFT } else { 0.0 };
        // A pane at rest isn't written to: a changed Transform has the engine extract its mesh
        // again, and there are hundreds of panes. Within half a millimetre it is put there.
        let left = target - pane.lift;
        if left == 0.0 {
            continue;
        }
        pane.lift = if left.abs() < 0.0005 { target } else { pane.lift + left * t };
        // Ground-floor panes are placed by `cut_edge_pieces`, with their wall's cut.
        if !piece.ground {
            let y = pane.base + pane.lift;
            if tf.translation.y != y {
                tf.translation.y = y;
            }
        }
    }
}

fn animate_doors(time: Res<Time>, mut leaves: Query<(&DoorLeaf, &mut Transform)>) {
    let t = 1.0 - (-12.0 * time.delta_secs()).exp();
    for (leaf, mut tf) in &mut leaves {
        let target = if leaf.open { leaf.open_rot } else { leaf.closed };
        // As for the panes: a door at rest is left alone, one within 0.03° of it is put there.
        if tf.rotation == target {
            continue;
        }
        tf.rotation = if tf.rotation.angle_between(target) < 0.0005 { target } else { tf.rotation.slerp(target, t) };
    }
}

/// Seconds for the roof and upper storeys of the building you walk into to fade out (and back in
/// when you leave).
const UPPER_FADE_SECS: f32 = 0.15;

/// Fade out a building's upper storeys and roof while you're inside it, so you see the floor
/// you're on: every floor above the player's (players are only ever on the ground floor). The
/// whole of them, as one, in a stipple by the amount in their `MeshTag` (world.wesl), then hidden.
/// From outside every building keeps its roof and floors: nothing opens a hole in them, and a
/// player they cover shows as a silhouette instead (`silhouette.rs`).
fn hide_upper_floors(
    map: Res<ClientMap>,
    time: Res<Time>,
    mut streamer: ResMut<Streamer>,
    player: Query<&Transform, With<Player>>,
    mut upper: Query<(&mut Visibility, &mut MeshTag)>,
    mut started: Local<bool>,
) {
    let Ok(p) = player.single() else { return };
    let inside = map.0.building_at(p.translation.xz());
    // The first frame's state is taken at once (a picture of a save), like the walls'.
    let step = if *started { time.delta_secs() / UPPER_FADE_SECS } else { 1.0 };
    *started = true;
    for (i, render) in streamer.buildings.iter_mut() {
        let to = if Some(*i) == inside { 1.0 } else { 0.0 };
        if render.gone != to {
            render.gone = if to > render.gone { (render.gone + step).min(1.0) } else { (render.gone - step).max(0.0) };
        }
        // Nearly every building, nearly every frame: as it was drawn last.
        if render.drawn == render.gone {
            continue;
        }
        let (want, tag) = upper_look(render.gone);
        let mut all = true;
        for e in &render.upper {
            // A building spawned this frame has its entities from the next.
            let Ok((mut v, mut t)) = upper.get_mut(*e) else {
                all = false;
                continue;
            };
            if *v != want {
                *v = want;
            }
            if t.value != tag {
                t.value = tag;
            }
        }
        if all {
            render.drawn = render.gone;
        }
    }
}

/// How a building's upper storeys and roof are drawn when they are `gone` this far (0 there, 1
/// gone): hidden once fully gone, else the stipple's amount for the shader (0 = solid).
fn upper_look(gone: f32) -> (Visibility, u32) {
    if gone >= 1.0 {
        return (Visibility::Hidden, 0);
    }
    let eased = gone * gone * (3.0 - 2.0 * gone);
    (Visibility::Inherited, (eased * 255.0).round() as u32)
}

/// How tall the lip of a window on a cut-away wall stands over the stub (see `setup_shared`).
const LIP_HEIGHT: f32 = 0.24;

/// How a door leaf or window pane is drawn on a wall cut this far (0 standing, 1 a stub).
#[derive(Clone, Copy, Debug, PartialEq)]
struct PieceLook {
    shown: bool,
    /// Its height, as a share of its mesh's.
    height: f32,
    /// How far it is lifted off its storey's floor.
    lift: f32,
    /// A window drawn as the low lip on the stub, not as the pane in its frame.
    lip: bool,
}

/// **A door's leaf goes down with its wall and is gone once the wall is cut**: a leaf squashed to
/// the stub's height read as a low gate, so a cut door is shown by its outline at full size
/// instead (`opening_hints::cut_door`, which is what says a doorway holds a door: the
/// playtest's "he doesn't fit through the door"). A window is never hidden: it sinks with its wall for the first half of the way, then is a low
/// framed lip of glass that grows out of the stub, so you still see where the windows are. Only
/// a battered-down door has no leaf.
fn piece_look(shown: bool, cut: f32, door: bool) -> PieceLook {
    if !shown {
        return PieceLook { shown: false, height: 1.0, lift: 0.0, lip: false };
    }
    if door {
        PieceLook { shown: cut < 0.995, height: (1.0 - cut).max(0.02), lift: 0.0, lip: false }
    } else if cut < 0.5 {
        PieceLook { shown: true, height: 1.0 - cut, lift: 0.0, lip: false }
    } else {
        PieceLook { shown: true, height: ((cut - 0.5) * 2.0).max(0.02), lift: STUB_HEIGHT, lip: true }
    }
}

/// Ground-floor door leaves and window panes follow their wall down to the stub and up again, by
/// the wall's own eased amount (`Cutaway::amount`, what the shader draws the wall by).
fn cut_edge_pieces(
    cutaway: Res<crate::cutaway::Cutaway>,
    shared: Option<Res<SharedMeshes>>,
    mut pieces: Query<(&EdgePiece, Option<&WindowPane>, &mut Transform, &mut Visibility, &mut Mesh3d)>,
) {
    let Some(shared) = shared else { return };
    for (piece, pane, mut tf, mut vis, mut mesh) in &mut pieces {
        if !piece.ground {
            continue;
        }
        let cut = cutaway.amount(piece.key);
        let look = piece_look(piece.shown, cut, pane.is_none());
        let want = if look.shown { Visibility::Inherited } else { Visibility::Hidden };
        if *vis != want {
            *vis = want;
        }
        if tf.scale.y != look.height {
            tf.scale.y = look.height;
        }
        if let Some(pane) = pane {
            let wanted = pane.mesh_ref(&shared, look.lip);
            if mesh.0 != *wanted {
                mesh.0 = wanted.clone();
            }
            // An open sash is up in its frame; on a stub there is no frame to be up in.
            let y = pane.base + look.lift + if look.lip { 0.0 } else { pane.lift * (1.0 - cut) };
            if tf.translation.y != y {
                tf.translation.y = y;
            }
        }
    }
}

/// Chunks holding a tree whose foliage is fading draw their props with the cutaway material;
/// the rest without it, keeping the GPU's hidden-surface removal (see `materials.rs`).
fn fade_chunks(mats: Option<Res<WorldMaterials>>, fades: Res<materials::TreeFades>, mut props: Query<(&ChunkProps, &mut MeshMaterial3d<materials::WorldMaterial>)>) {
    let Some(mats) = mats else { return };
    for (chunk, mut material) in &mut props {
        let fading = fades.0.iter().any(|f| f.0.div_euclid(IVec2::splat(CHUNK_SIZE)) == chunk.0);
        let want = if fading { &mats.props_cut } else { &mats.props };
        if material.0 != *want {
            material.0 = want.clone();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn town() -> TileMap {
        let data = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/data");
        let templates = zombye_net::content::load_building_templates(&data).expect("templates");
        zombye_sim::worldgen::generate(&zombye_sim::worldgen::GenConfig::default(), &templates).map
    }

    /// A door's leaf sinks with its wall and is gone once the wall is cut (no stubby leaf: the
    /// door's outline stands in for it, `opening_hints::cut_door`); a pane still goes with its
    /// wall, and a battered-down door's leaf is gone.
    #[test]
    fn a_door_in_a_cut_wall_has_no_stub_of_a_leaf() {
        let door = piece_look(true, 1.0, true);
        assert!(!door.shown && door.lift == 0.0, "no leaf on a cut-away wall");
        assert!(piece_look(true, 0.9, true).shown, "still there on the way down");
        let whole = PieceLook { shown: true, height: 1.0, lift: 0.0, lip: false };
        assert_eq!(piece_look(true, 0.0, true), whole, "whole in a standing wall");
        assert_eq!(piece_look(true, 0.0, false), whole);
        assert!(!piece_look(false, 1.0, true).shown && !piece_look(false, 0.0, true).shown, "a broken door has no leaf");
        // Half way down it is half way between.
        let half = piece_look(true, 0.5, true).height;
        assert!(half > 0.25 && half < 0.75, "{half}");
    }

    /// The cutaway never hides a window: on a cut wall it is a low framed lip standing on the stub,
    /// and on the way there the pane sinks with its wall (never left over the wall's top) and the
    /// lip grows out of the stub. The roof and upper storeys fade as one and are hidden only once
    /// fully gone.
    #[test]
    fn a_window_in_a_cut_wall_stands_on_the_stub() {
        let cut = piece_look(true, 1.0, false);
        assert!(cut.shown && cut.lip, "a window stays on a cut-away wall, as its lip");
        assert_eq!((cut.lift, cut.height), (STUB_HEIGHT, 1.0));
        assert!(STUB_HEIGHT + LIP_HEIGHT < WINDOW_SILL, "the lip is lower than the sill was");
        for k in 0..=20 {
            let a = k as f32 / 20.0;
            let look = piece_look(true, a, false);
            assert!(look.shown && look.height > 0.0);
            // The wall's top at this amount (world.wesl): the window is never left over it.
            let wall_top = 3.05 + (STUB_HEIGHT - 3.05) * a;
            let top = if look.lip { look.lift + LIP_HEIGHT * look.height } else { WINDOW_HEAD * look.height + look.lift };
            assert!(top <= wall_top.max(STUB_HEIGHT + LIP_HEIGHT) + 1e-4, "cut {a}: the window's top {top} over the wall's {wall_top}");
        }
        assert_eq!(upper_look(0.0), (Visibility::Inherited, 0));
        assert_eq!(upper_look(1.0).0, Visibility::Hidden);
        let (vis, tag) = upper_look(0.5);
        assert!(vis == Visibility::Inherited && tag > 100 && tag < 160, "{tag}");
    }

    /// DCM-59: every building gets contact shadows, lying just over the ground along its walls
    /// (within their reach of the footprint) and costing a few hundred triangles at most.
    #[test]
    fn buildings_get_contact_shadows() {
        let map = town();
        let mut most = 0;
        for (i, b) in map.buildings.iter().enumerate() {
            let mb = contact_shadows(&map, i, b);
            assert!(mb.tri_count() >= 16, "building {i} ({}) has {} triangles of contact shadow", b.template, mb.tri_count());
            most = most.max(mb.tri_count());
            let (lo, hi) = mb.bounds();
            let reach = WALL_T + CONTACT_REACH + 1e-3;
            assert!(lo.y == CONTACT_Y && hi.y == CONTACT_Y);
            assert!(lo.x >= b.min.x as f32 - reach && lo.z >= b.min.y as f32 - reach, "building {i}: {lo}");
            assert!(hi.x <= (b.min.x + b.size.x) as f32 + reach && hi.z <= (b.min.y + b.size.y) as f32 + reach, "building {i}: {hi}");
        }
        assert!(most <= 1500, "{most} triangles of contact shadow on one building");
    }

    /// Building meshes (walls, trim, furniture, upper storeys, roof), over every building of the
    /// default map (`cargo test -p zombye-client --release building_build_time -- --ignored --nocapture`).
    #[test]
    #[ignore]
    fn building_build_time() {
        let map = town();
        let furniture = FurnitureModels::load(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/models/furniture"));
        let mut times = Vec::new();
        let mut tris = 0;
        let mut by_template: std::collections::BTreeMap<String, (f64, usize)> = Default::default();
        for _ in 0..3 {
            for (i, b) in map.buildings.iter().enumerate() {
                let start = std::time::Instant::now();
                let m = build_building(&map, i, b, &furniture);
                let ms = start.elapsed().as_secs_f64() * 1000.0;
                times.push(ms);
                let e = by_template.entry(b.template.clone()).or_default();
                (e.0, e.1) = (e.0 + ms, e.1 + 1);
                tris += m.lower.tri_count() + m.upper.tri_count() + m.roof.tri_count();
            }
        }
        for (t, (ms, n)) in &by_template {
            eprintln!("  {t}: {:.3} ms", ms / *n as f64);
        }
        times.sort_by(f64::total_cmp);
        let n = times.len();
        let mean = times.iter().sum::<f64>() / n as f64;
        eprintln!("building build: {} buildings x 3, mean {mean:.3} ms, median {:.3} ms, max {:.3} ms; {} triangles a building", n / 3, times[n / 2], times[n - 1], tris / n);
        // Where to find one of each, for pictures (`--center X,Y`).
        let mut seen = std::collections::BTreeMap::new();
        for b in &map.buildings {
            seen.entry(b.template.clone()).or_insert((b.min + b.size / 2, b.look.map(|l| l.front)));
        }
        for (t, (at, front)) in seen {
            eprintln!("  {t}: centre {},{} front {front:?}", at.x, at.y);
        }
    }

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
