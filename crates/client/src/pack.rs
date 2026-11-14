//! The release pack: the bought art a packaged game carries (`release-models`, `release-characters`,
//! `release-anim`, `release-audio`; see those modules) packed into one file, `zombye.pak`, beside
//! the executable, so a player's install never holds them as loose files anyone can lift out (the
//! packs' licences allow them only embedded in a game). From a checkout nothing changes: the
//! folders are read from disk as before, and a real folder always wins over the pack.
//!
//! The pack is mounted where it lies: an entry `release-models/trees/pine.glb` in
//! `<dir>/zombye.pak` reads as the file `<dir>/release-models/trees/pine.glb`. The loaders of the
//! release folders read through [`read`], [`read_dir`], [`is_file`] and [`is_dir`], which look in
//! the pack first and the disk after.
//!
//! Format: `ZOMBYPAK`, then everything after it scrambled with a keystream (`ZOMBYE_PACK_KEY` when
//! the client was compiled, else a built-in key): a `u32` count, each entry's path (`u16` length,
//! UTF-8, `/` separators), offset and length (`u64`s, from the start of the file), then the files'
//! bytes. All little-endian. Written by `zombye --write-pack OUT DIR...` (`tools/release/package.sh`).

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

const MAGIC: &[u8; 8] = b"ZOMBYPAK";
/// The pack's file name, beside the executable.
pub const FILE: &str = "zombye.pak";

fn key() -> u64 {
    let text = option_env!("ZOMBYE_PACK_KEY").unwrap_or("dead commons zombye release pack");
    // FNV-1a over the key's text.
    text.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x0100_0000_01b3))
}

fn splitmix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// Scramble (or unscramble: it is its own inverse) `bytes` lying at `pos` in the file.
fn scramble(key: u64, pos: u64, bytes: &mut [u8]) {
    let mut block = u64::MAX;
    let mut stream = [0u8; 8];
    for (i, b) in bytes.iter_mut().enumerate() {
        let at = pos + i as u64;
        if at / 8 != block {
            block = at / 8;
            stream = splitmix(key ^ block.wrapping_mul(0xd6e8_feb8_6659_fd93)).to_le_bytes();
        }
        *b ^= stream[(at % 8) as usize];
    }
}

/// An open pack: where it is mounted, its entries by path, and the file to read them from.
pub struct Pack {
    mount: PathBuf,
    entries: BTreeMap<String, (u64, u64)>,
    file: Mutex<File>,
    key: u64,
}

impl Pack {
    pub fn open(path: &Path, key: u64) -> Result<Self, String> {
        let mut file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let size = file.metadata().map_err(|e| e.to_string())?.len();
        let mut magic = [0u8; 8];
        file.read_exact(&mut magic).map_err(|_| "not a pack".to_string())?;
        if &magic != MAGIC {
            return Err("not a pack".into());
        }
        let mut pos = 8u64;
        let mut take = |file: &mut File, n: usize| -> Result<Vec<u8>, String> {
            let mut b = vec![0u8; n];
            file.read_exact(&mut b).map_err(|_| "the pack is cut short".to_string())?;
            scramble(key, pos, &mut b);
            pos += n as u64;
            Ok(b)
        };
        let count = u32::from_le_bytes(take(&mut file, 4)?.try_into().unwrap());
        if count > 1_000_000 {
            return Err("the pack is damaged (or made with another key)".into());
        }
        let mut entries = BTreeMap::new();
        for _ in 0..count {
            let len = u16::from_le_bytes(take(&mut file, 2)?.try_into().unwrap()) as usize;
            let name = String::from_utf8(take(&mut file, len)?).map_err(|_| "the pack is damaged (or made with another key)".to_string())?;
            let at = u64::from_le_bytes(take(&mut file, 8)?.try_into().unwrap());
            let n = u64::from_le_bytes(take(&mut file, 8)?.try_into().unwrap());
            if at.checked_add(n).is_none_or(|end| end > size) {
                return Err(format!("the pack is damaged: {name} runs past its end"));
            }
            entries.insert(name, (at, n));
        }
        let mount = path.parent().map(Path::to_path_buf).unwrap_or_default();
        Ok(Self { mount, entries, file: Mutex::new(file), key })
    }

