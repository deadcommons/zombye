//! The multiplayer server (#21): a world, a TCP listener and the clients connected to it.
//!
//! [`Host::tick`] is one server tick: take new connections and what clients sent, apply each
//! client's intents as its own player, step the world, and send every client its snapshot (cut
//! down to its area of interest, as a delta, within its byte budget; #22, see [`crate::wire`]).
//! The caller runs it at the tick rate
//! ([`Host::run`], or by hand in tests). Each connection has a thread reading frames into one
//! channel and a thread writing frames out of a bounded queue, so a slow or stuck client never
//! holds the tick up: a client whose queue fills is dropped.
//!
//! **Input, one a tick.** Over a network a client's per-tick inputs arrive in bursts (none this
//! tick, two the next). Each connection's intents wait in a queue, and a tick applies them in
//! order up to and including one movement input, so every input moves the player for exactly one
//! tick, as the client predicted; with none waiting the last one holds, and a queue that grows
//! (a client running fast, or a burst after a stall) is caught up by skipping to the newest few.
//!
//! **Who is who.** A client says its name. A name whose survivor nobody is playing takes that
//! survivor up again; otherwise it gets a new one, at a place round the edge of the town away
//! from other players and zombies, with the start kit and some seconds of spawn protection.
//!
//! **Logging out (#68).** A client that leaves (or is dropped) leaves its survivor lying where it
//! was, asleep and defenceless, for the server's logout time (shorter somewhere safe), so leaving
//! can't end a fight; then it is out of the world, kept whole, until its player is back
//! (`zombye_sim::logout`). Killed meanwhile, it is a dead player whose things lie where it fell.
//! Clients only act as their own player: every
//! intent's `player` is overwritten with the sender's, and the dev and debug intents are dropped
//! unless the server allows them.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io::{self, Write};
use std::net::{IpAddr, Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender, TrySendError};
use std::time::{Duration, Instant};

use glam::{IVec2, Vec2};
use zombye_sim::save::ContentPrint;
use zombye_sim::{TICK_DT, World};

use crate::identity::{self, Key};
use crate::modpack::{Hash, ModPack};
use crate::wire::{self, Agents, Camera, ClientMsg, DeltaEncoder, Hello, Limits, MapInterest, Rules, ServerInfo, ServerMsg, Tunings, Welcome};
use crate::{EdgeDamage, Intent, Snapshot, apply_intent};

/// Where new survivors start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Spawn {
    /// Spread round the edge of the town (open PvP, docs/PLAN.md §4.8).
    Spread,
    /// Together, outside the house a single player wakes up in (co-op).
    Together,
}

#[derive(Clone, Debug)]
pub struct HostOptions {
    /// The server's name, told to clients.
    pub name: String,
    /// Clients connected at once (at most [`wire::MAX_PLAYERS`]).
    pub max_players: usize,
    /// Take dev toggles and debug intents from clients.
    pub allow_dev: bool,
    /// Clients must give this to join; empty: none.
    pub password: String,
    /// `None`: spread with PvP on, together with it off.
    pub spawn: Option<Spawn>,
    /// Seconds a survivor stays in the world, asleep and defenceless, after its player leaves
    /// (#68), so leaving can't end a fight; and the shorter time when it was left somewhere safe
    /// (`World::logout_is_safe`). After that it is out of the world until its player is back.
    pub logout_secs: f32,
    pub logout_safe_secs: f32,
    /// Seconds a new survivor can't be hurt by other players (ended early by attacking; 0: none).
    /// Only matters with PvP on.
    pub spawn_protection_secs: f32,
    /// What a new survivor starts with (`content::load_start_kit`).
    pub kit: zombye_sim::logout::StartKit,
    /// Bytes a second down to each client (#22): what its snapshots are kept within, zombies
    /// further off getting coarser when a crowd wouldn't fit (see [`wire::Limits`]).
    pub budget: u32,
    /// Fair information (#66, [`crate::visibility`]): tell a client of another player only while
    /// its survivor sees them, or as a sound when it hears them. `None`: on with PvP, off in co-op.
    pub fair_sight: Option<bool>,
    /// The camera this server's players play with (#69). A rule clients keep: one that hasn't the
    /// camera doesn't join.
    pub camera: Camera,
    /// Whether clients that say they are programs may join, and whether others are told (#69).
    pub agents: Agents,
    /// The mods this server's world was made with (`--mod`), told to whoever asks what server
    /// this is (#27: the server browser shows them, and a client needs the same ones).
    pub mods: Vec<wire::ModInfo>,
    /// The data of those mods, handed to clients that haven't got it (#24, [`crate::modpack`]):
    /// set both with [`HostOptions::with_mods`]. A mod listed in `mods` with no pack here must be
    /// installed by hand on each client. Read once, when the host starts.
    pub packs: Vec<ModPack>,
}

impl HostOptions {
    /// Run with these mods: told to whoever asks, and handed to clients that join without them
    /// (those small enough: `ModPack::check`; the rest are told only, and the names of those
    /// are returned for the server to say so).
    pub fn with_mods(&mut self, packs: Vec<ModPack>) -> Vec<String> {
        self.mods = packs.iter().map(ModPack::info).collect();
        let (offered, kept_back): (Vec<ModPack>, Vec<ModPack>) = packs.into_iter().partition(|p| p.check().is_ok());
        self.packs = offered;
        kept_back.into_iter().map(|p| p.id).collect()
    }
}

/// The default budget per client: a quarter of a megabit.
pub const DEFAULT_BUDGET: u32 = 32_000;
/// The least and most a server may be set to.
pub const BUDGET_RANGE: (u32, u32) = (8_000, 1_000_000);
/// A zombie the client hasn't got is taken up this far inside the edge of its view, so one
/// walking along the edge doesn't come and go (metres).
const ADMIT_INSIDE: f32 = 6.0;
/// About what a snapshot takes before any zombies (its header, our own player, a few others).
const FRAME_BYTES: f32 = 150.0;

