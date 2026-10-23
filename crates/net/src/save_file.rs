//! Save files on disk. The sim turns a world into bytes and back ([`World::save`],
//! [`World::load`]); this module does the I/O: atomic writes into a saves folder, listing the
//! saves there (from each file's small info section) and loading one.
//!
//! A save is one `.zsave` file (format in `zombye_sim::save`). Writes go to a temporary file in the
//! same folder that is flushed to disk and then renamed over the old save, so a crash or power cut
//! mid-write leaves the previous save intact, never half of one.

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime};

use zombye_sim::save::{EXTENSION, read_info};
use zombye_sim::{SaveInfo, World, WorldConfig};

/// The saves folder used when none is given (relative to the working directory).
pub const DEFAULT_DIR: &str = "saves";
/// How much of a file [`list`] reads to get its info.
const INFO_PREFIX: u64 = 64 * 1024;

/// Where save `name` lives in `dir`: `dir/name.zsave`. Names are plain file names (letters,
/// digits, `-`, `_`, `.`; no folders), so a request can't write outside the folder.
pub fn path_for(dir: &Path, name: &str) -> Result<PathBuf, String> {
    let name = name.strip_suffix(&format!(".{EXTENSION}")).unwrap_or(name);
    let ok = !name.is_empty() && name.len() <= 64 && !name.starts_with('.') && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if !ok {
        return Err(format!("bad save name {name:?}: use letters, digits, - _ and . (no folders)"));
    }
    Ok(dir.join(format!("{name}.{EXTENSION}")))
}

/// A save name, or a path to a save file (anything with a `/` or the `.zsave` extension).
pub fn resolve(dir: &Path, name_or_path: &str) -> Result<PathBuf, String> {
    let p = Path::new(name_or_path);
    if name_or_path.contains(std::path::MAIN_SEPARATOR) || name_or_path.contains('/') || p.extension().is_some_and(|e| e == EXTENSION) {
        Ok(p.to_path_buf())
    } else {
        path_for(dir, name_or_path)
    }
}

/// Write `bytes` to `path` atomically: into a temporary file next to it, flushed to disk, then
/// renamed over `path`. Creates the folder if needed.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    fs::create_dir_all(dir)?;
    let file_name = path.file_name().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no file name"))?;
    let tmp = dir.join(format!(".{}.{}.tmp", file_name.to_string_lossy(), std::process::id()));
    let result = (|| {
        let mut f = File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        fs::rename(&tmp, path)?;
        // Make the rename itself durable where the platform allows syncing a folder.
        #[cfg(unix)]
        if let Ok(d) = File::open(dir) {
            let _ = d.sync_all();
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// What [`save`] wrote.
#[derive(Clone, Debug)]
pub struct Saved {
    pub path: PathBuf,
    pub bytes: usize,
    /// Time to serialize, and to write the file.
    pub encode_ms: f32,
    pub write_ms: f32,
}

impl std::fmt::Display for Saved {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({}, {:.0} ms + {:.0} ms to write)", self.path.display(), size(self.bytes as u64), self.encode_ms, self.write_ms)
    }
}

/// Save `world` to `path` (atomically).
pub fn save(world: &World, path: &Path) -> Result<Saved, String> {
    let start = Instant::now();
    let bytes = world.save();
    let encode_ms = start.elapsed().as_secs_f32() * 1000.0;
    let start = Instant::now();
    write_atomic(path, &bytes).map_err(|e| format!("couldn't write {}: {e}", path.display()))?;
    Ok(Saved { path: path.to_path_buf(), bytes: bytes.len(), encode_ms, write_ms: start.elapsed().as_secs_f32() * 1000.0 })
}

/// Load the save at `path`, with `content` supplying the building templates, weapons and population
/// settings (see [`World::load`]). Returns the world and how long loading took (ms).
pub fn load(path: &Path, content: &WorldConfig) -> Result<(World, f32), String> {
    let bytes = fs::read(path).map_err(|e| format!("couldn't read {}: {e}", path.display()))?;
    let start = Instant::now();
    let world = World::load(&bytes, content).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok((world, start.elapsed().as_secs_f32() * 1000.0))
}

/// One save in a folder.
#[derive(Clone, Debug)]
pub struct SaveEntry {
    /// File name without the extension (what [`path_for`] takes).
    pub name: String,
    pub path: PathBuf,
    pub bytes: u64,
    pub modified: Option<SystemTime>,
    /// What's in it, or why it can't be read.
    pub info: Result<SaveInfo, String>,
}

/// The saves in `dir`, newest first. A missing folder has none.
pub fn list(dir: &Path) -> io::Result<Vec<SaveEntry>> {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != EXTENSION) {
            continue;
        }
        let meta = entry.metadata().ok();
        let mut prefix = Vec::new();
        let info = File::open(&path)
            .and_then(|f| f.take(INFO_PREFIX).read_to_end(&mut prefix))
            .map_err(|e| e.to_string())
            .and_then(|_| read_info(&prefix).map_err(|e| e.to_string()));
        out.push(SaveEntry {
            name: path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
            bytes: meta.as_ref().map_or(0, |m| m.len()),
            modified: meta.and_then(|m| m.modified().ok()),
            path,
            info,
        });
    }
    out.sort_by(|a, b| b.modified.cmp(&a.modified).then(a.name.cmp(&b.name)));
    Ok(out)
}

