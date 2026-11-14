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
//! panel) says INJURED ARM / LEG and DROWSY (PAINKILLERS), and for a few seconds where the last
//! strike landed; bleeding, sickness and the rest are status tags (`needs_ui.rs`). All of it comes from our private view in the latest
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

use crate::theme::{self, TEXT, flat, font};

const BLOOD: Color = theme::BAD;
const HIT_SECS: f32 = 3.0;

pub struct HealthUiPlugin;

impl Plugin for HealthUiPlugin {
    fn build(&self, app: &mut App) {
        // A screen of the frame, opened by H, the rail or `--health` (not left open between sessions).
        let open = app.world().get_resource::<crate::Options>().is_some_and(|o| o.health);
        app.insert_resource(HealthPanel { open, ..default() })
            .init_resource::<LastHit>()
            .init_resource::<Shown>()
            .init_resource::<Clicks>()
            .add_systems(Startup, spawn)
            .add_systems(Update, (toggle, preselect, last_hit, tags, send_clicks, rebuild).chain());
    }
}

fn spawn(mut commands: Commands) {
    // The middle of the frame (`inventory_ui`): between its rail, top bar and footer, over its veil.
    commands.spawn((PanelRoot, GlobalZIndex(7), Visibility::Hidden, Node { position_type: PositionType::Absolute, left: px(flat::RAIL), top: px(flat::TOP_BAR), right: px(0), bottom: px(flat::FOOTER), flex_direction: FlexDirection::Row, ..default() }));
}

/// As the screen opens, the wound that most needs seeing to is the one picked (unless an item
/// is waiting for a wound, or the one picked before is still there).
fn preselect(mut panel: ResMut<HealthPanel>, link: Res<ServerLink>, me: Res<LocalPlayer>, mut was: Local<bool>) {
    let opened = panel.open && !*was;
    *was = panel.open;
    if !opened || panel.use_item.is_some() {
        return;
    }
    let Some(v) = link.own(me.id) else { return };
    let wounds = &v.health.wounds;
    if panel.selected.is_some_and(|id| wounds.iter().any(|w| w.id == id)) {
        return;
    }
    let worst = wounds.iter().max_by(|a, b| (a.bleed_per_h, a.festering).partial_cmp(&(b.bleed_per_h, b.festering)).unwrap_or(std::cmp::Ordering::Equal)).map(|w| w.id);
    if panel.selected != worst {
        panel.selected = worst;
    }
}

