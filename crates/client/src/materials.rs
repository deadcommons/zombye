//! The world material: `StandardMaterial` plus two tricks done in the fragment shader:
//! - **fog of war**: anything outside the player's line of sight is darkened and desaturated;
//! - **cutaway**: walls that would hide the player drop to knee-high stubs as whole pieces, and the
//!   foliage of trees in front of the player fades out over a quarter of a second, leaving their
//!   trunk and branches dimmed and thinned, so your character is never hidden. Inside a building,
//!   only the walls that hide part of your room drop: its far walls and the rest of the building's
//!   walls stand (#84). Roofs and upper storeys are hidden whole on the CPU when they cover you
//!   (`world_render::hide_upper_floors`).
//!
//! Every world mesh uses one of a handful of [`WorldMaterial`]s, and their shared parameters are
//! refreshed once per frame. Only materials that need it are built with the cutaway (`CUTAWAY` in
//! the shader): a shader that can discard a fragment loses the GPU's hidden-surface removal for
//! everything drawn with it, so the ground and most chunks' props don't carry it. A chunk's props
//! switch to the cutaway version while one of its trees is fading.

use bevy::mesh::MeshVertexBufferLayoutRef;
use bevy::pbr::{ExtendedMaterial, MaterialExtension, MaterialExtensionKey, MaterialExtensionPipeline};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, RenderPipelineDescriptor, ShaderType, SpecializedMeshPipelineError};
use bevy::shader::ShaderRef;

use crate::camera::IsoCamera;
use crate::fog::FogState;
use crate::player::Player;
use crate::server_link::ClientMap;
use zombye_sim::map::TileMap;

pub type WorldMaterial = ExtendedMaterial<StandardMaterial, WorldExt>;

const SHADER: &str = "shaders/world.wesl";

#[derive(Asset, AsBindGroup, Reflect, Debug, Clone, Default)]
#[bind_group_data(WorldKey)]
pub struct WorldExt {
    #[uniform(100)]
    pub params: WorldParams,
    #[texture(101)]
    #[sampler(102)]
    pub fog: Handle<Image>,
    /// The sight rays' reach, one texel each (`fog.rs`).
    #[texture(103, sample_type = "float", filterable = false)]
    pub rays: Handle<Image>,
    /// Built with the cutaway (walls, roofs, fading foliage)? Fixed when the material is made.
    pub cutaway: bool,
}

/// The pipeline key of a [`WorldExt`]: whether its shader has the cutaway.
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, Hash, PartialEq, Eq)]
pub struct WorldKey {
    pub cutaway: bool,
}

impl From<&WorldExt> for WorldKey {
    fn from(ext: &WorldExt) -> Self {
        WorldKey { cutaway: ext.cutaway }
    }
}

#[derive(ShaderType, Reflect, Debug, Clone)]
pub struct WorldParams {
    /// xyz = player chest position, w = cutaway radius in metres (0 = off).
    pub player: Vec4,
    /// xyz = unit vector from the scene toward the camera.
    pub camera_dir: Vec4,
    /// x, y = world x/z of the fog mask's min corner, z = mask size in metres.
    pub fog_rect: Vec4,
    /// rgb = tint for unseen areas, w = how dark unseen areas get (0..1).
    pub fog_tint: Vec4,
    /// Bounds of the ground-floor room the player is in: (min x, min z, max x, max z). min > max = outdoors.
    pub inside_rect: Vec4,
    /// x = how many entries of `trees` are in use.
    pub tree_count: Vec4,
    /// Sight for the per-pixel fog of war (`FogState::sight`).
    pub sight: [Vec4; 3],
    /// Trees whose foliage is fading: (trunk foot x, z, fade 0..1 (1 = foliage gone), 0).
    pub trees: [Vec4; MAX_FADING_TREES],
}

impl Default for WorldParams {
    fn default() -> Self {
        Self {
            player: Vec4::ZERO,
            camera_dir: Vec4::ZERO,
            fog_rect: Vec4::ZERO,
            fog_tint: Vec4::ZERO,
            inside_rect: Vec4::ZERO,
            tree_count: Vec4::ZERO,
            sight: [Vec4::ZERO; 3],
            trees: [Vec4::ZERO; MAX_FADING_TREES],
        }
    }
}

