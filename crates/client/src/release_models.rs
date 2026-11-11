//! Release models: furniture and street prop models the game may ship in its builds but that we
//! may not publish in this repo (models from bought asset packs, whose licence allows them only
//! embedded in a game). Like release audio and release animation they live in the private art
//! repo and a release build carries them in a folder of their own:
//!
//! ```text
//! release-models/
//!   furniture/<kind>.glb    in place of assets/models/furniture/<kind>.glb
//!   street/<model>.glb      in place of assets/models/street/<model>.glb
//! ```
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
