//! Talking to whoever is near, and gestures (#67, docs/PLAN.md §4.8).
//!
//! **Saying something:** Enter opens the chat line at the bottom left; type, Tab picks the volume
//! (whisper, talk, shout), Enter says it (`Intent::Say`), Esc drops it. While it is open every
//! other key is held back (as the inventory's search box does).
//!
//! **Hearing:** the server sends what our survivor heard ([`ServerLink::speech`]; our own lines
//! too, once it took them). Each is a line in the chat log above the chat line and a bubble in the
//! world: over the speaker when we were told who spoke (our survivor sees them), else where the
//! voice came from, and the log then says which way that is ("A voice to the north-east"). A
//! voice through a wall is muffled: greyed, and the log says so. Shouts are bigger and paler
//! yellow, whispers small and dim. Nobody has a name tag (PLAN §4.8): others are "A survivor".
//!
//! **Gestures:** V waves, U puts your hands up (again lowers them), P points the way you face
//! (`Intent::Gesture`); `survivor.rs` poses them.
//!
//! For recordings and pictures: `--say TEXT[@SECS]`, `--shout`, `--whisper` and
//! `--gesture NAME[@SECS]` do it by themselves (offscreen, where nothing ticks, the lines are
//! shown as if just said), and `--talk-demo` / `--talk-wall` add another survivor driven from
//! here through the intents a client sends ([`TalkDemo`]): it turns to us, greets, puts its hands
//! up, points and shouts, in a loop.

use std::collections::VecDeque;

use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::prelude::*;
use zombye_net::Intent;
use zombye_sim::{Gesture, Said, Volume};

use crate::Options;
use crate::camera::IsoCamera;
use crate::player::{LocalPlayer, PlayerInputSet};
use crate::server_link::ServerLink;
use crate::survivor::SurvivorAnim;

/// `--talk-demo`: how far east of us the other survivor starts.
pub const TALK_DISTANCE: f32 = 4.0;
/// The longest line the chat line takes (the server cuts at its own limit, 120 by default).
const MAX_CHARS: usize = 120;
/// Lines the log shows, and how long each stays (seconds).
const LOG_LINES: usize = 6;
const LOG_SECS: f32 = 14.0;
/// Bubbles alive at once, and how high over the ground a voice's bubble sits (metres).
const MAX_BUBBLES: usize = 8;
const BUBBLE_HEIGHT: f32 = 2.15;
const BUBBLE_WIDTH: f32 = 300.0;

const TEXT: Color = Color::srgb(0.93, 0.9, 0.82);
const OWN: Color = Color::srgb(0.95, 0.8, 0.45);
const SHOUT: Color = Color::srgb(1.0, 0.86, 0.5);
const DIM: Color = Color::srgb(0.66, 0.66, 0.68);
const BACK: Color = Color::srgba(0.02, 0.02, 0.03, 0.78);

/// The chat line being typed.
#[derive(Resource, Default)]
pub struct ChatEntry {
    pub typing: bool,
    text: String,
    volume: Volume,
}

struct LogLine {
    text: String,
    color: Color,
    at: f32,
}

#[derive(Resource, Default)]
struct ChatLog {
    lines: VecDeque<LogLine>,
    changed: bool,
}

#[derive(Component)]
struct LogText(usize);
#[derive(Component)]
struct EntryText;

/// A line in the world: over `speaker` while that survivor is drawn (0: nobody we know), else at `pos`.
#[derive(Component)]
struct Bubble {
    speaker: u32,
    pos: Vec2,
    until: f32,
}

/// `--say` and `--gesture` done so far.
#[derive(Resource, Default)]
struct Script {
    said: usize,
    gestured: usize,
}

pub struct ChatPlugin;

impl Plugin for ChatPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ChatEntry>()
            .init_resource::<ChatLog>()
            .init_resource::<Script>()
            .add_systems(Startup, spawn)
            // Before the search box and every other key handler: while the chat line is open the
            // keys are ours alone.
            .add_systems(PreUpdate, chat_keys.after(bevy::input::InputSystems).before(crate::inventory_ui::TextEntry).run_if(crate::live))
            .add_systems(Update, (gesture_keys.run_if(crate::live), script, hear, show_log, show_entry, place_bubbles).chain().run_if(resource_exists::<ServerLink>))
            .add_systems(FixedUpdate, drive_demo.before(PlayerInputSet).run_if(resource_exists::<TalkDemo>).run_if(crate::live));
    }
}

