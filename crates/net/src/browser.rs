//! The server browser's working half (#27): a list of servers read from a file or a URL, each one
//! asked what it is ([`crate::remote::probe`]: its rules, players, mods and ping) on threads of its
//! own, and whether this game could join it. The game client draws it (`browser_ui.rs`); nothing
//! here needs a window, so the tests run it against real servers on loopback.
//!
//! **No hosted service.** The list is a file anyone can write and put anywhere: the data folder's
//! `servers.ron` (shipped empty), another file, or an `http://` address. A server is in the browser
//! because a list names it, and is shown as up because it answered us, not because anything
//! vouches for it.
//!
//! **The list** is RON, like the rest of the game's data:
//!
//! ```ron
//! (servers: [
//!     (address: "play.example.org", name: "Example co-op", featured: true, note: "EU, restarts at 04:00"),
//!     (address: "203.0.113.7:7778"),
//! ])
//! ```
//!
//! `address` is `host[:port]` (port 7777 without). `name` is shown until the server answers with
//! its own, `note` is a line the list's keeper adds, `featured` puts the server in the section at
//! the top. Fields this version doesn't know are ignored, so a list can grow fields without
//! breaking older games.

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::remote;
use crate::wire::{self, Camera, ModInfo, ServerInfo};

/// The most servers a list may name, and the most of a list that is read.
pub const MAX_SERVERS: usize = 200;
pub const MAX_LIST_BYTES: usize = 256 * 1024;
/// The list file in the data folder, used when nothing else is set.
pub const LIST_FILE: &str = "servers.ron";
/// Servers asked at once.
const WORKERS: usize = 8;
const NAME_CHARS: usize = 48;
const NOTE_CHARS: usize = 120;

/// One server in a list.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct ListEntry {
    pub address: String,
    pub name: String,
    pub featured: bool,
    pub note: String,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct ServerList {
    pub servers: Vec<ListEntry>,
}

/// One line of text someone else wrote: no control characters, trimmed, at most `max` characters.
fn one_line(text: &str, max: usize) -> String {
    let clean: String = text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    clean.trim().chars().take(max).collect()
}

/// Is `address` a `host[:port]` we'd try? Letters, digits, dots, dashes and underscores, or an
/// IPv6 address in brackets.
pub fn check_address(address: &str) -> Result<(), String> {
    let a = address.trim();
    if a.is_empty() {
        return Err("no address".into());
    }
    if a.len() > 255 {
        return Err("the address is too long".into());
    }
    if let Some(bad) = a.chars().find(|c| !(c.is_ascii_alphanumeric() || ".-_:[]".contains(*c))) {
        return Err(format!("{a:?} isn't a server address (host or host:port): {bad:?} can't be in one"));
    }
    if let Some((host, port)) = a.rsplit_once(':') {
        // `[::1]` and bare IPv6 have colons that aren't a port's.
        if !host.contains(':') && !host.starts_with('[') && port.parse::<u16>().is_err() {
            return Err(format!("{a:?}: {port:?} isn't a port"));
        }
    }
    Ok(())
}

/// The key two spellings of one server share.
fn key(address: &str) -> String {
    remote::with_port(address.trim()).to_ascii_lowercase()
}

/// Read a list. Mistakes are explained by entry; a server named twice is kept once (featured if
/// either says so).
pub fn parse_list(text: &str) -> Result<ServerList, String> {
    let list: ServerList = ron::from_str(text).map_err(|e| format!("not a server list: {e}"))?;
    if list.servers.len() > MAX_SERVERS {
        return Err(format!("{} servers in the list: at most {MAX_SERVERS}", list.servers.len()));
    }
    let mut servers: Vec<ListEntry> = Vec::new();
    for (k, s) in list.servers.into_iter().enumerate() {
        check_address(&s.address).map_err(|e| format!("server {}: {e}", k + 1))?;
        let entry = ListEntry { address: s.address.trim().to_string(), name: one_line(&s.name, NAME_CHARS), featured: s.featured, note: one_line(&s.note, NOTE_CHARS) };
        match servers.iter_mut().find(|o| key(&o.address) == key(&entry.address)) {
            Some(o) => o.featured |= entry.featured,
            None => servers.push(entry),
        }
    }
    Ok(ServerList { servers })
}

