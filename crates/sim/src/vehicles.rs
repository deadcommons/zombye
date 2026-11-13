//! Vehicles you can drive (Linear DCM-85; this is stage 1, DCM-86): a share of the cars parked in
//! the street are real vehicles, which a survivor gets into (E beside one), drives (W/S throttle
//! and reverse, A/D steer, Space brake) and gets out of again (E).
//!
//! **What they are is data** (`assets/data/vehicles.ron`, [`VehicleData`]): each kind names the
//! street prop it is parked as (`prop`), its body type ([`Body`]: how it moves, the one part in
//! code), its size, speeds, steering and seats; `drivable_share` says how many of the parked props
//! of those kinds are vehicles (the rest stay props). A data folder without the file has none.
//!
//! **Where they come from:** a new world ([`World::new`]) takes the parked props it picks (by a hash
//! of the seed and the tile, so the same seed gives the same cars) off the map and puts a vehicle
//! there instead, facing the way the prop faced. The map's base hash is the generated map's, with
//! every prop, so a save regenerates the same map and its `VEHC` section takes the same props off
//! again (by each vehicle's `from` tile) wherever the cars have since been driven.
//!
//! **Driving** is one pure function, [`step_vehicle`], for the server and a client's prediction
//! alike: an arcade bicycle model (throttle, brake, coasting, steering that eases to the wheel's
//! angle and narrows with speed) moved in short steps, its body a row of circles along its length
//! kept out of walls, fences, trees, props, the map's edge and the other vehicles
//! ([`TileMap::resolve_vehicle_circle`]); what it runs into takes the speed into it away.
//!
//! **On the map:** every vehicle is a box ([`crate::map::VehicleBox`]) that bodies are kept out of
//! ([`TileMap::resolve_circle`] and the zombies' open-ground short cut look at it), set whenever one
//! moves; a client sets the same boxes from its snapshots. A moving vehicle pushes the zombies and
//! survivors in its way aside and loses some speed for each zombie (running them over: stage 2).
//!
//! **A driver** ([`Player::vehicle`]) sits in the driver's seat: its body is put there every tick
//! ([`seat_of`]), facing the way the car does, and it doesn't walk, fight, climb, sit or sleep. It
//! gets out on the driver's side, else the other, the back or the front, whichever is clear, and
//! only when the car is nearly stopped. Dead, asleep, logged out or tied, it lets go of the wheel.
//!
//! **Saves:** an optional `VEHC` section (the vehicles, by kind id, and the next id; no format bump),
//! in the state hash only while there are any.

use glam::{IVec2, Vec2};
use serde::{Deserialize, Serialize};

use crate::map::{Feature, TileMap, VehicleBox};
use crate::players::PLAYER_RADIUS;
use crate::world::World;

/// How a kind of vehicle moves. Only cars for now; every kind is one of these, and everything
/// else about it is data.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Body {
    /// Four wheels, steered by the front pair: cars, vans, pickups.
    #[default]
    Car,
    /// The same, long (a bus, a lorry): turns wider for its wheelbase, nothing else differs yet.
    Long,
}

/// A kind of vehicle (`vehicles.ron`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VehicleKind {
    pub id: String,
    /// What it is called in notes ("the police car").
    #[serde(default)]
    pub name: String,
    /// The street prop kind it is parked as (`street_props.ron`): its model, and where it stands.
    pub prop: String,
    #[serde(default)]
    pub body: Body,
    /// Metres, nose to tail and side to side: the box bodies are kept out of.
    pub length: f32,
    pub width: f32,
    /// Metres between the axles: how tight it turns.
    pub wheelbase: f32,
    /// Top speed forward and in reverse, m/s.
    pub max_speed: f32,
    pub reverse_speed: f32,
    /// m/s² of throttle from standing (less as it nears top speed), of braking, and of slowing by
    /// itself with neither.
    pub accel: f32,
    pub brake: f32,
    pub coast: f32,
    /// The front wheels' furthest turn standing still, and how fast they turn (degrees a second).
    pub steer_degrees: f32,
    pub steer_rate_degrees: f32,
    /// How far the wheels still turn at speed: their furthest turn is divided by
    /// `1 + speed / steer_falloff` (m/s).
    pub steer_falloff: f32,
    /// Seats, the driver's included.
    pub seats: u8,
    /// Where the driver sits, metres forward of its middle and to its left.
    pub driver_seat: (f32, f32),
}

impl VehicleKind {
    /// The box of half its length and half its width.
    pub fn half(&self) -> Vec2 {
        Vec2::new(self.length, self.width) * 0.5
    }