fn font(size: f32) -> TextFont {
    TextFont { font_size: bevy::text::FontSize::Px(size), ..default() }
}

fn spawn(mut commands: Commands) {
    // Bottom left, above the vitals and the item note: the log, then the line being typed.
    commands
        .spawn((Node { position_type: PositionType::Absolute, bottom: px(178), left: px(18), max_width: px(345), flex_direction: FlexDirection::Column, align_items: AlignItems::FlexStart, row_gap: px(2), ..default() }, Pickable::IGNORE))
        .with_children(|c| {
            for k in 0..LOG_LINES {
                c.spawn((LogText(k), Text::new(""), font(15.0), TextColor(TEXT), BackgroundColor(Color::NONE), Pickable::IGNORE));
            }
            c.spawn((EntryText, Text::new(""), font(16.0), TextColor(OWN), BackgroundColor(Color::NONE), Pickable::IGNORE));
        });
}

/// The next volume round (Tab in the chat line).
fn next_volume(v: Volume) -> Volume {
    match v {
        Volume::Talk => Volume::Shout,
        Volume::Shout => Volume::Whisper,
        Volume::Whisper => Volume::Talk,
    }
}

#[allow(clippy::too_many_arguments)]
fn chat_keys(
    mut typed: MessageReader<KeyboardInput>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut entry: ResMut<ChatEntry>,
    find: Option<Res<crate::inventory_ui::Find>>,
    night: Option<Res<crate::firstnight_ui::FirstNightUi>>,
    link: Option<ResMut<ServerLink>>,
    me: Option<Res<LocalPlayer>>,
) {
    let (Some(mut link), Some(me)) = (link, me) else { return };
    if !entry.typing {
        typed.clear();
        // Not over the search box (its Enter ends the search), the morning screen (its Enter
        // carries on) or the death screen.
        let dead = link.curr.players.iter().any(|p| p.id == me.id && p.dead);
        let busy = find.is_some_and(|f| f.typing) || night.is_some_and(|n| n.morning) || dead;
        if !busy && keys.any_just_pressed([KeyCode::Enter, KeyCode::NumpadEnter]) {
            entry.typing = true;
            entry.text.clear();
            keys.reset_all();
        }
        return;
    }
    for k in typed.read().filter(|k| k.state.is_pressed()) {
        match &k.logical_key {
            Key::Character(c) => {
                let room = MAX_CHARS.saturating_sub(entry.text.chars().count());
                entry.text.extend(c.chars().filter(|c| !c.is_control()).take(room));
            }
            Key::Space if entry.text.chars().count() < MAX_CHARS => entry.text.push(' '),
            Key::Backspace => {
                entry.text.pop();
            }
            Key::Tab => entry.volume = next_volume(entry.volume),
            Key::Enter => {
                if !entry.text.trim().is_empty() {
                    link.send(Intent::Say { player: me.id, text: entry.text.clone(), volume: entry.volume });
                }
                entry.typing = false;
                entry.text.clear();
            }
            Key::Escape => {
                entry.typing = false;
                entry.text.clear();
            }
            _ => {}
        }
    }
    keys.reset_all();
}

/// V waves, U puts the hands up (again lowers them), P points.
fn gesture_keys(keys: Res<ButtonInput<KeyCode>>, mut link: ResMut<ServerLink>, me: Res<LocalPlayer>) {
    for (key, gesture) in [(KeyCode::KeyV, Gesture::Wave), (KeyCode::KeyU, Gesture::HandsUp), (KeyCode::KeyP, Gesture::Point)] {
        if keys.just_pressed(key) {
            link.send(Intent::Gesture { player: me.id, gesture: Some(gesture) });
        }
    }
}

