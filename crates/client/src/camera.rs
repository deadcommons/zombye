//! Fixed isometric camera: orthographic, 30° elevation (classic 2:1 look), following
//! the player. The `,` and `.` keys turn it a quarter at a time between the four fixed angles
//! (Q and E are taken: quit on the death screen, pick up), each turn eased over [`TURN_SECS`].
//! Never free rotation. Everything that goes by the camera reads [`IsoCamera::yaw`] (or the
//! camera's transform) every frame, so movement keys, the cursor's aim, the cutaways, the
//! silhouettes, the opening hints and the sound's panning follow a turn as it happens.
//!
//! **The first-person view** ([`View::Shoulder`], `;` switches to it and back, `--first-person`
//! starts in it): the same camera entity with a perspective projection, at the survivor's eyes
//! (he isn't drawn: [`IsoCamera::too_close`]). The mouse turns it (the pointer is held at the
//! middle of the window, where a small dot marks it; [`PointerLock`]), W is the way it looks, and
//! the pointer everything else asks for (the aim, the aim marker, a right tap's menu) is that
//! middle point, so aiming, swings, shoves, stomps and right-clicks go by what the dot is on. No
//! wall is cut and no roof fades in this view (`Cutaway::set_held`). The pointer comes free
//! while the inventory, a menu, the chat line or the server browser is up, once dead, and while
//! Alt is held (for the health and skills panels). Back in the fixed view the camera settles on
//! the nearest of its four angles.

use std::f32::consts::FRAC_PI_2;

use bevy::camera::{Hdr, RenderTarget, ScalingMode, ShadowLodOrigin};
use bevy::input::mouse::{AccumulatedMouseMotion, MouseWheel};
use bevy::anti_alias::smaa::{Smaa, SmaaPreset};
use bevy::pbr::{ScreenSpaceAmbientOcclusion, ScreenSpaceAmbientOcclusionQualityLevel};
use bevy::post_process::bloom::{Bloom, BloomPrefilter};
use bevy::post_process::effect_stack::Vignette;
use bevy::prelude::*;
use bevy::render::view::{ColorGrading, Tonemapping};
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};
use zombye_sim::map::TileMap;

use crate::Options;
use crate::fx::CameraShake;
use crate::offscreen::OffscreenTarget;
use crate::player::Player;
use crate::server_link::{ClientMap, JoinInfo};

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

/// The first-person view's lens: its vertical field of view, and how far it draws.
pub const SHOULDER_FOV: f32 = 68.0_f32 * std::f32::consts::PI / 180.0;
const SHOULDER_FAR: f32 = 260.0;
/// How high the survivor's eyes are, and how far in front of the middle of his head the camera sits.
const SHOULDER_HEIGHT: f32 = 1.6;
const EYES_AHEAD: f32 = 0.12;
/// How far down it looks (radians below level; below 0 it looks up): at first, and its limits.
const PITCH: f32 = 0.06;
const PITCH_RANGE: (f32, f32) = (-1.2, 1.2);
/// Radians of turn for a pixel of mouse.
const LOOK_RATE: f32 = 0.0026;
/// What the first-person aim is on when the middle of the screen is on ground nearer than this
/// beyond the survivor, or on none (the sky): a point this far, or [`AIM_FAR`], ahead.
const AIM_NEAR: f32 = 5.0;
const AIM_FAR: f32 = 60.0;

/// Which of its two views the camera is in.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum View {
    /// The fixed isometric view from above (the default).
    #[default]
    Iso,
    /// First person: at the survivor's eyes.
    Shoulder,
}

/// Whether the pointer is held at the middle of the window (the first-person view, with nothing
/// up that needs a pointer). `aim_marker.rs` keeps the system pointer hidden while it is.
#[derive(Resource, Default)]
pub struct PointerLock(pub bool);

/// The dot at the middle of the screen while the pointer is held there.
#[derive(Component)]
struct CentreDot;