    /// Its name for a note: the name, else the id with spaces.
    pub fn label(&self) -> String {
        if self.name.is_empty() { self.id.replace('_', " ") } else { self.name.clone() }
    }
}

/// The rules round driving (`vehicles.ron`'s `tuning`; ours).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct VehicleTuning {
    /// How far from the side of a car (m, to its box) a survivor can get in.
    pub enter_reach: f32,
    /// Faster than this (m/s) nobody gets in or out.
    pub enter_max_speed: f32,
    pub exit_max_speed: f32,
    /// The share of its speed a vehicle loses for each zombie it pushes aside.
    pub zombie_slow: f32,
    /// The share of the speed into a wall that bounces back off it.
    pub bounce: f32,
}

impl Default for VehicleTuning {
    fn default() -> Self {
        Self { enter_reach: 1.2, enter_max_speed: 1.0, exit_max_speed: 2.0, zombie_slow: 0.12, bounce: 0.2 }
    }
}

/// `vehicles.ron`: the kinds and how many of the parked props of those kinds are vehicles.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct VehicleData {
    /// Of the parked props whose kind a vehicle kind names, the share that are vehicles (0..1).
    pub drivable_share: f32,
    pub kinds: Vec<VehicleKind>,
    pub tuning: VehicleTuning,
}

impl VehicleData {
    /// The kind parked as prop kind `prop`.
    pub fn of_prop(&self, prop: &str) -> Option<usize> {
        self.kinds.iter().position(|k| k.prop == prop)
    }

    pub fn by_id(&self, id: &str) -> Option<usize> {
        self.kinds.iter().position(|k| k.id == id)
    }
}

/// Read `vehicles.ron`, checking every number (errors name the kind).
pub fn parse_vehicles(text: &str) -> Result<VehicleData, String> {
    let data: VehicleData = ron::from_str(text).map_err(|e| e.to_string())?;
    check(&data)?;
    Ok(data)
}

/// Every number in range, ids unique, one kind a prop.
pub fn check(data: &VehicleData) -> Result<(), String> {
    if !(0.0..=1.0).contains(&data.drivable_share) {
        return Err(format!("drivable_share {} must be between 0 and 1", data.drivable_share));
    }
    let t = &data.tuning;
    for (what, v, lo, hi) in [("enter_reach", t.enter_reach, 0.1, 5.0), ("enter_max_speed", t.enter_max_speed, 0.0, 30.0), ("exit_max_speed", t.exit_max_speed, 0.0, 30.0), ("zombie_slow", t.zombie_slow, 0.0, 1.0), ("bounce", t.bounce, 0.0, 1.0)] {
        if !v.is_finite() || !(lo..=hi).contains(&v) {
            return Err(format!("tuning: {what} {v} must be between {lo} and {hi}"));
        }
    }
    for (i, k) in data.kinds.iter().enumerate() {
        let bad = |why: String| Err(format!("vehicle {:?}: {why}", k.id));
        if k.id.is_empty() || !k.id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_') {
            return bad("an id is lower-case letters, digits and _".into());
        }
        if data.kinds[..i].iter().any(|o| o.id == k.id) {
            return bad("listed twice".into());
        }
        if data.kinds[..i].iter().any(|o| o.prop == k.prop) {
            return bad(format!("another kind is already parked as {:?}", k.prop));
        }
        for (what, v, lo, hi) in [
            ("length", k.length, 1.0, 20.0),
            ("width", k.width, 0.5, 4.0),
            ("wheelbase", k.wheelbase, 0.5, 15.0),
            ("max_speed", k.max_speed, 1.0, 80.0),
            ("reverse_speed", k.reverse_speed, 0.0, 30.0),
            ("accel", k.accel, 0.1, 30.0),
            ("brake", k.brake, 0.1, 50.0),
            ("coast", k.coast, 0.0, 20.0),
            ("steer_degrees", k.steer_degrees, 1.0, 60.0),
            ("steer_rate_degrees", k.steer_rate_degrees, 1.0, 1000.0),
            ("steer_falloff", k.steer_falloff, 0.1, 200.0),
        ] {
            if !v.is_finite() || !(lo..=hi).contains(&v) {
                return bad(format!("{what} {v} must be between {lo} and {hi}"));
            }
        }
        if k.wheelbase > k.length {
            return bad("its wheelbase is longer than it is".into());
        }
        if k.seats == 0 {
            return bad("needs a seat for the driver".into());
        }
        let (f, l) = k.driver_seat;
        if !f.is_finite() || !l.is_finite() || f.abs() > k.length * 0.5 || l.abs() > k.width * 0.5 {
            return bad(format!("driver_seat {:?} is outside it", k.driver_seat));
        }
    }
    Ok(())
}