/// `--say TEXT[@SECS]` (and `--shout`, `--whisper`) and `--gesture NAME[@SECS]`. Offscreen nothing
/// ticks, so the lines are shown as if our survivor had just said them.
fn script(time: Res<Time>, options: Res<Options>, mut link: ResMut<ServerLink>, me: Res<LocalPlayer>, mut done: ResMut<Script>) {
    let now = time.elapsed_secs();
    while let Some((volume, text, _)) = options.say.get(done.said).filter(|s| now >= s.2 || options.offscreen()) {
        done.said += 1;
        if options.offscreen() {
            let pos = link.curr.players.iter().find(|p| p.id == me.id).map_or(Vec2::ZERO, |p| p.pos.into());
            let tick = link.curr.tick;
            link.speech.push(Said { listener: me.id, speaker: me.id, seen: true, pos, volume: *volume, muffled: false, text: text.clone(), tick });
        } else {
            link.send(Intent::Say { player: me.id, text: text.clone(), volume: *volume });
        }
    }
    while let Some((gesture, _)) = options.gestures.get(done.gestured).filter(|g| now >= g.1 && !options.offscreen()) {
        done.gestured += 1;
        link.send(Intent::Gesture { player: me.id, gesture: Some(*gesture) });
    }
}

/// Which way `to` lies from `from`, in words (the map's north is -y).
pub fn compass(from: Vec2, to: Vec2) -> &'static str {
    let d = to - from;
    if d.length_squared() < 0.25 {
        return "close by";
    }
    // Eighths of a turn, clockwise from north.
    let turn = d.x.atan2(-d.y).rem_euclid(std::f32::consts::TAU) / std::f32::consts::TAU;
    ["to the north", "to the north-east", "to the east", "to the south-east", "to the south", "to the south-west", "to the west", "to the north-west"][(turn * 8.0).round() as usize % 8]
}

/// A heard line as the log writes it: `You say: ...`, `A survivor shouts: ...`,
/// `A muffled voice to the east says: ...`.
///
/// `agent`: the speaker is one we were told a program plays (#69, a server that marks agents):
/// `An agent says: ...`.
pub fn log_text(s: &Said, me: u32, my_pos: Vec2, agent: bool) -> String {
    let verb = s.volume.verb();
    if s.speaker == me {
        return format!("You {}: {}", &verb[..verb.len() - 1], s.text);
    }
    let muffled = if s.muffled { "muffled " } else { "" };
    if s.speaker != 0 {
        let through = if s.muffled { ", muffled" } else { "" };
        let who = if agent { "An agent" } else { "A survivor" };
        format!("{who} {verb}{through}: {}", s.text)
    } else {
        format!("A {muffled}voice {} {verb}: {}", compass(my_pos, s.pos), s.text)
    }
}

/// How long a bubble stays up: longer for more to read.
pub fn bubble_secs(text: &str) -> f32 {
    (2.5 + 0.07 * text.chars().count() as f32).min(8.0)
}

/// Each line our survivor heard this frame: the log, stdout and a bubble in the world.
fn hear(mut commands: Commands, time: Res<Time>, link: Res<ServerLink>, me: Res<LocalPlayer>, mut log: ResMut<ChatLog>, bubbles: Query<(Entity, &Bubble)>) {
    if link.speech.is_empty() {
        return;
    }
    let now = time.elapsed_secs();
    let my_pos = link.curr.players.iter().find(|p| p.id == me.id).map_or(Vec2::ZERO, |p| p.pos.into());
    let mut alive: Vec<(Entity, u32, f32)> = bubbles.iter().map(|(e, b)| (e, b.speaker, b.until)).collect();
    for s in link.speech.iter().filter(|s| s.listener == me.id) {
        let agent = s.speaker != 0 && link.curr.players.iter().any(|p| p.id == s.speaker && p.agent);
        let line = log_text(s, me.id, my_pos, agent);
        println!("chat: {line}");
        let color = match (s.speaker == me.id, s.muffled, s.volume) {
            (true, ..) => OWN,
            (_, true, _) => DIM,
            (_, _, Volume::Shout) => SHOUT,
            (_, _, Volume::Whisper) => DIM,
            _ => TEXT,
        };
        log.lines.push_back(LogLine { text: line, color, at: now });
        while log.lines.len() > LOG_LINES {
            log.lines.pop_front();
        }
        log.changed = true;
        // One bubble a speaker (the new line takes the old one's place), and not too many in all.
        alive.retain(|&(e, speaker, _)| {
            let same = speaker != 0 && speaker == s.speaker;
            if same {
                commands.entity(e).despawn();
            }
            !same
        });
        if alive.len() >= MAX_BUBBLES {
            alive.sort_by(|a, b| a.2.total_cmp(&b.2));
            commands.entity(alive.remove(0).0).despawn();
        }
        let (size, shade) = match s.volume {
            Volume::Shout => (19.0, SHOUT),
            Volume::Talk => (15.0, TEXT),
            Volume::Whisper => (13.0, DIM),
        };
        let until = now + bubble_secs(&s.text);
        let e = commands
            .spawn((
                Bubble { speaker: s.speaker, pos: s.pos, until },
                Text::new(s.text.clone()),
                font(size),
                TextColor(if s.muffled { DIM } else { shade }),
                TextLayout::justify(Justify::Center),
                Node { position_type: PositionType::Absolute, left: px(-2000), top: px(-2000), max_width: px(BUBBLE_WIDTH), ..default() },
                BackgroundColor(BACK),
                GlobalZIndex(4),
                Pickable::IGNORE,
            ))
            .id();
        alive.push((e, s.speaker, until));
    }
}

