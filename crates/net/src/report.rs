//! Problem reports sent to the developers (Linear DCM-92): crashes, panics, F12 reports, hitches,
//! prediction desyncs and a server's slow ticks, to the report service (`zombye-reports serve`,
//! `crates/reports`) where an agent can read them.
//!
//! **Where they go:** [`endpoint`]: `ZOMBYE_REPORT_URL` when the game runs (`off` turns sending
//! off), else the one compiled in by the packaging script (`ZOMBYE_REPORT_URL` at build time). A
//! checkout's build has none, so nothing is sent from development unless asked for. The address
//! is never in this repo.
//!
//! **What is sent:** a [`Report`]: what kind it is, a title, a [`signature`] that the service groups
//! the same problem by, the build, the system, an anonymous id for the install ([`install_id`]: a
//! hash of the identity key, not the key, so it can't be matched to a server's records), a few
//! details and a few files (a log's tail, a screenshot, a save). No name, address or IP is put in
//! a report, and the service keeps no IP.
//!
//! **How:** [`queue`] writes a report to a `pending/` folder first and [`flush`] sends what's
//! there, deleting each once the service has it, so a report made as the game falls over (or
//! with the network down) goes the next time anything flushes. One plain HTTP/1.1 POST per report
//! on the standard library alone; the body is [`encode`]'s: `ZRPT1\n`, the length of the RON
//! [`Meta`] (u32, little-endian), the meta, then each file's bytes in the order the meta lists them.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// The first bytes of a report's body.
pub const MAGIC: &[u8; 6] = b"ZRPT1\n";
/// The largest body the service takes (and this side sends).
pub const MAX_BODY: usize = 24 << 20;
/// Files in one report.
pub const MAX_FILES: usize = 8;
/// The largest meta.
const MAX_META: usize = 256 * 1024;
/// Reports kept waiting to be sent; the oldest go beyond this.
const MAX_PENDING: usize = 24;
/// A report waiting longer than this is dropped.
const PENDING_DAYS: u64 = 14;

/// Report kinds the game sends (the service takes any short word).
pub mod kind {
    /// The game or server panicked: a Rust panic with its message and place.
    pub const PANIC: &str = "panic";
    /// The game ended badly without a panic (a signal, an abort, an exit code).
    pub const CRASH: &str = "crash";
    /// A player pressed F12.
    pub const PLAYER: &str = "f12";
    /// A frame that took far too long.
    pub const HITCH: &str = "hitch";
    /// The client's prediction of its own survivor kept being far from the server's.
    pub const DESYNC: &str = "desync";
    /// A server tick far over its budget.
    pub const SLOW_TICK: &str = "slow_tick";
    /// Anything else that went wrong and was caught.
    pub const ERROR: &str = "error";
}

/// One file of a report, as the meta lists it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct FileMeta {
    pub name: String,
    pub len: u64,
}

/// Everything in a report but its files.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct Meta {
    /// One of [`kind`].
    pub kind: String,
    /// `client` or `server`.
    pub source: String,
    /// One line for a person: a panic's message, "Frame took 2.4 s", "Report from a player".
    pub title: String,
    /// What the service groups the same problem by ([`signature`]).
    pub signature: String,
    /// The whole message (a panic's, or what the report is about).
    pub message: String,
    /// Where in the code, when known (`crates/sim/src/ai.rs:120:9`).
    pub location: String,
    /// [`crate::install::build_name`].
    pub build: String,
    pub os: String,
    pub arch: String,
    /// [`install_id`] (empty when unknown).
    pub install: String,
    /// When it happened, seconds since 1970 (UTC).
    pub at: u64,
    /// Small facts: `frame_ms`, `zombies`, `mode`, `tick`, ...
    pub details: BTreeMap<String, String>,
    /// The files after the meta, in order.
    pub files: Vec<FileMeta>,
}

/// A report: its meta and its files' bytes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Report {
    pub meta: Meta,
    pub files: Vec<(String, Vec<u8>)>,
}

/// Seconds since 1970.
pub fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

impl Report {
    /// A report of `kind` from `source` with the build and system filled in.
    pub fn new(kind: &str, source: &str, title: impl Into<String>, signature: impl Into<String>) -> Self {
        Self {
            meta: Meta {
                kind: kind.into(),
                source: source.into(),
                title: one_line(&title.into(), 200),
                signature: signature.into(),
                build: crate::install::build_name().into(),
                os: std::env::consts::OS.into(),
                arch: std::env::consts::ARCH.into(),
                at: now_secs(),
                ..Meta::default()
            },
            files: Vec::new(),
        }
    }

    pub fn detail(mut self, key: &str, value: impl ToString) -> Self {
        self.meta.details.insert(key.into(), value.to_string());
        self
    }

