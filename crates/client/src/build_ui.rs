//! The build menu (Shift+B): everything a survivor can build, by category, from the build
//! catalog (`assets/data/build.ron`, `zombye_sim::build`).
//!
//! Barricades are all there is for now: boarding up a window or a door, a plank at a time. Walls,
//! house upgrades and the rest go in the same list as new categories of the catalog. Each entry
//! shows what it's for, the materials one piece uses and the tools it needs, how many it stacks to,
//! and whether what we carry is enough (the same check the server makes: `build::check_build`).
//! A click on an entry builds it on what our survivor faces (`Intent::Build`), as the right-click
//! menu on a window or door does (`world_actions.rs`). The rows come from [`rows`], a pure function,
//! so the panel can move into a shared frame without the logic moving with it.

use bevy::prelude::*;
use zombye_net::{Intent, PrivateView};
use zombye_sim::{BuildCategory, BuildDef, ItemDef};

use crate::player::LocalPlayer;
use crate::server_link::{JoinInfo, ServerLink};
use crate::theme::{self, BAD, DIM, GOLD, TEXT, font};

const ROW_W: f32 = 460.0;

/// Is the menu open.
#[derive(Resource, Default)]
pub struct BuildMenu {
    pub open: bool,
}

#[derive(Component)]
struct MenuRoot;

/// An entry of the menu, to build when clicked.
#[derive(Component)]
struct BuildRow(String);

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
    commands.spawn((
        MenuRoot,
        crate::hud::TopPanel,
        Visibility::Hidden,
        Node {
            position_type: PositionType::Absolute,
            top: px(70),
            right: px(24),
            flex_direction: FlexDirection::Column,
            row_gap: px(4),
            padding: UiRect::axes(px(theme::SHEET_PAD.0), px(theme::SHEET_PAD.1)),
            width: px(ROW_W + 2.0 * theme::SHEET_PAD.0),
            max_height: Val::Percent(80.0),
            overflow: Overflow::scroll_y(),
            ..default()
        },
        theme::sheet(),
        Pickable::IGNORE,
    ));
}

fn toggle(keys: Res<ButtonInput<KeyCode>>, mut menu: ResMut<BuildMenu>) {
    let shift = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    if shift && keys.just_pressed(KeyCode::KeyB) {
        menu.open = !menu.open;
    } else if menu.open && keys.just_pressed(KeyCode::Escape) {
        menu.open = false;
    }
}

fn rebuild(mut commands: Commands, menu: Res<BuildMenu>, link: Res<ServerLink>, join: Res<JoinInfo>, me: Res<LocalPlayer>, mut shown: ResMut<Shown>, mut root: Query<(Entity, &mut Visibility), With<MenuRoot>>) {
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
    commands.entity(root).with_children(|c| {
        theme::title(c, "Build");
        c.spawn((Text::new("Face what to build on, then click an entry. Shift+B: close"), font(theme::SMALL), TextColor(DIM), Pickable::IGNORE));
        if list.is_empty() {
            c.spawn((Text::new("Nothing to build: this game's data has no build.ron."), font(theme::BODY), TextColor(DIM), Pickable::IGNORE));
        }
        let mut last = None;
        for r in &list {
            if last != Some(r.category) {
                last = Some(r.category);
                theme::spawn_rule(c);
                c.spawn((Text::new(r.category.label().to_uppercase()), theme::head(theme::BODY), TextColor(TEXT), Pickable::IGNORE));
            }
            let ready = r.ready.is_ok();
            c.spawn((
                BuildRow(r.id.clone()),
                Node { width: px(ROW_W), flex_direction: FlexDirection::Column, row_gap: px(2), padding: UiRect::all(px(6)), ..default() },
                BackgroundColor(theme::ROW),
            ))
            .observe(|over: On<PointerOver>, mut bg: Query<&mut BackgroundColor>| {
                if let Ok(mut b) = bg.get_mut(over.entity) {
                    b.0 = theme::ROW_HOVER;
                }
            })
            .observe(|out: On<PointerOut>, mut bg: Query<&mut BackgroundColor>| {
                if let Ok(mut b) = bg.get_mut(out.entity) {
                    b.0 = theme::ROW;
                }
            })
            .observe(|click: On<PointerPress>, rows: Query<&BuildRow>, me: Res<LocalPlayer>, mut link: ResMut<ServerLink>| {
                if click.button == PointerButton::Primary
                    && let Ok(r) = rows.get(click.entity)
                {
                    link.send(Intent::Build { player: me.id, build: r.0.clone() });
                }
            })
            .with_children(|row| {
                row.spawn((Text::new(r.name.clone()), theme::strong(theme::BODY), TextColor(if ready { GOLD } else { TEXT }), Pickable::IGNORE));
                if !r.description.is_empty() {
                    row.spawn((Text::new(r.description.clone()), font(theme::SMALL), TextColor(DIM), Pickable::IGNORE));
                }
                row.spawn((Text::new(format!("On {}: {}", r.on, r.needs)), font(theme::SMALL), TextColor(DIM), Pickable::IGNORE));
                let (text, color) = match &r.ready {
                    Ok(()) => ("You have what it takes.".to_string(), GOLD),
                    Err(why) => (format!("Missing: {why}"), BAD),
                };
                row.spawn((Text::new(text), font(theme::SMALL), TextColor(color), Pickable::IGNORE));
            });
        }
    });
    shown.0 = Some(list);
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