/// Where a list comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    File(PathBuf),
    Http { host: String, port: u16, path: String },
}

impl Source {
    /// `http://host[:port]/path`, or a file's path. `https://` is refused with the reason: the
    /// game carries no TLS (it would be its first dependency for it).
    pub fn parse(text: &str) -> Result<Self, String> {
        let t = text.trim();
        if t.is_empty() {
            return Err("no server list set".into());
        }
        let lower = t.to_ascii_lowercase();
        if lower.starts_with("https://") {
            return Err(format!("{t}: https lists can't be read yet (the game has no TLS): use an http:// address or a file"));
        }
        if !lower.starts_with("http://") {
            return Ok(Self::File(PathBuf::from(t)));
        }
        let rest = &t["http://".len()..];
        let (authority, path) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, "/"),
        };
        let (host, port) = match authority.rsplit_once(':') {
            Some((h, p)) if !h.is_empty() && !p.contains(']') => (h, p.parse::<u16>().map_err(|_| format!("{t}: {p:?} isn't a port"))?),
            _ => (authority, 80),
        };
        if host.is_empty() || authority.contains('@') || t.chars().any(|c| c.is_control() || c == ' ') {
            return Err(format!("{t} isn't an http address"));
        }
        Ok(Self::Http { host: host.to_string(), port, path: path.to_string() })
    }

    /// The list's text.
    pub fn read(&self, timeout: Duration) -> Result<String, String> {
        match self {
            Self::File(path) => {
                let meta = std::fs::metadata(path).map_err(|e| format!("can't read {}: {e}", path.display()))?;
                if meta.len() > MAX_LIST_BYTES as u64 {
                    return Err(format!("{} is too big for a server list ({} KB at most)", path.display(), MAX_LIST_BYTES / 1024));
                }
                std::fs::read_to_string(path).map_err(|e| format!("can't read {}: {e}", path.display()))
            }
            Self::Http { host, port, path } => http_get(host, *port, path, timeout),
        }
    }
}

/// A plain HTTP GET with the standard library: one request, the connection closed after it, at
/// most [`MAX_LIST_BYTES`] of body, `timeout` for the whole of it. No redirects, no TLS.
fn http_get(host: &str, port: u16, path: &str, timeout: Duration) -> Result<String, String> {
    let at = format!("http://{host}:{port}{path}");
    let deadline = Instant::now() + timeout;
    let addrs: Vec<_> = (host.trim_matches(['[', ']']), port).to_socket_addrs().map_err(|e| format!("can't find {host}: {e}"))?.collect();
    let mut stream = None;
    let mut last = format!("can't find {host}");
    for a in addrs {
        match TcpStream::connect_timeout(&a, timeout) {
            Ok(s) => {
                stream = Some(s);
                break;
            }
            Err(e) => last = format!("can't connect to {a}: {e}"),
        }
    }
    let mut stream = stream.ok_or(last)?;
    let io = |e: std::io::Error| format!("{at}: {e}");
    stream.set_write_timeout(Some(timeout)).map_err(io)?;
    let host_header = if port == 80 { host.to_string() } else { format!("{host}:{port}") };
    let request = format!("GET {path} HTTP/1.0\r\nHost: {host_header}\r\nUser-Agent: zombye\r\nAccept: */*\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).map_err(io)?;
    // Headers and body, up to the limit (and a little for the headers).
    let limit = MAX_LIST_BYTES + 16 * 1024;
    let mut bytes = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(format!("{at}: no answer in {:.0} s", timeout.as_secs_f32()));
        }
        stream.set_read_timeout(Some(left)).map_err(io)?;
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                bytes.extend_from_slice(&buf[..n]);
                if bytes.len() > limit {
                    return Err(format!("{at}: too big for a server list ({} KB at most)", MAX_LIST_BYTES / 1024));
                }
            }
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => return Err(format!("{at}: no answer in {:.0} s", timeout.as_secs_f32())),
            Err(e) => return Err(io(e)),
        }
    }
    let split = bytes.windows(4).position(|w| w == b"\r\n\r\n").ok_or(format!("{at}: not an HTTP answer"))?;
    let head = String::from_utf8_lossy(&bytes[..split]).into_owned();
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap_or_default();
    let status: u16 = status_line.split_whitespace().nth(1).and_then(|s| s.parse().ok()).filter(|_| status_line.starts_with("HTTP/")).ok_or(format!("{at}: not an HTTP answer"))?;
    let header = |name: &str| head.split("\r\n").skip(1).filter_map(|l| l.split_once(':')).find(|(k, _)| k.trim().eq_ignore_ascii_case(name)).map(|(_, v)| v.trim().to_string());
    if (300..400).contains(&status) {
        let to = one_line(&header("location").unwrap_or_default(), 200);
        return Err(format!("{at}: moved to {to} (redirects aren't followed{})", if to.to_ascii_lowercase().starts_with("https") { ", and https lists can't be read yet" } else { "" }));
    }
    if status != 200 {
        return Err(format!("{at}: the web server said {status}"));
    }
    if header("transfer-encoding").is_some_and(|v| v.to_ascii_lowercase().contains("chunked")) {
        return Err(format!("{at}: sent in chunks, which this game can't read"));
    }
    String::from_utf8(bytes[split + 4..].to_vec()).map_err(|_| format!("{at}: not text"))
}

