//! Day/night and the mood (#83, docs/art-direction.md "Light and mood"): sun and moon, ambient
//! light, the camera's grade and vignette, and the player's own light (a flashlight cone or a
//! candle's glow, from what the local player holds) all follow the server's clock.

use std::f32::consts::PI;

use bevy::light::{CascadeShadowConfig, CascadeShadowConfigBuilder, DirectionalLightShadowMap};
use bevy::post_process::effect_stack::Vignette;
use bevy::prelude::*;
use bevy::render::view::{ColorGrading, ColorGradingGlobal, ColorGradingSection};

use crate::camera::IsoCamera;
use crate::player::{LocalPlayer, Player};
use crate::server_link::{JoinInfo, ServerLink};

const SUNRISE: f32 = 6.0;
const SUNSET: f32 = 20.0;

/// 0 at night, 1 in full daylight (smooth through dawn and dusk).
#[derive(Resource, Default)]
pub struct Daylight(pub f32);

#[derive(Component)]
struct Sun;

/// How much more ambient light there is indoors by day in the first-person view (times, at full daylight).
const INDOOR_FILL: f32 = 2.0;

/// The faint glow that lets you make out your own feet in the dark.
#[derive(Component)]
struct PlayerGlow;

/// The light the local player holds: a flashlight's cone or a candle's pool.
#[derive(Component)]
struct HeldBeam;

#[derive(Component)]
struct HeldFlame;

/// A light the local player holds in either hand: its reach, and whether it's a flame (a candle)
/// rather than a beam (a flashlight).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HeldLight {
    pub radius_m: f32,
    pub flame: bool,
}

/// Half-angle of a flashlight's cone (radians) and of its bright middle.
pub const BEAM_OUTER: f32 = 0.5;
pub const BEAM_INNER: f32 = 0.3;
/// Where the beam leaves the body, and how far ahead its middle meets the ground.
pub const BEAM_HEIGHT: f32 = 1.25;
pub const BEAM_AIM_M: f32 = 4.5;

pub struct DayNightPlugin;

impl Plugin for DayNightPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Daylight>()
            // The graphics preset sets its size (`graphics_ui::apply`); High's until then.
            .insert_resource(DirectionalLightShadowMap { size: 4096 })
            .add_systems(Startup, spawn_lights)
            .add_systems(Update, (update_lights, attach_player_glow, fit_sun_cascade, (held_light_system, grade_camera).after(update_lights)));
    }
}

fn spawn_lights(mut commands: Commands) {
    commands.spawn((Sun, DirectionalLight { shadow_maps_enabled: true, ..default() }, Transform::default(), sun_cascade(26.0)));
    let beam = SpotLight {
        intensity: 0.0,
        range: 14.0,
        radius: 0.05,
        color: Color::srgb(1.0, 0.93, 0.8),
        shadow_maps_enabled: true,
        outer_angle: BEAM_OUTER,
        inner_angle: BEAM_INNER,
        ..default()
    };
    commands.spawn((HeldBeam, beam, Transform::default(), Visibility::Hidden));
    let flame = PointLight { intensity: 0.0, range: 4.0, radius: 0.05, color: Color::srgb(1.0, 0.72, 0.42), shadow_maps_enabled: true, ..default() };
    commands.spawn((HeldFlame, flame, Transform::default(), Visibility::Hidden));
}

