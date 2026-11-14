//! Render scale (a graphics setting): on a HiDPI screen the 3D world is drawn at a fraction of the
//! window's pixels and scaled up smoothly, while the UI stays at full resolution, sharp.
//!
//! At scale 1 the world camera draws straight to the window and is the UI's camera, as it always
//! was. Below 1 it draws to an image of the scaled size (with a light contrast-adaptive sharpen so
//! the upscale doesn't look soft), and a second camera on the window shows that image behind the
//! UI, which it draws itself at the window's own resolution.
//!
//! Everything that turns a cursor into a point in the world, or a point in the world into a place
//! on screen, goes through [`to_view`] and [`from_view`] so it works at any scale.
//!
//! The scale applies only where it pays and doesn't show: on a screen with a scale factor of 1.5 or
//! more (Retina), the preset's scale; on an ordinary screen, a scale this far below 1 would be
//! visibly soft, so it is raised toward 1 ([`effective`]). Offscreen renders ignore it.
//!
//! **In full screen the Display tab's Resolution list caps it** ([`cap`], [`choices`]): the 3D
//! resolution, `ClientSettings::fullscreen_scale`. Left on Auto it follows the graphics preset
//! ([`preset_lines`]): the screen's own pixels at High and Ultra (the default, so the scaled path
//! is only taken when asked for), 1080 lines at Medium, 900 at Low. A window is drawn at its own
//! size. A 6K screen at 2x is 20 million pixels a frame, ten times 1080p: measured on an M1 Max in
//! full screen at Medium, 31 fps at full size, 75 fps at half. `--render-scale` sets the scale
//! outright.
use bevy::anti_alias::contrast_adaptive_sharpening::ContrastAdaptiveSharpening;
use bevy::asset::RenderAssetUsages;
use bevy::camera::{ClearColorConfig, RenderTarget};
use bevy::image::ImageSampler;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat, TextureUsages};
use bevy::window::{PrimaryWindow, WindowMode, WindowRef};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::camera::IsoCamera;
use crate::graphics::Graphics;

/// The sharpening used while scaled down (CAS, 0..1).
const SHARPEN: f32 = 0.35;

/// The shares the list offers, biggest first: the halves and quarters a 2x screen upscales cleanly.
pub const STEPS: [f32; 4] = [1.0, 0.75, 2.0 / 3.0, 0.5];
/// The lines the list offers below [`STEPS`], where the screen is big enough for them.
pub const LINES: [f32; 3] = [1440.0, 1080.0, 900.0];
/// The least a picked full-screen resolution can be.
pub const MIN_SCALE: f32 = 0.2;

/// The scale the world is drawn at now (1: the window's own pixels), for the performance monitor.
#[derive(Resource, Clone, Copy)]
pub struct ScaleNow {
    pub scale: f32,
    pub size: UVec2,
}

/// The full-screen window's pixels as last seen (width << 32 | height; 0: not known), so the
/// settings screen can list resolutions for it.
static SCREEN: AtomicU64 = AtomicU64::new(0);

/// The screen's pixels, once a full-screen window has been seen.
pub fn screen() -> Option<UVec2> {
    let v = SCREEN.load(Ordering::Relaxed);
    (v != 0).then(|| UVec2::new((v >> 32) as u32, v as u32))
}

/// The lines a graphics preset draws the world at in full screen when the Resolution is Auto
/// (`None`: the screen's own).
pub fn preset_lines(preset: crate::graphics::Preset) -> Option<f32> {
    use crate::graphics::Preset;
    match preset {
        Preset::Low => Some(900.0),
        Preset::Medium => Some(1080.0),
        Preset::High | Preset::Ultra | Preset::Auto | Preset::Custom => None,
    }
}

/// Auto's share of a screen of `physical` pixels for a preset.
pub fn auto_scale(physical: UVec2, preset: crate::graphics::Preset) -> f32 {
    match preset_lines(preset) {
        Some(lines) => (lines / physical.y.max(1) as f32).clamp(MIN_SCALE, 1.0),
        None => 1.0,
    }
}

/// The size the world is drawn at for a share of `physical`.
pub fn size_at(physical: UVec2, scale: f32) -> UVec2 {
    (physical.as_vec2() * scale).round().as_uvec2().max(UVec2::ONE)
}

