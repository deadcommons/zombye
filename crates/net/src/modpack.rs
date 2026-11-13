//! Mods a server hands to the clients that join it (#24, docs/PLAN.md "Distribution").
//!
//! A [`ModPack`] is the **data** of a mod: its name and its data files as text (`items.ron`,
//! `recipes.ron`, `loot.ron`, `buildings/*.ron`). **Never its scripts**: those run on the server
//! only, so a client never runs code a server sent it. A pack is named by its [`ModPack::hash`]
//! (SHA-256 over its contents): a server lists its mods with their hashes
//! ([`crate::wire::ModInfo`]), and a client that hasn't a pack with that hash (among its own
//! `--mod` folders, or in its cache of earlier downloads) asks for it
//! ([`crate::remote::fetch_mods`]), checks what arrived against the hash it asked for, and keeps
//! it ([`Cache`]). [`gather`] is all of that: the server's mods, in the server's order, ready to
//! be put into the game's data.
//!
//! A pack's file names never touch the disk: the cache keeps each pack as one file named by id
//! and hash, so nothing a server sends can write outside the cache folder.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::wire::ModInfo;

/// SHA-256 of a pack's contents.
pub type Hash = [u8; 32];

/// The most a pack may hold (its files' text together) to be handed out, files in it, and mods a
/// server lists.
pub const MAX_PACK_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_FILES: usize = 256;
pub const MAX_MODS: usize = 64;
/// The most a game downloads on joining one server, all its missing mods together.
pub const MAX_DOWNLOAD_BYTES: usize = 32 * 1024 * 1024;

/// A mod's data, as a server sends it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModPack {
    /// Lowercase letters, digits and `_` (at most 48), as in `mod.ron`.
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    /// Data files, name and text, in the order they are applied ([`data_file`] says which names).
    pub files: Vec<(String, String)>,
}

/// Is `id` a mod id: lowercase letters, digits and `_`, at most 48?
pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 48 && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// Is `name` a data file a pack may hold: `items.ron`, `recipes.ron`, `loot.ron` or
/// `buildings/NAME.ron` (NAME of letters, digits, `_` and `-`)?
pub fn data_file(name: &str) -> bool {
    if matches!(name, "items.ron" | "recipes.ron" | "loot.ron") {
        return true;
    }
    name.strip_prefix("buildings/").and_then(|n| n.strip_suffix(".ron")).is_some_and(|n| !n.is_empty() && n.len() <= 64 && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'))
}

impl ModPack {
    /// The bytes of its files' text.
    pub fn size(&self) -> usize {
        self.files.iter().map(|(n, t)| n.len() + t.len()).sum::<usize>() + self.name.len() + self.version.len() + self.description.len()
    }

    /// What names this pack: SHA-256 over its id, name, version, description and every file's
    /// name and text, in order, each with its length (so nothing can shift between fields).
    pub fn hash(&self) -> Hash {
        let mut h = Sha256::new();
        h.update(b"zombye-mod-pack/1\0");
        let mut field = |s: &str| {
            h.update(&(s.len() as u64).to_le_bytes());
            h.update(s.as_bytes());
        };
        field(&self.id);
        field(&self.name);
        field(&self.version);
        field(&self.description);
        field(&self.files.len().to_string());
        for (name, text) in &self.files {
            field(name);
            field(text);
        }
        h.finish()
    }

    /// Is this a pack a game can take: a proper id, only data files, none twice, not too big?
    /// (What the files say is checked when they are put into the game's data.)
    pub fn check(&self) -> Result<(), String> {
        if !valid_id(&self.id) {
            return Err(format!("mod id {:?} must be lowercase letters, digits and _ (at most 48)", self.id));
        }
        if self.files.len() > MAX_FILES {
            return Err(format!("mod {}: {} files (at most {MAX_FILES})", self.id, self.files.len()));
        }
        if self.size() > MAX_PACK_BYTES {
            return Err(format!("mod {}: {} of data (at most {})", self.id, kb(self.size()), kb(MAX_PACK_BYTES)));
        }
        for (k, (name, _)) in self.files.iter().enumerate() {
            if !data_file(name) {
                return Err(format!("mod {}: {name:?} is not a data file a mod may bring", self.id));
            }
            if self.files[..k].iter().any(|(n, _)| n == name) {
                return Err(format!("mod {}: {name} is there twice", self.id));
            }
        }
        Ok(())
    }

    /// What a server says of this mod to whoever asks.
    pub fn info(&self) -> ModInfo {
        ModInfo { id: self.id.clone(), name: self.name.clone(), version: self.version.clone(), hash: self.hash(), size: self.size().min(u32::MAX as usize) as u32 }
    }
}

/// `1.4 KB`, `2.0 MB`.
pub fn kb(bytes: usize) -> String {
    if bytes >= 1024 * 1024 { format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0)) } else { format!("{:.1} KB", bytes as f64 / 1024.0) }
}

