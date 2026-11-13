//! Vehicles on screen (`zombye_sim::vehicles`): each car the snapshot lists is an entity of its
//! own, its street prop's model (the same one a parked prop of its kind draws, worn as
//! `decay.ron` says) or a grey-box of the prop's size and colour, drawn with the props' world
//! material (fog of war, lighting, shadows). Our own car is drawn where we predict it
//! (`player::LocalPlayer::car`, a snapshot's correction glided out), the others as
//! [`RemoteCars`] carries them: from snapshot to snapshot, and on along their way while none comes
//! (a stall, or a far car sent only now and then), a correction glided out the same way. A car
//! someone else drives is shown only where our survivor could see it, as other players are; a
//! parked one always, as the props are.

use std::collections::HashMap;

use bevy::app::RunFixedMainLoopSystems;
use bevy::prelude::*;
use zombye_sim::map::Side;
use zombye_sim::{TICK_DT, VehicleState};

use crate::fog::FogState;
use crate::materials::WorldMaterials;
use crate::meshgen::MeshBuilder;
use crate::player::{CAR_SMOOTH_KEEP, CAR_SNAP_M, CAR_SNAP_RAD, LocalPlayer, short_turn};
use crate::server_link::{ClientMap, JoinInfo, ServerLink};
use crate::world_render::SharedMeshes;
use crate::zombie_render::lerp_angle;

/// A vehicle's body, by its id.
#[derive(Component)]
struct CarBody(u32);

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

/// Each vehicle's mesh, made once (its wear goes by where it was parked).
#[derive(Resource, Default)]
struct CarMeshes(HashMap<u32, Handle<Mesh>>);

pub struct VehiclesRenderPlugin;

impl Plugin for VehiclesRenderPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CarMeshes>()
            .init_resource::<RemoteCars>()
            .add_systems(FixedUpdate, follow_remote_cars.after(crate::server_link::ServerTick))
            .add_systems(RunFixedMainLoop, draw_vehicles.in_set(RunFixedMainLoopSystems::AfterFixedMainLoop));
    }
}

/// The mesh for vehicle `v`: its prop's model, facing +z (south) about the origin, or a grey-box.
fn car_mesh(v: &zombye_sim::VehicleView, join: &JoinInfo, map: &ClientMap, shared: &SharedMeshes) -> Option<Mesh> {
    let kind = join.vehicles.kinds.get(v.kind as usize)?;
    let prop = map.0.prop_kinds.iter().find(|p| p.id == kind.prop);
    let model = prop.map_or(kind.prop.as_str(), |p| p.model());
    let models = &shared.furniture;
    // Worn as a prop parked where it was would be: the salt is where it was parked.
    let at = Vec2::new(v.from[0] as f32, v.from[1] as f32);
    let wear = crate::meshgen::decay::Wear::of(crate::meshgen::decay::look(), &kind.prop, at, models.body_colour(model, at));
    let mut mb = MeshBuilder::default();
    if !models.add_at_worn(&mut mb, model, Vec2::ZERO, Side::S, 0.0, wear.as_ref()) {
        let (size, color) = prop.map_or(((kind.width, kind.length, 1.5), (0.4, 0.4, 0.45)), |p| (p.size, p.color));
        crate::meshgen::street_props::grey_box(&mut mb, Vec2::ZERO, Side::S, size, color);
    }
    // A model in the far copy only (stand-ins) is still drawn: take the full one.
    let _ = mb.take_far();
    Some(mb.build())
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
    mut made: ResMut<CarMeshes>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut cars: Query<(Entity, &CarBody, &mut Transform, &mut Visibility)>,
) {
    let views = &link.curr.vehicles;
    if views.is_empty() && cars.is_empty() {
        return;
    }
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
        let (pos, heading) = match ours.filter(|c| c.id == v.id) {
            Some(c) => (c.prev.pos.lerp(c.curr.pos, alpha) + c.off, lerp_angle(c.prev.heading, c.curr.heading, alpha) + c.off_heading),
            None => remote.0.get(&v.id).map_or((v.state.pos, v.state.heading), |c| c.shown(alpha)),
        };
        let tf = Transform::from_xyz(pos.x, map.0.ground(pos), pos.y).with_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2 - heading));
        let shown = v.driver == 0 || v.driver == me_id || fog.visibility(pos) > 0.35;
        let want = if shown { Visibility::Inherited } else { Visibility::Hidden };
        if let Some((_, _, mut t, mut vis)) = cars.iter_mut().find(|(_, b, ..)| b.0 == v.id) {
            if *t != tf {
                *t = tf;
            }
            if *vis != want {
                *vis = want;
            }
            continue;
        }
        let mesh = match made.0.get(&v.id) {
            Some(m) => m.clone(),
            None => {
                let Some(m) = car_mesh(v, &join, &map, &shared) else { continue };
                let h = meshes.add(m);
                made.0.insert(v.id, h.clone());
                h
            }
        };
        commands.spawn((CarBody(v.id), Mesh3d(mesh), MeshMaterial3d(mats.props.clone()), tf, want));
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
}
