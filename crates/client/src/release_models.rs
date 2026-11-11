//! Release models: furniture and street prop models the game may ship in its builds but that we
//! may not publish in this repo (models from bought asset packs, whose licence allows them only
//! embedded in a game). Like release audio and release animation they live in the private art
//! repo and a release build carries them in a folder of their own:
//!
//! ```text
//! release-models/
//!   furniture/<kind>.glb    in place of assets/models/furniture/<kind>.glb
//!   street/<model>.glb      in place of assets/models/street/<model>.glb
//!   trees/<kind>.glb        in place of the generated trees: broadleaf, birch, pine, spruce, and bush
//!   items/<item id>.glb     an item lying on the ground, in place of its grey-box
//!   held/<item id>.glb      a weapon as the hand holds it, in place of its grey-box bar: the grip
//!                           at the origin, pointing along +z, its top up
//! ```
//!
//! `<name>.2.glb`, `<name>.3.glb`, ... beside any of them are variants, one picked per place. A tree
//! stands with its foot at the origin; its foliage is the triangles of a material named `leaves`.
//!
//! Each file is a model as `furniture_models.rs` reads them (facing south, origin at the middle of
//! its footprint on the floor, the footprint in the node's extras), named as the model it replaces;
//! a name the game has no model for is one more. Colours are the materials' base colours times the
//! vertex colours (`COLOR_0`), so a model painted from a texture atlas needs no texture.
//!
//! The folder is `$ZOMBYE_RELEASE_MODELS`, else `release-models` beside the game's binary. Without
//! one the game draws this repo's models: anyone can build and mod it from this repo alone.

use crate::furniture_models::FurnitureModels;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The release models folder, if there is one (looked for once).
pub fn root() -> Option<&'static Path> {
    static ROOT: OnceLock<Option<PathBuf>> = OnceLock::new();
    ROOT.get_or_init(|| {
        let found = std::env::var_os("ZOMBYE_RELEASE_MODELS")
            .map(PathBuf::from)
            .or_else(|| std::env::current_exe().ok()?.parent().map(|d| d.join("release-models")))
            .filter(|p| p.is_dir());
        if let Some(p) = &found {
            println!("release models: {}", p.display());
        }
        found
    })
    .as_deref()
}

/// The overlay's tree and bush models (`trees/`), read once; none without the folder.
pub fn trees() -> &'static FurnitureModels {
    static TREES: OnceLock<FurnitureModels> = OnceLock::new();
    TREES.get_or_init(|| match root() {
        Some(dir) => {
            let m = FurnitureModels::load(&dir.join("trees"));
            if m.len() > 0 {
                println!("release models: {} tree and bush models", m.len());
            }
            m
        }
        None => FurnitureModels::default(),
    })
}

/// The overlay's item models (`items/`, named by item id), read once; none without the folder.
pub fn items() -> &'static FurnitureModels {
    static ITEMS: OnceLock<FurnitureModels> = OnceLock::new();
    ITEMS.get_or_init(|| match root() {
        Some(dir) => {
            let m = FurnitureModels::load(&dir.join("items"));
            if m.len() > 0 {
                println!("release models: {} item models", m.len());
            }
            m
        }
        None => FurnitureModels::default(),
    })
}

/// The overlay's models of weapons in the hand (`held/`, named by item id), read once; none without
/// the folder.
pub fn held() -> &'static FurnitureModels {
    static HELD: OnceLock<FurnitureModels> = OnceLock::new();
    HELD.get_or_init(|| match root() {
        Some(dir) => {
            let m = FurnitureModels::load(&dir.join("held"));
            if m.len() > 0 {
                println!("release models: {} models of weapons in the hand", m.len());
            }
            m
        }
        None => FurnitureModels::default(),
    })
}

/// Put the overlay's models among `models`, replacing those of the same name. Returns how many it
/// held.
pub fn load_into(overlay: Option<&Path>, models: &mut FurnitureModels) -> usize {
    let Some(dir) = overlay else { return 0 };
    let mut n = 0;
    for sub in ["furniture", "street"] {
        let more = FurnitureModels::load(&dir.join(sub));
        n += more.len();
        models.extend(more);
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Checks a real release folder (`ZOMBYE_RELEASE_MODELS`): every file reads, trees have a trunk
    /// and foliage and stand on the origin, items are item-sized. Prints each model.
    #[test]
    #[ignore]
    fn check_the_release_folder() {
        let dir = PathBuf::from(std::env::var_os("ZOMBYE_RELEASE_MODELS").expect("set ZOMBYE_RELEASE_MODELS"));
        for sub in ["furniture", "street", "trees", "items", "held"] {
            let files = std::fs::read_dir(dir.join(sub)).map(|d| d.flatten().filter(|e| e.path().extension().is_some_and(|x| x == "glb")).count()).unwrap_or(0);
            let models = FurnitureModels::load(&dir.join(sub));
            assert_eq!(models.len(), files, "{sub}: a file didn't read");
            for (name, tris, leaves, size, colour) in models.report() {
                println!("{sub}/{name}: {tris} triangles ({leaves} foliage), {:.2} x {:.2} x {:.2} m, colour {:.2} {:.2} {:.2}", size.x, size.z, size.y, colour.x, colour.y, colour.z);
                assert!(tris > 0 && size.is_finite(), "{sub}/{name}");
                assert!(colour.min_element() >= 0.0 && colour.max_element() <= 1.01, "{sub}/{name}: colours");
                match sub {
                    "trees" => assert!(leaves > 0 && (leaves < tris || name.starts_with("bush")) && size.y > 0.5, "{sub}/{name}: trunk and foliage"),
                    "items" => assert!(size.max_element() <= 1.2 && leaves == 0, "{sub}/{name}: item-sized"),
                    "held" => assert!(size.z <= 1.5 && size.z >= size.x && size.z >= size.y && leaves == 0, "{sub}/{name}: lies along the hand's reach"),
                    "street" => assert!(size.y <= 4.5 && size.x.max(size.z) <= 10.0 && leaves == 0, "{sub}/{name}"),
                    _ => assert!(size.y <= 4.5 && leaves == 0, "{sub}/{name}"),
                }
            }
        }
    }

    #[test]
    fn the_release_folder_replaces_and_adds_models_by_name() {
        let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/models");
        let mut models = FurnitureModels::load(&assets.join("furniture"));
        let n = models.len();
        let dir = std::env::temp_dir().join(format!("zombye-release-models-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("furniture")).unwrap();
        std::fs::create_dir_all(dir.join("street")).unwrap();
        // A sofa under the bed's name, a model the game hasn't got, and a file that isn't one.
        std::fs::copy(assets.join("furniture/sofa.glb"), dir.join("furniture/bed.glb")).unwrap();
        std::fs::copy(assets.join("street/mailbox.glb"), dir.join("street/phone_box.glb")).unwrap();
        std::fs::write(dir.join("street/broken.glb"), b"not a glb").unwrap();
        assert_eq!(load_into(Some(&dir), &mut models), 2);
        assert_eq!(models.len(), n + 1);
        assert_eq!(models.triangles("bed"), models.triangles("sofa"));
        assert!(models.triangles("phone_box").is_some());
        assert_eq!(load_into(None, &mut models), 0);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
