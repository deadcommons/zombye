//! The current action (#98): while an item job is under way (moving, searching, eating, treating,
//! crafting) a bar at the bottom centre says what it is, how far along, how many more are queued, and
//! how to cancel. X (or clicking the bar) cancels the one under way, Shift+X the whole queue
//! (`Intent::CancelJobs`). The progress is `busy` (seconds left, from the snapshot) over the most it was
//! seen at for this job.

use bevy::prelude::*;
use zombye_net::Intent;
use zombye_sim::{ItemCategory, JobKind};

use crate::player::LocalPlayer;
use crate::server_link::{JoinInfo, ServerLink};

use crate::theme::{self, TEXT, font};

const FILL: Color = theme::GOLD;
const WIDTH: f32 = 420.0;

#[derive(Component)]
struct Panel;
#[derive(Component)]
struct Title;
#[derive(Component)]
struct Fill;
#[derive(Component)]
struct Hint;

/// The job under way as last seen, for its total time.
#[derive(Resource, Default)]
struct Track {
    kind: Option<JobKind>,
    total: f32,
    last: f32,
}

pub struct ActionUiPlugin;

impl Plugin for ActionUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Track>().add_systems(Startup, spawn).add_systems(Update, (cancel_key, update).chain());
    }
}

fn spawn(mut commands: Commands) {
    commands
        .spawn((
            Panel,
            crate::hud::BottomHud,
            Node {
                position_type: PositionType::Absolute,
                // Just above the hotbar (#97).
                bottom: px(crate::hotbar::REACH + 8.0),
                left: percent(50),
                margin: UiRect::left(px(-WIDTH / 2.0)),
                width: px(WIDTH),
                flex_direction: FlexDirection::Column,
                row_gap: px(4),
                padding: UiRect::axes(px(theme::STRIP_PAD.0 + 2.0), px(theme::STRIP_PAD.1)),
                ..default()
            },
            theme::strip(),
            // Over the inventory screen's frame too: what you are doing stays in view there.
            GlobalZIndex(8),
            Visibility::Hidden,
        ))
        .with_children(|c| {
            c.spawn((Title, Text::new(""), font(theme::BODY), TextColor(TEXT), Pickable::IGNORE));
            c.spawn((Node { width: percent(100), height: px(10), ..default() }, theme::bar_track(), Pickable::IGNORE))
                .with_children(|b| {
                    b.spawn((Fill, Node { width: percent(0), height: percent(100), ..default() }, theme::bar_fill(FILL), Pickable::IGNORE));
                });
            c.spawn((Hint, Text::new(""), font(crate::theme::SMALL), TextColor(crate::theme::DIM), Pickable::IGNORE));
        })
        .observe(|_: On<PointerPress>, me: Res<LocalPlayer>, keys: Res<ButtonInput<KeyCode>>, mut link: ResMut<ServerLink>| {
            let all = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
            link.send(Intent::CancelJobs { player: me.id, all });
        });
}

/// What a job is, in words ("Washing rags").
fn describe(kind: JobKind, join: &JoinInfo) -> String {
    let item = |i: u16| join.items.get(i as usize).map_or("item".to_string(), |d| d.name.to_lowercase());
    match kind {
        JobKind::Move { item: i, .. } => format!("Moving {}", item(i)),
        JobKind::Search { .. } => "Searching".to_string(),
        JobKind::Use { item: i, .. } => {
            let drink = join.items.get(i as usize).is_some_and(|d| d.category == ItemCategory::Drink);
            format!("{} {}", if drink { "Drinking" } else { "Eating" }, item(i))
        }
        JobKind::Treat { treatment, .. } => {
            let v = treatment.verb();
            let mut c = v.chars();
            c.next().map_or(String::new(), |f| f.to_uppercase().collect::<String>() + c.as_str())
        }
        JobKind::Craft { recipe } => join.recipes.get(recipe as usize).map_or("Making".to_string(), |r| r.name.clone()),
        JobKind::Reload { spare: false } => "Reloading".to_string(),
        JobKind::Reload { spare: true } => "Filling a magazine".to_string(),
        JobKind::Blind { fit: true, .. } => "Fitting a blind".to_string(),
        JobKind::Blind { fit: false, .. } => "Taking a blind down".to_string(),
    }
}

fn cancel_key(keys: Res<ButtonInput<KeyCode>>, me: Res<LocalPlayer>, mut link: ResMut<ServerLink>) {
    if me.dead || !keys.just_pressed(KeyCode::KeyX) {
        return;
    }
    let all = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    link.send(Intent::CancelJobs { player: me.id, all });
}

#[allow(clippy::too_many_arguments)]
fn update(
    link: Res<ServerLink>,
    join: Res<JoinInfo>,
    me: Res<LocalPlayer>,
    mut track: ResMut<Track>,
    mut panel: Query<&mut Visibility, With<Panel>>,
    mut title: Query<&mut Text, (With<Title>, Without<Hint>)>,
    mut hint: Query<&mut Text, (With<Hint>, Without<Title>)>,
    mut fill: Query<&mut Node, With<Fill>>,
) {
    let (Ok(mut vis), Ok(mut title), Ok(mut hint), Ok(mut fill)) = (panel.single_mut(), title.single_mut(), hint.single_mut(), fill.single_mut()) else { return };
    let job = link.own(me.id).and_then(|v| v.jobs.first().copied()).filter(|_| !me.dead);
    let busy = link.curr.players.iter().find(|p| p.id == me.id).map_or(0.0, |p| p.busy);
    let queued = link.curr.players.iter().find(|p| p.id == me.id).map_or(0, |p| p.jobs);
    let Some(kind) = job else {
        *track = Track::default();
        *vis = Visibility::Hidden;
        return;
    };
    // A new job: another kind, or the seconds went up (the next one began).
    if track.kind != Some(kind) || busy > track.last + 0.01 {
        track.kind = Some(kind);
        // Seen part way (a loaded save): a craft's own time is the best guess at its length.
        let guess = match kind {
            JobKind::Craft { recipe } => join.recipes.get(recipe as usize).map_or(0.0, |r| r.secs),
            _ => 0.0,
        };
        track.total = busy.max(guess);
    }
    track.last = busy;
    track.total = track.total.max(busy);
    let frac = if track.total > 0.0 { (1.0 - busy / track.total).clamp(0.0, 1.0) } else { 1.0 };
    let more = if queued > 1 { format!("  (+{} queued)", queued - 1) } else { String::new() };
    title.0 = format!("{}  {busy:.1} s{more}", describe(kind, &join));
    hint.0 = if queued > 1 { "X or click: cancel  |  Shift+X: all".into() } else { "X or click: cancel".into() };
    fill.width = percent(frac * 100.0);
    *vis = Visibility::Inherited;
}
