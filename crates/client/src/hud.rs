//! On-screen stats; health and endurance bars, the weapon held, stance, a winded warning and the
//! health tags (bleeding, injured leg..., from `health_ui.rs`) and this life's kills and time alive (the
//! server's run stats, `sim::run`) (bottom left); plus the automation hooks agents and CI use: periodic
//! stdout stats, `--screenshot` and `--exit-after`. The death screen is `death.rs`.

use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};

use zombye_sim::Stance;

use crate::Options;
use crate::player::LocalPlayer;
use crate::server_link::{ClientMap, JoinInfo, ServerLink};

#[derive(Component)]
struct HudText;

// Vitals panel, bottom left.
#[derive(Component)]
struct WeaponText;
#[derive(Component)]
struct StanceText;
#[derive(Component)]
struct HpFill;
#[derive(Component)]
struct HpNumber;
#[derive(Component)]
struct EnduranceFill;
#[derive(Component)]
struct WindedText;
#[derive(Component)]
struct TallyText;
/// "GOD" / "IGNORED" while the dev toggles are on.
#[derive(Component)]
struct DevText;

#[derive(Default)]
struct FrameStats {
    frames: u32,
    elapsed: f32,
    worst: f32,
}

pub struct HudPlugin;

impl Plugin for HudPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_hud).add_systems(Update, (update_hud, update_vitals, automation, fit_top_panels, scroll_panels).chain());
    }
}

const BAR_W: f32 = 220.0;
const BAR_H: f32 = 12.0;
const TEXT: Color = Color::srgb(0.93, 0.9, 0.82);
const DIM: Color = Color::srgb(0.7, 0.68, 0.62);
const HP_COLOR: Color = Color::srgb(0.78, 0.12, 0.1);
const END_COLOR: Color = Color::srgb(0.85, 0.75, 0.25);
const END_TIRED: Color = Color::srgb(0.9, 0.45, 0.15);
const END_WINDED: Color = Color::srgb(0.45, 0.5, 0.6);
const DEV_COLOR: Color = Color::srgb(0.35, 0.9, 1.0);

/// "GOD IGNORED", "GOD", "IGNORED" or "" for the dev toggles on.
fn dev_tags(dev: zombye_sim::DevFlags) -> String {
    [(dev.god, "GOD"), (dev.ignored, "IGNORED")].iter().filter(|(on, _)| *on).map(|(_, t)| *t).collect::<Vec<_>>().join(" ")
}

fn font(size: f32) -> TextFont {
    TextFont { font_size: bevy::text::FontSize::Px(size), ..default() }
}

/// A labelled bar: returns the row; the fill gets `marker`.
fn bar(c: &mut ChildSpawnerCommands, label: &str, color: Color, marker: impl Component, extra: impl FnOnce(&mut ChildSpawnerCommands)) {
    c.spawn(Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(8), ..default() }).with_children(|row| {
        row.spawn((Text::new(label), font(13.0), TextColor(DIM), Node { width: px(30), ..default() }));
        row.spawn((Node { width: px(BAR_W), height: px(BAR_H), ..default() }, BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.6)))).with_children(|b| {
            b.spawn((marker, Node { width: Val::Percent(100.0), height: Val::Percent(100.0), ..default() }, BackgroundColor(color)));
        });
        extra(row);
    });
}

/// The top-left column: the stats and keys text, then the yellow message line (`saving.rs`) under
/// it, wherever the text ends (it wraps on a narrow window, or with a bigger UI scale).
#[derive(Component)]
pub struct TopLeft;

/// A panel hung from the top of the screen (inventory, health, skills): [`fit_top_panels`] keeps it
/// under the stats text and above every [`BottomHud`] piece, at any UI scale.
#[derive(Component)]
pub struct TopPanel;

/// A piece of the HUD standing on the bottom of the screen (the vitals, the current action): the
/// top panels stop above the highest one.
#[derive(Component)]
pub struct BottomHud;

/// Room left between the top panels and what's above and below them (UI pixels).
const PANEL_GAP: f32 = 8.0;

