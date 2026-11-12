//! The brand: the Zombye logo on the main menu and the loading screen, the road Z in the top
//! bar's mark, the window's icon.
//!
//! The name and logos are trade marks of Grillo Ltd and are **not** open source
//! (docs/brand/LICENSE-brand.txt), so their files are not in this repo. They live in the private
//! art repo and a release build carries them in a folder of their own:
//!
//! ```text
//! release-brand/
//!   logo.png        the whole logo: main menu, loading screen
//!   wordmark.png    the word alone: menu screens with no room for the logo
//!   mark.png        the road Z: the top bar's mark
//!   icon.png        the window's icon (Windows and Linux; a Mac shows its app's own)
//! ```
//!
//! The folder is `$ZOMBYE_RELEASE_BRAND`, else `release-brand` beside the game's binary. Without
//! one, or without a file, the game shows what this repo can: the name as plain text, the UI's
//! own green mark, no window icon. Anyone can build and mod the game from this repo alone; a fork
//! that is published needs a name and logo of its own.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use bevy::asset::{RenderAssetUsages, uuid_handle};
use bevy::ecs::system::NonSendMarker;
use bevy::image::{CompressedImageFormats, ImageSampler, ImageType};
use bevy::prelude::*;
use bevy::render::render_resource::TextureFormat;
use bevy::window::PrimaryWindow;

const LOGO: Handle<Image> = uuid_handle!("6f1f8a52-51c1-4c0e-9a57-2f6f3a1d0cb1");
const WORDMARK: Handle<Image> = uuid_handle!("6f1f8a52-51c1-4c0e-9a57-2f6f3a1d0cb2");
const MARK: Handle<Image> = uuid_handle!("6f1f8a52-51c1-4c0e-9a57-2f6f3a1d0cb3");

/// The name as text, where the logo isn't there to say it.
pub const NAME: &str = "ZOMBYE";
/// What follows the name wherever it is shown as a mark (a trade mark, not yet registered).
pub const TM: &str = "TM";
/// The small print under a title screen.
pub const NOTICE: &str = "ZOMBYE\u{2122} is a trade mark of Grillo Ltd";

/// One of the brand's pictures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    /// The whole logo: the sun and water tower over the word and the road.
    Logo,
    /// The word alone.
    Wordmark,
    /// The road Z.
    Z,
}

impl Mark {
    fn file(self) -> &'static str {
        match self {
            Mark::Logo => "logo.png",
            Mark::Wordmark => "wordmark.png",
            Mark::Z => "mark.png",
        }
    }

    fn handle(self) -> Handle<Image> {
        match self {
            Mark::Logo => LOGO,
            Mark::Wordmark => WORDMARK,
            Mark::Z => MARK,
        }
    }
}

/// The size in pixels of each picture that loaded (`Mark as usize`).
static LOADED: OnceLock<[Option<Vec2>; 3]> = OnceLock::new();

/// The brand folder, if there is one (looked for once).
pub fn root() -> Option<&'static Path> {
    static ROOT: OnceLock<Option<PathBuf>> = OnceLock::new();
    ROOT.get_or_init(|| {
        let found = std::env::var_os("ZOMBYE_RELEASE_BRAND")
            .map(PathBuf::from)
            .or_else(|| std::env::current_exe().ok()?.parent().map(|d| d.join("release-brand")))
            .filter(|p| p.is_dir());
        if let Some(p) = &found {
            println!("release brand: {}", p.display());
        }
        found
    })
    .as_deref()
}

/// `mark` drawn `width` UI pixels wide, as high as its shape makes that: what to put on a node.
/// `None` without the brand folder (or that file): draw the plain fallback instead.
pub fn image(mark: Mark, width: f32) -> Option<(ImageNode, Node)> {
    let size = (*LOADED.get()?)[mark as usize]?;
    Some((ImageNode::new(mark.handle()), Node { width: px(width), height: px(height(size, width)), flex_shrink: 0.0, ..default() }))
}

/// How high a picture of `size` is when drawn `width` wide.
fn height(size: Vec2, width: f32) -> f32 {
    (width * size.y / size.x.max(1.0)).round()
}

fn decode(dir: &Path, file: &str) -> Option<Image> {
    let path = dir.join(file);
    let bytes = std::fs::read(&path).ok()?;
    match Image::from_buffer(&bytes, ImageType::Extension("png"), CompressedImageFormats::NONE, true, ImageSampler::linear(), RenderAssetUsages::default()) {
        Ok(image) => Some(image),
        Err(e) => {
            eprintln!("can't use {}: {e}", path.display());
            None
        }
    }
}

/// The window icon's pixels (RGBA, row by row) and size, until the window has taken them.
#[derive(Resource)]
struct IconPixels(Vec<u8>, UVec2);

