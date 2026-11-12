//! Where an installed game finds its files (playtest builds): run from a checkout, everything is
//! where the repo keeps it; run from a package (a zip on itch.io, a macOS `.app`), the game's
//! files sit beside the executable (or in the `.app`'s `Contents/Resources`), and saves and
//! settings go to the player's own folder, never wherever the game was started from.
//!
//! ```text
//! zombye(.exe)          the game          Zombye.app/Contents/MacOS/zombye
//! assets/               its files         Zombye.app/Contents/Resources/assets/
//! zombye.pak            bought art, packed (`crate::pack`), optional
//! ```

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The repo's `assets` folder, as it was where this binary was built.
fn checkout_assets() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")
}

/// The folders an installed game's files may sit in, nearest first: beside the executable, then
/// (macOS) the `.app`'s `Contents/Resources`.
pub fn install_dirs() -> Vec<PathBuf> {
    let Some(exe_dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)) else { return Vec::new() };
    let resources = exe_dir.join("../Resources");
    let mut dirs = vec![exe_dir];
    if resources.is_dir() {
        dirs.push(resources);
    }
    dirs
}

/// `name` in the first install folder that has it.
pub fn beside_exe(name: &str) -> Option<PathBuf> {
    install_dirs().into_iter().map(|d| d.join(name)).find(|p| p.exists())
}

/// The game's `assets` folder: `$ZOMBYE_ASSETS`, else the checkout's when it is there, else
/// `assets` in an install folder. With none of them, the checkout's, so errors name a path.
pub fn assets_dir() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        if let Some(dir) = std::env::var_os("ZOMBYE_ASSETS").map(PathBuf::from).filter(|d| !d.as_os_str().is_empty()) {
            return dir;
        }
        let checkout = checkout_assets();
        if checkout.is_dir() && !forced_packaged() {
            return checkout;
        }
        beside_exe("assets").filter(|p| p.is_dir()).unwrap_or(checkout)
    })
}

/// `ZOMBYE_PACKAGED=1`: act as a packaged build even next to a checkout (for trying one out on
/// the machine that built it).
fn forced_packaged() -> bool {
    std::env::var("ZOMBYE_PACKAGED").is_ok_and(|v| v == "1")
}

/// Is this a packaged game (its assets not the checkout's)?
pub fn packaged() -> bool {
    static PACKAGED: OnceLock<bool> = OnceLock::new();
    *PACKAGED.get_or_init(|| forced_packaged() || assets_dir() != checkout_assets())
}

/// Are the dev tools on: always from a checkout; in a package only with `--dev` or
/// `ZOMBYE_DEV=1` (god mode, spawning zombies, skipping time, `--give`, the debug window).
pub fn dev_tools() -> bool {
    static DEV: OnceLock<bool> = OnceLock::new();
    *DEV.get_or_init(|| !packaged() || std::env::var("ZOMBYE_DEV").is_ok_and(|v| v == "1") || std::env::args().any(|a| a == "--dev"))
}

/// The player's own folder for saves, settings and logs in a packaged game: macOS
/// `~/Library/Application Support/Zombye`, Windows `%APPDATA%\Zombye`, elsewhere
/// `$XDG_DATA_HOME/zombye` (`~/.local/share/zombye`). From a checkout, `None`: the working folder
/// as before.
pub fn user_dir() -> Option<PathBuf> {
    if !packaged() {
        return None;
    }
    let var = |k: &str| std::env::var_os(k).map(PathBuf::from).filter(|p| !p.as_os_str().is_empty());
    let dir = if cfg!(target_os = "macos") {
        var("HOME").map(|h| h.join("Library/Application Support/Zombye"))
    } else if cfg!(windows) {
        var("APPDATA").map(|a| a.join("Zombye"))
    } else {
        var("XDG_DATA_HOME").or_else(|| var("HOME").map(|h| h.join(".local/share"))).map(|d| d.join("zombye"))
    }?;
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

/// The client's default saves folder: `saves` in the player's own folder when packaged, else
/// `saves` in the working folder.
pub fn default_saves_dir() -> PathBuf {
    user_dir().map_or_else(|| PathBuf::from(crate::save_file::DEFAULT_DIR), |d| d.join("saves"))
}

/// The build this is: `ZOMBYE_BUILD` when it was compiled (the packaging script sets the commit
/// and date), else the crate version.
pub fn build_name() -> &'static str {
    match option_env!("ZOMBYE_BUILD") {
        Some(b) if !b.is_empty() => b,
        _ => concat!("dev ", env!("CARGO_PKG_VERSION")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_a_checkout_everything_is_where_the_repo_keeps_it() {
        // Tests run from the checkout, with no ZOMBYE_ASSETS / ZOMBYE_PACKAGED set.
        if std::env::var_os("ZOMBYE_ASSETS").is_some() || forced_packaged() {
            return;
        }
        assert!(!packaged());
        assert!(dev_tools());
        assert_eq!(user_dir(), None);
        assert_eq!(default_saves_dir(), PathBuf::from("saves"));
        assert!(assets_dir().join("data").is_dir());
    }
}