    /// The pack's name for `path`, if it lies under the mount.
    fn name_of(&self, path: &Path) -> Option<String> {
        let rel = path.strip_prefix(&self.mount).ok()?;
        let parts: Vec<&str> = rel.components().map(|c| c.as_os_str().to_str()).collect::<Option<_>>()?;
        Some(parts.join("/"))
    }

    pub fn is_file(&self, path: &Path) -> bool {
        self.name_of(path).is_some_and(|n| self.entries.contains_key(&n))
    }

    pub fn is_dir(&self, path: &Path) -> bool {
        let Some(n) = self.name_of(path) else { return false };
        let prefix = if n.is_empty() { String::new() } else { format!("{n}/") };
        self.entries.range(prefix.clone()..).next().is_some_and(|(k, _)| k.starts_with(&prefix))
    }

    /// What lies straight in the folder `path` (files and folders), sorted.
    pub fn list(&self, path: &Path) -> Vec<PathBuf> {
        let Some(n) = self.name_of(path) else { return Vec::new() };
        let prefix = if n.is_empty() { String::new() } else { format!("{n}/") };
        let mut out: Vec<String> = Vec::new();
        for (k, _) in self.entries.range(prefix.clone()..) {
            let Some(rest) = k.strip_prefix(&prefix) else { break };
            let first = rest.split('/').next().unwrap_or(rest).to_string();
            if out.last() != Some(&first) {
                out.push(first);
            }
        }
        out.into_iter().map(|f| path.join(f)).collect()
    }

    pub fn read(&self, path: &Path) -> Option<std::io::Result<Vec<u8>>> {
        let &(at, n) = self.entries.get(&self.name_of(path)?)?;
        let mut file = self.file.lock().unwrap_or_else(|e| e.into_inner());
        let mut bytes = vec![0u8; n as usize];
        let read = file.seek(SeekFrom::Start(at)).and_then(|_| file.read_exact(&mut bytes));
        Some(read.map(|_| {
            scramble(self.key, at, &mut bytes);
            bytes
        }))
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

/// The pack beside the executable (or `$ZOMBYE_PACK`), opened once; none from a checkout
/// without one.
pub fn pack() -> Option<&'static Pack> {
    static PACK: OnceLock<Option<Pack>> = OnceLock::new();
    PACK.get_or_init(|| {
        let path = std::env::var_os("ZOMBYE_PACK").map(PathBuf::from).or_else(|| zombye_net::install::beside_exe(FILE))?;
        match Pack::open(&path, key()) {
            Ok(p) => {
                println!("release pack: {} ({} files)", path.display(), p.len());
                Some(p)
            }
            Err(e) => {
                eprintln!("can't read the release pack: {e}");
                None
            }
        }
    })
    .as_ref()
}

/// `name` beside the executable, on disk or in the pack (where a release folder is looked for).
pub fn beside_exe(name: &str) -> Option<PathBuf> {
    zombye_net::install::beside_exe(name).or_else(|| pack().map(|p| p.mount.join(name)).filter(|p| is_dir(p)))
}

/// The release folder `name` (`release-models`, ...): `$var` if set, else beside the executable
/// (on disk or in the pack), else, from a checkout, `name` in the private art repo's checkout
/// ([`art_repo`]). So a developer with the art repo cloned plays with the bought art without
/// setting anything, and anyone without it gets this repo's art. Says on stdout where it came from.
pub fn release_dir(var: &str, name: &str) -> Option<PathBuf> {
    let set = std::env::var_os(var).filter(|v| !v.is_empty()).map(PathBuf::from);
    let (found, from) = match set {
        Some(p) => (Some(p), format!("${var}")),
        None => match beside_exe(name) {
            Some(p) => (Some(p), "beside the game".to_string()),
            None if zombye_net::install::packaged() => (None, String::new()),
            None => (art_repo().map(|a| a.join(name)), "the art repo".to_string()),
        },
    };
    let found = found.filter(|p| is_dir(p));
    if let Some(p) = &found {
        println!("{}: {} (from {from})", name.replace('-', " "), p.display());
    }
    found
}

/// The private art repo's checkout, for a game run from this repo's checkout: `$ZOMBYE_ART`
/// (`none`: no art repo, to see the game as this repo alone draws it), else `zombye-art` beside
/// this checkout, else `~/games/zombye-art` (the default the asset tools use too). Unit tests look
/// only at `$ZOMBYE_ART`, so they test this repo's art wherever they run, as before.
pub fn art_repo() -> Option<PathBuf> {
    let var = std::env::var_os("ZOMBYE_ART").map(PathBuf::from);
    if cfg!(test) && var.is_none() {
        return None;
    }
    art_repo_from(
        var,
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.."),
        std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from),
    )
}

