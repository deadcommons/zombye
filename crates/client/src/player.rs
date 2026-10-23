//! The local player: a survivor simulated on the server, predicted on the client.
//!
//! Every fixed tick the keys (WASD relative to the camera, Shift run, C toggles sneak) and the mouse
//! facing become a numbered [`PlayerInput`]. It is sent to the server as [`Intent::PlayerInput`] and
//! applied locally with the same [`step_movement`] the server runs (prediction). When a snapshot
//! arrives, the server's state for our player replaces the prediction and the inputs the server hasn't
//! applied yet are replayed on top (reconciliation). The body is drawn interpolated between the last
//! two predicted states. Other players are drawn from snapshots, interpolated like zombies.
//!
//! Keys: WASD move, Shift run, C sneak, left mouse swing the weapon you hold (or punch), Space shove
//! (or stomp, when a downed zombie lies right in front of you), E pick up the weapon you face,
//! F open/close the door or window you face, G smash the window you face, R get up again after
//! dying, T skip an hour, Z spawn zombies. Climbing through an open or broken window is just
//! walking into it (slowly, predicted like any other movement). Combat itself is all on the server;
//! the client only sends the intents and draws what the snapshots say (the held weapon, a swing,
//! lying dead). The body is a [`crate::survivor`] rig.

use std::collections::VecDeque;

use bevy::app::RunFixedMainLoopSystems;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use zombye_net::Intent;
use zombye_sim::players::step_movement;
use zombye_sim::{ActionKind, PlayerInput, PlayerState, TICK_DT, ZombieAnim};

use crate::autoplay::AutoInput;
use crate::camera::IsoCamera;
use crate::fog::FogState;
use crate::server_link::{ClientMap, JoinInfo, ServerLink, ServerTick};
use crate::survivor::{self, SurvivorAssets};
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
    /// Facing toward the mouse cursor (or chosen by `--autoplay`), sent with the next input.
    pub aim: f32,
    last_snapshot_tick: u64,
    /// From the latest snapshot: health, dead, weapon held, action under way.
    pub hp: f32,
    pub dead: bool,
    pub weapon: Option<u16>,
    pub action: Option<ActionKind>,
}

impl LocalPlayer {
    /// Start again from a loaded world's view of our player: forget unconfirmed inputs (they were
    /// for the old world) and keep numbering inputs above anything the save has seen.
    pub fn resume(&mut self, view: &zombye_net::PlayerView, tick: u64) {
        self.id = view.id;
        self.seq = self.seq.max(view.last_input_seq);
        self.pending.clear();
        (self.prev, self.curr) = (view.state(), view.state());
        self.aim = view.facing;
        self.last_snapshot_tick = tick;
        (self.hp, self.dead, self.weapon, self.action) = (view.hp, view.dead, view.weapon, view.action);
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
            .add_systems(FixedUpdate, (predict.in_set(PlayerInputSet).before(ServerTick), reconcile.after(ServerTick)))
            // After the fixed ticks of this frame, before anything in Update reads the player's transform.
            .add_systems(RunFixedMainLoop, (place_player, sync_other_players).in_set(RunFixedMainLoopSystems::AfterFixedMainLoop));
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
) {
    let state = link.curr.players.iter().find(|p| p.id == join.player_id).map(|v| v.state()).unwrap_or_else(|| PlayerState::new(join.player_spawn));
    commands.insert_resource(LocalPlayer {
        id: join.player_id,
        seq: 0,
        pending: VecDeque::new(),
        prev: state,
        curr: state,
        sneak: options.sneak,
        aim: state.facing,
        last_snapshot_tick: link.curr.tick,
        hp: 100.0,
        dead: false,
        weapon: None,
        action: None,
    });

    let assets = SurvivorAssets::new(&mut meshes, &mut materials, &mut bindposes);
    let p = state.pos;
    let yaw = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2 - state.facing);
    let me = commands.spawn((Player { facing: state.facing }, Transform::from_xyz(p.x, 0.0, p.y).with_rotation(yaw), Visibility::default())).id();
    survivor::attach(&mut commands, &assets, me, join.player_id, true);
    commands.insert_resource(assets);
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
    let input = PlayerInput { move_dir, facing: me.aim, run, sneak: me.sneak };
    if me.dead {
        // The server ignores a dead player's input; nothing to predict.
        return;
    }
    me.seq += 1;
    let seq = me.seq;
    link.send(Intent::PlayerInput { player: me.id, seq, move_dir: input.move_dir.to_array(), facing: input.facing, run, sneak: input.sneak });

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
    (me.hp, me.dead, me.weapon, me.action) = (view.hp, view.dead, view.weapon, view.action);
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

/// Mirror other players from snapshots, interpolated between the last two.
fn sync_other_players(
    mut commands: Commands,
    link: Res<ServerLink>,
    me: Res<LocalPlayer>,
    fog: Res<FogState>,
    fixed: Res<Time<Fixed>>,
    assets: Option<Res<SurvivorAssets>>,
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
        survivor::attach(&mut commands, &assets, e, v.id, false);
    }
}