/// What the driver does this tick: `throttle` -1 (reverse) to 1, `steer` -1 (left) to 1 (right),
/// `brake` held.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DriveInput {
    pub throttle: f32,
    pub steer: f32,
    pub brake: bool,
}

impl DriveInput {
    /// Nobody at the wheel: the brake on.
    pub const PARKED: Self = Self { throttle: 0.0, steer: 0.0, brake: true };

    /// In range, and nothing that isn't a number.
    pub fn sanitized(self) -> Self {
        let f = |v: f32| if v.is_finite() { v.clamp(-1.0, 1.0) } else { 0.0 };
        Self { throttle: f(self.throttle), steer: f(self.steer), brake: self.brake }
    }
}

/// Where a vehicle is and how it moves: `heading` radians (0 east, as a player's facing), `speed`
/// m/s along it (negative in reverse), `steer` the front wheels' angle (radians, positive right).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct VehicleState {
    pub pos: Vec2,
    pub heading: f32,
    pub speed: f32,
    pub steer: f32,
}

impl VehicleState {
    pub fn forward(&self) -> Vec2 {
        Vec2::from_angle(self.heading)
    }

    pub fn moving(&self) -> bool {
        self.speed.abs() > 0.05 || self.steer != 0.0
    }
}

/// A vehicle in the world.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Vehicle {
    pub id: u32,
    /// Index into [`VehicleData::kinds`] (saved by id).
    pub kind: u16,
    pub state: VehicleState,
    /// The driver's latest input, held until the next (saved: the future depends on it).
    pub input: DriveInput,
    /// The player at the wheel.
    pub driver: Option<u32>,
    /// The least corner of the parked prop it was (taken off the map again on load).
    pub from: IVec2,
}

impl Vehicle {
    pub fn boxed(&self, kind: &VehicleKind) -> VehicleBox {
        VehicleBox { id: self.id, pos: self.state.pos, heading: self.state.heading, half: kind.half() }
    }
}

/// The world's vehicles, in id order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Vehicles {
    pub list: Vec<Vehicle>,
    pub next_id: u32,
}

/// A vehicle as a client sees it (`Snapshot::vehicles`).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct VehicleView {
    pub id: u32,
    pub kind: u16,
    pub state: VehicleState,
    /// The player at the wheel; 0 none.
    pub driver: u32,
    /// The parked prop it was: the client takes it off its map too.
    pub from: [i32; 2],
}

impl VehicleView {
    pub fn of(v: &Vehicle) -> Self {
        Self { id: v.id, kind: v.kind, state: v.state, driver: v.driver.unwrap_or(0), from: v.from.to_array() }
    }

    pub fn boxed(&self, kinds: &[VehicleKind]) -> Option<VehicleBox> {
        kinds.get(self.kind as usize).map(|k| VehicleBox { id: self.id, pos: self.state.pos, heading: self.state.heading, half: k.half() })
    }
}

/// Where the driver of a vehicle of `kind` in `state` sits.
pub fn seat_of(kind: &VehicleKind, state: &VehicleState) -> Vec2 {
    let (f, l) = kind.driver_seat;
    let fwd = state.forward();
    // Left of the way it faces, with y down (south): the facing turned a quarter anticlockwise.
    let left = Vec2::new(fwd.y, -fwd.x);
    state.pos + fwd * f + left * l
}

/// Most a vehicle moves in one step of [`step_vehicle`] (m), so it can't jump a wall.
const MAX_STEP: f32 = 0.25;

/// The circles along a vehicle's length (in its own frame, x along it) and their radius: as wide
/// as it, at most 0.9 m (what the map's three-by-three tile look covers with a wall's thickness).
fn circles(kind: &VehicleKind) -> (impl Iterator<Item = f32>, f32) {
    let r = (kind.width * 0.5).min(0.9);
    let reach = (kind.length * 0.5 - r).max(0.0);
    let n = ((2.0 * reach / r).ceil() as usize).max(1);
    ((0..=n).map(move |i| if n == 0 { 0.0 } else { -reach + 2.0 * reach * i as f32 / n as f32 }), r)
}

