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
const BEAM_INNER: f32 = 0.3;
/// Where the beam leaves the body, and how far ahead its middle meets the ground.
const BEAM_HEIGHT: f32 = 1.25;
const BEAM_AIM_M: f32 = 4.5;

pub struct DayNightPlugin;

impl Plugin for DayNightPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Daylight>()
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
        // Sun crosses from east (+x) to west (-x), tilted south, at most about 35° up.
        let azimuth = PI * (1.0 - day);
        let dir = -Vec3::new(azimuth.cos(), 0.22 + 0.55 * elevation, 0.55).normalize();
        let warm = 1.0 - smoothstep(0.1, 0.7, elevation);
        let color = Color::srgb(1.0, 0.88 - 0.24 * warm, 0.7 - 0.38 * warm);
        (dir, color, 15_000.0 * daylight, daylight)
    } else {
        (Vec3::new(0.35, -1.0, 0.25).normalize(), Color::srgb(0.55, 0.65, 1.0), 320.0, 0.0)
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

fn fit_sun_cascade(cams: Query<&IsoCamera>, mut sun: Query<&mut CascadeShadowConfig, With<Sun>>, mut fitted: Local<f32>) {
    let (Ok(cam), Ok(mut config)) = (cams.single(), sun.single_mut()) else { return };
    // Refit when the zoom has moved by more than a few percent.
    if (cam.zoom - *fitted).abs() > cam.zoom * 0.05 {
        *fitted = cam.zoom;
        *config = sun_cascade(cam.zoom);
    }
}

fn update_lights(
    link: Res<ServerLink>,
    preview: Res<crate::dev_time::LightPreview>,
    mut daylight: ResMut<Daylight>,
    mut sun: Query<(&mut DirectionalLight, &mut Transform), With<Sun>>,
    mut ambient: ResMut<GlobalAmbientLight>,
    mut clear: ResMut<ClearColor>,
    mut glow: Query<&mut PointLight, With<PlayerGlow>>,
) {
    let (dir, color, lux, day) = sky_at(preview.hour_or(link.curr.time_of_day));
    daylight.0 = day;
    if let Ok((mut light, mut tf)) = sun.single_mut() {
        light.illuminance = lux;
        light.color = color;
        *tf = Transform::default().looking_to(dir, Vec3::Y);
    }
    // Little fill light, so a shadow side is clearly darker by day and the night is dark.
    let night = Color::srgb(0.3, 0.38, 0.62).to_linear();
    let noon = Color::srgb(0.66, 0.7, 0.82).to_linear();
    ambient.color = Color::from(night.mix(&noon, day));
    ambient.brightness = 12.0 + 110.0 * day;
    clear.0 = Color::srgb(0.005 + 0.04 * day, 0.006 + 0.04 * day, 0.012 + 0.05 * day);
    // Just enough at night to see your own feet; a light in hand does the rest.
    if let Ok(mut l) = glow.single_mut() {
        l.intensity = 4_000.0 * (1.0 - day);
    }
}

fn attach_player_glow(mut commands: Commands, players: Query<Entity, Added<Player>>) {
    for e in &players {
        commands.entity(e).with_children(|c| {
            c.spawn((PlayerGlow, PointLight { range: 3.0, radius: 0.2, intensity: 0.0, color: Color::srgb(0.75, 0.8, 1.0), ..default() }, Transform::from_xyz(0.0, 1.6, 0.0)));
        });
    }
}

/// The light player `id` holds in either hand, if any (`light` on its item definition). Only the
/// local player's inventory reaches this client, so others' lights aren't drawn yet.
pub fn held_light(join: &JoinInfo, link: &ServerLink, id: u32) -> Option<HeldLight> {
    let inv = &link.own(id)?.inventory;
    [inv.hand, inv.off_hand].into_iter().flatten().find_map(|s| {
        let def = join.items.get(s.item as usize)?;
        let l = def.light.as_ref()?;
        Some(HeldLight { radius_m: l.radius_m, flame: def.tags.iter().any(|t| t == "needs_fire") })
    })
}

fn held_light_system(
    join: Res<JoinInfo>,
    link: Res<ServerLink>,
    me: Res<LocalPlayer>,
    daylight: Res<Daylight>,
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
            point.intensity = 40_000.0 * dark;
            *point_vis = Visibility::Visible;
            *spot_vis = Visibility::Hidden;
        }
        None => {
            *spot_vis = Visibility::Hidden;
            *point_vis = Visibility::Hidden;
        }
    }
}

/// The grade and vignette: warm and a little faded by day, cold and closed in at night.
fn grade_camera(daylight: Res<Daylight>, mut cams: Query<(&mut ColorGrading, &mut Vignette), With<IsoCamera>>) {
    let Ok((mut grading, mut vignette)) = cams.single_mut() else { return };
    let day = daylight.0;
    let lerp = |night: f32, noon: f32| night + (noon - night) * day;
    // Warmest and richest through dawn and dusk, not in between.
    let dusk = 4.0 * day * (1.0 - day);
    *grading = ColorGrading {
        global: ColorGradingGlobal {
            exposure: lerp(0.35, 0.15),
            temperature: lerp(-0.03, 0.03) + 0.08 * dusk,
            tint: lerp(0.0, -0.01),
            post_saturation: lerp(0.72, 0.95) + 0.1 * dusk,
            ..default()
        },
        shadows: ColorGradingSection { saturation: lerp(0.6, 0.85), contrast: 1.0, ..default() },
        midtones: ColorGradingSection { saturation: lerp(0.8, 0.95), contrast: lerp(1.04, 1.1), ..default() },
        highlights: ColorGradingSection { saturation: 1.0, gain: lerp(1.0, 1.05), ..default() },
    };
    vignette.intensity = lerp(0.6, 0.42);
    vignette.radius = lerp(0.62, 0.8);
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}