pub(crate) fn toggle(keys: Res<ButtonInput<KeyCode>>, me: Option<Res<LocalPlayer>>, mut panel: ResMut<HealthPanel>) {
    // At the wheel the key is the horn's (`keys::Action::Horn`, `player.rs`).
    if crate::keys::just_pressed(&keys, crate::keys::Action::Health) && !me.is_some_and(|m| m.car.is_some()) {
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

/// The HUD's tags for our health that the status tags (`needs_ui.rs`) don't show: an injured leg
/// or arm, drowsy from a double dose of painkillers. (Bleeding, the fever, festering and food
/// poisoning are status tags, #57.)
fn tag_line(h: &HealthView) -> String {
    let mut tags = Vec::new();
    if h.aid.drowsy {
        tags.push("DROWSY (PAINKILLERS)");
    }
    if h.tags.limping {
        tags.push("INJURED LEG");
    }
    if h.tags.injured_arm {
        tags.push("INJURED ARM");
    }
    tags.join("  ")
}

fn tags(time: Res<Time>, link: Res<ServerLink>, me: Res<LocalPlayer>, last: Res<LastHit>, mut text: Query<(&mut Text, &mut TextColor), With<HealthTagsText>>) {
    let Ok((mut t, mut color)) = text.single_mut() else { return };
    let mut line = if me.dead { String::new() } else { link.own(me.id).map_or(String::new(), |v| tag_line(&v.health)) };
    if !me.dead && !last.text.is_empty() && time.elapsed_secs() - last.at < HIT_SECS {
        line = if line.is_empty() { format!("hit: {}", last.text) } else { format!("{line}   hit: {}", last.text) };
    }
    let want = if line.contains("hit:") { BLOOD } else { TEXT };
    if t.0 != line {
        t.0 = line;
    }
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

/// The colour of how much health is left.
fn hp_colour(share: f32) -> Color {
    if share < 0.25 {
        flat::RED
    } else if share < 0.5 {
        flat::AMBER
    } else {
        flat::GREEN
    }
}

/// A status's colour by how bad it is (0: a good state).
fn severity_colour(severity: u8) -> Color {
    match severity {
        0 => flat::GREEN,
        1 => flat::BONE,
        2 | 3 => flat::AMBER,
        _ => flat::RED_TEXT,
    }
}

/// What a wound's card says after its name: (text, colour) chips, the worst first.
fn wound_chips(w: &WoundView) -> Vec<(String, Color)> {
    let mut chips = Vec::new();
    if w.bleed_per_h > 0.0 {
        chips.push((format!("bleeding {:.0}/h", w.bleed_per_h), flat::RED_TEXT));
    }
    if w.festering {
        chips.push(("festering".into(), flat::AMBER));
    }
    if let Some(d) = dressing_text(w) {
        chips.push((d, if w.dressing == Dressing::Dirty { flat::AMBER } else { flat::DIM }));
    }
    if w.cleaned {
        chips.push(("cleaned".into(), flat::DIM));
    }
    if w.stitched {
        chips.push(("stitched".into(), flat::DIM));
    }
    chips
}

#[allow(clippy::too_many_arguments)]
fn rebuild(
    mut commands: Commands,
    panel: Res<HealthPanel>,
    link: Res<ServerLink>,
    join: Res<JoinInfo>,
    me: Res<LocalPlayer>,
    icons: Option<Res<crate::needs_ui::StatusIcons>>,
    scrolls: Option<Res<crate::inventory_ui::Scrolls>>,
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
    let status = icons.as_ref().map_or(Vec::new(), |i| i.tags(&link, &join, &me));
    let signature = format!("{view:?}|{:.0}|{}|{:?}|{:?}|{:.1}|{status:?}|{:?}", me.hp, me.dead, panel.selected, panel.use_item, busy, crate::help_text::level());
    if shown.0 == signature {
        return;
    }
    shown.0 = signature;
    commands.entity(root).despawn_children();
    let items = &join.items;
    let use_def = panel.use_item.and_then(|i| items.get(i as usize));
    let y = scrolls.map_or((0.0, 0.0), |s| (s.0[4], s.0[6]));
    let head = |c: &mut ChildSpawnerCommands, s: &str| {
        c.spawn((Text::new(s.to_uppercase()), theme::head(flat::SMALL), TextColor(flat::DIM), Node { margin: UiRect::top(px(10)), flex_shrink: 0.0, ..default() }, Pickable::IGNORE));
    };
    let line = |c: &mut ChildSpawnerCommands, s: String, color: Color| {
        c.spawn((Text::new(s), font(flat::BODY), TextColor(color), Node { flex_shrink: 0.0, ..default() }, Pickable::IGNORE));
    };
    commands.entity(root).with_children(|c| {
        // ---- Condition: how you are, what ails you, and the pills you can take ----
        c.spawn((Node { width: percent(34), min_width: px(340), max_width: px(480), flex_direction: FlexDirection::Column, border: UiRect::right(px(1)), flex_shrink: 0.0, ..default() }, BorderColor::all(flat::LINE))).with_children(|c| {
          crate::inventory_ui::scroller(c, 6, y.1, UiRect::new(px(24), px(24), px(18), px(16)), 6.0).with_children(|c| {
            c.spawn((Text::new("CONDITION"), theme::head(flat::TITLE), TextColor(flat::BONE), Pickable::IGNORE));
            let share = (me.hp / join.combat.player_hp.max(1.0)).clamp(0.0, 1.0);
            c.spawn((Node { flex_direction: FlexDirection::Row, justify_content: JustifyContent::SpaceBetween, align_items: AlignItems::FlexEnd, margin: UiRect::top(px(6)), ..default() }, Pickable::IGNORE)).with_children(|t| {
                t.spawn((Text::new(if me.dead { "DEAD" } else { "HEALTH" }), theme::head(flat::HEAD), TextColor(flat::DIM), Pickable::IGNORE));
                t.spawn((Node { flex_direction: FlexDirection::Row, align_items: AlignItems::FlexEnd, column_gap: px(5), ..default() }, Pickable::IGNORE)).with_children(|n| {
                    n.spawn((Text::new(format!("{:.0}", me.hp.max(0.0))), theme::strong(flat::TITLE), TextColor(flat::BONE), Pickable::IGNORE));
                    n.spawn((Text::new(format!("/ {:.0}", join.combat.player_hp)), font(flat::BODY), TextColor(flat::DIM), Node { margin: UiRect::bottom(px(3)), ..default() }, Pickable::IGNORE));
                });
            });
            flat::bar(c, share, hp_colour(share), Node { height: px(6), flex_shrink: 0.0, ..default() });
            if health.losing_per_h > 0.0 {
                line(c, format!("Losing {:.1} HP per game hour", health.losing_per_h), flat::RED_TEXT);
            }
            if health.pain > 0.0 {
                line(c, format!("Pain {:.0}%", (health.pain * 100.0).min(999.0)), flat::BONE);
            }
            if let Some(h) = health.aid.painkillers_h {
                line(c, format!("Painkillers working, ~{h:.0} h{}", if health.aid.drowsy { " (two doses: drowsy)" } else { "" }), flat::DIM);
            }
            if let Some(course) = health.aid.course {
                let s = match course.lapses_in_h {
                    Some(l) if course.next_in_h > 0.0 => format!("Antibiotics {} of {}: next in ~{:.0} h (lapses in ~{l:.0} h)", course.doses, course.of, course.next_in_h),
                    Some(l) => format!("Antibiotics {} of {}: next due now (lapses in ~{l:.0} h)", course.doses, course.of),
                    None => "Antibiotics: course done".to_string(),
                };
                line(c, s, flat::DIM);
            }
            match health.infection {
                Some(InfectionStage::Feverish) => line(c, "Feverish: no healing, slow to get your breath back".into(), flat::AMBER),
                Some(InfectionStage::Turning) => line(c, "Turning: losing HP fast, night sight halved".into(), flat::RED_TEXT),
                _ => {}
            }
            if health.food_poisoning {
                line(c, "Food poisoning: can't eat, thirsty".into(), flat::AMBER);
            }
            if busy > 0.0 {
                line(c, format!("Busy: {busy:.1} s"), flat::GREEN);
            }
            if !status.is_empty() {
                head(c, "Status");
            }
            for t in &status {
                c.spawn((Node { flex_direction: FlexDirection::Row, column_gap: px(10), align_items: AlignItems::FlexStart, flex_shrink: 0.0, ..default() }, Pickable::IGNORE)).with_children(|r| {
                    r.spawn((Node { width: px(8), height: px(8), margin: UiRect::top(px(6)), border_radius: BorderRadius::MAX, flex_shrink: 0.0, ..default() }, BackgroundColor(severity_colour(t.severity)), Pickable::IGNORE));
                    r.spawn((Node { flex_direction: FlexDirection::Column, flex_grow: 1.0, flex_basis: px(0), min_width: px(0), ..default() }, Pickable::IGNORE)).with_children(|x| {
                        x.spawn((Text::new(t.label.clone()), theme::strong(flat::BODY), TextColor(severity_colour(t.severity)), Pickable::IGNORE));
                        // (The tips start with the label, for the HUD's one line: not twice here.)
                        let tip = t.tip.strip_prefix(t.label.as_str()).map_or(t.tip.as_str(), |rest| rest.trim_start_matches(':').trim());
                        let mut tip = tip.to_string();
                        if let Some(first) = tip.get(..1).map(str::to_uppercase) {
                            tip.replace_range(..1, &first);
                        }
                        // (What a status does to you is explaining: `help_text.rs`.)
                        if !tip.is_empty() && crate::help_text::explains() {
                            x.spawn((Text::new(tip), font(flat::SMALL), TextColor(flat::DIM), Pickable::IGNORE));
                        }
                    });
                });
            }
            let Some(view) = view.as_ref() else { return };
            let pills = firstaid::options(None, &view.inventory, items);
            if !pills.is_empty() {
                head(c, "Take");
                c.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(6), align_items: AlignItems::FlexStart, flex_shrink: 0.0, ..default() }).with_children(|b| {
                    for (t, item) in pills {
                        let label = option_label(t, item, None, view, items, &join);
                        press(flat::button(b, &label, false), PanelClick::Send(Intent::Treat { player: me.id, wound: None, action: t, item }));
                    }
                });
            }
          });
        });
        // ---- Wounds, head to feet: click one for what it needs and what you can do ----
        c.spawn(Node { flex_direction: FlexDirection::Column, flex_grow: 1.0, flex_basis: px(0), min_width: px(0), ..default() }).with_children(|col| {
            col.spawn((Node { flex_direction: FlexDirection::Row, justify_content: JustifyContent::SpaceBetween, align_items: AlignItems::FlexEnd, padding: UiRect::new(px(24), px(24), px(18), px(10)), flex_shrink: 0.0, ..default() }, Pickable::IGNORE)).with_children(|t| {
                t.spawn((Text::new("WOUNDS"), theme::head(flat::TITLE), TextColor(flat::BONE), Pickable::IGNORE));
                let (n, bleeding) = (health.wounds.len(), health.wounds.iter().filter(|w| w.bleed_per_h > 0.0).count());
                let count = match (n, bleeding) {
                    (0, _) => String::new(),
                    (n, 0) => format!("{n} open"),
                    (n, b) => format!("{n} open, {b} bleeding"),
                };
                t.spawn((Text::new(count), font(flat::SMALL), TextColor(if bleeding > 0 { flat::RED_TEXT } else { flat::DIM }), Pickable::IGNORE));
            });
            crate::inventory_ui::scroller(col, 4, y.0, UiRect::new(px(24), px(24), px(0), px(16)), 8.0).with_children(|c| {
                if let Some(d) = use_def {
                    c.spawn((Node { padding: UiRect::axes(px(12), px(8)), border: UiRect::all(px(1)), border_radius: BorderRadius::all(px(flat::RADIUS)), flex_shrink: 0.0, ..default() }, BorderColor::all(flat::GREEN), BackgroundColor(flat::GREEN_SOFT), Pickable::IGNORE)).with_children(|b| {
                        b.spawn((Text::new(format!("Use the {} on a wound: click the wound. Right-click to stop.", d.name.to_lowercase())), theme::strong(flat::BODY), TextColor(flat::BONE), Pickable::IGNORE));
                    });
                }
                if health.wounds.is_empty() {
                    line(c, "No wounds.".into(), flat::DIM);
                }
                let Some(view) = view.as_ref() else { return };
                for part in Part::ALL {
                    let here: Vec<&WoundView> = health.wounds.iter().filter(|w| w.part == part).collect();
                    if here.is_empty() {
                        continue;
                    }
                    head(c, part.name());
                    for w in here {
                        let fits = use_def.is_some_and(|d| Treatment::of_item(d).is_some_and(|t| firstaid::check(t, Some(w), Some(d)).is_ok()));
                        let picked = panel.selected == Some(w.id);
                        wound_card(c, w, picked || fits);
                        if !picked || use_def.is_some() {
                            continue;
                        }
                        c.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(8), margin: UiRect::new(px(14), px(0), px(0), px(6)), flex_shrink: 0.0, ..default() }).with_children(|o| {
                            o.spawn((Text::new(format!("Needs {}", firstaid::needs(w))), font(flat::SMALL), TextColor(flat::DIM), Pickable::IGNORE));
                            let options = firstaid::options(Some(w), &view.inventory, items);
                            if options.is_empty() {
                                o.spawn((Text::new("Nothing you carry treats it."), font(flat::SMALL), TextColor(flat::DIM), Pickable::IGNORE));
                            }
                            o.spawn(Node { flex_direction: FlexDirection::Row, flex_wrap: FlexWrap::Wrap, column_gap: px(8), row_gap: px(6), ..default() }).with_children(|b| {
                                for (k, (t, item)) in options.into_iter().enumerate() {
                                    let label = option_label(t, item, Some(w), view, items, &join);
                                    press(flat::button(b, &label, k == 0), PanelClick::Send(Intent::Treat { player: me.id, wound: Some(w.id), action: t, item }));
                                }
                            });
                        });
                    }
                }
            });
        });
    });
}