/// The resolutions offered in full screen on a screen of `physical` pixels, as shares of it,
/// smallest first: [`STEPS`], and [`LINES`] where those are smaller still.
pub fn choices(physical: UVec2) -> Vec<f32> {
    let mut v: Vec<f32> = STEPS.to_vec();
    for lines in LINES {
        let s = lines / physical.y.max(1) as f32;
        if s < STEPS[STEPS.len() - 1] - 0.02 && s >= MIN_SCALE {
            v.push(s);
        }
    }
    v.sort_by(f32::total_cmp);
    v.dedup_by(|a, b| (*a - *b).abs() < 0.01);
    v
}

/// The most the screen lets the world be drawn at: `forced` (`--render-scale`) is no cap, nor is a
/// window; in full screen the player's pick, else the preset's ([`auto_scale`]).
pub fn cap(physical: UVec2, preset: crate::graphics::Preset, fullscreen: bool, picked: Option<f32>, forced: bool) -> f32 {
    if forced || !fullscreen {
        return 1.0;
    }
    match picked {
        Some(p) if p.is_finite() => p.clamp(MIN_SCALE, 1.0),
        _ => auto_scale(physical, preset),
    }
}

pub struct RenderScalePlugin;

impl Plugin for RenderScalePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PostUpdate, apply.run_if(crate::live).before(bevy::camera::CameraUpdateSystems));
    }
}

/// The scale actually used for a setting on a screen with this scale factor: the setting on HiDPI
/// screens, and on a screen of scale factor 1 a scale a third as far below 1 (0.75 there is 0.92).
pub fn effective(setting: f32, scale_factor: f32) -> f32 {
    let s = setting.clamp(0.5, 1.0);
    if scale_factor >= 1.5 { s } else { 1.0 - (1.0 - s) / 3.0 }
}

/// The world camera's viewport size over the window's (1 at full scale).
fn ratio(camera: &Camera, window: &Window) -> f32 {
    match camera.logical_viewport_size() {
        Some(size) if window.width() > 0.0 => size.x / window.width(),
        _ => 1.0,
    }
}

/// A point in the window (logical pixels, as `Window::cursor_position` gives) in the world camera's
/// viewport.
pub fn to_view(camera: &Camera, window: &Window, p: Vec2) -> Vec2 {
    p * ratio(camera, window)
}

/// A point in the world camera's viewport (as `Camera::world_to_viewport` gives) in the window.
pub fn from_view(camera: &Camera, window: &Window, p: Vec2) -> Vec2 {
    p / ratio(camera, window)
}

/// The image the world is drawn into while scaled, and the camera that shows it with the UI.
#[derive(Resource)]
struct Scaled {
    image: Handle<Image>,
    ui_camera: Entity,
    backdrop: Entity,
    size: UVec2,
}

#[derive(Component)]
struct Backdrop;

fn target_image(size: UVec2) -> Image {
    let mut image = Image::new_fill(
        Extent3d { width: size.x, height: size.y, depth_or_array_layers: 1 },
        TextureDimension::D2,
        &[0, 0, 0, 255],
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    );
    image.texture_descriptor.usage = TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST | TextureUsages::RENDER_ATTACHMENT;
    image.sampler = ImageSampler::linear();
    image
}

