//! Vehicles on screen (`zombye_sim::vehicles`): each car the snapshot lists is an entity of its
//! own, its street prop's model (the same one a parked prop of its kind draws, worn as
//! `decay.ron` says) or a grey-box of the prop's size and colour, drawn with the props' world
//! material (fog of war, lighting, shadows). Our own car is drawn where we predict it
//! (`player::LocalPlayer::car`, a snapshot's correction glided out), the others as
//! [`RemoteCars`] carries them: from snapshot to snapshot, and on along their way while none comes
//! (a stall, or a far car sent only now and then), a correction glided out the same way. A car
//! someone else drives is shown only where our survivor could see it, as other players are; a
//! parked one always, as the props are.
//!
//! Cars stage 5 (#39): the body sways on its springs as drawn ([`Sway`]: a lean out of turns, a
//! pitch back as it speeds up and forward as it brakes), and a running car has headlights and tail
//! lights ([`car_lights`], the nearest [`MAX_CAR_LIGHTS`]), whose beams show what they light
//! through the fog of war as other lights do. Client only: nothing here is in the sim.
//!
//! Cars in parts (#66): a model whose wheels, steering wheel and glass are nodes of their own
//! (`furniture_models::PartKind`, the release models' cars) is drawn as its body with each part a
//! child of it: the wheels roll by the distance driven, the front ones and the steering wheel turn
//! with the steering, and the glass is see-through. A parked prop of the same model is still drawn
//! whole in its chunk's mesh, and a model without parts as one mesh, as before.
//!
//! Damage (#68, `car_damage`): a car is drawn by the stage its body and windows are at (scrapes
//! baked into its colours, cracked or missing glass) and made again when that changes; it sits
//! lower on flat tyres.

use std::collections::HashMap;

use bevy::app::RunFixedMainLoopSystems;
use bevy::prelude::*;
use zombye_sim::map::Side;
use zombye_sim::{TICK_DT, VehicleState};

use crate::car_damage;
use crate::fog::FogState;
use crate::furniture_models::PartKind;
use crate::materials::WorldMaterials;
use crate::meshgen::MeshBuilder;
use crate::player::{CAR_SMOOTH_KEEP, CAR_SNAP_M, CAR_SNAP_RAD, LocalPlayer, short_turn};
use crate::server_link::{ClientMap, JoinInfo, ServerLink};
use crate::world_render::SharedMeshes;
use crate::zombie_render::lerp_angle;

/// A vehicle's body, by its id, drawn at this damage stage (`car_damage::stage`).
#[derive(Component)]
struct CarBody(u32, u8);

/// How a car moves as drawn this frame, for its wheels and steering wheel.
#[derive(Component, Default, Clone, Copy)]
struct CarMotion {
    speed: f32,
    steer: f32,
    /// How far the body sits down on flat tyres (m): the wheels stay on the ground.
    sink: f32,
}

/// A moving part of a car's model, a child of its [`CarBody`]: where it sits on the body, its
/// radius (a wheel's) and how far it has rolled (radians).
#[derive(Component)]
struct CarPart {
    kind: PartKind,
    rest: Transform,
    radius: f32,
    /// A steering wheel's column points back toward the driver in its model: it turns the other way.
    flip: bool,
    rolled: f32,
}

/// How many turns of the steering wheel one of the front wheels' (a car's steering ratio).
const STEERING_RATIO: f32 = 12.0;

/// How a body sits on its springs as drawn, eased toward [`sway_target`].
#[derive(Component, Default, Clone, Copy, Debug, PartialEq)]
pub struct Sway {
    /// Radians about the car's length (below 0: the roof toward its right) and about its width
    /// (above 0: the nose down).
    pub roll: f32,
    pub pitch: f32,
    /// The speed at the last frame, and how fast it has been changing (m/s², eased).
    last_speed: Option<f32>,
    pub accel: f32,
}

/// The most a body leans out of a turn and pitches (radians: about 4 and 2.5 degrees).
const MAX_ROLL: f32 = 0.07;
const MAX_PITCH: f32 = 0.045;
/// Lean per m/s² across the car, and pitch per m/s² along it.
const ROLL_PER: f32 = 0.012;
const PITCH_PER: f32 = 0.009;
/// Slowing faster than this (m/s²) lights the brake lights.
const BRAKING: f32 = 1.5;