impl Default for HostOptions {
    fn default() -> Self {
        Self {
            name: "Zombye server".into(),
            max_players: wire::DEFAULT_PLAYERS,
            allow_dev: false,
            password: String::new(),
            spawn: None,
            logout_secs: DEFAULT_LOGOUT_SECS,
            logout_safe_secs: DEFAULT_LOGOUT_SAFE_SECS,
            spawn_protection_secs: DEFAULT_SPAWN_PROTECTION_SECS,
            kit: Vec::new(),
            budget: DEFAULT_BUDGET,
            fair_sight: None,
            camera: Camera::default(),
            agents: Agents::default(),
            mods: Vec::new(),
            packs: Vec::new(),
        }
    }
}

/// The server rules' defaults (#68; ours): a logged-out survivor lies in the world for 30 s, 5 s
/// when left somewhere safe, and a new one has 10 s of protection from other players.
pub const DEFAULT_LOGOUT_SECS: f32 = 30.0;
pub const DEFAULT_LOGOUT_SAFE_SECS: f32 = 5.0;
pub const DEFAULT_SPAWN_PROTECTION_SECS: f32 = 10.0;
/// Spread spawns: places tried round the edge of the town, and how far a new survivor starts from
/// every living player and zombie if anywhere allows it (metres).
const SPAWN_PLACES: u32 = 64;
const SPAWN_CLEAR_OF_PLAYERS: f32 = 40.0;
const SPAWN_CLEAR_OF_ZOMBIES: f32 = 15.0;

/// Frames waiting to be written to one client before it is dropped as too slow (5 s of snapshots).
const OUT_QUEUE: usize = 100;
/// Intents waiting from one client before it is dropped as flooding.
const MAX_QUEUE: usize = 400;
/// Movement inputs kept waiting per client: more are skipped down to `KEEP_INPUTS`.
/// (A client drawing few frames a second sends a frame's worth of inputs at once: 10 is half a
/// second.)
const MAX_INPUTS: usize = 10;
const KEEP_INPUTS: usize = 2;
/// A client's lines of chat are taken at most this often (ticks: half a second); the rest are
/// dropped, so nobody floods the others' screens or rings a dinner bell twenty times a second.
const SAY_EVERY: u64 = 10;
/// A client that sends nothing for this long is gone (clients ping every second).
const IDLE: Duration = Duration::from_secs(20);
const HELLO_TIMEOUT: Duration = Duration::from_secs(5);
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);
/// Connections that haven't said hello yet, beyond the players allowed.
const MAX_PENDING: usize = 16;

enum Event {
    Hello(u64, Hello, TcpStream),
    /// "What server are you?" from someone not joining (#69).
    Query(TcpStream),
    /// "Send me these mods" from someone about to join (#24).
    Fetch(TcpStream, wire::Fetch),
    Msg(u64, ClientMsg, usize),
    Closed(u64),
}

struct Conn {
    id: u64,
    player: u32,
    name: String,
    /// The key it proved it holds on joining (DCM-66).
    key: Key,
    addr: String,
    out: SyncSender<Vec<u8>>,
    queue: VecDeque<Intent>,
    encoder: DeltaEncoder,
    /// The doors and windows it is behind on (out of its interest when they changed).
    map: MapInterest,
    /// Bytes it may be sent now: filled at the budget's rate up to a second's worth, spent by
    /// every frame (below zero when what must go was more).
    tokens: f32,
    /// The world's tick when it last said something (#67): one line every `SAY_EVERY` ticks.
    said_at: Option<u64>,
    /// Zombie updates held back for the budget, in all.
    held: u64,
    bytes_out: u64,
    bytes_in: u64,
    /// Intents received so far.
    intents: u64,
    /// Set when it is to be dropped, with the reason.
    closing: Option<String>,
    joined: Instant,
}

/// Someone an operator has shut out (#26): a hello with this name, or from this address, is
/// refused. Kept by the host while it runs; the caller keeps them across restarts if it wants to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ban {
    /// The name banned (compared without regard to case).
    pub name: String,
    /// The address banned with it; never this machine's own (loopback), which would shut out
    /// everyone playing from the server's computer.
    pub ip: Option<IpAddr>,
    /// The player's key banned with it (DCM-66), so a new name doesn't get round the ban.
    pub key: Option<Key>,
    pub why: String,
}

/// Whose a survivor is (DCM-66): the key that plays it, and the name it last played under (what
/// others see). `key` is `None` only for a name kept from before keys (an old `PATH.players`
/// line): the first key that joins under that name takes the survivor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Owner {
    pub name: String,
    pub key: Option<Key>,
}

/// One connected client, for reports.
#[derive(Clone, Debug)]
pub struct ClientInfo {
    pub player: u32,
    pub name: String,
    pub addr: String,
    pub bytes_out: u64,
    pub bytes_in: u64,
    /// Zombies it has now, and zombie updates held back for its budget so far (#22).
    pub zombies: u32,
    pub held: u64,
    /// Doors and windows changed outside its interest that it hasn't been told of.
    pub map_owed: usize,
    /// Seconds since it joined.
    pub secs: f32,
}

/// Totals since the server started.
#[derive(Clone, Copy, Debug, Default)]
pub struct HostStats {
    pub ticks: u64,
    /// The last tick, all of it (intents, the world, snapshots), and the world's step alone.
    pub tick_ms: f32,
    pub sim_ms: f32,
    pub worst_tick_ms: f32,
    pub bytes_out: u64,
    pub bytes_in: u64,
    pub joined: u64,
    pub left: u64,
    pub refused: u64,
    /// Dev and debug intents dropped.
    pub refused_dev: u64,
    /// Zombie updates held back for clients' budgets (#22).
    pub held: u64,
    /// Times someone asked what server this is without joining (#69).
    pub queries: u64,
    /// Mods handed to clients that hadn't got them (#24), and their bytes.
    pub mods_sent: u64,
    pub mod_bytes: u64,
}

