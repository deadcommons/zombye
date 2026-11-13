//! Light from the sky in the shade (light.ron `sky_light`, the graphics settings' "Sky light"):
//! part of the fill light comes from around instead of evenly, so a roof or the ground facing up
//! takes the sky's colour, a wall facing the low sun its glow, and what faces down (eaves, the
//! underside of a car) the warm light the ground throws back. The fill as a whole stays as bright
//! as light.ron's `fill_day` and `fill_night` say: only where it comes from changes.
//!
//! How: the camera has an environment light whose diffuse map is a cube of one texel a face (the
//! sky above, the horizon round the sides with the sun's side brighter, the ground below), blended
//! across the faces as it is sampled, and no specular (a black cube). The six colours are worked
//! out on the CPU from the hour (`faces`) and the image is written only when one of them moves.
//! Both views get it, and every material that is lit by the engine (the world, the crowd, the
//! survivors).
//!
//! The camera has the environment light only when the setting is on at start-up: adding it builds
//! every material's shaders again (a stall), so turning it on takes the next start, and turning it
//! off in play hands its share back to the even fill.

use bevy::asset::RenderAssetUsages;
use bevy::light::EnvironmentMapLight;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat, TextureViewDescriptor, TextureViewDimension};

use crate::camera::IsoCamera;
use crate::daynight::{look, sky_at, sky_colours};

pub struct SkyLightPlugin;

impl Plugin for SkyLightPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PostStartup, attach).add_systems(Update, update.after(crate::daynight::update_lights));
    }
}

/// The diffuse cube and the colours it holds now.
#[derive(Resource)]
struct SkyCube {
    diffuse: Handle<Image>,
    faces: [Vec3; 6],
}

fn luma(c: Vec3) -> f32 {
    c.dot(Vec3::new(0.2126, 0.7152, 0.0722))
}

fn srgb(c: (f32, f32, f32)) -> Vec3 {
    Color::srgb(c.0, c.1, c.2).to_linear().to_vec3()
}

/// The sky's light from each side at this hour, as the cube's layers: +x, -x, up, down, then the
/// world's -z and +z (the engine samples cubes with z turned round). `fill` is the fill light's
/// colour (linear); the six average its luminance, so the fill keeps its brightness.
pub fn faces(hour: f32, fill: Vec3) -> [Vec3; 6] {
    let s = look().sky_light;
    let (zenith, horizon) = sky_colours(hour);
    let (dir, sun, _, day) = sky_at(hour);
    let sun = sun.to_linear().to_vec3();
    let to_sun = Vec2::new(-dir.x, -dir.z).normalize_or_zero();
    let glow = srgb(look().sky.glow);
    // Above: most of the sky, the zenith's colour with some of the horizon's.
    let up = horizon.lerp(zenith, 0.6);
    // Below: the sun's light off the ground by day, a little of the night sky's at night.
    let bounce = ground_light(srgb(s.ground) * sun, luma(up));
    let down = (horizon * 0.5).lerp(bounce, day) * s.bounce;
    // Round the sides: the horizon, brighter toward the sun (or, faintly, the moon).
    let towards = s.sun_side * (0.3 + 0.7 * day) * luma(horizon);
    let side = |x: f32, z: f32| horizon * 0.8 + glow * towards * to_sun.dot(Vec2::new(x, z)).max(0.0);
    let raw = [side(1.0, 0.0), side(-1.0, 0.0), up, down, side(0.0, -1.0), side(0.0, 1.0)];
    let mean = raw.iter().map(|c| luma(*c)).sum::<f32>() / 6.0;
    let fill_hue = fill / luma(fill).max(1e-6);
    raw.map(|c| {
        let rel = c / mean.max(1e-6);
        let hue = rel / luma(rel).max(1e-6);
        fill_hue.lerp(hue, s.tint) * luma(rel) * luma(fill)
    })
}

/// The ground's colour in the sun, at the brightness of the sky above (so `bounce` is a share).
fn ground_light(c: Vec3, sky: f32) -> Vec3 {
    c / luma(c).max(1e-6) * sky
}

/// A cube of one texel a face.
fn cube(faces: &[Vec3; 6]) -> Image {
    Image {
        texture_view_descriptor: Some(TextureViewDescriptor { dimension: Some(TextureViewDimension::Cube), ..default() }),
        ..Image::new(Extent3d { width: 1, height: 1, depth_or_array_layers: 6 }, TextureDimension::D2, texels(faces), TextureFormat::Rgba16Float, RenderAssetUsages::RENDER_WORLD)
    }
}