fn art_repo_from(var: Option<PathBuf>, checkout_parent: PathBuf, home: Option<PathBuf>) -> Option<PathBuf> {
    if let Some(v) = var.filter(|v| !v.as_os_str().is_empty()) {
        return (v.as_os_str() != "none").then_some(v).filter(|v| v.is_dir());
    }
    std::iter::once(checkout_parent.join("zombye-art"))
        .chain(home.map(|h| h.join("games").join("zombye-art")))
        .find(|p| p.is_dir())
}

pub fn is_file(path: &Path) -> bool {
    path.is_file() || pack().is_some_and(|p| p.is_file(path))
}

pub fn is_dir(path: &Path) -> bool {
    path.is_dir() || pack().is_some_and(|p| p.is_dir(path))
}

/// The file's bytes, from the disk if it is there, else from the pack.
pub fn read(path: &Path) -> std::io::Result<Vec<u8>> {
    match std::fs::read(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => pack().and_then(|p| p.read(path)).unwrap_or(Err(e)),
        other => other,
    }
}

pub fn read_to_string(path: &Path) -> std::io::Result<String> {
    String::from_utf8(read(path)?).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

/// What lies straight in the folder `path`: the disk's if it is a folder there, else the pack's.
pub fn read_dir(path: &Path) -> std::io::Result<Vec<PathBuf>> {
    match std::fs::read_dir(path) {
        Ok(d) => Ok(d.flatten().map(|e| e.path()).collect()),
        Err(e) => match pack() {
            Some(p) if p.is_dir(path) => Ok(p.list(path)),
            _ => Err(e),
        },
    }
}

/// Every file under `dir` with its name under `prefix`, sorted (hidden files left out).
fn walk(dir: &Path, prefix: &str, out: &mut Vec<(String, PathBuf)>) -> std::io::Result<()> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)?.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let path = e.path();
        let full = format!("{prefix}/{name}");
        if path.is_dir() {
            walk(&path, &full, out)?;
        } else {
            out.push((full, path));
        }
    }
    Ok(())
}