/// Must match the array size in world.wesl.
pub const MAX_FADING_TREES: usize = 32;
/// Seconds for a tree's foliage to fade out or come back.
const FADE_SECS: f32 = 0.25;

/// Does a tree's crown hide (part of) the player, seen from the camera? The crown and the player
/// are compared where they land in the view plane: a crown nearer the camera than the player whose
/// outline overlaps the player's.
pub fn crown_hides(t: &crate::meshgen::trees::TreeSpec, player: Vec3, camera_dir: Vec3) -> bool {
    let d = t.crown - player;
    let along = d.dot(camera_dir);
    if along <= 0.0 {
        return false;
    }
    let perp = (d - along * camera_dir).length();
    perp < t.crown_r.max(t.crown_h * 0.9) * 0.9 + 0.6
}

/// How far behind a wall (on the ground, away from the camera) it can hide things: a 3 m wall seen
/// from 30° up covers about 5.2 m of floor behind it. Same as `WALL_REACH` in world.wesl.
const WALL_REACH: f32 = 5.5;

/// Does the ground segment from `p` to `p - c * WALL_REACH` pass through the rectangle?
fn behind_hits(p: Vec2, c: Vec2, lo: Vec2, hi: Vec2) -> bool {
    let d = -c * WALL_REACH;
    let (mut t0, mut t1) = (0.0f32, 1.0f32);
    for i in 0..2 {
        if d[i].abs() < 1e-5 {
            if p[i] < lo[i] || p[i] > hi[i] {
                return false;
            }
        } else {
            let (a, b) = ((lo[i] - p[i]) / d[i], (hi[i] - p[i]) / d[i]);
            t0 = t0.max(a.min(b));
            t1 = t1.min(a.max(b));
        }
    }
    t0 <= t1
}

/// The shader's wall rule (`wall_cut` in world.wesl), for things drawn as separate entities (door
/// leaves, window panes). Indoors (`room` = the player's room's bounds) a piece is cut only if it
/// hides part of that room; outdoors only if it covers the player.
pub fn wall_is_cut(mid: Vec2, player: Vec2, camera_dir: Vec3, room: Vec4) -> bool {
    let c = camera_dir.xz().normalize_or_zero();
    if room.x <= room.z {
        let (lo, hi) = (Vec2::new(room.x, room.y) + 0.03, Vec2::new(room.z, room.w) - 0.03);
        let half = if (mid.x.fract() - 0.5).abs() < 0.25 { Vec2::new(0.45, 0.0) } else { Vec2::new(0.0, 0.45) };
        return [mid, mid - half, mid + half].into_iter().any(|p| behind_hits(p, c, lo, hi));
    }
    let d = mid - player;
    let along = d.dot(c);
    let perp = (d - along * c).length();
    along > 0.0 && along < WALL_REACH && perp < 1.6
}

/// Bounds (min x, min z, max x, max z) of the ground-floor room containing `p`; min > max outdoors.
/// An L-shaped room gives the box round it.
pub fn inside_rect(map: &TileMap, p: Vec2) -> Vec4 {
    let room = map.room_at(p);
    let Some(i) = map.building_at(p) else { return Vec4::new(1.0, 1.0, 0.0, 0.0) };
    let b = &map.buildings[i];
    let (mut lo, mut hi) = (IVec2::MAX, IVec2::MIN);
    for y in b.min.y..b.min.y + b.size.y {
        for x in b.min.x..b.min.x + b.size.x {
            let t = IVec2::new(x, y);
            if map.tile(t).room == room {
                lo = lo.min(t);
                hi = hi.max(t + 1);
            }
        }
    }
    if lo.x > hi.x {
        return Vec4::new(1.0, 1.0, 0.0, 0.0);
    }
    Vec4::new(lo.x as f32, lo.y as f32, hi.x as f32, hi.y as f32)
}

impl MaterialExtension for WorldExt {
    fn fragment_shader() -> ShaderRef {
        SHADER.into()
    }