/// A hash as 64 hex digits.
pub fn hex(hash: &Hash) -> String {
    hash.iter().map(|b| format!("{b:02x}")).collect()
}

/// The start of a hash, for messages.
pub fn short(hash: &Hash) -> String {
    hex(hash)[..12].to_string()
}

fn unhex(text: &str) -> Option<Hash> {
    if text.len() != 64 || !text.is_ascii() {
        return None;
    }
    let mut hash = [0u8; 32];
    for (k, b) in hash.iter_mut().enumerate() {
        *b = u8::from_str_radix(&text[k * 2..k * 2 + 2], 16).ok()?;
    }
    Some(hash)
}

/// Downloaded packs, kept between sessions: a folder of `<id>-<hash>.zmod` files, each one pack.
/// A file is only ever taken for what its contents hash to, so a damaged or swapped one is as
/// good as missing.
#[derive(Clone, Debug)]
pub struct Cache {
    pub dir: PathBuf,
}

/// The file type of a cached pack, and what it starts with.
const CACHE_EXT: &str = "zmod";
const CACHE_MAGIC: &[u8; 8] = b"ZOMBYMOD";

impl Cache {
    /// The cache in a client's saves folder.
    pub fn in_saves(saves: &Path) -> Self {
        Self { dir: saves.join("mods") }
    }

    fn path(&self, id: &str, hash: &Hash) -> PathBuf {
        self.dir.join(format!("{id}-{}.{CACHE_EXT}", hex(hash)))
    }

    /// The pack with this id and hash, if it is here and whole.
    pub fn get(&self, id: &str, hash: &Hash) -> Option<ModPack> {
        if !valid_id(id) {
            return None;
        }
        let bytes = std::fs::read(self.path(id, hash)).ok()?;
        let pack: ModPack = postcard::from_bytes(bytes.strip_prefix(CACHE_MAGIC)?).ok()?;
        (pack.id == id && pack.hash() == *hash && pack.check().is_ok()).then_some(pack)
    }

    /// Keep `pack` (written whole or not at all).
    pub fn put(&self, pack: &ModPack) -> Result<PathBuf, String> {
        pack.check()?;
        let path = self.path(&pack.id, &pack.hash());
        let bytes = postcard::to_extend(pack, CACHE_MAGIC.to_vec()).map_err(|e| e.to_string())?;
        crate::save_file::write_atomic(&path, &bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(path)
    }

    /// The hashes the files here are named for (not read or checked: for saying how much a join
    /// would download, before it does).
    pub fn listed(&self) -> Vec<Hash> {
        let Ok(dir) = std::fs::read_dir(&self.dir) else { return Vec::new() };
        let mut hashes: Vec<Hash> = dir
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == CACHE_EXT))
            .filter_map(|p| p.file_stem().and_then(|s| s.to_str()).and_then(|s| s.rsplit_once('-')).and_then(|(_, h)| unhex(h)))
            .collect();
        hashes.sort();
        hashes
    }
}

