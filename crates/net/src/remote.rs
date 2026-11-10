//! A client's connection to a multiplayer server (#21): what the game client and the bots use
//! where single-player uses a [`crate::LocalServer`]. Intents go out, whole [`Snapshot`]s come
//! back (rebuilt from the server's deltas). See [`crate::wire`] for the protocol.

use std::io::Write;
use std::net::{Shutdown, TcpStream, ToSocketAddrs};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use zombye_sim::save::ContentPrint;
use zombye_sim::{TileMap, World, WorldConfig};

use crate::wire::{self, ClientMsg, DeltaDecoder, Hello, ServerMsg, Welcome};
use crate::{Intent, Snapshot};

/// How often a client pings (which also tells the server it is still there).
const PING_EVERY: Duration = Duration::from_secs(1);
/// A server that sends nothing for this long is taken for gone.
const SILENCE: Duration = Duration::from_secs(10);

pub struct Remote {
    pub welcome: Welcome,
    stream: TcpStream,
    /// Frames to send with the next [`Remote::flush`].
    out: Vec<u8>,
    /// From the reading thread (behind a mutex only so a `Remote` can be shared, as a Bevy
    /// resource must be; it is only ever taken through `&mut self`).
    incoming: Mutex<Receiver<(Instant, Result<ServerMsg, String>)>>,
    /// A made-up slow line, for trying out how the game plays over one ([`Remote::set_lag`]):
    /// how long each way takes, what has arrived but isn't "here" yet, and the thread that sends
    /// what we write when its time has come.
    lag_each_way: Duration,
    held: std::collections::VecDeque<(Instant, Result<ServerMsg, String>)>,
    late_out: Option<mpsc::Sender<(Instant, Vec<u8>)>>,
    decoder: DeltaDecoder,
    received: Arc<AtomicU64>,
    /// Bytes sent so far.
    pub bytes_out: u64,
    /// Intents sent (or queued to send) so far.
    pub intents_sent: u64,
    /// Why the connection ended, once it has.
    pub closed: Option<String>,
    /// The last ping's round trip, in milliseconds.
    pub ping_ms: Option<f32>,
    started: Instant,
    last_ping: Instant,
}

