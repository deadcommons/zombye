//! On-screen stats; the weapon held, stance, health as one brushed bar (Linear DCM-61), the health
//! tags (injured leg..., from `health_ui.rs`) and this life's kills and time alive (the server's run
//! stats, `sim::run`) (bottom left; no other stat bars: the status icons of `needs_ui.rs` stand
//! over it); plus the automation hooks agents and CI use: periodic
//! stdout stats, `--screenshot` and `--exit-after`. The death screen is `death.rs`.

use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};

use zombye_sim::Stance;

use crate::Options;
use crate::player::LocalPlayer;
use crate::server_link::{ClientMap, JoinInfo, ServerLink};
use crate::theme::{self, font};

#[derive(Component)]
pub struct HudText;

// Vitals panel, bottom left.
#[derive(Component)]
struct WeaponText;
#[derive(Component)]
struct StanceText;
#[derive(Component)]
struct TallyText;
/// The weapon panel, bottom left: the status icons (`needs_ui.rs`) stand right of it.
#[derive(Component)]
pub struct VitalsPanel;
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

const TEXT: Color = theme::TEXT;
const DIM: Color = theme::DIM;
const DEV_COLOR: Color = Color::srgb(0.35, 0.9, 1.0);
/// The health bar's length and height (UI pixels).
const HP_BAR: (f32, f32) = (200.0, 10.0);

/// The filled part of the health bar.
#[derive(Component)]
struct HpFill;

/// The health bar's colour with `share` (0..1) of the HP left: pale, yellow under half, red under
/// a quarter.
pub fn hp_colour(share: f32) -> Color {
    if share < 0.25 {
        theme::BAD
    } else if share < 0.5 {
        theme::GOLD
    } else {
        theme::TEXT
    }
}

