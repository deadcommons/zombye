//! Hold-ups and groups (#73, docs/PLAN.md §4.8): tying another survivor's hands, freeing them,
//! searching one who is tied, getting out of ties, and an optional group with names and shared
//! markers. Everything here only sends the intents (`zombye_sim::bonds`, `zombye_sim::groups`
//! decide) and shows what the snapshots say.
//!
//! **Keys:** Y ties the hands of the survivor you face (they must have them up, or be on the
//! ground; you need cable ties or rope), or frees them if they are tied (a cable tie needs
//! something that cuts). L searches a tied survivor: their things come up in a panel, a click
//! takes a stack (Alt-click one of it), L again or walking off shuts it. X stops what your hands
//! are at. Tied yourself, stand still and hold J to struggle out. N invites the nearest survivor
//! in sight into your group, or accepts an invitation to you; Shift+N leaves. The middle mouse
//! button puts a marker your group shares where you point, or takes away the one pointed at.
//!
//! **Shown:** a bar over the current-action bar while your hands are at work (what, how long) and
//! while they are tied (how far out you are); the search panel on the right; your group's roster
//! top right; a name under each group member in view (nobody else has one, PLAN §4.8); each
//! shared marker as a label where it is, with its distance, kept on screen. `survivor.rs` poses
//! tied hands behind the back and working hands out in front.
//!
//! For recordings and pictures: `--holdup-demo` turns PvP on and puts another survivor right in
//! front of you with a few things on it, who puts its hands up; you get cable ties and scissors.
//! `--holdup-auto` plays your side too ([`SCRIPT`]): tie, search, take, free, group, mark.

use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use zombye_net::{FriskView, GroupOp, Intent};
use zombye_sim::{Gesture, GroupView, ItemStack, Place, Slot, Volume};

use crate::camera::IsoCamera;
use crate::player::{LocalPlayer, PlayerInputSet};
use crate::server_link::{JoinInfo, ServerLink};
use crate::survivor::SurvivorAnim;
use crate::theme::{self, BODY, DIM, ROW, SMALL, TEXT, font};

/// `--holdup-demo`: how far east of us the other survivor stands.
pub const DEMO_DISTANCE: f32 = 1.2;
/// How near another survivor has to be for Y to be about them (the server has the last word).
const NEAR: f32 = 1.9;
/// A marker within this of the point clicked is taken away instead of another put down (metres).
const MARKER_PICK: f32 = 1.5;

// The skin is `theme.rs`'s (Linear DCM-61): the bar a strip with a brushed bar, the search panel a
// sheet with flat rows, the lines of text that carry their own backing (roster, names) `HUD_BG`.
const BG: Color = theme::HUD_BG;
const FILL: Color = Color::srgb(0.8, 0.45, 0.25);
const GROUP: Color = Color::srgb(0.45, 0.85, 0.8);
const ROW_OVER: Color = theme::ROW_HOVER;
const BAR_WIDTH: f32 = 380.0;
/// (As wide inside the sheet's ragged edge as before it had one.)
const PANEL_WIDTH: f32 = 284.0 + 2.0 * theme::SHEET_PAD.0;

#[derive(Component)]
struct Bar;
#[derive(Component)]
struct BarTitle;
#[derive(Component)]
struct BarFill;
#[derive(Component)]
struct BarHint;
#[derive(Component)]
struct FriskPanel;
#[derive(Component)]
struct Roster;

/// A label in the world: a group member's name, or a shared marker.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
enum Tag {
    Member(u32),
    Marker(u32),
}

/// What the search panel was last built from, and whether J was held last frame.
#[derive(Resource, Default)]
struct Shown {
    frisk: Option<FriskView>,
    struggling: bool,
    roster: String,
}

pub struct HoldupPlugin;

impl Plugin for HoldupPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Shown>()
            .add_systems(Startup, spawn)
            .add_systems(Update, (keys.run_if(crate::live), show_bar, show_frisk, show_roster, place_tags).chain().run_if(resource_exists::<ServerLink>).run_if(resource_exists::<LocalPlayer>))
            .add_systems(FixedUpdate, drive_demo.before(PlayerInputSet).run_if(resource_exists::<HoldupDemo>).run_if(crate::live));
    }
}

