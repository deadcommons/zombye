//! The world material: `StandardMaterial` plus two tricks done in the fragment shader:
//! - **fog of war**: anything outside the player's line of sight is darkened and desaturated;
//! - **cutaway**: walls drop to knee-high stubs as whole pieces, by the amounts `cutaway.rs`
//!   decides per building and per wall edge and hands over in a small texture (no hole round the
//!   player anywhere); the foliage of trees in front of the player fades out over a quarter of a
//!   second, leaving their trunk and branches dimmed and thinned; and building details standing
//!   over the player (a porch roof, an awning, a post) are thinned out whole, in a stipple. The
//!   roof and upper storeys of the building you're inside fade out (`world_render::hide_upper_floors`).
//!
//! Every world mesh uses one of a handful of [`WorldMaterial`]s, and their shared parameters are
//! refreshed once per frame. Only materials that need it are built with the cutaway (`CUTAWAY` in
//! the shader): a shader that can discard a fragment loses the GPU's hidden-surface removal for
//! everything drawn with it, so the ground and most chunks' props don't carry it. A chunk's props
//! switch to the cutaway version while one of its trees is fading.

use bevy::mesh::MeshVertexBufferLayoutRef;
use bevy::pbr::{ExtendedMaterial, MaterialExtension, MaterialExtensionKey, MaterialExtensionPipeline};
use bevy::prelude::*;
use bevy::asset::RenderAssetUsages;
use bevy::render::render_resource::{AsBindGroup, RenderPipelineDescriptor, SpecializedMeshPipelineError};
use bevy::render::storage::ShaderBuffer;
use bevy::shader::ShaderRef;

use crate::camera::IsoCamera;
use crate::fog::FogState;
use crate::player::Player;
use crate::server_link::ClientMap;

pub type WorldMaterial = ExtendedMaterial<StandardMaterial, WorldExt>;

const SHADER: &str = "shaders/world.wesl";

#[derive(Asset, AsBindGroup, Reflect, Debug, Clone, Default)]
#[bind_group_data(WorldKey)]
pub struct WorldExt {
    /// The shared [`WorldParams`], in a buffer of their own ([`ParamBuffer`]): they change every
    /// frame, and changing a material asset makes Bevy extract, specialise and queue again every
    /// mesh drawn with it, while a buffer's new contents are only uploaded.
    #[storage(100, read_only)]
    pub params: Handle<ShaderBuffer>,
    #[texture(101)]
    #[sampler(102)]
    pub fog: Handle<Image>,
    /// The sight rays' reach, one texel each (`fog.rs`).
    #[texture(103, sample_type = "float", filterable = false)]
    pub rays: Handle<Image>,
    /// How far each wall edge near the player is cut, one texel a tile (`cutaway.rs`).
    #[texture(105, sample_type = "float", filterable = false)]
    pub cut: Handle<Image>,
    /// Built with the cutaway (walls, roofs, fading foliage)? Fixed when the material is made.
    pub cutaway: bool,
    /// Built with the ground detail (patchy colour worked out per pixel, DCM-59)? Only the ground.
    pub ground: bool,
    /// The chunks' ground, whose first UVs carry the height of the terrain under each vertex
    /// (`MeshBuilder::drape`) rather than a cutaway anchor.
    pub terrain: bool,
}

/// The pipeline key of a [`WorldExt`]: whether its shader has the cutaway and the ground detail.
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, Hash, PartialEq, Eq)]
pub struct WorldKey {
    pub cutaway: bool,
    pub ground: bool,
    pub terrain: bool,
}