/// What we know of a server.
#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    /// Asked, no answer yet.
    Asking,
    Online { info: Box<ServerInfo>, ping_ms: u32 },
    /// No answer, and why.
    Offline(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub entry: ListEntry,
    /// The address with its port: what is joined, and what names the row.
    pub addr: String,
    /// Added by hand in the browser rather than named by the list.
    pub mine: bool,
    pub status: Status,
}

impl Row {
    /// The server's own name once it answered, else the list's, else its address.
    pub fn name(&self) -> &str {
        match &self.status {
            Status::Online { info, .. } if !info.rules.name.trim().is_empty() => &info.rules.name,
            _ if !self.entry.name.is_empty() => &self.entry.name,
            _ => &self.addr,
        }
    }

    pub fn info(&self) -> Option<&ServerInfo> {
        match &self.status {
            Status::Online { info, .. } => Some(info),
            _ => None,
        }
    }
}

/// The list itself.
#[derive(Clone, Debug, PartialEq)]
pub enum ListState {
    Loading,
    Ready,
    /// Couldn't be read, and why. Servers added by hand are still asked.
    Failed(String),
}

enum Event {
    List(u32, Result<ServerList, String>),
    Probe(u32, String, Result<(ServerInfo, Duration), String>),
}

/// A list of servers being asked what they are. [`Browser::poll`] takes up the answers that have
/// come in; nothing here blocks.
pub struct Browser {
    source: String,
    mine: Vec<String>,
    pub list: ListState,
    rows: Vec<Row>,
    tx: Sender<Event>,
    /// (In a mutex only so that a `Browser` can be shared between threads, as a game resource is.)
    rx: Mutex<Receiver<Event>>,
    /// Answers to an earlier refresh are dropped.
    generation: u32,
    timeout: Duration,
}

impl Browser {
    /// Start reading the list at `source` (a file or an `http://` address; empty: no list) and
    /// asking its servers and those in `mine` (added by hand).
    pub fn open(source: &str, mine: &[String]) -> Self {
        Self::with_timeout(source, mine, Duration::from_secs(5))
    }