fn spawn(mut commands: Commands) {
    // Over the current-action bar (`action_ui.rs`, at 70 px) and the first night's hint line.
    commands
        .spawn((
            Bar,
            crate::hud::BottomHud,
            Node {
                position_type: PositionType::Absolute,
                bottom: px(236),
                left: percent(50),
                margin: UiRect::left(px(-BAR_WIDTH / 2.0)),
                width: px(BAR_WIDTH),
                flex_direction: FlexDirection::Column,
                row_gap: px(3),
                padding: UiRect::axes(px(theme::STRIP_PAD.0 + 2.0), px(theme::STRIP_PAD.1)),
                ..default()
            },
            theme::strip(),
            Visibility::Hidden,
            Pickable::IGNORE,
        ))
        .with_children(|c| {
            c.spawn((BarTitle, Text::new(""), font(BODY), TextColor(TEXT), Pickable::IGNORE));
            c.spawn((Node { width: percent(100), height: px(10), ..default() }, theme::bar_track(), Pickable::IGNORE)).with_children(|b| {
                b.spawn((BarFill, Node { width: percent(0), height: percent(100), ..default() }, theme::bar_fill(FILL), Pickable::IGNORE));
            });
            c.spawn((BarHint, Text::new(""), font(SMALL), TextColor(DIM), Pickable::IGNORE));
        });
    commands.spawn((
        FriskPanel,
        Node {
            position_type: PositionType::Absolute,
            right: px(18),
            top: px(180),
            width: px(PANEL_WIDTH),
            flex_direction: FlexDirection::Column,
            row_gap: px(2),
            padding: UiRect::axes(px(theme::SHEET_PAD.0), px(theme::SHEET_PAD.1)),
            ..default()
        },
        theme::sheet(),
        GlobalZIndex(3),
        Visibility::Hidden,
    ));
    commands.spawn((
        Roster,
        Text::new(""),
        font(SMALL),
        TextColor(GROUP),
        Node { position_type: PositionType::Absolute, right: px(18), top: px(118), max_width: px(PANEL_WIDTH), padding: UiRect::axes(px(6), px(3)), ..default() },
        BackgroundColor(Color::NONE),
        Pickable::IGNORE,
    ));
}

/// The point on the ground under the mouse cursor.
fn cursor_ground(windows: &Query<&Window, With<PrimaryWindow>>, cams: &Query<(&Camera, &GlobalTransform), With<IsoCamera>>) -> Option<Vec2> {
    let (window, (camera, cam_tf)) = (windows.single().ok()?, cams.single().ok()?);
    let ray = camera.viewport_to_world(cam_tf, crate::render_scale::to_view(camera, window, window.cursor_position()?)).ok()?;
    // A ray that never comes down (first person, looking level or up) is on no ground.
    let t = if ray.direction.y.abs() < 1e-4 { return None } else { -ray.origin.y / ray.direction.y };
    let hit = ray.origin + *ray.direction * t;
    (t > 0.0).then_some(Vec2::new(hit.x, hit.z))
}

/// What Y asks for: free the survivor nearest us if its hands are tied, else tie them.
pub fn tie_or_free(me: u32, players: &[zombye_net::PlayerView]) -> Intent {
    let at: Vec2 = players.iter().find(|p| p.id == me).map_or(Vec2::ZERO, |p| p.pos.into());
    let near = players
        .iter()
        .filter(|p| p.id != me && !p.dead && Vec2::from(p.pos).distance(at) <= NEAR)
        .min_by(|a, b| Vec2::from(a.pos).distance(at).total_cmp(&Vec2::from(b.pos).distance(at)));
    match near {
        Some(p) if p.restrained => Intent::Free { player: me, target: Some(p.id) },
        // Nobody near: the server says so.
        _ => Intent::Restrain { player: me, target: None },
    }
}

