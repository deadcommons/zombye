//! The local player: a survivor simulated on the server, predicted on the client.
//!
//! Every fixed tick the keys (WASD relative to the camera, Shift run, C toggles sneak, the right mouse
//! held to aim) and the facing become a numbered [`PlayerInput`]. Not aiming, the survivor faces the
//! way it walks; aiming (right mouse), it faces the mouse cursor and strafes or backs off, slower and
//! unable to run, and only then can it attack, shove or stomp. It is sent to the server as [`Intent::PlayerInput`] and
//! applied locally with the same [`step_movement`] the server runs (prediction). When a snapshot
//! arrives, the server's state for our player replaces the prediction and the inputs the server hasn't
//! applied yet are replayed on top (reconciliation). The body is drawn interpolated between the last
//! two predicted states. Other players are drawn from snapshots, interpolated like zombies.
//!
//! Keys: WASD move, Shift run, C sneak, right mouse (held) aim, and while aiming left mouse swing the
//! weapon you hold (or punch) and Space shove (or stomp, when a downed zombie lies right in front of you), E pick up the item you face (or, with
//! nothing on the floor within reach, search the cupboard, drawer or fridge you face), Tab or I the
//! inventory panel (`inventory_ui`; the left mouse button doesn't swing while it's open),
//! F open/close the door or window you face, G smash the window you face, T skip an hour, Z spawn zombies, F1 god mode and F2 ignored by zombies (dev toggles, sent as
//! `Intent::Dev`; the HUD shows GOD / IGNORED from the snapshot). Climbing through an open or broken window is just
//! walking into it (slowly, predicted like any other movement). Combat itself is all on the server;
//! the client only sends the intents and draws what the snapshots say (the held weapon, a swing,
//! lying dead). The body is a [`crate::survivor`] rig.

use std::collections::VecDeque;

use bevy::app::RunFixedMainLoopSystems;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use zombye_net::Intent;
use zombye_sim::players::step_movement;
use zombye_sim::{ActionKind, DevFlags, PlayerInput, PlayerState, TICK_DT, ZombieAnim};

use crate::autoplay::AutoInput;
use crate::camera::IsoCamera;
use crate::fog::FogState;
use crate::server_link::{ClientMap, JoinInfo, ServerLink, ServerTick};
use crate::survivor::{self, SurvivorAssets};
use crate::survivor_model::SurvivorModel;
use crate::zombie_render::lerp_angle;

/// Unconfirmed inputs kept for replay; older ones are dropped (the server is far behind or gone).
const MAX_PENDING: usize = 256;

/// The local player's body. Its `Transform` is the interpolated predicted position.
#[derive(Component)]
pub struct Player {
    /// Facing angle in sim convention: 0 = +x, counter-clockwise toward +y (= world +z).
    pub facing: f32,
}

/// Another player, drawn from snapshots.
#[derive(Component)]
struct OtherPlayer(u32);

/// Prediction state for the player this client controls.
#[derive(Resource)]
pub struct LocalPlayer {
    pub id: u32,
    seq: u32,
    /// Inputs sent but not yet confirmed by a snapshot, oldest first.
    pending: VecDeque<(u32, PlayerInput)>,
    /// Predicted state after the previous and the latest tick, for interpolation.
    pub prev: PlayerState,
    pub curr: PlayerState,
    /// Sneak is a toggle (C).
    pub sneak: bool,
    /// Facing toward the mouse cursor (or chosen by `--autoplay`), sent with the next input while aiming.
    pub aim: f32,
    /// Aiming: the right mouse held (not over an open panel), or `--autoplay`.
    pub aiming: bool,
    /// How far up or down of level a gun is aimed (#72, radians): at the body the cursor is on, at
    /// the height it is on it (`gun_aim`), else level at chest height. Sent as `Intent::Aim`; the
    /// last one sent is kept so a gun put away says "level" once.
    pub pitch: f32,
    sent_pitch: f32,
    last_snapshot_tick: u64,
    /// From the latest snapshot: health, dead, weapon held, action under way.
    pub hp: f32,
    pub dead: bool,
    pub weapon: Option<u16>,
    pub action: Option<ActionKind>,
    /// Dev toggles the server has on for us (god mode, ignored by zombies).
    pub dev: DevFlags,
    /// On a multiplayer server: our latest swing, shove or stomp, begun on the click rather than
    /// when the server's word of it comes back (`LocalPlayer::act`; `survivor.rs` plays it).
    pub swing: Option<OwnSwing>,
}

