//! The health panel (H) and the health tags under the vitals bars (#9), with first aid (#10).
//!
//! The panel lists your wounds by body part, head to feet: the kind, whether it bleeds and how
//! fast, how far it has healed and the hours to go, the dressing (clean or dirty, and when a clean
//! one turns), cleaned, stitched, festering; plus HP lost per game hour, pain, painkillers and
//! antibiotics, food poisoning and the zombie infection once it shows (Feverish, then Turning).
//! **Click a wound** to see what it needs and the treatments you can give it with what you carry
//! (the same rule the server checks, `zombye_sim::firstaid::options`); click one to do it. Pills
//! are offered at the foot. Right-clicking a dressing, antiseptic or sewing kit in the inventory
//! opens the panel to **use it on** a wound: the wounds it fits are marked; right-click cancels.
//! While the panel is open, left mouse doesn't swing. The tags line (bottom left, in the vitals
//! panel) says BLEEDING, INJURED ARM / LEG, FESTERING, FEVERISH / TURNING, FOOD POISONING, and for
//! a few seconds where the last strike landed. All of it comes from our private view in the latest
//! snapshot: the client never works it out itself.

use bevy::prelude::*;
use zombye_net::{Intent, PrivateView};
use zombye_sim::firstaid::{self, Treatment};
use zombye_sim::{CombatEventKind, Dressing, HealthView, InfectionStage, ItemDef, Part, WoundView};

use crate::player::LocalPlayer;
use crate::server_link::{CombatEventsThisFrame, JoinInfo, ServerLink};

/// Is the panel open, which wound is picked, and which item is being used on a wound.
#[derive(Resource, Default)]
pub struct HealthPanel {
    pub open: bool,
    /// The wound whose treatments are shown (its id).
    pub selected: Option<u32>,
    /// "Use on…" from the inventory: the item to treat a wound with, waiting for a wound.
    pub use_item: Option<u16>,
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

/// A press on a panel row, as its observer saw it.
#[derive(Clone, Debug)]
enum PanelClick {
    /// Show (or hide) a wound's treatments; with an item to use, treat it with that.
    Wound(u32),
    Send(Intent),
    /// Right-click: stop using an item on a wound.
    Cancel,
}

/// Presses on rows since [`send_clicks`] last ran.
#[derive(Resource, Default)]
struct Clicks(Vec<PanelClick>);

const TEXT: Color = Color::srgb(0.93, 0.9, 0.82);
const DIM: Color = Color::srgb(0.65, 0.63, 0.58);
const BLOOD: Color = Color::srgb(0.95, 0.35, 0.3);
const SICK: Color = Color::srgb(0.75, 0.85, 0.35);
const OK: Color = Color::srgb(0.95, 0.85, 0.35);
const ROW: Color = Color::srgba(0.12, 0.12, 0.14, 0.85);
const ROW_PICKED: Color = Color::srgba(0.22, 0.2, 0.12, 0.95);
const ROW_HOVER: Color = Color::srgba(0.28, 0.26, 0.2, 0.95);
const HIT_SECS: f32 = 3.0;

pub struct HealthUiPlugin;

impl Plugin for HealthUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<HealthPanel>()
            .init_resource::<LastHit>()
            .init_resource::<Shown>()
            .init_resource::<Clicks>()
            .add_systems(Startup, spawn)
            .add_systems(Update, (toggle, last_hit, tags, send_clicks, rebuild).chain());
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
            max_height: Val::Percent(85.0),
            overflow: Overflow::clip_y(),
            ..default()
        },
        BackgroundColor(Color::srgba(0.02, 0.02, 0.03, 0.8)),
    ));
}