fn show_log(time: Res<Time>, mut log: ResMut<ChatLog>, mut texts: Query<(&LogText, &mut Text, &mut TextColor, &mut BackgroundColor)>) {
    let now = time.elapsed_secs();
    let before = log.lines.len();
    log.lines.retain(|l| now - l.at < LOG_SECS);
    if log.lines.len() == before && !log.changed {
        return;
    }
    log.changed = false;
    for (slot, mut text, mut color, mut back) in &mut texts {
        // The newest at the bottom, the empty rows above.
        let line = (slot.0 + log.lines.len()).checked_sub(LOG_LINES).and_then(|k| log.lines.get(k));
        let want = line.map_or("", |l| l.text.as_str());
        if text.0 != want {
            text.0 = want.to_string();
        }
        color.0 = line.map_or(TEXT, |l| l.color);
        back.0 = if line.is_some() { BACK } else { Color::NONE };
    }
}

fn show_entry(entry: Res<ChatEntry>, mut text: Query<(&mut Text, &mut BackgroundColor), With<EntryText>>) {
    if !entry.is_changed() {
        return;
    }
    let Ok((mut t, mut back)) = text.single_mut() else { return };
    let label = match entry.volume {
        Volume::Whisper => "Whisper",
        Volume::Talk => "Say",
        Volume::Shout => "Shout",
    };
    t.0 = if entry.typing { format!("{label}: {}_    (Tab: whisper / talk / shout, Enter: say it, Esc: never mind)", entry.text) } else { String::new() };
    back.0 = if entry.typing { BACK } else { Color::NONE };
}

/// Each bubble over its speaker's head on screen (or where the voice came from), until its time is up.
fn place_bubbles(
    mut commands: Commands,
    time: Res<Time>,
    ui: Res<UiScale>,
    cameras: Query<(&Camera, &GlobalTransform), With<IsoCamera>>,
    survivors: Query<(&SurvivorAnim, &GlobalTransform)>,
    mut bubbles: Query<(Entity, &Bubble, &mut Node, &ComputedNode)>,
) {
    if bubbles.is_empty() {
        return;
    }
    let now = time.elapsed_secs();
    let Ok((camera, cam_tf)) = cameras.single() else { return };
    let scale = ui.0.max(0.01);
    let view = camera.logical_viewport_size().unwrap_or(Vec2::new(1280.0, 720.0)) / scale;
    for (e, b, mut node, computed) in &mut bubbles {
        if now >= b.until {
            commands.entity(e).despawn();
            continue;
        }
        let ground = survivors.iter().find(|(a, _)| b.speaker != 0 && a.player == b.speaker).map_or(Vec3::new(b.pos.x, 0.0, b.pos.y), |(_, tf)| tf.translation());
        let Ok(at) = camera.world_to_viewport(cam_tf, ground + Vec3::Y * BUBBLE_HEIGHT) else { continue };
        let size = computed.size() * computed.inverse_scale_factor();
        if size.x <= 0.0 {
            // Not laid out yet: next frame.
            continue;
        }
        // Centred over the head, kept on screen (a voice off screen sits at the edge, on its side).
        let at = at / scale;
        let left = (at.x - size.x * 0.5).clamp(4.0, (view.x - size.x - 4.0).max(4.0));
        let top = (at.y - size.y).clamp(4.0, (view.y - size.y - 4.0).max(4.0));
        let (left, top) = (px(left.round()), px(top.round()));
        if node.left != left || node.top != top {
            (node.left, node.top) = (left, top);
        }
    }
}