    pub fn message(mut self, message: impl Into<String>) -> Self {
        self.meta.message = message.into();
        self
    }

    pub fn install(mut self, install: impl Into<String>) -> Self {
        self.meta.install = install.into();
        self
    }

    /// Add a file (skipped past [`MAX_FILES`], or if it would take the report past [`MAX_BODY`]).
    pub fn file(mut self, name: &str, bytes: Vec<u8>) -> Self {
        let size: usize = self.files.iter().map(|(_, b)| b.len()).sum();
        if self.files.len() < MAX_FILES && size + bytes.len() + MAX_META < MAX_BODY && safe_name(name) {
            self.files.push((name.into(), bytes));
        }
        self
    }

    /// Add the last `max` bytes of the file at `path`, if it can be read.
    pub fn file_tail(self, name: &str, path: &Path, max: usize) -> Self {
        match read_tail(path, max) {
            Some(bytes) => self.file(name, bytes),
            None => self,
        }
    }
}

/// The last `max` bytes of a file.
pub fn read_tail(path: &Path, max: usize) -> Option<Vec<u8>> {
    let bytes = std::fs::read(path).ok()?;
    Some(if bytes.len() > max { bytes[bytes.len() - max..].to_vec() } else { bytes })
}

/// A file name a report may carry: letters, digits, `.`, `_` and `-`, not starting with a dot.
pub fn safe_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 64 && !name.starts_with('.') && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

fn one_line(text: &str, max: usize) -> String {
    let line = text.lines().next().unwrap_or("").trim();
    line.chars().take(max).collect()
}

/// What groups reports of the same problem: the kind, the file it happened in (without the line,
/// which moves from build to build) and the first line of the message with its numbers taken out
/// (`index out of bounds: the len is # but the index is #`).
pub fn signature(kind: &str, location: &str, message: &str) -> String {
    let file = location.split(':').next().unwrap_or("");
    let mut norm = String::new();
    let mut in_number = false;
    for c in one_line(message, 160).chars() {
        let digit = c.is_ascii_digit();
        if digit && !in_number {
            norm.push('#');
        } else if !digit {
            norm.push(c);
        }
        in_number = digit || (in_number && c == '.');
    }
    [kind, file, norm.trim()].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join(" | ")
}

/// An anonymous id for an install: 12 hex digits of a hash of its identity key.
pub fn install_id(key: &[u8]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in b"zombye-report:".iter().chain(key) {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{:012x}", h & 0xffff_ffff_ffff)
}

/// The report body: see the module's docs.
pub fn encode(report: &Report) -> Vec<u8> {
    let mut meta = report.meta.clone();
    meta.files = report.files.iter().map(|(name, bytes)| FileMeta { name: name.clone(), len: bytes.len() as u64 }).collect();
    let text = ron::to_string(&meta).unwrap_or_default();
    let mut out = Vec::with_capacity(MAGIC.len() + 4 + text.len() + report.files.iter().map(|(_, b)| b.len()).sum::<usize>());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&(text.len() as u32).to_le_bytes());
    out.extend_from_slice(text.as_bytes());
    for (_, bytes) in &report.files {
        out.extend_from_slice(bytes);
    }
    out
}

/// Read a body [`encode`] made, checking everything (it came over the network).
pub fn decode(body: &[u8]) -> Result<Report, String> {
    if body.len() > MAX_BODY {
        return Err(format!("too big: {} bytes (at most {MAX_BODY})", body.len()));
    }
    let rest = body.strip_prefix(MAGIC.as_slice()).ok_or("not a report")?;
    if rest.len() < 4 {
        return Err("cut short".into());
    }
    let len = u32::from_le_bytes([rest[0], rest[1], rest[2], rest[3]]) as usize;
    if len > MAX_META || rest.len() < 4 + len {
        return Err("meta too long or cut short".into());
    }
    let text = std::str::from_utf8(&rest[4..4 + len]).map_err(|_| "meta isn't UTF-8")?;
    let meta: Meta = ron::from_str(text).map_err(|e| format!("bad meta: {e}"))?;
    if meta.kind.is_empty() || meta.kind.len() > 32 || !meta.kind.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_') {
        return Err("bad kind".into());
    }
    if meta.files.len() > MAX_FILES {
        return Err(format!("{} files (at most {MAX_FILES})", meta.files.len()));
    }
    let mut at = 4 + len;
    let mut files = Vec::with_capacity(meta.files.len());
    for f in &meta.files {
        if !safe_name(&f.name) || files.iter().any(|(n, _): &(String, Vec<u8>)| *n == f.name) {
            return Err(format!("bad file name {:?}", f.name));
        }
        let end = at.checked_add(f.len as usize).filter(|e| *e <= rest.len()).ok_or("files cut short")?;
        files.push((f.name.clone(), rest[at..end].to_vec()));
        at = end;
    }
    if at != rest.len() {
        return Err("bytes after the last file".into());
    }
    Ok(Report { meta, files })
}

