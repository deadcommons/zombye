//! The health panel (H) and the health tags under the vitals bars (#9).
//!
//! The panel lists your wounds by body part, head to feet: the kind, whether it bleeds and how
//! fast, how far it has healed and the hours to go, the dressing, festering; plus HP lost per game
//! hour, pain, food poisoning and the zombie infection once it shows (Feverish, then Turning). The
//! tags line (bottom left, in the vitals panel) says BLEEDING, INJURED ARM / LEG, FESTERING,
//! FEVERISH / TURNING, FOOD POISONING, and for a few seconds where the last strike landed. All of
//! it comes from our private view in the latest snapshot: the client never works it out itself.

use bevy::prelude::*;
use zombye_net::PrivateView;
use zombye_sim::{CombatEventKind, Dressing, HealthView, InfectionStage, Part};

use crate::player::LocalPlayer;
use crate::server_link::{CombatEventsThisFrame, JoinInfo, ServerLink};

/// Is the panel open?
#[derive(Resource, Default)]
pub struct HealthPanel {
    pub open: bool,
}

/// The tags line in the vitals panel (spawned by `hud.rs`).
#[derive(Component)]
pub struct HealthTagsText;

#[derive(Component)]
struct PanelRoot;

/// Where the last strike on us landed, and when (seconds).
#[derive(Resource, Default)]
struct LastHit {
    text: String,
    at: f32,
}

/// What the panel shows, to rebuild it only when that changes.
#[derive(Resource, Default, PartialEq)]
struct Shown(String);

const TEXT: Color = Color::srgb(0.93, 0.9, 0.82);
const DIM: Color = Color::srgb(0.65, 0.63, 0.58);
const BLOOD: Color = Color::srgb(0.95, 0.35, 0.3);
const SICK: Color = Color::srgb(0.75, 0.85, 0.35);
const HIT_SECS: f32 = 3.0;

pub struct HealthUiPlugin;

impl Plugin for HealthUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<HealthPanel>().init_resource::<LastHit>().init_resource::<Shown>().add_systems(Startup, spawn).add_systems(Update, (toggle, last_hit, tags, rebuild).chain());
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
            left: px(16),
            flex_direction: FlexDirection::Column,
            row_gap: px(3),
            padding: UiRect::all(px(10)),
            min_width: px(300),
            ..default()
        },
        BackgroundColor(Color::srgba(0.02, 0.02, 0.03, 0.8)),
    ));
}

fn toggle(keys: Res<ButtonInput<KeyCode>>, mut panel: ResMut<HealthPanel>) {
    if keys.just_pressed(KeyCode::KeyH) {
        panel.open = !panel.open;
    }
}

fn last_hit(time: Res<Time>, me: Res<LocalPlayer>, events: Res<CombatEventsThisFrame>, mut last: ResMut<LastHit>) {
    if let Some(e) = events.0.iter().rev().find(|e| e.kind == CombatEventKind::PlayerHurt && e.player == me.id)
        && let (Some(part), Some(kind)) = (e.part, e.wound)
    {
        *last = LastHit { text: format!("{} on the {}", kind.name(), part.name()), at: time.elapsed_secs() };
    }
}

/// The HUD's tags for our health.
fn tag_line(h: &HealthView) -> String {
    let mut tags = Vec::new();
    let bleeding = h.wounds.iter().filter(|w| w.bleed_per_h > 0.0).count();
    match bleeding {
        0 => {}
        1 => tags.push("BLEEDING".to_string()),
        n => tags.push(format!("BLEEDING x{n}")),
    }
    match h.infection {
        Some(InfectionStage::Feverish) => tags.push("FEVERISH".into()),
        Some(InfectionStage::Turning) => tags.push("TURNING".into()),
        _ => {}
    }
    if h.food_poisoning {
        tags.push("FOOD POISONING".into());
    }
    if h.wounds.iter().any(|w| w.festering) {
        tags.push("FESTERING".into());
    }
    if h.tags.limping {
        tags.push("INJURED LEG".into());
    }
    if h.tags.injured_arm {
        tags.push("INJURED ARM".into());
    }
    tags.join("  ")
}

