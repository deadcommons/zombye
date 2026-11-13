//! Vehicles on screen (`zombye_sim::vehicles`): each car the snapshot lists is an entity of its
//! own, its street prop's model (the same one a parked prop of its kind draws, worn as
//! `decay.ron` says) or a grey-box of the prop's size and colour, drawn with the props' world
//! material (fog of war, lighting, shadows). Our own car is drawn where we predict it
//! (`player::LocalPlayer::car`), the others between the last two snapshots. A car someone else
//! drives is shown only where our survivor could see it, as other players are; a parked one
//! always, as the props are.

use std::collections::HashMap;

use bevy::app::RunFixedMainLoopSystems;
use bevy::prelude::*;
use zombye_sim::map::Side;

use crate::fog::FogState;
use crate::materials::WorldMaterials;
use crate::meshgen::MeshBuilder;
use crate::player::LocalPlayer;
use crate::server_link::{ClientMap, JoinInfo, ServerLink};
use crate::world_render::SharedMeshes;
use crate::zombie_render::lerp_angle;

/// A vehicle's body, by its id.
#[derive(Component)]
struct CarBody(u32);

/// Each vehicle's mesh, made once (its wear goes by where it was parked).
#[derive(Resource, Default)]
struct CarMeshes(HashMap<u32, Handle<Mesh>>);

pub struct VehiclesRenderPlugin;

impl Plugin for VehiclesRenderPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CarMeshes>().add_systems(RunFixedMainLoop, draw_vehicles.in_set(RunFixedMainLoopSystems::AfterFixedMainLoop));
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
            Some(c) => (c.prev.pos.lerp(c.curr.pos, alpha), lerp_angle(c.prev.heading, c.curr.heading, alpha)),
            None => {
                let prev = link.prev.vehicles.iter().find(|p| p.id == v.id).map_or(v.state, |p| p.state);
                (prev.pos.lerp(v.state.pos, alpha), lerp_angle(prev.heading, v.state.heading, alpha))
            }
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