/// Turn to face the point under the mouse cursor on the ground.
fn face_cursor(
    windows: Query<&Window, With<PrimaryWindow>>,
    cams: Query<(&Camera, &GlobalTransform), With<IsoCamera>>,
    auto: Res<AutoInput>,
    mut players: Query<(&mut Player, &mut Transform)>,
    mut me: ResMut<LocalPlayer>,
) {
    if me.dead {
        return;
    }
    let Ok((mut player, mut tf)) = players.single_mut() else { return };
    if auto.active {
        // `--autoplay` aims for us.
        player.facing = me.aim;
        tf.rotation = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2 - player.facing);
        return;
    }
    let (Ok(window), Ok((camera, cam_tf))) = (windows.single(), cams.single()) else { return };
    let Some(cursor) = window.cursor_position() else { return };
    let Ok(ray) = camera.viewport_to_world(cam_tf, cursor) else { return };
    if ray.direction.y.abs() < 1e-4 {
        return;
    }
    let t = -ray.origin.y / ray.direction.y;
    let hit = ray.origin + *ray.direction * t;
    let d = Vec2::new(hit.x - tf.translation.x, hit.z - tf.translation.z);
    if d.length_squared() > 0.04 {
        player.facing = d.y.atan2(d.x);
        me.aim = player.facing;
        // The body's nose points along local +z.
        tf.rotation = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2 - player.facing);
    }
}

fn actions(
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    me: Res<LocalPlayer>,
    join: Res<JoinInfo>,
    players: Query<(&Transform, &Player)>,
    mut link: ResMut<ServerLink>,
) {
    if me.dead {
        if keys.just_pressed(KeyCode::KeyR) {
            link.send(Intent::Respawn { player: me.id });
        }
        return;
    }
    if mouse.just_pressed(MouseButton::Left) {
        link.send(Intent::Attack { player: me.id });
    }
    if keys.just_pressed(KeyCode::Space) {
        // Stomp if a downed zombie lies right in front, else shove (the server decides what it hits).
        let c = &join.combat;
        let forward = Vec2::from_angle(me.aim);
        let half_arc = (c.stomp_arc_degrees * 0.5).to_radians().cos();
        let downed = link.curr.zombies.iter().any(|z| {
            let to = Vec2::from(z.pos) - me.curr.pos;
            matches!(z.anim, ZombieAnim::Down | ZombieAnim::GetUp) && to.length() <= c.stomp_range && forward.dot(to.normalize_or_zero()) >= half_arc
        });
        link.send(if downed { Intent::Stomp { player: me.id } } else { Intent::Shove { player: me.id } });
    }
    if keys.just_pressed(KeyCode::KeyE) {
        link.send(Intent::PickUp { player: me.id });
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