/// The lean and pitch a car going `speed` (m/s) with its wheels at `steer` (radians, above 0
/// turning toward +y) on a `wheelbase`, its speed changing at `accel` (m/s²), sits at: out of
/// the turn, back as it gets going, forward as it brakes. (The body's +x is its right as it faces
/// +z: see the turn in [`draw_vehicles`].)
pub fn sway_target(speed: f32, steer: f32, wheelbase: f32, accel: f32) -> (f32, f32) {
    let yaw_rate = speed / wheelbase.max(0.5) * steer.tan();
    let across = speed * yaw_rate;
    ((-across * ROLL_PER).clamp(-MAX_ROLL, MAX_ROLL), (-accel * PITCH_PER).clamp(-MAX_PITCH, MAX_PITCH))
}

impl Sway {
    /// One frame of `dt` seconds with the car at `speed` and `steer`.
    pub fn step(&mut self, speed: f32, steer: f32, wheelbase: f32, dt: f32) {
        if dt <= 0.0 {
            return;
        }
        let raw = self.last_speed.map_or(0.0, |l| (speed - l) / dt).clamp(-30.0, 30.0);
        self.last_speed = Some(speed);
        self.accel += (raw - self.accel) * (dt * 5.0).min(1.0);
        let (roll, pitch) = sway_target(speed, steer, wheelbase, self.accel);
        let ease = (dt * 6.0).min(1.0);
        self.roll += (roll - self.roll) * ease;
        self.pitch += (pitch - self.pitch) * ease;
    }

    /// Turned this way on top of the car's heading.
    pub fn turn(&self) -> Quat {
        Quat::from_rotation_z(self.roll) * Quat::from_rotation_x(self.pitch)
    }
}

/// A car as drawn this frame, for its lights and sounds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DrawnCar {
    pub id: u32,
    pub pos: Vec2,
    pub heading: f32,
    pub speed: f32,
    pub engine_on: bool,
    /// Slowing hard (the brake lights).
    pub braking: bool,
    /// We drive it.
    pub ours: bool,
    /// Drawn (someone else's car out of sight isn't).
    pub shown: bool,
    pub length: f32,
    pub width: f32,
}

/// The cars drawn this frame.
#[derive(Resource, Default)]
pub struct DrawnCars(pub Vec<DrawnCar>);

/// Cars other than ours, as drawn: carried on along their way between snapshots, and the
/// difference a snapshot makes taken out over a few ticks.
#[derive(Resource, Default)]
pub struct RemoteCars(pub HashMap<u32, RemoteCar>);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RemoteCar {
    pub prev: VehicleState,
    pub curr: VehicleState,
    /// The state the last snapshot said.
    pub got: VehicleState,
    /// Ticks carried on since then.
    pub ahead: u32,
    /// Drawn this far from `curr` (metres, radians), going away (`CAR_SMOOTH_KEEP` a tick).
    pub off: Vec2,
    pub off_heading: f32,
}

/// A car with no news is carried on along its way for at most this many ticks (a far car is sent
/// every `FAR_CAR_EVERY`), then waits where it is.
pub const CARRY_TICKS: u32 = 12;

/// `s` moved on `dt` seconds at its speed and steering, as `step_vehicle` turns it (nothing in the
/// way is known here).
pub fn carry(s: VehicleState, wheelbase: f32, dt: f32) -> VehicleState {
    let heading = (s.heading + s.speed / wheelbase.max(0.5) * s.steer.tan() * dt).rem_euclid(std::f32::consts::TAU);
    VehicleState { pos: s.pos + Vec2::from_angle(heading) * s.speed * dt, heading, ..s }
}

impl RemoteCar {
    pub fn new(s: VehicleState) -> Self {
        Self { prev: s, curr: s, got: s, ahead: 0, off: Vec2::ZERO, off_heading: 0.0 }
    }

    /// One client tick, with the state the snapshot gives for the car now.
    pub fn tick(&mut self, now: VehicleState, wheelbase: f32) {
        self.off *= CAR_SMOOTH_KEEP;
        self.off_heading *= CAR_SMOOTH_KEEP;
        self.prev = self.curr;
        let on = carry(self.curr, wheelbase, TICK_DT);
        if now != self.got {
            // News: there it is; draw it from where it was going and glide the rest.
            self.got = now;
            self.ahead = 0;
            let err = on.pos - now.pos;
            let turn = short_turn(on.heading - now.heading);
            if err.length() > CAR_SNAP_M || turn.abs() > CAR_SNAP_RAD {
                (self.off, self.off_heading, self.prev) = (Vec2::ZERO, 0.0, now);
            } else {
                self.off += err;
                self.off_heading += turn;
                // The tick being drawn from moves with it, so nothing on screen jumps.
                self.prev.pos -= err;
                self.prev.heading -= turn;
            }
            self.curr = now;
        } else if self.curr.moving() && self.ahead < CARRY_TICKS {
            self.ahead += 1;
            self.curr = on;
        }
    }