/// One tick (`dt` s) of driving a vehicle of `kind` with `input`: its speed and steering change,
/// then it moves, kept out of what blocks it on `map` (but its own box, `id`). The server's and a
/// client's prediction alike. Returns the push of the hardest thing it ran into this tick (zero
/// for none): its direction is away from what it hit.
pub fn step_vehicle(map: &TileMap, kind: &VehicleKind, id: u32, state: &mut VehicleState, input: DriveInput, dt: f32, bounce: f32) -> Vec2 {
    let input = input.sanitized();
    // Steering: toward the wheel's angle, narrower at speed.
    let max = kind.steer_degrees.to_radians() / (1.0 + state.speed.abs() / kind.steer_falloff);
    let want = input.steer * max;
    let rate = kind.steer_rate_degrees.to_radians() * dt;
    state.steer = (state.steer + (want - state.steer).clamp(-rate, rate)).clamp(-max, max);
    if state.steer.abs() < 1e-4 && want == 0.0 {
        state.steer = 0.0;
    }
    // Speed: throttle, braking (the throttle against the way it rolls brakes too), coasting.
    let v = state.speed;
    let toward = |v: f32, by: f32| if v > 0.0 { (v - by).max(0.0) } else { (v + by).min(0.0) };
    state.speed = if input.brake {
        toward(v, kind.brake * dt)
    } else if input.throttle > 0.0 && v < -0.05 || input.throttle < 0.0 && v > 0.05 {
        toward(v, kind.brake * input.throttle.abs() * dt)
    } else if input.throttle > 0.0 {
        (v + kind.accel * input.throttle * (1.0 - v / kind.max_speed).max(0.0) * dt).min(kind.max_speed)
    } else if input.throttle < 0.0 {
        (v + kind.accel * input.throttle * (1.0 + v / kind.reverse_speed.max(0.1)).max(0.0) * dt).max(-kind.reverse_speed)
    } else {
        toward(v, kind.coast * dt)
    };
    if state.speed == 0.0 {
        return Vec2::ZERO;
    }
    let dist = state.speed.abs() * dt;
    let n = (dist / MAX_STEP).ceil().max(1.0) as u32;
    let sub = dt / n as f32;
    let mut hardest = Vec2::ZERO;
    for _ in 0..n {
        state.heading = (state.heading + state.speed / kind.wheelbase * state.steer.tan() * sub).rem_euclid(std::f32::consts::TAU);
        state.pos += state.forward() * state.speed * sub;
        let push = collide(map, kind, id, state);
        if push.length_squared() > 1e-8 {
            if push.length_squared() > hardest.length_squared() {
                hardest = push;
            }
            // The speed into what it hit goes (a little of it comes back), the rest carries on.
            let normal = push.normalize();
            let vel = state.forward() * state.speed;
            let into = vel.dot(normal);
            if into < 0.0 {
                state.speed = (vel - normal * into * (1.0 + bounce)).dot(state.forward());
            }
        }
    }
    hardest
}

/// Push a vehicle's circles out of what blocks them; returns the push applied to it.
fn collide(map: &TileMap, kind: &VehicleKind, id: u32, state: &mut VehicleState) -> Vec2 {
    let mut total = Vec2::ZERO;
    for _ in 0..3 {
        let fwd = state.forward();
        let (along, r) = circles(kind);
        let mut push = Vec2::ZERO;
        for x in along {
            let c = state.pos + fwd * x;
            let d = map.resolve_vehicle_circle(c, r, id) - c;
            if d.length_squared() > push.length_squared() {
                push = d;
            }
        }
        if push.length_squared() < 1e-10 {
            break;
        }
        state.pos += push;
        total += push;
    }
    total
}

/// Which way a side points, as a heading.
fn heading_of(side: crate::map::Side) -> f32 {
    side.offset().as_vec2().to_angle().rem_euclid(std::f32::consts::TAU)
}

/// The vehicle (by index into `boxes`) a survivor at `pos` facing `facing` would get into: the
/// nearest within `reach` of its box that it faces (within 75 degrees of the way to it). The
/// server's rule, and the client's for its E key.
pub fn enterable(boxes: &[VehicleBox], pos: Vec2, facing: f32, reach: f32) -> Option<usize> {
    let mut best: Option<(f32, usize)> = None;
    for (i, b) in boxes.iter().enumerate() {
        let d = b.distance(pos);
        if d > reach {
            continue;
        }
        let o = b.local(pos).clamp(-b.half, b.half);
        let closest = b.pos + Vec2::from_angle(b.heading).rotate(o);
        let to = closest - pos;
        let faces = to.length() < 0.05 || Vec2::from_angle(facing).dot(to.normalize()) > 75f32.to_radians().cos();
        if faces && best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, i));
        }
    }
    best.map(|(_, i)| i)
}