    fn specialize(
        _pipeline: &MaterialExtensionPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        key: MaterialExtensionKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        if key.bind_group_data.cutaway
            && let Some(fragment) = descriptor.fragment.as_mut()
        {
            fragment.shader_defs.push("CUTAWAY".into());
        }
        Ok(())
    }
}

/// Every world material, so their shared parameters can be refreshed together.
#[derive(Resource, Default)]
pub struct WorldMaterials {
    pub all: Vec<Handle<WorldMaterial>>,
    /// Terrain and floors (vertex coloured).
    pub ground: Handle<WorldMaterial>,
    /// Buildings and furniture (vertex coloured, double-sided).
    pub building: Handle<WorldMaterial>,
    /// Trees, bushes, hedges and fences: closed shapes wound outward, so single-sided (half the
    /// fragments of drawing their insides too), and without the cutaway...
    pub props: Handle<WorldMaterial>,
    /// ...but with it for the chunks where a tree is fading (`world_render::fade_chunks`).
    pub props_cut: Handle<WorldMaterial>,
    /// Window glass: vertex coloured with alpha, alpha-to-coverage (needs MSAA, on by default),
    /// double-sided.
    pub glass: Handle<WorldMaterial>,
}

pub struct WorldMaterialPlugin;

impl Plugin for WorldMaterialPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MaterialPlugin::<WorldMaterial>::default())
            .init_resource::<TreeFades>()
            .add_systems(Startup, create_materials)
            .add_systems(PostUpdate, update_params);
    }
}

fn create_materials(mut commands: Commands, fog: Res<FogState>, mut materials: ResMut<Assets<WorldMaterial>>) {
    let mut all = Vec::new();
    let mut make = |base: StandardMaterial, cutaway: bool| {
        let h = materials.add(ExtendedMaterial { base, extension: WorldExt { params: WorldParams::default(), fog: fog.image.clone(), rays: fog.rays.clone(), cutaway } });
        all.push(h.clone());
        h
    };
    let vertex_colored = |roughness: f32, double_sided: bool| StandardMaterial {
        base_color: Color::WHITE,
        perceptual_roughness: roughness,
        double_sided,
        cull_mode: if double_sided { None } else { Some(bevy::render::render_resource::Face::Back) },
        ..default()
    };
    let ground = make(vertex_colored(0.95, false), false);
    let building = make(vertex_colored(0.85, true), true);
    let props = make(vertex_colored(0.85, false), false);
    let props_cut = make(vertex_colored(0.85, false), true);
    // Alpha to coverage instead of blending: drawn with the opaque pass (depth tested and written,
    // nothing to sort), each pixel covering as many MSAA samples as its alpha asks, so panes seen
    // through other panes, zombies behind glass and the cutaway all just work.
    let glass = make(StandardMaterial { alpha_mode: AlphaMode::AlphaToCoverage, reflectance: 0.6, ..vertex_colored(0.15, true) }, true);
    commands.insert_resource(WorldMaterials { all, ground, building, props, props_cut, glass });
}

/// Each fading tree's foliage, by tile: (tile, trunk foot, 0 shown .. 1 gone).
#[derive(Resource, Default)]
pub struct TreeFades(pub Vec<(IVec2, Vec2, f32)>);