/// An action of our own begun on the click, on a multiplayer server (#23). Our survivor is
/// predicted, so its blow lands on screen where the server will have it land; the server tests the
/// targets where our picture had them then (lag compensation), so what we see hit is what is hit.
#[derive(Clone, Copy, Debug)]
pub struct OwnSwing {
    pub kind: ActionKind,
    /// The input that follows it: a snapshot that has applied this one knows of the action.
    pub seq: u32,
    /// When it began (`Time::elapsed_secs`), and our estimate of its windup and whole length.
    pub at: f32,
    pub windup: f32,
    pub total: f32,
}

impl LocalPlayer {
    /// How far behind our own survivor the picture of everything else is, in seconds: the inputs
    /// we have sent that `view` (our player in the snapshot shown) hasn't applied yet. 0 in
    /// single-player, where the world is in step with us.
    pub fn view_lag(&self, link: &ServerLink, view: &zombye_net::PlayerView) -> f32 {
        if link.net.is_none() {
            return 0.0;
        }
        self.seq.saturating_sub(view.last_input_seq).min(40) as f32 * TICK_DT
    }

    /// Swing, shove or stomp: ask the server, and on a multiplayer server begin it here and now
    /// (when it looks as if the server will take it) instead of a round trip later.
    pub fn act(&mut self, link: &mut ServerLink, join: &JoinInfo, kind: ActionKind, now: f32) {
        link.send(match kind {
            ActionKind::Swing => Intent::Attack { player: self.id },
            ActionKind::Shove => Intent::Shove { player: self.id },
            ActionKind::Stomp => Intent::Stomp { player: self.id },
        });
        if link.net.is_none() || self.dead || !self.aiming || self.acting(link, now) {
            return;
        }
        let Some(view) = link.curr.players.iter().find(|p| p.id == self.id) else { return };
        if view.asleep || view.knock.is_some() {
            return;
        }
        let c = &join.combat;
        let (windup, total) = match kind {
            ActionKind::Swing => {
                let unarmed = join.weapons.iter().position(|w| w.category == zombye_sim::WeaponCategory::Unarmed).unwrap_or(0);
                let w = &join.weapons[self.weapon.map_or(unarmed, |w| w as usize).min(join.weapons.len().saturating_sub(1))];
                (w.windup_seconds, w.swing_seconds)
            }
            ActionKind::Shove => (c.shove_windup, c.shove_seconds),
            ActionKind::Stomp => (c.stomp_windup, c.stomp_seconds),
        };
        // Tired arms are slower (the server's rule; wounds and skill it adds are corrected when
        // its word comes back).
        let slow = c.fatigue(self.curr.endurance).1;
        self.swing = Some(OwnSwing { kind, seq: self.seq + 1, at: now, windup: windup * slow, total: total * slow });
    }

    /// Whether an action of ours is under way or we are still recovering from one, as far as we
    /// can tell: our own latest (multiplayer), or what the snapshot says once it knows of that one.
    pub fn acting(&self, link: &ServerLink, now: f32) -> bool {
        let view = link.curr.players.iter().find(|p| p.id == self.id);
        let own = self.swing.filter(|_| link.net.is_some());
        if own.is_some_and(|s| now < s.at + s.total) {
            return true;
        }
        // A snapshot from before our latest click says nothing new.
        if own.is_some_and(|s| view.is_some_and(|v| v.last_input_seq < s.seq)) {
            return false;
        }
        view.is_none_or(|v| v.action.is_some() || v.ready_in - self.view_lag(link, v) > 0.0)
    }