fn tags(time: Res<Time>, link: Res<ServerLink>, me: Res<LocalPlayer>, last: Res<LastHit>, mut text: Query<(&mut Text, &mut TextColor), With<HealthTagsText>>) {
    let Ok((mut t, mut color)) = text.single_mut() else { return };
    let mut line = if me.dead { String::new() } else { link.own(me.id).map_or(String::new(), |v| tag_line(&v.health)) };
    if !me.dead && !last.text.is_empty() && time.elapsed_secs() - last.at < HIT_SECS {
        line = if line.is_empty() { format!("hit: {}", last.text) } else { format!("{line}   hit: {}", last.text) };
    }
    if t.0 != line {
        t.0 = line;
    }
    let sick = link.own(me.id).is_some_and(|v| v.health.infection.is_some() || v.health.food_poisoning);
    let want = if link.own(me.id).is_some_and(|v| v.health.tags.bleeding) { BLOOD } else if sick { SICK } else { TEXT };
    if color.0 != want {
        color.0 = want;
    }
}

fn wound_line(w: &zombye_sim::WoundView) -> (String, Color) {
    let mut bits = Vec::new();
    if w.bleed_per_h > 0.0 {
        bits.push(format!("bleeding {:.0}/h", w.bleed_per_h));
    }
    bits.push(format!("{:.0}% healed", w.healed * 100.0));
    bits.push(w.heals_in_h.map_or("not healing".to_string(), |h| format!("~{h:.0} h to go")));
    if w.dressing != Dressing::None {
        bits.push(if w.dressing == Dressing::Clean { "clean dressing".into() } else { "dirty dressing".into() });
    }
    if w.stitched {
        bits.push("stitched".into());
    }
    if w.festering {
        bits.push("FESTERING".into());
    }
    let color = if w.bleed_per_h > 0.0 {
        BLOOD
    } else if w.festering {
        SICK
    } else {
        TEXT
    };
    (format!("  {}: {}", w.kind.name(), bits.join(", ")), color)
}

fn rebuild(
    mut commands: Commands,
    panel: Res<HealthPanel>,
    link: Res<ServerLink>,
    join: Res<JoinInfo>,
    me: Res<LocalPlayer>,
    mut shown: ResMut<Shown>,
    mut root: Query<(Entity, &mut Visibility), With<PanelRoot>>,
) {
    let Ok((root, mut vis)) = root.single_mut() else { return };
    let want = if panel.open { Visibility::Inherited } else { Visibility::Hidden };
    if *vis != want {
        *vis = want;
    }
    if !panel.open {
        return;
    }
    let view: Option<&PrivateView> = link.own(me.id);
    let health = view.map(|v| v.health.clone()).unwrap_or_default();
    let signature = format!("{health:?}|{:.0}|{}", me.hp, me.dead);
    if shown.0 == signature {
        return;
    }
    shown.0 = signature;
    commands.entity(root).despawn_children();
    commands.entity(root).with_children(|c| {
        let line = |c: &mut ChildSpawnerCommands, s: String, size: f32, color: Color| {
            c.spawn((Text::new(s), font(size), TextColor(color)));
        };
        line(c, "Health".into(), 18.0, TEXT);
        line(c, format!("HP {:.0} of {:.0}{}", me.hp.max(0.0), join.combat.player_hp, if me.dead { " (dead)" } else { "" }), 14.0, TEXT);
        if health.losing_per_h > 0.0 {
            line(c, format!("losing {:.1} HP per game hour", health.losing_per_h), 14.0, BLOOD);
        }
        if health.pain > 0.0 {
            line(c, format!("pain {:.0}%", (health.pain * 100.0).min(999.0)), 14.0, TEXT);
        }
        match health.infection {
            Some(InfectionStage::Feverish) => line(c, "Feverish: no healing, slow to get your breath back".into(), 14.0, SICK),
            Some(InfectionStage::Turning) => line(c, "Turning: losing HP fast, night sight halved".into(), 14.0, SICK),
            _ => {}
        }
        if health.food_poisoning {
            line(c, "Food poisoning: can't eat, thirsty".into(), 14.0, SICK);
        }
        if health.wounds.is_empty() {
            line(c, "No wounds".into(), 14.0, DIM);
        }
        for part in Part::ALL {
            let here: Vec<_> = health.wounds.iter().filter(|w| w.part == part).collect();
            if here.is_empty() {
                continue;
            }
            let mut name = part.name().to_string();
            name[..1].make_ascii_uppercase();
            line(c, name, 14.0, DIM);
            for w in here {
                let (s, color) = wound_line(w);
                line(c, s, 13.0, color);
            }
        }
        line(c, "H: close".into(), 12.0, DIM);
    });
}