/// What a survivor asks of a vehicle (`Intent::Vehicle`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum VehicleOp {
    /// Get in at the wheel: of `vehicle`, else the one faced.
    Enter { vehicle: Option<u32> },
    Exit,
}

impl World {
    pub fn vehicles(&self) -> &[Vehicle] {
        &self.vehicles.list
    }

    /// Player `id` is at a wheel.
    pub fn in_vehicle(&self, id: u32) -> bool {
        self.players.iter().any(|p| p.id == id && p.vehicle.is_some())
    }

    /// The vehicle player `id` is in, if any.
    pub fn vehicle_of(&self, id: u32) -> Option<&Vehicle> {
        let vid = self.players.iter().find(|p| p.id == id)?.vehicle?;
        self.vehicles.list.iter().find(|v| v.id == vid)
    }

    fn vehicle_kind(&self, v: &Vehicle) -> Option<&VehicleKind> {
        self.config.vehicles.kinds.get(v.kind as usize)
    }

    /// The vehicles' boxes, for the map.
    fn vehicle_boxes(&self) -> Vec<VehicleBox> {
        self.vehicles.list.iter().filter_map(|v| self.vehicle_kind(v).map(|k| v.boxed(k))).collect()
    }

    /// A new world's vehicles: the parked props the share picks become vehicles, off the map.
    /// Before the walk grid and the nav graph are built on the map (`World::new`).
    pub(crate) fn place_vehicles(&mut self) {
        let data = &self.config.vehicles;
        if data.kinds.is_empty() || data.drivable_share <= 0.0 {
            return;
        }
        let seed = self.config.generation.seed;
        let mut picked = Vec::new();
        for p in &self.map.street_props {
            let Some(pk) = self.map.prop_kinds.get(p.kind as usize) else { continue };
            let Some(kind) = data.of_prop(&pk.id) else { continue };
            if crate::rng::hash01(seed ^ 0x5645_4843, p.tile.x as u32 as u64, p.tile.y as u32 as u64, 0) < data.drivable_share {
                picked.push((*p, kind));
            }
        }
        for (p, kind) in picked {
            let (min, size) = self.map.street_prop_rect(&p);
            let state = VehicleState { pos: min.as_vec2() + size.as_vec2() * 0.5, heading: heading_of(p.facing), speed: 0.0, steer: 0.0 };
            take_prop(&mut self.map, p.tile);
            self.vehicles.next_id = self.vehicles.next_id.max(1);
            let id = self.vehicles.next_id;
            self.vehicles.next_id += 1;
            self.vehicles.list.push(Vehicle { id, kind: kind as u16, state, input: DriveInput::PARKED, driver: None, from: p.tile });
        }
        let boxes = self.vehicle_boxes();
        self.map.set_vehicles(boxes);
    }

    /// Put saved vehicles back on a map regenerated from the seed (load): their props off it, the
    /// vehicles on. Kinds by id; a vehicle whose kind is gone is dropped (counted). Before the walk
    /// grid is rebuilt.
    pub(crate) fn restore_vehicles(&mut self, save: VehicleSave) -> Result<usize, String> {
        let mut dropped = 0;
        let mut prev = 0;
        for r in save.list {
            if r.id <= prev || r.id >= save.next_id {
                return Err(format!("vehicle {} out of order or past the next id {}", r.id, save.next_id));
            }
            prev = r.id;
            let s = r.state;
            let fine = [s.pos.x, s.pos.y, s.heading, s.speed, s.steer, r.input.throttle, r.input.steer].iter().all(|v| v.is_finite());
            if !fine || !self.map.in_bounds(s.pos.floor().as_ivec2()) || s.speed.abs() > 100.0 || s.steer.abs() > 1.6 {
                return Err(format!("vehicle {}: impossible state {s:?}", r.id));
            }
            let Some(kind) = self.config.vehicles.by_id(&r.kind) else {
                dropped += 1;
                continue;
            };
            take_prop(&mut self.map, r.from);
            self.vehicles.list.push(Vehicle { id: r.id, kind: kind as u16, state: s, input: r.input.sanitized(), driver: r.driver, from: r.from });
        }
        self.vehicles.next_id = save.next_id;
        let boxes = self.vehicle_boxes();
        self.map.set_vehicles(boxes);
        Ok(dropped)
    }