impl From<&WorldExt> for WorldKey {
    fn from(ext: &WorldExt) -> Self {
        WorldKey { cutaway: ext.cutaway, ground: ext.ground, terrain: ext.terrain }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct WorldParams {
    /// xyz = player chest position, w = the height of the floor whose walls the cutaway cuts.
    pub player: Vec4,
    /// xyz = unit vector from the scene toward the camera.
    pub camera_dir: Vec4,
    /// x, y = world x/z of the fog mask's min corner, z = mask size in metres.
    pub fog_rect: Vec4,
    /// rgb = tint for unseen areas, w = how dark unseen areas get (0..1).
    pub fog_tint: Vec4,
    /// x = how many entries of `trees` are in use. y = how much of a room other than the player's
    /// shows (`FogLook::other_rooms`). z = how far roofs are kept out of the sight shadows now
    /// (`FogLook::roofs_seen`, none at night).
    pub tree_count: Vec4,
    /// x = how dark it is, 0 by day to 1 at night: lit windows glow by it (#59). y = how much of
    /// its colour an unseen surface keeps (`FogLook::unseen_colour`). z, w = the fog's look:
    /// `FogState::soft`.
    pub lights: Vec4,
    /// The haze (`daynight::haze_at`): rgb its colour, w how much it takes (0 = none).
    pub haze: Vec4,
    /// x, y = where its banks have drifted to (metres), z, w = where it starts and where it is
    /// thickest, metres from the player.
    pub haze_at: Vec4,
    /// xyz = unit vector toward the sun (the one directional light: the moon at night), w = 1 when
    /// there is one. The terrain's facets are shaded by it (world.wesl, `FACET_TOWARD`).
    pub sun: Vec4,
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
            tree_count: Vec4::ZERO,
            lights: Vec4::ZERO,
            haze: Vec4::ZERO,
            haze_at: Vec4::ZERO,
            sun: Vec4::ZERO,
            sight: [Vec4::ZERO; 3],
            trees: [Vec4::ZERO; MAX_FADING_TREES],
        }
    }
}

impl WorldParams {
    /// The struct as world.wesl declares it: every field a `vec4<f32>` (or an array of them), in order.
    fn floats(&self) -> Vec<f32> {
        let head = [self.player, self.camera_dir, self.fog_rect, self.fog_tint, self.tree_count, self.lights, self.haze, self.haze_at, self.sun];
        head.iter().chain(&self.sight).chain(&self.trees).flat_map(|v| v.to_array()).collect()
    }
}

/// The buffer every world material (and the vegetation's) reads its [`WorldParams`] from.
#[derive(Resource, Clone)]
pub struct ParamBuffer(pub Handle<ShaderBuffer>);

/// A buffer for shader parameters that change every frame, holding `floats`.
pub fn param_buffer(buffers: &mut Assets<ShaderBuffer>, floats: Vec<f32>) -> Handle<ShaderBuffer> {
    buffers.add(ShaderBuffer::new(floats, RenderAssetUsages::default()))
}

/// Put `floats` in a parameter buffer, if they aren't what it holds (the same size as it was made
/// with, so the GPU buffer is written to, not replaced, and no bind group changes).
pub fn write_params(buffers: &mut Assets<ShaderBuffer>, handle: &Handle<ShaderBuffer>, floats: &[f32]) {
    if buffers.get(handle).is_some_and(|b| b.cast_slice::<f32>() == Some(floats)) {
        return;
    }
    if let Some(mut buffer) = buffers.get_mut(handle) {
        buffer.clear();
        buffer.extend_from_slice(floats);
    }
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
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
            if key.bind_group_data.terrain {
                fragment.shader_defs.push("TERRAIN".into());
            }
        }
        Ok(())
    }
}

/// The world materials. Their shared parameters are in a buffer of their own ([`ParamBuffer`]).
#[derive(Resource, Default)]
pub struct WorldMaterials {
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
        let floats = WorldParams::default().floats();
        let buffer = param_buffer(&mut app.world_mut().resource_mut::<Assets<ShaderBuffer>>(), floats);
        app.add_plugins(MaterialPlugin::<WorldMaterial>::default())
            .insert_resource(ParamBuffer(buffer))
            .init_resource::<TreeFades>()
            .add_systems(Startup, create_materials)
            .add_systems(PostUpdate, update_params);
    }
}

