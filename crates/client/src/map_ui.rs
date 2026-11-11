//! The map screen (M): the survivor's paper map of the town, over the whole window.
//!
//! The street plan is printed on the sheet from the start; gardens, trees, fences and buildings
//! are inked in where the survivor has been, and a building's rooms once it has been inside
//! (`sim::explore`: the server keeps what each survivor has seen, [`ServerLink`]'s snapshot
//! carries ours). Our survivor is an arrowhead in a ring, where it woke up a cross, the group's
//! survivors in view dots and its markers diamonds.
//!
//! **Drawn per pixel** by `assets/shaders/map.wesl`, a [`UiMaterial`] on one node: a texture with
//! a texel a tile (what the ground is, its edges, what stands on it, whether its building has been
//! entered: [`bake_tiles`]) and one with a texel an explored cell, so the map is sharp at any zoom
//! and costs nothing while it is shut. The look is data: `assets/data/map.ron` ([`MapStyle`]).
//!
//! **Keys:** M opens and shuts it (mute moved to Ctrl+M), Esc shuts it; drag moves the sheet, the
//! wheel zooms about the pointer, a double-click centres on our survivor again. The game goes on
//! behind it and WASD still walks; mouse buttons and the wheel are the map's while it is up.
//! `--map` starts with it open, `--map-zoom PX` and `--map-center X,Y` set the view and `--map-all`
//! shows everything as explored (pictures).

use bevy::asset::RenderAssetUsages;
use bevy::image::ImageSampler;
use bevy::input::InputSystems;
use bevy::input::mouse::{MouseButtonInput, MouseScrollUnit, MouseWheel};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, Extent3d, ShaderType, TextureDimension, TextureFormat};
use bevy::shader::ShaderRef;
use bevy::window::PrimaryWindow;
use serde::Deserialize;
use zombye_sim::explore::{CELL, Explored};
use zombye_sim::map::{Edge, Feature, Floor, TileMap};
use zombye_sim::template::BuildingKind;

use crate::Options;
use crate::player::LocalPlayer;
use crate::server_link::{ClientMap, ServerLink};
use crate::theme::{self, BODY, SMALL, font, strong};

/// How many survivors and markers the sheet shows besides our own (`PINS` in map.wesl).
const PINS: usize = 16;
/// Two presses this close together (seconds) are a double-click.
const DOUBLE_CLICK_SECS: f32 = 0.35;
/// The margin of sheet round the map (metres; `margin` in map.wesl), which the view may show.
const MARGIN: f32 = 14.0;

const SHIPPED: &str = include_str!("../../../assets/data/map.ron");

/// `map.ron`: the map screen's colours and scale.
#[derive(Resource, Clone, Debug, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MapStyle {
    pub paper: String,
    pub ink: String,
    pub table: String,
    pub grass: String,
    pub road: String,
    pub walk: String,
    pub path: String,
    pub dirt: String,
    pub house: String,
    pub shop: String,
    pub office: String,
    pub civic: String,
    pub tree: String,
    pub pine: String,
    pub marker: String,
    pub friend: String,
    pub pin: String,
    /// UI pixels a metre when the map opens, and the closest it goes.
    pub zoom: f32,
    pub zoom_max: f32,
}

impl Default for MapStyle {
    fn default() -> Self {
        let c = |s: &str| s.to_string();
        Self {
            paper: c("#e9dcbb"),
            ink: c("#2b2620"),
            table: c("#141414"),
            grass: c("#d2d6a2"),
            road: c("#f8f0da"),
            walk: c("#e4d8b8"),
            path: c("#dccb9f"),
            dirt: c("#d2bc8e"),
            house: c("#c9a27a"),
            shop: c("#c98372"),
            office: c("#8f9baa"),
            civic: c("#d8b850"),
            tree: c("#8c9a6a"),
            pine: c("#77875e"),
            marker: c("#d7262b"),
            friend: c("#3f8f8a"),
            pin: c("#d8a530"),
            zoom: 3.0,
            zoom_max: 16.0,
        }
    }
}

/// `"#rrggbb"` as sRGB 0..1.
pub fn hex(text: &str) -> Result<Vec4, String> {
    let h = text.trim().strip_prefix('#').unwrap_or(text.trim());
    if h.len() != 6 || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("\"{text}\" is not a colour like \"#e9dcbb\""));
    }
    let v = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).unwrap_or(0) as f32 / 255.0;
    Ok(Vec4::new(v(0), v(2), v(4), 1.0))
}

