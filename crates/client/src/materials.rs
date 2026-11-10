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
    /// Built with the ground detail (patchy colour worked out per pixel, DCM-59)? Only the ground.
    pub ground: bool,
}

/// The pipeline key of a [`WorldExt`]: whether its shader has the cutaway and the ground detail.
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, Hash, PartialEq, Eq)]
pub struct WorldKey {
    pub cutaway: bool,
    pub ground: bool,
}

impl From<&WorldExt> for WorldKey {
    fn from(ext: &WorldExt) -> Self {
        WorldKey { cutaway: ext.cutaway, ground: ext.ground }
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
    /// x = how dark it is, 0 by day to 1 at night: lit windows glow by it (#59). y = how much of
    /// its colour an unseen surface keeps ([`unseen_colour`]).
    pub lights: Vec4,
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
            lights: Vec4::ZERO,
            sight: [Vec4::ZERO; 3],
            trees: [Vec4::ZERO; MAX_FADING_TREES],
        }
    }
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// What you can't see (rgb its tint, w how dark it gets): nearly black and cold at night (#83), and
/// by day a dimmer, warm, faded version of itself rather than a grey hole (DCM-59), so the town
/// reads as one sunlit place with your own view the brightest part of it.
pub fn unseen_tint(daylight: f32) -> Vec4 {
    Vec4::new(0.55, 0.6, 0.75, 0.86).lerp(Vec4::new(0.9, 0.83, 0.74, 0.56), daylight)
}

/// How much of its colour an unseen surface keeps (the rest goes grey): a quarter at night, three
/// quarters by day (at dusk the unseen street is still amber, just dimmer).
pub fn unseen_colour(daylight: f32) -> f32 {
    0.25 + 0.5 * daylight
}

/// Must match the array size in world.wesl.
pub const MAX_FADING_TREES: usize = 32;
/// Seconds for a tree's foliage to fade out or come back.
const FADE_SECS: f32 = 0.25;

/// Does a tree's crown hide (part of) the player, seen from the camera? Compared where they land
/// in the view plane: the crown as an ellipse (its radius across, its height foreshortened plus its
/// radius seen from above), the player as the box from feet to head. Only crowns nearer the camera
/// whose outline actually covers the body fade, not every tree in a column behind it.
pub fn crown_hides(t: &crate::meshgen::trees::TreeSpec, chest: Vec3, camera_dir: Vec3) -> bool {
    let d = t.crown - chest;
    if d.dot(camera_dir) <= 0.0 {
        return false;
    }
    let right = Vec3::Y.cross(camera_dir).normalize_or(Vec3::X);
    let up = camera_dir.cross(right);
    let (cos_e, sin_e) = (up.y, camera_dir.y);
    // The crown's outline in the view plane, a little inside its outermost facets.
    let a = t.crown_r * CROWN_COVER;
    let b = ((t.crown_h * cos_e).powi(2) + (t.crown_r * sin_e).powi(2)).sqrt() * CROWN_COVER;
    // The body from the chest: across, and from the feet to the top of the head.
    let (dx, dy) = (d.dot(right), d.dot(up));
    let (feet, head) = (-chest.y * cos_e, (BODY_TOP - chest.y) * cos_e);
    // The point of the body nearest the crown's middle, inside the ellipse?
    let px = dx.clamp(-BODY_HALF_W, BODY_HALF_W) - dx;
    let py = dy.clamp(feet, head) - dy;
    (px / a).powi(2) + (py / b).powi(2) < 1.0
}

/// How much of a crown's radius and height counts as covering (its outline is ragged).
const CROWN_COVER: f32 = 0.95;
/// The player's body for the fade: half its width, and the top of its head.
const BODY_HALF_W: f32 = 0.3;
const BODY_TOP: f32 = 1.85;

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

/// Half a side of the square the player stands in, for the outdoor wall rule (`BODY_HALF` in world.wesl).
const BODY_HALF: f32 = 0.3;