    /// What the save keeps of the vehicles.
    pub(crate) fn vehicle_save(&self) -> VehicleSave {
        let kinds = &self.config.vehicles.kinds;
        VehicleSave {
            next_id: self.vehicles.next_id,
            list: self
                .vehicles
                .list
                .iter()
                .map(|v| VehicleRecord { id: v.id, kind: kinds.get(v.kind as usize).map_or_else(String::new, |k| k.id.clone()), state: v.state, input: v.input, driver: v.driver, from: v.from })
                .collect(),
        }
    }

    /// The drivers back in their seats after a load: each named driver a living player in no other.
    pub(crate) fn seat_drivers(&mut self) -> Result<(), String> {
        for p in &mut self.players {
            p.vehicle = None;
        }
        for v in &self.vehicles.list {
            let Some(d) = v.driver else { continue };
            let p = self.players.iter_mut().find(|p| p.id == d).ok_or_else(|| format!("vehicle {} is driven by player {d}, who isn't in the save", v.id))?;
            if p.dead || p.vehicle.is_some() {
                return Err(format!("vehicle {}: player {d} can't be at its wheel", v.id));
            }
            p.vehicle = Some(v.id);
        }
        for pi in 0..self.players.len() {
            if self.players[pi].vehicle.is_some() {
                self.refresh_carry(pi);
            }
        }
        Ok(())
    }

    /// The driver's input for this tick (`Intent::Drive`), held until the next. Ignored from
    /// anyone not at a wheel.
    pub fn player_drive(&mut self, id: u32, input: DriveInput) {
        let Some(vid) = self.players.iter().find(|p| p.id == id && !p.dead).and_then(|p| p.vehicle) else { return };
        if let Some(v) = self.vehicles.list.iter_mut().find(|v| v.id == vid) {
            v.input = input.sanitized();
        }
    }

    /// Player `id` gets into a vehicle or out of the one it is in. Refused, with a note, when it
    /// can't (and why). Returns what it did.
    pub fn player_vehicle(&mut self, id: u32, op: VehicleOp) -> Result<String, String> {
        let Some(pi) = self.players.iter().position(|p| p.id == id) else { return Err(format!("no player {id}")) };
        let out = match op {
            VehicleOp::Exit => self.get_out(pi),
            VehicleOp::Enter { vehicle } => self.get_in(pi, vehicle),
        };
        match &out {
            Ok(text) => self.note(id, true, text.clone()),
            Err(why) => self.note(id, false, why.clone()),
        }
        out
    }

    fn get_in(&mut self, pi: usize, vehicle: Option<u32>) -> Result<String, String> {
        let p = &self.players[pi];
        if p.vehicle.is_some() {
            return Err("already in a car".into());
        }
        let why = if p.dead {
            Some("dead")
        } else if p.needs.asleep() || p.away.is_some() {
            Some("asleep")
        } else if p.knock.is_some() {
            Some("still reeling")
        } else if p.restraint.is_some() {
            Some("your hands are tied")
        } else if p.state.climb.is_some() || p.state.level > 0 {
            Some("not from here")
        } else if p.action.is_some() {
            Some("busy fighting")
        } else {
            None
        };
        if let Some(why) = why {
            return Err(format!("can't get in: {why}"));
        }
        let tune = self.config.vehicles.tuning;
        let boxes = self.vehicle_boxes();
        let (pos, facing) = (p.state.pos, p.state.facing);
        let vi = match vehicle {
            Some(vid) => {
                let i = self.vehicles.list.iter().position(|v| v.id == vid).ok_or("no such car")?;
                if boxes.get(i).is_none_or(|b| b.distance(pos) > tune.enter_reach) {
                    return Err("can't get in: too far from it".into());
                }
                i
            }
            None => enterable(&boxes, pos, facing, tune.enter_reach).ok_or("no car to get into here")?,
        };
        let v = &self.vehicles.list[vi];
        if v.driver.is_some() {
            return Err("can't get in: someone is at the wheel".into());
        }
        if v.state.speed.abs() > tune.enter_max_speed {
            return Err("can't get in: it's moving".into());
        }
        let Some(kind) = self.vehicle_kind(v).cloned() else { return Err("no such car".into()) };
        let vid = v.id;
        let seat = seat_of(&kind, &v.state);
        let heading = v.state.heading;
        let v = &mut self.vehicles.list[vi];
        v.driver = Some(self.players[pi].id);
        v.input = DriveInput::PARKED;
        let p = &mut self.players[pi];
        p.vehicle = Some(vid);
        p.seated = None;
        p.queued = None;
        p.state.pos = seat;
        p.state.facing = heading;
        self.refresh_carry(pi);
        Ok(format!("you get into the {}", kind.label()))
    }