impl MapStyle {
    pub fn parse(text: &str) -> Result<MapStyle, String> {
        let style: MapStyle = ron::from_str(text).map_err(|e| e.to_string())?;
        for (name, colour) in style.colours() {
            hex(colour).map_err(|e| format!("{name}: {e}"))?;
        }
        if !(style.zoom >= 0.2 && style.zoom <= style.zoom_max && style.zoom_max <= 64.0) {
            return Err(format!("the zoom must have 0.2 <= zoom <= zoom_max <= 64, not {} and {}", style.zoom, style.zoom_max));
        }
        Ok(style)
    }

    /// `map.ron` from the data folder `dir`, the shipped one if it has none.
    pub fn load(dir: &std::path::Path) -> Result<MapStyle, String> {
        let path = dir.join("map.ron");
        match std::fs::read_to_string(&path) {
            Ok(text) => MapStyle::parse(&text).map_err(|e| format!("{}: {e}", path.display())),
            Err(_) => Ok(MapStyle::parse(SHIPPED).expect("the shipped map.ron is good (a test checks)")),
        }
    }

    fn colours(&self) -> [(&'static str, &str); 17] {
        [
            ("paper", &self.paper),
            ("ink", &self.ink),
            ("table", &self.table),
            ("grass", &self.grass),
            ("road", &self.road),
            ("walk", &self.walk),
            ("path", &self.path),
            ("dirt", &self.dirt),
            ("house", &self.house),
            ("shop", &self.shop),
            ("office", &self.office),
            ("civic", &self.civic),
            ("tree", &self.tree),
            ("pine", &self.pine),
            ("marker", &self.marker),
            ("friend", &self.friend),
            ("pin", &self.pin),
        ]
    }

    fn colour(&self, text: &str) -> Vec4 {
        hex(text).unwrap_or(Vec4::ONE)
    }
}

/// The shader's uniform (`MapLook` in map.wesl; keep the two in step).
#[derive(Clone, Copy, Debug, Default, ShaderType)]
pub struct MapLook {
    view: Vec4,
    size: Vec4,
    grid: Vec4,
    me: Vec4,
    home: Vec4,
    pins: [Vec4; PINS],
    paper: Vec4,
    ink: Vec4,
    table: Vec4,
    grass: Vec4,
    road: Vec4,
    walk: Vec4,
    path: Vec4,
    dirt: Vec4,
    house: Vec4,
    shop: Vec4,
    office: Vec4,
    civic: Vec4,
    tree: Vec4,
    pine: Vec4,
    marker: Vec4,
    friend: Vec4,
    pin: Vec4,
}

#[derive(Asset, TypePath, AsBindGroup, Clone, Debug)]
pub struct MapMaterial {
    #[uniform(0)]
    look: MapLook,
    #[texture(1)]
    tiles: Handle<Image>,
    #[texture(2)]
    #[sampler(3)]
    seen: Handle<Image>,
}

impl UiMaterial for MapMaterial {
    fn fragment_shader() -> ShaderRef {
        "shaders/map.wesl".into()
    }
}

/// The map screen's state.
#[derive(Resource, Default)]
pub struct MapUi {
    pub open: bool,
    /// The world point in the middle of the screen (metres).
    centre: Vec2,
    /// UI pixels a metre (0: not set yet, takes the style's).
    zoom: f32,
    /// The pointer (logical pixels) while the left button drags the sheet.
    drag: Option<Vec2>,
    last_press: f32,
    /// What the textures were last made from.
    baked: Option<Baked>,
    shown: Option<Explored>,
    material: Handle<MapMaterial>,
    tiles: Handle<Image>,
    seen: Handle<Image>,
}

#[derive(Clone, PartialEq)]
struct Baked {
    size: IVec2,
    entered: Vec<u16>,
}

impl MapUi {
    /// The closest and furthest zooms for a window this many UI pixels across and a map this big.
    fn zoom_range(style: &MapStyle, window: Vec2, map: Vec2) -> (f32, f32) {
        let whole = (window / (map + Vec2::splat(MARGIN * 2.0 + 8.0))).min_element();
        let min = whole.clamp(0.05, style.zoom_max);
        (min, style.zoom_max.max(min))
    }