    /// Where it is drawn, `alpha` of the way through the tick.
    pub fn shown(&self, alpha: f32) -> (Vec2, f32) {
        (self.prev.pos.lerp(self.curr.pos, alpha) + self.off, lerp_angle(self.prev.heading, self.curr.heading, alpha) + self.off_heading)
    }
}

/// Each client tick after the snapshot is taken up: every car but ours moves on.
fn follow_remote_cars(link: Res<ServerLink>, join: Res<JoinInfo>, me: Option<Res<LocalPlayer>>, mut cars: ResMut<RemoteCars>) {
    let views = &link.curr.vehicles;
    if views.is_empty() && cars.0.is_empty() {
        return;
    }
    let ours = me.as_ref().and_then(|m| m.car).map(|c| c.id);
    cars.0.retain(|id, _| Some(*id) != ours && views.iter().any(|v| v.id == *id));
    for v in views.iter().filter(|v| Some(v.id) != ours) {
        let wheelbase = join.vehicles.kinds.get(v.kind as usize).map_or(2.6, |k| k.wheelbase);
        match cars.0.get_mut(&v.id) {
            Some(c) => c.tick(v.state, wheelbase),
            None => {
                cars.0.insert(v.id, RemoteCar::new(v.state));
            }
        }
    }
}

/// Each vehicle's meshes at the damage stage it was last drawn at, made once a stage (its wear
/// goes by where it was parked): the body, and its moving parts in their own frames.
#[derive(Resource, Default)]
struct CarMeshes(HashMap<u32, (u8, CarLook)>);

#[derive(Clone)]
struct CarLook {
    body: Handle<Mesh>,
    parts: Vec<(PartKind, Transform, Handle<Mesh>, f32)>,
}

pub struct VehiclesRenderPlugin;

impl Plugin for VehiclesRenderPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CarMeshes>()
            .init_resource::<car_damage::Glass>()
            .init_resource::<RemoteCars>()
            .init_resource::<DrawnCars>()
            .add_systems(Startup, spawn_car_lights)
            .add_systems(FixedUpdate, follow_remote_cars.after(crate::server_link::ServerTick))
            .add_systems(RunFixedMainLoop, (draw_vehicles, turn_car_parts).chain().in_set(RunFixedMainLoopSystems::AfterFixedMainLoop));
    }
}

/// The meshes for vehicle `v`: its prop's model, facing +z (south) about the origin, split into its
/// body and moving parts when the model has them; else a grey-box. Its body scraped and its glass
/// as `stage` says (`car_damage::stage`).
fn car_look(v: &zombye_sim::VehicleView, stage: u8, join: &JoinInfo, map: &ClientMap, shared: &SharedMeshes, meshes: &mut Assets<Mesh>) -> Option<CarLook> {
    let kind = join.vehicles.kinds.get(v.kind as usize)?;
    let prop = map.0.prop_kinds.iter().find(|p| p.id == kind.prop);
    let model = prop.map_or(kind.prop.as_str(), |p| p.model());
    let models = &shared.furniture;
    // Worn as a prop parked where it was would be: the salt is where it was parked.
    let at = Vec2::new(v.from[0] as f32, v.from[1] as f32);
    let (body_stage, glass_stage) = car_damage::stages(stage);
    let wear = crate::meshgen::decay::Wear::of(crate::meshgen::decay::look(), &kind.prop, at, models.body_colour(model, at)).map(|w| crate::meshgen::decay::Wear { scuffs: car_damage::scuffs(body_stage), ..w });
    if let Some(split) = models.split_worn(model, at, wear.as_ref()) {
        let parts = split
            .parts
            .into_iter()
            // Wrecked windows are gone; cracked ones get the cracks laid over them.
            .filter(|(k, ..)| !(*k == PartKind::Glass && glass_stage >= 3))
            .map(|(k, rest, mesh, r)| (k, rest, meshes.add(if k == PartKind::Glass { car_damage::with_crack_uvs(mesh) } else { mesh }), r))
            .collect();
        return Some(CarLook { body: meshes.add(split.body), parts });
    }
    let mut mb = MeshBuilder::default();
    if !models.add_at_worn(&mut mb, model, Vec2::ZERO, Side::S, 0.0, wear.as_ref()) {
        let (size, color) = prop.map_or(((kind.width, kind.length, 1.5), (0.4, 0.4, 0.45)), |p| (p.size, p.color));
        crate::meshgen::street_props::grey_box(&mut mb, Vec2::ZERO, Side::S, size, color);
    }
    // A model in the far copy only (stand-ins) is still drawn: take the full one.
    let _ = mb.take_far();
    Some(CarLook { body: meshes.add(mb.build()), parts: Vec::new() })
}