    /// Start again from a loaded world's view of our player: forget unconfirmed inputs (they were
    /// for the old world) and keep numbering inputs above anything the save has seen.
    pub fn resume(&mut self, view: &zombye_net::PlayerView, tick: u64) {
        self.id = view.id;
        self.seq = self.seq.max(view.last_input_seq);
        self.pending.clear();
        (self.prev, self.curr) = (view.state(), view.state());
        self.aim = view.facing;
        self.last_snapshot_tick = tick;
        self.swing = None;
        (self.hp, self.dead, self.weapon, self.action, self.dev) = (view.hp, view.dead, view.weapon, view.action, view.dev);
    }
}

/// Builds the local player's input each tick; `--autoplay` runs before it.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct PlayerInputSet;

pub struct PlayerPlugin;

impl Plugin for PlayerPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_player)
            .add_systems(Update, (toggle_sneak, face_cursor, actions))
            .add_systems(FixedUpdate, (predict.in_set(PlayerInputSet).before(ServerTick).run_if(crate::live), reconcile.after(ServerTick)))
            // After the fixed ticks of this frame, before anything in Update reads the player's transform.
            .add_systems(RunFixedMainLoop, (place_player, sync_other_players).in_set(RunFixedMainLoopSystems::AfterFixedMainLoop))
            .add_systems(PostUpdate, lie_on_bed.before(bevy::transform::TransformSystems::Propagate));
    }
}

fn spawn_player(
    mut commands: Commands,
    join: Res<JoinInfo>,
    link: Res<ServerLink>,
    options: Res<crate::Options>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut bindposes: ResMut<Assets<bevy::mesh::skinning::SkinnedMeshInverseBindposes>>,
    mut images: ResMut<Assets<Image>>,
) {
    let view = link.curr.players.iter().find(|p| p.id == join.player_id);
    let state = view.map(|v| v.state()).unwrap_or_else(|| PlayerState::new(join.player_spawn));
    // From the first snapshot too (`reconcile` only takes them from later ones; offscreen there are none).
    let (hp, dead, weapon, action, dev) = view.map_or((100.0, false, None, None, DevFlags::default()), |v| (v.hp, v.dead, v.weapon, v.action, v.dev));
    commands.insert_resource(LocalPlayer {
        id: join.player_id,
        // Above anything the server has taken from this player (a survivor taken up again on a
        // multiplayer server, or from a save): it ignores input numbered at or below that.
        seq: view.map_or(0, |v| v.last_input_seq),
        pending: VecDeque::new(),
        prev: state,
        curr: state,
        sneak: options.sneak,
        aim: state.facing,
        aiming: false,
        pitch: 0.0,
        sent_pitch: 0.0,
        last_snapshot_tick: link.curr.tick,
        hp,
        dead,
        weapon,
        action,
        dev,
        swing: None,
    });

    let assets = SurvivorAssets::new(&mut meshes, &mut materials, &mut bindposes);
    let model = SurvivorModel::load(&options, &mut meshes, &mut materials, &mut images, &mut bindposes);
    let p = state.pos;
    let yaw = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2 - state.facing);
    let me = commands.spawn((Player { facing: state.facing }, Transform::from_xyz(p.x, 0.0, p.y).with_rotation(yaw), Visibility::default())).id();
    survivor::attach(&mut commands, &assets, model.as_ref(), me, join.player_id, true);
    commands.insert_resource(assets);
    if let Some(model) = model {
        commands.insert_resource(model);
    }
}

fn toggle_sneak(keys: Res<ButtonInput<KeyCode>>, mut me: ResMut<LocalPlayer>) {
    if keys.just_pressed(KeyCode::KeyC) {
        me.sneak = !me.sneak;
    }
}