    /// Keeps the middle of the screen on the sheet.
    fn keep_on_sheet(&mut self, map: Vec2) {
        self.centre = self.centre.clamp(Vec2::splat(-MARGIN), map + Vec2::splat(MARGIN));
    }
}

#[derive(Component)]
struct MapRoot;
#[derive(Component)]
struct MapSheet;
#[derive(Component)]
struct ScaleBar;
#[derive(Component)]
struct ScaleText;
#[derive(Component)]
struct SeenText;

pub struct MapUiPlugin;

impl Plugin for MapUiPlugin {
    fn build(&self, app: &mut App) {
        let style = match MapStyle::load(&app.world().resource::<Options>().data) {
            Ok(style) => style,
            Err(e) => {
                eprintln!("can't use the map's look: {e}");
                std::process::exit(1);
            }
        };
        app.insert_resource(style)
            .init_resource::<MapUi>()
            .add_plugins(UiMaterialPlugin::<MapMaterial>::default())
            .add_systems(Startup, spawn)
            .add_systems(PreUpdate, input.after(InputSystems).after(crate::inventory_ui::TextEntry).run_if(crate::live))
            .add_systems(Update, show);
    }
}

fn blank(width: u32, height: u32, format: TextureFormat, bytes: usize, sampler: ImageSampler) -> Image {
    let mut image = Image::new(
        Extent3d { width: width.max(1), height: height.max(1), depth_or_array_layers: 1 },
        TextureDimension::D2,
        vec![0; width.max(1) as usize * height.max(1) as usize * bytes],
        format,
        RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
    );
    image.sampler = sampler;
    image
}

// The frame every full screen shares (docs/ui-design.md): flat dark chrome round the paper. These
// are the guide's tokens, named here until `theme.rs` has them.
/// How much of the game the veil round the sheet hides (the UI blends in linear light, where a
/// few per cent of a sunlit street is a lot: `theme::PANEL_ALPHA`).
const VEIL: f32 = 0.93;
/// The top bar's and the footer's height (UI pixels).
const TOP_BAR: f32 = 76.0;
const FOOTER: f32 = 46.0;
/// The frame's fill (brand near-black, see-through: the game goes on behind it), its text, its
/// secondary text and its hairlines.
const CHROME: Color = Color::srgba(0.078, 0.078, 0.078, theme::PANEL_ALPHA);
const BONE: Color = Color::srgb(0.949, 0.902, 0.788);
const CHROME_DIM: Color = Color::srgb(0.725, 0.69, 0.604);
const HAIRLINE: Color = Color::srgba(0.949, 0.902, 0.788, 0.11);

/// A key to press and what it does, as the footer shows them.
fn key_hint(c: &mut ChildSpawnerCommands, key: &str, does: &str) {
    c.spawn((Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(8), ..default() }, Pickable::IGNORE)).with_children(|row| {
        row.spawn((Node { padding: UiRect::axes(px(7), px(1)), border: UiRect::all(px(1)), border_radius: BorderRadius::all(px(4)), ..default() }, BorderColor::all(Color::srgba(0.949, 0.902, 0.788, 0.35)), Pickable::IGNORE)).with_children(|k| {
            k.spawn((Text::new(key.to_string()), strong(SMALL), TextColor(BONE), TextLayout::no_wrap(), Pickable::IGNORE));
        });
        if !does.is_empty() {
            row.spawn((Text::new(does.to_string()), font(SMALL), TextColor(CHROME_DIM), TextLayout::no_wrap(), Pickable::IGNORE));
        }
    });
}

