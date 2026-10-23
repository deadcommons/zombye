//! Fixed isometric camera: orthographic, 30° elevation (classic 2:1 look), following
//! the player. With the "Camera rotation" setting (`--rotate`), the `,` and `.` keys snap between the four fixed
//! angles with a short eased turn. Never free rotation.

use std::f32::consts::FRAC_PI_2;

use bevy::camera::ScalingMode;
use bevy::input::mouse::MouseWheel;
use bevy::prelude::*;

use crate::Options;
use crate::fx::CameraShake;
use crate::player::Player;
use crate::server_link::JoinInfo;

const ELEVATION: f32 = 30.0_f32 * std::f32::consts::PI / 180.0;
const DISTANCE: f32 = 120.0;
const MIN_ZOOM: f32 = 8.0;
const MAX_ZOOM: f32 = 90.0;

#[derive(Component)]
pub struct IsoCamera {
    /// Which of the four fixed angles (0..4); yaw = 45° + 90° × index.
    pub angle_index: i32,
    /// Current (animated) yaw in radians.
    pub yaw: f32,
    /// Metres of world visible vertically.
    pub zoom: f32,
    pub focus: Vec3,
}

impl IsoCamera {
    pub fn target_yaw(&self) -> f32 {
        std::f32::consts::FRAC_PI_4 + FRAC_PI_2 * self.angle_index as f32
    }

    /// Unit vector from focus toward the camera.
    pub fn offset_dir(&self) -> Vec3 {
        Vec3::new(self.yaw.sin() * ELEVATION.cos(), ELEVATION.sin(), self.yaw.cos() * ELEVATION.cos())
    }

    /// Camera-relative "forward" (screen up) and "right" on the ground plane.
    pub fn ground_axes(&self) -> (Vec3, Vec3) {
        let forward = Vec3::new(-self.yaw.sin(), 0.0, -self.yaw.cos());
        let right = Vec3::new(self.yaw.cos(), 0.0, -self.yaw.sin());
        (forward, right)
    }
}

pub struct IsoCameraPlugin;

impl Plugin for IsoCameraPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_camera).add_systems(Update, (rotate_and_zoom, follow).chain());
    }
}

fn spawn_camera(mut commands: Commands, join: Res<JoinInfo>, options: Res<Options>) {
    let focus = Vec3::new(join.player_spawn.x, 0.0, join.player_spawn.y);
    let zoom = options.zoom.unwrap_or(26.0).clamp(MIN_ZOOM, MAX_ZOOM);
    let cam = IsoCamera { angle_index: 0, yaw: std::f32::consts::FRAC_PI_4, zoom, focus };
    let transform = Transform::from_translation(cam.focus + cam.offset_dir() * DISTANCE).looking_at(cam.focus, Vec3::Y);
    commands.spawn((
        Camera3d::default(),
        Projection::from(OrthographicProjection {
            scaling_mode: ScalingMode::FixedVertical { viewport_height: cam.zoom },
            near: 0.1,
            far: 400.0,
            ..OrthographicProjection::default_3d()
        }),
        transform,
        // Window glass is drawn with alpha to coverage, which needs multisampling.
        Msaa::Sample4,
        cam,
    ));
}

fn rotate_and_zoom(
    keys: Res<ButtonInput<KeyCode>>,
    mut wheel: MessageReader<MouseWheel>,
    options: Res<Options>,
    time: Res<Time>,
    mut cams: Query<(&mut IsoCamera, &mut Projection)>,
) {
    let Ok((mut cam, mut projection)) = cams.single_mut() else { return };
    if options.rotate {
        // Comma / period (E is pick up).
        if keys.just_pressed(KeyCode::Comma) {
            cam.angle_index -= 1;
        }
        if keys.just_pressed(KeyCode::Period) {
            cam.angle_index += 1;
        }
    }
    // Ease toward the target angle (≈0.25 s turn).
    let target = cam.target_yaw();
    let t = 1.0 - (-14.0 * time.delta_secs()).exp();
    cam.yaw += (target - cam.yaw) * t;

    for ev in wheel.read() {
        cam.zoom = (cam.zoom * (1.0 - ev.y.clamp(-3.0, 3.0) * 0.08)).clamp(MIN_ZOOM, MAX_ZOOM);
    }
    if let Projection::Orthographic(ortho) = projection.as_mut() {
        ortho.scaling_mode = ScalingMode::FixedVertical { viewport_height: cam.zoom };
    }
}

fn follow(
    time: Res<Time>,
    shake: Res<CameraShake>,
    player: Query<&Transform, (With<Player>, Without<IsoCamera>)>,
    mut cams: Query<(&mut IsoCamera, &mut Transform)>,
) {
    let Ok((mut cam, mut transform)) = cams.single_mut() else { return };
    if let Ok(p) = player.single() {
        let t = 1.0 - (-8.0 * time.delta_secs()).exp();
        cam.focus = cam.focus.lerp(p.translation.with_y(0.0), t);
    }
    *transform = Transform::from_translation(cam.focus + cam.offset_dir() * DISTANCE).looking_at(cam.focus, Vec3::Y);
    if shake.trauma > 0.0 {
        // A small jitter in the screen plane, strongest right after a hit.
        let s = time.elapsed_secs() * 40.0;
        let amount = shake.trauma * shake.trauma * 0.3 * cam.zoom / 26.0;
        let jitter = Vec2::new((s * 1.3).sin() + (s * 2.9).sin() * 0.5, (s * 1.7).cos() + (s * 3.3).sin() * 0.5) * amount;
        let (right, up) = (transform.right(), transform.up());
        transform.translation += right * jitter.x + up * jitter.y;
    }
}
