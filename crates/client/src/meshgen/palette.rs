//! The generated world's colours (ground, roads, trees, buildings, interiors, grey-box furniture),
//! from `assets/data/palette.ron` (#60): the game's one palette file, which the Blender assets read
//! too (`tools/blender/palette.py`). Every name the world uses is looked up once, when the client
//! starts ([`load`]), into a [`WorldPalette`] of linear colours that mesh building reads through
//! [`pal`]: a missing name or a malformed file is an error then, never a wrong colour later.
//! Without [`load`] (tests, tools) the shipped file is used.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::OnceLock;

use bevy::prelude::*;
use serde::Deserialize;

/// The shipped palette.
const SHIPPED: &str = include_str!("../../../../assets/data/palette.ron");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PaletteFile {
    /// Name -> ("#rrggbb" sRGB, roughness, metallic).
    colours: BTreeMap<String, (String, f32, f32)>,
    sets: BTreeMap<String, Vec<String>>,
}

/// `#rrggbb` -> linear colour.
fn hex(s: &str) -> Option<LinearRgba> {
    let h = s.strip_prefix('#').filter(|h| h.len() == 6)?;
    let byte = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
    Some(Color::srgb_u8(byte(0)?, byte(2)?, byte(4)?).to_linear())
}

macro_rules! world_palette {
    (colours { $($c:ident),* $(,)? } sets { $($s:ident: $n:expr),* $(,)? }) => {
        /// Every colour the generated world uses, by its name in the palette file, and the named
        /// sets (lists of colours, at least as long as the world indexes into).
        #[derive(Debug)]
        pub struct WorldPalette {
            $(pub $c: LinearRgba,)*
            $(pub $s: Vec<LinearRgba>,)*
            /// Wall colours by room type (`interior_<room type>`), and `interior_default`.
            interiors: BTreeMap<String, LinearRgba>,
        }

        impl WorldPalette {
            fn from_file(f: &PaletteFile) -> Result<Self, String> {
                let mut colours = BTreeMap::new();
                for (name, (h, rough, metal)) in &f.colours {
                    let c = hex(h).ok_or_else(|| format!("colour {name:?}: {h:?} isn't \"#rrggbb\""))?;
                    if !(0.0..=1.0).contains(rough) || !(0.0..=1.0).contains(metal) {
                        return Err(format!("colour {name:?}: roughness and metallic go from 0 to 1"));
                    }
                    colours.insert(name.as_str(), c);
                }
                let colour = |name: &str| colours.get(name).copied().ok_or_else(|| format!("no colour {name:?}"));
                let mut sets = BTreeMap::new();
                for (name, members) in &f.sets {
                    let list = members.iter().map(|m| colour(m).map_err(|e| format!("set {name:?}: {e}"))).collect::<Result<Vec<_>, _>>()?;
                    sets.insert(name.as_str(), list);
                }
                let set = |name: &str, at_least: usize| match sets.get(name) {
                    Some(list) if list.len() >= at_least => Ok(list.clone()),
                    Some(list) => Err(format!("set {name:?} has {} colours, at least {at_least} wanted", list.len())),
                    None => Err(format!("no set {name:?}")),
                };
                let interiors: BTreeMap<String, LinearRgba> = colours.iter().filter_map(|(k, v)| k.strip_prefix("interior_").map(|r| (r.to_string(), *v))).collect();
                if !interiors.contains_key("default") {
                    return Err("no colour \"interior_default\"".to_string());
                }
                Ok(Self { $($c: colour(stringify!($c))?,)* $($s: set(stringify!($s), $n)?,)* interiors })
            }
        }
    };
}