fn spawn(mut commands: Commands, options: Res<Options>, style: Res<MapStyle>, mut ui: ResMut<MapUi>, mut images: ResMut<Assets<Image>>, mut materials: ResMut<Assets<MapMaterial>>) {
    ui.open = options.map;
    ui.zoom = options.map_zoom.unwrap_or(0.0);
    ui.tiles = images.add(blank(1, 1, TextureFormat::Rgba8Unorm, 4, ImageSampler::nearest()));
    ui.seen = images.add(blank(1, 1, TextureFormat::R8Unorm, 1, ImageSampler::linear()));
    ui.material = materials.add(MapMaterial { look: MapLook::default(), tiles: ui.tiles.clone(), seen: ui.seen.clone() });
    let colour = |text: &str| {
        let c = style.colour(text);
        Color::srgb(c.x, c.y, c.z)
    };
    let ink = colour(&style.ink);
    let mark = colour(&style.marker);
    // What a colour or a mark on the sheet is: a small sample and a word.
    let legend = |c: &mut ChildSpawnerCommands, sample: Option<Color>, cross: bool, name: &str| {
        c.spawn((Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(7), ..default() }, Pickable::IGNORE)).with_children(|row| {
            row.spawn((Node { width: px(14), height: px(14), justify_content: JustifyContent::Center, align_items: AlignItems::Center, ..default() }, Pickable::IGNORE)).with_children(|s| {
                if let Some(fill) = sample {
                    s.spawn((Node { width: px(14), height: px(14), border: UiRect::all(px(1.5)), ..default() }, BackgroundColor(fill), BorderColor::all(ink), Pickable::IGNORE));
                } else if cross {
                    s.spawn((Text::new("\u{00d7}"), strong(BODY + 5.0), TextColor(mark), Pickable::IGNORE));
                } else {
                    // Our ring, the arrowhead in it as a dot.
                    s.spawn((Node { width: px(14), height: px(14), border: UiRect::all(px(1.5)), border_radius: BorderRadius::MAX, justify_content: JustifyContent::Center, align_items: AlignItems::Center, ..default() }, BorderColor::all(mark), Pickable::IGNORE)).with_children(|ring| {
                        ring.spawn((Node { width: px(6), height: px(6), border_radius: BorderRadius::MAX, ..default() }, BackgroundColor(mark), Pickable::IGNORE));
                    });
                }
            });
            row.spawn((Text::new(name.to_string()), font(SMALL), TextColor(CHROME_DIM), TextLayout::no_wrap(), Pickable::IGNORE));
        });
    };
    commands
        .spawn((MapRoot, Node { position_type: PositionType::Absolute, left: px(0), top: px(0), width: Val::Percent(100.0), height: Val::Percent(100.0), flex_direction: FlexDirection::Column, display: Display::None, ..default() }, GlobalZIndex(25)))
        .with_children(|root| {
            // The top bar: the screen's title, a line of context, what the marks are, and Close.
            root.spawn((Node { height: px(TOP_BAR), flex_shrink: 0.0, flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(18), padding: UiRect::horizontal(px(24)), border: UiRect::bottom(px(1)), overflow: Overflow::clip(), ..default() }, BackgroundColor(CHROME), BorderColor::all(HAIRLINE))).with_children(|bar| {
                bar.spawn((Text::new("MAP"), theme::head(30.0), TextColor(BONE), Pickable::IGNORE));
                bar.spawn((SeenText, Text::new(""), font(BODY), TextColor(CHROME_DIM), TextLayout::no_wrap(), Pickable::IGNORE));
                bar.spawn((Node { flex_grow: 1.0, ..default() }, Pickable::IGNORE));
                legend(bar, Some(colour(&style.house)), false, "House");
                legend(bar, Some(colour(&style.shop)), false, "Shop");
                legend(bar, Some(colour(&style.office)), false, "Office");
                legend(bar, None, false, "You");
                legend(bar, None, true, "Where you started");
                bar.spawn((Node { width: px(18), ..default() }, Pickable::IGNORE));
                bar.spawn((Text::new("Close"), font(BODY), TextColor(CHROME_DIM), Pickable::IGNORE));
                key_hint(bar, "M", "");
            });
            // The paper, on the see-through veil.
            root.spawn((MapSheet, Node { flex_grow: 1.0, min_height: px(0), width: Val::Percent(100.0), ..default() }, MaterialNode(ui.material.clone())));
            // The footer: the keys that work here, and the scale.
            root.spawn((Node { height: px(FOOTER), flex_shrink: 0.0, flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(22), padding: UiRect::horizontal(px(24)), border: UiRect::top(px(1)), overflow: Overflow::clip(), ..default() }, BackgroundColor(CHROME), BorderColor::all(HAIRLINE))).with_children(|bar| {
                key_hint(bar, "Drag", "move the map");
                key_hint(bar, "Wheel", "closer or further");
                key_hint(bar, "Double-click", "centre on you");
                key_hint(bar, "Esc", "back to the game");
                bar.spawn((Node { flex_grow: 1.0, ..default() }, Pickable::IGNORE));
                bar.spawn((ScaleBar, Node { width: px(100), height: px(6), border: UiRect { left: px(1.5), right: px(1.5), bottom: px(1.5), ..default() }, ..default() }, BorderColor::all(BONE), Pickable::IGNORE));
                bar.spawn((ScaleText, Text::new(""), strong(SMALL), TextColor(BONE), TextLayout::no_wrap(), Pickable::IGNORE));
            });
        });
}