    /// As [`Browser::open`], waiting `timeout` for the list and for each server.
    pub fn with_timeout(source: &str, mine: &[String], timeout: Duration) -> Self {
        let (tx, rx) = mpsc::channel();
        let mut b = Self { source: source.trim().to_string(), mine: Vec::new(), list: ListState::Loading, rows: Vec::new(), tx, rx: Mutex::new(rx), generation: 0, timeout };
        for a in mine {
            if check_address(a).is_ok() && !b.mine.iter().any(|m| key(m) == key(a)) {
                b.mine.push(a.trim().to_string());
            }
        }
        b.refresh();
        b
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    /// The servers added by hand, as they were typed.
    pub fn mine(&self) -> &[String] {
        &self.mine
    }

    /// Read the list again and ask every server again. What was known stays on show until the
    /// new answers come.
    pub fn refresh(&mut self) {
        self.generation += 1;
        let generation = self.generation;
        for r in &mut self.rows {
            r.status = Status::Asking;
        }
        // Our own servers don't wait for the list.
        let mine: Vec<String> = self.mine.clone();
        for a in &mine {
            self.put_row(ListEntry { address: a.clone(), ..Default::default() }, true);
        }
        self.ask(self.rows.iter().filter(|r| r.mine).map(|r| r.addr.clone()).collect());
        if self.source.is_empty() {
            self.rows.retain(|r| r.mine);
            self.list = ListState::Ready;
            return;
        }
        self.list = ListState::Loading;
        let (tx, source, timeout) = (self.tx.clone(), self.source.clone(), self.timeout);
        let read = move || {
            let list = Source::parse(&source).and_then(|s| s.read(timeout)).and_then(|text| parse_list(&text));
            let _ = tx.send(Event::List(generation, list));
        };
        if std::thread::Builder::new().name("server-list".into()).spawn(read).is_err() {
            self.list = ListState::Failed("can't start a thread to read the list".into());
        }
    }

    /// Add a server by hand (or ask it again if it's there). Its row's address.
    pub fn add(&mut self, address: &str) -> Result<String, String> {
        check_address(address)?;
        let address = address.trim().to_string();
        if !self.rows.iter().any(|r| key(&r.addr) == key(&address)) && !self.mine.iter().any(|m| key(m) == key(&address)) {
            self.mine.push(address.clone());
        }
        let addr = self.put_row(ListEntry { address, ..Default::default() }, true);
        if let Some(r) = self.rows.iter_mut().find(|r| r.addr == addr) {
            r.status = Status::Asking;
        }
        self.ask(vec![addr.clone()]);
        Ok(addr)
    }

    /// Take a server added by hand off the list. False for one the list names.
    pub fn remove(&mut self, addr: &str) -> bool {
        let Some(i) = self.rows.iter().position(|r| r.addr == addr && r.mine) else { return false };
        self.rows.remove(i);
        self.mine.retain(|m| key(m) != key(addr));
        true
    }

    /// The row for `entry`, made if it isn't there; its address.
    fn put_row(&mut self, entry: ListEntry, mine: bool) -> String {
        let k = key(&entry.address);
        match self.rows.iter_mut().find(|r| key(&r.addr) == k) {
            Some(r) => {
                if !mine {
                    // The list's word on a server we'd added by hand too.
                    r.entry = entry;
                    r.mine = false;
                }
                r.addr.clone()
            }
            None => {
                let addr = remote::with_port(&entry.address);
                self.rows.push(Row { entry, addr: addr.clone(), mine, status: Status::Asking });
                addr
            }
        }
    }

    /// Ask these servers, a few at a time, each on a worker thread.
    fn ask(&self, addrs: Vec<String>) {
        if addrs.is_empty() {
            return;
        }
        let workers = addrs.len().min(WORKERS);
        let queue = Arc::new(Mutex::new(VecDeque::from(addrs)));
        for _ in 0..workers {
            let (queue, tx, generation, timeout) = (queue.clone(), self.tx.clone(), self.generation, self.timeout);
            let work = move || {
                loop {
                    let Some(addr) = queue.lock().ok().and_then(|mut q| q.pop_front()) else { return };
                    let answer = remote::probe(&addr, timeout);
                    if tx.send(Event::Probe(generation, addr, answer)).is_err() {
                        return;
                    }
                }
            };
            let _ = std::thread::Builder::new().name("server-probe".into()).spawn(work);
        }
    }

    /// Take up what has come in. True if anything on show changed.
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        let events: Vec<Event> = self.rx.get_mut().map(|rx| rx.try_iter().collect()).unwrap_or_default();
        for event in events {
            match event {
                Event::List(g, _) | Event::Probe(g, _, _) if g != self.generation => {}
                Event::List(_, Ok(list)) => {
                    // The list's servers in its order, then ours; a server it no longer names goes.
                    let named: Vec<String> = list.servers.iter().map(|s| key(&s.address)).collect();
                    self.rows.retain(|r| r.mine || named.contains(&key(&r.addr)));
                    let mut ask = Vec::new();
                    for entry in list.servers {
                        let was_mine = self.rows.iter().any(|r| key(&r.addr) == key(&entry.address) && r.mine);
                        let addr = self.put_row(entry, false);
                        if !was_mine {
                            ask.push(addr);
                        }
                    }
                    self.rows.sort_by_key(|r| named.iter().position(|n| *n == key(&r.addr)).unwrap_or(usize::MAX));
                    self.ask(ask);
                    self.list = ListState::Ready;
                    changed = true;
                }
                Event::List(_, Err(e)) => {
                    self.rows.retain(|r| r.mine);
                    self.list = ListState::Failed(e);
                    changed = true;
                }
                Event::Probe(_, addr, answer) => {
                    if let Some(r) = self.rows.iter_mut().find(|r| r.addr == addr) {
                        r.status = match answer {
                            Ok((info, ping)) => Status::Online { info: Box::new(clean_info(info)), ping_ms: ping.as_millis().min(9999) as u32 },
                            Err(e) => Status::Offline(e),
                        };
                        changed = true;
                    }
                }
            }
        }
        changed
    }