impl Remote {
    /// Connect to `addr` (`host:port`, or `host` for the default port) and join as `name`. Waits
    /// up to `timeout` for the connection and for the server's answer.
    pub fn connect(addr: &str, name: &str, password: &str, timeout: Duration) -> Result<Self, String> {
        let with_port = if addr.rsplit_once(':').is_some_and(|(_, p)| p.parse::<u16>().is_ok()) { addr.to_string() } else { format!("{addr}:{}", wire::DEFAULT_PORT) };
        let addrs: Vec<_> = with_port.to_socket_addrs().map_err(|e| format!("can't find {with_port}: {e}"))?.collect();
        let mut last = format!("can't find {with_port}");
        let mut stream = None;
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
        let io = |e: std::io::Error| format!("lost the connection to {with_port}: {e}");
        stream.set_nodelay(true).map_err(io)?;
        stream.set_read_timeout(Some(timeout)).map_err(io)?;
        stream.set_write_timeout(Some(Duration::from_secs(5))).map_err(io)?;
        let hello = ClientMsg::Hello(Hello { protocol: wire::PROTOCOL, name: name.to_string(), password: password.to_string() });
        let bytes_out = wire::write_frame(&mut stream, &hello).map_err(io)? as u64;
        let (answer, n) = wire::read_frame::<ServerMsg>(&mut stream, wire::MAX_SERVER_FRAME).map_err(io)?;
        let welcome = match answer {
            ServerMsg::Welcome(w) => *w,
            ServerMsg::Refused(why) | ServerMsg::Bye(why) => return Err(format!("{with_port} refused: {why}")),
            _ => return Err(format!("{with_port} isn't a Zombye server of this version")),
        };
        // A server sends 20 snapshots a second: one silent this long is gone (or the line is).
        stream.set_read_timeout(Some(SILENCE)).map_err(io)?;
        let received = Arc::new(AtomicU64::new(n as u64));
        let (tx, incoming) = mpsc::channel();
        let (mut reader, count) = (stream.try_clone().map_err(io)?, received.clone());
        std::thread::Builder::new()
            .name("net-client-read".into())
            .spawn(move || {
                loop {
                    match wire::read_frame::<ServerMsg>(&mut reader, wire::MAX_SERVER_FRAME) {
                        Ok((msg, n)) => {
                            count.fetch_add(n as u64, Ordering::Relaxed);
                            if tx.send((Instant::now(), Ok(msg))).is_err() {
                                break;
                            }
                        }
                        Err(e) => {
                            let silent = matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut);
                            let _ = tx.send((Instant::now(), Err(if silent { "the server stopped answering".to_string() } else { e.to_string() })));
                            break;
                        }
                    }
                }
            })
            .map_err(io)?;
        let now = Instant::now();
        // The first ping at once, so the HUD has one soon.
        Ok(Self { welcome, stream, out: Vec::new(), incoming: Mutex::new(incoming), lag_each_way: Duration::ZERO, held: Default::default(), late_out: None, decoder: DeltaDecoder::default(), received, bytes_out, intents_sent: 0, closed: None, ping_ms: None, started: now, last_ping: now - PING_EVERY })
    }

    /// The player this connection plays.
    pub fn player(&self) -> u32 {
        self.welcome.player
    }

    /// Play as if over a slow line (the client's `--lag MS`, tests): everything sent leaves half of
    /// `round_trip` late and everything received is held back the other half, so the ping reads
    /// `round_trip` more. For trying out lag compensation (#23) on one machine; 0 turns it off.
    pub fn set_lag(&mut self, round_trip: Duration) {
        self.lag_each_way = round_trip / 2;
        if self.lag_each_way.is_zero() {
            self.late_out = None;
        } else if self.late_out.is_none()
            && let Ok(mut stream) = self.stream.try_clone()
        {
            let (tx, rx) = mpsc::channel::<(Instant, Vec<u8>)>();
            let spawned = std::thread::Builder::new().name("net-client-lag".into()).spawn(move || {
                while let Ok((due, bytes)) = rx.recv() {
                    std::thread::sleep(due.saturating_duration_since(Instant::now()));
                    if stream.write_all(&bytes).is_err() {
                        break;
                    }
                }
            });
            self.late_out = spawned.is_ok().then_some(tx);
        }
    }

    /// Bytes received so far.
    pub fn bytes_in(&self) -> u64 {
        self.received.load(Ordering::Relaxed)
    }

    /// Queue an intent; it goes with the next [`Remote::flush`].
    pub fn send(&mut self, intent: Intent) {
        self.out.extend(wire::encode_frame(&ClientMsg::Intent(intent)));
        self.intents_sent += 1;
    }

    /// Send what is queued (and a ping when one is due). Call once a tick.
    pub fn flush(&mut self) {
        if self.closed.is_some() {
            self.out.clear();
            return;
        }
        if self.last_ping.elapsed() >= PING_EVERY {
            self.last_ping = Instant::now();
            self.out.extend(wire::encode_frame(&ClientMsg::Ping(self.started.elapsed().as_millis() as u32)));
        }
        if self.out.is_empty() {
            return;
        }
        if let Some(late) = &self.late_out {
            // The slow line: the lag thread writes it when its time has come.
            self.bytes_out += self.out.len() as u64;
            if late.send((Instant::now() + self.lag_each_way, std::mem::take(&mut self.out))).is_err() {
                self.closed.get_or_insert("lost the connection".into());
            }
            return;
        }
        match self.stream.write_all(&self.out) {
            Ok(()) => self.bytes_out += self.out.len() as u64,
            Err(e) => self.closed = Some(format!("lost the connection: {e}")),
        }
        self.out.clear();
    }

    /// The snapshots that have arrived since the last call, oldest first.
    pub fn poll(&mut self) -> Vec<Snapshot> {
        let mut snapshots = Vec::new();
        loop {
            // On a made-up slow line, what has arrived waits its time in `held`.
            let next = if self.lag_each_way.is_zero() && self.held.is_empty() {
                self.incoming.get_mut().unwrap_or_else(|e| e.into_inner()).try_recv().map(|(_, m)| m)
            } else {
                let incoming = self.incoming.get_mut().unwrap_or_else(|e| e.into_inner());
                let mut gone = false;
                loop {
                    match incoming.try_recv() {
                        Ok(m) => self.held.push_back(m),
                        Err(TryRecvError::Empty) => break,
                        Err(TryRecvError::Disconnected) => {
                            gone = true;
                            break;
                        }
                    }
                }
                match self.held.front() {
                    Some((at, _)) if at.elapsed() >= self.lag_each_way => Ok(self.held.pop_front().expect("peeked").1),
                    None if gone => Err(TryRecvError::Disconnected),
                    _ => Err(TryRecvError::Empty),
                }
            };
            match next {
                Ok(Ok(ServerMsg::Snapshot(delta))) => snapshots.push(self.decoder.apply(*delta, self.welcome.player)),
                Ok(Ok(ServerMsg::Pong(sent))) => self.ping_ms = Some((self.started.elapsed().as_millis() as u32).wrapping_sub(sent) as f32),
                Ok(Ok(ServerMsg::Bye(why))) => self.closed = Some(format!("the server closed the connection: {why}")),
                Ok(Ok(ServerMsg::Refused(why))) => self.closed = Some(format!("refused: {why}")),
                Ok(Ok(ServerMsg::Welcome(_))) => {}
                Ok(Err(e)) => {
                    self.closed.get_or_insert(format!("lost the connection: {e}"));
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.closed.get_or_insert("lost the connection".into());
                    break;
                }
            }
        }
        snapshots
    }

    /// Wait up to `timeout` for the next snapshot (the first comes with the server's next tick).
    /// If several have arrived (a client that took a while to build its map), the newest, with
    /// what the earlier ones said once only carried into it: private views (a new survivor's
    /// pockets and health come with its first snapshot alone), map changes, cracks, hits, notes
    /// and level-ups.
    pub fn wait_snapshot(&mut self, timeout: Duration) -> Result<Snapshot, String> {
        self.wait_tick(0, timeout)
    }

    /// As [`Remote::wait_snapshot`], but waits for the snapshot of server tick `tick` (or a later
    /// one): earlier ones still on their way are folded into it the same way, not returned.
    pub fn wait_tick(&mut self, tick: u64, timeout: Duration) -> Result<Snapshot, String> {
        let until = Instant::now() + timeout;
        let mut arrived: Vec<Snapshot> = Vec::new();
        loop {
            self.flush();
            arrived.append(&mut self.poll());
            if arrived.last().is_some_and(|s| s.tick >= tick) {
                let mut s = arrived.pop().expect("checked");
                // Oldest first, the newest's own last.
                let mut once = Snapshot::default();
                for mut part in std::mem::take(&mut arrived).into_iter().chain([Snapshot {
                    private: std::mem::take(&mut s.private),
                    edge_changes: std::mem::take(&mut s.edge_changes),
                    edge_damage: std::mem::take(&mut s.edge_damage),
                    hits: std::mem::take(&mut s.hits),
                    item_notes: std::mem::take(&mut s.item_notes),
                    level_ups: std::mem::take(&mut s.level_ups),
                    ..Snapshot::default()
                }]) {
                    for v in part.private {
                        match once.private.iter_mut().find(|o| o.player == v.player) {
                            Some(o) => *o = v,
                            None => once.private.push(v),
                        }
                    }
                    once.edge_changes.append(&mut part.edge_changes);
                    once.edge_damage.append(&mut part.edge_damage);
                    once.hits.append(&mut part.hits);
                    once.item_notes.append(&mut part.item_notes);
                    once.level_ups.append(&mut part.level_ups);
                }
                (s.private, s.edge_changes, s.edge_damage, s.hits, s.item_notes, s.level_ups) = (once.private, once.edge_changes, once.edge_damage, once.hits, once.item_notes, once.level_ups);
                return Ok(s);
            }
            if let Some(why) = &self.closed {
                return Err(why.clone());
            }
            if Instant::now() >= until {
                return Err("the server sent nothing".into());
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}

impl std::fmt::Debug for Remote {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Remote({} as player {})", self.welcome.name, self.welcome.player)
    }
}

impl Drop for Remote {
    fn drop(&mut self) {
        let _ = wire::write_frame(&mut self.stream, &ClientMsg::Bye);
        let _ = self.stream.shutdown(Shutdown::Both);
    }
}

/// What a client builds for itself on joining: the server's map, from the server's seed and the
/// client's own game data, and the config it was made with (the server's tunings put in), whose
/// weapons, items and recipes are the ones snapshots name by index.
pub struct Joined {
    pub map: TileMap,
    pub config: WorldConfig,
    pub player_spawn: glam::Vec2,
}

/// Build the server's map from `content` (this client's game data) as [`Welcome`] describes it.
/// Refuses, saying what differs, when the data isn't the server's: the map would be another town,
/// or snapshots would name other weapons and items.
pub fn build_map(welcome: &Welcome, content: &WorldConfig) -> Result<Joined, String> {
    let mut config = content.clone();
    config.generation = welcome.generation.clone();
    config.zombies = Some(0);
    let t = &welcome.tunings;
    (config.combat, config.health, config.infection, config.needs, config.skills, config.carry) = (t.combat.clone(), t.health.clone(), t.infection.clone(), t.needs, t.skills.clone(), t.carry.clone());
    let (ours, theirs) = (ContentPrint::of(&config), &welcome.content);
    let mut differs = Vec::new();
    for (what, same) in [
        ("building templates or street props", ours.templates == theirs.templates),
        ("weapons", ours.weapons == theirs.weapons),
        ("items", ours.items == theirs.items),
        ("recipes", ours.recipes == theirs.recipes),
    ] {
        if !same {
            differs.push(what);
        }
    }
    if !differs.is_empty() {
        return Err(format!("your game data isn't the server's ({} differ): both need the same version of the game and its data folder", differs.join(", ")));
    }
    let mut world = World::new(config);
    if world.map_base_hash() != welcome.map_hash {
        return Err("your game builds a different town from the server's seed: both need the same version of the game".into());
    }
    for c in &welcome.map_edits {
        if !world.map.in_bounds(c.tile) {
            return Err("the server sent a door outside the map".into());
        }
        world.map.set_owned_edge(c.tile, c.edge, c.kind);
    }
    let map = std::mem::replace(&mut world.map, TileMap::new(1, 1));
    Ok(Joined { map, player_spawn: world.player_spawn, config: world.config })
}