/// M, Esc, and the mouse while the map is up.
#[allow(clippy::too_many_arguments)]
fn input(
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut mouse: ResMut<ButtonInput<MouseButton>>,
    mut presses: MessageReader<MouseButtonInput>,
    mut wheels: ResMut<Messages<MouseWheel>>,
    mut ui: ResMut<MapUi>,
    style: Res<MapStyle>,
    scale: Res<UiScale>,
    time: Res<Time<Real>>,
    window: Query<&Window, With<PrimaryWindow>>,
    me: Res<LocalPlayer>,
    map: Res<ClientMap>,
    browser: Option<Res<crate::browser_ui::BrowserUi>>,
) {
    if browser.is_some_and(|b| b.open) {
        presses.clear();
        return;
    }
    let ctrl = keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight, KeyCode::SuperLeft, KeyCode::SuperRight]);
    if keys.just_pressed(KeyCode::KeyM) && !ctrl {
        ui.open = !ui.open;
        ui.drag = None;
        if ui.open {
            ui.centre = me.curr.pos;
        }
    } else if ui.open && keys.just_pressed(KeyCode::Escape) {
        ui.open = false;
        // Esc was the map's: nothing else acts on it this frame.
        keys.clear_just_pressed(KeyCode::Escape);
    }
    if !ui.open {
        presses.clear();
        return;
    }
    let size = Vec2::new(map.0.width() as f32, map.0.height() as f32);
    let Ok(window) = window.single() else { return };
    let k = scale.0.max(0.01);
    let win = Vec2::new(window.width(), window.height());
    // The paper's part of the window, between the top bar and the footer (logical pixels).
    let (top, bottom) = (TOP_BAR * k, win.y - FOOTER * k);
    let middle = Vec2::new(win.x / 2.0, (top + bottom) / 2.0);
    let (lo, hi) = MapUi::zoom_range(&style, Vec2::new(win.x, (bottom - top).max(1.0)) / k, size);
    if ui.zoom <= 0.0 {
        ui.zoom = style.zoom;
    }
    let cursor = window.cursor_position();
    let on_paper = cursor.is_some_and(|c| c.y >= top && c.y <= bottom);
    for press in presses.read() {
        if press.button != MouseButton::Left {
            continue;
        }
        if press.state.is_pressed() {
            let now = time.elapsed_secs();
            if !on_paper {
                // The top bar's Close, at its right end.
                if cursor.is_some_and(|c| c.y < top && c.x > win.x - 130.0 * k) {
                    ui.open = false;
                }
            } else if now - ui.last_press < DOUBLE_CLICK_SECS {
                ui.centre = me.curr.pos;
                ui.drag = None;
            } else {
                ui.drag = cursor;
            }
            ui.last_press = now;
        } else {
            ui.drag = None;
        }
    }
    if let (Some(from), Some(to)) = (ui.drag, cursor) {
        let by = (to - from) / (ui.zoom * k);
        ui.centre -= by;
        ui.drag = Some(to);
    }
    for w in wheels.drain() {
        let factor = if w.unit == MouseScrollUnit::Line { 1.15f32.powf(w.y) } else { (w.y * 0.002).exp() };
        let zoom = (ui.zoom * factor).clamp(lo, hi);
        // The point under the pointer stays under it.
        if let Some(c) = cursor {
            let off = (c - middle) / k;
            let centre = ui.centre + off / ui.zoom - off / zoom;
            ui.centre = centre;
        }
        ui.zoom = zoom;
    }
    ui.zoom = ui.zoom.clamp(lo, hi);
    ui.keep_on_sheet(size);
    // The mouse is the map's: the game behind it neither swings nor zooms.
    mouse.reset_all();
}