/// Where a gathered pack came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// One of this game's own `--mod` folders holds the same data.
    Own,
    /// Downloaded before.
    Cached,
    /// Downloaded now.
    Downloaded,
}

/// The server's mods as packs, in the server's order, each with where it came from.
#[derive(Clone, Debug, Default)]
pub struct Gathered {
    pub packs: Vec<(ModPack, Source)>,
    /// Mods of our own that the server doesn't run (by id), left out while we play there.
    pub left_out: Vec<String>,
}

impl Gathered {
    /// A line per mod, and one for what was left out: what a joining game prints.
    pub fn lines(&self) -> Vec<String> {
        let mut lines: Vec<String> = self
            .packs
            .iter()
            .map(|(p, from)| {
                let v = if p.version.is_empty() { String::new() } else { format!(" {}", p.version) };
                let how = match from {
                    Source::Own => "your own copy".to_string(),
                    Source::Cached => "downloaded before".to_string(),
                    Source::Downloaded => format!("downloaded, {}", kb(p.size())),
                };
                format!("mod {}{v} ({how}; sha256 {})", p.id, short(&p.hash()))
            })
            .collect();
        if !self.left_out.is_empty() {
            lines.push(format!("left out on this server, which doesn't run {}: {}", if self.left_out.len() == 1 { "it" } else { "them" }, self.left_out.join(", ")));
        }
        lines
    }

    pub fn downloaded(&self) -> usize {
        self.packs.iter().filter(|(_, s)| *s == Source::Downloaded).count()
    }
}

/// How many of `mods` a join would download, and their bytes: those `have` (hashes of our own
/// mods and of the cache) doesn't hold.
pub fn to_download(mods: &[ModInfo], have: &[Hash]) -> (usize, usize) {
    let missing = mods.iter().filter(|m| !have.contains(&m.hash));
    (missing.clone().count(), missing.map(|m| m.size as usize).sum())
}

/// Get the mods a server runs (`mods`, from its answer to a query), in its order: each from
/// `own` (this game's `--mod` folders as packs) when one has the same hash, else from `cache`,
/// else downloaded from `addr` (all missing ones over one connection), checked against the hash
/// the server named, and kept in `cache`. Nothing of the server's is trusted but through its
/// hash; nothing is downloaded when nothing is missing.
pub fn gather(addr: &str, password: &str, mods: &[ModInfo], own: &[ModPack], cache: Option<&Cache>, timeout: Duration) -> Result<Gathered, String> {
    if mods.len() > MAX_MODS {
        return Err(format!("the server lists {} mods (at most {MAX_MODS})", mods.len()));
    }
    for (k, m) in mods.iter().enumerate() {
        if !valid_id(&m.id) {
            return Err(format!("the server lists a mod with the id {:?}", m.id.chars().take(60).collect::<String>()));
        }
        if mods[..k].iter().any(|o| o.id == m.id) {
            return Err(format!("the server lists mod {} twice", m.id));
        }
    }
    let mut found: Vec<Option<(ModPack, Source)>> = mods
        .iter()
        .map(|m| {
            own.iter().find(|p| p.id == m.id && p.hash() == m.hash).map(|p| (p.clone(), Source::Own)).or_else(|| cache.and_then(|c| c.get(&m.id, &m.hash)).map(|p| (p, Source::Cached)))
        })
        .collect();
    let missing: Vec<ModInfo> = mods.iter().zip(&found).filter(|(_, f)| f.is_none()).map(|(m, _)| m.clone()).collect();
    // What a join downloads unasked has a limit, whatever a server lists.
    let (_, bytes) = to_download(&missing, &[]);
    if bytes > MAX_DOWNLOAD_BYTES {
        return Err(format!("the server's mods are {} to download (at most {} on joining): get them yourself and start the game with --mod", kb(bytes), kb(MAX_DOWNLOAD_BYTES)));
    }
    if !missing.is_empty() {
        let mut got = crate::remote::fetch_mods(addr, password, &missing, timeout)?.into_iter();
        for (slot, _) in found.iter_mut().zip(mods).filter(|(f, _)| f.is_none()) {
            let pack = got.next().ok_or("the server sent fewer mods than asked for")?;
            if let Some(c) = cache {
                // A cache that can't be written (a read-only folder) costs a download next time.
                if let Err(e) = c.put(&pack) {
                    eprintln!("can't keep mod {}: {e}", pack.id);
                }
            }
            *slot = Some((pack, Source::Downloaded));
        }
    }
    let left_out = own.iter().filter(|p| !mods.iter().any(|m| m.id == p.id)).map(|p| p.id.clone()).collect();
    Ok(Gathered { packs: found.into_iter().flatten().collect(), left_out })
}