/// One fixed tick of input: send it, and predict its effect with the shared movement code.
fn predict(
    keys: Res<ButtonInput<KeyCode>>,
    map: Res<ClientMap>,
    auto: Res<AutoInput>,
    cams: Query<&IsoCamera>,
    mut me: ResMut<LocalPlayer>,
    mut link: ResMut<ServerLink>,
) {
    let mut dir = Vec3::ZERO;
    if let Ok(cam) = cams.single() {
        let (forward, right) = cam.ground_axes();
        for (key, d) in [(KeyCode::KeyW, forward), (KeyCode::KeyS, -forward), (KeyCode::KeyD, right), (KeyCode::KeyA, -right)] {
            if keys.pressed(key) {
                dir += d;
            }
        }
    }
    let mut run = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    let mut move_dir = Vec2::new(dir.x, dir.z).normalize_or_zero();
    if auto.active {
        (move_dir, run) = (auto.move_dir, auto.run);
    }
    // Aiming, face the aim; otherwise the way we walk (standing still, as we were).
    let facing = if me.aiming {
        me.aim
    } else if move_dir != Vec2::ZERO {
        move_dir.y.atan2(move_dir.x)
    } else {
        me.curr.facing
    };
    if !me.aiming {
        me.aim = facing;
    }
    let input = PlayerInput { move_dir, facing, run, sneak: me.sneak, aim: me.aiming };
    if me.dead {
        // The server ignores a dead player's input; nothing to predict.
        return;
    }
    me.seq += 1;
    let seq = me.seq;
    if link.net.is_some() {
        // On a server: say which snapshot we are showing as this input goes, so our blows land
        // where we see their targets (lag compensation, #23).
        let tick = link.curr.tick;
        link.send(Intent::Seen { player: me.id, tick });
    }
    // Aiming a gun (#72): the pitch goes with the facing, before the input a trigger pull follows.
    let pitch = if me.aiming { me.pitch } else { 0.0 };
    if pitch != 0.0 || me.sent_pitch != 0.0 {
        link.send(Intent::Aim { player: me.id, pitch });
        me.sent_pitch = pitch;
    }
    link.send(Intent::PlayerInput { player: me.id, seq, move_dir: input.move_dir.to_array(), facing: input.facing, run, sneak: input.sneak, aim: input.aim });

    if me.pending.len() >= MAX_PENDING {
        me.pending.pop_front();
    }
    me.pending.push_back((seq, input));
    me.prev = me.curr;
    let mut s = me.curr;
    step_movement(&map.0, &mut s, &input, TICK_DT);
    me.curr = s;
}

/// On a new snapshot: start from the server's state and replay the inputs it hasn't applied yet.
fn reconcile(link: Res<ServerLink>, map: Res<ClientMap>, mut me: ResMut<LocalPlayer>) {
    if link.curr.tick == me.last_snapshot_tick {
        return;
    }
    let me = &mut *me;
    me.last_snapshot_tick = link.curr.tick;
    let Some(view) = link.curr.players.iter().find(|p| p.id == me.id) else { return };
    (me.hp, me.dead, me.weapon, me.action, me.dev) = (view.hp, view.dead, view.weapon, view.action, view.dev);
    if me.dead {
        // Lying where we fell (or just respawned): take the server's word for it.
        me.pending.clear();
        me.prev = view.state();
        me.curr = view.state();
        return;
    }
    while me.pending.front().is_some_and(|(seq, _)| *seq <= view.last_input_seq) {
        me.pending.pop_front();
    }
    let mut s = view.state();
    for (_, input) in &me.pending {
        step_movement(&map.0, &mut s, input, TICK_DT);
    }
    me.curr = s;
}

fn place_player(fixed: Res<Time<Fixed>>, me: Res<LocalPlayer>, mut players: Query<&mut Transform, With<Player>>) {
    let Ok(mut tf) = players.single_mut() else { return };
    let p = me.prev.pos.lerp(me.curr.pos, fixed.overstep_fraction());
    tf.translation.x = p.x;
    tf.translation.z = p.y;
}

/// How high a sleeper lies on a bed or a sofa, and how far from the middle of it its feet are (its
/// body lies back from them, see the fall onto the back in `survivor.rs`).
const BED_TOP: f32 = 0.56;
const SOFA_TOP: f32 = 0.45;
const BED_FEET: f32 = 0.85;