/// A tile's texel for the shader: what the ground is, its edges, what stands on it, entered.
/// Classes and edge kinds as map.wesl names them.
pub fn bake_tiles(map: &TileMap, entered: &dyn Fn(usize) -> bool) -> Vec<u8> {
    let (w, h) = (map.width().max(0), map.height().max(0));
    let mut out = vec![0u8; (w * h) as usize * 4];
    let building_of = |room: u16| if room == 0 { None } else { map.rooms.get(room as usize).map(|r| r.building as usize) };
    // An edge between two tiles: 1 an outside wall, 2 a wall between rooms, 3 a fence, 0 nothing
    // drawn (open ground, doors and arches, which read as gaps in the wall).
    let edge_kind = |edge: Edge, a: u16, b: u16| -> u8 {
        match edge {
            Edge::None | Edge::Arch => 0,
            Edge::Fence => 3,
            e if e.is_door() || matches!(e, Edge::DoorOpen | Edge::DoorBroken) => 0,
            _ => {
                if building_of(a).is_some() && building_of(a) == building_of(b) {
                    2
                } else {
                    1
                }
            }
        }
    };
    for y in 0..h {
        for x in 0..w {
            let t = IVec2::new(x, y);
            let tile = map.tile(t);
            let building = building_of(tile.room);
            let class = match building {
                Some(b) => match map.buildings.get(b).and_then(|b| b.look.as_ref()).map(|l| l.kind) {
                    Some(BuildingKind::Shop) => 9,
                    Some(BuildingKind::Office) => 10,
                    Some(BuildingKind::Civic) => 11,
                    _ => 8,
                },
                None => match tile.floor {
                    Floor::Grass => 0,
                    Floor::Dirt => 1,
                    Floor::Asphalt | Floor::RoadLine => 2,
                    Floor::Sidewalk | Floor::Concrete => 3,
                    Floor::Path | Floor::Wood | Floor::Carpet | Floor::TileFloor => 4,
                },
            };
            let north = edge_kind(tile.north, tile.room, map.tile(t + IVec2::NEG_Y).room);
            let west = edge_kind(tile.west, tile.room, map.tile(t + IVec2::NEG_X).room);
            let feature = match tile.feature {
                Feature::Tree => 1,
                Feature::PineTree => 2,
                Feature::Bush => 3,
                _ => 0,
            };
            let i = (y * w + x) as usize * 4;
            out[i] = class;
            out[i + 1] = north | west << 3;
            out[i + 2] = feature;
            // Walls between rooms are drawn from either side of them, so both sides say it.
            out[i + 3] = if building.is_some_and(entered) { 255 } else { 0 };
        }
    }
    out
}

/// The explored cells as a texel each: 255 seen, 0 not.
pub fn bake_seen(seen: &Explored) -> Vec<u8> {
    let mut out = vec![0u8; seen.w as usize * seen.h as usize];
    for cy in 0..seen.h as i32 {
        for cx in 0..seen.w as i32 {
            if seen.cell(cx, cy) {
                out[cy as usize * seen.w as usize + cx as usize] = 255;
            }
        }
    }
    out
}

/// A round length for the scale bar that is 60 to 150 UI pixels long at this zoom.
pub fn scale_length(zoom: f32) -> f32 {
    [5.0, 10.0, 20.0, 50.0, 100.0, 200.0, 500.0, 1000.0].into_iter().find(|m| m * zoom >= 60.0).unwrap_or(1000.0)
}