/// Where reports go, or `None`: don't send.
pub fn endpoint() -> Option<String> {
    let url = match std::env::var("ZOMBYE_REPORT_URL") {
        Ok(v) => v,
        Err(_) => option_env!("ZOMBYE_REPORT_URL").unwrap_or("").to_string(),
    };
    let url = url.trim();
    (!url.is_empty() && url != "off" && url.starts_with("http://")).then(|| url.trim_end_matches('/').to_string())
}

/// The key the service asks for (`X-Zombye-Key`): it keeps out junk, it is no secret.
fn report_key() -> String {
    std::env::var("ZOMBYE_REPORT_KEY").unwrap_or_else(|_| option_env!("ZOMBYE_REPORT_KEY").unwrap_or("").to_string())
}

/// What the service said.
#[derive(Debug, PartialEq)]
pub enum Sent {
    /// Taken, under this id.
    Taken(u64),
    /// Refused for good (a bad report, a full store): don't try it again.
    Refused(String),
}

/// POST one report body to `url` (`http://host[:port][/path]`). `Err` is a network failure:
/// worth trying again later.
pub fn send(url: &str, body: &[u8], timeout: Duration) -> Result<Sent, String> {
    let rest = url.strip_prefix("http://").ok_or("only http:// addresses")?;
    let (host, path) = rest.split_once('/').map_or((rest, String::new()), |(h, p)| (h, format!("/{p}")));
    let addr = if host.contains(':') { host.to_string() } else { format!("{host}:80") };
    let target = addr.to_socket_addrs().map_err(|e| format!("{host}: {e}"))?.next().ok_or_else(|| format!("{host}: no address"))?;
    let mut stream = TcpStream::connect_timeout(&target, timeout).map_err(|e| format!("{host}: {e}"))?;
    stream.set_read_timeout(Some(timeout)).ok();
    stream.set_write_timeout(Some(timeout)).ok();
    let head = format!(
        "POST {path}/v1/report HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/x-zombye-report\r\nContent-Length: {}\r\nX-Zombye-Key: {}\r\nConnection: close\r\n\r\n",
        body.len(),
        report_key(),
    );
    stream.write_all(head.as_bytes()).and_then(|_| stream.write_all(body)).map_err(|e| format!("sending: {e}"))?;
    let mut answer = Vec::new();
    stream.take(16 * 1024).read_to_end(&mut answer).map_err(|e| format!("reading the answer: {e}"))?;
    let text = String::from_utf8_lossy(&answer);
    let status: u16 = text.split_whitespace().nth(1).and_then(|s| s.parse().ok()).ok_or("no answer")?;
    let reply = text.split_once("\r\n\r\n").map_or("", |(_, b)| b).trim().to_string();
    match status {
        200..=299 => Ok(Sent::Taken(reply.trim_start_matches("id ").parse().unwrap_or(0))),
        // Too many from here for now, or the service is busy: later.
        429 | 500..=599 => Err(format!("{status} {reply}")),
        _ => Ok(Sent::Refused(format!("{status} {reply}"))),
    }
}

/// Write `report` to `dir/pending/` to be sent by [`flush`]. Returns its path.
pub fn queue(dir: &Path, report: &Report) -> std::io::Result<PathBuf> {
    let pending = dir.join("pending");
    std::fs::create_dir_all(&pending)?;
    let mut old: Vec<PathBuf> = std::fs::read_dir(&pending)?.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().is_some_and(|x| x == "zrpt")).collect();
    old.sort();
    while old.len() >= MAX_PENDING {
        let _ = std::fs::remove_file(old.remove(0));
    }
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.subsec_nanos());
    let path = pending.join(format!("{:010}-{:09}-{}.zrpt", report.meta.at, nanos, report.meta.kind));
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, encode(report))?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

/// What a flush did.
#[derive(Debug, Default, PartialEq)]
pub struct Flushed {
    /// Ids the service gave, in the order sent.
    pub taken: Vec<u64>,
    pub refused: usize,
    /// Still waiting (the service couldn't be reached).
    pub left: usize,
    pub error: Option<String>,
}

