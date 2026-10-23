//! Weapons lying on the floor, drawn from the snapshot's full item list as grey-box bars in each
//! weapon's colour (real models come later). Hidden under the fog of war like zombies.

use std::collections::HashMap;

use bevy::prelude::*;

use crate::fog::FogState;
use crate::server_link::{JoinInfo, ServerLink};

/// Shared look of every weapon: one unit cube, one material per weapon (by index), and its length.
#[derive(Resource)]
pub struct WeaponLooks {
    pub cube: Handle<Mesh>,
    pub materials: Vec<Handle<StandardMaterial>>,
    pub lengths: Vec<f32>,
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
}

#[derive(Resource, Default)]
struct ItemEntities(HashMap<u32, (Entity, u16)>);

pub struct ItemsRenderPlugin;

impl Plugin for ItemsRenderPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ItemEntities>().add_systems(Startup, setup).add_systems(Update, sync_items);
    }
}

fn setup(mut commands: Commands, join: Res<JoinInfo>, mut meshes: ResMut<Assets<Mesh>>, mut materials: ResMut<Assets<StandardMaterial>>) {
    let cube = meshes.add(Cuboid::new(1.0, 1.0, 1.0));
    let mats = join
        .weapons
        .iter()
        .map(|w| materials.add(StandardMaterial { base_color: Color::srgb(w.color.0, w.color.1, w.color.2), perceptual_roughness: 0.6, ..default() }))
        .collect();
    let lengths = join.weapons.iter().map(|w| w.length).collect();
    commands.insert_resource(WeaponLooks { cube, materials: mats, lengths });
}

fn sync_items(mut commands: Commands, link: Res<ServerLink>, fog: Res<FogState>, looks: Option<Res<WeaponLooks>>, mut entities: ResMut<ItemEntities>, mut query: Query<(&mut Visibility, &mut Transform)>) {
    let Some(looks) = looks else { return };
    let items = &link.curr.items;
    // Gone (picked up): despawn. Swapped for another weapon in place: respawn with the new look.
    entities.0.retain(|id, (e, w)| {
        let keep = items.iter().any(|it| it.id == *id && it.weapon == *w);
        if !keep {
            commands.entity(*e).despawn();
        }
        keep
    });
    for it in items {
        let visibility = if fog.visibility(it.pos) > 0.35 { Visibility::Inherited } else { Visibility::Hidden };
        match entities.0.get(&it.id) {
            Some(&(e, _)) => {
                if let Ok((mut v, mut tf)) = query.get_mut(e) {
                    if *v != visibility {
                        *v = visibility;
                    }
                    // A same-kind swap moves it to where the player stood.
                    let at = Vec3::new(it.pos.x, tf.translation.y, it.pos.y);
                    if tf.translation != at {
                        tf.translation = at;
                    }
                }
            }
            None => {
                let len = looks.length(it.weapon).max(MIN_LENGTH);
                let tf = Transform::from_xyz(it.pos.x, THICK * 0.5 + 0.01, it.pos.y)
                    .with_rotation(Quat::from_rotation_y(-it.angle))
                    .with_scale(Vec3::new(len, THICK, THICK * 1.4));
                let material = looks.materials.get(it.weapon as usize).cloned().unwrap_or_default();
                let e = commands.spawn((Mesh3d(looks.cube.clone()), MeshMaterial3d(material), tf, visibility)).id();
                entities.0.insert(it.id, (e, it.weapon));
            }
        }
    }
}