    fn get_out(&mut self, pi: usize) -> Result<String, String> {
        let Some(vid) = self.players[pi].vehicle else { return Err("not in a car".into()) };
        let Some(vi) = self.vehicles.list.iter().position(|v| v.id == vid) else {
            self.players[pi].vehicle = None;
            self.refresh_carry(pi);
            return Ok("you get out".into());
        };
        let v = &self.vehicles.list[vi];
        if v.state.speed.abs() > self.config.vehicles.tuning.exit_max_speed {
            return Err("can't get out: slow down first".into());
        }
        let kind = self.vehicle_kind(v).cloned().ok_or("no such car")?;
        let at = self.exit_spot(&kind, &v.state, vid).ok_or("can't get out: no room beside it")?;
        self.leave_vehicle(pi, Some(at));
        Ok(format!("you get out of the {}", kind.label()))
    }

    /// Where someone getting out of a vehicle stands: beside the driver's door, else the other
    /// side's, behind it or in front, whichever is clear of everything (the map, the other
    /// vehicles, this one).
    fn exit_spot(&self, kind: &VehicleKind, s: &VehicleState, vid: u32) -> Option<Vec2> {
        let fwd = s.forward();
        let left = Vec2::new(fwd.y, -fwd.x);
        let (f, _) = kind.driver_seat;
        let out = PLAYER_RADIUS + 0.15;
        let spots = [
            s.pos + fwd * f + left * (kind.width * 0.5 + out),
            s.pos + fwd * f - left * (kind.width * 0.5 + out),
            s.pos - fwd * (kind.length * 0.5 + out),
            s.pos + fwd * (kind.length * 0.5 + out),
        ];
        spots.into_iter().find(|&q| {
            let t = q.floor().as_ivec2();
            self.map.in_bounds(t)
                && !matches!(self.map.tile(t).feature, Feature::Tree | Feature::PineTree | Feature::Furniture | Feature::StreetProp)
                && self.map.resolve_circle_skip(q, PLAYER_RADIUS, 0).distance(q) < 0.02
                && self.map.vehicles.iter().filter(|b| b.id != vid).all(|b| b.distance(q) > PLAYER_RADIUS)
                && vision_clear(&self.map, s.pos + fwd * f, q)
        })
    }

    /// Player `pi` leaves the vehicle it is in, standing at `at` if given (else where it sits).
    pub(crate) fn leave_vehicle(&mut self, pi: usize, at: Option<Vec2>) {
        let Some(vid) = self.players[pi].vehicle.take() else { return };
        if let Some(v) = self.vehicles.list.iter_mut().find(|v| v.id == vid) {
            v.driver = None;
            v.input = DriveInput::PARKED;
        }
        if let Some(at) = at {
            self.players[pi].state.pos = at;
        }
        self.refresh_carry(pi);
    }

    /// Once a tick, before players move: drivers who can't drive let go; every vehicle that moves
    /// or is driven steps ([`step_vehicle`]); the map's boxes follow; what is in the way of a
    /// moving one is pushed aside; drivers are put in their seats.
    pub(crate) fn step_vehicles(&mut self, dt: f32) {
        if self.vehicles.list.is_empty() {
            return;
        }
        // Drivers who can't drive any more let go of the wheel (and get out if they can).
        for pi in 0..self.players.len() {
            let p = &self.players[pi];
            if p.vehicle.is_some() && (p.dead || p.needs.asleep() || p.away.is_some() || p.restraint.is_some()) {
                let at = self.vehicle_of(p.id).and_then(|v| self.vehicle_kind(v).map(|k| (k.clone(), v.state, v.id))).and_then(|(k, s, id)| self.exit_spot(&k, &s, id));
                self.leave_vehicle(pi, at);
            }
        }
        let bounce = self.config.vehicles.tuning.bounce;
        let mut moved = false;
        for vi in 0..self.vehicles.list.len() {
            let v = &self.vehicles.list[vi];
            if v.driver.is_none() && !v.state.moving() {
                continue;
            }
            let Some(kind) = self.config.vehicles.kinds.get(v.kind as usize) else { continue };
            let input = if v.driver.is_some() { v.input } else { DriveInput::PARKED };
            let (id, mut state) = (v.id, v.state);
            step_vehicle(&self.map, kind, id, &mut state, input, dt, bounce);
            if state != self.vehicles.list[vi].state {
                self.vehicles.list[vi].state = state;
                moved = true;
                // Its box moves now, so the next vehicle runs into it where it is.
                let b = self.vehicles.list[vi].boxed(kind);
                let mut boxes = self.map.vehicles.clone();
                if let Some(slot) = boxes.iter_mut().find(|o| o.id == id) {
                    *slot = b;
                }
                self.map.set_vehicles(boxes);
            }
        }
        if moved {
            self.push_aside();
        }
        // Drivers sit at the wheel, facing the way it does.
        for pi in 0..self.players.len() {
            let Some(vid) = self.players[pi].vehicle else { continue };
            let Some(v) = self.vehicles.list.iter().find(|v| v.id == vid) else { continue };
            let Some(kind) = self.config.vehicles.kinds.get(v.kind as usize) else { continue };
            let (seat, heading) = (seat_of(kind, &v.state), v.state.heading);
            let p = &mut self.players[pi];
            p.state.pos = seat;
            p.state.facing = heading;
        }
    }