/// Players drawn: ours and the others.
type Bodies<'w, 's> = Query<'w, 's, (&'static mut Transform, Option<&'static OtherPlayer>), Or<(With<Player>, With<OtherPlayer>)>>;

/// A sleeper on a bed or sofa (#58) is drawn lying on it, feet to its front, rather than on the
/// floor where it stands beside it: the sim keeps its place, this only moves the body drawn.
fn lie_on_bed(
    link: Res<ServerLink>,
    map: Res<ClientMap>,
    me: Res<LocalPlayer>,
    mut players: Bodies,
) {
    for (mut tf, other) in &mut players {
        let id = other.map_or(me.id, |o| o.0);
        let on = link.curr.players.iter().find(|p| p.id == id).filter(|v| v.asleep && !v.dead).and_then(|v| zombye_sim::needs::sleep_furniture(&map.0, Vec2::from(v.pos)));
        let Some((f, spot)) = on else {
            // Up again: back on the floor (`place_player` only sets our x and z).
            if tf.translation.y != 0.0 {
                tf.translation.y = 0.0;
            }
            continue;
        };
        let centre = f.min.as_vec2() + f.size.as_vec2() * 0.5;
        let front = f.facing.offset().as_vec2();
        let feet = centre + front * BED_FEET;
        let top = if spot == zombye_sim::needs::SleepSpot::Bed { BED_TOP } else { SOFA_TOP };
        tf.translation = Vec3::new(feet.x, top, feet.y);
        tf.rotation = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2 - front.y.atan2(front.x));
    }
}

/// Mirror other players from snapshots, interpolated between the last two.
fn sync_other_players(
    mut commands: Commands,
    link: Res<ServerLink>,
    me: Res<LocalPlayer>,
    fog: Res<FogState>,
    fixed: Res<Time<Fixed>>,
    assets: Option<Res<SurvivorAssets>>,
    model: Option<Res<SurvivorModel>>,
    mut others: Query<(Entity, &OtherPlayer, &mut Transform, &mut Visibility)>,
) {
    let Some(assets) = assets else { return };
    let alpha = fixed.overstep_fraction();
    let mut seen = Vec::new();
    for (e, other, mut tf, mut vis) in &mut others {
        let Some(v) = link.curr.players.iter().find(|p| p.id == other.0) else {
            commands.entity(e).despawn();
            continue;
        };
        let prev = link.prev.players.iter().find(|p| p.id == v.id).unwrap_or(v);
        let pos = Vec2::from(prev.pos).lerp(Vec2::from(v.pos), alpha);
        tf.translation = Vec3::new(pos.x, 0.0, pos.y);
        tf.rotation = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2 - lerp_angle(prev.facing, v.facing, alpha));
        *vis = if fog.visibility(pos) > 0.35 { Visibility::Inherited } else { Visibility::Hidden };
        seen.push(v.id);
    }
    for v in link.curr.players.iter().filter(|p| p.id != me.id && !seen.contains(&p.id)) {
        let tf = Transform::from_xyz(v.pos[0], 0.0, v.pos[1]).with_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2 - v.facing));
        let e = commands.spawn((OtherPlayer(v.id), tf, Visibility::default())).id();
        survivor::attach(&mut commands, &assets, model.as_deref(), e, v.id, false);
    }
}

