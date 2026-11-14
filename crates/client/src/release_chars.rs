//! Release characters: character models the game may ship in its builds but that we may not
//! publish in this repo (models from packs whose licence allows them only embedded in a game,
//! never handed out as files). Like release animation (`release_anim.rs`) they live in the private
//! art repo and a release build carries them in a folder of their own:
//!
//! ```text
//! release-characters/
//!   survivor/survivor_01.glb      the survivors' body (as assets/models/characters/survivor_01.glb)
//!   survivor/wearables/*.glb      its wearables, replacing the game's of the same names
//!   crowd/*.zvat (+ textures)     the zombie crowd's outfit variants
//! ```
//!
//! The folder is `$ZOMBYE_RELEASE_CHARACTERS`, else `release-characters` beside the game's binary (or in its pack), else, run from a checkout, that
//! folder in the private art repo's checkout (`pack::art_repo`: `$ZOMBYE_ART`, else `zombye-art` beside
//! this checkout, else `~/games/zombye-art`).
//! `--survivor` / `ZOMBYE_SURVIVOR` and `--crowd-vat` / `ZOMBYE_CROWD_VAT` win over it (`none`
//! included). Without it the game draws this repo's characters: anyone can build and mod it from
//! this repo alone. An offscreen render (`--render-to`) reads it too, so pictures show the game as
//! it ships.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The release characters folder, if there is one (looked for once).
pub fn root() -> Option<&'static Path> {
    static ROOT: OnceLock<Option<PathBuf>> = OnceLock::new();
    ROOT.get_or_init(|| {
        crate::pack::release_dir("ZOMBYE_RELEASE_CHARACTERS", "release-characters")
    })
    .as_deref()
}

/// The survivors' body named `body` (a file stem) in `overlay`, if it holds one.
pub fn survivor_in(overlay: Option<&Path>, body: &str) -> Option<PathBuf> {
    overlay.map(|o| o.join("survivor").join(format!("{body}.glb"))).filter(|p| crate::pack::is_file(p))
}

/// The crowd's folder in `overlay`, if it holds one.
pub fn crowd_in(overlay: Option<&Path>) -> Option<PathBuf> {
    overlay.map(|o| o.join("crowd")).filter(|p| crate::pack::is_dir(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_overlay_gives_its_survivor_and_crowd_and_nothing_else() {
        let dir = std::env::temp_dir().join(format!("zombye-release-chars-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(survivor_in(Some(&dir), "survivor_01"), None);
        assert_eq!(crowd_in(Some(&dir)), None);
        std::fs::create_dir_all(dir.join("survivor")).unwrap();
        std::fs::create_dir_all(dir.join("crowd")).unwrap();
        assert_eq!(survivor_in(Some(&dir), "survivor_01"), None, "a folder with no body file");
        std::fs::write(dir.join("survivor/survivor_01.glb"), b"x").unwrap();
        assert_eq!(survivor_in(Some(&dir), "survivor_01"), Some(dir.join("survivor/survivor_01.glb")));
        assert_eq!(survivor_in(Some(&dir), "another"), None);
        assert_eq!(crowd_in(Some(&dir)), Some(dir.join("crowd")));
        assert_eq!(survivor_in(None, "survivor_01"), None);
        assert_eq!(crowd_in(None), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