/// The shader's wall rule (`wall_cut` in world.wesl), for things drawn as separate entities (door
/// leaves, window panes). A piece is cut only if the floor behind it on screen runs into the
/// player's room (`room`, its bounds, indoors) or the square the player stands in (outdoors).
pub fn wall_is_cut(mid: Vec2, player: Vec2, camera_dir: Vec3, room: Vec4) -> bool {
    let c = camera_dir.xz().normalize_or_zero();
    let (lo, hi) = if room.x <= room.z {
        (Vec2::new(room.x, room.y) + 0.03, Vec2::new(room.z, room.w) - 0.03)
    } else {
        (player - BODY_HALF, player + BODY_HALF)
    };
    let half = if (mid.x.fract() - 0.5).abs() < 0.25 { Vec2::new(0.45, 0.0) } else { Vec2::new(0.0, 0.45) };
    [mid, mid - half, mid + half].into_iter().any(|p| behind_hits(p, c, lo, hi))
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
        if let Some(fragment) = descriptor.fragment.as_mut() {
            if key.bind_group_data.cutaway {
                fragment.shader_defs.push("CUTAWAY".into());
            }
            if key.bind_group_data.ground {
                fragment.shader_defs.push("GROUND".into());
            }
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

fn create_materials(mut commands: Commands, fog: Res<FogState>, graphics: Res<crate::graphics::Graphics>, mut materials: ResMut<Assets<WorldMaterial>>) {
    let mut all = Vec::new();
    let mut make_with = |base: StandardMaterial, cutaway: bool, ground: bool| {
        let h = materials.add(ExtendedMaterial { base, extension: WorldExt { params: WorldParams::default(), fog: fog.image.clone(), rays: fog.rays.clone(), cutaway, ground } });
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
    let ground = make_with(vertex_colored(0.95, false), false, graphics.ground_detail);
    let mut make = |base: StandardMaterial, cutaway: bool| make_with(base, cutaway, false);
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
        fog_tint: unseen_tint(daylight.0),
        inside_rect: inside_rect(&map.0, p.translation.xz()),
        tree_count: Vec4::new(n as f32, 0.0, 0.0, 0.0),
        lights: Vec4::new(1.0 - smoothstep(0.05, 0.4, daylight.0), unseen_colour(daylight.0), 0.0, 0.0),
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

    /// DCM-59: by day what's unseen is a dimmer, warmer version of itself; at night it is as dark,
    /// cold and grey as it was (#83).
    #[test]
    fn unseen_ground_is_warm_by_day_and_black_at_night() {
        assert_eq!(unseen_tint(0.0), Vec4::new(0.55, 0.6, 0.75, 0.86));
        assert_eq!(unseen_colour(0.0), 0.25);
        let day = unseen_tint(1.0);
        assert!(day.x > day.z && day.w < 0.7 && day.w > 0.4, "{day}");
        assert!(unseen_colour(1.0) > 0.4 && unseen_colour(1.0) < 0.8);
    }

    /// #81: a crown in front of the player fades; one beside the line of sight, or behind the
    /// player, doesn't, however tall.
    #[test]
    fn only_crowns_covering_the_player_fade() {
        use crate::meshgen::trees::tree_at;
        use zombye_sim::map::Feature;
        let yaw = std::f32::consts::FRAC_PI_4;
        let elev = 30f32.to_radians();
        let cam = Vec3::new(yaw.sin() * elev.cos(), elev.sin(), yaw.cos() * elev.cos());
        // A pine (tall) two tiles toward the camera from the player hides it...
        let pine = (0..200).find_map(|i| tree_at(i, 0, Feature::PineTree)).unwrap();
        let chest = pine.base + Vec3::new(-1.5, 1.1, -1.5);
        assert!(crown_hides(&pine, chest, cam));
        // ...but not from 3 m to the side of the line of sight, nor from in front of it.
        let right = Vec3::Y.cross(cam).normalize();
        assert!(!crown_hides(&pine, chest + right * 3.0, cam));
        assert!(!crown_hides(&pine, pine.base + Vec3::new(1.5, 1.1, 1.5), cam));
    }

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

    /// Outdoors, a wall drops only when it covers the player: the facade you walk along stands,
    /// the one between you and the camera goes.
    #[test]
    fn outdoors_only_walls_covering_you_drop() {
        let yaw = std::f32::consts::FRAC_PI_4;
        let elev = 30f32.to_radians();
        let cam = Vec3::new(yaw.sin() * elev.cos(), elev.sin(), yaw.cos() * elev.cos());
        let outdoors = Vec4::new(1.0, 1.0, 0.0, 0.0);
        // Walking along a facade at x = 20 on its camera side (the camera looks from +x, +z), half a
        // metre off it: the wall is behind you, so none of it drops, the pieces level with you too.
        let player = Vec2::new(20.5, 30.5);
        let cut = |x: f32, z: f32| wall_is_cut(Vec2::new(x, z), player, cam, outdoors);
        for z in [28.5, 29.5, 30.5, 31.5, 32.5] {
            assert!(!cut(20.0, z), "facade piece at z {z} beside the player");
        }
        // A wall a metre or two between the player and the camera (south-east).
        assert!(cut(21.5, 32.0));
        // One behind the player, and one off to the side.
        assert!(!cut(18.5, 28.0));
        assert!(!cut(24.5, 27.0));
    }
}