pub fn spawn_hud(mut commands: Commands) {
    commands
        .spawn((
            TopLeft,
            Node { position_type: PositionType::Absolute, top: px(10), left: px(12), right: px(12), flex_direction: FlexDirection::Column, row_gap: px(4), ..default() },
            Pickable::IGNORE,
        ))
        .with_children(|c| {
            c.spawn((HudText, Text::new(""), font(14.0), TextColor(Color::srgb(0.9, 0.9, 0.85))));
        });
    commands
        .spawn((
            BottomHud,
            Node {
                position_type: PositionType::Absolute,
                bottom: px(16),
                left: px(16),
                flex_direction: FlexDirection::Column,
                row_gap: px(6),
                padding: UiRect::all(px(10)),
                ..default()
            },
            BackgroundColor(Color::srgba(0.02, 0.02, 0.03, 0.55)),
        ))
        .with_children(|c| {
            c.spawn(Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Baseline, column_gap: px(10), ..default() }).with_children(|row| {
                row.spawn((WeaponText, Text::new(""), font(20.0), TextColor(TEXT)));
                row.spawn((StanceText, Text::new(""), font(14.0), TextColor(DIM)));
                row.spawn((DevText, Text::new(""), font(14.0), TextColor(DEV_COLOR)));
            });
            bar(c, "HP", HP_COLOR, HpFill, |row| {
                row.spawn((HpNumber, Text::new(""), font(14.0), TextColor(TEXT)));
            });
            bar(c, "END", END_COLOR, EnduranceFill, |row| {
                row.spawn((WindedText, Text::new("WINDED"), font(13.0), TextColor(Color::srgb(0.95, 0.55, 0.3)), Visibility::Hidden));
            });
            // Bleeding, injured leg, feverish... (`health_ui.rs` fills it in).
            c.spawn((crate::health_ui::HealthTagsText, Text::new(""), font(13.0), TextColor(TEXT)));
            c.spawn((TallyText, Text::new(""), font(13.0), TextColor(DIM)));
        });
}

/// UI pixels the panel moves for one line of the mouse wheel.
const WHEEL_LINE_PX: f32 = 40.0;

/// The shown top panels, for finding the one under the pointer.
pub type PanelHits<'w, 's> = Query<'w, 's, (Entity, &'static ComputedNode, &'static bevy::ui::ComputedStackIndex, &'static UiGlobalTransform, &'static InheritedVisibility), With<TopPanel>>;

/// The shown top panel under the pointer, if any: the mouse wheel scrolls it rather than zooming
/// (`camera.rs` asks the same). Where two overlap (inventory and skills on a narrow screen), the
/// one drawn on top.
pub fn panel_under_cursor(window: Option<&Window>, panels: &PanelHits) -> Option<Entity> {
    let cursor = window?.physical_cursor_position()?;
    topmost_under_cursor(panels.iter().filter(|(_, node, _, at, _)| node.contains_point(**at, cursor)).map(|(e, _, stack, _, vis)| (e, stack.0, vis.get())))
}

/// Of the panels under the pointer (entity, stack index, shown), the shown one drawn on top: the
/// highest stack index, as Bevy draws and picks, not the query's order.
pub fn topmost_under_cursor(hits: impl IntoIterator<Item = (Entity, u32, bool)>) -> Option<Entity> {
    hits.into_iter().filter(|&(_, _, shown)| shown).max_by_key(|&(_, stack, _)| stack).map(|(e, ..)| e)
}

/// A panel's scroll offset after `wheel_px` of wheel (up positive: back toward the top), within
/// `0..=max` (how far its content reaches past it).
pub fn scrolled(pos: f32, wheel_px: f32, max: f32) -> f32 {
    (pos - wheel_px).clamp(0.0, max.max(0.0))
}

/// The mouse wheel over a top panel scrolls it, so whatever its content holds can be reached when
/// it's taller than the room the panel has (a big inventory, the Make list opened, treatments).
fn scroll_panels(mut wheel: MessageReader<bevy::input::mouse::MouseWheel>, window: Query<&Window, With<bevy::window::PrimaryWindow>>, hits: PanelHits, mut scroll: Query<&mut ScrollPosition, With<TopPanel>>) {
    use bevy::input::mouse::MouseScrollUnit;
    let px = wheel
        .read()
        .map(|ev| match ev.unit {
            MouseScrollUnit::Line => ev.y * WHEEL_LINE_PX,
            MouseScrollUnit::Pixel => ev.y,
        })
        .fold(0.0, |a, b| a + b);
    if px == 0.0 {
        return;
    }
    let Some(panel) = panel_under_cursor(window.single().ok(), &hits) else { return };
    let (Ok((_, node, ..)), Ok(mut pos)) = (hits.get(panel), scroll.get_mut(panel)) else { return };
    // Physical pixels to UI pixels, as `ScrollPosition` is.
    let max = (node.content_size.y - node.size.y) * node.inverse_scale_factor();
    let y = scrolled(pos.y, px, max);
    if pos.y != y {
        pos.y = y;
    }
}