#[allow(clippy::too_many_arguments)]
fn keys(
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    cams: Query<(&Camera, &GlobalTransform), With<IsoCamera>>,
    panel: Res<crate::inventory_ui::InventoryPanel>,
    me: Res<LocalPlayer>,
    mut shown: ResMut<Shown>,
    mut link: ResMut<ServerLink>,
) {
    let id = me.id;
    let Some(own) = link.curr.players.iter().find(|p| p.id == id).copied().filter(|p| !p.dead) else { return };
    if keys.just_pressed(KeyCode::KeyY) {
        let intent = tie_or_free(id, &link.curr.players);
        link.send(intent);
    }
    if keys.just_pressed(KeyCode::KeyL) {
        let searching = link.own(id).is_some_and(|v| v.frisk.is_some());
        link.send(if searching { Intent::StopHands { player: id } } else { Intent::Frisk { player: id, target: None } });
    }
    if keys.just_pressed(KeyCode::KeyX) && own.hands.is_some() {
        link.send(Intent::StopHands { player: id });
    }
    // Struggling is held: said when it changes.
    let struggling = own.restrained && keys.pressed(KeyCode::KeyJ);
    if struggling != shown.struggling {
        shown.struggling = struggling;
        link.send(Intent::Struggle { player: id, on: struggling });
    }
    if keys.just_pressed(KeyCode::KeyN) {
        let invited = link.curr.group_of(id).is_some_and(|g| !g.invites.is_empty());
        let op = if keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]) {
            GroupOp::Leave
        } else if invited {
            GroupOp::Accept { from: None }
        } else {
            GroupOp::Invite { target: None }
        };
        link.send(Intent::Group { player: id, op });
    }
    if mouse.just_pressed(MouseButton::Middle)
        && !panel.open
        && let Some(at) = cursor_ground(&windows, &cams)
    {
        let on = link.curr.group_of(id).and_then(|g| g.markers.iter().find(|m| m.pos.distance(at) <= MARKER_PICK)).map(|m| m.id);
        let op = match on {
            Some(marker) => GroupOp::Unmark { marker: Some(marker), near: at.into() },
            None => GroupOp::Mark { pos: at.into() },
        };
        link.send(Intent::Group { player: id, op });
    }
}

/// What the bar says: the title, how full it is (0 to 1) and the hint under it.
pub fn bar_text(own: &zombye_net::PlayerView, tied_with: Option<&str>) -> Option<(String, f32, String)> {
    if own.dead {
        return None;
    }
    if let Some(act) = own.hands {
        let frac = if own.hands_total > 0.0 { 1.0 - own.hands_left / own.hands_total } else { 1.0 };
        return Some((format!("{}  {:.1} s", act.doing(), own.hands_left), frac.clamp(0.0, 1.0), "X: stop".into()));
    }
    if own.restrained {
        let with = tied_with.map_or(String::new(), |n| format!(" ({})", n.to_lowercase()));
        let pct = (own.struggle * 100.0).floor();
        let hint = if own.struggling { "Struggling: keep still and keep at it" } else { "Stand still and hold J to struggle free" };
        return Some((format!("Your hands are tied{with}  {pct:.0}% out"), own.struggle, hint.into()));
    }
    None
}

#[allow(clippy::type_complexity)]
fn show_bar(
    link: Res<ServerLink>,
    join: Res<JoinInfo>,
    me: Res<LocalPlayer>,
    mut bar: Query<&mut Visibility, With<Bar>>,
    mut title: Query<&mut Text, (With<BarTitle>, Without<BarHint>)>,
    mut hint: Query<&mut Text, (With<BarHint>, Without<BarTitle>)>,
    mut fill: Query<&mut Node, With<BarFill>>,
) {
    let (Ok(mut vis), Ok(mut title), Ok(mut hint), Ok(mut fill)) = (bar.single_mut(), title.single_mut(), hint.single_mut(), fill.single_mut()) else { return };
    let own = link.curr.players.iter().find(|p| p.id == me.id);
    let with = link.own(me.id).and_then(|v| v.restraint).and_then(|i| join.items.get(i as usize)).map(|d| d.name.as_str());
    let Some((t, frac, h)) = own.and_then(|p| bar_text(p, with)) else {
        if *vis != Visibility::Hidden {
            *vis = Visibility::Hidden;
        }
        return;
    };
    if title.0 != t {
        title.0 = t;
    }
    if hint.0 != h {
        hint.0 = h;
    }
    fill.width = percent(frac * 100.0);
    if *vis != Visibility::Inherited {
        *vis = Visibility::Inherited;
    }
}

/// The rows of the search panel: where each stack is, and how it reads.
pub fn frisk_rows(v: &FriskView, name: impl Fn(u16) -> String) -> Vec<(Place, u32, ItemStack, String)> {
    let inv = &v.inventory;
    let count = |s: &ItemStack| if s.count > 1 { format!("{} x {}", s.count, name(s.item)) } else { name(s.item) };
    let mut rows = Vec::new();
    for (slot, what) in [(Slot::Hand, "in hand"), (Slot::OffHand, "in the other hand"), (Slot::Back, "on the back"), (Slot::Shoulder, "on the shoulder")] {
        if let Some(s) = inv.slot(slot) {
            rows.push((Place::Slot(slot), 0, s, format!("{}  ({what})", count(&s))));
        }
    }
    rows.extend(inv.carried.iter().enumerate().map(|(k, s)| (Place::Carried, k as u32, *s, count(s))));
    rows.extend(inv.worn.iter().enumerate().map(|(k, s)| (Place::Worn, k as u32, *s, format!("{}  (worn)", count(s)))));
    rows
}