    /// Nothing more is on its way: the list is read (or couldn't be) and every server answered or didn't.
    pub fn settled(&self) -> bool {
        self.list != ListState::Loading && self.rows.iter().all(|r| r.status != Status::Asking)
    }

    /// Wait until [`Browser::settled`] (tests, and tools without a frame loop). False on `timeout`.
    pub fn wait(&mut self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            self.poll();
            if self.settled() {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    pub fn row(&self, addr: &str) -> Option<&Row> {
        self.rows.iter().find(|r| r.addr == addr)
    }

    /// The featured servers, in the list's order.
    pub fn featured(&self) -> Vec<&Row> {
        self.rows.iter().filter(|r| r.entry.featured).collect()
    }

    /// The rest: those that answered first, quickest first, then those still asked, then those
    /// that didn't answer; otherwise in the list's order.
    pub fn others(&self) -> Vec<&Row> {
        let mut rows: Vec<&Row> = self.rows.iter().filter(|r| !r.entry.featured).collect();
        rows.sort_by_key(|r| match &r.status {
            Status::Online { ping_ms, .. } => (0, *ping_ms),
            Status::Asking => (1, 0),
            Status::Offline(_) => (2, 0),
        });
        rows
    }

    /// Every row as shown: featured, then the others.
    pub fn shown(&self) -> Vec<&Row> {
        let mut rows = self.featured();
        rows.extend(self.others());
        rows
    }

    /// "5 servers, 3 up, 41 playing".
    pub fn summary(&self) -> String {
        let up = self.rows.iter().filter(|r| r.info().is_some()).count();
        let playing: u32 = self.rows.iter().filter_map(|r| r.info()).map(|i| i.online).sum();
        let n = self.rows.len();
        format!("{n} server{}, {up} up, {playing} playing", if n == 1 { "" } else { "s" })
    }
}

/// A server's answer is someone else's text too: one line each, and no more than a sane number.
fn clean_info(mut info: ServerInfo) -> ServerInfo {
    info.rules.name = one_line(&info.rules.name, NAME_CHARS);
    info.version = one_line(&info.version, 24);
    info.mods.truncate(64);
    for m in &mut info.mods {
        *m = ModInfo { id: one_line(&m.id, 48), name: one_line(&m.name, NAME_CHARS), version: one_line(&m.version, 24) };
    }
    info.rules.start_kit.truncate(12);
    for (item, _) in &mut info.rules.start_kit {
        *item = one_line(item, NAME_CHARS);
    }
    info
}

/// A server's mods in a few words: "Alarm clock 1.0, Winter" or "none".
pub fn mods_text(mods: &[ModInfo]) -> String {
    if mods.is_empty() {
        return "none".into();
    }
    let names: Vec<String> = mods.iter().map(|m| {
        let name = if m.name.is_empty() { &m.id } else { &m.name };
        if m.version.is_empty() { name.clone() } else { format!("{name} {}", m.version) }
    }).collect();
    names.join(", ")
}

/// Could this game join that server now? `mods` are the ids of the mods this game has loaded
/// (`--mod`), `agent` whether a program plays it. `Err` says why not, in words for the player. A
/// wrong password, a name in use and other game data are only found out by joining.
pub fn can_join(info: &ServerInfo, mods: &[String], agent: bool) -> Result<(), String> {
    if info.protocol != wire::PROTOCOL {
        return Err(format!("it runs version {} (protocol {}), this game speaks protocol {}: both need the same version", info.version, info.protocol, wire::PROTOCOL));
    }
    if info.rules.camera != Camera::Isometric {
        return Err("it is first person only, and this version of the game has no first-person camera yet".into());
    }
    if agent && info.rules.agents == wire::Agents::Refused {
        return Err("its rules don't allow agents".into());
    }
    let missing: Vec<&str> = info.mods.iter().map(|m| m.id.as_str()).filter(|id| !mods.iter().any(|m| m == id)).collect();
    if !missing.is_empty() {
        return Err(format!("it runs the mod{} {}: start the game with the same --mod folder{}", if missing.len() == 1 { "" } else { "s" }, missing.join(", "), if missing.len() == 1 { "" } else { "s" }));
    }
    let extra: Vec<&str> = mods.iter().map(String::as_str).filter(|id| !info.mods.iter().any(|m| m.id == *id)).collect();
    if !extra.is_empty() {
        return Err(format!("this game has the mod{} {} loaded and the server doesn't: start the game without", if extra.len() == 1 { "" } else { "s" }, extra.join(", ")));
    }
    if info.online >= info.rules.max_players {
        return Err(format!("it is full ({} of {})", info.online, info.rules.max_players));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_list_is_read_and_its_mistakes_explained() {
        let list = parse_list(
            r#"(servers: [
                (address: "play.example.org", name: "  Example\n co-op ", featured: true, note: "EU", region: "a field from the future"),
                (address: "203.0.113.7:7778"),
                (address: "PLAY.example.org:7777"),
                (address: "[::1]:7777", featured: true),
            ])"#,
        )
        .unwrap();
        assert_eq!(list.servers.len(), 3, "the same server twice is one row");
        assert_eq!(list.servers[0], ListEntry { address: "play.example.org".into(), name: "Example  co-op".into(), featured: true, note: "EU".into() });
        assert_eq!(list.servers[1], ListEntry { address: "203.0.113.7:7778".into(), ..Default::default() });
        assert!(list.servers[2].featured);
        assert_eq!(parse_list("()").unwrap().servers.len(), 0);
        assert_eq!(parse_list("(servers: [])").unwrap().servers.len(), 0);

        for (text, why) in [
            ("servers: nope", "not a server list"),
            (r#"(servers: [(name: "no address")])"#, "server 1: no address"),
            (r#"(servers: [(address: "ok.example"), (address: "two words")])"#, "server 2"),
            (r#"(servers: [(address: "http://example.org/")])"#, "can't be in one"),
            (r#"(servers: [(address: "example.org:port")])"#, "isn't a port"),
        ] {
            let e = parse_list(text).unwrap_err();
            assert!(e.contains(why), "{text}: {e}");
        }
        let many = format!("(servers: [{}])", (0..=MAX_SERVERS).map(|k| format!("(address: \"s{k}.example\")")).collect::<Vec<_>>().join(","));
        assert!(parse_list(&many).unwrap_err().contains("at most"));
        // A long name or note is cut, not refused.
        let long = parse_list(&format!("(servers: [(address: \"a.example\", name: \"{}\", note: \"{}\")])", "n".repeat(300), "x".repeat(300))).unwrap();
        assert_eq!((long.servers[0].name.len(), long.servers[0].note.len()), (NAME_CHARS, NOTE_CHARS));
    }

    #[test]
    fn sources_are_files_or_plain_http() {
        assert_eq!(Source::parse("saves/my-servers.ron"), Ok(Source::File("saves/my-servers.ron".into())));
        assert_eq!(Source::parse("http://lists.example.org/zombye.ron"), Ok(Source::Http { host: "lists.example.org".into(), port: 80, path: "/zombye.ron".into() }));
        assert_eq!(Source::parse(" HTTP://127.0.0.1:8080 "), Ok(Source::Http { host: "127.0.0.1".into(), port: 8080, path: "/".into() }));
        assert!(Source::parse("https://lists.example.org/zombye.ron").unwrap_err().contains("no TLS"));
        assert!(Source::parse("http://user@host/x").is_err());
        assert!(Source::parse("http://host:port/x").unwrap_err().contains("isn't a port"));
        assert!(Source::parse("  ").is_err());
        assert!(Source::File("no/such/list.ron".into()).read(Duration::from_secs(1)).unwrap_err().contains("can't read"));
    }

    fn info(online: u32) -> ServerInfo {
        ServerInfo { protocol: wire::PROTOCOL, version: "0.1.0".into(), rules: wire::Rules { name: "Test".into(), max_players: 8, ..Default::default() }, online, password: false, mods: Vec::new() }
    }

    #[test]
    fn joining_is_checked_before_leaving_your_own_world() {
        assert_eq!(can_join(&info(0), &[], false), Ok(()));
        assert!(can_join(&info(8), &[], false).unwrap_err().contains("full (8 of 8)"));
        let other = ServerInfo { protocol: wire::PROTOCOL + 1, ..info(0) };
        assert!(can_join(&other, &[], false).unwrap_err().contains("same version"));
        let first = ServerInfo { rules: wire::Rules { camera: Camera::FirstPerson, ..info(0).rules }, ..info(0) };
        assert!(can_join(&first, &[], false).unwrap_err().contains("first person"));
        let no_agents = ServerInfo { rules: wire::Rules { agents: wire::Agents::Refused, ..info(0).rules }, ..info(0) };
        assert_eq!(can_join(&no_agents, &[], false), Ok(()));
        assert!(can_join(&no_agents, &[], true).unwrap_err().contains("agents"));
        let modded = ServerInfo { mods: vec![ModInfo { id: "alarm_clock".into(), name: "Alarm clock".into(), version: "1.0".into() }], ..info(0) };
        assert!(can_join(&modded, &[], false).unwrap_err().contains("runs the mod alarm_clock"));
        assert_eq!(can_join(&modded, &["alarm_clock".to_string()], false), Ok(()));
        assert!(can_join(&info(0), &["winter".to_string()], false).unwrap_err().contains("winter"));
        assert_eq!(mods_text(&modded.mods), "Alarm clock 1.0");
        assert_eq!(mods_text(&[]), "none");
    }

    #[test]
    fn what_a_server_says_is_made_one_line() {
        let mut i = info(1);
        i.rules.name = format!("Evil\nserver{}", "!".repeat(200));
        i.mods = (0..100).map(|k| ModInfo { id: format!("m{k}"), name: "a\tb".into(), version: String::new() }).collect();
        let c = clean_info(i);
        assert!(!c.rules.name.contains('\n') && c.rules.name.chars().count() == NAME_CHARS);
        assert_eq!((c.mods.len(), c.mods[0].name.as_str()), (64, "a b"));
    }
}