world_palette! {
    colours {
        // Ground and roads.
        asphalt, asphalt_patch, wheel_track, gutter, paint_yellow, paint_white, oil_stain, sidewalk, sidewalk_joint,
        kerb_top, kerb_face, kerb_joint, driveway, path, long_grass, verge, tuft, tuft_dry, dirt, soil, hedge, hedge_top,
        fence_wood, needles, rock, cliff, deadwood, cut_wood,
        // Instanced vegetation and litter (vegetation.rs).
        grass_blade, grass_blade_dry, worn_ground, flower_red, flower_cream, paper,
        // Trees.
        bark, bark_dark, birch_bark, birch_mark,
        // Buildings.
        trim_light, trim_dark, shopfront, foundation, step, deck, rain_gutter, chimney, chimney_cap, flue, aerial,
        roof_unit, coping, sign, bone, gable_glass, door_leaf, door_handle, window_glass, door_glass, blind, blind_rail, crack_door, crack_glass, splinter,
        floor_road_line,
        // Grey-box furniture.
        greybox_bed, greybox_bed_sheet, greybox_wardrobe, greybox_fridge, greybox_stove, greybox_counter, greybox_washer,
        greybox_table, greybox_coffee_table, greybox_sofa, greybox_sofa_back, greybox_tv_stand, greybox_tv_screen,
        greybox_toilet, greybox_shelf, greybox_workbench, greybox_workbench_top, greybox_bathtub, greybox_stairs, greybox_other,
    }
    sets {
        lawn: 3, flowers: 1, shrub: 1, forest_floor: 2, fern: 1, leaf: 2, birch_leaf: 1, pine: 1, spruce: 1,
        siding: 1, roof: 1, brick: 1, awning: 1,
        floor_grass: 1, floor_dirt: 1, floor_asphalt: 1, floor_sidewalk: 1, floor_path: 1, floor_wood: 1,
        floor_carpet: 1, floor_tile: 1, floor_concrete: 1,
    }
}

impl WorldPalette {
    /// The wall colour of a room of `room_type`.
    pub fn interior(&self, room_type: &str) -> LinearRgba {
        self.interiors.get(room_type).or_else(|| self.interiors.get("default")).copied().unwrap_or(LinearRgba::WHITE)
    }
}

/// The `i`th colour of a set, wrapping round (sets may be any length).
pub fn pick(set: &[LinearRgba], i: usize) -> LinearRgba {
    set[i % set.len()]
}

/// Between the first and last colour of a set, `t` (0..1) of the way.
pub fn between(set: &[LinearRgba], t: f32) -> LinearRgba {
    set[0].mix(&set[set.len() - 1], t)
}

/// Parse a palette file into the world's colours.
pub fn parse(text: &str) -> Result<WorldPalette, String> {
    let file: PaletteFile = ron::from_str(text).map_err(|e| e.to_string())?;
    WorldPalette::from_file(&file)
}

static PALETTE: OnceLock<WorldPalette> = OnceLock::new();

/// The world's colours: the ones [`load`] read, else the shipped palette.
pub fn pal() -> &'static WorldPalette {
    PALETTE.get_or_init(|| parse(SHIPPED).expect("the shipped palette.ron is good (a test checks)"))
}

/// Use `palette.ron` from the data folder `dir` (the shipped one if it has none). Call before
/// anything is drawn: once colours have been read, the palette can't change.
pub fn load(dir: &Path) -> Result<(), String> {
    let path = dir.join("palette.ron");
    let palette = match std::fs::read_to_string(&path) {
        Ok(text) => parse(&text).map_err(|e| format!("{}: {e}", path.display()))?,
        Err(_) => parse(SHIPPED).expect("the shipped palette.ron is good (a test checks)"),
    };
    PALETTE.set(palette).map_err(|_| "the palette was already in use".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shipped_palette_has_every_colour_the_world_uses() {
        let p = parse(SHIPPED).unwrap();
        assert_eq!(p.siding.len(), 8);
        assert_ne!(p.interior("kitchen"), p.interior("no such room"));
        assert_eq!(p.interior("no such room"), p.interior("default"));
    }

    #[test]
    fn mistakes_are_named() {
        let bad = SHIPPED.replacen("\"asphalt\": (\"#", "\"asphalt\": (\"x", 1);
        assert!(parse(&bad).unwrap_err().contains("asphalt"));
        let missing = SHIPPED.replacen("\"kerb_joint\":", "\"kerb_joint_x\":", 1);
        assert!(parse(&missing).unwrap_err().contains("kerb_joint"));
        let short = SHIPPED.replacen("\"lawn\": [\"lawn_1\", ", "\"lawn\": [", 1);
        assert!(parse(&short).unwrap_err().contains("lawn"));
    }
}