/// Shows or hides the map and, while it shows, keeps its textures and the shader's numbers right.
#[allow(clippy::too_many_arguments)]
fn show(
    mut ui: ResMut<MapUi>,
    style: Res<MapStyle>,
    options: Res<Options>,
    link: Res<ServerLink>,
    me: Res<LocalPlayer>,
    map: Res<ClientMap>,
    scale: Res<UiScale>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<MapMaterial>>,
    mut roots: Query<&mut Node, (With<MapRoot>, Without<ScaleBar>)>,
    sheets: Query<&ComputedNode, With<MapSheet>>,
    mut bars: Query<&mut Node, (With<ScaleBar>, Without<MapRoot>)>,
    mut texts: Query<(&mut Text, Has<ScaleText>), Or<(With<ScaleText>, With<SeenText>)>>,
    mut first: Local<bool>,
) {
    let Ok(mut root) = roots.single_mut() else { return };
    let display = if ui.open { Display::Flex } else { Display::None };
    if root.display != display {
        root.display = display;
    }
    if !ui.open {
        return;
    }
    let map = &map.0;
    let size = Vec2::new(map.width() as f32, map.height() as f32);
    let all = options.map_all;
    let blank = Explored::new(map.width(), map.height());
    let seen = link.curr.map_of(me.id).filter(|s| s.w == blank.w && s.h == blank.h).unwrap_or(&blank);

    // The first frame it is up: where to look.
    if !*first {
        *first = true;
        ui.centre = options.map_center.map_or(me.curr.pos, Vec2::from);
        if ui.zoom <= 0.0 {
            ui.zoom = style.zoom;
        }
    }
    if let Ok(node) = sheets.single() {
        let win = node.size() * node.inverse_scale_factor() / scale.0.max(0.01);
        if win.min_element() > 1.0 {
            let (lo, hi) = MapUi::zoom_range(&style, win, size);
            ui.zoom = ui.zoom.clamp(lo, hi);
        }
    }
    ui.keep_on_sheet(size);

    // The tiles, when the map or the buildings entered changed.
    let baked = Baked { size: IVec2::new(map.width(), map.height()), entered: seen.entered.clone() };
    if ui.baked.as_ref() != Some(&baked) {
        let bytes = bake_tiles(map, &|b| all || seen.has_entered(b));
        let mut image = blank_image(baked.size.x as u32, baked.size.y as u32, TextureFormat::Rgba8Unorm, ImageSampler::nearest());
        image.data = Some(bytes);
        let _ = images.insert(&ui.tiles, image);
        ui.baked = Some(baked);
    }
    if ui.shown.as_ref() != Some(seen) {
        let mut image = blank_image(seen.w as u32, seen.h as u32, TextureFormat::R8Unorm, ImageSampler::linear());
        image.data = Some(bake_seen(seen));
        let _ = images.insert(&ui.seen, image);
        ui.shown = Some(seen.clone());
    }

    // The shader's numbers.
    let device = sheets.single().map_or(1.0, |n| 1.0 / n.inverse_scale_factor().max(0.001));
    let mut pins = [Vec4::ZERO; PINS];
    let mut n = 0;
    if let Some(group) = link.curr.group_of(me.id) {
        for m in &group.members {
            if m.id != me.id
                && let Some(p) = link.curr.players.iter().find(|p| p.id == m.id && !p.dead)
                && n < PINS
            {
                pins[n] = Vec4::new(p.pos[0], p.pos[1], 1.0, 0.0);
                n += 1;
            }
        }
        for marker in &group.markers {
            if n < PINS {
                pins[n] = Vec4::new(marker.pos.x, marker.pos.y, 2.0, 0.0);
                n += 1;
            }
        }
    }
    let c = |text: &str| style.colour(text);
    let look = MapLook {
        view: Vec4::new(ui.centre.x, ui.centre.y, ui.zoom * device, device),
        size: Vec4::new(size.x, size.y, CELL as f32, if all { 1.0 } else { 0.0 }),
        grid: Vec4::new(seen.w.max(1) as f32, seen.h.max(1) as f32, 0.0, 0.0),
        me: Vec4::new(me.curr.pos.x, me.curr.pos.y, me.curr.facing, 1.0),
        home: seen.home.map_or(Vec4::ZERO, |(x, y)| Vec4::new(x as f32 + 0.5, y as f32 + 0.5, 1.0, 0.0)),
        pins,
        paper: c(&style.paper),
        ink: c(&style.ink),
        table: c(&style.table).with_w(VEIL),
        grass: c(&style.grass),
        road: c(&style.road),
        walk: c(&style.walk),
        path: c(&style.path),
        dirt: c(&style.dirt),
        house: c(&style.house),
        shop: c(&style.shop),
        office: c(&style.office),
        civic: c(&style.civic),
        tree: c(&style.tree),
        pine: c(&style.pine),
        marker: c(&style.marker),
        friend: c(&style.friend),
        pin: c(&style.pin),
    };
    if let Some(mut material) = materials.get_mut(&ui.material) {
        material.look = look;
    }

    // The scale bar and how much of the town we know.
    let metres = scale_length(ui.zoom);
    if let Ok(mut bar) = bars.single_mut() {
        let width = px(metres * ui.zoom);
        if bar.width != width {
            bar.width = width;
        }
    }
    for (mut text, is_scale) in &mut texts {
        let line = if is_scale {
            if metres >= 1000.0 { format!("{} km", metres / 1000.0) } else { format!("{metres} m") }
        } else if all {
            "All of it".to_string()
        } else {
            format!("{:.0}% explored \u{00b7} {} {} entered", seen.share() * 100.0, seen.entered.len(), if seen.entered.len() == 1 { "building" } else { "buildings" })
        };
        if text.0 != line {
            text.0 = line;
        }
    }
}

