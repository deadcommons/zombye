//! Day/night: sun and moon, ambient light and sky colour follow the server's clock.

use std::f32::consts::PI;

use bevy::prelude::*;

use crate::player::Player;
use crate::server_link::ServerLink;

const SUNRISE: f32 = 6.0;
const SUNSET: f32 = 20.0;

/// 0 at night, 1 in full daylight (smooth through dawn and dusk).
#[derive(Resource, Default)]
pub struct Daylight(pub f32);

#[derive(Component)]
struct Sun;

#[derive(Component)]
struct PlayerLight;

pub struct DayNightPlugin;

impl Plugin for DayNightPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Daylight>().add_systems(Startup, spawn_lights).add_systems(Update, (update_lights, attach_player_light));
    }
}

fn spawn_lights(mut commands: Commands) {
    commands.spawn((Sun, DirectionalLight { shadow_maps_enabled: true, ..default() }, Transform::default()));
}

/// Direction the light travels (from sun or moon toward the ground), its colour and brightness.
pub fn sky_at(hour: f32) -> (Vec3, Color, f32, f32) {
    let day = ((hour - SUNRISE) / (SUNSET - SUNRISE)).clamp(0.0, 1.0);
    let elevation = (day * PI).sin();
    let daylight = smoothstep(0.0, 0.25, elevation) * if (SUNRISE..SUNSET).contains(&hour) { 1.0 } else { 0.0 };
    if daylight > 0.02 {
        // Sun crosses from east (+x) to west (-x), tilted south.
        let azimuth = PI * (1.0 - day);
        let dir = -Vec3::new(azimuth.cos(), elevation.max(0.15) * 1.6, 0.45).normalize();
        let warm = 1.0 - smoothstep(0.15, 0.5, elevation);
        let color = Color::srgb(1.0, 0.95 - 0.25 * warm, 0.85 - 0.45 * warm);
        (dir, color, 10_000.0 * daylight, daylight)
    } else {
        (Vec3::new(0.35, -1.0, 0.25).normalize(), Color::srgb(0.6, 0.7, 1.0), 450.0, 0.0)
    }
}

fn update_lights(
    link: Res<ServerLink>,
    mut daylight: ResMut<Daylight>,
    mut sun: Query<(&mut DirectionalLight, &mut Transform), With<Sun>>,
    mut ambient: ResMut<GlobalAmbientLight>,
    mut clear: ResMut<ClearColor>,
    mut lamp: Query<&mut PointLight, With<PlayerLight>>,
) {
    let (dir, color, lux, day) = sky_at(link.curr.time_of_day);
    daylight.0 = day;
    if let Ok((mut light, mut tf)) = sun.single_mut() {
        light.illuminance = lux;
        light.color = color;
        *tf = Transform::default().looking_to(dir, Vec3::Y);
    }
    let night = Color::srgb(0.32, 0.4, 0.65).to_linear();
    let noon = Color::srgb(0.75, 0.8, 0.9).to_linear();
    ambient.color = Color::from(night.mix(&noon, day));
    ambient.brightness = 70.0 + 300.0 * day;
    clear.0 = Color::srgb(0.02 + 0.04 * day, 0.02 + 0.05 * day, 0.04 + 0.06 * day);
    // A faint glow around the player at night so you can still make out your surroundings.
    if let Ok(mut l) = lamp.single_mut() {
        l.intensity = 60_000.0 * (1.0 - day);
    }
}

fn attach_player_light(mut commands: Commands, players: Query<Entity, Added<Player>>) {
    for e in &players {
        commands.entity(e).with_children(|c| {
            c.spawn((PlayerLight, PointLight { range: 9.0, radius: 0.2, intensity: 0.0, color: Color::srgb(0.85, 0.85, 1.0), ..default() }, Transform::from_xyz(0.0, 1.5, 0.0)));
        });
    }
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}