/// Loads the brand's pictures, if this build has them. Add it after Bevy's own plugins (it needs
/// the image assets) and before any screen that draws a mark.
pub struct BrandPlugin;

impl Plugin for BrandPlugin {
    fn build(&self, app: &mut App) {
        let Some(dir) = root() else { return };
        let Some(mut images) = app.world_mut().get_resource_mut::<Assets<Image>>() else { return };
        let mut loaded = [None; 3];
        for mark in [Mark::Logo, Mark::Wordmark, Mark::Z] {
            if let Some(image) = decode(dir, mark.file()) {
                loaded[mark as usize] = Some(image.size().as_vec2());
                let _ = images.insert(mark.handle().id(), image);
            }
        }
        let _ = LOADED.set(loaded);
        // (No window offscreen, so no icon.)
        if app.world().get_resource::<crate::Options>().is_some_and(|o| o.offscreen()) {
            return;
        }
        let icon = decode(dir, "icon.png").and_then(|i| i.convert(TextureFormat::Rgba8UnormSrgb)).and_then(|i| Some(IconPixels(i.data.clone()?, i.size())));
        if let Some(icon) = icon {
            app.insert_resource(icon).add_systems(Update, window_icon.run_if(resource_exists::<IconPixels>));
        }
    }
}

/// Give the window its icon, once it exists. (On the main thread: winit's windows live there.)
fn window_icon(_main: NonSendMarker, mut commands: Commands, icon: Res<IconPixels>, window: Query<Entity, With<PrimaryWindow>>) {
    let Ok(entity) = window.single() else { return };
    let set = bevy::winit::WINIT_WINDOWS.with_borrow(|windows| {
        let Some(window) = windows.get_window(entity) else { return false };
        match winit::window::Icon::from_rgba(icon.0.clone(), icon.1.x, icon.1.y) {
            Ok(i) => window.set_window_icon(Some(i)),
            Err(e) => eprintln!("can't use the window icon: {e}"),
        }
        true
    });
    if set {
        commands.remove_resource::<IconPixels>();
    }
}

/// The name as a title, for where there is no logo to draw: "ZOMBYE" `size` high with a small TM
/// at its shoulder.
pub fn text_title(parent: &mut ChildSpawnerCommands, size: f32, ink: Color, dim: Color) {
    parent.spawn((Node { flex_direction: FlexDirection::Row, align_items: AlignItems::FlexStart, column_gap: px(size * 0.05), flex_shrink: 0.0, ..default() }, Pickable::IGNORE)).with_children(|row| {
        row.spawn((Text::new(NAME), crate::theme::head(size), TextColor(ink), TextLayout::no_wrap(), Pickable::IGNORE));
        row.spawn((Text::new(TM), crate::theme::head((size * 0.16).max(11.0)), TextColor(dim), TextLayout::no_wrap(), Node { margin: UiRect::top(px(size * 0.2)), ..default() }, Pickable::IGNORE));
    });
}

/// `mark` (the logo or the wordmark) `width` wide with a small TM at the word's shoulder, or
/// `false` when this build has no such picture.
pub fn title(parent: &mut ChildSpawnerCommands, mark: Mark, width: f32, dim: Color) -> bool {
    let Some((picture, node)) = image(mark, width) else { return false };
    // Where the word's top right corner is in the picture, as fractions of its size.
    let (right, top) = match mark {
        Mark::Logo => (0.965, 0.47),
        _ => (0.985, 0.06),
    };
    let (w, h) = (width, match node.height { Val::Px(h) => h, _ => 0.0 });
    parent.spawn((Node { width: node.width, height: node.height, flex_shrink: 0.0, ..default() }, Pickable::IGNORE)).with_children(|c| {
        c.spawn((picture, Node { width: percent(100), height: percent(100), ..default() }, Pickable::IGNORE));
        c.spawn((Text::new(TM), crate::theme::head((width * 0.03).clamp(11.0, 18.0)), TextColor(dim), TextLayout::no_wrap(), Node { position_type: PositionType::Absolute, left: px(w * right + 3.0), top: px(h * top), ..default() }, Pickable::IGNORE));
    });
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_picture_keeps_its_shape_at_any_width() {
        assert_eq!(height(Vec2::new(1040.0, 678.0), 520.0), 339.0);
        assert_eq!(height(Vec2::new(152.0, 152.0), 38.0), 38.0);
        // (A file that says it is nothing wide is nothing high, not a division by zero.)
        assert!(height(Vec2::new(0.0, 10.0), 38.0).is_finite());
    }

    #[test]
    fn without_the_brand_there_is_nothing_to_draw() {
        // (No plugin added in a test: nothing loaded, so every screen falls back to text.)
        assert!(image(Mark::Logo, 520.0).is_none());
        assert!(image(Mark::Z, 38.0).is_none());
    }
}
