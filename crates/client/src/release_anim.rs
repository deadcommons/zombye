//! Release animation: character clips the game may ship in its builds but that we may not publish
//! in this repo (clips retargeted from animation packs whose licence allows them only embedded in
//! a game, never handed out as files). Like release audio (`release_audio.rs`) they live in the
//! private art repo and a release build carries them in a folder of their own:
//!
//! ```text
//! release-anim/
//!   characters/survivor_01/*.glb    clips for assets/models/characters/survivor_01.glb
//! ```
//!
//! Each file holds the body's skeleton (nodes named as its bones) and clips, no mesh
//! (`tools/blender/zretarget.py --clips`). Files are read in name order; a clip named as one the
//! body has replaces it, any other is one more (`guard_two`, `swing_two`, ...: see the names
//! `survivor_model.rs` plays). A body is matched by its file's name, so a `--survivor` body of
//! another name takes none.
//!
//! The folder is `$ZOMBYE_RELEASE_ANIM`, else `release-anim` beside the game's binary. Without one
//! the game plays the public clips: anyone can build and mod it from this repo alone.

use crate::character::{Clip, Rig, parse_clips};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The release animation folder, if there is one (looked for once).
pub fn root() -> Option<&'static Path> {
    static ROOT: OnceLock<Option<PathBuf>> = OnceLock::new();
    ROOT.get_or_init(|| {
        let found = std::env::var_os("ZOMBYE_RELEASE_ANIM")
            .map(PathBuf::from)
            .or_else(|| std::env::current_exe().ok()?.parent().map(|d| d.join("release-anim")))
            .filter(|p| p.is_dir());
        if let Some(p) = &found {
            println!("release animation: {}", p.display());
        }
        found
    })
    .as_deref()
}

/// Put `more` among `clips`: a clip of a name already there (case ignored) replaces it. Returns how
/// many were replaced and how many added.
pub fn merge(clips: &mut Vec<Clip>, more: Vec<Clip>) -> (usize, usize) {
    let (mut replaced, mut added) = (0, 0);
    for clip in more {
        match clips.iter_mut().find(|c| c.name.eq_ignore_ascii_case(&clip.name)) {
            Some(old) => {
                *old = clip;
                replaced += 1;
            }
            None => {
                clips.push(clip);
                added += 1;
            }
        }
    }
    (replaced, added)
}

/// Read the overlay's clip files for the body named `body` into `clips`. Returns a line a file, to
/// print: what it gave, or why it was left out (a bad file never stops the game).
pub fn load_into(overlay: Option<&Path>, body: &str, rig: &Rig, clips: &mut Vec<Clip>) -> Vec<String> {
    let Some(dir) = overlay.map(|o| o.join("characters").join(body)).filter(|d| d.is_dir()) else { return Vec::new() };
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("glb"))).collect();
    files.sort();
    files
        .iter()
        .map(|file| match std::fs::read(file).map_err(|e| e.to_string()).and_then(|b| parse_clips(&b, rig)) {
            Ok(more) => {
                let names: Vec<String> = more.iter().map(|c| c.name.clone()).collect();
                let (replaced, added) = merge(clips, more);
                format!("release animation {}: {replaced} replaced, {added} added ({})", file.display(), names.join(", "))
            }
            Err(e) => format!("release animation {}: left out: {e}", file.display()),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::character::parse_body;

    #[test]
    fn the_release_folder_replaces_and_adds_clips_by_name() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/models/characters/survivor_01.glb");
        let bytes = std::fs::read(path).unwrap();
        let body = parse_body(&bytes).unwrap();
        // A body's own file read as a clips file gives its clips back.
        let own = parse_clips(&bytes, &body.rig).unwrap();
        assert_eq!(own.len(), body.clips.len());
        let walk = body.clip("walk_alt").unwrap();
        let again = own.iter().find(|c| c.name == "walk_alt").unwrap();
        assert_eq!(walk.frames.len(), again.frames.len());
        assert!(walk.frames[3].r.iter().zip(&again.frames[3].r).all(|(a, b)| a.abs_diff_eq(*b, 1e-5)));

        let dir = std::env::temp_dir().join(format!("zombye-release-anim-{}", std::process::id()));
        let folder = dir.join("characters/survivor_01");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("a.glb"), &bytes).unwrap();
        std::fs::write(folder.join("b.glb"), b"not a glb").unwrap();
        // The game's clips, short of one and with one renamed: the folder puts one back and replaces the rest.
        let mut clips = body.clips.clone();
        clips.retain(|c| c.name != "punch");
        clips.iter_mut().find(|c| c.name == "idle").unwrap().name = "IDLE".into();
        let n = clips.len();
        let lines = load_into(Some(&dir), "survivor_01", &body.rig, &mut clips);
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines[0].contains(&format!("{n} replaced, 1 added")), "{lines:?}");
        assert!(lines[1].contains("left out"), "{lines:?}");
        assert_eq!(clips.len(), n + 1);
        assert!(clips.iter().any(|c| c.name == "punch"));
        // Another body's name, or no folder: nothing.
        assert!(load_into(Some(&dir), "someone_else", &body.rig, &mut clips).is_empty());
        assert!(load_into(None, "survivor_01", &body.rig, &mut clips).is_empty());
        // A file for another skeleton is refused.
        let other = Rig { bones: body.rig.bones.iter().map(|b| { let mut b = b.clone(); b.name = format!("x_{}", b.name); b }).collect(), ..body.rig.clone() };
        assert!(parse_clips(&bytes, &other).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