/// "GOD IGNORED", "GOD", "IGNORED" or "" for the dev toggles on.
/// And "PROTECTED" while our spawn protection lasts (#68: other players can't hurt us yet).
fn dev_tags(dev: zombye_sim::DevFlags, protected: bool) -> String {
    [(dev.god, "GOD"), (dev.ignored, "IGNORED"), (protected, "PROTECTED")].iter().filter(|(on, _)| *on).map(|(_, t)| *t).collect::<Vec<_>>().join(" ")
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

/// The column standing in the bottom-left corner: the status tags, then the vitals under them.
/// The hotbar has the rest of the bottom edge (#97).
#[derive(Component)]
pub struct BottomLeft;

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
            // On a dark backing of its own, so it reads over sunlit ground too (#97).
            // (The stats line and the keys: a developer's read-out, its numbers changing every
            // second, so it stays monospace and nothing in it shifts.)
            c.spawn((HudText, Text::new(""), theme::mono(14.0), TextColor(TEXT), BackgroundColor(theme::HUD_BG.with_alpha(0.7))));
        });
    // The bottom-left column: the status tags (`needs_ui.rs` puts them first) over the vitals.
    let column = commands
        .spawn((
            BottomLeft,
            BottomHud,
            Node { position_type: PositionType::Absolute, bottom: px(16), left: px(16), flex_direction: FlexDirection::Column, align_items: AlignItems::FlexStart, row_gap: px(6), ..default() },
            Pickable::IGNORE,
        ))
        .id();
    commands
        .spawn((VitalsPanel, Node { flex_direction: FlexDirection::Column, row_gap: px(6), padding: UiRect::axes(px(theme::STRIP_PAD.0 + 2.0), px(theme::STRIP_PAD.1 + 2.0)), ..default() }, theme::strip(), ChildOf(column)))
        .with_children(|c| {
            c.spawn(Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Baseline, column_gap: px(10), ..default() }).with_children(|row| {
                row.spawn((WeaponText, Text::new(""), theme::head(theme::TITLE), TextColor(TEXT)));
                row.spawn((StanceText, Text::new(""), font(theme::BODY), TextColor(DIM)));
                row.spawn((DevText, Text::new(""), theme::strong(theme::BODY), TextColor(DEV_COLOR)));
            });
            // Health as one rough brushed bar (Linear DCM-61, from the owner's reference; DCM-57
            // took the stat bars out: this is the one that came back). What else is wrong is still
            // the status icons over this panel (`needs_ui.rs`), the numbers the debug window (F4).
            c.spawn((Node { width: px(HP_BAR.0), height: px(HP_BAR.1), ..default() }, theme::bar_track())).with_children(|b| {
                b.spawn((HpFill, Node { width: percent(100), height: percent(100), ..default() }, theme::bar_fill(hp_colour(1.0))));
            });
            // Injured leg, drowsy from painkillers, where the last strike landed (`health_ui.rs`).
            c.spawn((crate::health_ui::HealthTagsText, Text::new(""), font(theme::SMALL), TextColor(TEXT)));
            c.spawn((TallyText, Text::new(""), font(theme::SMALL), TextColor(DIM)));
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
pub fn fit_top_panels(
    options: Res<Options>,
    ui: Res<UiScale>,
    window: Query<&Window, With<bevy::window::PrimaryWindow>>,
    stats: Query<&ComputedNode, With<HudText>>,
    bottom: Query<(&Node, &ComputedNode), (With<BottomHud>, Without<TopPanel>)>,
    places: Option<Res<crate::panels::PanelPlaces>>,
    mut panels: Query<(&mut Node, &ComputedNode, Option<&crate::panels::PanelId>), With<TopPanel>>,
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
    let view = view / ui.0.max(0.01);
    let (top, height) = top_panel_span(ui_h(column), reach, view.y);
    for (mut node, computed, id) in &mut panels {
        // A panel dragged somewhere (`panels.rs`) goes there, kept on the screen and above the
        // bottom HUD; the others hang where they're laid out.
        let at = id.and_then(|id| places.as_ref().and_then(|p| p.of(*id))).map(|at| crate::panels::placed(at, computed.size() * computed.inverse_scale_factor(), view, reach + PANEL_GAP));
        let (left, right) = match (at, id) {
            (Some(at), _) => (px(at.x), Val::Auto),
            (None, Some(id)) => id.home(),
            (None, None) => (node.left, node.right),
        };
        let (top, height) = match at {
            Some(at) => (at.y, (view.y - at.y - reach - PANEL_GAP).max(80.0)),
            None => (top, height),
        };
        let (top, height) = (px(top.round()), px(height.round()));
        if node.top != top || node.max_height != height || node.left != left || node.right != right {
            node.top = top;
            node.max_height = height;
            node.left = left;
            node.right = right;
        }
    }
}

/// What follows a gun's name in the weapon panel (#72): the rounds in it of those it takes, and
/// what is carried to reload it with: "  9/12  +12 +24" is nine in the gun, a full magazine and 24
/// loose rounds in the pockets; "  0/5  (empty)" with nothing to reload it with. Empty when
/// what's held is no gun. (R reloads, as the keys line says.) After a weapon that wears:
/// "  (worn)", "  (battered)" or "  (broken)", nothing while it is good.
pub(crate) fn gun_line(join: &JoinInfo, inventory: Option<&zombye_sim::Inventory>) -> String {
    let Some(inv) = inventory else { return String::new() };
    let Some(gun) = inv.hand else { return String::new() };
    let holds = |item: u16| zombye_sim::ranged::holds(&join.items, &join.weapons, item);
    // A weapon that has seen use says how it is holding up (`zombye_sim::wear`).
    if let Some((_, _, state)) = zombye_sim::wear::condition(&join.items, &join.weapons, &gun, join.combat.battered_below) {
        return if state == zombye_sim::wear::Condition::Good { String::new() } else { format!("  ({})", state.label()) };
    }
    let Some(h) = holds(gun.item).filter(|h| !h.magazine) else { return String::new() };
    let mut line = format!("  {}/{}", gun.used, h.capacity);
    for s in inv.carried.iter().filter(|s| holds(s.item).is_some_and(|m| m.magazine && m.ammo == h.ammo)) {
        line += &format!("  +{}", s.used);
    }
    let loose: u32 = inv.carried.iter().filter(|s| s.item == h.ammo).map(|s| s.count as u32).sum();
    if loose > 0 {
        line += &format!("  +{loose}");
    }
    if gun.used == 0 && !line.contains('+') {
        line += "  (empty)";
    }
    line
}

#[allow(clippy::type_complexity)]
fn update_vitals(
    me: Res<LocalPlayer>,
    join: Res<JoinInfo>,
    link: Res<ServerLink>,
    mut texts: ParamSet<(Query<&mut Text, With<WeaponText>>, Query<&mut Text, With<StanceText>>, Query<&mut Text, With<TallyText>>, Query<&mut Text, With<DevText>>)>,
    mut hp: Query<(&mut Node, &mut ImageNode), With<HpFill>>,
) {
    if let Ok((mut node, mut image)) = hp.single_mut() {
        let share = (me.hp / join.combat.player_hp.max(1.0)).clamp(0.0, 1.0);
        let (width, colour) = (percent((share * 100.0).round()), hp_colour(share));
        if node.width != width {
            node.width = width;
        }
        if image.color != colour {
            image.color = colour;
        }
    }
    // Takes the `Mut` itself: handed over as `&mut Text` it would count as changed every frame,
    // and a changed text is measured and the HUD laid out again.
    let set = |mut t: Mut<Text>, s: String| {
        if t.0 != s {
            t.0 = s;
        }
    };
    let weapon = me.weapon.and_then(|w| join.weapons.get(w as usize)).map_or("Bare hands", |w| w.name.as_str());
    if let Ok(t) = texts.p0().single_mut() {
        set(t,format!("{weapon}{}", gun_line(&join, link.own(me.id).map(|v| &v.inventory))));
    }
    let s = &me.curr;
    let asleep = link.curr.players.iter().any(|p| p.id == me.id && p.asleep);
    let stance = if me.dead {
        "dead"
    } else if asleep {
        "asleep"
    } else if let Some(k) = link.curr.players.iter().find(|p| p.id == me.id).and_then(|p| p.knock) {
        // Another player's blow (PvP): no moving or acting until it passes.
        if k.down { "knocked down" } else { "staggered" }
    } else {
        match s.stance {
            Stance::Walk => "walking",
            Stance::Run => "running",
            Stance::Sneak => "sneaking",
        }
    };
    // At a wheel: the car's speed, fuel and what is worn (`zombye_sim::vehicles`).
    let stance = match me.car.and_then(|c| link.curr.vehicles.iter().find(|v| v.id == c.id).map(|v| (c, v))) {
        Some((c, v)) => driving_line(&join.vehicles, v, c.curr.speed),
        None => stance.to_string(),
    };
    if let Ok(t) = texts.p1().single_mut() {
        set(t,stance);
    }
    // This life so far, as the server counts it.
    if let (Ok(t), Some(run)) = (texts.p2().single_mut(), link.curr.run_of(me.id)) {
        let survivors = if run.player_kills > 0 { format!(" (+{} survivors)", run.player_kills) } else { String::new() };
        set(t,format!("kills {}{survivors}   alive {}   walked {}", run.kills, run.played(), run.distance()));
    }
    if let Ok(t) = texts.p3().single_mut() {
        set(t,dev_tags(me.dev, link.curr.players.iter().any(|p| p.id == me.id && p.protected)));
    }
}

/// "driving the van  31 km/h  fuel 12/70 L  engine off  tyres 80%": the parts only once worn.
pub(crate) fn driving_line(data: &zombye_sim::VehicleData, v: &zombye_sim::VehicleView, speed: f32) -> String {
    let kind = data.kinds.get(v.kind as usize);
    let name = kind.map_or("car".to_string(), |k| k.label());
    let tank = kind.map_or(0.0, |k| k.tank_l);
    let mut line = format!("driving the {name}  {:.0} km/h  fuel {:.0}/{tank:.0} L", speed.abs() * 3.6, v.fuel().ceil());
    if v.parts[0] == 0 {
        line.push_str("  engine wrecked");
    } else if !v.engine_on {
        line.push_str("  engine off");
    }
    for (part, pct) in zombye_sim::vehicles::PARTS.iter().zip(v.parts) {
        if pct < 100 && !(*part == "engine" && pct == 0) {
            line.push_str(&format!("  {part} {pct}%"));
        }
    }
    line
}

#[allow(clippy::too_many_arguments)]
fn update_hud(
    time: Res<Time>,
    link: Res<ServerLink>,
    map: Res<ClientMap>,
    me: Res<LocalPlayer>,
    join: Res<JoinInfo>,
    icons: Res<crate::needs_ui::StatusIcons>,
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
    let dev = match dev_tags(me.dev, link.curr.players.iter().any(|p| p.id == me.id && p.protected)) {
        t if t.is_empty() => t,
        t => format!(" | {t}"),
    };
    // The status icons' labels (#11, DCM-57), and time running fast while asleep.
    let tags = icons.tags(&link, &join, &me);
    let tags = zombye_sim::needs::tags_line(&tags);
    let tags = if tags.is_empty() { String::new() } else { format!(" | {tags}") };
    let fast = if link.curr.time_scale > 1.0 { format!(" x{:.0}", link.curr.time_scale) } else { String::new() };
    // Sounds started since the last line (audio.rs), so a run shows what was heard.
    let heard = sounds.as_deref_mut().map_or(String::new(), |c| format!(" | {}", c.take_line()));
    // In single-player, what the world's tick cost; on a multiplayer server (#21), who and where
    // we are, the others in view and the ping (or that the connection is gone).
    let sim = match &link.net {
        None => format!("sim tick {:.3} ms", link.sim_ms),
        Some(net) => match &net.closed {
            Some(why) => format!("DISCONNECTED from {} ({why})", net.addr),
            None => format!(
                "online: {} on {} (pvp {}) | {} other survivors in view{} | ping {} | view {:.0} ms behind | down {}",
                net.name,
                net.server,
                if net.pvp { "on" } else { "off" },
                link.curr.players.len().saturating_sub(1),
                // On a server that marks agents (#69): how many of them a program plays.
                match link.curr.players.iter().filter(|p| p.id != me.id && p.agent).count() {
                    0 => String::new(),
                    n => format!(" ({n} agent{})", if n == 1 { "" } else { "s" }),
                },
                net.ping_ms.map_or("?".to_string(), |p| format!("{p:.0} ms")),
                // How old the picture of everything else is by our survivor's clock: what the
                // server rewinds our blows' targets by (#23).
                link.curr.players.iter().find(|p| p.id == me.id).map_or(0.0, |v| me.view_lag(&link, v)) * 1000.0,
                net.down_kbps.map_or("?".to_string(), |k| format!("{k:.1} KB/s"))
            ),
        },
    };
    let line = format!(
        "{:02}:{:02}{fast} | {place} | {:?} | hp {:.0}{}{dev}{tags} | endurance {:.0}%{winded} | fps {:.0} | frame {:.2} ms (worst {:.2}) | zombies {} (chasing {}; tiers {}/{}/{}) | {sim}{heard}",
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
    );
    println!("{line}");
    if let Ok(mut t) = text.single_mut() {
        // A packaged game (playtesters) without `--dev`: no developer's read-out, only where the
        // keys are and how to report a problem.
        if !zombye_net::install::dev_tools() {
            t.0 = format!("{:02}:{:02} | Esc menu (the keys are under Settings, Controls) | F12 report a problem", hour.floor() as u32, ((hour - hour.floor()) * 60.0) as u32);
            *stats = FrameStats::default();
            return;
        }
        let rotate = ", . turn camera | ; first person | ";
        t.0 = format!(
            "{line}\nWASD move | Shift run | C sneak{} | RMB (hold) aim | aiming: LMB attack or shoot, Space shove/stomp | R reload | E use what you face (hold: everything) | 1-9 hotbar | Tab inventory | H health | K skills | B sleep | Enter talk | V wave | U hands up | P point | Y tie/free | L search them | J (hold) struggle | N group | F flashlight | G smash window | T +1 hour | wheel zoom | {rotate}Z spawn 250 zombies | F5 save | F9 load | F1 god{} | F2 ignored{} | F3 light preview | F4 debug stats{} | M map | Ctrl+M mute | O objectives",
            if me.sneak { " (on)" } else { "" },
            if me.dev.god { " (on)" } else { "" },
            if me.dev.ignored { " (on)" } else { "" },
            // The server's rules panel (#69): only on a server.
            if link.net.is_some() { " | F6 server rules | F7 servers | Esc menu | F10 walls up/cut | Ctrl+F10 opening hints" } else { " | F7 servers | Esc menu | F10 walls up/cut | Ctrl+F10 opening hints" }
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
    fn the_health_bar_turns_yellow_then_red() {
        assert_eq!(hp_colour(1.0), theme::TEXT);
        assert_eq!(hp_colour(0.5), theme::TEXT);
        assert_eq!(hp_colour(0.49), theme::GOLD);
        assert_eq!(hp_colour(0.2), theme::BAD);
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