/// Pack the folders `dirs` (each under its own name: `release-models/...`) into `out`.
pub fn write(out: &Path, dirs: &[PathBuf], key: u64) -> Result<(usize, u64), String> {
    let mut files = Vec::new();
    for dir in dirs {
        let name = dir.file_name().and_then(|n| n.to_str()).ok_or_else(|| format!("{}: no folder name", dir.display()))?;
        walk(dir, name, &mut files).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let index_len: u64 = 4 + files.iter().map(|(n, _)| 2 + n.len() as u64 + 16).sum::<u64>();
    let mut index = Vec::with_capacity(index_len as usize);
    index.extend_from_slice(&(files.len() as u32).to_le_bytes());
    let mut at = 8 + index_len;
    let mut sizes = Vec::new();
    for (name, path) in &files {
        let n = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?.len();
        index.extend_from_slice(&(name.len() as u16).to_le_bytes());
        index.extend_from_slice(name.as_bytes());
        index.extend_from_slice(&at.to_le_bytes());
        index.extend_from_slice(&n.to_le_bytes());
        sizes.push(n);
        at += n;
    }
    let mut f = std::io::BufWriter::new(File::create(out).map_err(|e| format!("{}: {e}", out.display()))?);
    let io = |e: std::io::Error| format!("{}: {e}", out.display());
    f.write_all(MAGIC).map_err(io)?;
    scramble(key, 8, &mut index);
    f.write_all(&index).map_err(io)?;
    let mut pos = 8 + index_len;
    for ((_, path), n) in files.iter().zip(sizes) {
        let mut bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        if bytes.len() as u64 != n {
            return Err(format!("{} changed while being packed", path.display()));
        }
        scramble(key, pos, &mut bytes);
        f.write_all(&bytes).map_err(io)?;
        pos += n;
    }
    f.flush().map_err(io)?;
    Ok((files.len(), pos))
}

/// `zombye --write-pack OUT DIR...`: pack the release folders for a packaged build, and exit.
pub fn write_from_args(args: &[String]) -> ! {
    let (Some(out), dirs) = (args.first(), &args[1.min(args.len())..]) else {
        eprintln!("usage: zombye --write-pack OUT DIR...");
        std::process::exit(2);
    };
    let dirs: Vec<PathBuf> = dirs.iter().map(PathBuf::from).collect();
    if dirs.is_empty() {
        eprintln!("usage: zombye --write-pack OUT DIR...");
        std::process::exit(2);
    }
    match write(Path::new(out), &dirs, key()) {
        Ok((n, bytes)) => {
            println!("wrote {out}: {n} files, {:.1} MB", bytes as f64 / 1e6);
            std::process::exit(0);
        }
        Err(e) => {
            eprintln!("can't write the pack: {e}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_art_repo_is_found_beside_the_checkout_or_in_games() {
        let root = std::env::temp_dir().join(format!("zombye-art-find-{}", std::process::id()));
        let (side, home) = (root.join("code"), root.join("home"));
        let games = home.join("games").join("zombye-art");
        std::fs::create_dir_all(&games).unwrap();
        std::fs::create_dir_all(&side).unwrap();
        // Nothing beside the checkout: the one in ~/games.
        assert_eq!(art_repo_from(None, side.clone(), Some(home.clone())), Some(games.clone()));
        // One beside the checkout wins.
        std::fs::create_dir_all(side.join("zombye-art")).unwrap();
        assert_eq!(art_repo_from(None, side.clone(), Some(home.clone())), Some(side.join("zombye-art")));
        // $ZOMBYE_ART wins over both, and `none` turns it off.
        assert_eq!(art_repo_from(Some(games.clone()), side.clone(), Some(home.clone())), Some(games.clone()));
        assert_eq!(art_repo_from(Some("none".into()), side.clone(), Some(home.clone())), None);
        // A $ZOMBYE_ART that isn't there is nothing, not a guess elsewhere.
        assert_eq!(art_repo_from(Some(root.join("gone")), side.clone(), Some(home.clone())), None);
        // Neither there: none.
        assert_eq!(art_repo_from(None, root.join("elsewhere"), Some(root.join("nobody"))), None);
        std::fs::remove_dir_all(&root).ok();
    }

    use super::*;

    #[test]
    fn a_pack_reads_back_what_went_in_and_hides_it() {
        let dir = std::env::temp_dir().join(format!("zombye-pack-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let src = dir.join("src/release-models");
        std::fs::create_dir_all(src.join("trees")).unwrap();
        std::fs::create_dir_all(src.join("street")).unwrap();
        std::fs::write(src.join("trees/pine.glb"), b"a pine tree's bytes").unwrap();
        std::fs::write(src.join("trees/pine.2.glb"), b"another pine").unwrap();
        std::fs::write(src.join("street/car.glb"), vec![7u8; 10_000]).unwrap();
        std::fs::write(src.join(".DS_Store"), b"junk").unwrap();
        let out = dir.join("game/zombye.pak");
        std::fs::create_dir_all(out.parent().unwrap()).unwrap();
        assert_eq!(write(&out, &[src.clone()], 42).unwrap().0, 3);
        let raw = std::fs::read(&out).unwrap();
        assert!(!raw.windows(6).any(|w| w == b"a pine"), "the bytes are scrambled");
        assert!(!raw.windows(5).any(|w| w == b"trees"), "so are the names");

        let p = Pack::open(&out, 42).unwrap();
        let game = dir.join("game");
        let models = game.join("release-models");
        assert!(p.is_dir(&models) && p.is_dir(&models.join("trees")) && !p.is_dir(&models.join("veg")));
        assert!(p.is_file(&models.join("trees/pine.glb")) && !p.is_file(&models.join("trees")));
        assert_eq!(p.read(&models.join("trees/pine.glb")).unwrap().unwrap(), b"a pine tree's bytes");
        assert_eq!(p.read(&models.join("street/car.glb")).unwrap().unwrap(), vec![7u8; 10_000]);
        assert!(p.read(&models.join("nothing.glb")).is_none());
        assert_eq!(p.list(&models.join("trees")), vec![models.join("trees/pine.2.glb"), models.join("trees/pine.glb")]);
        assert_eq!(p.list(&models), vec![models.join("street"), models.join("trees")]);
        assert_eq!(p.list(&game), vec![models.clone()]);
        assert!(p.read(&models.join(".DS_Store")).is_none());

        assert!(Pack::open(&out, 43).is_err(), "another key reads nothing");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