/// Where a top panel goes, in UI pixels, from the height of the stats text, how high the
/// bottom HUD reaches (from the bottom of the screen) and the screen's height: its top just under
/// the column (never above the 70 it was laid out with) and its height down to just above the
/// bottom HUD.
pub fn top_panel_span(top_left_h: f32, bottom_reach: f32, view_h: f32) -> (f32, f32) {
    let top = (10.0 + top_left_h + PANEL_GAP).max(70.0);
    let bottom = bottom_reach + PANEL_GAP;
    (top, (view_h - top - bottom).max(80.0))
}

/// Keeps the top panels clear of the stats text above them, and of the vitals
/// and current action below, as text wraps and the UI scale changes (last frame's layout: a frame
/// late at most).
#[allow(clippy::type_complexity)]
fn fit_top_panels(
    options: Res<Options>,
    ui: Res<UiScale>,
    window: Query<&Window, With<bevy::window::PrimaryWindow>>,
    stats: Query<&ComputedNode, With<HudText>>,
    bottom: Query<(&Node, &ComputedNode), (With<BottomHud>, Without<TopPanel>)>,
    mut panels: Query<&mut Node, With<TopPanel>>,
) {
    // The stats text only: the yellow line under it comes and goes, and is drawn over the panels
    // (`saving.rs`) rather than pushing them down and back every few seconds.
    let Ok(column) = stats.single() else { return };
    let Some(view) = crate::ui_scale::view(&options, &window) else { return };
    // Computed sizes are in physical pixels; times the inverse scale factor, in UI pixels.
    let ui_h = |n: &ComputedNode| n.size().y * n.inverse_scale_factor();
    let reach = bottom
        .iter()
        .map(|(node, c)| {
            let above = if let Val::Px(b) = node.bottom { b } else { 0.0 };
            above + ui_h(c)
        })
        .fold(0.0, f32::max);
    let (top, height) = top_panel_span(ui_h(column), reach, view.y / ui.0.max(0.01));
    let (top, height) = (px(top.round()), px(height.round()));
    for mut node in &mut panels {
        if node.top != top || node.max_height != height {
            node.top = top;
            node.max_height = height;
        }
    }
}