    /// Zombies and survivors in the way of a moving vehicle are pushed out of its box; it loses a
    /// share of its speed for each zombie.
    fn push_aside(&mut self) {
        let slow = self.config.vehicles.tuning.zombie_slow;
        for vi in 0..self.vehicles.list.len() {
            let v = &self.vehicles.list[vi];
            if v.state.speed.abs() < 0.1 {
                continue;
            }
            let Some(kind) = self.config.vehicles.kinds.get(v.kind as usize) else { continue };
            let b = v.boxed(kind);
            let reach = b.half.length() + 1.0;
            let mut hits = 0;
            let z = &mut self.zombies;
            for i in 0..z.pos.len() {
                if z.state[i] == crate::zombies::ZombieState::Dead || z.tier[i] == crate::zombies::Tier::Virtual || z.pos[i].distance_squared(b.pos) > reach * reach {
                    continue;
                }
                let q = b.push_out(z.pos[i], crate::ai::ZOMBIE_RADIUS);
                if q != z.pos[i] {
                    z.pos[i] = self.map.resolve_circle_skip(q, crate::ai::ZOMBIE_RADIUS, b.id);
                    hits += 1;
                }
            }
            for p in self.players.iter_mut().filter(|p| p.vehicle.is_none() && !p.dead && p.state.level == 0) {
                if p.state.pos.distance_squared(b.pos) > reach * reach {
                    continue;
                }
                let q = b.push_out(p.state.pos, PLAYER_RADIUS);
                if q != p.state.pos {
                    p.state.pos = self.map.resolve_circle_skip(q, PLAYER_RADIUS, b.id);
                }
            }
            if hits > 0 {
                self.vehicles.list[vi].state.speed *= (1.0 - slow).powi(hits);
            }
        }
    }

    /// Mix the vehicles into the state hash (only called while there are any).
    pub(crate) fn hash_vehicles(&self, mix: &mut impl FnMut(u32)) {
        mix(0x5645_4843);
        mix(self.vehicles.next_id);
        for v in &self.vehicles.list {
            mix(v.id);
            mix(v.kind as u32 | (v.input.brake as u32) << 16);
            for f in [v.state.pos.x, v.state.pos.y, v.state.heading, v.state.speed, v.state.steer, v.input.throttle, v.input.steer] {
                mix(f.to_bits());
            }
            mix(v.driver.unwrap_or(0));
        }
    }
}

/// Nothing that blocks a body between `a` and `b` (a survivor climbing out reaches its spot).
fn vision_clear(map: &TileMap, a: Vec2, b: Vec2) -> bool {
    crate::vision::clear_reach(map, a, b)
}

/// Take the street prop whose least corner is `tile` off the map (its tiles free again).
pub fn take_prop(map: &mut TileMap, tile: IVec2) {
    let Ok(i) = map.street_props.binary_search_by_key(&(tile.y, tile.x), |p| (p.tile.y, p.tile.x)) else { return };
    let p = map.street_props.remove(i);
    let (min, size) = map.street_prop_rect(&p);
    for y in min.y..min.y + size.y {
        for x in min.x..min.x + size.x {
            if let Some(t) = map.tile_mut(IVec2::new(x, y))
                && t.feature == Feature::StreetProp
            {
                t.feature = Feature::None;
            }
        }
    }
}

/// The `VEHC` section.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct VehicleSave {
    pub next_id: u32,
    pub list: Vec<VehicleRecord>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VehicleRecord {
    pub id: u32,
    pub kind: String,
    pub state: VehicleState,
    pub input: DriveInput,
    pub driver: Option<u32>,
    pub from: IVec2,
}

#[cfg(test)]
mod tests;
