//! The build menu (Shift+B): everything a survivor can build, by category, from the build
//! catalog (`assets/data/build.ron`, `zombye_sim::build`). A screen of the inventory's flat frame
//! (`inventory_ui`, its rail's Build tab; docs/ui-design.md), drawn between its rail, top bar and
//! footer as Health and Skills are.
//!
//! Barricades are all there is for now: boarding up a window or a door, a plank at a time. Walls,
//! house upgrades and the rest go in the same list as new categories of the catalog. Each entry
//! shows what it's for, the materials one piece uses and the tools it needs, how many it stacks to,
//! and whether what we carry is enough (the same check the server makes: `build::check_build`).
//! Its Build button builds it on what our survivor faces (`Intent::Build`), as the right-click
//! menu on a window or door does (`world_actions.rs`). The rows come from [`rows`], a pure function.

use bevy::prelude::*;
use zombye_net::{Intent, PrivateView};
use zombye_sim::{BuildCategory, BuildDef, ItemDef};

use crate::player::LocalPlayer;
use crate::server_link::{JoinInfo, ServerLink};
use crate::theme::{self, flat, font};

/// The width of one entry's card (several to a row on a wide screen).
const CARD_W: f32 = 360.0;

/// Is the menu open (the frame keeps it in step: `inventory_ui::screens`).
#[derive(Resource, Default)]
pub struct BuildMenu {
    pub open: bool,
}

#[derive(Component)]
struct MenuRoot;

/// One entry of the build menu, in words.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub id: String,
    pub category: BuildCategory,
    pub name: String,
    pub description: String,
    /// "1 x wooden plank + 4 x nail, with a claw hammer (5 s a piece, 4 at most)".
    pub needs: String,
    /// What it goes on ("a window").
    pub on: String,
    /// `Ok` when what we carry is enough for a piece, else what's missing.
    pub ready: Result<(), String>,
}

/// The menu's entries, in the catalog's order, sorted by category: what each needs and whether
/// `view`'s inventory has it (no view yet: not ready).
pub fn rows(builds: &[BuildDef], items: &[ItemDef], view: Option<&PrivateView>) -> Vec<Row> {
    let mut out: Vec<Row> = builds
        .iter()
        .map(|d| Row {
            id: d.id.clone(),
            category: d.category,
            name: d.name.clone(),
            description: d.description.clone(),
            needs: zombye_sim::build::describe(d, items),
            on: format!("a {}", d.on.label()),
            ready: match view {
                Some(v) => zombye_sim::build::check_build(d, &v.inventory, items, 1),
                None => Err("nothing carried yet".to_string()),
            },
        })
        .collect();
    out.sort_by_key(|r| r.category);
    out
}

pub struct BuildUiPlugin;

impl Plugin for BuildUiPlugin {
    fn build(&self, app: &mut App) {
        let open = app.world().get_resource::<crate::Options>().is_some_and(|o| o.build_menu);
        app.insert_resource(BuildMenu { open }).init_resource::<Shown>().add_systems(Startup, spawn).add_systems(Update, (toggle, rebuild).chain());
    }
}

/// What the menu shows, to rebuild it only when that changes.
#[derive(Resource, Default, PartialEq)]
struct Shown(Option<Vec<Row>>);

fn spawn(mut commands: Commands) {
    // The middle of the frame (`inventory_ui`): between its rail, top bar and footer, over its veil.
    commands.spawn((MenuRoot, GlobalZIndex(7), Visibility::Hidden, Node { position_type: PositionType::Absolute, left: px(flat::RAIL), top: px(flat::TOP_BAR), right: px(0), bottom: px(flat::FOOTER), flex_direction: FlexDirection::Column, ..default() }));
}

fn toggle(keys: Res<ButtonInput<KeyCode>>, mut menu: ResMut<BuildMenu>) {
    let shift = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    if shift && keys.just_pressed(KeyCode::KeyB) {
        menu.open = !menu.open;
    }
}