/// Aiming (the right mouse held), turn to face the point under the mouse cursor on the ground;
/// otherwise turn (smoothly) to the way we walk.
#[allow(clippy::too_many_arguments)]
fn face_cursor(
    windows: Query<&Window, With<PrimaryWindow>>,
    cams: Query<(&Camera, &GlobalTransform), With<IsoCamera>>,
    auto: Res<AutoInput>,
    mouse: Res<ButtonInput<MouseButton>>,
    panel: Res<crate::inventory_ui::InventoryPanel>,
    health: Res<crate::health_ui::HealthPanel>,
    time: Res<Time>,
    join: Res<JoinInfo>,
    link: Res<ServerLink>,
    mut players: Query<(&mut Player, &mut Transform)>,
    mut me: ResMut<LocalPlayer>,
) {
    me.aiming = auto.active || (mouse.pressed(MouseButton::Right) && !panel.open && !health.open);
    if me.dead {
        return;
    }
    let Ok((mut player, mut tf)) = players.single_mut() else { return };
    if !me.aiming {
        let target = me.curr.facing;
        let turn = (target - player.facing + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
        player.facing += turn * (1.0 - (-14.0 * time.delta_secs()).exp());
        tf.rotation = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2 - player.facing);
        return;
    }
    if auto.active {
        // `--autoplay` aims for us.
        player.facing = me.aim;
        tf.rotation = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2 - player.facing);
        return;
    }
    let (Ok(window), Ok((camera, cam_tf))) = (windows.single(), cams.single()) else { return };
    let Some(cursor) = window.cursor_position() else { return };
    let Ok(ray) = camera.viewport_to_world(cam_tf, crate::render_scale::to_view(camera, window, cursor)) else { return };
    if ray.direction.y.abs() < 1e-4 {
        return;
    }
    let t = -ray.origin.y / ray.direction.y;
    let hit = ray.origin + *ray.direction * t;
    let from = Vec2::new(tf.translation.x, tf.translation.z);
    let mut d = Vec2::new(hit.x, hit.z) - from;
    // With a gun (#72) the aim is the body the cursor is on, at the height it is on it: the head
    // when it's on the head. On nothing, level at chest height toward the ground under the cursor.
    if me.weapon.and_then(|w| join.weapons.get(w as usize)).is_some_and(|w| w.ranged.is_some()) {
        let zombies = link.curr.zombies.iter().filter(|z| z.anim != ZombieAnim::Dead).map(|z| (Vec2::from(z.pos), matches!(z.anim, ZombieAnim::Down | ZombieAnim::GetUp)));
        let others = link.curr.players.iter().filter(|p| p.id != me.id && !p.dead).map(|p| (Vec2::from(p.pos), p.asleep || p.knock.is_some_and(|k| k.down)));
        match gun_aim(from, ray.origin, *ray.direction, zombies.chain(others)) {
            Some((to, pitch)) => (d, me.pitch) = (to, pitch),
            None => me.pitch = level_pitch(d.length()),
        }
    } else {
        me.pitch = 0.0;
    }
    if d.length_squared() > 0.04 {
        player.facing = d.y.atan2(d.x);
        me.aim = player.facing;
        // The body's nose points along local +z.
        tf.rotation = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2 - player.facing);
    }
}

/// How far the cursor's ray may pass from a body's middle and still be on it (a little more than
/// the body, so a zombie is easy to pick).
const AIM_PICK: f32 = 0.42;
/// With the cursor on nothing a gun is held level at this height (the chest) at the cursor's distance.
const CHEST: f32 = 1.15;

/// The pitch that puts a shot [`CHEST`] high `dist` metres off.
pub fn level_pitch(dist: f32) -> f32 {
    ((CHEST - zombye_sim::ranged::MUZZLE_HEIGHT) / dist.max(2.0)).atan()
}

/// What a gun held at `from` is aimed at when the cursor's ray (`origin`, unit `dir`, in the
/// world: y up) is on one of `bodies` (where each stands, and whether it lies down): the way to
/// that body over the ground, and the pitch to the height on it the ray passes nearest (the head,
/// with the cursor on its head). `None`: the cursor is on nobody.
pub fn gun_aim(from: Vec2, origin: Vec3, dir: Vec3, bodies: impl Iterator<Item = (Vec2, bool)>) -> Option<(Vec2, f32)> {
    use zombye_sim::ranged::{BODY_HEIGHT, DOWN_HEIGHT, MUZZLE_HEIGHT};
    let denom = 1.0 - dir.y * dir.y;
    if denom < 1e-6 {
        return None;
    }
    let mut best: Option<(f32, Vec2, f32, f32)> = None;
    for (pos, down) in bodies {
        let top = if down { DOWN_HEIGHT } else { BODY_HEIGHT };
        // The nearest points of the ray and the body's upright middle line.
        let w = origin - Vec3::new(pos.x, 0.0, pos.y);
        let (b, d, e) = (dir.y, dir.dot(w), w.y);
        let s = ((e - b * d) / denom).clamp(0.0, top);
        let on_body = Vec3::new(pos.x, s, pos.y);
        let t = (on_body - origin).dot(dir);
        let miss = (origin + dir * t).distance(on_body);
        if miss <= AIM_PICK && best.is_none_or(|(m, ..)| miss < m) {
            best = Some((miss, pos, s, top));
        }
    }
    let (_, pos, s, top) = best?;
    let to = pos - from;
    // Not at the very top of the head or the soles of the feet.
    let height = s.clamp(0.15, top - 0.12);
    Some((to, ((height - MUZZLE_HEIGHT) / to.length().max(0.5)).atan()))
}