/// A moving part's place on its body this frame: a wheel rolled `rolled` radians about its axle
/// (its own x) and, a front one, turned with the steering `steer` (radians, above 0 toward the
/// car's right) about its own up; the steering wheel turned about its column (its own z).
pub fn part_turn(kind: PartKind, rest: Transform, rolled: f32, steer: f32, flip: bool) -> Transform {
    let turn = match kind {
        PartKind::Wheel { front } => Quat::from_rotation_y(if front { -steer } else { 0.0 }) * Quat::from_rotation_x(rolled),
        PartKind::SteeringWheel => Quat::from_rotation_z(steer * STEERING_RATIO * if flip { -1.0 } else { 1.0 }),
        PartKind::Glass => Quat::IDENTITY,
    };
    Transform { rotation: rest.rotation * turn, ..rest }
}

/// Rolls the wheels by the distance each car went this frame and turns the front ones and the
/// steering wheel with the steering.
fn turn_car_parts(time: Res<Time>, cars: Query<&CarMotion>, mut parts: Query<(&mut CarPart, &mut Transform, &ChildOf)>) {
    let dt = time.delta_secs();
    for (mut part, mut tf, parent) in &mut parts {
        if part.kind == PartKind::Glass {
            continue;
        }
        let Ok(m) = cars.get(parent.parent()) else { continue };
        if !(m.speed.is_finite() && m.steer.is_finite()) {
            continue;
        }
        if matches!(part.kind, PartKind::Wheel { .. }) && m.speed != 0.0 {
            part.rolled = (part.rolled + m.speed * dt / part.radius).rem_euclid(std::f32::consts::TAU);
        }
        let mut want = part_turn(part.kind, part.rest, part.rolled, m.steer, part.flip);
        if matches!(part.kind, PartKind::Wheel { .. }) {
            want.translation.y += m.sink;
        }
        if *tf != want {
            *tf = want;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_vehicles(
    mut commands: Commands,
    link: Res<ServerLink>,
    join: Res<JoinInfo>,
    map: Res<ClientMap>,
    me: Option<Res<LocalPlayer>>,
    fixed: Res<Time<Fixed>>,
    fog: Res<FogState>,
    shared: Option<Res<SharedMeshes>>,
    mats: Option<Res<WorldMaterials>>,
    remote: Res<RemoteCars>,
    time: Res<Time>,
    mut made: ResMut<CarMeshes>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut drawn: ResMut<DrawnCars>,
    // One parameter for both: a system takes at most 16.
    (mut glass, mut standard, mut images): (ResMut<car_damage::Glass>, ResMut<Assets<StandardMaterial>>, ResMut<Assets<Image>>),
    mut cars: Query<(Entity, &CarBody, &mut Transform, &mut Visibility, &mut Sway, &mut CarMotion)>,
) {
    let views = &link.curr.vehicles;
    if views.is_empty() && cars.is_empty() {
        if !drawn.0.is_empty() {
            drawn.0.clear();
        }
        return;
    }
    drawn.0.clear();
    let dt = time.delta_secs();
    let (Some(shared), Some(mats)) = (shared, mats) else { return };
    for (e, body, ..) in &cars {
        if !views.iter().any(|v| v.id == body.0) {
            commands.entity(e).despawn();
        }
    }
    let alpha = fixed.overstep_fraction();
    let me_id = me.as_ref().map_or(0, |m| m.id);
    let ours = me.as_ref().and_then(|m| m.car);
    for v in views {
        // Where it is: ours as predicted, the rest between the snapshots.
        let mine = ours.filter(|c| c.id == v.id);
        let (pos, heading) = match mine {
            Some(c) => (c.prev.pos.lerp(c.curr.pos, alpha) + c.off, lerp_angle(c.prev.heading, c.curr.heading, alpha) + c.off_heading),
            None => remote.0.get(&v.id).map_or((v.state.pos, v.state.heading), |c| c.shown(alpha)),
        };
        let now = mine.map(|c| c.curr).or_else(|| remote.0.get(&v.id).map(|c| c.curr)).unwrap_or(v.state);
        // A car whose place isn't a number (#67) is drawn where the snapshot says, never at a NaN:
        // a transform or light there turns everything lit near it black.
        let (pos, heading, now) = if pos.is_finite() && heading.is_finite() && now.speed.is_finite() && now.steer.is_finite() {
            (pos, heading, now)
        } else if v.state.pos.is_finite() && v.state.heading.is_finite() {
            (v.state.pos, v.state.heading, VehicleState { speed: 0.0, steer: 0.0, ..v.state })
        } else {
            continue;
        };
        let kind = join.vehicles.kinds.get(v.kind as usize);
        let wheelbase = kind.map_or(2.6, |k| k.wheelbase);
        let stage = car_damage::stage(v.parts);
        let sink = car_damage::tyre_sink(v.parts[car_damage::TYRES]);
        // Drawn at another stage of damage: made again below.
        if let Some((e, ..)) = cars.iter().find(|(_, b, ..)| b.0 == v.id && b.1 != stage) {
            commands.entity(e).despawn();
        }
        let shown = v.driver == 0 || v.driver == me_id || fog.visibility(pos) > 0.35;
        let want = if shown { Visibility::Inherited } else { Visibility::Hidden };
        let yaw = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2 - heading);
        let mut car = DrawnCar {
            id: v.id,
            pos,
            heading,
            speed: now.speed,
            engine_on: v.engine_on,
            braking: false,
            ours: mine.is_some(),
            shown,
            length: kind.map_or(4.4, |k| k.length),
            width: kind.map_or(1.8, |k| k.width),
        };
        if let Some((_, _, mut t, mut vis, mut sway, mut motion)) = cars.iter_mut().find(|(_, b, ..)| b.0 == v.id && b.1 == stage) {
            sway.step(now.speed, now.steer, wheelbase, dt);
            if !(sway.roll.is_finite() && sway.pitch.is_finite() && sway.accel.is_finite()) {
                *sway = Sway::default();
            }
            let m = CarMotion { speed: now.speed, steer: now.steer, sink };
            if motion.speed != m.speed || motion.steer != m.steer || motion.sink != m.sink {
                *motion = m;
            }
            car.braking = now.speed.abs() > 0.5 && sway.accel * now.speed.signum() < -BRAKING;
            drawn.0.push(car);
            let tf = Transform::from_xyz(pos.x, map.0.ground(pos) - sink, pos.y).with_rotation(yaw * sway.turn());
            if *t != tf {
                *t = tf;
            }
            if *vis != want {
                *vis = want;
            }
            continue;
        }
        let look = match made.0.get(&v.id) {
            Some((s, m)) if *s == stage => m.clone(),
            _ => {
                let Some(m) = car_look(v, stage, &join, &map, &shared, &mut meshes) else { continue };
                made.0.insert(v.id, (stage, m.clone()));
                m
            }
        };
        drawn.0.push(car);
        let tf = Transform::from_xyz(pos.x, map.0.ground(pos) - sink, pos.y).with_rotation(yaw);
        let body = commands.spawn((CarBody(v.id, stage), Sway::default(), CarMotion { speed: now.speed, steer: now.steer, sink }, Mesh3d(look.body), MeshMaterial3d(mats.props.clone()), tf, want)).id();
        for (kind, rest, mesh, radius) in look.parts {
            // Its column's way along the car: one pointing back at the driver turns the other way.
            let flip = (rest.rotation * Vec3::Z).z < 0.0;
            let part = CarPart { kind, rest, radius, flip, rolled: 0.0 };
            let mut at = part_turn(kind, rest, 0.0, now.steer, flip);
            if matches!(kind, PartKind::Wheel { .. }) {
                at.translation.y += sink;
            }
            if kind == PartKind::Glass {
                let Some(m) = glass.at(car_damage::stages(stage).1, &mut standard, &mut images) else { continue };
                commands.spawn((part, Mesh3d(mesh), MeshMaterial3d(m), at, ChildOf(body), bevy::light::NotShadowCaster));
            } else {
                commands.spawn((part, Mesh3d(mesh), MeshMaterial3d(mats.props.clone()), at, ChildOf(body)));
            }
        }
    }
}

/// How many running cars have lights at once (the nearest), each a shadowed headlight beam and a
/// red tail light.
pub const MAX_CAR_LIGHTS: usize = 3;
/// How far a car's lights carry before they are left off (m from our survivor).
const CAR_LIGHT_REACH: f32 = 70.0;
/// The headlights: how high, how far ahead they are aimed, how far they reach.
const HEAD_HEIGHT: f32 = 0.75;
const HEAD_AIM_M: f32 = 12.0;
const HEAD_RANGE: f32 = 30.0;
const HEAD_OUTER: f32 = 0.5;

#[derive(Component)]
pub(crate) struct HeadLight(usize);

#[derive(Component)]
pub(crate) struct TailLight(usize);

fn spawn_car_lights(mut commands: Commands) {
    for slot in 0..MAX_CAR_LIGHTS {
        let beam = SpotLight {
            intensity: 0.0,
            range: HEAD_RANGE,
            radius: 0.1,
            color: Color::srgb(1.0, 0.95, 0.85),
            shadow_maps_enabled: true,
            outer_angle: HEAD_OUTER,
            inner_angle: HEAD_OUTER * 0.55,
            ..default()
        };
        commands.spawn((HeadLight(slot), beam, Transform::default(), Visibility::Hidden));
        let tail = PointLight { intensity: 0.0, range: 4.0, radius: 0.1, color: Color::srgb(1.0, 0.08, 0.05), shadow_maps_enabled: false, ..default() };
        commands.spawn((TailLight(slot), tail, Transform::default(), Visibility::Hidden));
    }
}

/// The running cars nearest `from` (ours first) that get lights, nearest first.
pub fn lit_cars(cars: &[DrawnCar], from: Option<Vec2>) -> Vec<DrawnCar> {
    let mut lit: Vec<(f32, DrawnCar)> = cars
        .iter()
        .filter(|c| c.engine_on && c.shown)
        .map(|c| (if c.ours { -1.0 } else { from.map_or(0.0, |f| f.distance(c.pos)) }, *c))
        .filter(|(d, _)| *d < CAR_LIGHT_REACH)
        .collect();
    lit.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.id.cmp(&b.1.id)));
    lit.into_iter().take(MAX_CAR_LIGHTS).map(|(_, c)| c).collect()
}

