//! Fixed isometric camera: orthographic, 30° elevation (classic 2:1 look), following
//! the player. The `,` and `.` keys turn it a quarter at a time between the four fixed angles
//! (Q and E are taken: quit on the death screen, pick up), each turn eased over [`TURN_SECS`].
//! Never free rotation. Everything that goes by the camera reads [`IsoCamera::yaw`] (or the
//! camera's transform) every frame, so movement keys, the cursor's aim, the cutaways, the
//! silhouettes, the opening hints and the sound's panning follow a turn as it happens.

use std::f32::consts::FRAC_PI_2;

use bevy::camera::{Hdr, RenderTarget, ScalingMode, ShadowLodOrigin};
use bevy::input::mouse::MouseWheel;
use bevy::post_process::bloom::{Bloom, BloomPrefilter};
use bevy::post_process::effect_stack::Vignette;
use bevy::prelude::*;
use bevy::render::view::{ColorGrading, Tonemapping};

use crate::Options;
use crate::fx::CameraShake;
use crate::offscreen::OffscreenTarget;
use crate::player::Player;
use crate::server_link::JoinInfo;

const ELEVATION: f32 = 30.0_f32 * std::f32::consts::PI / 180.0;
pub const DISTANCE: f32 = 120.0;
/// The near and far planes. The camera is orthographic, so depth is linear between them: one
/// metre is `1 / (FAR - NEAR)` of the depth buffer (`silhouette.rs` goes by it).
pub const NEAR: f32 = 0.1;
pub const FAR: f32 = 400.0;
const MIN_ZOOM: f32 = 8.0;
const MAX_ZOOM: f32 = 90.0;
/// Seconds a quarter turn takes.
pub const TURN_SECS: f32 = 0.3;

#[derive(Component)]
pub struct IsoCamera {
    /// Which of the four fixed angles (0..4); yaw = 45° + 90° × index.
    pub angle_index: i32,
    /// Current (animated) yaw in radians.
    pub yaw: f32,
    /// Metres of world visible vertically.
    pub zoom: f32,
    pub focus: Vec3,
    /// The turn under way: the yaw it began at and how far through it is (1 = at rest).
    turn_from: f32,
    turn_at: f32,
}

/// The yaw `u` (0..1) of the way through a turn from `from` to `to`: slow out, slow in, and
/// exactly `to` at the end.
pub fn eased_yaw(from: f32, to: f32, u: f32) -> f32 {
    let u = u.clamp(0.0, 1.0);
    if u >= 1.0 { to } else { from + (to - from) * u * u * (3.0 - 2.0 * u) }
}

/// The yaw of fixed angle `index` (fractions: part of the way to the next).
fn yaw_of(index: f32) -> f32 {
    std::f32::consts::FRAC_PI_4 + FRAC_PI_2 * index
}

impl IsoCamera {
    /// A camera at rest at `angle` (0..4; a fraction is that far through the turn to the next
    /// angle and stays there until turned: for pictures).
    pub fn new(angle: f32, zoom: f32, focus: Vec3) -> Self {
        Self { angle_index: angle.round() as i32, yaw: yaw_of(angle), zoom, focus, turn_from: yaw_of(angle), turn_at: 1.0 }
    }

    pub fn target_yaw(&self) -> f32 {
        yaw_of(self.angle_index as f32)
    }

    /// Start turning `quarters` quarter turns on from where the camera is headed. A turn asked
    /// for during another carries on from the yaw the camera has reached.
    pub fn turn(&mut self, quarters: i32) {
        if quarters != 0 {
            self.angle_index += quarters;
            self.turn_from = self.yaw;
            self.turn_at = 0.0;
        }
    }

    /// Is a turn under way?
    pub fn turning(&self) -> bool {
        self.turn_at < 1.0
    }