fn blank_image(width: u32, height: u32, format: TextureFormat, sampler: ImageSampler) -> Image {
    blank(width, height, format, if format == TextureFormat::R8Unorm { 1 } else { 4 }, sampler)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shipped_look_is_the_built_in_one() {
        assert_eq!(MapStyle::parse(SHIPPED).unwrap(), MapStyle::default());
        assert!(MapStyle::parse("(paper: \"#12345\")").is_err());
        assert!(MapStyle::parse("(papr: \"#123456\")").is_err());
        assert!(MapStyle::parse("(zoom: 40.0)").is_err());
        assert_eq!(hex("#ff0080").unwrap(), Vec4::new(1.0, 0.0, 128.0 / 255.0, 1.0));
    }

    #[test]
    fn the_scale_bar_is_a_round_length() {
        assert_eq!(scale_length(3.0), 20.0);
        assert_eq!(scale_length(16.0), 5.0);
        assert_eq!(scale_length(0.7), 100.0);
    }

    #[test]
    fn the_view_keeps_to_the_sheet_and_the_zoom_to_its_range() {
        let style = MapStyle::default();
        let (lo, hi) = MapUi::zoom_range(&style, Vec2::new(1280.0, 720.0), Vec2::splat(480.0));
        assert!(lo < 1.5 && lo * (480.0 + 2.0 * MARGIN) <= 720.0, "the whole sheet fits at {lo}");
        assert_eq!(hi, style.zoom_max);
        let mut ui = MapUi { centre: Vec2::new(-500.0, 900.0), ..default() };
        ui.keep_on_sheet(Vec2::splat(480.0));
        assert_eq!(ui.centre, Vec2::new(-MARGIN, 480.0 + MARGIN));
    }

    #[test]
    fn tiles_bake_to_what_the_shader_reads() {
        let content = zombye_net::content::load_config(&zombye_net::content::default_data_dir()).unwrap();
        let world = zombye_sim::World::new(content);
        let map = &world.map;
        let (w, h) = (map.width(), map.height());
        let none = bake_tiles(map, &|_| false);
        let all = bake_tiles(map, &|_| true);
        assert_eq!(none.len(), (w * h * 4) as usize);
        let at = |bytes: &[u8], t: IVec2| -> [u8; 4] { bytes[((t.y * w + t.x) * 4) as usize..][..4].try_into().unwrap() };
        let (mut houses, mut roads, mut outer, mut inner, mut trees) = (0, 0, 0, 0, 0);
        for y in 0..h {
            for x in 0..w {
                let t = IVec2::new(x, y);
                let [class, edges, feature, entered] = at(&none, t);
                assert_eq!(entered, 0);
                let indoors = map.tile(t).room != 0;
                assert_eq!(class >= 8, indoors, "{t}: only rooms are buildings");
                assert_eq!(at(&all, t)[3] == 255, indoors);
                houses += (class == 8) as u32;
                roads += (class == 2) as u32;
                trees += (feature != 0) as u32;
                for kind in [edges & 7, edges >> 3 & 7] {
                    outer += (kind == 1) as u32;
                    inner += (kind == 2) as u32;
                }
            }
        }
        assert!(houses > 2000 && roads > 5000 && trees > 500, "{houses} house tiles, {roads} road, {trees} trees");
        assert!(outer > 3000 && inner > 1000, "{outer} outside walls, {inner} inside");
    }

    #[test]
    fn seen_cells_bake_row_by_row() {
        let mut e = Explored::new(40, 20);
        e.reveal(Vec2::new(6.0, 14.0), 1.0);
        let bytes = bake_seen(&e);
        assert_eq!(bytes.len(), 10 * 5);
        assert_eq!(bytes[3 * 10 + 1], 255);
        assert_eq!(bytes.iter().filter(|b| **b == 255).count() as u32, e.count());
    }
}
