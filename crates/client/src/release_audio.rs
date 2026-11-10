//! Release audio (#154): sounds and music the game may ship in its builds but that we may not
//! publish as separate files under the repo's CC licence (ElevenLabs output, for one: its terms
//! forbid handing it out "as isolated audio files"). They live in the private art repo and a
//! release build carries them in a folder of their own, laid out like the game's assets:
//!
//! ```text
//! release-audio/
//!   audio/sfx/footstep_hard_1.wav    replaces assets/audio/sfx/footstep_hard_1.wav
//!   data/sounds.ron                  optional: replaces assets/data/sounds.ron (new variants, cues' numbers)
//!   data/music.ron                   optional: the same for music.ron
//! ```
//!
//! The folder is `$ZOMBYE_RELEASE_AUDIO`, else `release-audio` beside the game's binary. Without
//! one the game plays the public files: anyone can build and mod it from this repo alone.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The release audio folder, if there is one (looked for once).
pub fn root() -> Option<&'static Path> {
    static ROOT: OnceLock<Option<PathBuf>> = OnceLock::new();
    ROOT.get_or_init(|| {
        let found = std::env::var_os("ZOMBYE_RELEASE_AUDIO")
            .map(PathBuf::from)
            .or_else(|| std::env::current_exe().ok()?.parent().map(|d| d.join("release-audio")))
            .filter(|p| p.is_dir());
        if let Some(p) = &found {
            println!("release audio: {}", p.display());
        }
        found
    })
    .as_deref()
}

/// The data file `name` (`sounds.ron`, `music.ron`): the release folder's if it has one and the
/// game is using its own data, else the one in `data`.
pub fn data_file(overlay: Option<&Path>, data: &Path, own_data: bool, name: &str) -> PathBuf {
    if own_data {
        if let Some(p) = overlay.map(|o| o.join("data").join(name)).filter(|p| p.is_file()) {
            return p;
        }
    }
    data.join(name)
}

/// Read the audio file `file` (relative to `assets/audio`): the release folder's copy if it has
/// one, else the one in `audio`.
pub fn read(overlay: Option<&Path>, audio: &Path, file: &str) -> std::io::Result<Vec<u8>> {
    if let Some(p) = overlay.map(|o| o.join("audio").join(file)).filter(|p| p.is_file()) {
        return std::fs::read(p);
    }
    std::fs::read(audio.join(file))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_release_folder_replaces_files_it_has_and_only_those() {
        let dir = std::env::temp_dir().join(format!("zombye-release-audio-{}", std::process::id()));
        let (public, overlay) = (dir.join("public"), dir.join("overlay"));
        std::fs::create_dir_all(public.join("sfx")).unwrap();
        std::fs::create_dir_all(overlay.join("audio/sfx")).unwrap();
        std::fs::create_dir_all(overlay.join("data")).unwrap();
        std::fs::write(public.join("sfx/a.wav"), b"public a").unwrap();
        std::fs::write(public.join("sfx/b.wav"), b"public b").unwrap();
        std::fs::write(overlay.join("audio/sfx/a.wav"), b"release a").unwrap();
        std::fs::write(overlay.join("data/sounds.ron"), b"()").unwrap();
        let o = Some(overlay.as_path());
        assert_eq!(read(o, &public, "sfx/a.wav").unwrap(), b"release a");
        assert_eq!(read(o, &public, "sfx/b.wav").unwrap(), b"public b");
        assert_eq!(read(None, &public, "sfx/a.wav").unwrap(), b"public a");
        let data = dir.join("data");
        assert_eq!(data_file(o, &data, true, "sounds.ron"), overlay.join("data/sounds.ron"));
        assert_eq!(data_file(o, &data, true, "music.ron"), data.join("music.ron"));
        // A --data folder of its own (a mod's) keeps its own sounds.ron.
        assert_eq!(data_file(o, &data, false, "sounds.ron"), data.join("sounds.ron"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
