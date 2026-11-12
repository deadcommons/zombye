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
//!
//! **The switch between the two is a move, not a cut** ([`SWITCH_SECS`]): the camera comes down
//! from the fixed view to the survivor's eyes (and back up) along the line of sight, turning to
//! behind him as it goes, while its lens opens from one narrow enough to pass for the fixed
//! view's parallel one to first person's (a dolly zoom: what stands at the survivor keeps
//! growing smoothly, never jumps). `;` again on the way turns it round where it is.
//! **Both views draw through one kind of lens** ([`Lens`], a custom projection): Bevy compiles
//! every material's shaders once for each kind of projection a camera has, so a camera that
//! changed from an orthographic to a perspective one stalled on the first switch while all of
//! them were built again. With one kind there is nothing to build, at the first switch or any.

use std::f32::consts::FRAC_PI_2;

use bevy::camera::primitives::Frustum;
use bevy::camera::{CameraProjection, Hdr, RenderTarget, ScalingMode, ShadowLodOrigin, SubCameraView};
use bevy::math::Vec3A;
use bevy::input::mouse::{AccumulatedMouseMotion, MouseWheel};
use bevy::post_process::bloom::{Bloom, BloomPrefilter};
use bevy::post_process::effect_stack::Vignette;
use bevy::prelude::*;
use bevy::render::view::{ColorGrading, Tonemapping};
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};

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

/// Seconds the switch between the two views takes.
pub const SWITCH_SECS: f32 = 0.75;
/// The lens the switch starts from above with: so narrow, from so far, that the picture is the
/// fixed view's (things 20 m nearer or further are 0.4% bigger or smaller, a pixel or two at the
/// screen's edge: at 2.5 degrees it was 3%, and the picture jumped as the view from above took
/// over). At the default zoom that is 5 km away, so the near plane goes out with the camera.
const SWITCH_FOV: f32 = 0.3_f32 * std::f32::consts::PI / 180.0;
/// The lens the rest of the switch is paced from, and how far through (eased) the narrow start
/// has given way to it: the camera is soon near enough to be past the trees and roofs on its
/// line of sight.
const PACE_FOV: f32 = 2.5_f32 * std::f32::consts::PI / 180.0;
const PACE_FROM: f32 = 0.25;
/// Nearer the eyes than this the survivor isn't drawn (the camera is about to be in his head).
const HIDE_WITHIN: f32 = 0.9;
/// From this far through the switch the world is dressed for first person (walls and roofs up).
const DRESSED_FROM: f32 = 0.85;

/// The camera's projection in either view: the fixed view's parallel one or first person's
/// perspective, as one type behind [`Projection::Custom`], so that changing between them doesn't
/// change the kind of projection the renderer sees (see the module's docs).
#[derive(Debug, Clone)]
pub enum Lens {
    Flat(OrthographicProjection),
    Deep(PerspectiveProjection),
}

impl Lens {
    fn inner(&self) -> &dyn CameraProjection {
        match self {
            Lens::Flat(p) => p,
            Lens::Deep(p) => p,
        }
    }
}