#[allow(clippy::too_many_arguments)]
fn actions(
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    panel: Res<crate::inventory_ui::InventoryPanel>,
    health: Res<crate::health_ui::HealthPanel>,
    preview: Res<crate::dev_time::LightPreview>,
    morning: Option<Res<crate::firstnight_ui::FirstNightUi>>,
    time: Res<Time>,
    mut me: ResMut<LocalPlayer>,
    join: Res<JoinInfo>,
    players: Query<(&Transform, &Player)>,
    mut link: ResMut<ServerLink>,
) {
    let now = time.elapsed_secs();
    // Dev toggles: flip what the server last said.
    if keys.just_pressed(KeyCode::F1) {
        link.send(Intent::Dev { player: me.id, god: Some(!me.dev.god), ignored: None });
    }
    if keys.just_pressed(KeyCode::F2) {
        link.send(Intent::Dev { player: me.id, god: None, ignored: Some(!me.dev.ignored) });
    }
    // Dead: the death screen (`death.rs`) takes R (a new run) and Q / Esc (quit).
    if me.dead {
        return;
    }
    // B lies down to sleep or gets up; asleep, setting off (WASD) gets up too (#11). Anything
    // else done asleep gets up on the server.
    let asleep = link.curr.players.iter().any(|p| p.id == me.id && p.asleep);
    let moving = keys.any_just_pressed([KeyCode::KeyW, KeyCode::KeyA, KeyCode::KeyS, KeyCode::KeyD]);
    if keys.just_pressed(KeyCode::KeyB) || (asleep && moving) {
        link.send(Intent::Sleep { player: me.id, sleep: !asleep });
    }
    // Fighting is for aiming (right mouse held); the server refuses it otherwise.
    if me.aiming && mouse.just_pressed(MouseButton::Left) && !panel.open && !health.open && !preview.hovered {
        me.act(&mut link, &join, ActionKind::Swing, now);
    }
    if me.aiming && keys.just_pressed(KeyCode::Space) {
        // Stomp if a downed zombie lies right in front, else shove (the server decides what it hits).
        let c = &join.combat;
        let forward = Vec2::from_angle(me.aim);
        let half_arc = (c.stomp_arc_degrees * 0.5).to_radians().cos();
        let downed = link.curr.zombies.iter().any(|z| {
            let to = Vec2::from(z.pos) - me.curr.pos;
            matches!(z.anim, ZombieAnim::Down | ZombieAnim::GetUp) && to.length() <= c.stomp_range && forward.dot(to.normalize_or_zero()) >= half_arc
        });
        me.act(&mut link, &join, if downed { ActionKind::Stomp } else { ActionKind::Shove }, now);
    }
    // R reloads the gun in hand (#72): the ordinary use of it. (Dead, R is the death screen's;
    // on the morning screen, a new run.)
    let gun = me.weapon.and_then(|w| join.weapons.get(w as usize)).is_some_and(|w| w.ranged.is_some());
    if gun && keys.just_pressed(KeyCode::KeyR) && !morning.is_some_and(|m| m.morning) {
        link.send(Intent::Use { player: me.id, from: zombye_sim::Place::Slot(zombye_sim::Slot::Hand), index: 0 });
    }
    if keys.just_pressed(KeyCode::KeyE) {
        // Something on our floor (the snapshot's, by the server's reach rule): pick it up.
        // Otherwise search what we face; with nothing there either, the server says so once.
        let floor = !link.curr.floor_of(me.id).is_empty();
        link.send(if floor { Intent::PickUp { player: me.id } } else { Intent::Search { player: me.id, container: None } });
    }
    if keys.just_pressed(KeyCode::KeyF) {
        // The server picks the door or window this player faces and checks it's within reach.
        link.send(Intent::Interact { player: me.id });
    }
    if keys.just_pressed(KeyCode::KeyG) {
        link.send(Intent::SmashWindow { player: me.id });
    }
    if keys.just_pressed(KeyCode::KeyT) {
        link.send(Intent::SkipTime { hours: 1.0 });
    }
    let Ok((tf, player)) = players.single() else { return };
    if keys.just_pressed(KeyCode::KeyZ) {
        let p = Vec2::new(tf.translation.x, tf.translation.z);
        let center = p + Vec2::from_angle(player.facing) * 30.0;
        link.send(Intent::SpawnZombies { count: 250, center: center.to_array(), radius: 8.0 });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zombye_sim::ranged::MUZZLE_HEIGHT;

    /// With a gun the aim is the body the cursor is on, at the height it is on it (#72): on the
    /// head it is a head shot, at the feet a low one, beside the body nobody.
    #[test]
    fn a_gun_aims_at_what_the_cursor_is_on() {
        // The isometric camera looks down at the ground from far off, across the map's diagonal.
        let dir = Vec3::new(0.61, -0.5, 0.61).normalize();
        let zombie = Vec2::new(10.0, 0.0);
        // The cursor's ray through the point `up` metres up a body standing at `at`, `beside` off it.
        let ray = |at: Vec2, up: f32, beside: f32| Vec3::new(at.x, up, at.y) + Vec3::new(dir.z, 0.0, -dir.x).normalize() * beside - dir * 60.0;
        // The height a shot with that pitch passes at the body.
        let height = |aim: Option<(Vec2, f32)>| aim.map(|(to, pitch)| MUZZLE_HEIGHT + to.length() * pitch.tan());
        let at = |up: f32, beside: f32, down: bool| gun_aim(Vec2::ZERO, ray(zombie, up, beside), dir, [(zombie, down)].into_iter());
        let (to, _) = at(1.2, 0.0, false).expect("on the chest");
        assert!(to.distance(zombie) < 1e-4, "it faces the body, not the ground behind it under the cursor: {to}");
        assert!((height(at(1.2, 0.0, false)).unwrap() - 1.2).abs() < 0.02);
        assert!((height(at(1.62, 0.1, false)).unwrap() - 1.62).abs() < 0.02, "the cursor on its head: the head");
        assert!((height(at(0.0, 0.0, false)).unwrap() - 0.15).abs() < 0.02, "at its feet: low, not into the ground");
        assert!(height(at(1.8, 0.0, false)).unwrap() <= 1.69, "the top of the head: still on the head");
        assert_eq!(at(1.0, 0.8, false), None, "beside it: nobody");
        // One lying down: low, wherever on it the cursor is.
        assert!((0.14..=0.3).contains(&height(at(0.2, 0.2, true)).unwrap()));
        assert_eq!(at(1.4, 0.0, true), None, "the air over a body on the ground");
        // Two close together: the one the cursor is nearer.
        let other = Vec2::new(10.3, 0.0);
        let (to, _) = gun_aim(Vec2::ZERO, ray(other, 1.0, 0.0), dir, [(zombie, false), (other, false)].into_iter()).unwrap();
        assert!(to.distance(other) < 1e-4);
        // On nothing: level at chest height at the cursor's distance, never steeply down close in.
        assert!((MUZZLE_HEIGHT + 12.0 * level_pitch(12.0).tan() - CHEST).abs() < 1e-3);
        assert_eq!(level_pitch(0.1), level_pitch(2.0));
    }
}