fn show_frisk(mut commands: Commands, link: Res<ServerLink>, join: Res<JoinInfo>, me: Res<LocalPlayer>, mut shown: ResMut<Shown>, mut panel: Query<(Entity, &mut Visibility), With<FriskPanel>>) {
    let Ok((root, mut vis)) = panel.single_mut() else { return };
    let frisk = link.own(me.id).and_then(|v| v.frisk.clone());
    if frisk == shown.frisk {
        return;
    }
    shown.frisk = frisk.clone();
    commands.entity(root).despawn_children();
    let Some(v) = frisk else {
        *vis = Visibility::Hidden;
        return;
    };
    *vis = Visibility::Inherited;
    let id = me.id;
    let rows = frisk_rows(&v, |i| join.items.get(i as usize).map_or("item".to_string(), |d| d.name.clone()));
    commands.entity(root).with_children(|c| {
        theme::title(c, "Searching a survivor");
        c.spawn((Text::new("Click: take | Alt: one | L: stop"), font(SMALL), TextColor(DIM), Node { align_self: AlignSelf::Center, margin: UiRect::bottom(px(6)), ..default() }, Pickable::IGNORE));
        if rows.is_empty() {
            c.spawn((Text::new("Nothing on them."), font(BODY), TextColor(DIM), Pickable::IGNORE));
        }
        for (from, index, stack, text) in rows {
            c.spawn((Node { padding: UiRect::axes(px(6), px(3)), ..default() }, BackgroundColor(ROW)))
                .with_children(|r| {
                    r.spawn((Text::new(text), font(BODY), TextColor(TEXT), TextLayout::no_wrap(), Pickable::IGNORE));
                })
                .observe(|ev: On<PointerOver>, mut bg: Query<&mut BackgroundColor>| {
                    if let Ok(mut b) = bg.get_mut(ev.entity) {
                        b.0 = ROW_OVER;
                    }
                })
                .observe(|ev: On<PointerOut>, mut bg: Query<&mut BackgroundColor>| {
                    if let Ok(mut b) = bg.get_mut(ev.entity) {
                        b.0 = ROW;
                    }
                })
                .observe(move |ev: On<PointerPress>, keys: Res<ButtonInput<KeyCode>>, mut link: ResMut<ServerLink>| {
                    if ev.button == PointerButton::Primary {
                        let one = keys.any_pressed([KeyCode::AltLeft, KeyCode::AltRight]);
                        link.send(Intent::TakeFrom { player: id, from, index, item: stack.item, count: one as u16 });
                    }
                });
        }
    });
}

/// The roster line: who is in our group, or who has asked us into theirs.
pub fn roster_text(g: Option<&GroupView>, me: u32) -> String {
    let Some(g) = g else { return String::new() };
    let mut lines = Vec::new();
    if !g.members.is_empty() {
        let names: Vec<String> = g.members.iter().map(|m| if m.id == me { format!("{} (you)", m.name) } else { m.name.clone() }).collect();
        lines.push(format!("Group: {}", names.join(", ")));
        lines.push("N: invite  |  Shift+N: leave  |  middle mouse: marker".to_string());
    }
    if let Some(from) = g.invites.last() {
        lines.push(format!("{} invites you into a group  -  N: accept", from.name));
    }
    lines.join("\n")
}

fn show_roster(link: Res<ServerLink>, me: Res<LocalPlayer>, mut shown: ResMut<Shown>, mut roster: Query<(&mut Text, &mut BackgroundColor), With<Roster>>) {
    let text = roster_text(link.curr.group_of(me.id), me.id);
    if text == shown.roster {
        return;
    }
    let Ok((mut t, mut back)) = roster.single_mut() else { return };
    back.0 = if text.is_empty() { Color::NONE } else { BG };
    t.0.clone_from(&text);
    shown.roster = text;
}

