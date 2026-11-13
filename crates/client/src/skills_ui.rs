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

use crate::theme::{self, flat, font};

/// The width of one skill's card (two to a row).
const CARD_W: f32 = 330.0;

/// Is the panel open.
#[derive(Resource, Default)]
pub struct SkillsPanel {
    pub open: bool,
}

#[derive(Component)]
struct PanelRoot;

/// What the panel shows, to rebuild it only when that changes.
#[derive(Resource, Default, PartialEq)]
struct Shown(Option<(Vec<SkillView>, crate::help_text::HelpText)>);

pub struct SkillsUiPlugin;

impl Plugin for SkillsUiPlugin {
    fn build(&self, app: &mut App) {
        // A screen of the frame, opened by K, the rail or `--skills` (not left open between sessions).
        let open = app.world().get_resource::<crate::Options>().is_some_and(|o| o.skills);
        app.insert_resource(SkillsPanel { open }).init_resource::<Shown>().add_systems(Startup, spawn).add_systems(Update, (toggle, announce, rebuild).chain());
    }
}

fn spawn(mut commands: Commands) {
    // The middle of the frame (`inventory_ui`): between its rail, top bar and footer, over its veil.
    commands.spawn((PanelRoot, GlobalZIndex(7), Visibility::Hidden, Node { position_type: PositionType::Absolute, left: px(flat::RAIL), top: px(flat::TOP_BAR), right: px(0), bottom: px(flat::FOOTER), flex_direction: FlexDirection::Column, ..default() }));
}

pub(crate) fn toggle(keys: Res<ButtonInput<KeyCode>>, mut panel: ResMut<SkillsPanel>) {
    if crate::keys::just_pressed(&keys, crate::keys::Action::Skills) {
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

fn rebuild(mut commands: Commands, panel: Res<SkillsPanel>, link: Res<ServerLink>, join: Res<JoinInfo>, me: Res<LocalPlayer>, scrolls: Option<Res<crate::inventory_ui::Scrolls>>, mut shown: ResMut<Shown>, mut root: Query<(Entity, &mut Visibility), With<PanelRoot>>) {
    let Ok((root, mut vis)) = root.single_mut() else { return };
    let want = if panel.open { Visibility::Inherited } else { Visibility::Hidden };
    if *vis != want {
        *vis = want;
    }
    if !panel.open {
        return;
    }
    let skills = link.own(me.id).map(|v| v.skills.clone()).unwrap_or_default();
    // (With the help text short a card keeps what its level gives now; the rest is explaining.)
    let help = crate::help_text::level();
    if shown.0.as_ref().is_some_and(|(s, h)| *s == skills && *h == help) {
        return;
    }
    commands.entity(root).despawn_children();
    let (t, carry, aid) = (&join.skills, &join.carry, &join.first_aid);
    let y = scrolls.map_or(0.0, |s| s.0[5]);
    commands.entity(root).with_children(|c| {
        c.spawn((Node { flex_direction: FlexDirection::Row, justify_content: JustifyContent::SpaceBetween, align_items: AlignItems::FlexEnd, padding: UiRect::new(px(24), px(24), px(18), px(12)), flex_shrink: 0.0, ..default() }, Pickable::IGNORE)).with_children(|h| {
            h.spawn((Text::new("SKILLS"), theme::head(flat::TITLE), TextColor(flat::BONE), Pickable::IGNORE));
            if help.explains() {
                h.spawn((Text::new("Practice raises them. Nothing is lost."), font(flat::SMALL), TextColor(flat::DIM), Pickable::IGNORE));
            }
        });
        crate::inventory_ui::scroller(c, 5, y, UiRect::new(px(24), px(24), px(0), px(16)), 0.0).with_children(|s| {
            // Cards that wrap: three across on a small screen, more on a wide one.
            s.spawn((Node { flex_direction: FlexDirection::Row, flex_wrap: FlexWrap::Wrap, column_gap: px(12), row_gap: px(12), flex_shrink: 0.0, ..default() }, Pickable::IGNORE)).with_children(|grid| {
                for v in &skills {
                    let sk = v.skill;
                    let colour = if v.level > 0 { flat::GREEN } else { flat::FAINT };
                    grid.spawn((Node { flex_grow: 1.0, flex_basis: px(CARD_W), min_width: px(CARD_W), max_width: px(CARD_W * 1.6), flex_direction: FlexDirection::Column, row_gap: px(5), padding: UiRect::axes(px(14), px(10)), border: UiRect::all(px(1)), border_radius: BorderRadius::all(px(flat::RADIUS)), ..default() }, BorderColor::all(flat::LINE), BackgroundColor(flat::CELL), Pickable::IGNORE)).with_children(|card| {
                        card.spawn((Node { flex_direction: FlexDirection::Row, justify_content: JustifyContent::SpaceBetween, align_items: AlignItems::FlexEnd, ..default() }, Pickable::IGNORE)).with_children(|r| {
                            let xp = match v.next_at {
                                Some(n) => format!("{:.0} / {n:.0} XP", v.xp),
                                None => format!("{:.0} XP, top level", v.xp),
                            };
                            r.spawn((Node { flex_direction: FlexDirection::Row, align_items: AlignItems::FlexEnd, column_gap: px(10), ..default() }, Pickable::IGNORE)).with_children(|n| {
                                n.spawn((Text::new(sk.name().to_uppercase()), theme::head(flat::HEAD), TextColor(flat::BONE), TextLayout::no_wrap(), Pickable::IGNORE));
                                n.spawn((Text::new(xp), font(flat::SMALL), TextColor(flat::DIM), TextLayout::no_wrap(), Node { margin: UiRect::bottom(px(1)), ..default() }, Pickable::IGNORE));
                            });
                            r.spawn((Node { flex_direction: FlexDirection::Row, align_items: AlignItems::FlexEnd, column_gap: px(4), ..default() }, Pickable::IGNORE)).with_children(|n| {
                                n.spawn((Text::new(v.level.to_string()), theme::strong(flat::TITLE), TextColor(if v.level > 0 { flat::GREEN } else { flat::DIM }), Pickable::IGNORE));
                                n.spawn((Text::new(format!("/ {}", t.top())), font(flat::SMALL), TextColor(flat::DIM), Node { margin: UiRect::bottom(px(3)), ..default() }, Pickable::IGNORE));
                            });
                        });
                        // Filled as far as the label says: all the XP so far against the next level's.
                        flat::bar(card, v.next_at.map_or(1.0, |n| (v.xp / n.max(1.0)).clamp(0.0, 1.0)), colour, Node { height: px(5), ..default() });
                        let line = |card: &mut ChildSpawnerCommands, text: String, color: Color| {
                            card.spawn((Text::new(text), font(flat::SMALL), TextColor(color), Pickable::IGNORE));
                        };
                        if v.level > 0 {
                            line(card, format!("Now: {}", t.gives(sk, v.level, carry, aid)), flat::BONE);
                        }
                        if help.explains() {
                            if v.level < t.top() {
                                line(card, format!("Level {}: {}", v.level + 1, t.gives(sk, v.level + 1, carry, aid)), flat::DIM);
                            }
                            line(card, format!("From {}", t.earned_by(sk)), flat::DIM);
                        }
                    });
                }
            });
        });
    });
    shown.0 = Some((skills, help));
}