/// SHA-256 (FIPS 180-4), here so that the game needs no dependency for one hash.
pub fn sha256(bytes: &[u8]) -> Hash {
    let mut h = Sha256::new();
    h.update(bytes);
    h.finish()
}

struct Sha256 {
    state: [u32; 8],
    block: [u8; 64],
    filled: usize,
    len: u64,
}

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

impl Sha256 {
    fn new() -> Self {
        Self { state: [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19], block: [0; 64], filled: 0, len: 0 }
    }

    fn update(&mut self, mut bytes: &[u8]) {
        self.len = self.len.wrapping_add(bytes.len() as u64);
        while !bytes.is_empty() {
            let n = (64 - self.filled).min(bytes.len());
            self.block[self.filled..self.filled + n].copy_from_slice(&bytes[..n]);
            self.filled += n;
            bytes = &bytes[n..];
            if self.filled == 64 {
                self.compress();
                self.filled = 0;
            }
        }
    }

    fn compress(&mut self) {
        let mut w = [0u32; 64];
        for (k, word) in w.iter_mut().take(16).enumerate() {
            *word = u32::from_be_bytes([self.block[k * 4], self.block[k * 4 + 1], self.block[k * 4 + 2], self.block[k * 4 + 3]]);
        }
        for k in 16..64 {
            let s0 = w[k - 15].rotate_right(7) ^ w[k - 15].rotate_right(18) ^ (w[k - 15] >> 3);
            let s1 = w[k - 2].rotate_right(17) ^ w[k - 2].rotate_right(19) ^ (w[k - 2] >> 10);
            w[k] = w[k - 16].wrapping_add(s0).wrapping_add(w[k - 7]).wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = self.state;
        for k in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = h.wrapping_add(s1).wrapping_add(ch).wrapping_add(K[k]).wrapping_add(w[k]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            (h, g, f, e, d, c, b, a) = (g, f, e, d.wrapping_add(t1), c, b, a, t1.wrapping_add(t2));
        }
        for (s, v) in self.state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *s = s.wrapping_add(v);
        }
    }

