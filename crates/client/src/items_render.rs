//! Items lying on the floor, drawn from the snapshot's full item list as grey-box shapes in each
//! item's colour: weapons as bars of their length, anything else as a small box about its size.
//! An item a release build has a model for (`release_models.rs`, `items/<item id>.glb`) is drawn as
//! that, at its own size. Hidden under the fog of war like zombies.

use std::collections::HashMap;

use bevy::prelude::*;
use zombye_sim::ItemStack;

use crate::fog::FogState;
use crate::server_link::{JoinInfo, ServerLink};

/// Shared look of every weapon (the held one too): one unit cube, one material per weapon (by
/// index), and its length.
#[derive(Resource)]
pub struct WeaponLooks {
    pub cube: Handle<Mesh>,
    pub materials: Vec<Handle<StandardMaterial>>,
    pub lengths: Vec<f32>,
    /// One material per item (by index), and the size (x, y, z) it's drawn at on the floor.
    item_materials: Vec<Handle<StandardMaterial>>,
    item_sizes: Vec<Vec3>,
    /// A release build's model of an item (by index), where it has one, and the material they share.
    item_models: Vec<Option<Handle<Mesh>>>,
    model_material: Handle<StandardMaterial>,
    /// A release build's model of a weapon in the hand (by weapon index), where it has one.
    held_models: Vec<Option<Handle<Mesh>>>,
}

/// Bar thickness and the shortest bar drawn (so a frying pan still shows up).
const THICK: f32 = 0.06;
const MIN_LENGTH: f32 = 0.2;

impl WeaponLooks {
    /// Drawn length of weapon `w` (0 for bare hands).
    pub fn length(&self, w: u16) -> f32 {
        let l = self.lengths.get(w as usize).copied().unwrap_or(0.0);
        if l > 0.0 { l.max(MIN_LENGTH) } else { 0.0 }
    }

    /// The model drawn for weapon `w` in a hand (its grip at the origin, pointing along +z), with
    /// its material, where a release build has one; else the weapon is a bar.
    pub fn held(&self, w: u16) -> Option<(Handle<Mesh>, Handle<StandardMaterial>)> {
        Some((self.held_models.get(w as usize)?.clone()?, self.model_material.clone()))
    }
}

#[derive(Resource, Default)]
struct ItemEntities(HashMap<u32, (Entity, ItemStack)>);

pub struct ItemsRenderPlugin;

impl Plugin for ItemsRenderPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ItemEntities>().add_systems(Startup, setup).add_systems(Update, sync_items);
    }
}

fn setup(mut commands: Commands, join: Res<JoinInfo>, mut meshes: ResMut<Assets<Mesh>>, mut materials: ResMut<Assets<StandardMaterial>>) {
    let cube = meshes.add(Cuboid::new(1.0, 1.0, 1.0));
    let mut mat = |c: (f32, f32, f32)| materials.add(StandardMaterial { base_color: Color::srgb(c.0, c.1, c.2), perceptual_roughness: 0.6, ..default() });
    let weapon_mats = join.weapons.iter().map(|w| mat(w.color)).collect();
    let item_materials = join.items.iter().map(|d| mat(d.color)).collect();
    let lengths: Vec<f32> = join.weapons.iter().map(|w| w.length).collect();
    let item_sizes = join
        .items
        .iter()
        .map(|d| {
            let weapon = d.weapon.as_ref().and_then(|id| join.weapons.iter().position(|w| &w.id == id));
            match weapon {
                Some(w) => Vec3::new(lengths[w].max(MIN_LENGTH), THICK, THICK * 1.4),
                None => {
                    // A box of about its volume, flattened a little, never too small to see.
                    let side = (d.volume_l / 1000.0).cbrt().clamp(0.08, 0.45);
                    Vec3::new(side * 1.2, side * 0.7, side)
                }
            }
        })
        .collect();
    let released = crate::release_models::items();
    let item_models = join.items.iter().map(|d| released.mesh(&d.id).map(|m| meshes.add(m))).collect();
    let in_hand = crate::release_models::held();
    let held_models = join.weapons.iter().map(|w| join.items.iter().find(|d| d.weapon.as_deref() == Some(w.id.as_str())).and_then(|d| in_hand.mesh(&d.id)).map(|m| meshes.add(m))).collect();
    let model_material = materials.add(StandardMaterial { base_color: Color::WHITE, perceptual_roughness: 0.8, ..default() });
    commands.insert_resource(WeaponLooks { cube, materials: weapon_mats, lengths, item_materials, item_sizes, item_models, model_material, held_models });
}

fn sync_items(mut commands: Commands, link: Res<ServerLink>, fog: Res<FogState>, view: Res<crate::player::ViewFloor>, looks: Option<Res<WeaponLooks>>, mut entities: ResMut<ItemEntities>, mut query: Query<(&mut Visibility, &mut Transform)>) {
    let Some(looks) = looks else { return };
    let items = &link.curr.items;
    // Gone (picked up) or changed into another item: despawn (and respawn with the new look).
    entities.0.retain(|id, (e, s)| {
        let keep = items.iter().any(|it| it.id == *id && it.stack.item == s.item);
        if !keep {
            commands.entity(*e).despawn();
        }
        keep
    });
    for it in items {
        // Things lying upstairs (stairs) are drawn on that floor, and only while it is the one open.
        let up = link.curr.upstairs.binary_search(&it.id).is_ok();
        let lift = if up { zombye_sim::map::STOREY } else { 0.0 };
        let visibility = if fog.visibility(it.pos) > 0.35 && (!up || view.level > 0) { Visibility::Inherited } else { Visibility::Hidden };
        match entities.0.get(&it.id) {
            Some(&(e, _)) => {
                if let Ok((mut v, mut tf)) = query.get_mut(e) {
                    if *v != visibility {
                        *v = visibility;
                    }
                    let at = Vec3::new(it.pos.x, tf.translation.y, it.pos.y);
                    if tf.translation != at {
                        tf.translation = at;
                    }
                }
            }
            None => {
                let e = match looks.item_models.get(it.stack.item as usize).cloned().flatten() {
                    // A model lies on the floor at its own size.
                    Some(model) => {
                        let tf = Transform::from_xyz(it.pos.x, 0.01 + lift, it.pos.y).with_rotation(Quat::from_rotation_y(-it.angle));
                        commands.spawn((Mesh3d(model), MeshMaterial3d(looks.model_material.clone()), tf, visibility)).id()
                    }
                    None => {
                        let size = looks.item_sizes.get(it.stack.item as usize).copied().unwrap_or(Vec3::splat(0.15));
                        let tf = Transform::from_xyz(it.pos.x, size.y * 0.5 + 0.01 + lift, it.pos.y).with_rotation(Quat::from_rotation_y(-it.angle)).with_scale(size);
                        let material = looks.item_materials.get(it.stack.item as usize).cloned().unwrap_or_default();
                        commands.spawn((Mesh3d(looks.cube.clone()), MeshMaterial3d(material), tf, visibility)).id()
                    }
                };
                entities.0.insert(it.id, (e, it.stack));
            }
        }
    }
}