impl CameraProjection for Lens {
    fn get_clip_from_view(&self) -> Mat4 {
        self.inner().get_clip_from_view()
    }
    fn get_clip_from_view_for_sub(&self, sub_view: &SubCameraView) -> Mat4 {
        self.inner().get_clip_from_view_for_sub(sub_view)
    }
    fn update(&mut self, width: f32, height: f32) {
        match self {
            Lens::Flat(p) => p.update(width, height),
            Lens::Deep(p) => p.update(width, height),
        }
    }
    fn far(&self) -> f32 {
        self.inner().far()
    }
    fn get_frustum_corners(&self, z_near: f32, z_far: f32) -> [Vec3A; 8] {
        self.inner().get_frustum_corners(z_near, z_far)
    }
    fn compute_frustum(&self, camera_transform: &GlobalTransform) -> Frustum {
        self.inner().compute_frustum(camera_transform)
    }
}

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
    /// How far through the switch it is: 0 the fixed view, 1 first person, moving toward `view`.
    blend: f32,
    /// The two yaws `yaw` is mixed from by `blend`: the fixed view's (its turns) and first
    /// person's (the mouse's; kept unwrapped, so the mix never goes the long way round).
    fixed_yaw: f32,
    look_yaw: f32,
    /// Held where it is on the way (`--view-mix`, a picture of the switch).
    held: bool,
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
        Self { angle_index: angle.round() as i32, yaw: yaw_of(angle), zoom, focus, turn_from: yaw_of(angle), turn_at: 1.0, view: View::Iso, pitch: PITCH, behind: false, blend: 0.0, fixed_yaw: yaw_of(angle), look_yaw: yaw_of(angle), held: false }
    }

    /// Switch to the other view: the camera sets off for it ([`Self::step`] moves it). Into first
    /// person from the fixed view it goes behind the survivor; back from first person, to the
    /// nearest of the four fixed angles from where it was looking. Asked for on the way, it
    /// turns round and goes back to where it came from.
    pub fn toggle_view(&mut self) {
        match self.view {
            View::Iso => {
                self.view = View::Shoulder;
                if self.blend == 0.0 {
                    (self.behind, self.pitch, self.look_yaw) = (true, PITCH, self.fixed_yaw);
                }
            }
            View::Shoulder => {
                self.view = View::Iso;
                if self.blend == 1.0 {
                    self.angle_index = ((self.look_yaw - std::f32::consts::FRAC_PI_4) / FRAC_PI_2).round() as i32;
                    (self.fixed_yaw, self.turn_at) = (self.target_yaw(), 1.0);
                }
            }
        }
    }

    /// Is first person the view it is in, or on its way to? (The mouse turns it, the pointer is
    /// held: what the player does goes by this from the moment of the key.)
    pub fn shoulder(&self) -> bool {
        self.view == View::Shoulder
    }

    /// Is a switch between the views under way?
    pub fn switching(&self) -> bool {
        self.blend != if self.shoulder() { 1.0 } else { 0.0 }
    }

    /// Is the world to be dressed for first person (walls up, roofs on, no silhouettes or
    /// opening hints)? Only once the camera is nearly down: on its way it looks in from above.
    pub fn low(&self) -> bool {
        self.eased() > DRESSED_FROM
    }

    /// Move a switch under way on by `dt` seconds. At rest nothing is written.
    pub fn step(&mut self, dt: f32) {
        let to = if self.shoulder() { 1.0 } else { 0.0 };
        if self.blend != to && !self.held {
            let by = dt / SWITCH_SECS;
            self.blend = if to > self.blend { (self.blend + by).min(1.0) } else { (self.blend - by).max(0.0) };
            self.mix();
        }
    }

    /// End a switch under way at once (a view to start in, a picture).
    pub fn land(&mut self) {
        self.blend = if self.shoulder() { 1.0 } else { 0.0 };
        self.mix();
    }

    /// How far through the switch the camera is drawn (0 the fixed view, 1 first person): slow
    /// out of either view and slow into the other.
    pub fn eased(&self) -> f32 {
        let u = self.blend;
        u * u * (3.0 - 2.0 * u)
    }

    /// `yaw` from the two views' yaws: exactly either's at rest.
    fn mix(&mut self) {
        self.yaw = match self.blend {
            0.0 => self.fixed_yaw,
            1.0 => self.look_yaw,
            _ => self.fixed_yaw + (self.look_yaw - self.fixed_yaw) * self.eased(),
        };
    }

    /// First person: look along `yaw` (taken the short way round from the fixed view's).
    pub fn face(&mut self, yaw: f32) {
        let turn = (yaw - self.fixed_yaw + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
        self.look_yaw = self.fixed_yaw + turn;
        self.mix();
    }

    /// On the way between the views: the lens's vertical field of view and how far the camera is
    /// from its pivot, `e` (eased) of the way. The metres of world to the screen's height at the
    /// pivot shrink from the zoom to nothing, each moment by about the same share of what is
    /// left (so it reads as one even zoom in, not a wait and a rush: `zoom^(1-e) x CLOSE^e`,
    /// less `CLOSE x e` so it ends on nothing), while the lens opens, evenly in its logarithm,
    /// from [`SWITCH_FOV`] (quickly, then from [`PACE_FOV`]) to first person's and the camera comes in to match: at `e` = 1 it is
    /// at the pivot, the eyes.
    fn lens_at(&self, e: f32) -> (f32, f32) {
        const CLOSE: f32 = 1.5;
        let narrow = (1.0 - e / PACE_FROM).max(0.0);
        let fov = PACE_FOV * (SHOULDER_FOV / PACE_FOV).powf(e) * (SWITCH_FOV / PACE_FOV).powf(narrow * narrow);
        let height = (self.zoom.powf(1.0 - e) * CLOSE.powf(e) - CLOSE * e).max(0.0);
        (fov, height / (2.0 * (fov * 0.5).tan()))
    }

    /// How far the camera is from its pivot when its lens is a perspective one (a switch under
    /// way, or first person: 0); `None` in the fixed view, where the lens is parallel.
    pub fn boom(&self) -> Option<f32> {
        (self.blend > 0.0).then(|| self.lens_at(self.eased()).1)
    }

    /// The projection as the camera is now.
    pub fn projection(&self) -> Projection {
        Projection::custom(match self.boom() {
            None => Lens::Flat(OrthographicProjection { scaling_mode: ScalingMode::FixedVertical { viewport_height: self.zoom }, near: NEAR, far: FAR, ..OrthographicProjection::default_3d() }),
            // (A perspective lens draws to any distance; `far` is only where it stops looking for things to draw.)
            Some(boom) => Lens::Deep(PerspectiveProjection { fov: self.lens_at(self.eased()).0, near: (boom - SHOULDER_FAR).max(NEAR), far: boom + SHOULDER_FAR, ..default() }),
        })
    }

    /// First person: turn by this much mouse (pixels; right and down positive).
    pub fn look(&mut self, by: Vec2) {
        self.look_yaw -= by.x * LOOK_RATE;
        self.pitch = (self.pitch + by.y * LOOK_RATE).clamp(PITCH_RANGE.0, PITCH_RANGE.1);
        self.mix();
    }

    /// How far above level the camera looks down from: the fixed view's elevation, first
    /// person's pitch, and between them on the way (staying high until late, then levelling out).
    fn elevation(&self) -> f32 {
        let e = self.eased();
        ELEVATION + (self.pitch - ELEVATION) * e * e
    }

    /// Unit vector from the pivot toward the camera.
    fn boom_dir(&self) -> Vec3 {
        let el = self.elevation();
        Vec3::new(self.yaw.sin() * el.cos(), el.sin(), self.yaw.cos() * el.cos())
    }

    /// Where the camera is and looks. The fixed view: [`DISTANCE`] back from the focus. First
    /// person: at the survivor's eyes, looking the way the mouse has turned it (no wall can come
    /// between: the eyes are inside his own body's radius). On the way: looking at a pivot that
    /// rises from his feet to his eyes, from as far as the lens of the moment asks.
    pub fn transform(&self) -> Transform {
        let Some(boom) = self.boom() else {
            return Transform::from_translation(self.focus + self.offset_dir() * DISTANCE).looking_at(self.focus, Vec3::Y);
        };
        let (forward, _) = self.ground_axes();
        let dir = self.boom_dir();
        let pivot = self.focus + (Vec3::Y * SHOULDER_HEIGHT + forward * EYES_AHEAD) * self.eased();
        Transform::from_translation(pivot + dir * boom).looking_to(-dir, Vec3::Y)
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

    /// First person, and the last of the way down to it: our own survivor isn't drawn (he would
    /// only be in the way of the view).
    pub fn too_close(&self) -> bool {
        self.boom().is_some_and(|b| b < HIDE_WITHIN)
    }

    /// Metres of world top to bottom of the screen where the survivor stands, in either view.
    pub fn view_metres(&self) -> f32 {
        match self.boom() {
            None => self.zoom,
            Some(boom) => 2.0 * boom.max(1.0) * (self.lens_at(self.eased()).0 * 0.5).tan(),
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
            self.turn_from = self.fixed_yaw;
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
            self.fixed_yaw = eased_yaw(self.turn_from, self.target_yaw(), self.turn_at);
            self.mix();
        }
    }

    /// Unit vector from focus toward the camera. Out of the fixed view no flatter than a quarter
    /// of a radian: the cutaways and fades that go by it take it for the slope of the line of sight.
    pub fn offset_dir(&self) -> Vec3 {
        let e = if self.blend > 0.0 { self.elevation().max(0.25) } else { ELEVATION };
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
        // There from the start (`follow` puts it behind the survivor at the first frame).
        cam.toggle_view();
        cam.land();
    }
    // `--view-mix U`: held that far on the way down to first person, for a picture of the switch.
    if let Some(u) = options.view_mix {
        if !cam.shoulder() {
            cam.toggle_view();
        }
        (cam.blend, cam.held) = (u, true);
        cam.mix();
    }
    let transform = cam.transform();
    // The dot at the middle of the screen while the pointer is held there (first person).
    commands.spawn((
        CentreDot,
        Node { position_type: PositionType::Absolute, left: percent(50), top: percent(50), width: px(6), height: px(6), margin: UiRect::all(px(-3)), border: UiRect::all(px(1)), border_radius: BorderRadius::MAX, ..default() },
        BackgroundColor(Color::srgb(0.95, 0.9, 0.79)),
        BorderColor::all(Color::srgba(0.08, 0.08, 0.08, 0.85)),
        Visibility::Hidden,
        Pickable::IGNORE,
    ));
    let mut camera = commands.spawn((
        Camera3d::default(),
        cam.projection(),
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
    // First person's haze, on from the start and out of reach until then (`daynight.rs` sets it):
    // a camera that gains or loses it has every shader built again.
    camera.insert(bevy::pbr::DistanceFog { falloff: bevy::pbr::FogFalloff::Linear { start: 1.0e6, end: 2.0e6 }, ..default() });
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
    inventory: Option<Res<crate::inventory_ui::InventoryPanel>>,
    motion: Res<AccumulatedMouseMotion>,
    lock: Res<PointerLock>,
    notice: Option<ResMut<crate::saving::Notice>>,
    mut turned: Local<usize>,
    mut viewed: Local<usize>,
) {
    let Ok((mut cam, mut projection)) = cams.single_mut() else { return };
    // (The inventory screen covers the window: the wheel scrolls its columns.)
    let inventory_open = inventory.is_some_and(|i| i.open);
    // `;` switches between the fixed view and first person (not in a picture: `--first-person`).
    if keys.just_pressed(KeyCode::Semicolon) && !options.offscreen() {
        cam.toggle_view();
        if let Some(mut notice) = notice {
            notice.say(time.elapsed_secs(), if cam.shoulder() { "First-person view (; goes back, hold Alt for the pointer)" } else { "View from above (; for first person)" }.to_string());
        }
    }
    // `--view-at SECS,...`: switches by themselves, for recordings and for timing them.
    while options.view_at.get(*viewed).is_some_and(|&t| time.elapsed_secs() >= t) {
        cam.toggle_view();
        *viewed += 1;
    }
    // A switch under way moves on; the lens is set again each frame of it.
    if cam.switching() {
        cam.step(time.delta_secs());
        *projection = cam.projection();
    }
    if cam.shoulder() {
        // The mouse turns the camera while the pointer is held (the wheel does nothing here).
        if lock.0 {
            cam.look(motion.delta);
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
        *projection = cam.projection();
    }
}

fn follow(
    time: Res<Time>,
    options: Res<Options>,
    shake: Res<CameraShake>,
    view: Res<crate::player::ViewFloor>,
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
                cam.face(yaw_behind(player.facing) + if options.offscreen() { FRAC_PI_2 * options.angle } else { 0.0 });
                cam.behind = false;
                if !cam.switching() {
                    cam.focus = p.translation.with_y(view.height + view.ground);
                }
            }
            // Close behind him: a loose follow would swing him about the screen. (On the way
            // down the follow tightens from the fixed view's to this.)
            let t = 1.0 - (-(8.0 + 16.0 * cam.eased()) * time.delta_secs()).exp();
            cam.focus = cam.focus.lerp(p.translation.with_y(view.height + view.ground), t);
        }
        *transform = cam.transform();
    } else {
        // An offscreen render keeps the camera where it was put (on the player, or `--center`).
        if let Some((p, ..)) = player.single().ok().filter(|_| !options.offscreen()) {
            let t = 1.0 - (-(8.0 + 16.0 * cam.eased()) * time.delta_secs()).exp();
            cam.focus = cam.focus.lerp(p.translation.with_y(view.height + view.ground), t);
        }
        *transform = cam.transform();
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
            assert!(cam.shoulder() && cam.behind && cam.switching());
            cam.land();
            cam.face(yaw_behind(facing));
            let ahead = Vec3::new(facing.cos(), 0.0, facing.sin());
            let (forward, right) = cam.ground_axes();
            assert!(forward.distance(ahead) < 1e-4, "W is not the way he faces at {facing}");
            let tf = cam.transform();
            // At his eyes, looking the way he faces, and he isn't drawn.
            let rel = tf.translation - cam.focus;
            assert!(rel.with_y(0.0).length() < 0.2 && (rel.y - SHOULDER_HEIGHT).abs() < 1e-4 && cam.too_close(), "{rel}");
            assert!(tf.forward().with_y(0.0).normalize().distance(ahead) < 1e-4);
            // Mouse right: the view turns right. Mouse down: it looks further down.
            let before = *tf.forward();
            cam.look(Vec2::new(40.0, 0.0));
            let after = *cam.transform().forward();
            assert!(after.dot(right) > before.dot(right) + 0.05);
            cam.look(Vec2::new(0.0, 60.0));
            assert!(cam.transform().forward().y < after.y - 0.05);
            cam.look(Vec2::new(0.0, 1e6));
            assert_eq!(cam.pitch, PITCH_RANGE.1);
            // Back: to the nearest fixed angle, never more than an eighth of a turn away.
            cam.toggle_view();
            assert!(!cam.shoulder() && cam.switching());
            assert!((cam.target_yaw() - cam.yaw).abs() <= std::f32::consts::FRAC_PI_4 + 1e-4);
            cam.step(SWITCH_SECS + 0.01);
            assert!(!cam.switching());
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
        cam.toggle_view();
        cam.land();
        for f in [-2.5f32, -1.0, 0.0, 0.7, 1.6, 3.0] {
            cam.face(yaw_behind(f));
            let (forward, _) = cam.ground_axes();
            assert!(Vec2::new(forward.x, forward.z).distance(Vec2::from_angle(f)) < 1e-4, "facing {f}");
        }
    }

    #[test]
    fn first_person_aims_ahead() {
        let mut cam = IsoCamera::new(0.0, 26.0, Vec3::ZERO);
        cam.toggle_view();
        cam.land();
        cam.face(yaw_behind(0.0));
        for pitch in [-1.0, -0.3, 0.0, 0.1, 0.24, 0.6, 1.0] {
            cam.pitch = pitch;
            let tf = cam.transform();
            let at = cam.aim_ground(tf.translation, *tf.forward()).expect("an aim");
            // East of him (the way he faces), nearly straight ahead.
            assert!(at.y == 0.0 && at.x >= AIM_NEAR - 0.1 && at.x <= AIM_FAR + 0.2 && at.z.abs() < 0.6, "pitch {pitch}: {at}");
        }
        // The fixed view: plainly where the ray comes down.
        cam.toggle_view();
        cam.land();
        let at = cam.aim_ground(Vec3::new(1.0, 10.0, 2.0), Vec3::new(0.0, -1.0, 0.0)).unwrap();
        assert!(at.distance(Vec3::new(1.0, 0.0, 2.0)) < 1e-5);
        assert_eq!(cam.aim_ground(Vec3::Y, Vec3::X), None);
    }

    /// What the lens shows of the ground: the height of the screen in metres at `at`, a point
    /// in front of the camera, and whether that point is in the middle of the screen.
    fn seen(cam: &IsoCamera, at: Vec3) -> (f32, f32) {
        let tf = cam.transform();
        let rel = at - tf.translation;
        let depth = rel.dot(*tf.forward());
        let off = (rel - *tf.forward() * depth).length();
        let Projection::Custom(lens) = cam.projection() else { panic!("the camera's lens is always a custom projection") };
        match lens.get::<Lens>().expect("a Lens") {
            Lens::Flat(_) => (cam.zoom, off),
            Lens::Deep(p) => (2.0 * depth * (p.fov * 0.5).tan(), off),
        }
    }

    /// The switch between the views is a move: it takes `SWITCH_SECS` whatever the frame rate,
    /// starts on the fixed view's picture and ends exactly on first person's, and on the way the
    /// camera never jumps and what stands at its pivot only grows (down) or shrinks (up).
    #[test]
    fn the_switch_between_views_is_a_move() {
        let focus = Vec3::new(40.0, 0.0, 55.0);
        for (fps, facing) in [(240.0, 0.3f32), (60.0, -2.0), (30.0, 3.0), (11.0, 1.2)] {
            let mut cam = IsoCamera::new(1.0, 26.0, focus);
            let rest = cam.transform();
            // Both views draw through one kind of lens (no shaders to build again on a switch).
            assert!(matches!(cam.projection(), Projection::Custom(_)));
            cam.toggle_view();
            cam.face(yaw_behind(facing));
            cam.behind = false;
            // The first frame: the same picture as the fixed view's, from further off.
            cam.step(1e-4);
            let tf = cam.transform();
            assert!(matches!(cam.projection(), Projection::Custom(_)));
            assert!(tf.forward().distance(*rest.forward()) < 1e-3, "{fps}: it looks the same way at first");
            let (height, off) = seen(&cam, focus);
            assert!((height - 26.0).abs() < 0.05 && off < 0.05, "{fps}: {height} m at the survivor, {off} m off the middle");
            // Down: closer every frame, never a jump, he grows on the screen, and he is hidden
            // only once the camera is nearly at his head.
            let (mut last, mut last_height, mut secs) = (tf, height, 1e-4);
            let mut was = cam.boom().unwrap();
            while cam.switching() {
                cam.step(1.0 / fps);
                secs += 1.0 / fps;
                let tf = cam.transform();
                let to = cam.boom().unwrap();
                assert!(to <= was, "{fps}: back from {was} to {to}");
                was = to;
                assert!(tf.forward().angle_between(*last.forward()) < 6.0 * std::f32::consts::PI / (fps * SWITCH_SECS), "{fps}: it swung round in a frame");
                let (fov, boom) = cam.lens_at(cam.eased());
                let height = 2.0 * boom * (fov * 0.5).tan();
                // He grows on the screen, and evenly: no frame takes more of the 26 m than its share
                // of the time allows (at its steepest two and a half times the mean).
                assert!(height <= last_height + 1e-3 && last_height - height <= 26.0 * 2.5 / (fps * SWITCH_SECS) + 0.01, "{fps}: {last_height} then {height}");
                assert_eq!(cam.too_close(), cam.boom().unwrap() < HIDE_WITHIN);
                (last, last_height) = (tf, height);
            }
            assert!(secs >= SWITCH_SECS - 1e-3 && secs < SWITCH_SECS + 1.0 / fps + 1e-3, "{fps} fps: {secs} s");
            // There: at his eyes, looking the way he faces, exactly first person's lens.
            let tf = cam.transform();
            assert!(cam.too_close() && cam.low() && cam.boom() == Some(0.0));
            assert!((tf.translation - focus - Vec3::Y * SHOULDER_HEIGHT).length() < EYES_AHEAD + 1e-4);
            assert!(tf.forward().with_y(0.0).normalize().distance(Vec3::new(facing.cos(), 0.0, facing.sin())) < 1e-4);
            let Projection::Custom(lens) = cam.projection() else { panic!() };
            assert!(matches!(lens.get::<Lens>(), Some(Lens::Deep(p)) if (p.fov - SHOULDER_FOV).abs() < 1e-6));
            // Back up: the fixed view again, exactly, on the angle nearest to where he looked.
            cam.toggle_view();
            let mut up = 0.0;
            while cam.switching() {
                cam.step(1.0 / fps);
                let now = cam.boom().unwrap_or(f32::MAX);
                assert!(now >= up, "{fps}: down again from {up} to {now}");
                up = now;
            }
            assert!(cam.boom().is_none() && !cam.too_close() && !cam.low());
            assert_eq!(cam.yaw, cam.target_yaw());
            assert!((cam.target_yaw() - cam.look_yaw).abs() <= std::f32::consts::FRAC_PI_4 + 1e-4);
            let Projection::Custom(lens) = cam.projection() else { panic!() };
            assert!(matches!(lens.get::<Lens>(), Some(Lens::Flat(_))));
            assert!(cam.transform().translation.distance(focus + cam.offset_dir() * DISTANCE) < 1e-3);
        }
    }

    /// `;` again on the way turns the camera round where it is: no jump, and it ends where it
    /// set out from.
    #[test]
    fn a_switch_turns_round_on_the_way() {
        let mut cam = IsoCamera::new(2.0, 30.0, Vec3::ZERO);
        cam.toggle_view();
        cam.face(yaw_behind(1.0));
        cam.behind = false;
        cam.step(SWITCH_SECS * 0.6);
        let (mid, yaw) = (cam.transform(), cam.yaw);
        cam.toggle_view();
        assert!(!cam.shoulder() && cam.switching());
        assert_eq!((cam.transform().translation, cam.yaw, cam.angle_index), (mid.translation, yaw, 2));
        cam.step(SWITCH_SECS * 0.3);
        cam.toggle_view();
        let again = cam.transform();
        cam.step(1e-4);
        assert!(cam.transform().translation.distance(again.translation) < 0.5);
        cam.toggle_view();
        cam.step(SWITCH_SECS);
        assert!(!cam.switching());
        assert_eq!((cam.yaw, cam.angle_index), (yaw_of(2.0), 2));
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