/// The yaw that puts the first-person camera behind a survivor facing `facing` (sim convention).
pub fn yaw_behind(facing: f32) -> f32 {
    (-facing.cos()).atan2(-facing.sin())
}

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
    /// Which view it is in.
    pub view: View,
    /// First person: how far down it looks (radians; below 0, up).
    pub pitch: f32,
    /// First person: go behind the survivor at the next frame (just switched to).
    behind: bool,
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
        Self { angle_index: angle.round() as i32, yaw: yaw_of(angle), zoom, focus, turn_from: yaw_of(angle), turn_at: 1.0, view: View::Iso, pitch: PITCH, behind: false }
    }

    /// Switch to the other view. Into first person the camera goes behind the survivor; back, it
    /// turns to the nearest of the four fixed angles from where it was looking.
    pub fn toggle_view(&mut self) {
        match self.view {
            View::Iso => {
                (self.view, self.behind, self.turn_at) = (View::Shoulder, true, 1.0);
                self.pitch = PITCH;
            }
            View::Shoulder => {
                self.view = View::Iso;
                self.angle_index = ((self.yaw - std::f32::consts::FRAC_PI_4) / FRAC_PI_2).round() as i32;
                (self.turn_from, self.turn_at) = (self.yaw, 0.0);
            }
        }
    }

    pub fn shoulder(&self) -> bool {
        self.view == View::Shoulder
    }

    /// The projection of the view it is in.
    pub fn projection(&self) -> Projection {
        match self.view {
            View::Iso => Projection::from(OrthographicProjection { scaling_mode: ScalingMode::FixedVertical { viewport_height: self.zoom }, near: NEAR, far: FAR, ..OrthographicProjection::default_3d() }),
            View::Shoulder => Projection::from(PerspectiveProjection { fov: SHOULDER_FOV, near: NEAR, far: SHOULDER_FAR, ..default() }),
        }
    }

    /// First person: turn by this much mouse (pixels; right and down positive).
    pub fn look(&mut self, by: Vec2) {
        self.yaw -= by.x * LOOK_RATE;
        self.pitch = (self.pitch + by.y * LOOK_RATE).clamp(PITCH_RANGE.0, PITCH_RANGE.1);
    }

    /// First person: unit vector from the pivot toward the camera.
    fn boom_dir(&self) -> Vec3 {
        Vec3::new(self.yaw.sin() * self.pitch.cos(), self.pitch.sin(), self.yaw.cos() * self.pitch.cos())
    }

    /// First person: the camera at the survivor's eyes, looking the way the mouse has turned it.
    /// (No wall can come between: the eyes are inside his own body's radius.)
    fn shoulder_transform(&self, _map: Option<&TileMap>, _dt: f32) -> Transform {
        let (forward, _) = self.ground_axes();
        let at = self.focus + Vec3::Y * SHOULDER_HEIGHT + forward * EYES_AHEAD;
        Transform::from_translation(at).looking_to(-self.boom_dir(), Vec3::Y)
    }

    /// The point on the ground the aim is on, for the pointer's ray (`origin`, unit `dir`). The
    /// fixed view: where the ray comes down. First person: that too, but never nearer than a few
    /// metres ahead of the survivor (looking down at his feet he still aims ahead) and, with the
    /// pointer on the sky, a point far off the way the camera looks.
    pub fn aim_ground(&self, origin: Vec3, dir: Vec3) -> Option<Vec3> {
        match self.view {
            View::Iso => (dir.y.abs() > 1e-4).then(|| origin + dir * (-origin.y / dir.y)),
            View::Shoulder => {
                let flat = dir.xz();
                let len = flat.length();
                if len < 1e-4 {
                    return None;
                }
                let down = if dir.y < -1e-4 { -origin.y / dir.y * len } else { f32::MAX };
                let out = down.clamp(AIM_NEAR, AIM_FAR);
                let p = origin.xz() + flat / len * out;
                Some(Vec3::new(p.x, 0.0, p.y))
            }
        }
    }

    /// First person: our own survivor isn't drawn (he would only be in the way of the view).
    pub fn too_close(&self) -> bool {
        self.shoulder()
    }

    /// Metres of world top to bottom of the screen where the survivor stands, in either view.
    pub fn view_metres(&self) -> f32 {
        match self.view {
            View::Iso => self.zoom,
            View::Shoulder => 2.0 * (SHOULDER_FOV * 0.5).tan(),
        }
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

    /// Unit vector from focus toward the camera. In first person no flatter than a quarter of
    /// a radian: the cutaways and fades that go by it take it for the slope of the line of sight.
    pub fn offset_dir(&self) -> Vec3 {
        let e = if self.shoulder() { self.pitch.max(0.25) } else { ELEVATION };
        Vec3::new(self.yaw.sin() * e.cos(), e.sin(), self.yaw.cos() * e.cos())
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
        app.init_resource::<PointerLock>().add_systems(Startup, spawn_camera).add_systems(Update, (rotate_and_zoom, follow, lock_pointer).chain());
    }
}