    /// Move a turn under way on by `dt` seconds. At rest nothing is written.
    pub fn advance(&mut self, dt: f32) {
        if self.turn_at < 1.0 {
            self.turn_at = (self.turn_at + dt / TURN_SECS).min(1.0);
            self.yaw = eased_yaw(self.turn_from, self.target_yaw(), self.turn_at);
        }
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

fn spawn_camera(mut commands: Commands, join: Res<JoinInfo>, options: Res<Options>, graphics: Res<crate::graphics::Graphics>, offscreen: Option<Res<OffscreenTarget>>) {
    // `--center` only applies offscreen (a live camera follows the player).
    let at = options.center.filter(|_| offscreen.is_some()).map_or(join.player_spawn, Vec2::from);
    let focus = Vec3::new(at.x, 0.0, at.y);
    let zoom = options.zoom.unwrap_or(26.0).clamp(MIN_ZOOM, MAX_ZOOM);
    let cam = IsoCamera::new(options.angle, zoom, focus);
    let transform = Transform::from_translation(cam.focus + cam.offset_dir() * DISTANCE).looking_at(cam.focus, Vec3::Y);
    let mut camera = commands.spawn((
        Camera3d::default(),
        Projection::from(OrthographicProjection {
            scaling_mode: ScalingMode::FixedVertical { viewport_height: cam.zoom },
            near: NEAR,
            far: FAR,
            ..OrthographicProjection::default_3d()
        }),
        transform,
        // Window glass is drawn with alpha to coverage, which needs multisampling (graphics
        // preset: 4 from Medium up).
        crate::graphics_ui::msaa(graphics.msaa),
        cam,
    ));
    // The mood (#83): HDR so light sources can glow, a filmic tone map, and the grade and
    // vignette that `daynight.rs` sets from the time of day. Bloom picks up only what's brighter
    // than sunlit ground: a flashlight's hot spot, a candle, a lit window.
    camera.insert((Hdr, Tonemapping::TonyMcMapface, ColorGrading::default(), Vignette { intensity: 0.45, radius: 0.8, smoothness: 2.2, ..default() }));
    // Each post effect is a graphics setting (`graphics.rs`): the film pass is added by `film.rs`.
    if graphics.bloom {
        camera.insert(bloom());
    }
    if let Some(target) = offscreen {
        // The only camera: it is also the origin for point-light shadow detail.
        camera.insert((RenderTarget::Image(target.0.clone().into()), ShadowLodOrigin));
        // `--hud`: the UI is drawn on it too (Bevy only finds a window's camera by itself).
        if options.hud {
            camera.insert(IsDefaultUiCamera);
        }
    }
}

/// The camera's bloom (a graphics setting): only what's brighter than sunlit ground glows.
pub fn bloom() -> Bloom {
    Bloom { intensity: 0.18, prefilter: BloomPrefilter { threshold: 1.1, threshold_softness: 0.4 }, ..Bloom::NATURAL }
}

fn rotate_and_zoom(
    keys: Res<ButtonInput<KeyCode>>,
    mut wheel: MessageReader<MouseWheel>,
    options: Res<Options>,
    time: Res<Time>,
    mut cams: Query<(&mut IsoCamera, &mut Projection)>,
    window: Query<&Window, With<bevy::window::PrimaryWindow>>,
    panels: crate::hud::PanelHits,
    browser: Option<Res<crate::browser_ui::BrowserUi>>,
    mut turned: Local<usize>,
) {
    let Ok((mut cam, mut projection)) = cams.single_mut() else { return };
    // Over a panel the wheel scrolls it (`hud.rs`), not the zoom; so with the server browser open.
    if crate::hud::panel_under_cursor(window.single().ok(), &panels).is_some() || browser.is_some_and(|b| b.open) {
        wheel.clear();
    }
    // Comma / period (Q quits on the death screen, E is pick up). While a text line is being
    // typed its handler has already taken the keys away (`inventory_ui::TextEntry`, `chat.rs`).
    let mut quarters = keys.just_pressed(KeyCode::Period) as i32 - keys.just_pressed(KeyCode::Comma) as i32;
    // `--turn N@SECS`: turns by themselves, for recordings.
    while let Some(&(n, _)) = options.turns.get(*turned).filter(|t| time.elapsed_secs() >= t.1) {
        quarters += n;
        *turned += 1;
    }
    if quarters != 0 {
        cam.turn(quarters);
    }
    if cam.turning() {
        cam.advance(time.delta_secs());
    }
    let mut zoom = cam.zoom;
    for ev in wheel.read() {
        zoom = (zoom * (1.0 - ev.y.clamp(-3.0, 3.0) * 0.08)).clamp(MIN_ZOOM, MAX_ZOOM);
    }
    if zoom != cam.zoom {
        cam.zoom = zoom;
        if let Projection::Orthographic(ortho) = projection.as_mut() {
            ortho.scaling_mode = ScalingMode::FixedVertical { viewport_height: zoom };
        }
    }
}

fn follow(
    time: Res<Time>,
    options: Res<Options>,
    shake: Res<CameraShake>,
    view: Res<crate::player::ViewFloor>,
    player: Query<&Transform, (With<Player>, Without<IsoCamera>)>,
    mut cams: Query<(&mut IsoCamera, &mut Transform)>,
) {
    let Ok((mut cam, mut transform)) = cams.single_mut() else { return };
    // An offscreen render keeps the camera where it was put (on the player, or `--center`).
    if let Some(p) = player.single().ok().filter(|_| !options.offscreen()) {
        let t = 1.0 - (-8.0 * time.delta_secs()).exp();
        // The floor the player stands on (a sleeper's body lies on a bed: not that).
        cam.focus = cam.focus.lerp(p.translation.with_y(view.height), t);
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A quarter turn takes `TURN_SECS`, starts and ends slowly, never overshoots, and ends
    /// exactly on the fixed angle, whatever the frame rate.
    #[test]
    fn a_turn_is_eased_and_ends_on_the_angle() {
        let (from, to) = (yaw_of(0.0), yaw_of(1.0));
        assert_eq!(eased_yaw(from, to, 0.0), from);
        assert_eq!(eased_yaw(from, to, 1.0), to);
        assert!((eased_yaw(from, to, 0.5) - (from + to) * 0.5).abs() < 1e-6);
        // Slow at both ends: a tenth of the time moves under 3% of the way.
        assert!((eased_yaw(from, to, 0.1) - from) < (to - from) * 0.03);
        assert!((to - eased_yaw(from, to, 0.9)) < (to - from) * 0.03);
        for fps in [240.0, 60.0, 30.0, 7.0, 2.0] {
            let mut cam = IsoCamera::new(0.0, 26.0, Vec3::ZERO);
            assert!(!cam.turning());
            cam.advance(1.0);
            assert_eq!(cam.yaw, from, "at rest nothing moves");
            cam.turn(1);
            let (mut last, mut secs) = (cam.yaw, 0.0);
            while cam.turning() {
                cam.advance(1.0 / fps);
                secs += 1.0 / fps;
                assert!(cam.yaw >= last && cam.yaw <= to, "{fps} fps: {} after {last}", cam.yaw);
                last = cam.yaw;
            }
            assert_eq!(cam.yaw, to, "{fps} fps");
            assert!(secs >= TURN_SECS - 1e-4 && secs < TURN_SECS + 1.0 / fps + 1e-4, "{fps} fps: {secs} s");
        }
    }

    /// A turn asked for in the middle of another carries on from where the camera is: no jump,
    /// and back the other way returns to the angle it left. `--angle 0.5` rests mid-turn.
    #[test]
    fn turns_chain_without_a_jump() {
        let mut cam = IsoCamera::new(0.0, 26.0, Vec3::ZERO);
        cam.turn(1);
        cam.advance(TURN_SECS * 0.5);
        let mid = cam.yaw;
        assert!(mid > yaw_of(0.2) && mid < yaw_of(0.8));
        cam.turn(1);
        assert_eq!(cam.yaw, mid);
        cam.advance(0.001);
        assert!((cam.yaw - mid).abs() < 0.01);
        cam.advance(TURN_SECS);
        assert_eq!((cam.yaw, cam.angle_index), (yaw_of(2.0), 2));
        cam.turn(-2);
        cam.advance(TURN_SECS * 0.5);
        cam.turn(1);
        cam.advance(TURN_SECS);
        assert_eq!((cam.yaw, cam.angle_index), (yaw_of(1.0), 1));
        let rest = IsoCamera::new(0.5, 26.0, Vec3::ZERO);
        assert!(!rest.turning() && (rest.yaw - yaw_of(0.5)).abs() < 1e-6);
    }

    /// Mid-turn, the movement keys and the cursor go by the camera as it is drawn: W is up the
    /// screen and D to its right at any yaw, and a point on the ground seen at some place on the
    /// screen is the point a ray from that place comes down on.
    #[test]
    fn input_follows_the_camera_mid_turn() {
        for k in 0..=24 {
            let mut cam = IsoCamera::new(0.0, 26.0, Vec3::new(40.0, 0.0, 55.0));
            cam.turn(1);
            cam.advance(TURN_SECS * k as f32 / 24.0);
            let tf = Transform::from_translation(cam.focus + cam.offset_dir() * DISTANCE).looking_at(cam.focus, Vec3::Y);
            let (forward, right) = cam.ground_axes();
            let up_flat = tf.up().with_y(0.0).normalize();
            assert!(forward.distance(up_flat) < 1e-4, "W is not up the screen at yaw {}", cam.yaw);
            assert!(right.distance(*tf.right()) < 1e-4, "D is not to the right at yaw {}", cam.yaw);
            // The cursor over a ground point: where it is on the screen plane, and back down the
            // view direction to the ground.
            let p = cam.focus + Vec3::new(3.5, 0.0, -2.25);
            let rel = p - tf.translation;
            let (sx, sy) = (rel.dot(*tf.right()), rel.dot(*tf.up()));
            let origin = tf.translation + *tf.right() * sx + *tf.up() * sy;
            let dir = *tf.forward();
            let hit = origin + dir * (-origin.y / dir.y);
            assert!(hit.distance(p) < 1e-3, "yaw {}: {hit} for {p}", cam.yaw);
        }
    }
}