/// Send what's waiting in `dir/pending/`, oldest first, deleting each the service took or
/// refused for good; stop at the first network failure. Nothing to send without an [`endpoint`].
pub fn flush(dir: &Path) -> Flushed {
    let mut out = Flushed::default();
    let Some(url) = endpoint() else { return out };
    let Ok(entries) = std::fs::read_dir(dir.join("pending")) else { return out };
    let mut files: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().is_some_and(|x| x == "zrpt")).collect();
    files.sort();
    let too_old = now_secs().saturating_sub(PENDING_DAYS * 86_400);
    for (i, path) in files.iter().enumerate() {
        let stamp: u64 = path.file_name().and_then(|n| n.to_str()).and_then(|n| n.split('-').next()).and_then(|s| s.parse().ok()).unwrap_or(0);
        let Ok(body) = std::fs::read(path) else { continue };
        if stamp < too_old || decode(&body).is_err() {
            let _ = std::fs::remove_file(path);
            continue;
        }
        match send(&url, &body, Duration::from_secs(10)) {
            Ok(Sent::Taken(id)) => {
                out.taken.push(id);
                let _ = std::fs::remove_file(path);
            }
            Ok(Sent::Refused(why)) => {
                out.refused += 1;
                out.error = Some(why);
                let _ = std::fs::remove_file(path);
            }
            Err(e) => {
                out.left = files.len() - i;
                out.error = Some(e);
                break;
            }
        }
    }
    out
}

/// What a panic said and where, as the hook sees it.
pub fn panic_parts(info: &std::panic::PanicHookInfo<'_>) -> (String, String) {
    let payload = info.payload();
    let message = payload.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| payload.downcast_ref::<String>().cloned()).unwrap_or_else(|| "(no message)".into());
    let location = info.location().map_or(String::new(), |l| format!("{}:{}:{}", l.file(), l.line(), l.column()));
    (message, location)
}

/// A panic report: the message, the place, the thread and a backtrace.
pub fn panic_report(source: &str, info: &std::panic::PanicHookInfo<'_>) -> Report {
    let (message, location) = panic_parts(info);
    let thread = std::thread::current().name().unwrap_or("unnamed").to_string();
    let trace = std::backtrace::Backtrace::force_capture().to_string();
    let mut report = Report::new(kind::PANIC, source, &message, signature(kind::PANIC, &location, &message)).message(message).detail("thread", thread);
    report.meta.location = location;
    report.file("backtrace.txt", trace.into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_report_comes_back_as_it_went() {
        let r = Report::new(kind::HITCH, "client", "Frame took 2.4 s\nmore", "hitch").detail("frame_ms", 2400).install("abc").file("log.txt", b"hello".to_vec()).file("shot.png", vec![1, 2, 3]);
        assert_eq!(r.meta.title, "Frame took 2.4 s");
        let back = decode(&encode(&r)).unwrap();
        assert_eq!(back.files, r.files);
        assert_eq!(back.meta.details["frame_ms"], "2400");
        assert_eq!(back.meta.files.len(), 2);
    }

    #[test]
    fn bad_bodies_are_refused() {
        let good = encode(&Report::new(kind::ERROR, "client", "x", "x").file("a.txt", b"abc".to_vec()));
        assert!(decode(&good[..good.len() - 1]).is_err());
        assert!(decode(&[good.as_slice(), b"z"].concat()).is_err());
        assert!(decode(b"nope").is_err());
        let mut r = Report::new(kind::ERROR, "client", "x", "x");
        r.files.push(("../etc/passwd".into(), vec![1]));
        assert!(decode(&encode(&r)).is_err());
        r.files.clear();
        r.meta.kind = "Bad Kind".into();
        assert!(decode(&encode(&r)).is_err());
    }

    #[test]
    fn signatures_ignore_numbers_and_lines() {
        let a = signature(kind::PANIC, "crates/sim/src/ai.rs:120:9", "index out of bounds: the len is 3 but the index is 5");
        let b = signature(kind::PANIC, "crates/sim/src/ai.rs:131:13", "index out of bounds: the len is 40 but the index is 41");
        assert_eq!(a, b);
        assert_eq!(a, "panic | crates/sim/src/ai.rs | index out of bounds: the len is # but the index is #");
        assert_ne!(a, signature(kind::PANIC, "crates/sim/src/flow.rs:1:1", "index out of bounds: the len is 3 but the index is 5"));
    }

    #[test]
    fn install_ids_are_short_and_stable() {
        assert_eq!(install_id(&[7; 32]), install_id(&[7; 32]));
        assert_ne!(install_id(&[7; 32]), install_id(&[8; 32]));
        assert_eq!(install_id(&[7; 32]).len(), 12);
    }

    #[test]
    fn the_queue_keeps_reports_until_sent() {
        let dir = std::env::temp_dir().join(format!("zombye-report-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = queue(&dir, &Report::new(kind::ERROR, "client", "x", "x")).unwrap();
        assert!(path.exists());
        assert!(decode(&std::fs::read(&path).unwrap()).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