/// Direction the light travels (from sun or moon toward the ground), its colour and brightness,
/// and the daylight factor. The sun stays low all day (never overhead), so shadows stay long.
pub fn sky_at(hour: f32) -> (Vec3, Color, f32, f32) {
    let day = ((hour - SUNRISE) / (SUNSET - SUNRISE)).clamp(0.0, 1.0);
    let elevation = (day * PI).sin();
    let daylight = smoothstep(0.0, 0.25, elevation) * if (SUNRISE..SUNSET).contains(&hour) { 1.0 } else { 0.0 };
    if daylight > 0.02 {
        // Sun crosses from east (+x) to west (-x), tilted south and kept low (about 47° up at noon,
        // 32° by mid-afternoon), so shadows are long all day. Its light is golden even at its
        // highest and deep amber toward dawn and dusk (DCM-59).
        let azimuth = PI * (1.0 - day);
        let dir = -Vec3::new(azimuth.cos(), 0.2 + 0.4 * elevation, 0.55).normalize();
        let warm = 1.0 - smoothstep(0.1, 0.8, elevation);
        let color = Color::srgb(1.0, 0.88 - 0.24 * warm, 0.7 - 0.36 * warm);
        (dir, color, 16_000.0 * daylight, daylight)
    } else {
        (Vec3::new(0.35, -1.0, 0.25).normalize(), Color::srgb(0.55, 0.65, 1.0), 320.0, 0.0)
    }
}

/// How far the sun (or moon) moves across the sky before the light's direction follows: 0.3°,
/// about every 3.5 s of a default day.
const SUN_STEP: f32 = 0.005;

/// The direction the light is drawn from: the one `held` until the sky's has moved [`SUN_STEP`]
/// from it. The clock moves the sun a little every server tick, and each new direction turns the
/// shadow map's texel grid about the world's origin, hundreds of metres from the player: about a
/// texel a tick, so every shadow edge was resampled 20 times a second and its stair-steps (long
/// teeth under a low sun) flickered like interlaced video. Held still between steps, the edges
/// stand still; a step moves a 3 m shadow's tip 1.5 cm, half a texel.
fn sun_step(held: Option<Vec3>, dir: Vec3) -> Vec3 {
    match held {
        Some(h) if h.angle_between(dir) < SUN_STEP => h,
        _ => dir,
    }
}

/// One shadow cascade fitted round what the camera sees at this zoom. The camera is orthographic
/// and [`crate::camera::DISTANCE`] back, so the default cascades (made for a perspective camera,
/// the first ending 10 m out) put the whole view in the last and coarsest one: blocky shadows.
/// Fitted, a 4096 map gives about 3 cm a texel at the default zoom.
fn sun_cascade(zoom: f32) -> CascadeShadowConfig {
    let reach = zoom * 1.6 + 12.0;
    CascadeShadowConfigBuilder {
        num_cascades: 1,
        minimum_distance: (crate::camera::DISTANCE - reach).max(0.1),
        maximum_distance: crate::camera::DISTANCE + reach,
        ..default()
    }
    .build()
}

/// The first-person view's cascades: a perspective camera right behind the survivor, so fine
/// shadows near it and coarser ones out to where they stop mattering.
fn shoulder_cascade() -> CascadeShadowConfig {
    CascadeShadowConfigBuilder { num_cascades: 3, minimum_distance: 0.1, first_cascade_far_bound: 9.0, maximum_distance: 90.0, ..default() }.build()
}

fn fit_sun_cascade(cams: Query<&IsoCamera>, mut sun: Query<&mut CascadeShadowConfig, With<Sun>>, mut fitted: Local<f32>) {
    let (Ok(cam), Ok(mut config)) = (cams.single(), sun.single_mut()) else { return };
    // Refit when the zoom has moved by more than a few percent, or the view changed (-1: first person).
    let want = if cam.shoulder() { -1.0 } else { cam.zoom };
    if (want - *fitted).abs() > want.abs() * 0.05 {
        *fitted = want;
        *config = if cam.shoulder() { shoulder_cascade() } else { sun_cascade(cam.zoom) };
    }
}

/// First person: where the haze starts and where it has taken everything (metres from the
/// camera): the chunks drawn end about 96 m out.
const HAZE: (f32, f32) = (45.0, 92.0);

/// The sky behind the first-person view's horizon at this much daylight (0 night, 1 day).
pub fn sky_colour(day: f32) -> Color {
    let night = LinearRgba::rgb(0.006, 0.008, 0.016);
    let noon = LinearRgba::rgb(0.42, 0.55, 0.72);
    Color::from(night.mix(&noon, day.clamp(0.0, 1.0)))
}