pub struct Host {
    pub world: World,
    pub options: HostOptions,
    listener: TcpListener,
    events: Receiver<Event>,
    events_tx: Sender<Event>,
    conns: Vec<Conn>,
    next_conn: u64,
    /// Sockets open: connected clients and those still saying hello.
    open: Arc<AtomicUsize>,
    /// Every survivor someone has played here, by player id: its key and name.
    owners: BTreeMap<u32, Owner>,
    /// The players whose client said it is a program when it last joined (#69). With the agents
    /// rule at `Marked`, their views say so to whoever sees them.
    agents: BTreeSet<u32>,
    bans: Vec<Ban>,
    /// The mods handed out (#24): each pack's hash, id and its frame as it goes on the wire,
    /// encoded once.
    offer: Vec<(Hash, String, Arc<Vec<u8>>)>,
    /// The next place round the town to try for a new survivor (`new_spawn`).
    next_spawn: u32,
    pub stats: HostStats,
    /// What happened (joins, leaves, refusals) since the caller last emptied it, to log.
    pub log: Vec<String>,
}

impl Host {
    /// Serve `world` on `addr` (e.g. `0.0.0.0:7777`; port 0 picks a free one, see
    /// [`Host::local_addr`]).
    pub fn bind(world: World, addr: impl ToSocketAddrs, mut options: HostOptions) -> io::Result<Self> {
        let listener = TcpListener::bind(addr)?;
        listener.set_nonblocking(true)?;
        options.max_players = options.max_players.clamp(1, wire::MAX_PLAYERS);
        let (events_tx, events) = mpsc::channel();
        // Carry on round the town from where a loaded world's survivors left off.
        let next_spawn = (world.players.len() + world.parked().len()) as u32;
        let offer = options.packs.iter().filter(|p| p.check().is_ok()).map(|p| (p.hash(), p.id.clone(), Arc::new(wire::encode_frame(&ServerMsg::Mod(Box::new(p.clone())))))).collect();
        Ok(Self { world, options, listener, events, events_tx, conns: Vec::new(), next_conn: 1, open: Arc::new(AtomicUsize::new(0)), owners: BTreeMap::new(), agents: BTreeSet::new(), bans: Vec::new(), offer, next_spawn, stats: HostStats::default(), log: Vec::new() })
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    pub fn rules(&self) -> Rules {
        let (o, w) = (&self.options, &self.world);
        Rules {
            name: o.name.clone(),
            pvp: self.world.config.combat.pvp,
            max_players: o.max_players as u32,
            allow_dev: o.allow_dev,
            logout_secs: o.logout_secs,
            logout_safe_secs: o.logout_safe_secs,
            spawn_protection_secs: o.spawn_protection_secs,
            fair_sight: self.fair_sight(),
            camera: o.camera,
            agents: o.agents,
            spread_spawns: self.spawn_mode() == Spawn::Spread,
            // By name, as a player reads it (an id the data no longer has: the id).
            start_kit: o.kit.iter().map(|(id, n)| (w.config.items.iter().find(|i| i.id == *id).map_or_else(|| id.clone(), |i| i.name.clone()), *n)).collect(),
            map_m: [w.map.width() as u32, w.map.height() as u32],
            zombies: (0..w.zombies.len()).filter(|&i| w.zombies.state[i] != zombye_sim::ZombieState::Dead).count() as u32,
        }
    }

    /// What this server says of itself to someone who asks without joining (#69).
    pub fn info(&self) -> ServerInfo {
        ServerInfo { protocol: wire::PROTOCOL, version: crate::BUILD.to_string(), rules: self.rules(), online: self.conns.len() as u32, password: !self.options.password.is_empty(), mods: self.options.mods.clone() }
    }

    /// Is `player` played by a program, by its client's own word (#69)?
    pub fn is_agent(&self, player: u32) -> bool {
        self.agents.contains(&player)
    }

    /// Are clients told of other players only through their survivor's senses (#66)?
    pub fn fair_sight(&self) -> bool {
        self.options.fair_sight.unwrap_or(self.world.config.combat.pvp)
    }

    /// The clients connected now.
    pub fn clients(&self) -> Vec<ClientInfo> {
        self.conns
            .iter()
            .map(|c| ClientInfo { player: c.player, name: c.name.clone(), addr: c.addr.clone(), bytes_out: c.bytes_out, bytes_in: c.bytes_in, zombies: c.encoder.stats.known, held: c.held, map_owed: c.map.owed(), secs: c.joined.elapsed().as_secs_f32() })
            .collect()
    }

    /// Drop the client playing `player` (#26), telling it why ("kicked: ..."); its survivor stays
    /// in the world like any that left. With `ban`, its name and address (unless that is this
    /// machine's own) are refused from now on. The name kicked, or `None` if nobody is playing
    /// that player now.
    pub fn kick(&mut self, player: u32, why: &str, ban: bool) -> Option<String> {
        let why = why.trim();
        let c = self.conns.iter_mut().find(|c| c.player == player && c.closing.is_none())?;
        let word = if ban { "banned" } else { "kicked" };
        c.closing = Some(if why.is_empty() { format!("{word} by the server's operator") } else { format!("{word}: {why}") });
        let name = c.name.clone();
        if ban {
            let ip = c.addr.parse::<SocketAddr>().ok().map(|a| a.ip()).filter(|ip| !ip.is_loopback());
            let key = Some(c.key);
            self.ban(Ban { name: name.clone(), ip, key, why: why.to_string() });
        }
        Some(name)
    }

    /// Shut a name (and an address) out. One ban per name: a second replaces the first.
    pub fn ban(&mut self, ban: Ban) {
        self.bans.retain(|b| !b.name.eq_ignore_ascii_case(&ban.name) && (ban.key.is_none() || b.key != ban.key));
        self.bans.push(ban);
    }

    /// Lift the ban on a name, or the bans on an address. How many were lifted.
    pub fn unban(&mut self, who: &str) -> usize {
        let before = self.bans.len();
        let ip = who.parse::<IpAddr>().ok();
        self.bans.retain(|b| !(b.name.eq_ignore_ascii_case(who) || (ip.is_some() && b.ip == ip)));
        before - self.bans.len()
    }

    pub fn bans(&self) -> &[Ban] {
        &self.bans
    }

    /// Every survivor someone has played here, with its key and name (to keep beside a save, so
    /// survivors are their players' again after a restart).
    pub fn owners(&self) -> &BTreeMap<u32, Owner> {
        &self.owners
    }

    /// Owners from an earlier session of this world; those whose player the world no longer has
    /// are left out.
    pub fn set_owners(&mut self, owners: impl IntoIterator<Item = (u32, Owner)>) {
        self.owners = owners.into_iter().filter(|(id, _)| self.world.survivor(*id).is_some()).collect();
        for (&id, o) in &self.owners {
            self.world.set_player_name(id, &o.name);
        }
    }

    /// Names with no key yet (a `PATH.players` file from before keys, bots set up by a bench):
    /// the first key to join under each name takes its survivor.
    pub fn set_names(&mut self, names: impl IntoIterator<Item = (String, u32)>) {
        self.set_owners(names.into_iter().map(|(name, id)| (id, Owner { name, key: None })));
    }

    /// The player a name plays (or last played), if one does; with several, the newest.
    pub fn player_named(&self, name: &str) -> Option<u32> {
        self.owners.iter().rev().find(|(_, o)| o.name.eq_ignore_ascii_case(name.trim())).map(|(&id, _)| id)
    }

    /// The name playing (or that last played) `player`.
    pub fn name_of(&self, player: u32) -> Option<&str> {
        self.owners.get(&player).map(|o| o.name.as_str())
    }

    /// Survivors standing in the world with nobody playing them (a save from a server that was
    /// stopped with players on, or from before #68) are logged out: they lie down, and leave the
    /// world after the logout time, as if their players had just left. Called when a server starts.
    pub fn log_out_unplayed(&mut self) -> usize {
        let ids: Vec<u32> = self.world.players.iter().filter(|p| !p.dead && p.away.is_none() && !self.conns.iter().any(|c| c.player == p.id)).map(|p| p.id).collect();
        let (secs, safe) = (self.options.logout_secs, self.options.logout_safe_secs);
        ids.iter().filter(|&&id| self.world.player_logout(id, secs, safe).is_some()).count()
    }

    /// Intents received so far from the client playing `player` (tests: has what it sent arrived?).
    pub fn received(&self, player: u32) -> u64 {
        self.conns.iter().find(|c| c.player == player).map_or(0, |c| c.intents)
    }

    /// Intents waiting from the client playing `player` (tests).
    pub fn queued(&self, player: u32) -> usize {
        self.conns.iter().find(|c| c.player == player).map_or(0, |c| c.queue.len())
    }

    /// Take new connections and what clients sent, without stepping the world (a tick does this
    /// first; a paused server, or a test, can do it alone).
    pub fn pump(&mut self) {
        self.accept();
        self.take_events();
    }

    /// One server tick. See the module docs.
    pub fn tick(&mut self) {
        let start = Instant::now();
        self.pump();
        for k in 0..self.conns.len() {
            self.apply_queue(k);
        }
        self.world.step();
        for k in 0..self.world.left_world.len() {
            let id = self.world.left_world[k];
            let who = self.name_of(id).map_or_else(|| format!("player {id}"), str::to_string);
            self.log.push(format!("{who}'s survivor left the world (kept for when they are back)"));
        }
        self.stats.sim_ms = self.world.stats.tick_ms;
        self.send_snapshots();
        self.drop_closed();
        self.stats.ticks += 1;
        self.stats.tick_ms = start.elapsed().as_secs_f32() * 1000.0;
        self.stats.worst_tick_ms = self.stats.worst_tick_ms.max(self.stats.tick_ms);
    }

    /// Tick at the sim's rate until `each` (called after every tick) says stop. A tick that
    /// overruns isn't made up for: the next starts a full interval after it.
    pub fn run(&mut self, mut each: impl FnMut(&mut Host) -> bool) {
        let step = Duration::from_secs_f32(TICK_DT);
        let mut next = Instant::now() + step;
        loop {
            self.tick();
            if !each(self) {
                break;
            }
            let now = Instant::now();
            if next > now {
                std::thread::sleep(next - now);
                next += step;
            } else {
                next = now + step;
            }
        }
    }

    /// Tell every client the server is closing, and why, and drop them.
    pub fn close(&mut self, why: &str) {
        for c in &mut self.conns {
            c.closing = Some(why.to_string());
        }
        self.drop_closed();
    }

    fn accept(&mut self) {
        loop {
            let (stream, _) = match self.listener.accept() {
                Ok(s) => s,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(_) => break,
            };
            if self.open.load(Ordering::Relaxed) >= self.options.max_players + MAX_PENDING {
                continue;
            }
            let id = self.next_conn;
            self.next_conn += 1;
            let (tx, open) = (self.events_tx.clone(), self.open.clone());
            open.fetch_add(1, Ordering::Relaxed);
            let spawned = std::thread::Builder::new().name(format!("net-read-{id}")).spawn(move || {
                read_loop(id, stream, &tx);
                open.fetch_sub(1, Ordering::Relaxed);
                let _ = tx.send(Event::Closed(id));
            });
            if spawned.is_err() {
                self.open.fetch_sub(1, Ordering::Relaxed);
            }
        }
    }

    fn take_events(&mut self) {
        while let Ok(event) = self.events.try_recv() {
            match event {
                Event::Hello(id, hello, stream) => self.hello(id, hello, stream),
                Event::Query(mut stream) => {
                    // Whatever protocol asks: the answer says ours, and the asker sees whether
                    // it could join. Written here (a few hundred bytes into an empty socket
                    // buffer; a short timeout in case) and the connection is done.
                    self.stats.queries += 1;
                    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
                    let _ = wire::write_frame(&mut stream, &ServerMsg::Info(Box::new(self.info())));
                    let _ = stream.shutdown(Shutdown::Both);
                }
                Event::Fetch(stream, fetch) => self.fetch(stream, fetch),
                Event::Msg(id, msg, bytes) => {
                    let Some(c) = self.conns.iter_mut().find(|c| c.id == id) else { continue };
                    c.bytes_in += bytes as u64;
                    self.stats.bytes_in += bytes as u64;
                    match msg {
                        ClientMsg::Intent(intent) => {
                            c.queue.push_back(intent);
                            c.intents += 1;
                            if c.queue.len() > MAX_QUEUE {
                                c.closing = Some("sending too much".into());
                            }
                        }
                        ClientMsg::Ping(n) => send(c, &mut self.stats, &ServerMsg::Pong(n)),
                        ClientMsg::Bye => c.closing = Some(String::new()),
                        ClientMsg::Hello(_) | ClientMsg::Proof(_) => c.closing = Some("hello twice".into()),
                        ClientMsg::Query(_) | ClientMsg::Fetch(_) => send(c, &mut self.stats, &ServerMsg::Bye("a query comes instead of a hello".into())),
                    }
                }
                Event::Closed(id) => {
                    if let Some(c) = self.conns.iter_mut().find(|c| c.id == id) {
                        c.closing.get_or_insert(String::new());
                    }
                }
            }
        }
    }

    /// A new connection said hello: let it in as a player, or say why not.
    fn hello(&mut self, id: u64, hello: Hello, mut stream: TcpStream) {
        let refuse = |stream: &mut TcpStream, why: String| {
            let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
            let _ = wire::write_frame(stream, &ServerMsg::Refused(why));
            let _ = stream.shutdown(Shutdown::Both);
        };
        let ip = stream.peer_addr().ok().map(|a| a.ip());
        let key = hello.key;
        let banned = |name: &str| self.bans.iter().find(|b| b.name.eq_ignore_ascii_case(name.trim()) || (b.ip.is_some() && b.ip == ip) || b.key == Some(key)).map(|b| if b.why.is_empty() { "you are banned from this server".to_string() } else { format!("you are banned from this server: {}", b.why) });
        let name = match banned(&hello.name).map_or_else(|| self.check(&hello), Err) {
            Ok(name) => name,
            Err(why) => {
                self.stats.refused += 1;
                self.log.push(format!("refused {}: {why}", hello.name));
                return refuse(&mut stream, why);
            }
        };
        let addr = stream.peer_addr().map_or_else(|_| "?".into(), |a| a.to_string());
        // Its survivor from before, or a new one.
        // Back (#68): one lying logged out gets up, one that had left the world is put back where
        // it lay, a dead one is still theirs (they see how it ended, and start a new one from there).
        // Whose survivor: the one this key played (under whatever name), else one kept from before
        // keys under this name, which the key now takes (DCM-66). Someone else's name gets a new
        // survivor, never theirs.
        let mine = self.owners.iter().rev().find(|(_, o)| o.key == Some(key)).map(|(&p, _)| p);
        let unclaimed = || self.owners.iter().rev().find(|(_, o)| o.key.is_none() && o.name == name).map(|(&p, _)| p);
        let back = mine.or_else(unclaimed).filter(|&p| self.world.player_return(p));
        let player = back.unwrap_or_else(|| {
            let at = self.new_spawn();
            let id = self.world.add_player(at);
            self.outfit_new(id);
            id
        });
        let renamed = self.owners.get(&player).filter(|o| o.name != name).map(|o| o.name.clone());
        self.owners.retain(|_, o| o.key != Some(key));
        self.owners.insert(player, Owner { name: name.clone(), key: Some(key) });
        // What its group is told it is called (#73).
        self.world.set_player_name(player, &name);
        // Who plays it now: a program or a person, by this hello (#69).
        if hello.agent {
            self.agents.insert(player);
        } else {
            self.agents.remove(&player);
        }
        let c = &self.world.config;
        let welcome = Welcome {
            protocol: wire::PROTOCOL,
            player,
            name: name.clone(),
            rules: self.rules(),
            generation: c.generation.clone(),
            content: ContentPrint::of(c),
            map_hash: self.world.map_base_hash(),
            map_edits: self.world.map_edits_now(),
            edge_damage: EdgeDamage::all(&self.world),
            blinds: self.world.blinds_now(),
            glazed: self.world.glazed_doors(),
            tunings: Tunings { combat: c.combat.clone(), health: c.health.clone(), infection: c.infection.clone(), needs: c.needs, skills: c.skills.clone(), carry: c.carry.clone(), climb: c.climb.clone() },
        };
        let (out, frames) = mpsc::sync_channel::<Vec<u8>>(OUT_QUEUE);
        let _ = stream.set_write_timeout(Some(WRITE_TIMEOUT));
        let spawned = std::thread::Builder::new().name(format!("net-write-{id}")).spawn(move || {
            while let Ok(frame) = frames.recv() {
                if stream.write_all(&frame).is_err() {
                    break;
                }
            }
            let _ = stream.shutdown(Shutdown::Both);
        });
        if spawned.is_err() {
            return;
        }
        let tokens = self.options.budget as f32;
        let mut conn = Conn { id, player, name: name.clone(), key, addr: addr.clone(), out, queue: VecDeque::new(), encoder: DeltaEncoder::default(), map: MapInterest::default(), tokens, said_at: None, held: 0, bytes_out: 0, bytes_in: 0, intents: 0, closing: None, joined: Instant::now() };
        send(&mut conn, &mut self.stats, &ServerMsg::Welcome(Box::new(welcome)));
        self.conns.push(conn);
        // Its private view goes with its first snapshot.
        self.world.mark_all_private_dirty();
        self.stats.joined += 1;
        let was = renamed.map_or_else(String::new, |old| format!(", was {old}"));
        self.log.push(format!("{name}{} joined from {addr} as player {player}{} (key {}{was}; {} online)", if hello.agent { " (an agent)" } else { "" }, if back.is_some() { " (back)" } else { "" }, identity::short(&key), self.conns.len()));
    }

    /// Someone about to join asks for mods of ours it hasn't got (#24). Who may join may have
    /// them: the same protocol, the password, not banned by address. The packs (encoded when the
    /// host started) are written by a thread of their own, counted with the open connections, so
    /// a slow line never holds the tick up; a refusal is a few bytes written here.
    fn fetch(&mut self, mut stream: TcpStream, fetch: wire::Fetch) {
        let ip = stream.peer_addr().ok().map(|a| a.ip()).filter(|ip| !ip.is_loopback());
        let frames: Result<Vec<Arc<Vec<u8>>>, String> = if fetch.protocol != wire::PROTOCOL {
            Err(format!("this server speaks protocol {}, your game {}: both need the same version of the game", wire::PROTOCOL, fetch.protocol))
        } else if !self.options.password.is_empty() && fetch.password != self.options.password {
            Err("wrong password".into())
        } else if ip.is_some() && self.bans.iter().any(|b| b.ip == ip) {
            Err("you are banned from this server".into())
        } else if fetch.mods.len() > crate::modpack::MAX_MODS {
            Err("too many mods asked for at once".into())
        } else {
            fetch
                .mods
                .iter()
                .map(|hash| match self.offer.iter().find(|(h, ..)| h == hash) {
                    Some((_, _, frame)) => Ok(frame.clone()),
                    None => Err(match self.options.mods.iter().find(|m| m.hash == *hash) {
                        Some(m) => format!("this server doesn't hand out its mod {} (too big): install it yourself and start the game with --mod", m.id),
                        None => format!("this server has no mod with the hash {} (it may have restarted with other mods: try again)", crate::modpack::short(hash)),
                    }),
                })
                .collect()
        };
        match frames {
            Err(why) => {
                self.stats.refused += 1;
                let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
                let _ = wire::write_frame(&mut stream, &ServerMsg::Refused(why));
                let _ = stream.shutdown(Shutdown::Both);
            }
            Ok(frames) => {
                let bytes: usize = frames.iter().map(|f| f.len()).sum();
                self.stats.mods_sent += frames.len() as u64;
                self.stats.mod_bytes += bytes as u64;
                self.stats.bytes_out += bytes as u64;
                let from = stream.peer_addr().map_or_else(|_| "someone".to_string(), |a| a.to_string());
                self.log.push(format!("sent {} mod{} ({}) to {from}", frames.len(), if frames.len() == 1 { "" } else { "s" }, crate::modpack::kb(bytes)));
                let open = self.open.clone();
                open.fetch_add(1, Ordering::Relaxed);
                let counted = open.clone();
                let spawned = std::thread::Builder::new().name("net-mods".into()).spawn(move || {
                    let _ = stream.set_write_timeout(Some(WRITE_TIMEOUT));
                    for frame in frames {
                        if stream.write_all(&frame).is_err() {
                            break;
                        }
                    }
                    let _ = stream.shutdown(Shutdown::Both);
                    counted.fetch_sub(1, Ordering::Relaxed);
                });
                if spawned.is_err() {
                    open.fetch_sub(1, Ordering::Relaxed);
                }
            }
        }
    }

    /// May this hello come in? Its name (made up if it gave none), or why not.
    fn check(&self, hello: &Hello) -> Result<String, String> {
        if hello.protocol != wire::PROTOCOL {
            return Err(format!("this server speaks protocol {}, your game {}: both need the same version of the game", wire::PROTOCOL, hello.protocol));
        }
        if !self.options.password.is_empty() && hello.password != self.options.password {
            return Err("wrong password".into());
        }
        if hello.agent && self.options.agents == Agents::Refused {
            return Err("this server's rules don't allow agents: its survivors are played by people".into());
        }
        if self.conns.len() >= self.options.max_players {
            return Err(format!("the server is full ({} players)", self.options.max_players));
        }
        let name = hello.name.trim();
        if name.chars().count() > 24 || name.chars().any(|c| c.is_control()) {
            return Err("names are at most 24 letters".into());
        }
        if name.is_empty() {
            return Ok((1..).map(|n| format!("survivor-{n}")).find(|n| !self.owners.values().any(|o| o.name == *n)).expect("a free name"));
        }
        if self.conns.iter().any(|c| c.key == hello.key) {
            return Err("this game is already playing here (the same player key)".into());
        }
        if self.conns.iter().any(|c| c.name == name) {
            return Err(format!("{name} is already playing here"));
        }
        Ok(name.to_string())
    }

    /// A new survivor's start kit and spawn protection (#68).
    fn outfit_new(&mut self, id: u32) {
        self.world.give_kit(id, &self.options.kit);
        if self.world.config.combat.pvp {
            self.world.protect_player(id, self.options.spawn_protection_secs);
        }
    }

    /// Where the next new survivor starts (#68). Spread: the first of the places round the edge of
    /// the town, counting on from the last one used, with no living player within 40 m and no
    /// living zombie within 15 m; if nowhere is that clear (more players than the edge of the town
    /// has room for at 40 m), the place furthest from every player.
    /// Together: the next place by the first house.
    pub fn new_spawn(&mut self) -> Vec2 {
        let first = self.next_spawn;
        self.next_spawn = self.next_spawn.wrapping_add(1);
        if self.spawn_mode() == Spawn::Together {
            return self.spawn_point(first);
        }
        let w = &self.world;
        let z = &w.zombies;
        let from_players = |at: Vec2| w.players.iter().filter(|p| !p.dead).map(|p| p.state.pos.distance(at)).fold(f32::MAX, f32::min);
        let zombie_near = |at: Vec2| (0..z.len()).any(|i| z.state[i] != zombye_sim::ZombieState::Dead && z.pos[i].distance(at) < SPAWN_CLEAR_OF_ZOMBIES);
        let mut best = (f32::MIN, first);
        for k in (0..SPAWN_PLACES).map(|j| first.wrapping_add(j)) {
            let at = self.spawn_point(k);
            let d = from_players(at);
            let clear = !zombie_near(at);
            if d >= SPAWN_CLEAR_OF_PLAYERS && clear {
                best.1 = k;
                break;
            }
            // Nowhere clear yet (a full server): the furthest from players, a place without
            // zombies counting as 20 m further (never on top of somebody for the sake of it).
            let score = d.min(1e6) + if clear && d >= SPAWN_CLEAR_OF_ZOMBIES { 20.0 } else { 0.0 };
            if score > best.0 {
                best = (score, k);
            }
        }
        self.next_spawn = best.1.wrapping_add(1);
        self.spawn_point(best.1)
    }

    fn spawn_mode(&self) -> Spawn {
        self.options.spawn.unwrap_or(if self.world.config.combat.pvp { Spawn::Spread } else { Spawn::Together })
    }

    /// The `k`th place a survivor of this world may start: spread, round the edge of the town
    /// (each a golden angle on from the last, on the nearest open ground outdoors); together,
    /// outside the house a single player wakes up in.
    pub fn spawn_point(&self, k: u32) -> Vec2 {
        let w = &self.world;
        let spawn = self.spawn_mode();
        // The golden angle: each next one well away from those before.
        let angle = k as f32 * 2.399_963;
        let aim = match spawn {
            Spawn::Together => open_ground_near(w, w.player_spawn, 6) + Vec2::from_angle(angle) * (1.5 + 0.9 * (k as f32).sqrt()),
            Spawn::Spread => {
                let (min, max) = (w.town_min.as_vec2(), w.town_max.as_vec2());
                (min + max) * 0.5 + Vec2::from_angle(angle) * ((max - min) * 0.5 + Vec2::splat(6.0))
            }
        };
        let size = Vec2::new(w.map.width() as f32, w.map.height() as f32);
        open_ground_near(w, aim.clamp(Vec2::splat(2.0), size - Vec2::splat(2.0)), 0)
    }

    /// This tick's share of a client's queue: everything up to and including one movement input.
    fn apply_queue(&mut self, k: usize) {
        let player = self.conns[k].player;
        let is_input = |i: &Intent| matches!(i, Intent::PlayerInput { .. });
        // Too far behind: skip to the newest few inputs (what lies between still happens).
        let inputs = self.conns[k].queue.iter().filter(|i| is_input(i)).count();
        let mut skip = if inputs > MAX_INPUTS { inputs - KEEP_INPUTS } else { 0 };
        let mut moved = false;
        while let Some(next) = self.conns[k].queue.front() {
            if is_input(next) && moved && skip == 0 {
                break;
            }
            let mut intent = self.conns[k].queue.pop_front().expect("peeked");
            if is_input(&intent) {
                if skip > 0 {
                    skip -= 1;
                } else {
                    moved = true;
                }
            }
            if intent.dev_only() && !self.options.allow_dev {
                self.stats.refused_dev += 1;
                continue;
            }
            if let Some(p) = intent.player_mut() {
                *p = player;
            }
            if matches!(intent, Intent::Say { .. }) {
                let now = self.world.tick;
                if self.conns[k].said_at.is_some_and(|at| now < at + SAY_EVERY) {
                    continue;
                }
                self.conns[k].said_at = Some(now);
            }
            let respawn = matches!(intent, Intent::Respawn { .. }) && self.world.player(player).is_some_and(|p| p.dead);
            apply_intent(&mut self.world, &intent);
            if respawn {
                // A new survivor starts where new ones do, not in the single-player bedroom.
                // (The dead one, now alive at the bedroom, mustn't count as a player to keep clear of.)
                if let Some(p) = self.world.players.iter_mut().find(|p| p.id == player && !p.dead) {
                    p.state.pos = Vec2::splat(-1e6);
                    let at = self.new_spawn();
                    let at = self.world.map.resolve_circle(at, zombye_sim::players::PLAYER_RADIUS);
                    if let Some(p) = self.world.players.iter_mut().find(|p| p.id == player) {
                        p.state.pos = at;
                    }
                    self.outfit_new(player);
                }
            }
        }
    }

    fn send_snapshots(&mut self) {
        if self.conns.is_empty() {
            // Nobody to tell: let the world's queues for clients go (they'd only grow).
            self.world.edge_changes.clear();
            self.world.combat_events.clear();
            self.world.item_notes.clear();
            self.world.speech.clear();
            return;
        }
        let mut full = Snapshot::capture(&mut self.world);
        // Marked agents (#69): whoever sees one sees that it is one (its own client too).
        if self.options.agents == Agents::Marked && !self.agents.is_empty() {
            for p in &mut full.players {
                p.agent = self.agents.contains(&p.id);
            }
        }
        let radius = self.world.config.tiers.interest_radius;
        let budget = self.options.budget.clamp(BUDGET_RANGE.0, BUDGET_RANGE.1) as f32;
        let a_tick = budget * TICK_DT;
        let fair = self.fair_sight();
        for c in &mut self.conns {
            // Its area of interest: what is within the radius of its survivor, and of the map
            // the chunks that are (with what it missed in those that have just come in).
            let mut view = full.for_client(c.player, radius);
            // Of the other players, only those its survivor sees; those it hears, as sounds (#66).
            if fair {
                view = view.through_senses(c.player, &self.world);
            } else {
                view.heard.clear();
            }
            let me = self.world.player(c.player).map(|p| p.state.pos);
            c.map.cut(&mut view, me, radius, &self.world);
            // Its budget: zombies get what is left of it this tick, and never less than a
            // quarter of a tick's share, so the ones close by are always told.
            c.tokens = (c.tokens + a_tick).min(budget);
            let zombie_bytes = (c.tokens - FRAME_BYTES).max(a_tick * 0.25) as usize;
            let delta = c.encoder.encode(view, c.player, Limits { admit: radius - ADMIT_INSIDE, zombie_bytes });
            let held = c.encoder.stats.held as u64;
            c.held += held;
            self.stats.held += held;
            let before = c.bytes_out;
            send(c, &mut self.stats, &ServerMsg::Snapshot(Box::new(delta)));
            c.tokens = (c.tokens - (c.bytes_out - before) as f32).max(-budget);
        }
    }

    fn drop_closed(&mut self) {
        let mut k = 0;
        while k < self.conns.len() {
            if self.conns[k].closing.is_none() {
                k += 1;
                continue;
            }
            let mut c = self.conns.swap_remove(k);
            let why = c.closing.take().unwrap_or_default();
            if !why.is_empty() {
                let _ = c.out.try_send(wire::encode_frame(&ServerMsg::Bye(why.clone())));
            }
            // Its survivor stays for the logout time, lying asleep where it was left (#68).
            let stays = match self.world.player_logout(c.player, self.options.logout_secs, self.options.logout_safe_secs) {
                Some(secs) => format!("; its survivor stays in the world for {secs:.0} s"),
                None => String::new(),
            };
            self.stats.left += 1;
            self.log.push(format!("{} left{}{stays} ({} online)", c.name, if why.is_empty() { String::new() } else { format!(": {why}") }, self.conns.len()));
        }
    }
}

/// Queue a frame for a client; one whose queue is full (or whose writer is gone) is dropped.
fn send(c: &mut Conn, stats: &mut HostStats, msg: &ServerMsg) {
    let frame = wire::encode_frame(msg);
    let n = frame.len() as u64;
    match c.out.try_send(frame) {
        Ok(()) => {
            c.bytes_out += n;
            stats.bytes_out += n;
        }
        Err(TrySendError::Full(_)) => c.closing = Some("connection too slow".into()),
        Err(TrySendError::Disconnected(_)) => {
            c.closing.get_or_insert(String::new());
        }
    }
}

/// A connection's reading thread: the hello first (promptly), then messages until it ends.
fn read_loop(id: u64, mut stream: TcpStream, tx: &Sender<Event>) {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_nodelay(true);
    let _ = stream.set_read_timeout(Some(HELLO_TIMEOUT));
    let hello = match wire::read_frame::<ClientMsg>(&mut stream, wire::MAX_CLIENT_FRAME) {
        Ok((ClientMsg::Hello(hello), _)) => hello,
        // Not joining, only asking (#69): the host answers and closes it.
        Ok((ClientMsg::Query(_), _)) => {
            let _ = tx.send(Event::Query(stream));
            return;
        }
        // Not joining yet, fetching our mods first (#24): the host answers and closes it.
        Ok((ClientMsg::Fetch(fetch), _)) => {
            let _ = tx.send(Event::Fetch(stream, fetch));
            return;
        }
        _ => {
            let _ = stream.shutdown(Shutdown::Both);
            return;
        }
    };
    // Who it is (DCM-66): it signs a fresh challenge with the key it named, or isn't let in.
    let nonce = identity::nonce();
    let proved = wire::write_frame(&mut stream, &ServerMsg::Challenge(nonce)).is_ok()
        && matches!(wire::read_frame::<ClientMsg>(&mut stream, wire::MAX_CLIENT_FRAME), Ok((ClientMsg::Proof(sig), _)) if identity::verify(&hello.key, &nonce, &hello.name, &sig));
    if !proved {
        let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
        let _ = wire::write_frame(&mut stream, &ServerMsg::Refused("your game couldn't prove who you are (its player key): both need the same version of the game".into()));
        let _ = stream.shutdown(Shutdown::Both);
        return;
    }
    let Ok(writer) = stream.try_clone() else { return };
    let _ = stream.set_read_timeout(Some(IDLE));
    if tx.send(Event::Hello(id, hello, writer)).is_err() {
        return;
    }
    while let Ok((msg, bytes)) = wire::read_frame::<ClientMsg>(&mut stream, wire::MAX_CLIENT_FRAME) {
        if tx.send(Event::Msg(id, msg, bytes)).is_err() {
            break;
        }
    }
    let _ = stream.shutdown(Shutdown::Both);
}

/// The centre of the nearest outdoor tile to `p` that can be stood on with `clear` free tiles
/// round it (searching outward), else `p`.
fn open_ground_near(world: &World, p: Vec2, clear: i32) -> Vec2 {
    let map = &world.map;
    let open = |t: IVec2| map.in_bounds(t) && map.tile(t).room == 0 && !map.tile_blocked(t);
    let c = p.floor().as_ivec2();
    for r in 0..80i32 {
        for dy in -r..=r {
            for dx in -r..=r {
                let t = c + IVec2::new(dx, dy);
                if dx.abs().max(dy.abs()) == r && (-clear..=clear).all(|y| (-clear..=clear).all(|x| open(t + IVec2::new(x, y)))) {
                    return t.as_vec2() + Vec2::splat(0.5);
                }
            }
        }
    }
    p
}
