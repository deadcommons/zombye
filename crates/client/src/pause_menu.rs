//! The pause menu: Esc, while playing.
//!
//! Resume, Save game and Load last save (F5 and F9's quicksave), Settings (`settings_ui.rs`),
//! Main menu and Quit. **In your own world the game stops while it is up:** virtual time is
//! paused, so the local server doesn't tick, nothing is predicted and nothing animates. On a
//! multiplayer server the world is the server's and goes on; the menu says so, and has no Save or
//! Load.
//!
//! Esc opens it only when nothing else wants the key: not while a line is being typed (chat, the
//! inventory's search), a right-click menu, the graphics panel, the map or the server browser is up, which
//! Esc shuts as before. While the menu (or the settings screen) is up the game's keys and mouse
//! buttons are held back, as with the server browser, and Esc goes one step back: from the
//! settings to the menu, from the menu to the game.
//!
//! Main menu and Quit both leave the way closing the window does, so the world is autosaved
//! (`saving.rs`); Main menu then starts the game again with `--main-menu` (`main_menu.rs`).
//!
//! `--pause` starts with it up (for pictures: `--render-to shot.png --hud --pause`).

use bevy::input::InputSystems;
use bevy::prelude::*;

use crate::Options;
use crate::server_link::ServerLink;
use crate::settings_ui::SettingsUi;
use crate::menu_skin::{self as skin, DIM, Look};
use crate::theme::{SMALL, font};

const WIDTH: f32 = 360.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    Resume,
    Save,
    Load,
    Settings,
    MainMenu,
    Quit,
}

impl Act {
    pub fn label(self, on_server: bool) -> &'static str {
        match self {
            Act::Resume => "Resume",
            Act::Save => "Save game",
            Act::Load => "Load last save",
            Act::Settings => "Settings",
            Act::MainMenu if on_server => "Leave the server",
            Act::MainMenu => "Save and exit to menu",
            Act::Quit if on_server => "Quit",
            Act::Quit => "Save and quit",
        }
    }
}

/// The menu's entries, top to bottom: a server's world has nothing of ours to save or load.
pub fn entries(on_server: bool) -> Vec<Act> {
    if on_server { vec![Act::Resume, Act::Settings, Act::MainMenu, Act::Quit] } else { vec![Act::Resume, Act::Save, Act::Load, Act::Settings, Act::MainMenu, Act::Quit] }
}

#[derive(Resource, Default)]
pub struct PauseMenu {
    pub open: bool,
    acts: Vec<Act>,
    /// Up or not, and the controller in use (its footer names the button), as last drawn.
    shown: Option<(bool, bool)>,
}

pub struct PauseMenuPlugin;

impl Plugin for PauseMenuPlugin {
    fn build(&self, app: &mut App) {
        let options = app.world().resource::<Options>().clone();
        app.insert_resource(PauseMenu { open: options.pause || options.settings.is_some(), ..default() })
            .add_systems(Startup, spawn)
            // Before every other key handler: while the menu is up the keys are its own.
            .add_systems(PreUpdate, keys.in_set(PauseKeys).after(InputSystems).after(crate::settings_ui::capture).before(crate::browser_ui::keys).before(crate::chat::chat_keys).before(crate::inventory_ui::TextEntry).run_if(crate::live).run_if(crate::main_menu::layer_down))
            .add_systems(Update, (act, stop_time, draw).chain());
    }
}

#[derive(Component)]
struct Root;
#[derive(Component)]
struct Panel;

fn spawn(mut commands: Commands) {
    commands
        .spawn((
            Root,
            Node { position_type: PositionType::Absolute, width: percent(100), height: percent(100), justify_content: JustifyContent::Center, align_items: AlignItems::Center, display: Display::None, ..default() },
            BackgroundColor(skin::VEIL),
            GlobalZIndex(30),
        ))
        .with_children(|c| {
            c.spawn((Panel, Node { width: px(WIDTH), flex_direction: FlexDirection::Column, border: UiRect::all(px(1)), border_radius: BorderRadius::all(px(8)), ..default() }, skin::panel()));
        });
}

/// The pause menu's keys, in `PreUpdate`: what must see Esc before it runs before this.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct PauseKeys;

/// Is something up that Esc belongs to first?
#[allow(clippy::too_many_arguments)]
fn keys(
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut mouse: ResMut<ButtonInput<MouseButton>>,
    mut menu: ResMut<PauseMenu>,
    mut settings: ResMut<SettingsUi>,
    browser: Option<Res<crate::browser_ui::BrowserUi>>,
    chat: Option<Res<crate::chat::ChatEntry>>,
    find: Option<Res<crate::inventory_ui::Find>>,
    item_menu: Option<Res<crate::inventory_ui::ItemMenu>>,
    graphics: Option<Res<crate::graphics_ui::GraphicsUi>>,
    map: Option<Res<crate::map_ui::MapUi>>,
    mut inventory: Option<ResMut<crate::inventory_ui::InventoryPanel>>,
) {
    let esc = keys.just_pressed(KeyCode::Escape);
    if menu.open {
        if esc {
            if settings.open {
                settings.close();
            } else {
                menu.acts.push(Act::Resume);
            }
        }
        keys.reset_all();
        mouse.reset_all();
        return;
    }
    let busy = browser.is_some_and(|b| b.open) || chat.is_some_and(|c| c.typing) || find.is_some_and(|f| f.typing) || item_menu.is_some_and(|m| m.is_up()) || graphics.is_some_and(|g| g.open) || map.is_some_and(|m| m.open);
    // The inventory's frame (with its Health and Skills screens) is shut by Esc first.
    if esc && !busy && inventory.as_ref().is_some_and(|p| p.open) {
        if let Some(p) = inventory.as_mut() {
            p.open = false;
        }
        keys.clear_just_pressed(KeyCode::Escape);
        return;
    }
    if esc && !busy {
        menu.open = true;
        keys.reset_all();
        mouse.reset_all();
    }
}