/// Names under the group members in view, and the shared markers where they are.
#[allow(clippy::too_many_arguments)]
fn place_tags(
    mut commands: Commands,
    ui: Res<UiScale>,
    link: Res<ServerLink>,
    me: Res<LocalPlayer>,
    cameras: Query<(&Camera, &GlobalTransform), With<IsoCamera>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    survivors: Query<(&SurvivorAnim, &GlobalTransform)>,
    mut tags: Query<(Entity, &Tag, &mut Node, &mut Text, &ComputedNode)>,
) {
    let group = link.curr.group_of(me.id).filter(|g| !g.members.is_empty());
    if group.is_none() && tags.is_empty() {
        return;
    }
    let Ok((camera, cam_tf)) = cameras.single() else { return };
    let scale = ui.0.max(0.01);
    // The world camera may draw at a render scale below the window's (render_scale.rs).
    let window = windows.single().ok();
    let view = window.map_or_else(|| camera.logical_viewport_size().unwrap_or(Vec2::new(1280.0, 720.0)), |w| Vec2::new(w.width(), w.height())) / scale;
    let my_pos: Vec2 = link.curr.players.iter().find(|p| p.id == me.id).map_or(Vec2::ZERO, |p| p.pos.into());
    // Where each label wanted now stands, and what it says.
    let want = |tag: Tag| -> Option<(Vec3, String)> {
        let g = group?;
        match tag {
            Tag::Member(id) => {
                let name = g.name_of(id).filter(|_| id != me.id)?;
                let (_, tf) = survivors.iter().find(|(a, _)| a.player == id)?;
                link.curr.players.iter().find(|p| p.id == id && !p.dead)?;
                Some((tf.translation(), name.to_string()))
            }
            Tag::Marker(id) => {
                let m = g.markers.iter().find(|m| m.id == id)?;
                Some((Vec3::new(m.pos.x, 0.0, m.pos.y), format!("MARK  {:.0} m", m.pos.distance(my_pos))))
            }
        }
    };
    let mut have: Vec<Tag> = Vec::new();
    for (e, tag, mut node, mut text, computed) in &mut tags {
        let Some((at, label)) = want(*tag) else {
            commands.entity(e).despawn();
            continue;
        };
        have.push(*tag);
        if text.0 != label {
            text.0 = label;
        }
        let Ok(at) = camera.world_to_viewport(cam_tf, at) else { continue };
        let size = computed.size() * computed.inverse_scale_factor();
        if size.x <= 0.0 {
            continue;
        }
        let at = window.map_or(at, |w| crate::render_scale::from_view(camera, w, at)) / scale;
        // A name just under the feet; a marker's label standing on its point. Kept on screen.
        let top = if matches!(tag, Tag::Member(_)) { at.y + 8.0 } else { at.y - size.y };
        let left = px((at.x - size.x * 0.5).clamp(4.0, (view.x - size.x - 4.0).max(4.0)).round());
        let top = px(top.clamp(4.0, (view.y - size.y - 4.0).max(4.0)).round());
        if node.left != left || node.top != top {
            (node.left, node.top) = (left, top);
        }
    }
    let Some(g) = group else { return };
    let wanted = g.members.iter().map(|m| Tag::Member(m.id)).chain(g.markers.iter().map(|m| Tag::Marker(m.id)));
    for tag in wanted.filter(|t| !have.contains(t)) {
        if want(tag).is_none() {
            continue;
        }
        let marker = matches!(tag, Tag::Marker(_));
        commands.spawn((
            tag,
            Text::new(""),
            font(if marker { SMALL } else { BODY }),
            TextColor(if marker { theme::INK } else { GROUP }),
            TextLayout::no_wrap(),
            Node { position_type: PositionType::Absolute, left: px(-2000), top: px(-2000), padding: UiRect::axes(px(5), px(1)), ..default() },
            BackgroundColor(if marker { GROUP } else { BG }),
            GlobalZIndex(2),
            Pickable::IGNORE,
        ));
    }
}

/// The other survivor of `--holdup-demo`, and how far through [`SCRIPT`] `--holdup-auto` is.
#[derive(Resource)]
pub struct HoldupDemo {
    pub other: u32,
    /// `--holdup-auto`: our side plays itself too.
    pub auto: bool,
    seq: u32,
    began: Option<u64>,
    next: usize,
    /// It has been tied, and freed again: it keeps its hands down from then on.
    tied: bool,
    freed: bool,
}