#[allow(clippy::too_many_arguments)]
fn apply(
    mut commands: Commands,
    graphics: Res<Graphics>,
    store: Option<Res<crate::ui_scale::SettingsStore>>,
    options: Option<Res<crate::Options>>,
    now: Option<ResMut<ScaleNow>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    mut images: ResMut<Assets<Image>>,
    scaled: Option<ResMut<Scaled>>,
    mut world_cam: Query<(Entity, &mut RenderTarget), With<IsoCamera>>,
    mut backdrops: Query<&mut ImageNode, With<Backdrop>>,
) {
    let (Ok(window), Ok((cam, mut target))) = (windows.single(), world_cam.single_mut()) else { return };
    let full = window.physical_size();
    let fullscreen = !matches!(window.mode, WindowMode::Windowed);
    if fullscreen && full.x > 0 && full.y > 0 {
        SCREEN.store((full.x as u64) << 32 | full.y as u64, Ordering::Relaxed);
    }
    let forced = options.as_ref().is_some_and(|o| o.render_scale.is_some());
    let picked = store.as_ref().and_then(|s| s.settings.fullscreen_scale);
    let scale = effective(graphics.render_scale, window.scale_factor()).min(cap(full, graphics.preset, fullscreen, picked, forced));
    let size = size_at(full, scale);
    let shown = ScaleNow { scale, size: if scale < 0.999 { size } else { full } };
    match now {
        Some(mut n) if (n.scale, n.size) != (shown.scale, shown.size) => *n = shown,
        Some(_) => {}
        None => commands.insert_resource(shown),
    }
    let want_scaled = scale < 0.999 && full.x > 0 && full.y > 0;
    match (want_scaled, scaled) {
        (true, None) => {
            let image = images.add(target_image(size));
            *target = RenderTarget::Image(image.clone().into());
            commands.entity(cam).remove::<IsDefaultUiCamera>().insert(ContrastAdaptiveSharpening { enabled: true, sharpening_strength: SHARPEN, denoise: false });
            let ui_camera = commands
                .spawn((Camera2d, Camera { order: 1, clear_color: ClearColorConfig::Custom(Color::BLACK), ..default() }, RenderTarget::Window(WindowRef::Primary), Msaa::Off, IsDefaultUiCamera))
                .id();
            let backdrop = commands
                .spawn((Backdrop, ImageNode::new(image.clone()), Node { position_type: PositionType::Absolute, width: Val::Percent(100.0), height: Val::Percent(100.0), ..default() }, GlobalZIndex(i32::MIN), Pickable::IGNORE))
                .id();
            commands.insert_resource(Scaled { image, ui_camera, backdrop, size });
        }
        (true, Some(mut s)) if s.size != size => {
            // The window or the setting changed: a new image of the new size.
            let image = images.add(target_image(size));
            *target = RenderTarget::Image(image.clone().into());
            if let Ok(mut node) = backdrops.get_mut(s.backdrop) {
                node.image = image.clone();
            }
            images.remove(&s.image);
            s.image = image;
            s.size = size;
        }
        (false, Some(s)) => {
            *target = RenderTarget::Window(WindowRef::Primary);
            commands.entity(cam).insert(IsDefaultUiCamera).remove::<ContrastAdaptiveSharpening>();
            commands.entity(s.ui_camera).despawn();
            commands.entity(s.backdrop).despawn();
            images.remove(&s.image);
            commands.remove_resource::<Scaled>();
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_only_bites_on_hidpi() {
        assert_eq!(effective(0.75, 2.0), 0.75);
        assert_eq!(effective(1.0, 2.0), 1.0);
        assert!((effective(0.75, 1.0) - 0.9167).abs() < 1e-3);
        assert_eq!(effective(0.2, 2.0), 0.5);
    }

    #[test]
    fn auto_follows_the_preset_and_full_size_is_the_default() {
        use crate::graphics::Preset;
        let six_k = UVec2::new(6016, 3384);
        assert_eq!(auto_scale(six_k, Preset::High), 1.0);
        assert_eq!(auto_scale(six_k, Preset::Ultra), 1.0);
        assert_eq!(size_at(six_k, auto_scale(six_k, Preset::Medium)).y, 1080);
        assert_eq!(size_at(six_k, auto_scale(six_k, Preset::Low)).y, 900);
        // A 1080p screen is the same at Medium; nothing is drawn bigger than the screen.
        assert_eq!(auto_scale(UVec2::new(1920, 1080), Preset::Medium), 1.0);
        assert_eq!(auto_scale(UVec2::new(1280, 720), Preset::Low), 1.0);
        // The pick wins in full screen; a window and --render-scale are not capped.
        assert_eq!(cap(six_k, Preset::Medium, true, Some(1.0), false), 1.0);
        assert_eq!(cap(six_k, Preset::High, true, Some(0.5), false), 0.5);
        assert_eq!(cap(six_k, Preset::Medium, false, Some(0.5), false), 1.0);
        assert_eq!(cap(six_k, Preset::High, true, None, false), 1.0);
        assert_eq!(cap(six_k, Preset::Medium, true, None, true), 1.0);
        assert_eq!(cap(six_k, Preset::High, true, Some(0.01), false), MIN_SCALE);
    }

    #[test]
    fn full_screen_offers_the_steps_and_the_presets_lines() {
        let c = choices(UVec2::new(6016, 3384));
        let sizes: Vec<UVec2> = c.iter().map(|s| size_at(UVec2::new(6016, 3384), *s)).collect();
        assert_eq!(sizes.first().map(|s| s.y), Some(900));
        assert!(sizes.iter().any(|s| s.y == 1080) && sizes.iter().any(|s| s.y == 1440));
        assert_eq!(sizes.last(), Some(&UVec2::new(6016, 3384)));
        assert!(sizes.contains(&UVec2::new(3008, 1692)) && sizes.contains(&UVec2::new(4512, 2538)));
        assert!(c.windows(2).all(|w| w[0] < w[1]));
        // A 1080p screen has no lines to add below half.
        assert_eq!(choices(UVec2::new(1920, 1080)), STEPS.iter().rev().copied().collect::<Vec<_>>());
    }
}