fn create_materials(mut commands: Commands, fog: Res<FogState>, cut: Res<crate::cutaway::Cutaway>, params: Res<ParamBuffer>, graphics: Res<crate::graphics::Graphics>, mut materials: ResMut<Assets<WorldMaterial>>) {
    let mut make_with =
        |base: StandardMaterial, cutaway: bool, ground: bool, terrain: bool| materials.add(ExtendedMaterial { base, extension: WorldExt { params: params.0.clone(), fog: fog.image.clone(), rays: fog.rays.clone(), cut: cut.image.clone(), cutaway, ground, terrain } });
    let vertex_colored = |roughness: f32, double_sided: bool| StandardMaterial {
        base_color: Color::WHITE,
        perceptual_roughness: roughness,
        double_sided,
        cull_mode: if double_sided { None } else { Some(bevy::render::render_resource::Face::Back) },
        ..default()
    };
    let ground = make_with(vertex_colored(0.95, false), false, graphics.ground_detail, true);
    let mut make = |base: StandardMaterial, cutaway: bool| make_with(base, cutaway, false, false);
    let building = make(vertex_colored(0.85, true), true);
    let props = make(vertex_colored(0.85, false), false);
    let props_cut = make(vertex_colored(0.85, false), true);
    // Alpha to coverage instead of blending: drawn with the opaque pass (depth tested and written,
    // nothing to sort), each pixel covering as many MSAA samples as its alpha asks, so panes seen
    // through other panes, zombies behind glass and the cutaway all just work.
    let glass = make(StandardMaterial { alpha_mode: AlphaMode::AlphaToCoverage, reflectance: 0.6, ..vertex_colored(0.15, true) }, true);
    commands.insert_resource(WorldMaterials { ground, building, props, props_cut, glass });
}

/// Each fading tree's foliage, by tile: (tile, trunk foot, 0 shown .. 1 gone).
#[derive(Resource, Default)]
pub struct TreeFades(pub Vec<(IVec2, Vec2, f32)>);

#[allow(clippy::too_many_arguments)]
pub fn update_params(
    fog: Res<FogState>,
    cutaway: Option<Res<crate::cutaway::Cutaway>>,
    map: Res<ClientMap>,
    time: Res<Time>,
    daylight: Res<crate::daynight::Daylight>,
    look: Res<crate::fog::FogLook>,
    haze: Res<crate::daynight::Haze>,
    buffer: Res<ParamBuffer>,
    mut buffers: ResMut<Assets<ShaderBuffer>>,
    mut fades: ResMut<TreeFades>,
    player: Query<&Transform, With<Player>>,
    cams: Query<&IsoCamera>,
    sun: Query<&GlobalTransform, With<crate::daynight::Sun>>,
) {
    let (Ok(p), Ok(cam)) = (player.single(), cams.single()) else { return };
    let chest = p.translation.with_y(1.1);
    // Where the chest really is, over hills too (trees stand on the ground, `zombye_sim::terrain`).
    let chest_world = p.translation + Vec3::Y * 1.1;
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
                && crown_hides(&spec, chest_world - Vec3::Y * map.0.ground(spec.base.xz()), camera_dir)
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
        // w: the height of the floor whose walls are cut (the upper floor's while up there).
        player: chest.extend(cutaway.map_or(0.0, |c| c.level as f32 * 3.0)),
        camera_dir: camera_dir.extend(0.0),
        fog_rect: Vec4::new(fog.origin.x, fog.origin.y, fog.size_m, 0.0),
        fog_tint: look.unseen_tint(daylight.0),
        tree_count: Vec4::new(n as f32, fog.other_rooms, look.roofs_seen * smoothstep(0.05, 0.4, daylight.0), 0.0),
        lights: Vec4::new(1.0 - smoothstep(0.05, 0.4, daylight.0), look.unseen_colour(daylight.0), fog.soft.x, fog.soft.y),
        haze: haze.0,
        haze_at: {
            // The banks drift slowly with the wind; nothing to work out or upload with no haze.
            let h = crate::daynight::look().haze;
            let t = if haze.0.w > 0.0 { (time.elapsed_secs_wrapped() * 10.0).floor() / 10.0 } else { 0.0 };
            // Its reach is in metres at the default zoom: scaled with the zoom, the screen is as hazy at any.
            let k = if cam.low() { 1.0 } else { cam.zoom / crate::daynight::HAZE_ZOOM };
            Vec4::new(t * 0.35, t * 0.12, h.near * k, h.far * k)
        },
        sun: sun.single().map_or(Vec4::ZERO, |t| (-t.forward().as_vec3()).extend(1.0)),
        sight: fog.sight,
        trees,
    };
    write_params(&mut buffers, &buffer.0, &params.floats());
}

#[cfg(test)]
mod tests {
    use super::*;

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

}
