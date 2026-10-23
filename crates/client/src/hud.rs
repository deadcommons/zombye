//! On-screen stats; health and endurance bars, the weapon held, stance and a winded warning
//! (bottom left); the death screen (time survived and kills this life, counted from combat
//! events); plus the automation hooks agents and CI use: periodic stdout stats, `--screenshot`
//! and `--exit-after`.

use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};

use zombye_sim::{CombatEventKind, Stance, TICK_DT};

use crate::Options;
use crate::player::LocalPlayer;
use crate::server_link::{ClientMap, CombatEventsThisFrame, JoinInfo, ServerLink};

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

/// The death screen, and its "time survived · kills" line.
#[derive(Component)]
struct DeathScreen;
#[derive(Component)]
struct DeathStats;

/// This life's kills and when it began (sim ticks), counted from combat events. Reset on getting up.
#[derive(Resource, Default)]
pub struct Tally {
    pub kills: u32,
    pub life_start_tick: u64,
    pub died_tick: Option<u64>,
}

#[derive(Default)]
struct FrameStats {
    frames: u32,
    elapsed: f32,
    worst: f32,
}

pub struct HudPlugin;

impl Plugin for HudPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Tally>().add_systems(Startup, spawn_hud).add_systems(Update, (count_kills, update_hud, update_vitals, automation).chain());
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

fn spawn_hud(mut commands: Commands) {
    commands.spawn((
        HudText,
        Text::new(""),
        font(14.0),
        TextColor(Color::srgb(0.9, 0.9, 0.85)),
        Node { position_type: PositionType::Absolute, top: px(10), left: px(12), ..default() },
    ));
    commands
        .spawn((
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
            });
            bar(c, "HP", HP_COLOR, HpFill, |row| {
                row.spawn((HpNumber, Text::new(""), font(14.0), TextColor(TEXT)));
            });
            bar(c, "END", END_COLOR, EnduranceFill, |row| {
                row.spawn((WindedText, Text::new("WINDED"), font(13.0), TextColor(Color::srgb(0.95, 0.55, 0.3)), Visibility::Hidden));
            });
            c.spawn((TallyText, Text::new(""), font(13.0), TextColor(DIM)));
        });
    commands
        .spawn((
            DeathScreen,
            Visibility::Hidden,
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                row_gap: px(14),
                ..default()
            },
            BackgroundColor(Color::srgba(0.1, 0.0, 0.0, 0.55)),
        ))
        .with_children(|c| {
            c.spawn((Text::new("You died"), font(72.0), TextColor(Color::srgb(0.85, 0.1, 0.07))));
            c.spawn((DeathStats, Text::new(""), font(24.0), TextColor(TEXT)));
            c.spawn((Text::new("Press R to get up again"), font(18.0), TextColor(DIM)));
        });
}

/// Count our kills from this frame's combat events; reset the tally when we get up again.
fn count_kills(link: Res<ServerLink>, me: Res<LocalPlayer>, events: Res<CombatEventsThisFrame>, mut tally: ResMut<Tally>) {
    tally.kills += events.0.iter().filter(|e| e.kind == CombatEventKind::ZombieKilled && e.player == me.id).count() as u32;
    match (me.dead, tally.died_tick) {
        (true, None) => tally.died_tick = Some(link.curr.tick),
        (false, Some(_)) => *tally = Tally { kills: 0, life_start_tick: link.curr.tick, died_tick: None },
        _ => {}
    }
}

/// m:ss of sim time between two ticks.
fn duration(from: u64, to: u64) -> String {
    let secs = (to.saturating_sub(from) as f32 * TICK_DT) as u32;
    format!("{}:{:02}", secs / 60, secs % 60)
}

#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn update_vitals(
    time: Res<Time>,
    me: Res<LocalPlayer>,
    join: Res<JoinInfo>,
    link: Res<ServerLink>,
    tally: Res<Tally>,
    mut texts: ParamSet<(
        Query<&mut Text, With<WeaponText>>,
        Query<&mut Text, With<StanceText>>,
        Query<&mut Text, With<HpNumber>>,
        Query<&mut Text, With<TallyText>>,
        Query<&mut Text, With<DeathStats>>,
    )>,
    mut fills: ParamSet<(Query<(&mut Node, &mut BackgroundColor), With<HpFill>>, Query<(&mut Node, &mut BackgroundColor), With<EnduranceFill>>)>,
    mut shown: ParamSet<(Query<&mut Visibility, With<DeathScreen>>, Query<&mut Visibility, With<WindedText>>)>,
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
    let stance = if me.dead {
        "dead"
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
    let now_tick = tally.died_tick.unwrap_or(link.curr.tick);
    let alive_for = duration(tally.life_start_tick, now_tick);
    if let Ok(mut t) = texts.p3().single_mut() {
        set(&mut t, format!("kills {}   alive {alive_for}", tally.kills));
    }
    if let Ok(mut t) = texts.p4().single_mut() {
        let kills = if tally.kills == 1 { "1 kill".to_string() } else { format!("{} kills", tally.kills) };
        set(&mut t, format!("Survived {alive_for}  -  {kills}"));
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
    if let Ok(mut v) = shown.p0().single_mut() {
        let want = vis(me.dead);
        if *v != want {
            *v = want;
        }
    }
    if let Ok(mut v) = shown.p1().single_mut() {
        let want = vis(s.winded && !me.dead);
        if *v != want {
            *v = want;
        }
    }
}

fn update_hud(
    time: Res<Time>,
    link: Res<ServerLink>,
    map: Res<ClientMap>,
    me: Res<LocalPlayer>,
    options: Res<Options>,
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
    let line = format!(
        "{:02}:{:02} | {place} | {:?} | hp {:.0}{} | endurance {:.0}%{winded} | fps {:.0} | frame {:.2} ms (worst {:.2}) | zombies {} (chasing {}; tiers {}/{}/{}) | sim tick {:.3} ms",
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
            "{line}\nWASD move | Shift run | C sneak{} | mouse face | LMB attack | Space shove/stomp | E pick up / search | Tab inventory | F door/window | G smash window | T +1 hour | wheel zoom | {rotate}Z spawn 250 zombies | F5 save | F9 load",
            if me.sneak { " (on)" } else { "" }
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