fn rebuild(mut commands: Commands, menu: Res<BuildMenu>, link: Res<ServerLink>, join: Res<JoinInfo>, me: Res<LocalPlayer>, scrolls: Option<Res<crate::inventory_ui::Scrolls>>, mut shown: ResMut<Shown>, mut root: Query<(Entity, &mut Visibility), With<MenuRoot>>) {
    let Ok((root, mut vis)) = root.single_mut() else { return };
    let want = if menu.open { Visibility::Inherited } else { Visibility::Hidden };
    if *vis != want {
        *vis = want;
    }
    if !menu.open {
        return;
    }
    let list = rows(&join.builds, &join.items, link.own(me.id));
    if shown.0.as_ref() == Some(&list) {
        return;
    }
    commands.entity(root).despawn_children();
    let y = scrolls.map_or(0.0, |s| s.0[7]);
    commands.entity(root).with_children(|c| {
        c.spawn((Node { flex_direction: FlexDirection::Row, justify_content: JustifyContent::SpaceBetween, align_items: AlignItems::FlexEnd, padding: UiRect::new(px(24), px(24), px(18), px(12)), flex_shrink: 0.0, ..default() }, Pickable::IGNORE)).with_children(|h| {
            h.spawn((Text::new("BUILD"), theme::head(flat::TITLE), TextColor(flat::BONE), Pickable::IGNORE));
            h.spawn((Text::new("Face the window or door first, then build."), font(flat::SMALL), TextColor(flat::DIM), Pickable::IGNORE));
        });
        crate::inventory_ui::scroller(c, 7, y, UiRect::new(px(24), px(24), px(0), px(16)), 14.0).with_children(|s| {
            if list.is_empty() {
                s.spawn((Text::new("Nothing to build: this game's data has no build.ron."), font(flat::BODY), TextColor(flat::DIM), Pickable::IGNORE));
            }
            let mut i = 0;
            while i < list.len() {
                let cat = list[i].category;
                let n = list[i..].iter().take_while(|r| r.category == cat).count();
                s.spawn((Text::new(cat.label().to_uppercase()), theme::head(flat::HEAD), TextColor(flat::DIM), Pickable::IGNORE));
                s.spawn((Node { flex_direction: FlexDirection::Row, flex_wrap: FlexWrap::Wrap, column_gap: px(12), row_gap: px(12), flex_shrink: 0.0, ..default() }, Pickable::IGNORE)).with_children(|grid| {
                    for r in &list[i..i + n] {
                        card(grid, r, me.id);
                    }
                });
                i += n;
            }
        });
    });
    shown.0 = Some(list);
}

/// One entry: its picture, name, what it's for, what a piece takes, and its Build button.
fn card(grid: &mut ChildSpawnerCommands, r: &Row, me: u32) {
    let ready = r.ready.is_ok();
    grid.spawn((Node { flex_grow: 1.0, flex_basis: px(CARD_W), min_width: px(CARD_W), max_width: px(CARD_W * 1.6), flex_direction: FlexDirection::Column, row_gap: px(6), padding: UiRect::axes(px(14), px(12)), border: UiRect::all(px(1)), border_radius: BorderRadius::all(px(flat::RADIUS)), ..default() }, BorderColor::all(if ready { flat::GREEN } else { flat::LINE }), BackgroundColor(flat::CELL), Pickable::IGNORE)).with_children(|card| {
        card.spawn((Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(12), ..default() }, Pickable::IGNORE)).with_children(|t| {
            if let Some(image) = flat::icon("wooden_plank", Color::WHITE) {
                t.spawn((Node { width: px(44), height: px(44), flex_shrink: 0.0, ..default() }, image, Pickable::IGNORE));
            }
            t.spawn((Node { flex_direction: FlexDirection::Column, row_gap: px(2), ..default() }, Pickable::IGNORE)).with_children(|n| {
                n.spawn((Text::new(r.name.to_uppercase()), theme::head(flat::HEAD), TextColor(flat::BONE), Pickable::IGNORE));
                n.spawn((Text::new(format!("On {}", r.on)), font(flat::SMALL), TextColor(flat::DIM), Pickable::IGNORE));
            });
        });
        if !r.description.is_empty() {
            card.spawn((Text::new(r.description.clone()), font(flat::SMALL), TextColor(flat::DIM), Pickable::IGNORE));
        }
        card.spawn((Text::new(r.needs.clone()), font(flat::SMALL), TextColor(flat::BONE), Pickable::IGNORE));
        let (text, color) = match &r.ready {
            Ok(()) => ("You have what it takes.".to_string(), flat::GREEN),
            Err(why) => (format!("Missing: {why}"), flat::RED_TEXT),
        };
        card.spawn((Text::new(text), theme::strong(flat::SMALL), TextColor(color), Pickable::IGNORE));
        card.spawn((Node { flex_direction: FlexDirection::Row, margin: UiRect::top(px(4)), ..default() }, Pickable::IGNORE)).with_children(|b| {
            let id = r.id.clone();
            flat::button(b, "Build", ready).observe(move |click: On<PointerPress>, mut link: ResMut<ServerLink>| {
                if click.button == PointerButton::Primary {
                    link.send(Intent::Build { player: me, build: id.clone() });
                }
            });
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shipped catalog gives a row for each entry, barricades first, and says what's missing
    /// with nothing carried.
    #[test]
    fn the_shipped_catalog_makes_rows() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/data");
        let config = zombye_net::content::load_config(&dir, 1).expect("content");
        let r = rows(&config.builds, &config.items, None);
        assert!(r.iter().any(|x| x.id == "board_window"), "{r:?}");
        assert!(r.iter().all(|x| x.ready.is_err()));
        assert!(r.iter().any(|x| x.needs.contains("nail")), "{r:?}");
    }
}