/// A byte count for people: "812 KB", "2.4 MB".
pub fn size(bytes: u64) -> String {
    if bytes < 1_000_000 { format!("{:.0} KB", bytes as f64 / 1000.0) } else { format!("{:.1} MB", bytes as f64 / 1e6) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("zombye-save-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn config() -> WorldConfig {
        let data = content::default_data_dir();
        WorldConfig::new(3, 80, content::load_building_templates(&data).unwrap())
            .with_weapons(content::load_weapons(&data).unwrap())
            .with_population(content::load_population(&data).unwrap())
    }

    #[test]
    fn names_stay_in_the_folder() {
        let dir = Path::new("saves");
        assert_eq!(path_for(dir, "quick").unwrap(), dir.join("quick.zsave"));
        assert_eq!(path_for(dir, "quick.zsave").unwrap(), dir.join("quick.zsave"));
        for bad in ["", "../x", "a/b", ".hidden", "x\\y", "a b"] {
            assert!(path_for(dir, bad).is_err(), "{bad:?}");
        }
        assert_eq!(resolve(dir, "/tmp/w.zsave").unwrap(), PathBuf::from("/tmp/w.zsave"));
        assert_eq!(resolve(dir, "auto").unwrap(), dir.join("auto.zsave"));
    }

    #[test]
    fn save_list_and_load_from_a_folder() {
        let dir = scratch("folder");
        let mut world = World::new(config());
        world.add_player(world.player_spawn);
        for _ in 0..20 {
            world.step();
        }
        let path = path_for(&dir, "one").unwrap();
        let saved = save(&world, &path).unwrap();
        assert!(saved.bytes > 0 && path.exists());
        // Overwriting goes through a temporary file that doesn't linger.
        world.step();
        save(&world, &path).unwrap();
        save(&world, &path_for(&dir, "two").unwrap()).unwrap();
        fs::write(dir.join("junk.zsave"), b"not a save").unwrap();
        fs::write(dir.join("notes.txt"), b"ignored").unwrap();
        let names: Vec<String> = fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        assert!(names.iter().all(|n| !n.ends_with(".tmp")), "{names:?}");
        let listed = list(&dir).unwrap();
        assert_eq!(listed.len(), 3);
        let one = listed.iter().find(|e| e.name == "one").unwrap();
        assert_eq!(one.info.as_ref().unwrap().tick, 21);
        assert!(listed.iter().find(|e| e.name == "junk").unwrap().info.is_err());
        let (loaded, _) = load(&path, &config()).unwrap();
        assert_eq!(loaded.state_hash(), world.state_hash());
        assert!(load(&dir.join("junk.zsave"), &config()).err().unwrap().contains("not a Zombye save"));
        assert!(list(&dir.join("missing")).unwrap().is_empty());
        fs::remove_dir_all(&dir).unwrap();
    }
}
