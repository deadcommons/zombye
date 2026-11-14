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

use crate::survivor_model::Hold;

/// How a hand carries a thing while nothing else has the arm (#35): standing, walking, running
/// and sneaking; a guard, a blow, eating, drinking, a reload and the like take the arm from it
/// (`survivor_model::SurvivorModel::carry`).
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
pub enum Carry {
    /// The arm hangs and swings as the body's clips have it: a knife, a bat, a rifle at one's side.
    #[default]
    Side,
    /// The elbow bent, the forearm forward at the waist, the fist in front of the body with the
    /// thing upright in it: a mug, a tin, a bottle, a candle.
    Front,
    /// Low and ready at the side: the elbow bent less, the forearm forward and down 30 degrees, clear
    /// of the thigh. A pistol's barrel points that way; a flashlight is turned in the fist to shine
    /// ahead.
    Low,
}

/// One item's grip: metres to move it in the hand's frame (+Z along the item, +Y its top, +X out
/// of the right palm and out of the back of the left hand), degrees to turn it about the hand's X, Y and Z (in that order), and how
/// much to scale it. And how it is carried at rest (`carry`): left out, a weapon at the side and
/// anything else in front ([`carry`]).
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct Grip {
    pub offset: (f32, f32, f32),
    pub turn: (f32, f32, f32),
    pub scale: f32,
    #[serde(deserialize_with = "some_carry")]
    pub carry: Option<Carry>,
    /// How the hand holds it (`hold: Tool`): left out, a gun by its grip and any other weapon or
    /// thing with a held model by its handle, pointing forward and down at rest.
    #[serde(deserialize_with = "some_hold")]
    pub hold: Option<Hold>,
}

/// `carry: Front` in the file, not `carry: Some(Front)`.
fn some_carry<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Carry>, D::Error> {
    Carry::deserialize(d).map(Some)
}

/// `hold: Tool` in the file, not `hold: Some(Tool)`.
fn some_hold<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Hold>, D::Error> {
    Hold::deserialize(d).map(Some)
}

impl Default for Grip {
    fn default() -> Self {
        Self { offset: (0.0, 0.0, 0.0), turn: (0.0, 0.0, 0.0), scale: 1.0, carry: None, hold: None }
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

/// How item `id` is carried at rest: what its grip says, else by what it is: a weapon at the side,
/// anything else in front of the body.
pub fn carry(id: &str, weapon: bool) -> Carry {
    carry_of(grip(id), weapon)
}

fn carry_of(grip: Grip, weapon: bool) -> Carry {
    grip.carry.unwrap_or(if weapon { Carry::Side } else { Carry::Front })
}

/// The grips (not items) put on top of the thing in use at the mouth, in the left hand's frame:
/// food being eaten and a drink being drunk. The clips raise the fist to the face; these move what
/// is in it to the lips (#35).
pub const EATING: &str = "eating";
pub const DRINKING: &str = "drinking";

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

    /// How a thing is carried at rest is data (#35): a weapon at the side and anything else in
    /// front unless its grip says otherwise, and the shipped file has the pistol and the flashlight
    /// low and ready and a log at the side.
    #[test]
    fn carries_are_data_with_a_default_by_what_the_thing_is() {
        assert_eq!(carry_of(Grip::default(), true), Carry::Side);
        assert_eq!(carry_of(Grip::default(), false), Carry::Front);
        let grips = parse(r#"{"mug": (carry: Side), "bat": (carry: Front, scale: 2.0)}"#).unwrap();
        assert_eq!(carry_of(grips["mug"], false), Carry::Side);
        assert_eq!(carry_of(grips["bat"], true), Carry::Front);
        assert!(parse(r#"{"mug": (carry: Sideways)}"#).is_err());
        let shipped = shipped();
        let of = |id: &str, weapon| carry_of(shipped.get(id).copied().unwrap_or_default(), weapon);
        assert_eq!(of("pistol", true), Carry::Low);
        assert_eq!(of("flashlight", false), Carry::Low);
        assert_eq!(of("log", false), Carry::Side);
        assert_eq!(of("mug", false), Carry::Front);
        assert_eq!(of("kitchen_knife", true), Carry::Side);
    }

    /// How a handle is held is data too (#65): a tool with its head up, said by its grip. The
    /// shipped file has the claw hammer, the hatchet, the frying pan and the crowbar so, carried low and ready,
    /// and the hammer turned to face its striking face forward; a knife says nothing and stays as
    /// it was.
    #[test]
    fn holds_are_data() {
        let grips = parse(r#"{"mallet": (hold: Tool), "bat": (hold: Handle, carry: Side)}"#).unwrap();
        assert_eq!((grips["mallet"].hold, grips["bat"].hold), (Some(Hold::Tool), Some(Hold::Handle)));
        assert!(parse(r#"{"mallet": (hold: Upright)}"#).is_err());
        let shipped = shipped();
        for id in ["claw_hammer", "hatchet", "frying_pan", "crowbar"] {
            assert_eq!((shipped[id].hold, shipped[id].carry), (Some(Hold::Tool), Some(Carry::Low)), "{id}");
        }
        assert_eq!(shipped["claw_hammer"].turn, (0.0, 0.0, 180.0));
        assert!(shipped.get("kitchen_knife").is_none_or(|g| g.hold.is_none()));
        for id in ["baseball_bat", "golf_club", "wooden_plank", "fire_axe"] {
            assert!(shipped.get(id).is_none_or(|g| g.hold.is_none() && g.carry.is_none()), "{id} trails at the side");
        }
    }

    #[test]
    fn a_grip_moves_then_turns_the_item() {
        let g = Grip { offset: (0.0, 0.0, 0.1), turn: (0.0, 90.0, 0.0), scale: 2.0, carry: None, hold: None };
        let p = g.transform().transform_point(Vec3::Z);
        assert!((p - Vec3::new(2.0, 0.0, 0.1)).length() < 1e-5, "{p}");
    }
}
