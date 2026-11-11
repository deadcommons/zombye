//! Skills (#46): the skills panel (K) and the level-up line.
//!
//! The panel lists every skill with its level (0-10), a bar of the way to the next level with the XP
//! so far and the XP it needs, what its level gives now and what the next one would, and how XP is
//! earned. It all comes from our private view (`PrivateView::skills`) and the skill numbers we got
//! on joining, through the same `SkillTuning::gives` / `earned_by` text the MCP `skills` tool uses.
//! Each level-up in our snapshot ("Edged 2") shows as the yellow line under the HUD
//! (`saving::Notice`, which also prints it to stdout).

use bevy::prelude::*;
use zombye_sim::SkillView;

use crate::player::LocalPlayer;
use crate::saving::Notice;
use crate::server_link::{JoinInfo, LevelUpsThisFrame, ServerLink};

use crate::theme::{self, DIM, GOLD, TEXT, font};

const BAR_W: f32 = 120.0;
/// The width of one skill's card (two to a row).
const CARD_W: f32 = 420.0;

/// Is the panel open.
#[derive(Resource, Default)]
pub struct SkillsPanel {
    pub open: bool,
}

#[derive(Component)]
struct PanelRoot;

/// What the panel shows, to rebuild it only when that changes.
#[derive(Resource, Default, PartialEq)]
struct Shown(Option<Vec<SkillView>>);

pub struct SkillsUiPlugin;

impl Plugin for SkillsUiPlugin {
    fn build(&self, app: &mut App) {
        // Open as it was left last session (`panels.rs`), or by `--skills`.
        let open = app.world().get_resource::<crate::Options>().is_some_and(|o| o.skills) || crate::ui_scale::SettingsStore::ensure(app).panels.skills.open;
        app.insert_resource(SkillsPanel { open }).init_resource::<Shown>().add_systems(Startup, spawn).add_systems(Update, (toggle, announce, rebuild).chain());
    }
}

fn spawn(mut commands: Commands) {
    commands.spawn((
        PanelRoot,
        crate::hud::TopPanel,
        crate::panels::PanelId::Skills,
        Visibility::Hidden,
        Node {
            position_type: PositionType::Absolute,
            top: px(70),
            left: px(340),
            flex_direction: FlexDirection::Column,
            row_gap: px(2),
            // (A little less above and below than a sheet usually has: all nine skills fit 1280 x 720.)
            padding: UiRect::axes(px(theme::SHEET_PAD.0), px(theme::SHEET_PAD.1 - 6.0)),
            max_width: px(2.0 * CARD_W + 10.0 + 2.0 * theme::SHEET_PAD.0),
            max_height: Val::Percent(85.0),
            overflow: Overflow::scroll_y(),
            ..default()
        },
        theme::sheet(),
        Pickable::IGNORE,
    ));
}

fn toggle(keys: Res<ButtonInput<KeyCode>>, mut panel: ResMut<SkillsPanel>) {
    if keys.just_pressed(KeyCode::KeyK) {
        panel.open = !panel.open;
    }
}

/// Our level-ups this frame: the yellow line under the HUD, and stdout.
fn announce(time: Res<Time>, ups: Res<LevelUpsThisFrame>, notice: Option<ResMut<Notice>>) {
    let Some(mut notice) = notice else { return };
    for l in &ups.0 {
        notice.say(time.elapsed_secs(), l.text());
    }
}

fn rebuild(mut commands: Commands, panel: Res<SkillsPanel>, link: Res<ServerLink>, join: Res<JoinInfo>, me: Res<LocalPlayer>, mut shown: ResMut<Shown>, mut root: Query<(Entity, &mut Visibility), With<PanelRoot>>) {
    let Ok((root, mut vis)) = root.single_mut() else { return };
    let want = if panel.open { Visibility::Inherited } else { Visibility::Hidden };
    if *vis != want {
        *vis = want;
    }
    if !panel.open {
        return;
    }
    let skills = link.own(me.id).map(|v| v.skills.clone()).unwrap_or_default();
    if shown.0.as_ref() == Some(&skills) {
        return;
    }
    commands.entity(root).despawn_children();
    let (t, carry, aid) = (&join.skills, &join.carry, &join.first_aid);
    commands.entity(root).with_children(|c| {
        // The title, with the hint beside it rather than on a line of its own at the foot: at the
        // readable text size (#97) all nine skills then still fit a 1280 x 720 screen.
        c.spawn((Node { justify_content: JustifyContent::Center, align_items: AlignItems::Center, min_height: px(22), ..default() }, Pickable::IGNORE)).with_children(|t| {
            t.spawn((Text::new("SKILLS"), theme::head(theme::TITLE), TextColor(TEXT)));
            t.spawn((Text::new("Practice raises them; nothing is lost."), font(theme::SMALL), TextColor(DIM), Node { position_type: PositionType::Absolute, left: px(0), ..default() }));
            t.spawn((Text::new("K: close"), font(theme::SMALL), TextColor(DIM), Node { position_type: PositionType::Absolute, right: px(0), ..default() }));
        });
        // Two columns of cards, so all nine fit on the screen.
        c.spawn((Node { width: px(2.0 * CARD_W + 10.0), flex_direction: FlexDirection::Row, flex_wrap: FlexWrap::Wrap, column_gap: px(10), ..default() }, Pickable::IGNORE)).with_children(|grid| {
            for v in &skills {
                let s = v.skill;
                grid.spawn((Node { width: px(CARD_W), flex_direction: FlexDirection::Column, margin: UiRect::top(px(6)), ..default() }, Pickable::IGNORE)).with_children(|card| {
                    card.spawn((Node { flex_direction: FlexDirection::Row, column_gap: px(8), align_items: AlignItems::Center, ..default() }, Pickable::IGNORE)).with_children(|r| {
                        r.spawn((Text::new(format!("{} {}", s.name(), v.level)), theme::strong(15.0), TextColor(if v.level > 0 { GOLD } else { TEXT }), Node { width: px(120), ..default() }));
                        // Filled as far as the label says: all the XP so far against the next level's.
                        let fill = v.next_at.map_or(1.0, |n| (v.xp / n.max(1.0)).clamp(0.0, 1.0));
                        r.spawn((Node { width: px(BAR_W), height: px(8), ..default() }, theme::bar_track())).with_children(|b| {
                            b.spawn((Node { width: px(BAR_W * fill), height: Val::Percent(100.0), ..default() }, theme::bar_fill(GOLD)));
                        });
                        let xp = match v.next_at {
                            Some(n) => format!("{:.0} / {n:.0} XP", v.xp),
                            None => format!("{:.0} XP, top level", v.xp),
                        };
                        r.spawn((Text::new(xp), font(crate::theme::SMALL), TextColor(DIM)));
                    });
                    // Detail lines indented as a block, so wrapped lines keep the indent.
                    let line = |card: &mut ChildSpawnerCommands, text: String, color: Color| {
                        card.spawn((Text::new(text), font(crate::theme::SMALL), TextColor(color), Node { margin: UiRect::left(px(12)), ..default() }));
                    };
                    if v.level > 0 {
                        line(card, format!("now: {}", t.gives(s, v.level, carry, aid)), TEXT);
                    }
                    if v.level < t.top() {
                        line(card, format!("level {}: {}", v.level + 1, t.gives(s, v.level + 1, carry, aid)), DIM);
                    }
                    line(card, format!("from {}", t.earned_by(s)), DIM);
                });
            }
        });
    });
    shown.0 = Some(skills);
}