fn texels(faces: &[Vec3; 6]) -> Vec<u8> {
    faces.iter().flat_map(|c| [c.x, c.y, c.z, 1.0]).flat_map(|f| half::f16::from_f32(f).to_le_bytes()).collect()
}

fn attach(mut commands: Commands, graphics: Res<crate::graphics::Graphics>, cams: Query<Entity, With<IsoCamera>>, mut images: ResMut<Assets<Image>>) {
    if !graphics.sky_light {
        return;
    }
    let Ok(cam) = cams.single() else { return };
    let faces = [Vec3::ZERO; 6];
    let diffuse = images.add(cube(&faces));
    let specular = images.add(cube(&faces));
    commands.entity(cam).insert(EnvironmentMapLight { diffuse_map: diffuse.clone(), specular_map: specular, intensity: 0.0, ..default() });
    commands.insert_resource(SkyCube { diffuse, faces });
}

fn update(
    graphics: Res<crate::graphics::Graphics>,
    cube: Option<ResMut<SkyCube>>,
    link: Res<crate::server_link::ServerLink>,
    preview: Res<crate::dev_time::LightPreview>,
    mut ambient: ResMut<GlobalAmbientLight>,
    mut cams: Query<&mut EnvironmentMapLight, With<IsoCamera>>,
    mut images: ResMut<Assets<Image>>,
) {
    let (Some(mut cube), Ok(mut env)) = (cube, cams.single_mut()) else { return };
    // `update_lights` sets the fill afresh every frame; the sky takes its share of it.
    let share = if graphics.sky_light { look().sky_light.share } else { 0.0 };
    let intensity = ambient.brightness * share;
    ambient.brightness *= 1.0 - share;
    if env.intensity != intensity {
        env.intensity = intensity;
    }
    if share == 0.0 {
        return;
    }
    let want = faces(preview.hour_or(link.curr.time_of_day), ambient.color.to_linear().to_vec3());
    // Written only when a colour has moved by more than a fraction of a per cent: the image goes
    // to the GPU again each time.
    let moved = want.iter().zip(&cube.faces).any(|(a, b)| (*a - *b).abs().max_element() > 0.003 * a.max_element().max(1e-4));
    if moved {
        cube.faces = want;
        if let Some(mut img) = images.get_mut(&cube.diffuse) {
            img.data = Some(texels(&want));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fill keeps its brightness and colour on average; by day the sky above is bluer than
    /// the fill and the ground below darker and warmer, the side toward a low sun the brightest.
    #[test]
    fn the_sky_lights_the_shade_from_above() {
        let fill = Color::srgb(0.86, 0.74, 0.62).to_linear().to_vec3();
        for hour in [9.0, 13.0, 18.5, 1.0] {
            let f = faces(hour, fill);
            let mean = f.iter().map(|c| luma(*c)).sum::<f32>() / 6.0;
            assert!((mean - luma(fill)).abs() < luma(fill) * 0.02, "{hour}: the fill keeps its brightness: {mean} against {}", luma(fill));
            assert!(f.iter().all(|c| c.is_finite() && c.min_element() >= 0.0), "{hour}: {f:?}");
        }
        let noon = faces(13.0, fill);
        let (up, down) = (noon[2], noon[3]);
        assert!(luma(up) > luma(down) * 1.5, "brighter above than below: {noon:?}");
        assert!(up.z / up.x > fill.z / fill.x, "bluer above than the fill: {up} against {fill}");
        assert!(down.x / down.z > up.x / up.z, "warmer below than above: {noon:?}");
        let dusk = faces(18.5, fill);
        let sides = [dusk[0], dusk[1], dusk[4], dusk[5]];
        let to_sun = -sky_at(18.5).0;
        let sunny = if to_sun.x.abs() > to_sun.z.abs() { if to_sun.x > 0.0 { 0 } else { 1 } } else if to_sun.z < 0.0 { 2 } else { 3 };
        assert!(sides.iter().enumerate().all(|(i, c)| i == sunny || luma(*c) < luma(sides[sunny])), "the side toward the low sun is the brightest: {sides:?}");
    }
}