/// A wound's card: its kind, what is wrong with it, and how far it has healed. A press is only
/// recorded ([`Clicks`]); [`send_clicks`] acts on it.
fn wound_card(c: &mut ChildSpawnerCommands, w: &WoundView, on: bool) {
    let rest = if on { flat::GREEN_SOFT } else { flat::CELL };
    let mut name = w.kind.name().to_string();
    name[..1].make_ascii_uppercase();
    let mut e = c.spawn((Node { flex_direction: FlexDirection::Column, row_gap: px(7), padding: UiRect::axes(px(14), px(10)), border: UiRect::all(px(1)), border_radius: BorderRadius::all(px(flat::RADIUS)), flex_shrink: 0.0, ..default() }, BorderColor::all(if on { flat::GREEN } else { flat::LINE }), BackgroundColor(rest)));
    e.with_children(|card| {
        card.spawn((Node { flex_direction: FlexDirection::Row, flex_wrap: FlexWrap::Wrap, align_items: AlignItems::Center, column_gap: px(14), row_gap: px(2), ..default() }, Pickable::IGNORE)).with_children(|r| {
            r.spawn((Text::new(name), theme::strong(flat::HEAD), TextColor(flat::BONE), TextLayout::no_wrap(), Pickable::IGNORE));
            for (text, color) in wound_chips(w) {
                r.spawn((Text::new(text), font(flat::SMALL), TextColor(color), TextLayout::no_wrap(), Pickable::IGNORE));
            }
            r.spawn((Node { flex_grow: 1.0, ..default() }, Pickable::IGNORE));
            let healing = w.heals_in_h.map_or("not healing".to_string(), |h| format!("~{h:.0} h to go"));
            r.spawn((Text::new(format!("{:.0}% healed, {healing}", w.healed * 100.0)), font(flat::SMALL), TextColor(if w.heals_in_h.is_some() { flat::DIM } else { flat::AMBER }), TextLayout::no_wrap(), Pickable::IGNORE));
        });
        flat::bar(card, w.healed, if w.bleed_per_h > 0.0 { flat::RED } else { flat::GREEN }, Node { height: px(4), ..default() });
    });
    flat::hover(&mut e, rest);
    press(e, PanelClick::Wound(w.id));
}

/// Makes `e` one to press: the left button is `click`, the right stops using an item.
fn press(mut e: EntityCommands, click: PanelClick) {
    e.insert(crate::focus::Focusable).observe(move |mut ev: On<PointerPress>, mut clicks: ResMut<Clicks>| {
        ev.propagate(false);
        match ev.button {
            PointerButton::Primary => clicks.0.push(click.clone()),
            PointerButton::Secondary => clicks.0.push(PanelClick::Cancel),
            _ => {}
        }
    });
}
