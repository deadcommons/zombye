//! How each item sits in a survivor's hand (#35): `assets/data/held.ron`, with the release models'
//! `held/grips.ron` over it (its grips go with its models).
//!
//! A held item is drawn in the hand's grip frame: its grip at the origin on the palm, pointing along
//! +Z, its top +Y (`items_render::WeaponLooks::in_hand`; the frame is `survivor_model::Hand`'s). A grip moves, turns and scales the item in that frame,
//! after the item is put there, so a model that sits wrong in the fist is put right with data.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::OnceLock;

use bevy::prelude::*;
use serde::Deserialize;

/// One item's grip: metres to move it in the hand's frame (+Z along the item, +Y its top, +X out
/// of the right palm and out of the back of the left hand), degrees to turn it about the hand's X, Y and Z (in that order), and how
/// much to scale it.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct Grip {
    pub offset: (f32, f32, f32),
    pub turn: (f32, f32, f32),
    pub scale: f32,
}

impl Default for Grip {
    fn default() -> Self {
        Self { offset: (0.0, 0.0, 0.0), turn: (0.0, 0.0, 0.0), scale: 1.0 }
    }
}

impl Grip {
    /// The transform the grip puts on an item already in the hand.
    pub fn transform(&self) -> Transform {
        let (x, y, z) = self.turn;
        Transform::from_translation(Vec3::new(self.offset.0, self.offset.1, self.offset.2))
            .with_rotation(Quat::from_euler(EulerRot::XYZ, x.to_radians(), y.to_radians(), z.to_radians()))
            .with_scale(Vec3::splat(self.scale))
    }
}

/// Grips by item id.
pub type Grips = BTreeMap<String, Grip>;

const SHIPPED: &str = include_str!("../../../assets/data/held.ron");
static GRIPS: OnceLock<Grips> = OnceLock::new();

/// Read a grips file, checking every value.
pub fn parse(text: &str) -> Result<Grips, String> {
    let grips: Grips = ron::from_str(text).map_err(|e| e.to_string())?;
    for (id, g) in &grips {
        let (a, b, c) = g.offset;
        if ![a, b, c].iter().all(|v| v.is_finite() && v.abs() <= 1.0) {
            return Err(format!("{id}: offset must be within a metre"));
        }
        let (a, b, c) = g.turn;
        if ![a, b, c].iter().all(|v| v.is_finite() && v.abs() <= 360.0) {
            return Err(format!("{id}: turn must be within 360 degrees"));
        }
        if !(g.scale.is_finite() && (0.1..=10.0).contains(&g.scale)) {
            return Err(format!("{id}: scale must be from 0.1 to 10"));
        }
    }
    Ok(grips)
}

/// Use `held.ron` from the data folder `dir` (the shipped one if it has none), with the release
/// models' `held/grips.ron` over it. Call before the first frame; later calls change nothing.
pub fn load(dir: &Path) -> Result<(), String> {
    let path = dir.join("held.ron");
    let mut grips = match std::fs::read_to_string(&path) {
        Ok(text) => parse(&text).map_err(|e| format!("{}: {e}", path.display()))?,
        Err(_) => shipped(),
    };
    if let Some(root) = crate::release_models::root() {
        let path = root.join("held").join("grips.ron");
        if let Ok(text) = crate::pack::read_to_string(&path) {
            let over = parse(&text).map_err(|e| format!("{}: {e}", path.display()))?;
            println!("release models: {} grips of things in the hand", over.len());
            grips.extend(over);
        }
    }
    let _ = GRIPS.set(grips);
    Ok(())
}

fn shipped() -> Grips {
    parse(SHIPPED).expect("the shipped held.ron is good (a test checks)")
}

/// The grip of item `id`: the loaded file's, else the default (as put in the hand).
pub fn grip(id: &str) -> Grip {
    GRIPS.get_or_init(shipped).get(id).copied().unwrap_or_default()
}

/// Whether the grips name item `id`.
pub fn has(id: &str) -> bool {
    GRIPS.get_or_init(shipped).contains_key(id)
}

/// Food being eaten without a grip of its own: on the back of the left hand, in front of the mouth
/// rather than in the face (the eating clip lays the palm on the mouth), and a little bigger so it
/// reads (tuned on the tin of beans, #35).
pub const EATING: Grip = Grip { offset: (0.12, 0.0, -0.02), turn: (0.0, 0.0, 0.0), scale: 1.3 };

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shipped_grips_read() {
        shipped();
    }

    #[test]
    fn grips_are_checked() {
        assert!(parse(r#"{"bat": (offset: (0.0, 0.0, 0.1), turn: (0.0, 90.0, 0.0))}"#).is_ok());
        assert!(parse(r#"{"bat": (offset: (0.0, 0.0, 3.0))}"#).is_err());
        assert!(parse(r#"{"bat": (scale: 0.0)}"#).is_err());
        assert!(parse(r#"{"bat": (spin: 1.0)}"#).is_err());
    }

    #[test]
    fn a_grip_moves_then_turns_the_item() {
        let g = Grip { offset: (0.0, 0.0, 0.1), turn: (0.0, 90.0, 0.0), scale: 2.0 };
        let p = g.transform().transform_point(Vec3::Z);
        assert!((p - Vec3::new(2.0, 0.0, 0.1)).length() < 1e-5, "{p}");
    }
}
