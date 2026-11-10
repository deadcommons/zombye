//! The server's rules, shown to a player who joins (#69, docs/PLAN.md §4.8).
//!
//! On a multiplayer server (`--connect`) a panel at the top right lists the rules the server sent
//! in its welcome, a line each, in the words of `zombye_net::wire::Rules::lines`: how many play,
//! PvP, the camera, agents, what you're told of other players, logging out, how new survivors
//! start and with what, and the world. It is up for the first [`SHOW_SECS`] seconds after joining
//! and F6 brings it back (or puts it away) at any time. Single-player has no rules panel: F6 says
//! so. The same lines go to stdout on joining, and `zombye --server-info HOST[:PORT]` prints them
//! for a server without joining it (`zombye_net::remote::query`).

use bevy::input::InputSystems;
use bevy::prelude::*;
use zombye_net::wire::Rules;

use crate::saving::Notice;
use crate::server_link::ServerLink;
use crate::theme::{self, BODY, DIM, SMALL, TEXT, font};

/// How long the panel stays up after joining unless F6 says otherwise (seconds).
pub const SHOW_SECS: f32 = 20.0;
/// (As wide inside the sheet's ragged edge as before it had one.)
const WIDTH: f32 = 376.0 + 2.0 * theme::SHEET_PAD.0;

pub struct RulesUiPlugin;

impl Plugin for RulesUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<RulesUi>().add_systems(Startup, spawn).add_systems(PreUpdate, keys.after(InputSystems).after(crate::inventory_ui::TextEntry).run_if(crate::live)).add_systems(Update, show);
    }
}

/// Whether the player asked for the panel (F6): `None` until they do, and then it goes by the time
/// since joining.
#[derive(Resource, Default)]
struct RulesUi {
    asked: Option<bool>,
}

#[derive(Component)]
struct RulesPanel;

/// The panel's rows: a title, the server's address, then a label and a sentence per rule.
pub fn rows(server: &str, addr: &str, rules: &Rules) -> (String, String, Vec<(&'static str, String)>) {
    (format!("{server}: the rules"), format!("{addr}  |  F6 hides or shows this"), rules.lines())
}

fn spawn(mut commands: Commands, link: Res<ServerLink>) {
    let Some(net) = &link.net else { return };
    let (title, sub, lines) = rows(&net.server, &net.addr, &net.rules);
    commands
        .spawn((
            RulesPanel,
            Node { position_type: PositionType::Absolute, top: px(16), right: px(16), width: px(WIDTH), flex_direction: FlexDirection::Column, row_gap: px(5), padding: UiRect::axes(px(theme::SHEET_PAD.0), px(theme::SHEET_PAD.1)), ..default() },
            theme::sheet(),
            GlobalZIndex(8),
            Pickable::IGNORE,
        ))
        .with_children(|c| {
            // The skin's title (Linear DCM-61): centred capitals over a brushed stroke.
            theme::title(c, &title);
            c.spawn((Text::new(sub), font(SMALL), TextColor(DIM), Node { align_self: AlignSelf::Center, ..default() }, Pickable::IGNORE));
            theme::spawn_rule(c);
            for (label, text) in lines {
                c.spawn((Node { flex_direction: FlexDirection::Row, column_gap: px(10), ..default() }, Pickable::IGNORE)).with_children(|row| {
                    row.spawn((Text::new(label), font(BODY), TextColor(DIM), Node { width: px(112), flex_shrink: 0.0, ..default() }, Pickable::IGNORE));
                    row.spawn((Text::new(text), font(BODY), TextColor(TEXT), Pickable::IGNORE));
                });
            }
        });
}

fn keys(keys: Res<ButtonInput<KeyCode>>, link: Res<ServerLink>, time: Res<Time>, mut ui: ResMut<RulesUi>, mut notice: ResMut<Notice>) {
    if !keys.just_pressed(KeyCode::F6) {
        return;
    }
    if link.net.is_none() {
        notice.say(time.elapsed_secs(), "No server rules: this is your own world (single-player)".to_string());
        return;
    }
    let up = ui.asked.unwrap_or(time.elapsed_secs() < SHOW_SECS);
    ui.asked = Some(!up);
}

/// Where the panel's top goes, in UI pixels, under a stats text `stats_h` high (which wraps to
/// more or fewer lines with the window's width and the UI scale; none offscreen).
pub fn top_under(stats_h: f32) -> f32 {
    if stats_h > 0.0 { (10.0 + stats_h + 8.0).round() } else { 16.0 }
}

fn show(ui: Res<RulesUi>, time: Res<Time>, stats: Query<&ComputedNode, With<crate::hud::HudText>>, mut panel: Query<(&mut Visibility, &mut Node), With<RulesPanel>>) {
    let Ok((mut vis, mut node)) = panel.single_mut() else { return };
    // Clear of the stats text along the top, however it wraps (last frame's layout).
    let top = px(top_under(stats.single().map_or(0.0, |n| n.size().y * n.inverse_scale_factor())));
    if node.top != top {
        node.top = top;
    }
    let up = ui.asked.unwrap_or(time.elapsed_secs() < SHOW_SECS);
    let want = if up { Visibility::Inherited } else { Visibility::Hidden };
    if *vis != want {
        *vis = want;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zombye_net::wire::{Agents, Camera};

    #[test]
    fn the_panel_lists_every_rule_the_server_sent() {
        let rules = Rules {
            name: "Dead Commons".into(),
            pvp: true,
            max_players: 32,
            allow_dev: false,
            logout_secs: 30.0,
            logout_safe_secs: 5.0,
            spawn_protection_secs: 10.0,
            fair_sight: true,
            camera: Camera::Isometric,
            agents: Agents::Marked,
            spread_spawns: true,
            start_kit: vec![("Bottle of water".into(), 1), ("Rag".into(), 2)],
            map_m: [480, 480],
            zombies: 2950,
        };
        let (title, sub, lines) = rows("Dead Commons", "play.example:7777", &rules);
        assert_eq!(title, "Dead Commons: the rules");
        assert!(sub.contains("play.example:7777") && sub.contains("F6"));
        let labels: Vec<&str> = lines.iter().map(|(l, _)| *l).collect();
        assert_eq!(labels, ["Players", "PvP", "Camera", "Agents", "Other players", "Logging out", "New survivors", "Start kit", "World"]);
        let text = |label: &str| lines.iter().find(|(l, _)| *l == label).unwrap().1.as_str();
        assert_eq!(text("Agents"), "allowed, and marked as agents");
        assert_eq!(text("Start kit"), "Bottle of water, 2 x Rag");
        assert_eq!(text("World"), "480 x 480 m, 2950 zombies");
        // Under the stats text when there is one, at the top when there isn't (offscreen).
        assert_eq!((top_under(0.0), top_under(54.0), top_under(90.0)), (16.0, 72.0, 108.0));
    }
}