fn toggle(keys: Res<ButtonInput<KeyCode>>, mut panel: ResMut<HealthPanel>) {
    if keys.just_pressed(KeyCode::KeyH) {
        panel.open = !panel.open;
        panel.use_item = None;
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
    if h.aid.drowsy {
        tags.push("DROWSY".into());
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

/// The dressing on a wound, in words ("clean gauze, dirty in ~5 h").
fn dressing_text(w: &WoundView) -> Option<String> {
    let what = if w.plaster { "plaster" } else { "dressing" };
    match (w.dressing, w.dirty_in_h) {
        (Dressing::None, _) => None,
        (Dressing::Clean, Some(h)) => Some(format!("clean {what}, dirty in ~{h:.0} h")),
        (Dressing::Clean, None) => Some(format!("clean {what}")),
        (Dressing::Dirty, _) => Some(format!("DIRTY {what}")),
    }
}

fn wound_line(w: &WoundView) -> (String, Color) {
    let mut bits = Vec::new();
    if w.bleed_per_h > 0.0 {
        bits.push(format!("bleeding {:.0}/h", w.bleed_per_h));
    }
    bits.push(format!("{:.0}% healed", w.healed * 100.0));
    bits.push(w.heals_in_h.map_or("not healing".to_string(), |h| format!("~{h:.0} h to go")));
    bits.extend(dressing_text(w));
    if w.cleaned {
        bits.push("cleaned".into());
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
    (format!("{}: {}", w.kind.name(), bits.join(", ")), color)
}

/// The button text for a treatment ("Dress with the gauze roll (2 of 3 left), 4 s").
fn option_label(t: Treatment, item: Option<u16>, w: Option<&WoundView>, view: &PrivateView, items: &[ItemDef], join: &JoinInfo) -> String {
    let def = item.and_then(|i| items.get(i as usize));
    // Faster with First Aid (#46).
    let secs = join.first_aid.secs(t, w, def, view.skill_level(zombye_sim::Skill::FirstAid));
    let mut s = t.verb().to_string();
    s[..1].make_ascii_uppercase();
    if let (Some(d), Some(i)) = (def, item)
        && t.on_wound()
    {
        s.push_str(&format!(" with the {}", d.name.to_lowercase()));
        let uses = d.medical.as_ref().map_or(1, |m| m.uses);
        if let Some(o) = view.health.aid.opened.iter().find(|o| o.item == i) {
            s.push_str(&format!(" ({} of {uses} left)", uses.saturating_sub(o.used)));
        }
    }
    if t == Treatment::Dress && w.is_some_and(|w| w.dressing != Dressing::None) {
        s = s.replacen("Dress", "Change the dressing", 1);
    }
    format!("{s}, {secs:.0} s")
}

/// This frame's presses on panel rows: at most one acted on.
fn send_clicks(mut clicks: ResMut<Clicks>, mut panel: ResMut<HealthPanel>, me: Res<LocalPlayer>, join: Res<JoinInfo>, mut link: ResMut<ServerLink>) {
    let Some(click) = clicks.0.drain(..).next() else { return };
    match click {
        PanelClick::Wound(id) => match panel.use_item.take() {
            Some(item) => {
                let action = join.items.get(item as usize).and_then(Treatment::of_item).unwrap_or(Treatment::Dress);
                link.send(Intent::Treat { player: me.id, wound: Some(id), action, item: Some(item) });
                panel.selected = Some(id);
            }
            None => panel.selected = if panel.selected == Some(id) { None } else { Some(id) },
        },
        PanelClick::Send(intent) => {
            link.send(intent);
            panel.use_item = None;
        }
        PanelClick::Cancel => panel.use_item = None,
    }
}

#[allow(clippy::too_many_arguments)]
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
    let view: Option<PrivateView> = link.own(me.id).cloned();
    let busy = link.curr.players.iter().find(|p| p.id == me.id).map_or(0.0, |p| p.busy);
    let health = view.as_ref().map(|v| v.health.clone()).unwrap_or_default();
    let signature = format!("{view:?}|{:.0}|{}|{:?}|{:?}|{:.1}", me.hp, me.dead, panel.selected, panel.use_item, busy);
    if shown.0 == signature {
        return;
    }
    shown.0 = signature;
    commands.entity(root).despawn_children();
    let items = &join.items;
    let use_def = panel.use_item.and_then(|i| items.get(i as usize));
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
        if let Some(h) = health.aid.painkillers_h {
            line(c, format!("painkillers working, ~{h:.0} h{}", if health.aid.drowsy { " (two doses: drowsy)" } else { "" }), 13.0, DIM);
        }
        if let Some(course) = health.aid.course {
            let s = match course.lapses_in_h {
                Some(l) if course.next_in_h > 0.0 => format!("antibiotics {} of {}: next in ~{:.0} h (lapses in ~{l:.0} h)", course.doses, course.of, course.next_in_h),
                Some(l) => format!("antibiotics {} of {}: next due now (lapses in ~{l:.0} h)", course.doses, course.of),
                None => "antibiotics: course done".to_string(),
            };
            line(c, s, 13.0, DIM);
        }
        match health.infection {
            Some(InfectionStage::Feverish) => line(c, "Feverish: no healing, slow to get your breath back".into(), 14.0, SICK),
            Some(InfectionStage::Turning) => line(c, "Turning: losing HP fast, night sight halved".into(), 14.0, SICK),
            _ => {}
        }
        if health.food_poisoning {
            line(c, "Food poisoning: can't eat, thirsty".into(), 14.0, SICK);
        }
        if busy > 0.0 {
            line(c, format!("busy: {busy:.1} s"), 13.0, OK);
        }
        if let Some(d) = use_def {
            line(c, format!("Use the {} on: (right-click to cancel)", d.name.to_lowercase()), 14.0, OK);
        }
        if health.wounds.is_empty() {
            line(c, "No wounds".into(), 14.0, DIM);
        }
        let Some(view) = view.as_ref() else { return };
        for part in Part::ALL {
            let here: Vec<&WoundView> = health.wounds.iter().filter(|w| w.part == part).collect();
            if here.is_empty() {
                continue;
            }
            let mut name = part.name().to_string();
            name[..1].make_ascii_uppercase();
            line(c, name, 14.0, DIM);
            for w in here {
                let (s, color) = wound_line(w);
                let fits = use_def.is_some_and(|d| Treatment::of_item(d).is_some_and(|t| firstaid::check(t, Some(w), Some(d)).is_ok()));
                let picked = panel.selected == Some(w.id);
                let mark = if fits { "> " } else { "" };
                row(c, format!("  {mark}{s}"), color, if picked || fits { ROW_PICKED } else { ROW }, PanelClick::Wound(w.id));
                if !picked || use_def.is_some() {
                    continue;
                }
                line(c, format!("    needs {}", firstaid::needs(w)), 12.0, DIM);
                let options = firstaid::options(Some(w), &view.inventory, items);
                if options.is_empty() {
                    line(c, "    nothing you carry treats it".into(), 12.0, DIM);
                }
                for (t, item) in options {
                    let label = option_label(t, item, Some(w), view, items, &join);
                    row(c, format!("    {label}"), OK, ROW, PanelClick::Send(Intent::Treat { player: me.id, wound: Some(w.id), action: t, item }));
                }
            }
        }
        let pills = firstaid::options(None, &view.inventory, items);
        if !pills.is_empty() {
            line(c, "Take".into(), 14.0, DIM);
            for (t, item) in pills {
                let label = option_label(t, item, None, view, items, &join);
                row(c, format!("  {label}"), OK, ROW, PanelClick::Send(Intent::Treat { player: me.id, wound: None, action: t, item }));
            }
        }
        line(c, "Click a wound to treat it   H: close".into(), 12.0, DIM);
    });
}

/// A clickable row. A press is only recorded here ([`Clicks`]); [`send_clicks`] acts on it.
fn row(c: &mut ChildSpawnerCommands, label: String, color: Color, bg: Color, click: PanelClick) {
    c.spawn((Node { padding: UiRect::axes(px(6), px(2)), ..default() }, BackgroundColor(bg)))
        .with_children(|r| {
            r.spawn((Text::new(label), font(13.0), TextColor(color), Pickable::IGNORE));
        })
        .observe(|ev: On<PointerOver>, mut q: Query<&mut BackgroundColor>| {
            if let Ok(mut b) = q.get_mut(ev.entity) {
                b.0 = ROW_HOVER;
            }
        })
        .observe(move |ev: On<PointerOut>, mut q: Query<&mut BackgroundColor>| {
            if let Ok(mut b) = q.get_mut(ev.entity) {
                b.0 = bg;
            }
        })
        .observe(move |ev: On<PointerPress>, mut clicks: ResMut<Clicks>| match ev.button {
            PointerButton::Primary => clicks.0.push(click.clone()),
            PointerButton::Secondary => clicks.0.push(PanelClick::Cancel),
            _ => {}
        });
}