#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn update_vitals(
    time: Res<Time>,
    me: Res<LocalPlayer>,
    join: Res<JoinInfo>,
    link: Res<ServerLink>,
    mut texts: ParamSet<(
        Query<&mut Text, With<WeaponText>>,
        Query<&mut Text, With<StanceText>>,
        Query<&mut Text, With<HpNumber>>,
        Query<&mut Text, With<TallyText>>,
        Query<&mut Text, With<DevText>>,
    )>,
    mut fills: ParamSet<(Query<(&mut Node, &mut BackgroundColor), With<HpFill>>, Query<(&mut Node, &mut BackgroundColor), With<EnduranceFill>>)>,
    mut winded: Query<&mut Visibility, With<WindedText>>,
) {
    let set = |t: &mut Text, s: String| {
        if t.0 != s {
            t.0 = s;
        }
    };
    let weapon = me.weapon.and_then(|w| join.weapons.get(w as usize)).map_or("Bare hands", |w| w.name.as_str());
    if let Ok(mut t) = texts.p0().single_mut() {
        set(&mut t, weapon.to_string());
    }
    let s = &me.curr;
    let asleep = link.curr.players.iter().any(|p| p.id == me.id && p.asleep);
    let stance = if me.dead {
        "dead"
    } else if asleep {
        "asleep"
    } else {
        match s.stance {
            Stance::Walk => "walking",
            Stance::Run => "running",
            Stance::Sneak => "sneaking",
        }
    };
    if let Ok(mut t) = texts.p1().single_mut() {
        set(&mut t, stance.to_string());
    }
    let hp = me.hp.max(0.0);
    if let Ok(mut t) = texts.p2().single_mut() {
        set(&mut t, format!("{hp:.0}"));
    }
    // This life so far, as the server counts it.
    if let (Ok(mut t), Some(run)) = (texts.p3().single_mut(), link.curr.run_of(me.id)) {
        set(&mut t, format!("kills {}   alive {}   walked {}", run.kills, run.played(), run.distance()));
    }
    if let Ok(mut t) = texts.p4().single_mut() {
        set(&mut t, dev_tags(me.dev));
    }
    if let Ok((mut node, mut bg)) = fills.p0().single_mut() {
        let w = Val::Percent((hp / join.combat.player_hp.max(1.0) * 100.0).clamp(0.0, 100.0));
        if node.width != w {
            node.width = w;
        }
        // Pulses when low.
        let low = hp < 30.0 && !me.dead;
        let k = if low { 0.75 + 0.25 * (time.elapsed_secs() * 6.0).sin() } else { 1.0 };
        bg.0 = HP_COLOR.with_luminance(HP_COLOR.luminance() * k);
    }
    if let Ok((mut node, mut bg)) = fills.p1().single_mut() {
        let w = Val::Percent((s.endurance * 100.0).clamp(0.0, 100.0));
        if node.width != w {
            node.width = w;
        }
        let c = if s.winded {
            END_WINDED
        } else if s.endurance < join.combat.tired_below {
            END_TIRED
        } else {
            END_COLOR
        };
        if bg.0 != c {
            bg.0 = c;
        }
    }
    let vis = |on: bool| if on { Visibility::Inherited } else { Visibility::Hidden };
    if let Ok(mut v) = winded.single_mut() {
        let want = vis(s.winded && !me.dead);
        if *v != want {
            *v = want;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn update_hud(
    time: Res<Time>,
    link: Res<ServerLink>,
    map: Res<ClientMap>,
    me: Res<LocalPlayer>,
    join: Res<JoinInfo>,
    options: Res<Options>,
    mut sounds: Option<ResMut<crate::audio::SoundCount>>,
    mut stats: Local<FrameStats>,
    mut text: Query<&mut Text, With<HudText>>,
) {
    let dt = time.delta_secs();
    stats.frames += 1;
    stats.elapsed += dt;
    stats.worst = stats.worst.max(dt);
    if stats.elapsed < 1.0 {
        return;
    }
    let fps = stats.frames as f32 / stats.elapsed;
    let hour = link.curr.time_of_day;
    let room = map.0.room_at(me.curr.pos);
    let place = if room == 0 { "Outdoors".to_string() } else { map.0.rooms[room as usize].name.clone() };
    let s = &me.curr;
    let winded = if s.winded { " (winded)" } else { "" };
    let dev = match dev_tags(me.dev) {
        t if t.is_empty() => t,
        t => format!(" | {t}"),
    };
    // The status tags (#11), and time running fast while asleep.
    let tags = match link.own(me.id) {
        Some(v) if !me.dead => v.status_tags(&join.needs, s.endurance, s.winded, join.combat.tired_below, link.curr.time_scale),
        _ => Vec::new(),
    };
    let tags = if tags.is_empty() { String::new() } else { format!(" | {}", zombye_sim::needs::tags_line(&tags)) };
    let fast = if link.curr.time_scale > 1.0 { format!(" x{:.0}", link.curr.time_scale) } else { String::new() };
    // Sounds started since the last line (audio.rs), so a run shows what was heard.
    let heard = sounds.as_deref_mut().map_or(String::new(), |c| format!(" | {}", c.take_line()));
    let line = format!(
        "{:02}:{:02}{fast} | {place} | {:?} | hp {:.0}{}{dev}{tags} | endurance {:.0}%{winded} | fps {:.0} | frame {:.2} ms (worst {:.2}) | zombies {} (chasing {}; tiers {}/{}/{}) | sim tick {:.3} ms{heard}",
        hour.floor() as u32,
        ((hour - hour.floor()) * 60.0) as u32,
        s.stance,
        me.hp,
        if me.dead { " (dead)" } else { "" },
        s.endurance * 100.0,
        fps,
        1000.0 * stats.elapsed / stats.frames as f32,
        stats.worst * 1000.0,
        link.curr.zombies.len(),
        link.chasing,
        link.tiers[0],
        link.tiers[1],
        link.tiers[2],
        link.sim_ms,
    );
    println!("{line}");
    if let Ok(mut t) = text.single_mut() {
        let rotate = if options.rotate { ", . rotate | " } else { "" };
        t.0 = format!(
            "{line}\nWASD move | Shift run | C sneak{} | RMB (hold) aim | aiming: LMB attack, Space shove/stomp | E pick up / search | Tab inventory | H health | K skills | B sleep | F door/window | G smash window | T +1 hour | wheel zoom | {rotate}Z spawn 250 zombies | F5 save | F9 load | F1 god{} | F2 ignored{} | F3 light preview | M mute",
            if me.sneak { " (on)" } else { "" },
            if me.dev.god { " (on)" } else { "" },
            if me.dev.ignored { " (on)" } else { "" }
        );
    }
    *stats = FrameStats::default();
}

fn automation(
    mut commands: Commands,
    time: Res<Time>,
    options: Res<Options>,
    mut shots_taken: Local<usize>,
    mut exit: MessageWriter<AppExit>,
) {
    let t = time.elapsed_secs();
    if let Some(path) = &options.screenshot {
        let times = &options.screenshot_after;
        if *shots_taken < times.len() && t >= times[*shots_taken] {
            *shots_taken += 1;
            let file = if times.len() == 1 { path.clone() } else { numbered(path, *shots_taken) };
            commands.spawn(Screenshot::primary_window()).observe(save_to_disk(file));
        }
    }
    if options.exit_after.is_some_and(|limit| t >= limit) {
        exit.write(AppExit::Success);
    }
}

/// `shots/fight.png`, 2 → `shots/fight-2.png`.
fn numbered(path: &str, n: usize) -> String {
    let p = std::path::Path::new(path);
    match (p.file_stem(), p.extension()) {
        (Some(stem), Some(ext)) => p.with_file_name(format!("{}-{n}.{}", stem.to_string_lossy(), ext.to_string_lossy())).to_string_lossy().into_owned(),
        _ => format!("{path}-{n}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn top_panels_sit_between_the_top_left_text_and_the_vitals() {
        // A short column keeps the panels where they were laid out, down to above the vitals.
        assert_eq!(top_panel_span(20.0, 136.0, 720.0), (70.0, 720.0 - 70.0 - (136.0 + PANEL_GAP)));
        // Wrapped text and the yellow line push them down, and shorter.
        let (top, h) = top_panel_span(100.0, 136.0, 720.0);
        assert_eq!(top, 10.0 + 100.0 + PANEL_GAP);
        assert_eq!(top + h, 720.0 - 136.0 - PANEL_GAP);
        // Never squeezed to nothing.
        assert_eq!(top_panel_span(400.0, 300.0, 480.0).1, 80.0);
    }

    #[test]
    fn the_wheel_scrolls_a_panel_within_its_content() {
        // Wheel down (negative) moves further into the content, up moves back; never past either end.
        assert_eq!(scrolled(0.0, -40.0, 100.0), 40.0);
        assert_eq!(scrolled(80.0, -40.0, 100.0), 100.0);
        assert_eq!(scrolled(40.0, 40.0, 100.0), 0.0);
        assert_eq!(scrolled(10.0, 40.0, 100.0), 0.0);
        // Content that fits: nothing to scroll.
        assert_eq!(scrolled(0.0, -40.0, -20.0), 0.0);
    }

    #[test]
    fn the_wheel_picks_the_panel_drawn_on_top() {
        let (inventory, skills, health) = (Entity::from_raw_u32(1).unwrap(), Entity::from_raw_u32(2).unwrap(), Entity::from_raw_u32(3).unwrap());
        // Inventory and skills overlap under the pointer: whichever is drawn on top (the higher stack
        // index) wins, whatever order the query gives them in.
        assert_eq!(topmost_under_cursor([(inventory, 7, true), (skills, 12, true)]), Some(skills));
        assert_eq!(topmost_under_cursor([(skills, 12, true), (inventory, 7, true)]), Some(skills));
        assert_eq!(topmost_under_cursor([(inventory, 20, true), (skills, 12, true)]), Some(inventory));
        // Hits that are hidden don't count; no hit, no panel.
        assert_eq!(topmost_under_cursor([(inventory, 7, true), (health, 30, false)]), Some(inventory));
        assert_eq!(topmost_under_cursor([]), None);
    }
}