/// Headlights and tail lights on the nearest running cars (`lights.rs`'s chain, after the
/// survivors' own lights): the beam lights the road ahead, brighter in the dark, and what it falls
/// on shows through the fog of war; the tail light glows red, brighter while braking.
#[allow(clippy::too_many_arguments)]
pub fn car_lights(
    drawn: Res<DrawnCars>,
    link: Res<ServerLink>,
    me: Res<LocalPlayer>,
    daylight: Res<crate::daynight::Daylight>,
    map: Res<ClientMap>,
    mut heads: Query<(&HeadLight, &mut SpotLight, &mut Transform, &mut Visibility), Without<TailLight>>,
    mut tails: Query<(&TailLight, &mut PointLight, &mut Transform, &mut Visibility), Without<HeadLight>>,
    mut seen: ResMut<crate::lights::NightLights>,
) {
    let from = link.curr.players.iter().find(|p| p.id == me.id).map(|p| Vec2::from(p.pos));
    // Never a light at a place that isn't a number (#67: one there blacks out what it would light).
    let lit: Vec<DrawnCar> = lit_cars(&drawn.0, from).into_iter().filter(|c| c.pos.is_finite() && c.heading.is_finite()).collect();
    let dim = 1.0 - 0.85 * daylight.0;
    let dark = crate::lights::darkness(daylight.0);
    for (slot, mut spot, mut tf, mut vis) in &mut heads {
        let want = match lit.get(slot.0) {
            Some(c) => {
                let dir = Vec2::from_angle(c.heading);
                let front = c.pos + dir * (c.length * 0.5);
                let ground = map.0.ground(front);
                let at = Vec3::new(front.x, ground + HEAD_HEIGHT, front.y);
                let aim = front + dir * HEAD_AIM_M;
                let t = Transform::from_translation(at).looking_at(Vec3::new(aim.x, map.0.ground(aim), aim.y), Vec3::Y);
                if *tf != t {
                    *tf = t;
                }
                let power = 2_500_000.0 * dim;
                if spot.intensity != power {
                    spot.intensity = power;
                }
                if dark > 0.0 {
                    seen.0.push(crate::lights::SeenLight { pos: c.pos, radius: HEAD_RANGE * 0.8, dir, cos: (HEAD_OUTER * 0.9).cos() });
                    seen.0.push(crate::lights::SeenLight::round(c.pos, c.length * 0.6));
                }
                Visibility::Visible
            }
            None => Visibility::Hidden,
        };
        if *vis != want {
            *vis = want;
        }
    }
    for (slot, mut point, mut tf, mut vis) in &mut tails {
        let want = match lit.get(slot.0) {
            Some(c) => {
                let back = c.pos - Vec2::from_angle(c.heading) * (c.length * 0.5 + 0.3);
                let t = Transform::from_xyz(back.x, map.0.ground(back) + 0.8, back.y);
                if *tf != t {
                    *tf = t;
                }
                let power = if c.braking { 60_000.0 } else { 15_000.0 } * (0.3 + 0.7 * dark);
                if point.intensity != power {
                    point.intensity = power;
                }
                Visibility::Visible
            }
            None => Visibility::Hidden,
        };
        if *vis != want {
            *vis = want;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::CarPred;

    fn at(x: f32, speed: f32) -> VehicleState {
        VehicleState { pos: Vec2::new(x, 0.0), heading: 0.0, speed, steer: 0.0 }
    }

    /// A snapshot's correction of our own car is drawn as a glide: nothing moves on screen at the
    /// moment it comes, and it is gone within a second. A big one is taken at once.
    #[test]
    fn our_car_glides_into_a_correction() {
        let mut c = CarPred::new(1, 0, at(10.0, 0.0));
        let drawn = c.curr.pos + c.off;
        c.correct(at(9.0, 0.0));
        assert_eq!(c.curr.pos.x, 9.0);
        assert!((c.curr.pos + c.off - drawn).length() < 1e-5, "drawn where it was");
        assert!((c.prev.pos + c.off - drawn).length() < 1e-5, "the tick it is drawn from moved with it");
        for _ in 0..20 {
            c.prev = c.curr;
            c.settle();
        }
        assert!(c.off.length() < 0.05, "glided in within a second: {:?}", c.off);
        c.correct(at(30.0, 0.0));
        assert_eq!((c.off, c.prev.pos.x), (Vec2::ZERO, 30.0), "too far to glide: taken at once");
    }

    /// Another car is carried on along its way when no snapshot comes, for a while, then waits;
    /// news glides it to where it really is.
    #[test]
    fn other_cars_carry_on_between_snapshots() {
        let mut c = RemoteCar::new(at(0.0, 10.0));
        // No news for five ticks: half a metre a tick.
        for _ in 0..5 {
            c.tick(at(0.0, 10.0), 2.7);
        }
        assert!((c.curr.pos.x - 2.5).abs() < 1e-4, "carried on: {:?}", c.curr.pos);
        // News that it is at 2 m (it would have been at 3): drawn on from where it was going,
        // then glided there.
        let before = c.shown(1.0).0;
        c.tick(at(2.0, 10.0), 2.7);
        assert_eq!(c.curr.pos.x, 2.0);
        assert!((c.shown(0.0).0 - before).length() < 1e-4, "no jump at the snapshot");
        assert!((c.shown(1.0).0.x - 3.0).abs() < 1e-4, "drawn where it was going");
        for _ in 0..20 {
            let now = carry(c.got, 2.7, TICK_DT);
            c.tick(now, 2.7);
        }
        assert!(c.off.length() < 0.05, "glided in: {:?}", c.off);
        // Turning: carried round as the wheels point.
        let mut t = RemoteCar::new(VehicleState { steer: 0.3, ..at(0.0, 5.0) });
        t.tick(t.got, 2.7);
        assert!(t.curr.heading > 0.0 && t.curr.pos.y > 0.0, "turned clockwise (toward +y)");
        // A long stall: it stops after `CARRY_TICKS`.
        let mut s = RemoteCar::new(at(0.0, 10.0));
        for _ in 0..40 {
            s.tick(at(0.0, 10.0), 2.7);
        }
        assert!((s.curr.pos.x - CARRY_TICKS as f32 * 0.5).abs() < 1e-3, "waits after carrying on: {:?}", s.curr.pos);
        // A parked car stays put.
        let mut p = RemoteCar::new(at(4.0, 0.0));
        p.tick(at(4.0, 0.0), 2.7);
        assert_eq!((p.curr, p.off), (at(4.0, 0.0), Vec2::ZERO));
    }

    /// A car turning leans its roof out of the turn and pitches back as it gets going, forward as
    /// it brakes; standing still it sits level.
    #[test]
    fn a_car_leans_out_of_its_turns() {
        assert_eq!(sway_target(0.0, 0.3, 2.6, 0.0), (0.0, 0.0));
        // Heading east, turning toward +y: the turn's middle is on the +y side.
        let (roll, _) = sway_target(12.0, 0.3, 2.6, 0.0);
        assert!((-MAX_ROLL..0.0).contains(&roll));
        let heading = 0.0f32;
        let turn = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2 - heading) * Sway { roll, ..default() }.turn();
        let roof = turn * Vec3::Y;
        assert!(roof.z < -0.01, "the roof goes toward -y (sim), out of the turn: {roof}");
        let (_, getting_going) = sway_target(5.0, 0.0, 2.6, 3.0);
        let (_, braking) = sway_target(5.0, 0.0, 2.6, -6.0);
        assert!(getting_going < 0.0 && braking > 0.0, "back as it speeds up, nose down braking");
        // Eased: one frame doesn't take it all the way.
        let mut s = Sway::default();
        s.step(12.0, 0.3, 2.6, 1.0 / 60.0);
        assert!(s.roll < 0.0 && s.roll > roll);
    }

    /// Lights go on the running cars nearest us, ours first, at most three.
    #[test]
    fn the_nearest_running_cars_get_lights() {
        let car = |id, x: f32, on, ours| DrawnCar { id, pos: Vec2::new(x, 0.0), heading: 0.0, speed: 0.0, engine_on: on, braking: false, ours, shown: true, length: 4.4, width: 1.8 };
        let cars = [car(1, 50.0, true, false), car(2, 5.0, true, false), car(3, 2.0, false, false), car(4, 30.0, true, true), car(5, 20.0, true, false), car(6, 200.0, true, false)];
        let lit: Vec<u32> = lit_cars(&cars, Some(Vec2::ZERO)).iter().map(|c| c.id).collect();
        assert_eq!(lit, vec![4, 2, 5]);
    }

    #[test]
    fn wheels_roll_forward_and_the_front_ones_steer() {
        let rest = Transform::from_xyz(0.8, 0.35, 1.3);
        // Rolled forward a quarter turn, the top of a wheel has gone toward the front (+z).
        let t = part_turn(PartKind::Wheel { front: false }, rest, std::f32::consts::FRAC_PI_2, 0.4, false);
        assert!((t.rotation * Vec3::Y).z > 0.99, "{:?}", t.rotation * Vec3::Y);
        assert_eq!(t.translation, rest.translation);
        // A back wheel doesn't steer; a front one turns its axle with the steering.
        assert!((t.rotation * Vec3::X - Vec3::X).length() < 1e-5);
        let f = part_turn(PartKind::Wheel { front: true }, rest, 0.0, 0.4, false);
        assert!((f.rotation * Vec3::X).z.abs() > 0.3);
        // Steering right (above 0) turns the car's heading up, which on screen is a turn about -y:
        // the front of the wheel (+z) goes toward the car's right (-x, the body faces +z).
        assert!((f.rotation * Vec3::Z).x < -0.3, "{:?}", f.rotation * Vec3::Z);
        // The steering wheel goes round STEERING_RATIO times as far, the other way when flipped.
        let s = part_turn(PartKind::SteeringWheel, Transform::default(), 0.0, 0.1, false);
        let back = part_turn(PartKind::SteeringWheel, Transform::default(), 0.0, 0.1, true);
        assert!((s.rotation.to_axis_angle().1 - 0.1 * STEERING_RATIO).abs() < 1e-4);
        assert!(((s.rotation * Vec3::X) - (back.rotation * Vec3::X)).length() > 0.5);
        // Glass stays where it is.
        assert_eq!(part_turn(PartKind::Glass, rest, 1.0, 1.0, false), rest);
    }
}