#[allow(clippy::too_many_arguments)]
fn update_lights(
    link: Res<ServerLink>,
    preview: Res<crate::dev_time::LightPreview>,
    mut daylight: ResMut<Daylight>,
    mut sun: Query<(&mut DirectionalLight, &mut Transform), With<Sun>>,
    mut ambient: ResMut<GlobalAmbientLight>,
    mut clear: ResMut<ClearColor>,
    mut glow: Query<&mut PointLight, With<PlayerGlow>>,
    mut held: Local<Option<Vec3>>,
    mut commands: Commands,
    mut cams: Query<(Entity, &IsoCamera, Option<&mut DistanceFog>)>,
    map: Option<Res<crate::server_link::ClientMap>>,
    player: Query<&Transform, (With<Player>, Without<Sun>)>,
    time: Res<Time>,
    mut fill: Local<f32>,
) {
    let mut indoors_fill = false;
    let (dir, color, lux, day) = sky_at(preview.hour_or(link.curr.time_of_day));
    daylight.0 = day;
    if let Ok((mut light, mut tf)) = sun.single_mut() {
        light.illuminance = lux;
        light.color = color;
        // Only when the direction steps: the transform stays unchanged in between, so the shadow
        // map is drawn on the same texel grid frame after frame.
        let dir = sun_step(*held, dir);
        if *held != Some(dir) {
            *held = Some(dir);
            *tf = Transform::default().looking_to(dir, Vec3::Y);
        }
    }
    // Little fill light, so a shadow side is clearly darker by day and the night is dark.
    let night = Color::srgb(0.3, 0.38, 0.62).to_linear();
    // By day the fill is a soft, faintly violet grey, a little weaker than it was: shadows stay
    // strong, and a touch cool against the golden sun without going blue.
    let noon = Color::srgb(0.74, 0.7, 0.73).to_linear();
    ambient.color = Color::from(night.mix(&noon, day));
    ambient.brightness = 12.0 + 96.0 * day;
    clear.0 = Color::srgb(0.005 + 0.04 * day, 0.006 + 0.04 * day, 0.012 + 0.05 * day);
    // The first-person view sees the horizon: a sky of the hour's colour behind it, and a haze of
    // the same colour that the far end of the drawn world fades into.
    if let Ok((entity, cam, haze)) = cams.single_mut() {
        indoors_fill = cam.shoulder() && map.as_ref().zip(player.single().ok()).is_some_and(|(m, p)| m.0.building_at(p.translation.xz()).is_some());
        let sky = sky_colour(day);
        match (cam.shoulder(), haze) {
            (true, Some(mut haze)) => {
                clear.0 = sky;
                if haze.color != sky {
                    haze.color = sky;
                }
            }
            (true, None) => {
                clear.0 = sky;
                commands.entity(entity).insert(DistanceFog { color: sky, falloff: FogFalloff::Linear { start: HAZE.0, end: HAZE.1 }, ..default() });
            }
            (false, Some(_)) => {
                commands.entity(entity).remove::<DistanceFog>();
            }
            (false, None) => {}
        }
    }
    // Just enough at night to see your own feet; a light in hand does the rest.
    if let Ok(mut l) = glow.single_mut() {
        l.intensity = 4_000.0 * (1.0 - day);
    }
    // First person inside a building: more fill by day, so the walls the sun doesn't reach
    // aren't one dark shade (from above the eye has the whole lit room to go by; here a wall
    // can be all there is on screen).
    let to = if indoors_fill { 1.0 } else { 0.0 };
    *fill = if time.delta_secs() == 0.0 || (*fill - to).abs() < 0.01 { to } else { *fill + (to - *fill) * (1.0 - (-6.0 * time.delta_secs()).exp()) };
    ambient.brightness *= 1.0 + INDOOR_FILL * day * *fill;
}