#[allow(clippy::too_many_arguments)]
pub fn update_params(
    fog: Res<FogState>,
    map: Res<ClientMap>,
    time: Res<Time>,
    daylight: Res<crate::daynight::Daylight>,
    world_materials: Option<Res<WorldMaterials>>,
    mut materials: ResMut<Assets<WorldMaterial>>,
    mut fades: ResMut<TreeFades>,
    player: Query<&Transform, With<Player>>,
    cams: Query<&IsoCamera>,
) {
    let (Some(wm), Ok(p), Ok(cam)) = (world_materials, player.single(), cams.single()) else { return };
    let chest = p.translation.with_y(1.1);
    let camera_dir = cam.offset_dir();
    // Trees in front of the player fade their foliage out; the rest fade back in.
    let step = time.delta_secs() / FADE_SECS;
    let here = IVec2::new(chest.x.floor() as i32, chest.z.floor() as i32);
    let mut hiding: Vec<(IVec2, Vec2)> = Vec::new();
    const REACH: i32 = 14;
    for y in here.y - REACH..=here.y + REACH {
        for x in here.x - REACH..=here.x + REACH {
            let t = IVec2::new(x, y);
            if !map.0.in_bounds(t) {
                continue;
            }
            if let Some(spec) = crate::meshgen::trees::tree_at(x, y, map.0.tile(t).feature)
                && crown_hides(&spec, chest, camera_dir)
            {
                hiding.push((t, spec.base.xz()));
            }
        }
    }
    for f in fades.0.iter_mut() {
        f.2 = (f.2 - step).max(0.0);
    }
    for (t, anchor) in hiding {
        match fades.0.iter_mut().find(|f| f.0 == t) {
            // Undo this frame's fade-in and fade out instead.
            Some(f) => f.2 = (f.2 + 2.0 * step).min(1.0),
            None => fades.0.push((t, anchor, step.min(1.0))),
        }
    }
    fades.0.retain(|f| f.2 > 0.0);
    // The most faded first, should there ever be more than fit.
    fades.0.sort_by(|a, b| b.2.total_cmp(&a.2));
    let mut trees = [Vec4::ZERO; MAX_FADING_TREES];
    let n = fades.0.len().min(MAX_FADING_TREES);
    for (slot, f) in trees.iter_mut().zip(&fades.0) {
        *slot = Vec4::new(f.1.x, f.1.y, f.2, 0.0);
    }
    let params = WorldParams {
        player: chest.extend(0.0),
        camera_dir: camera_dir.extend(0.0),
        fog_rect: Vec4::new(fog.origin.x, fog.origin.y, fog.size_m, 0.0),
        // What you can't see goes dim and grey by day and nearly black at night (#83).
        fog_tint: Vec4::new(0.55, 0.6, 0.75, 0.72 + 0.14 * (1.0 - daylight.0)),
        inside_rect: inside_rect(&map.0, p.translation.xz()),
        tree_count: Vec4::new(n as f32, 0.0, 0.0, 0.0),
        sight: fog.sight,
        trees,
    };
    for h in &wm.all {
        if let Some(mut m) = materials.get_mut(h) {
            m.extension.params = params.clone();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Inside, only walls that hide part of your room drop: its near walls and walls just in front
    /// of it; its far walls and walls behind it stand, and so do walls well in front (#84).
    #[test]
    fn indoors_only_walls_hiding_your_room_drop() {
        // The default camera: yaw 45°, so it looks from +x, +z (south-east) down at 30°.
        let yaw = std::f32::consts::FRAC_PI_4;
        let elev = 30f32.to_radians();
        let cam = Vec3::new(yaw.sin() * elev.cos(), elev.sin(), yaw.cos() * elev.cos());
        let room = Vec4::new(10.0, 10.0, 14.0, 13.0);
        let player = Vec2::new(11.5, 11.5);
        let cut = |x: f32, z: f32| wall_is_cut(Vec2::new(x, z), player, cam, room);
        // Far walls: north (z = 10) and west (x = 10), every piece, the ones at the near ends too.
        for x in [10.5, 11.5, 12.5, 13.5] {
            assert!(!cut(x, 10.0), "north wall piece at x {x}");
        }
        for z in [10.5, 11.5, 12.5] {
            assert!(!cut(10.0, z), "west wall piece at z {z}");
        }
        // Near walls: south (z = 13) and east (x = 14).
        for x in [10.5, 11.5, 12.5, 13.5] {
            assert!(cut(x, 13.0), "south wall piece at x {x}");
        }
        for z in [10.5, 11.5, 12.5] {
            assert!(cut(14.0, z), "east wall piece at z {z}");
        }
        // A wall just in front of the room, one behind it, and one well in front.
        assert!(cut(12.5, 15.0));
        assert!(!cut(12.5, 7.0));
        assert!(!cut(20.5, 20.0));
        // Beside the room, not in front of it on screen.
        assert!(!cut(5.0, 11.5));
    }
}