/// The other survivor of `--talk-demo` / `--talk-wall`: its player, its input count, the tick it
/// began on and how far through its lines it is.
#[derive(Resource)]
pub struct TalkDemo {
    pub other: u32,
    seq: u32,
    began: Option<u64>,
    next: usize,
}

impl TalkDemo {
    pub fn new(other: u32) -> Self {
        Self { other, seq: 0, began: None, next: 0 }
    }
}

/// What the other survivor does, and when (seconds into each round).
enum Step {
    Say(Volume, &'static str),
    Gesture(Option<Gesture>),
}

const ROUND_SECS: f32 = 20.0;
const SCRIPT: [(f32, Step); 9] = [
    (1.0, Step::Gesture(Some(Gesture::Wave))),
    (1.2, Step::Say(Volume::Talk, "Hey! Over here.")),
    (5.0, Step::Gesture(Some(Gesture::HandsUp))),
    (5.3, Step::Say(Volume::Talk, "Friendly. Don't swing.")),
    (9.5, Step::Gesture(None)),
    (10.0, Step::Gesture(Some(Gesture::Point))),
    (10.3, Step::Say(Volume::Talk, "There's food in the shop on Main.")),
    (14.0, Step::Say(Volume::Whisper, "Keep it down, they hear us.")),
    (17.0, Step::Say(Volume::Shout, "BEHIND YOU! RUN!")),
];

/// The step due `secs` into a round when `next` have been done, if any.
fn step_due(secs: f32, next: usize) -> Option<&'static Step> {
    SCRIPT.get(next).filter(|(at, _)| secs >= *at).map(|(_, s)| s)
}

