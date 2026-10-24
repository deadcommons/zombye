//! Skills (#46): the skills panel (K) and the level-up line.
//!
//! The panel lists every skill with its level (0-5), a bar of the way to the next level with the XP
//! so far and the XP it needs, what its level gives now and what the next one would, and how XP is
//! earned. It all comes from our private view (`PrivateView::skills`) and the skill numbers we got
//! on joining, through the same `SkillTuning::gives` / `earned_by` text the MCP `skills` tool uses.
//! Each level-up in our snapshot ("Edged 2") shows as the yellow line under the HUD
//! (`saving::Notice`, which also prints it to stdout).

use bevy::prelude::*;
use zombye_sim::skills::MAX_LEVEL;
use zombye_sim::SkillView;

use crate::player::LocalPlayer;
use crate::saving::Notice;
use crate::server_link::{JoinInfo, LevelUpsThisFrame, ServerLink};

const TEXT: Color = Color::srgb(0.93, 0.9, 0.82);
const DIM: Color = Color::srgb(0.65, 0.63, 0.58);
const GOLD: Color = Color::srgb(0.95, 0.85, 0.35);
const BAR_BG: Color = Color::srgba(0.2, 0.2, 0.22, 0.9);
const BAR: Color = Color::srgb(0.85, 0.7, 0.25);
const BAR_W: f32 = 120.0;

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
        app.init_resource::<SkillsPanel>().init_resource::<Shown>().add_systems(Startup, spawn).add_systems(Update, (toggle, announce, rebuild).chain());
    }
}

fn font(size: f32) -> TextFont {
    TextFont { font_size: bevy::text::FontSize::Px(size), ..default() }
}

fn spawn(mut commands: Commands) {
    commands.spawn((
        PanelRoot,
        Visibility::Hidden,
        Node {
            position_type: PositionType::Absolute,
            top: px(70),
            left: px(340),
            flex_direction: FlexDirection::Column,
            row_gap: px(2),
            padding: UiRect::all(px(10)),
            max_width: px(460),
            max_height: Val::Percent(85.0),
            overflow: Overflow::clip_y(),
            ..default()
        },
        BackgroundColor(Color::srgba(0.02, 0.02, 0.03, 0.8)),
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
        c.spawn((Text::new("Skills"), font(18.0), TextColor(TEXT)));
        for v in &skills {
            let s = v.skill;
            c.spawn((Node { flex_direction: FlexDirection::Row, column_gap: px(8), align_items: AlignItems::Center, margin: UiRect::top(px(5)), ..default() }, Pickable::IGNORE)).with_children(|r| {
                r.spawn((Text::new(format!("{} {}", s.name(), v.level)), font(15.0), TextColor(if v.level > 0 { GOLD } else { TEXT }), Node { width: px(130), ..default() }));
                r.spawn((Node { width: px(BAR_W), height: px(8), ..default() }, BackgroundColor(BAR_BG))).with_children(|b| {
                    b.spawn((Node { width: px(BAR_W * v.progress), height: Val::Percent(100.0), ..default() }, BackgroundColor(BAR)));
                });
                let xp = match v.next_at {
                    Some(n) => format!("{:.0} / {n:.0} XP", v.xp),
                    None => format!("{:.0} XP, top level", v.xp),
                };
                r.spawn((Text::new(xp), font(12.0), TextColor(DIM)));
            });
            if v.level > 0 {
                c.spawn((Text::new(format!("  now: {}", t.gives(s, v.level, carry, aid))), font(12.0), TextColor(TEXT)));
            }
            if v.level < MAX_LEVEL {
                c.spawn((Text::new(format!("  level {}: {}", v.level + 1, t.gives(s, v.level + 1, carry, aid))), font(12.0), TextColor(DIM)));
            }
            c.spawn((Text::new(format!("  from {}", t.earned_by(s))), font(12.0), TextColor(DIM)));
        }
        c.spawn((Text::new("Skills only grow with practice; nothing is lost.   K: close"), font(12.0), TextColor(DIM), Node { margin: UiRect::top(px(6)), ..default() }));
    });
    shown.0 = Some(skills);
}