fn attach_player_glow(mut commands: Commands, players: Query<Entity, Added<Player>>) {
    for e in &players {
        commands.entity(e).with_children(|c| {
            c.spawn((PlayerGlow, PointLight { range: 3.0, radius: 0.2, intensity: 0.0, color: Color::srgb(0.75, 0.8, 1.0), ..default() }, Transform::from_xyz(0.0, 1.6, 0.0)));
        });
    }
}

/// The light player `id` holds in either hand, if any (`light` on its item definition). Only the
/// local player's inventory reaches this client; others' lights go by their outfits (`lights.rs`).
pub fn held_light(join: &JoinInfo, link: &ServerLink, id: u32) -> Option<HeldLight> {
    let inv = &link.own(id)?.inventory;
    held_light_in(join, [inv.hand.map(|s| s.item), inv.off_hand.map(|s| s.item)])
}

/// The light among the items (by index) in someone's two hands, if any.
pub fn held_light_in(join: &JoinInfo, hands: [Option<u16>; 2]) -> Option<HeldLight> {
    hands.into_iter().flatten().find_map(|item| {
        let def = join.items.get(item as usize)?;
        let l = def.light.as_ref()?;
        Some(HeldLight { radius_m: l.radius_m, flame: def.tags.iter().any(|t| t == "needs_fire") })
    })
}

fn held_light_system(
    join: Res<JoinInfo>,
    link: Res<ServerLink>,
    me: Res<LocalPlayer>,
    daylight: Res<Daylight>,
    time: Res<Time>,
    player: Query<(&Transform, &Player), (Without<HeldBeam>, Without<HeldFlame>)>,
    mut beam: Query<(&mut SpotLight, &mut Transform, &mut Visibility), (With<HeldBeam>, Without<HeldFlame>)>,
    mut flame: Query<(&mut PointLight, &mut Transform, &mut Visibility), (With<HeldFlame>, Without<HeldBeam>)>,
) {
    let (Ok((tf, p)), Ok((mut spot, mut spot_tf, mut spot_vis)), Ok((mut point, mut point_tf, mut point_vis))) = (player.single(), beam.single_mut(), flame.single_mut()) else { return };
    let held = held_light(&join, &link, me.id);
    let facing = Vec2::from_angle(p.facing);
    let forward = Vec3::new(facing.x, 0.0, facing.y);
    // Weaker by day, when it hardly matters (and would only wash out the sunlit ground).
    let dark = 1.0 - 0.85 * daylight.0;
    match held {
        Some(l) if !l.flame => {
            let from = tf.translation + Vec3::Y * BEAM_HEIGHT + forward * 0.35;
            let aim = tf.translation + forward * BEAM_AIM_M;
            *spot_tf = Transform::from_translation(from).looking_at(aim, Vec3::Y);
            spot.range = l.radius_m + 2.0;
            spot.intensity = 900_000.0 * dark;
            *spot_vis = Visibility::Visible;
            *point_vis = Visibility::Hidden;
        }
        Some(l) => {
            *point_tf = Transform::from_translation(tf.translation + Vec3::Y * 1.1 + forward * 0.3);
            point.range = l.radius_m + 1.0;
            point.intensity = 40_000.0 * dark * crate::lights::flicker(time.elapsed_secs(), me.id as f32);
            *point_vis = Visibility::Visible;
            *spot_vis = Visibility::Hidden;
        }
        None => {
            *spot_vis = Visibility::Hidden;
            *point_vis = Visibility::Hidden;
        }
    }
}

/// The grade for this much daylight (0 night .. 1 day): by day a warm, filmic late afternoon
/// (amber white balance, soft contrast, blacks a little lifted and faded, DCM-59), warmest and
/// richest through dawn and dusk; at night cold, drained and closed in, as it was (#83).
pub fn grade_at(day: f32) -> ColorGrading {
    let lerp = |night: f32, noon: f32| night + (noon - night) * day;
    let dusk = 4.0 * day * (1.0 - day);
    ColorGrading {
        global: ColorGradingGlobal {
            exposure: lerp(0.35, 0.2),
            temperature: lerp(-0.03, 0.04) + 0.06 * dusk,
            tint: lerp(0.0, -0.01),
            post_saturation: lerp(0.72, 0.92) + 0.1 * dusk,
            ..default()
        },
        shadows: ColorGradingSection { saturation: lerp(0.6, 0.8), contrast: lerp(1.0, 0.94), lift: lerp(0.0, 0.012), ..default() },
        midtones: ColorGradingSection { saturation: lerp(0.8, 0.95), contrast: lerp(1.04, 1.03), ..default() },
        highlights: ColorGradingSection { saturation: lerp(1.0, 0.9), gain: lerp(1.0, 1.04), ..default() },
    }
}