fn drive_demo(join: Res<crate::server_link::JoinInfo>, mut link: ResMut<ServerLink>, mut demo: ResMut<TalkDemo>) {
    let tick = link.curr.tick;
    let Some(world) = link.world() else { return };
    let (Some(it), Some(us)) = (world.player(demo.other), world.player(join.player_id)) else { return };
    if it.dead {
        return;
    }
    // It stands where it is and turns to us.
    let to = us.state.pos - it.state.pos;
    let facing = if to.length_squared() > 1e-4 { to.y.atan2(to.x) } else { it.state.facing };
    let other = demo.other;
    demo.seq += 1;
    link.send(Intent::PlayerInput { player: other, seq: demo.seq, move_dir: [0.0; 2], facing, run: false, sneak: false, aim: false });
    let began = *demo.began.get_or_insert(tick);
    let secs = (tick - began) as f32 * zombye_sim::TICK_DT;
    if secs >= ROUND_SECS {
        (demo.began, demo.next) = (Some(tick), 0);
        return;
    }
    while let Some(step) = step_due(secs, demo.next) {
        demo.next += 1;
        link.send(match step {
            Step::Say(volume, text) => Intent::Say { player: other, text: (*text).into(), volume: *volume },
            Step::Gesture(gesture) => Intent::Gesture { player: other, gesture: *gesture },
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn said(speaker: u32, volume: Volume, muffled: bool, pos: Vec2) -> Said {
        Said { listener: 1, speaker, seen: speaker != 0, pos, volume, muffled, text: "hello".into(), tick: 0 }
    }

    #[test]
    fn the_log_says_who_or_from_where() {
        let me = Vec2::new(10.0, 10.0);
        assert_eq!(log_text(&said(1, Volume::Talk, false, me), 1, me, false), "You say: hello");
        assert_eq!(log_text(&said(1, Volume::Shout, false, me), 1, me, false), "You shout: hello");
        assert_eq!(log_text(&said(1, Volume::Whisper, false, me), 1, me, false), "You whisper: hello");
        assert_eq!(log_text(&said(2, Volume::Talk, false, me + Vec2::X), 1, me, false), "A survivor says: hello");
        assert_eq!(log_text(&said(2, Volume::Shout, true, me + Vec2::X), 1, me, false), "A survivor shouts, muffled: hello");
        // On a server that marks agents (#69), a program's survivor says so.
        assert_eq!(log_text(&said(2, Volume::Talk, false, me + Vec2::X), 1, me, true), "An agent says: hello");
        assert_eq!(log_text(&said(1, Volume::Talk, false, me), 1, me, true), "You say: hello");
        // Not told who: a voice, and which way.
        assert_eq!(log_text(&said(0, Volume::Talk, true, me + Vec2::new(4.0, 0.0)), 1, me, false), "A muffled voice to the east says: hello");
        assert_eq!(log_text(&said(0, Volume::Shout, false, me + Vec2::new(-20.0, -20.0)), 1, me, false), "A voice to the north-west shouts: hello");
    }

    #[test]
    fn the_compass_goes_by_the_map() {
        let o = Vec2::ZERO;
        assert_eq!(compass(o, Vec2::new(0.0, -5.0)), "to the north");
        assert_eq!(compass(o, Vec2::new(5.0, 0.0)), "to the east");
        assert_eq!(compass(o, Vec2::new(0.0, 5.0)), "to the south");
        assert_eq!(compass(o, Vec2::new(-5.0, 0.2)), "to the west");
        assert_eq!(compass(o, Vec2::new(3.0, 3.0)), "to the south-east");
        assert_eq!(compass(o, Vec2::new(-3.0, -3.2)), "to the north-west");
        assert_eq!(compass(o, Vec2::new(0.1, 0.1)), "close by");
    }

    #[test]
    fn bubbles_stay_longer_for_longer_lines() {
        assert!(bubble_secs("hi") < bubble_secs("a much longer line to read through"));
        assert_eq!(bubble_secs(&"x".repeat(500)), 8.0);
    }

    #[test]
    fn the_volume_goes_round_and_the_script_is_in_order() {
        assert_eq!(next_volume(next_volume(next_volume(Volume::Talk))), Volume::Talk);
        assert!(SCRIPT.windows(2).all(|w| w[0].0 <= w[1].0) && SCRIPT.last().unwrap().0 < ROUND_SECS);
        assert!(step_due(0.5, 0).is_none());
        assert!(matches!(step_due(1.1, 0), Some(Step::Gesture(Some(Gesture::Wave)))));
        assert!(step_due(1.1, 1).is_none() && step_due(100.0, SCRIPT.len()).is_none());
    }

    /// The talk demo's survivor speaks and gestures through intents alone, and we hear it.
    #[test]
    fn the_talk_demo_talks_through_intents() {
        let (mut link, me) = ServerLink::for_test();
        let world = link.world().unwrap();
        let at = world.player(me).unwrap().state.pos;
        let other = link.join_for_test((at + Vec2::new(1.0, 0.0)).to_array());
        let mut heard = Vec::new();
        let mut gestures = Vec::new();
        let mut next = 0;
        for t in 0..(ROUND_SECS as u32 * zombye_sim::TICK_RATE) {
            let secs = t as f32 * zombye_sim::TICK_DT;
            while let Some(step) = step_due(secs, next) {
                next += 1;
                link.send(match step {
                    Step::Say(volume, text) => Intent::Say { player: other, text: (*text).into(), volume: *volume },
                    Step::Gesture(gesture) => Intent::Gesture { player: other, gesture: *gesture },
                });
            }
            let snap = link.tick_for_test(me);
            heard.extend(snap.speech.iter().filter(|s| s.listener == me).map(|s| (s.volume, s.text.clone())));
            if let Some(g) = snap.players.iter().find(|p| p.id == other).and_then(|p| p.gesture)
                && gestures.last() != Some(&g)
            {
                gestures.push(g);
            }
        }
        assert_eq!(heard.len(), SCRIPT.iter().filter(|(_, s)| matches!(s, Step::Say(..))).count(), "{heard:?}");
        assert!(heard.iter().any(|(v, t)| *v == Volume::Shout && t == "BEHIND YOU! RUN!"));
        assert_eq!(gestures, [Gesture::Wave, Gesture::HandsUp, Gesture::Point]);
    }
}
