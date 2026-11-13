//! The sky of the first-person view (`assets/shaders/sky.wesl`, light.ron `sky`): a gradient
//! from the horizon to the zenith that follows the hour, a glow round the sun or moon, their discs
//! and the stars at night. The haze over the far edge of the drawn world takes the sky's horizon
//! and glow (`daynight::update_lights`), so the two meet without a seam.
//!
//! It is one triangle covering the screen on the far plane, drawn in the main pass where nothing
//! else is. It is always there, in both views and with the graphics setting off: from above (and
//! with the setting off) it gives the clear colour, exactly what showed before, so switching the
//! view or the setting never builds a shader (see `camera.rs` on the first switch's stall). Cost:
//! one cheap fragment for each pixel nothing else covers, which from above is none.

use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::NoFrustumCulling;
use bevy::light::NotShadowCaster;
use bevy::mesh::{Indices, MeshVertexBufferLayoutRef, PrimitiveTopology};
use bevy::pbr::{Material, MaterialPipeline, MaterialPipelineKey, MaterialPlugin};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, RenderPipelineDescriptor, ShaderType, SpecializedMeshPipelineError};
use bevy::shader::ShaderRef;

use crate::camera::IsoCamera;
use crate::daynight::{look, sky_at, sky_colours};

pub struct SkyPlugin;

impl Plugin for SkyPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MaterialPlugin::<SkyMaterial>::default())
            .add_systems(Startup, spawn_sky)
            .add_systems(Update, update_sky.after(crate::daynight::update_lights));
    }
}

/// Must match `SkyLook` in sky.wesl.
#[derive(ShaderType, Debug, Clone, Copy, PartialEq, Default)]
pub struct SkyParams {
    pub zenith: Vec4,
    pub horizon: Vec4,
    pub clear: Vec4,
    pub glow: Vec4,
    pub sun: Vec4,
    pub night: Vec4,
}

#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
pub struct SkyMaterial {
    #[uniform(0)]
    pub params: SkyParams,
}

impl Material for SkyMaterial {
    fn vertex_shader() -> ShaderRef {
        "shaders/sky.wesl".into()
    }

    fn fragment_shader() -> ShaderRef {
        "shaders/sky.wesl".into()
    }

    // Only the main pass: the sky is in no prepass and casts no shadow.
    fn enable_prepass() -> bool {
        false
    }

    fn enable_shadows() -> bool {
        false
    }

    fn specialize(
        _pipeline: &MaterialPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        _key: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        descriptor.primitive.cull_mode = None;
        // On the far plane: drawn only where nothing else is, and it leaves the depth as it was.
        if let Some(depth) = descriptor.depth_stencil.as_mut() {
            depth.depth_write_enabled = Some(false);
        }
        Ok(())
    }
}

#[derive(Component)]
struct Sky;

/// One triangle that covers the screen, given in clip space (the vertex shader takes it as is).
fn sky_triangle() -> Mesh {
    Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD)
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, vec![[-1.0f32, -1.0, 0.0], [3.0, -1.0, 0.0], [-1.0, 3.0, 0.0]])
        .with_inserted_indices(Indices::U16(vec![0, 1, 2]))
}

fn spawn_sky(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>, mut materials: ResMut<Assets<SkyMaterial>>) {
    commands.spawn((
        Sky,
        Mesh3d(meshes.add(sky_triangle())),
        MeshMaterial3d(materials.add(SkyMaterial { params: SkyParams::default() })),
        Transform::default(),
        NoFrustumCulling,
        NotShadowCaster,
    ));
}

/// The sky's parameters at this hour: `clear` the clear colour (linear) the view from above
/// shows, `amount` how much of the sky shows over it (0 = none: the clear colour exactly).
pub fn params(hour: f32, clear: Vec3, amount: f32) -> SkyParams {
    let s = look().sky;
    let (zenith, horizon) = sky_colours(hour);
    let day = sky_at(hour).3;
    let glow = Color::srgb(s.glow.0, s.glow.1, s.glow.2).to_linear().to_vec3();
    SkyParams {
        zenith: zenith.extend(s.curve),
        horizon: horizon.extend(s.below),
        clear: clear.extend(amount.clamp(0.0, 1.0)),
        glow: glow.extend(s.glow_strength),
        sun: Vec4::new(s.glow_tightness, s.sun_size.to_radians(), s.sun_brightness, day),
        night: Vec4::new(s.moon_size.to_radians(), s.moon_brightness, s.stars, s.star_brightness),
    }
}

fn update_sky(
    link: Res<crate::server_link::ServerLink>,
    preview: Res<crate::dev_time::LightPreview>,
    graphics: Res<crate::graphics::Graphics>,
    clear: Res<ClearColor>,
    cams: Query<&IsoCamera>,
    sky: Query<&MeshMaterial3d<SkyMaterial>, With<Sky>>,
    mut materials: ResMut<Assets<SkyMaterial>>,
) {
    let Ok(handle) = sky.single() else { return };
    let amount = match (graphics.sky, cams.single()) {
        (true, Ok(cam)) => cam.eased(),
        _ => 0.0,
    };
    let hour = preview.hour_or(link.curr.time_of_day);
    let want = params(hour, clear.0.to_linear().to_vec3(), amount);
    // Only when it changed: a changed material is prepared again (one mesh, but still).
    if materials.get(&handle.0).is_some_and(|m| m.params != want) {
        if let Some(mut m) = materials.get_mut(&handle.0) {
            m.params = want;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// From above the sky is the clear colour exactly; in first person it is blue by day, warm
    /// low down at dusk, and dark at night with stars.
    #[test]
    fn the_sky_follows_the_hour() {
        let clear = Vec3::new(0.01, 0.02, 0.03);
        assert_eq!(params(13.0, clear, 0.0).clear, clear.extend(0.0));
        let noon = params(13.0, clear, 1.0);
        assert!(noon.zenith.z > noon.zenith.x && noon.zenith.z > noon.horizon.z * 0.5, "a blue zenith by day: {noon:?}");
        assert!(noon.horizon.truncate().length() > noon.zenith.truncate().length(), "paler at the horizon by day");
        let dusk = params(19.4, clear, 1.0);
        assert!(dusk.horizon.x > dusk.horizon.z, "a warm horizon with the sun low: {dusk:?}");
        let night = params(1.0, clear, 1.0);
        assert!(night.horizon.truncate().length() < noon.horizon.truncate().length() * 0.1, "dark at night");
        assert_eq!(night.sun.w, 0.0);
        assert!(night.night.z > 0.0);
    }
}