/// The grade and vignette: warm and a little faded by day, cold and closed in at night.
fn grade_camera(daylight: Res<Daylight>, graphics: Res<crate::graphics::Graphics>, mut cams: Query<(&mut ColorGrading, &mut Vignette), With<IsoCamera>>) {
    let Ok((mut grading, mut vignette)) = cams.single_mut() else { return };
    let day = daylight.0;
    let lerp = |night: f32, noon: f32| night + (noon - night) * day;
    *grading = grade_at(day);
    vignette.intensity = if graphics.vignette { lerp(0.6, 0.46) } else { 0.0 };
    vignette.radius = lerp(0.62, 0.8);
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The light's direction stands still while the clock ticks and follows the sky in small
    /// steps, never far behind it; the jump from sun to moon is taken at once.
    #[test]
    fn the_sun_moves_in_steps() {
        let mut held = None;
        let (mut steps, ticks) = (0, 20 * 60);
        for tick in 0..ticks {
            // A minute at 20 ticks a second, from 9:00, a game hour being 150 s.
            let dir = sky_at(9.0 + tick as f32 / 20.0 / 150.0).0;
            let next = sun_step(held, dir);
            steps += (held != Some(next)) as usize;
            assert!(next.angle_between(dir) < SUN_STEP);
            held = Some(next);
        }
        assert!((5..40).contains(&steps), "{steps} steps in a minute of {ticks} ticks");
        let moon = sky_at(23.0).0;
        assert_eq!(sun_step(held, moon), moon);
    }

    /// DCM-59: by day the sun is low and golden and the grade warm and soft; the night's light and
    /// grade are what they were (#83).
    #[test]
    fn the_day_is_golden_and_the_night_as_dark_as_it_was() {
        for hour in [7.5, 10.0, 12.0, 13.0, 14.0, 16.0, 18.5] {
            let (dir, color, lux, day) = sky_at(hour);
            let up = (-dir.y).asin().to_degrees();
            assert!(up > 8.0 && up < 50.0, "the sun is {up} degrees up at {hour}");
            let c = color.to_srgba();
            assert!(c.red > c.green && c.green > c.blue + 0.15, "sunlight at {hour}: {c:?}");
            assert!(day > 0.0 && lux > 0.0);
        }
        // Lower and redder late in the day than at noon.
        assert!(sky_at(18.0).0.y > sky_at(13.0).0.y && sky_at(18.0).1.to_srgba().blue < sky_at(13.0).1.to_srgba().blue);
        let (_, moon, lux, day) = sky_at(23.0);
        assert_eq!((moon.to_srgba(), lux, day), (Srgba::rgb(0.55, 0.65, 1.0), 320.0, 0.0));

        let (night, noon, dusk) = (grade_at(0.0), grade_at(1.0), grade_at(0.5));
        assert_eq!((night.global.exposure, night.global.temperature, night.global.tint, night.global.post_saturation), (0.35, -0.03, 0.0, 0.72));
        assert_eq!((night.shadows.saturation, night.shadows.contrast, night.shadows.lift, night.midtones.saturation, night.midtones.contrast, night.highlights.gain), (0.6, 1.0, 0.0, 0.8, 1.04, 1.0));
        assert!(noon.global.temperature > 0.0 && dusk.global.temperature > noon.global.temperature);
        assert!(noon.midtones.contrast <= night.midtones.contrast && noon.shadows.lift > 0.0, "soft contrast and lifted blacks by day");
    }
}