    fn finish(mut self) -> Hash {
        let bits = self.len.wrapping_mul(8);
        self.update(&[0x80]);
        while self.filled != 56 {
            self.update(&[0]);
        }
        self.block[56..].copy_from_slice(&bits.to_be_bytes());
        self.compress();
        let mut out = [0u8; 32];
        for (k, s) in self.state.iter().enumerate() {
            out[k * 4..k * 4 + 4].copy_from_slice(&s.to_be_bytes());
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pack() -> ModPack {
        ModPack { id: "tins".into(), name: "More tins".into(), version: "1.0".into(), description: "Tins.".into(), files: vec![("items.ron".into(), "[]".into()), ("buildings/shed-2.ron".into(), "()".into())] }
    }

    #[test]
    fn sha256_is_the_standard_one() {
        assert_eq!(hex(&sha256(b"")), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert_eq!(hex(&sha256(b"abc")), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(hex(&sha256(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq")), "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1");
        // Across block edges, and fed in pieces.
        let million = vec![b'a'; 1_000_000];
        assert_eq!(hex(&sha256(&million)), "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0");
        let mut h = Sha256::new();
        for part in million.chunks(977) {
            h.update(part);
        }
        assert_eq!(h.finish(), sha256(&million));
        for n in [55, 56, 63, 64, 65, 119, 120] {
            let (bytes, mut h) = (vec![7u8; n], Sha256::new());
            h.update(&bytes[..n / 2]);
            h.update(&bytes[n / 2..]);
            assert_eq!(h.finish(), sha256(&bytes), "{n}");
        }
        assert_eq!(unhex(&hex(&sha256(b"abc"))), Some(sha256(b"abc")));
    }

    #[test]
    fn a_packs_hash_is_its_contents() {
        let p = pack();
        assert_eq!(p.hash(), pack().hash());
        let mut changes: Vec<ModPack> = Vec::new();
        for edit in [
            (|p: &mut ModPack| p.id.push('x')) as fn(&mut ModPack),
            |p| p.name.push('x'),
            |p| p.version.push('x'),
            |p| p.description.push('x'),
            |p| p.files[0].1.push(' '),
            |p| p.files[1].0 = "buildings/shed-3.ron".into(),
            |p| p.files.swap(0, 1),
            |p| {
                p.files.pop();
            },
            // The same bytes cut another way between two fields.
            |p| {
                p.name = "More tins1".into();
                p.version = ".0".into();
            },
        ] {
            let mut q = pack();
            edit(&mut q);
            assert_ne!(q.hash(), p.hash(), "{q:?}");
            assert!(changes.iter().all(|o| o.hash() != q.hash()));
            changes.push(q);
        }
        assert_eq!((p.info().hash, p.info().size as usize, p.info().id.as_str()), (p.hash(), p.size(), "tins"));
    }

    #[test]
    fn a_pack_holds_only_data_files() {
        assert_eq!(pack().check(), Ok(()));
        for name in ["alarm.luau", "mod.ron", "../items.ron", "buildings/../../x.ron", "buildings/a/b.ron", "/etc/passwd", "buildings/.ron", "items.ron ", "Items.ron", "buildings\\a.ron"] {
            let mut p = pack();
            p.files.push((name.into(), String::new()));
            assert!(p.check().unwrap_err().contains("not a data file"), "{name}");
        }
        let mut twice = pack();
        twice.files.push(("items.ron".into(), String::new()));
        assert!(twice.check().unwrap_err().contains("twice"));
        let mut big = pack();
        big.files[0].1 = "x".repeat(MAX_PACK_BYTES);
        assert!(big.check().unwrap_err().contains("at most"));
        let mut id = pack();
        id.id = "../x".into();
        assert!(id.check().is_err());
    }

    #[test]
    fn the_cache_gives_back_only_what_hashes_right() {
        let dir = std::env::temp_dir().join(format!("zombye-modcache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let cache = Cache::in_saves(&dir);
        let p = pack();
        assert_eq!(cache.get(&p.id, &p.hash()), None);
        assert!(cache.listed().is_empty());
        let path = cache.put(&p).unwrap();
        assert!(path.starts_with(dir.join("mods")));
        assert_eq!(cache.get(&p.id, &p.hash()), Some(p.clone()));
        assert_eq!(cache.listed(), [p.hash()]);
        assert_eq!(to_download(&[p.info()], &cache.listed()), (0, 0));
        assert_eq!(to_download(&[p.info()], &[]), (1, p.size()));
        // Another pack's bytes under this one's name: not taken.
        let mut other = pack();
        other.files[0].1 = "[ ]".into();
        std::fs::copy(cache.put(&other).unwrap(), &path).unwrap();
        assert_eq!(cache.get(&p.id, &p.hash()), None);
        assert_eq!(cache.get(&other.id, &other.hash()), Some(other));
        // Cut short, or not a pack at all.
        std::fs::write(&path, b"ZOMBYMOD\x01").unwrap();
        assert_eq!(cache.get(&p.id, &p.hash()), None);
        std::fs::write(&path, b"junk").unwrap();
        assert_eq!(cache.get(&p.id, &p.hash()), None);
        // An id that would leave the folder is never looked up.
        assert_eq!(cache.get("../../x", &p.hash()), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