impl HoldupDemo {
    pub fn new(other: u32, auto: bool) -> Self {
        Self { other, auto, seq: 0, began: None, next: 0, tied: false, freed: false }
    }
}

/// A step of `--holdup-auto`: ours, or the other survivor's.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Step {
    Say(&'static str),
    Tie,
    Search,
    /// Take the first thing it carries, and what it holds.
    TakeCarried,
    TakeHeld,
    Stop,
    Free,
    Invite,
    /// A marker this far from us.
    Mark(f32, f32),
    TheySay(&'static str),
    TheyStruggle(bool),
    TheyWave,
}

/// What happens when (seconds in), with `--holdup-auto`.
pub const SCRIPT: [(f32, Step); 15] = [
    (0.6, Step::TheySay("Don't hurt me. Take what you want.")),
    (2.2, Step::Say("Hands where I can see them.")),
    (3.2, Step::Tie),
    (7.0, Step::Search),
    (9.8, Step::TakeCarried),
    (11.0, Step::TakeHeld),
    (13.4, Step::Stop),
    (13.6, Step::TheyStruggle(true)),
    (17.4, Step::TheyStruggle(false)),
    (17.6, Step::Free),
    (20.6, Step::TheySay("You could have just asked.")),
    (21.4, Step::Invite),
    (23.0, Step::Mark(5.0, -4.0)),
    (24.0, Step::TheyWave),
    (24.2, Step::TheySay("Fine. Together, then.")),
];

/// The step due `secs` in when `next` have been done, if any.
pub fn step_due(secs: f32, next: usize) -> Option<Step> {
    SCRIPT.get(next).filter(|(at, _)| secs >= *at).map(|(_, s)| *s)
}

fn drive_demo(join: Res<JoinInfo>, mut link: ResMut<ServerLink>, mut demo: ResMut<HoldupDemo>) {
    let tick = link.curr.tick;
    let (me, other) = (join.player_id, demo.other);
    let Some(world) = link.world() else { return };
    let (Some(it), Some(us)) = (world.player(other), world.player(me)) else { return };
    if it.dead {
        return;
    }
    // It stands where it is, facing us, with its hands up until it has been tied and freed.
    let to = us.state.pos - it.state.pos;
    let facing = if to.length_squared() > 1e-4 { to.y.atan2(to.x) } else { it.state.facing };
    let raise = !demo.freed && it.restraint.is_none() && it.gesture.is_none() && it.knock.is_none() && tick % 10 == 0;
    let invited = world.group_view(other).is_some_and(|g| !g.invites.is_empty());
    let (tied, us_at) = (it.restraint.is_some(), us.state.pos);
    let carried = it.inventory.carried.first().map(|s| s.item);
    let held = it.inventory.hand.map(|s| s.item);
    demo.freed |= demo.tied && !tied;
    demo.tied |= tied;
    demo.seq += 1;
    link.send(Intent::PlayerInput { player: other, seq: demo.seq, move_dir: [0.0; 2], facing, run: false, sneak: false, aim: false });
    if raise {
        link.send(Intent::Gesture { player: other, gesture: Some(Gesture::HandsUp) });
    }
    if invited {
        link.send(Intent::Group { player: other, op: GroupOp::Accept { from: None } });
    }
    if !demo.auto {
        return;
    }
    let began = *demo.began.get_or_insert(tick);
    let secs = (tick - began) as f32 * zombye_sim::TICK_DT;
    while let Some(step) = step_due(secs, demo.next) {
        demo.next += 1;
        let intent = match step {
            Step::Say(text) => Intent::Say { player: me, text: text.into(), volume: Volume::Talk },
            Step::Tie => Intent::Restrain { player: me, target: Some(other) },
            Step::Search => Intent::Frisk { player: me, target: Some(other) },
            Step::TakeCarried => match carried {
                Some(item) => Intent::TakeFrom { player: me, from: Place::Carried, index: 0, item, count: 0 },
                None => continue,
            },
            Step::TakeHeld => match held {
                Some(item) => Intent::TakeFrom { player: me, from: Place::Slot(Slot::Hand), index: 0, item, count: 0 },
                None => continue,
            },
            Step::Stop => Intent::StopHands { player: me },
            Step::Free => Intent::Free { player: me, target: Some(other) },
            Step::Invite => Intent::Group { player: me, op: GroupOp::Invite { target: Some(other) } },
            Step::Mark(dx, dy) => Intent::Group { player: me, op: GroupOp::Mark { pos: (us_at + Vec2::new(dx, dy)).into() } },
            Step::TheySay(text) => Intent::Say { player: other, text: text.into(), volume: Volume::Talk },
            Step::TheyStruggle(on) => Intent::Struggle { player: other, on },
            Step::TheyWave => Intent::Gesture { player: other, gesture: Some(Gesture::Wave) },
        };
        link.send(intent);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zombye_net::PlayerView;
    use zombye_sim::{HandsAct, Inventory, MemberView};

    fn view(id: u32, x: f32) -> PlayerView {
        zombye_net::wire::PlayerPacked { id, x: (x * 64.0) as u32, y: 640, facing: 0, bits: 0, hp: 100, weapon: None, action: None, action_left: 0, ready_in: 0, knock: None }.unpack()
    }

    #[test]
    fn y_ties_the_free_and_frees_the_tied() {
        let me = view(1, 10.0);
        let mut other = view(2, 11.0);
        let far = view(3, 20.0);
        assert_eq!(format!("{:?}", tie_or_free(1, &[me, other, far])), format!("{:?}", Intent::Restrain { player: 1, target: None }));
        other.restrained = true;
        assert_eq!(format!("{:?}", tie_or_free(1, &[me, other, far])), format!("{:?}", Intent::Free { player: 1, target: Some(2) }));
        // Nobody near: the server gets to say so.
        assert_eq!(format!("{:?}", tie_or_free(1, &[me, far])), format!("{:?}", Intent::Restrain { player: 1, target: None }));
    }

    #[test]
    fn the_bar_says_what_the_hands_are_at_and_how_far_out_we_are() {
        let mut p = view(1, 10.0);
        assert!(bar_text(&p, None).is_none());
        (p.hands, p.hands_left, p.hands_total) = (Some(HandsAct::Tie), 0.75, 3.0);
        let (title, frac, hint) = bar_text(&p, None).unwrap();
        assert_eq!((title.as_str(), frac, hint.as_str()), ("Tying their hands  0.8 s", 0.75, "X: stop"));
        p.hands = None;
        (p.restrained, p.struggle) = (true, 0.336);
        let (title, frac, hint) = bar_text(&p, Some("Cable tie")).unwrap();
        assert_eq!(title, "Your hands are tied (cable tie)  33% out");
        assert!(frac == 0.336 && hint.contains("hold J"));
        p.struggling = true;
        assert!(bar_text(&p, None).unwrap().2.starts_with("Struggling"));
        p.dead = true;
        assert!(bar_text(&p, None).is_none());
    }

    #[test]
    fn the_search_panel_lists_what_they_hold_carry_and_wear() {
        let mut inventory = Inventory::default();
        inventory.hand = Some(ItemStack::one(7));
        inventory.carried = vec![ItemStack::new(3, 2), ItemStack::one(4)];
        inventory.worn = vec![ItemStack::one(9)];
        let rows = frisk_rows(&FriskView { target: 2, inventory }, |i| format!("item{i}"));
        let texts: Vec<&str> = rows.iter().map(|r| r.3.as_str()).collect();
        assert_eq!(texts, ["item7  (in hand)", "2 x item3", "item4", "item9  (worn)"]);
        assert_eq!((rows[0].0, rows[2].0, rows[2].1, rows[3].0), (Place::Slot(Slot::Hand), Place::Carried, 1, Place::Worn));
    }

    #[test]
    fn the_roster_names_the_group_and_whoever_asks() {
        let m = |id: u32, name: &str| MemberView { id, name: name.into() };
        assert_eq!(roster_text(None, 1), "");
        let invited = GroupView { player: 1, invites: vec![m(2, "bob")], ..Default::default() };
        assert_eq!(roster_text(Some(&invited), 1), "bob invites you into a group  -  N: accept");
        let g = GroupView { player: 1, members: vec![m(2, "bob"), m(1, "ada")], ..Default::default() };
        assert!(roster_text(Some(&g), 1).starts_with("Group: bob, ada (you)\n"));
    }

    #[test]
    fn the_script_runs_in_order() {
        assert!(SCRIPT.windows(2).all(|w| w[0].0 < w[1].0));
        assert_eq!(step_due(0.0, 0), None);
        assert_eq!(step_due(3.3, 2), Some(Step::Tie));
        assert_eq!(step_due(100.0, SCRIPT.len()), None);
    }
}