fn spawn_camera(mut commands: Commands, join: Res<JoinInfo>, options: Res<Options>, store: Option<Res<crate::ui_scale::SettingsStore>>, graphics: Res<crate::graphics::Graphics>, offscreen: Option<Res<OffscreenTarget>>) {
    // `--center` only applies offscreen (a live camera follows the player).
    let at = options.center.filter(|_| offscreen.is_some()).map_or(join.player_spawn, Vec2::from);
    let focus = Vec3::new(at.x, 0.0, at.y);
    let zoom = options.zoom.unwrap_or(26.0).clamp(MIN_ZOOM, MAX_ZOOM);
    let mut cam = IsoCamera::new(options.angle, zoom, focus);
    // `--first-person`, else (in a window) the view the settings screen says to start in.
    if options.third_person || (!options.offscreen() && store.is_some_and(|s| s.settings.first_person)) {
        // `follow` puts it behind the survivor at the first frame.
        cam.toggle_view();
    }
    let transform = Transform::from_translation(cam.focus + cam.offset_dir() * DISTANCE).looking_at(cam.focus, Vec3::Y);
    // The dot at the middle of the screen while the pointer is held there (first person).
    commands.spawn((
        CentreDot,
        Node { position_type: PositionType::Absolute, left: percent(50), top: percent(50), width: px(6), height: px(6), margin: UiRect::all(px(-3)), border: UiRect::all(px(1)), border_radius: BorderRadius::MAX, ..default() },
        BackgroundColor(Color::srgb(0.95, 0.9, 0.79)),
        BorderColor::all(Color::srgba(0.08, 0.08, 0.08, 0.85)),
        Visibility::Hidden,
        Pickable::IGNORE,
    ));
    let occlusion = graphics.occlusion.then(occlusion);
    let mut camera = commands.spawn((
        Camera3d::default(),
        cam.projection(),
        transform,
        // Window glass is drawn with alpha to coverage, which needs multisampling (graphics
        // preset: 4 from Medium up).
        crate::graphics_ui::camera_msaa(&graphics),
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
    if let Some(occlusion) = occlusion {
        camera.insert(occlusion);
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

/// How far ambient occlusion looks for what shades a point, in metres: enough for a car's
/// underside, a doorway's corners and the foot of a wall, not so far that a whole house darkens.
const OCCLUSION_RADIUS: f32 = 1.1;

/// Ambient occlusion (a graphics setting) and SMAA, which smooths edges in place of the
/// multisampling SSAO can't run with (`graphics_ui::camera_msaa`).
pub fn occlusion() -> (ScreenSpaceAmbientOcclusion, Smaa) {
    (
        ScreenSpaceAmbientOcclusion { quality_level: ScreenSpaceAmbientOcclusionQualityLevel::Medium, radius: OCCLUSION_RADIUS, constant_object_thickness: 0.4 },
        Smaa { preset: SmaaPreset::High },
    )
}

/// Puts our copy of Bevy's SSAO shader (`ssao_ortho.wesl`, right for an orthographic camera) in
/// place of Bevy's own. Called after `DefaultPlugins` registered theirs and before the render
/// pipelines load it.
pub fn patch_ssao_shader(app: &mut App) {
    use bevy::asset::io::embedded::EmbeddedAssetRegistry;
    let registry = app.world_mut().resource_mut::<EmbeddedAssetRegistry>();
    registry.insert_asset(std::path::PathBuf::new(), std::path::Path::new("bevy_pbr/ssao/ssao.wesl"), include_bytes!("ssao_ortho.wesl").as_slice());
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
    inventory: Option<Res<crate::inventory_ui::InventoryPanel>>,
    motion: Res<AccumulatedMouseMotion>,
    lock: Res<PointerLock>,
    notice: Option<ResMut<crate::saving::Notice>>,
    mut turned: Local<usize>,
) {
    let Ok((mut cam, mut projection)) = cams.single_mut() else { return };
    // (The inventory screen covers the window: the wheel scrolls its columns.)
    let inventory_open = inventory.is_some_and(|i| i.open);
    // `;` switches between the fixed view and first person (not in a picture: `--first-person`).
    if keys.just_pressed(KeyCode::Semicolon) && !options.offscreen() {
        cam.toggle_view();
        *projection = cam.projection();
        if let Some(mut notice) = notice {
            notice.say(time.elapsed_secs(), if cam.shoulder() { "First-person view (; goes back, hold Alt for the pointer)" } else { "View from above (; for first person)" }.to_string());
        }
    }
    if cam.shoulder() {
        // The mouse turns the camera while the pointer is held; the wheel moves it in and out.
        if lock.0 {
            cam.look(motion.delta);
        }
        if crate::hud::panel_under_cursor(window.single().ok(), &panels).is_some() || browser.is_some_and(|b| b.open) || inventory_open {
            wheel.clear();
        }
        wheel.clear();
        return;
    }
    // Over a panel the wheel scrolls it (`hud.rs`), not the zoom; so with the server browser open.
    if crate::hud::panel_under_cursor(window.single().ok(), &panels).is_some() || browser.is_some_and(|b| b.open) || inventory_open {
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
    map: Option<Res<ClientMap>>,
    mut player: Query<(&Transform, &Player, &mut Visibility), Without<IsoCamera>>,
    mut cams: Query<(&mut IsoCamera, &mut Transform)>,
) {
    let Ok((mut cam, mut transform)) = cams.single_mut() else { return };
    let cam = &mut *cam;
    // Our own survivor is hidden in first person.
    if let Ok((.., mut shown)) = player.single_mut() {
        let want = if cam.too_close() { Visibility::Hidden } else { Visibility::Inherited };
        if *shown != want {
            *shown = want;
        }
    }
    if cam.shoulder() {
        if let Ok((p, player, _)) = player.single() {
            if cam.behind {
                // Behind him as he stands; in a picture `--angle` turns it on round him by quarters.
                cam.yaw = yaw_behind(player.facing) + if options.offscreen() { FRAC_PI_2 * options.angle } else { 0.0 };
                (cam.behind, cam.focus) = (false, p.translation.with_y(view.height));
            }
            // Close behind him: a loose follow would swing him about the screen.
            let t = 1.0 - (-24.0 * time.delta_secs()).exp();
            cam.focus = cam.focus.lerp(p.translation.with_y(view.height), t);
        }
        *transform = cam.shoulder_transform(map.as_deref().map(|m| &m.0), time.delta_secs());
    } else {
        // An offscreen render keeps the camera where it was put (on the player, or `--center`).
        if let Some((p, ..)) = player.single().ok().filter(|_| !options.offscreen()) {
            let t = 1.0 - (-8.0 * time.delta_secs()).exp();
            cam.focus = cam.focus.lerp(p.translation.with_y(view.height), t);
        }
        *transform = Transform::from_translation(cam.focus + cam.offset_dir() * DISTANCE).looking_at(cam.focus, Vec3::Y);
    }
    if shake.trauma > 0.0 {
        // A small jitter in the screen plane, strongest right after a hit.
        let s = time.elapsed_secs() * 40.0;
        let amount = shake.trauma * shake.trauma * 0.3 * cam.view_metres() / 26.0;
        let jitter = Vec2::new((s * 1.3).sin() + (s * 2.9).sin() * 0.5, (s * 1.7).cos() + (s * 3.3).sin() * 0.5) * amount;
        let (right, up) = (transform.right(), transform.up());
        transform.translation += right * jitter.x + up * jitter.y;
    }
}

/// First person: hold the pointer at the middle of the window (hidden; a dot marks it), so the
/// mouse turns the camera and everything that asks where the pointer is gets that middle point.
/// It comes free while something that needs a pointer is up, once dead, and while Alt is held.
#[allow(clippy::too_many_arguments)]
fn lock_pointer(
    keys: Res<ButtonInput<KeyCode>>,
    cams: Query<&IsoCamera>,
    me: Option<Res<crate::player::LocalPlayer>>,
    auto: Option<Res<crate::autoplay::AutoInput>>,
    inventory: Option<Res<crate::inventory_ui::InventoryPanel>>,
    menu: Option<Res<crate::inventory_ui::ItemMenu>>,
    chat: Option<Res<crate::chat::ChatEntry>>,
    browser: Option<Res<crate::browser_ui::BrowserUi>>,
    pause: Option<Res<crate::pause_menu::PauseMenu>>,
    map: Option<Res<crate::map_ui::MapUi>>,
    mut windows: Query<(&mut Window, &mut CursorOptions), With<PrimaryWindow>>,
    mut dots: Query<&mut Visibility, With<CentreDot>>,
    mut lock: ResMut<PointerLock>,
    mut grab_next: Local<bool>,
) {
    let Ok((mut window, mut cursor)) = windows.single_mut() else { return };
    let free = keys.any_pressed([KeyCode::AltLeft, KeyCode::AltRight])
        || !window.focused
        || me.is_some_and(|m| m.dead)
        || auto.is_some_and(|a| a.active)
        || inventory.is_some_and(|p| p.open)
        || menu.is_some_and(|m| m.is_up())
        || chat.is_some_and(|c| c.typing)
        || browser.is_some_and(|b| b.open)
        || pause.is_some_and(|p| p.open)
        || map.is_some_and(|m| m.open);
    let hold = cams.single().is_ok_and(|c| c.shoulder()) && !free;
    let middle = Vec2::new(window.width(), window.height()) * 0.5;
    if lock.0 != hold {
        lock.0 = hold;
        if hold {
            // Put it in the middle first and grab it on the next frame: on macOS moving a grabbed
            // pointer lets it go again.
            window.set_cursor_position(Some(middle));
            *grab_next = true;
        } else {
            *grab_next = false;
            cursor.grab_mode = CursorGrabMode::None;
        }
    } else if hold && *grab_next {
        *grab_next = false;
        cursor.grab_mode = CursorGrabMode::Locked;
    } else if hold && !cfg!(target_os = "macos") {
        // Where the pointer can only be kept in the window (X11), put it back each frame. Never on
        // macOS: a grabbed pointer stays put there, and moving it frees it and swallows the mouse's
        // movement for a moment, so the view shook and didn't turn.
        if window.cursor_position().is_none_or(|c| c.distance(middle) > 1.5) {
            window.set_cursor_position(Some(middle));
        }
    }
    let shown = if hold { Visibility::Inherited } else { Visibility::Hidden };
    for mut v in &mut dots {
        if *v != shown {
            *v = shown;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// First person: the camera starts behind the survivor whichever way he faces, W is the way
    /// it looks, the mouse turns it the way it moves, and back in the fixed view it settles on
    /// the nearest of the four angles.
    #[test]
    fn first_person_is_at_the_eyes_and_turns_with_the_mouse() {
        for k in 0..16 {
            let facing = k as f32 * std::f32::consts::TAU / 16.0 - 3.0;
            let mut cam = IsoCamera::new(0.0, 26.0, Vec3::new(40.0, 0.0, 55.0));
            cam.toggle_view();
            assert!(cam.shoulder() && cam.behind);
            cam.yaw = yaw_behind(facing);
            let ahead = Vec3::new(facing.cos(), 0.0, facing.sin());
            let (forward, right) = cam.ground_axes();
            assert!(forward.distance(ahead) < 1e-4, "W is not the way he faces at {facing}");
            let tf = cam.shoulder_transform(None, 0.016);
            // At his eyes, looking the way he faces, and he isn't drawn.
            let rel = tf.translation - cam.focus;
            assert!(rel.with_y(0.0).length() < 0.2 && (rel.y - SHOULDER_HEIGHT).abs() < 1e-4 && cam.too_close(), "{rel}");
            assert!(tf.forward().with_y(0.0).normalize().distance(ahead) < 1e-4);
            // Mouse right: the view turns right. Mouse down: it looks further down.
            let before = *tf.forward();
            cam.look(Vec2::new(40.0, 0.0));
            let after = *cam.shoulder_transform(None, 0.016).forward();
            assert!(after.dot(right) > before.dot(right) + 0.05);
            cam.look(Vec2::new(0.0, 60.0));
            assert!(cam.shoulder_transform(None, 0.016).forward().y < after.y - 0.05);
            cam.look(Vec2::new(0.0, 1e6));
            assert_eq!(cam.pitch, PITCH_RANGE.1);
            // Back: an eased turn to the nearest fixed angle, never more than an eighth of a turn.
            cam.toggle_view();
            assert!(!cam.shoulder() && cam.turning());
            assert!((cam.target_yaw() - cam.yaw).abs() <= std::f32::consts::FRAC_PI_4 + 1e-4);
            cam.advance(TURN_SECS);
            assert_eq!(cam.yaw, cam.target_yaw());
        }
    }

    /// First person: the aim is the ground under the middle of the screen, never nearer than a
    /// few metres ahead of the survivor, and far off the way the camera looks when on the sky.
    #[test]
    fn behind_him_the_camera_looks_the_way_he_faces() {
        // The fog of war's cone goes by the camera in first person and by the body from above: the
        // two must agree when the camera stands behind him.
        let mut cam = IsoCamera::new(0.0, 26.0, Vec3::ZERO);
        for f in [-2.5f32, -1.0, 0.0, 0.7, 1.6, 3.0] {
            cam.yaw = yaw_behind(f);
            let (forward, _) = cam.ground_axes();
            assert!(Vec2::new(forward.x, forward.z).distance(Vec2::from_angle(f)) < 1e-4, "facing {f}");
        }
    }

    #[test]
    fn first_person_aims_ahead() {
        let mut cam = IsoCamera::new(0.0, 26.0, Vec3::ZERO);
        cam.toggle_view();
        cam.yaw = yaw_behind(0.0);
        for pitch in [-1.0, -0.3, 0.0, 0.1, 0.24, 0.6, 1.0] {
            cam.pitch = pitch;
            let tf = cam.shoulder_transform(None, 0.016);
            let at = cam.aim_ground(tf.translation, *tf.forward()).expect("an aim");
            // East of him (the way he faces), nearly straight ahead.
            assert!(at.y == 0.0 && at.x >= AIM_NEAR - 0.1 && at.x <= AIM_FAR + 0.2 && at.z.abs() < 0.6, "pitch {pitch}: {at}");
        }
        // The fixed view: plainly where the ray comes down.
        cam.toggle_view();
        let at = cam.aim_ground(Vec3::new(1.0, 10.0, 2.0), Vec3::new(0.0, -1.0, 0.0)).unwrap();
        assert!(at.distance(Vec3::new(1.0, 0.0, 2.0)) < 1e-5);
        assert_eq!(cam.aim_ground(Vec3::Y, Vec3::X), None);
    }

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