fn act(mut menu: ResMut<PauseMenu>, mut settings: ResMut<SettingsUi>, mut asked: ResMut<crate::saving::Asked>, options: Res<Options>, link: Res<ServerLink>, layer: Option<ResMut<crate::main_menu::MenuLayer>>, mut exit: MessageWriter<AppExit>) {
    let mut layer = layer;
    for a in std::mem::take(&mut menu.acts) {
        match a {
            Act::Resume => menu.open = false,
            // Saving and loading say what came of them in the yellow line, so back to the game.
            Act::Save => {
                asked.save = true;
                menu.open = false;
            }
            Act::Load => {
                asked.load = true;
                menu.open = false;
            }
            Act::Settings => settings.open = true,
            // In your own world the menu comes up over it, saved (#36); a server is left.
            Act::MainMenu if link.net.is_none() && layer.is_some() => {
                if let Some(l) = layer.as_deref_mut() {
                    l.raise();
                }
                menu.open = false;
            }
            Act::MainMenu => {
                crate::main_menu::ask_relaunch(crate::main_menu::to_menu_args(&std::env::args().skip(1).collect::<Vec<_>>()), &options);
                exit.write(AppExit::Success);
            }
            Act::Quit => {
                exit.write(AppExit::Success);
            }
        }
    }
    // The settings screen is only ever up over this menu.
    if !menu.open && settings.open {
        settings.close();
    }
}

/// Your own world stands still while the menu is up; a server's goes on. (Not in a picture, whose
/// visual time must run for poses to settle.)
fn stop_time(menu: Res<PauseMenu>, layer: Option<Res<crate::main_menu::MenuLayer>>, loading: Option<Res<crate::loading::Loading>>, link: Res<ServerLink>, options: Res<Options>, mut time: ResMut<Time<Virtual>>) {
    // (The offscreen renderer stops and starts visual time itself.)
    if options.offscreen() {
        return;
    }
    // (And behind the loading screen: nothing happens to a survivor you can't see yet.)
    let stop = (menu.open || loading.is_some_and(|l| l.up) || !crate::main_menu::layer_down(layer)) && link.net.is_none();
    if time.is_paused() != stop {
        if stop { time.pause() } else { time.unpause() }
    }
}

fn draw(mut commands: Commands, mut menu: ResMut<PauseMenu>, settings: Res<SettingsUi>, link: Res<ServerLink>, mut root: Query<&mut Node, With<Root>>, panel: Query<Entity, With<Panel>>) {
    // (The settings screen stands in its place, not over it.)
    let up = menu.open && !settings.open;
    let want = (up, crate::pad::showing());
    if menu.shown == Some(want) {
        return;
    }
    menu.shown = Some(want);
    let (Ok(mut node), Ok(panel)) = (root.single_mut(), panel.single()) else { return };
    node.display = if up { Display::Flex } else { Display::None };
    commands.entity(panel).despawn_related::<Children>();
    if !up {
        return;
    }
    let on_server = link.net.is_some();
    commands.entity(panel).with_children(|p| {
        p.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(8), padding: UiRect::new(px(24), px(24), px(18), px(18)), ..default() }).with_children(|col| {
            skin::title(col, if on_server { "Menu" } else { "Paused" });
            let note = match &link.net {
                Some(net) => format!("{} goes on while this is up.", if net.server.trim().is_empty() { "The server's world" } else { net.server.as_str() }),
                None => "The world waits. Leaving saves it.".to_string(),
            };
            col.spawn((Text::new(note), font(SMALL), TextColor(DIM), Node { margin: UiRect::bottom(px(8)), ..default() }));
            for a in entries(on_server) {
                let look = if a == Act::Resume { Look::Primary } else { Look::Plain };
                skin::button(col, a.label(on_server), None, look, percent(100)).observe(move |_: On<PointerPress>, mut menu: ResMut<PauseMenu>| menu.acts.push(a));
            }
        });
        skin::rule(p);
        p.spawn(Node { height: px(46), padding: UiRect::horizontal(px(24)), align_items: AlignItems::Center, ..default() }).with_children(|row| {
            skin::keycap(row, "Esc", "resume");
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_servers_world_has_no_save_or_load() {
        let own = entries(false);
        assert_eq!(own.first(), Some(&Act::Resume));
        assert!(own.contains(&Act::Save) && own.contains(&Act::Load));
        let server = entries(true);
        assert!(!server.contains(&Act::Save) && !server.contains(&Act::Load));
        for a in [Act::Settings, Act::MainMenu, Act::Quit] {
            assert!(own.contains(&a) && server.contains(&a));
        }
        assert_eq!(Act::MainMenu.label(true), "Leave the server");
        assert_eq!(Act::Quit.label(false), "Save and quit");
    }
}
