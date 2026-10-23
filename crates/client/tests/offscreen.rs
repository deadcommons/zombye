//! The offscreen render (`zombye --render-to`) on a small saved world: it writes a PNG of the
//! asked-for size, with a real picture in it, and never needs a window.
//!
//! This needs a GPU (any adapter wgpu can use). Set `ZOMBYE_SKIP_GPU_TESTS=1` to skip it; on a
//! machine where the client finds no GPU adapter it skips itself (with a note on stderr) instead of
//! failing.

use std::path::{Path, PathBuf};
use std::process::Command;

use zombye_net::{content, save_file};
use zombye_sim::World;

/// A folder of its own for one test, removed when dropped (whether it passes, fails or skips).
struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn scratch(name: &str) -> Scratch {
    let dir = std::env::temp_dir().join(format!("zombye-offscreen-test-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    Scratch(dir)
}

/// A small world (8 × 8 chunks, a few zombies) a second into the game, saved to `dir`.
fn saved_world(dir: &Path) -> PathBuf {
    let data = content::default_data_dir();
    let mut config = content::load_config(&data, 3).unwrap().with_map_chunks(8);
    config.zombies = Some(40);
    let mut world = World::new(config);
    world.add_player(world.player_spawn);
    let near = world.player_spawn + bevy::math::Vec2::new(6.0, 4.0);
    world.spawn_zombies(5, near, 2.0);
    for _ in 0..20 {
        world.step();
    }
    let path = dir.join("world.zsave");
    save_file::save(&world, &path).unwrap();
    path
}

fn no_gpu(stderr: &str) -> bool {
    let s = stderr.to_lowercase();
    s.contains("unable to find a gpu") || s.contains("no suitable adapter") || s.contains("failed to find an appropriate adapter")
}

/// Width and height from a PNG's header.
fn png_size(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 24 || &bytes[..8] != b"\x89PNG\r\n\x1a\n" || &bytes[12..16] != b"IHDR" {
        return None;
    }
    let be = |i: usize| u32::from_be_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]);
    Some((be(16), be(20)))
}

#[test]
fn renders_a_saved_world_to_a_png_without_a_window() {
    if std::env::var_os("ZOMBYE_SKIP_GPU_TESTS").is_some() {
        eprintln!("skipped: ZOMBYE_SKIP_GPU_TESTS is set");
        return;
    }
    let scratch = scratch("render");
    let dir = &scratch.0;
    let save = saved_world(dir);
    let png = dir.join("shot.png");
    let out = Command::new(env!("CARGO_BIN_EXE_zombye"))
        .args(["--render-to", png.to_str().unwrap(), "--load", save.to_str().unwrap(), "--size", "320x200", "--angle", "3", "--saves"])
        .arg(dir)
        .env("RUST_LOG", "warn")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    if !out.status.success() && no_gpu(&stderr) {
        eprintln!("skipped: no GPU adapter here:\n{stderr}");
        return;
    }
    assert!(out.status.success(), "render failed ({}):\n{stderr}", out.status);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("rendered ") && !stdout.contains("warning"), "{stdout}\n{stderr}");
    let bytes = std::fs::read(&png).unwrap();
    assert_eq!(png_size(&bytes), Some((320, 200)));
    // A blank (or one-colour) frame compresses to almost nothing; a town with fog doesn't.
    assert!(bytes.len() > 20_000, "suspiciously small PNG: {} bytes", bytes.len());
    // It only looked: no autosave, no quicksave, nothing else written.
    let mut files: Vec<String> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    files.sort();
    assert_eq!(files, ["shot.png", "world.zsave"]);
}

#[test]
fn an_unknown_player_is_refused() {
    if std::env::var_os("ZOMBYE_SKIP_GPU_TESTS").is_some() {
        return;
    }
    let scratch = scratch("player");
    let dir = &scratch.0;
    let save = saved_world(dir);
    let out = Command::new(env!("CARGO_BIN_EXE_zombye"))
        .args(["--render-to", dir.join("x.png").to_str().unwrap(), "--load", save.to_str().unwrap(), "--player", "9"])
        .env("RUST_LOG", "warn")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    if no_gpu(&stderr) {
        // The renderer starts (and fails) before the save is looked at.
        eprintln!("skipped: no GPU adapter here");
        return;
    }
    assert!(!out.status.success() && stderr.contains("no player 9") && stderr.contains("players [1]"), "{stderr}");
    assert!(!dir.join("x.png").exists());
}
