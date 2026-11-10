//! MCP admin tools for whoever runs a multiplayer server (#26): `zombye-server --host ...
//! --admin-mcp [ADDR[:PORT]]`. An agent lists and kicks players, saves, changes the server's
//! settings, sets off events (zombies, a horde, a noise, items, the clock) and reads the log, on
//! the world people are playing in.
//!
//! **Its own endpoint, always behind a token.** The admin tools are not among the game's MCP tools
//! (`mcp.rs`: those play a world of their own with time paused between calls). They are served by
//! the hosting server itself, over the same streamable HTTP transport as `--mcp-http`
//! (`mcp_http::listen`: `POST /mcp`, by default on `127.0.0.1:8788`), and only to callers with the
//! bearer token (`ZOMBYE_ADMIN_TOKEN`, else `--admin-token T`; at least 16 characters). There is no
//! way to start it open. No TLS here: a reverse proxy or a tunnel in front, as for `--mcp-http`.
//!
//! **Between ticks.** Connections only pass JSON-RPC messages to the server's main thread, which
//! answers a few after each tick ([`Admin::tick`]), so a call sees and changes the world between
//! two ticks like a client's intents do, and the tick never waits on a socket.
//!
//! **Everything is logged.** The server's lines (joins, leaves, refusals, deaths, saves, the
//! report every ten seconds) and every admin call that changes something are kept, numbered, in a
//! ring ([`Log`]) that the `logs` tool reads, as well as printed.

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use glam::Vec2;
use serde_json::{Value, json};
use zombye_net::host::{BUDGET_RANGE, Host, Spawn};
use zombye_net::{save_file, wire};
use zombye_sim::players::PLAYER_RADIUS;

use crate::Persist;
use crate::mcp::{self, Called, Service};
use crate::mcp_http::{self, HttpArgs, Job, MIN_TOKEN};

/// The port when `--admin-mcp` names none (`--mcp-http`'s is 8787).
pub const DEFAULT_PORT: u16 = 8788;
/// Log lines kept.
const LOG_LINES: usize = 2000;
/// Admin calls answered after one tick, and the time they may take together before the rest wait
/// for the next (a save of a big world is about 15 ms).
const JOBS_A_TICK: usize = 4;
const JOB_TIME: Duration = Duration::from_millis(10);
/// The most zombies one call may add.
const MAX_SPAWN: u64 = 500;

/// `--admin-mcp [ADDR[:PORT]]`, `--admin-token T` (or `ZOMBYE_ADMIN_TOKEN`, `env_token`),
/// `--admin-origin ORIGIN`: where the admin endpoint listens, or `None` without `--admin-mcp`.
pub fn parse(args: &[String], env_token: Option<String>) -> Result<Option<HttpArgs>, String> {
    if !args.iter().any(|a| a == "--admin-mcp") {
        return Ok(None);
    }
    let text = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).filter(|v| !v.starts_with("--")).cloned();
    let addr = match text("--admin-mcp") {
        None => format!("127.0.0.1:{DEFAULT_PORT}"),
        Some(v) if v.parse::<u16>().is_ok() => format!("127.0.0.1:{v}"),
        Some(v) if v.contains(':') => v,
        Some(v) => format!("{v}:{DEFAULT_PORT}"),
    };
    let token = text("--admin-token").or(env_token).map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
    let Some(token) = token else {
        return Err(format!(
            "--admin-mcp needs a token: set ZOMBYE_ADMIN_TOKEN (or pass --admin-token T, at least {MIN_TOKEN} characters), which the operator's agent sends as `Authorization: Bearer T`. \
             The admin tools are never served without one"
        ));
    };
    // The transport's own checks (length, printable), under its own flag names.
    let mut http = vec!["--mcp-http".to_string(), addr, "--mcp-token".to_string(), token];
    for (i, a) in args.iter().enumerate() {
        if a == "--admin-origin" {
            http.extend(["--mcp-origin".to_string(), args.get(i + 1).cloned().unwrap_or_default()]);
        }
    }
    HttpArgs::parse(&http, None).map(Some).map_err(|e| e.replace("the MCP token", "the admin token"))
}

struct Entry {
    seq: u64,
    /// Seconds since 1970.
    unix: u64,
    tick: u64,
    text: String,
}

/// The server's log: every line is printed and the last [`LOG_LINES`] are kept, numbered from 1.
#[derive(Default)]
pub struct Log {
    lines: VecDeque<Entry>,
    /// The number of the newest line (0: none yet).
    last: u64,
    /// Don't print (tests).
    pub quiet: bool,
}

impl Log {
    pub fn add(&mut self, tick: u64, text: impl Into<String>) {
        let text = text.into();
        if !self.quiet {
            println!("{text}");
        }
        self.last += 1;
        if self.lines.len() >= LOG_LINES {
            self.lines.pop_front();
        }
        let unix = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
        self.lines.push_back(Entry { seq: self.last, unix, tick, text });
    }
}

/// `2026-11-10 17:04:09Z` from seconds since 1970 (the civil-from-days rule; no time crate here).
pub(crate) fn stamp(unix: u64) -> String {
    let (days, secs) = ((unix / 86_400) as i64, unix % 86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let (d, m) = (doy - (153 * mp + 2) / 5 + 1, if mp < 10 { mp + 3 } else { mp - 9 });
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}Z", secs / 3600, secs / 60 % 60, secs % 60)
}

/// The admin side of a hosting server: its log, and the endpoint if one was asked for.
pub struct Admin {
    pub log: Log,
    jobs: Option<Receiver<Job>>,
    started: Instant,
    /// Players known to be dead, so each death is logged once.
    dead: Vec<u32>,
}

impl Default for Admin {
    fn default() -> Self {
        Self { log: Log::default(), jobs: None, started: Instant::now(), dead: Vec::new() }
    }
}

/// Each tool: name, title, whether it only reads, whether it can lose something (a connection, a
/// save of that name, hours of the world).
const TOOLS: [(&str, &str, bool, bool); 14] = [
    ("status", "Server status", true, false),
    ("players", "Players online, offline and banned", true, false),
    ("logs", "Read the server log", true, false),
    ("saves", "List saves", true, false),
    ("kick", "Kick (or ban) a player", false, true),
    ("unban", "Lift a ban", false, false),
    ("save", "Save the world now", false, true),
    ("settings", "Show or change the server's settings", false, false),
    ("spawn_zombies", "Event: spawn zombies", false, false),
    ("horde", "Event: send a horde at a player or a place", false, false),
    ("make_noise", "Event: a noise that draws zombies", false, false),
    ("spawn_item", "Event: drop items", false, false),
    ("skip_time", "Event: move the clock forward", false, true),
    ("teleport", "Move a player", false, false),
];
/// The game's own read-only tools, offered here on the hosted world (`mcp.rs`).
const LOOK: [&str; 7] = ["observe", "stats", "inventory", "health", "regions", "zombie", "items"];
/// The game's debug tools that are admin events here, under the same names.
const EVENTS: [&str; 4] = ["spawn_zombies", "make_noise", "spawn_item", "skip_time"];

fn instructions() -> String {
    "You are the operator of a running Zombye multiplayer server (a zombie survival game: a town on a tile map, 1 tile = 1 m, x east, y south). \
     People are playing: the world ticks 20 times a second whatever you do, and each call happens between two ticks.\n\
     - Look first: status (load, rules, the save), players (who is on, where, and who is banned), logs (joins, leaves, deaths, your own actions; pass `after` to read on from a line). \
     observe / stats / inventory / health / regions show the world itself, and change nothing.\n\
     - Players are named by `player` (the id) or `name`. kick drops a connection and says why (the survivor stays in the world; `ban: true` keeps the name and address out, unban lifts it).\n\
     - save writes the world now (the server also autosaves); settings shows the rules and changes them (pvp, max_players, password, ...).\n\
     - Events change the game for everyone: spawn_zombies, horde, make_noise, spawn_item, skip_time, teleport. Use them when asked to, sparingly: a horde on a player usually kills them.\n\
     - Everything you change is written to the server's log."
        .to_string()
}

fn tool_list() -> Value {
    let who = || json!({"player": {"type": "integer", "description": "Player id (see players)"}, "name": {"type": "string", "description": "Or the player's name"}});
    let place = |extra: Value| {
        let mut p = who();
        p["x"] = json!({"type": "number", "description": "Metres east: with y, a place instead of a player"});
        p["y"] = json!({"type": "number"});
        for (k, v) in extra.as_object().into_iter().flatten() {
            p[k] = v.clone();
        }
        p
    };
    let mut kick = who();
    kick["reason"] = json!({"type": "string", "description": "Shown to the player and logged"});
    kick["ban"] = json!({"type": "boolean", "description": "Also keep them out (default false)"});
    let mut defs = vec![
        json!({"name": "status", "description": "The server now, as JSON: name and version, uptime, tick and game clock, tick time against the 50 ms budget, players online and survivors in the world, zombies by simulation tier, bytes sent, joins / leaves / refusals, the rules (pvp, fair_sight, allow_dev, whether a password is set), the save file and when it was last written, bans, and the number of the newest log line.",
            "inputSchema": {"type": "object", "properties": {}}}),
        json!({"name": "players", "description": "Who is here, as JSON: `online` (player id, name, address, seconds connected, position and room, HP, dead, kills this life, KB/s down and up, zombies in its view, how far its view lags), `offline` (survivors in the world nobody is playing now: they stand where they were left) and `bans`.",
            "inputSchema": {"type": "object", "properties": {}}}),
        json!({"name": "logs", "description": "The server's log, oldest first: joins, leaves (with the reason: left, kicked, too slow, timed out), refusals (wrong password, full, banned), deaths with their cause, saves, a load report every 10 s while anyone is on, and every admin call that changed something. Each line: its number, UTC time, tick, text. The last 2,000 are kept.",
            "inputSchema": {"type": "object", "properties": {
                "limit": {"type": "integer", "description": "Lines at most (default 50, up to 500): the newest, or with `after` the first ones after it"},
                "after": {"type": "integer", "description": "Only lines numbered above this (pass the last number you read, to follow the log)"},
                "contains": {"type": "string", "description": "Only lines containing this text (case ignored), e.g. \"died\", \"joined\", \"admin:\""}}}}),
        json!({"name": "saves", "description": "The saves folder: each save's size, age, tick, clock, seed, map, zombies and players; and where this server autosaves.",
            "inputSchema": {"type": "object", "properties": {}}}),
        json!({"name": "kick", "description": "Drop a player's connection, telling them why. Their survivor is left as when anyone leaves (standing where it was, then logged out by the server's logout rules), and is theirs again if they come back. With `ban`, their name and address (unless they play from the server's own machine) are refused until `unban`; bans last until the server restarts.",
            "inputSchema": {"type": "object", "properties": kick}}),
        json!({"name": "unban", "description": "Lift the ban on a name, or every ban on an address.",
            "inputSchema": {"type": "object", "properties": {"name": {"type": "string", "description": "The banned name, or an IP address"}}, "required": ["name"]}}),
        json!({"name": "save", "description": "Save the world now: with no name to the server's --save file (as the autosave does), with a name to that file in the saves folder (replacing a save of that name). Whose survivor is whose is written beside it. A few milliseconds between two ticks; players notice nothing.",
            "inputSchema": {"type": "object", "properties": {"name": {"type": "string", "description": "A plain file name, e.g. before-the-event"}}}}),
        json!({"name": "settings", "description": "The server's settings as JSON; with arguments, change those first. They apply at once. pvp belongs to the world and is saved with it; the rest are the server's and go back to its command line (zombie_respawn: to the game data's population.ron) when it restarts. Players already on see the new name, pvp label and limit when they next join.",
            "inputSchema": {"type": "object", "properties": {
                "name": {"type": "string", "description": "The server's name, told to players as they join"},
                "max_players": {"type": "integer", "description": "1 to 64. Lowering it kicks nobody"},
                "password": {"type": "string", "description": "Needed to join; \"\" removes it. Never shown back"},
                "pvp": {"type": "boolean", "description": "Players can hurt each other (off: co-op)"},
                "fair_sight": {"description": "Tell a client of other players only while its survivor sees or hears them: true, false, or \"auto\" (on with pvp)"},
                "allow_dev": {"type": "boolean", "description": "Take god mode and debug intents from players' clients"},
                "spawn": {"type": "string", "enum": ["spread", "together", "auto"], "description": "Where new survivors start (auto: spread with pvp, together without)"},
                "budget_kb": {"type": "integer", "description": "KB a second down to each client, 8 to 1000"},
                "autosave_mins": {"type": "number", "description": "Minutes between autosaves; 0 turns them off"},
                "zombie_respawn": {"type": "boolean", "description": "Regions far from every player refill with zombies over time"}}}}),
        json!({"name": "spawn_zombies", "description": "Event: put zombies down round a place or a player, scattered within `radius` on ground they can stand on. They behave as any zombie does (they notice players by sight and sound).",
            "inputSchema": {"type": "object", "properties": place(json!({"count": {"type": "integer", "description": "1 to 500"}, "radius": {"type": "number", "description": "Metres (default 10)"}})), "required": ["count"]}}),
        json!({"name": "horde", "description": "Event: a horde comes for a player or a place. `count` zombies appear in a clump `distance` metres away (out of sight by default) and a loud, lasting noise at the target brings them, and any zombies in earshot, to it over the next minute or so.",
            "inputSchema": {"type": "object", "properties": place(json!({
                "count": {"type": "integer", "description": "Default 30, up to 300"},
                "distance": {"type": "number", "description": "How far off they start, 20 to 150 m (default 60)"},
                "from": {"type": "string", "enum": ["n", "ne", "e", "se", "s", "sw", "w", "nw"], "description": "The side they come from (north is -y); default: one picked for you"}}))}}),
        json!({"name": "make_noise", "description": "Event: a noise at a place or a player that zombies within `radius` metres (less through walls) come to investigate. One of 25 m or more also draws the far horde in over the following minutes; `lasting` keeps that pull for half an hour (an alarm left ringing).",
            "inputSchema": {"type": "object", "properties": place(json!({"radius": {"type": "number", "description": "Metres, up to 500"}, "lasting": {"type": "boolean"}})), "required": ["radius"]}}),
        json!({"name": "spawn_item", "description": "Event: put items on the ground at a place or at a player's feet (a supply drop, a replacement for something lost). `item` is an id from the items tool, e.g. gauze, tinned_beans, baseball_bat.",
            "inputSchema": {"type": "object", "properties": place(json!({"item": {"type": "string"}, "count": {"type": "integer", "description": "Default 1, up to 1000"}})), "required": ["item"]}}),
        json!({"name": "skip_time", "description": "Event: move the game clock forward for everyone, e.g. to bring the night on or end it. Hunger, thirst, wounds and illness move on with it (someone bleeding may die of a long skip); zombies stay where they are. At most 24 hours a call.",
            "inputSchema": {"type": "object", "properties": {"hours": {"type": "number", "description": "Game hours to skip"}, "to_hour": {"type": "number", "description": "Or: skip forward to this hour of the day (0-24), e.g. 7 for morning"}}}}),
        json!({"name": "teleport", "description": "Move a player's survivor: to x, y, or beside another player (`to_player` / `to_name`). For someone stuck, or to bring people together. Refused if the place is off the map or inside something solid.",
            "inputSchema": {"type": "object", "properties": place(json!({"to_player": {"type": "integer", "description": "Put them beside this player instead of at x, y"}, "to_name": {"type": "string"}}))}}),
    ];
    for def in &mut defs {
        let Some(&(name, title, read_only, destructive)) = TOOLS.iter().find(|t| def["name"] == t.0) else { continue };
        def["title"] = json!(title);
        def["annotations"] = if read_only { json!({"title": title, "readOnlyHint": true, "openWorldHint": false}) } else { json!({"title": title, "readOnlyHint": false, "destructiveHint": destructive, "openWorldHint": false}) };
        if matches!(name, "status" | "players" | "settings") {
            def["outputSchema"] = json!({"type": "object"});
        }
    }
    defs.extend(mcp::look_defs(&LOOK));
    Value::Array(defs)
}

fn pretty(v: &Value) -> String {
    serde_json::to_string_pretty(v).unwrap_or_default()
}

fn round1(x: f32) -> f64 {
    (x as f64 * 10.0).round() / 10.0
}

/// The player an argument names: `id_key` (an id) or `name_key` (a name, case ignored).
fn who(host: &Host, args: &Value, id_key: &str, name_key: &str) -> Result<Option<u32>, String> {
    if let Some(id) = args.get(id_key).and_then(Value::as_u64) {
        return host.world.player(id as u32).map(|p| Some(p.id)).ok_or_else(|| format!("no player {id} (see players)"));
    }
    let Some(name) = args.get(name_key).and_then(Value::as_str) else { return Ok(None) };
    let found = host.names().iter().find(|(n, _)| n.eq_ignore_ascii_case(name.trim())).map(|(_, id)| *id).filter(|id| host.world.player(*id).is_some());
    found.map(Some).ok_or_else(|| format!("nobody called {name:?} has played here (see players)"))
}

fn name_of(host: &Host, player: u32) -> String {
    host.names().iter().find(|(_, id)| **id == player).map_or_else(|| format!("player {player}"), |(n, _)| format!("{n} (player {player})"))
}

/// The place an event is for: `x` and `y`, else where the player named stands.
fn target(host: &Host, args: &Value) -> Result<(Vec2, String), String> {
    let num = |k: &str| args.get(k).and_then(Value::as_f64);
    let (pos, at) = match (num("x"), num("y")) {
        (Some(x), Some(y)) => (Vec2::new(x as f32, y as f32), String::new()),
        _ => {
            let id = who(host, args, "player", "name")?.ok_or("say where: x and y, or a player (player or name)")?;
            (host.world.player(id).expect("checked").state.pos, format!(" at {}", name_of(host, id)))
        }
    };
    let map = &host.world.map;
    if !pos.is_finite() || !map.in_bounds(pos.floor().as_ivec2()) {
        return Err(format!("({:.1}, {:.1}) is off the map ({} x {} m)", pos.x, pos.y, map.width(), map.height()));
    }
    Ok((pos, format!("({:.1}, {:.1}){at}", pos.x, pos.y)))
}

impl Admin {
    /// Start the endpoint. The address bound (port 0 picks one).
    pub fn listen(&mut self, args: &HttpArgs) -> Result<SocketAddr, String> {
        let (addr, jobs) = mcp_http::listen(args)?;
        self.jobs = Some(jobs);
        Ok(addr)
    }

    /// After a server tick: take up what the host logged, note deaths, and answer the admin calls
    /// that are waiting (a few; the rest after the next tick). True if anything worth saving
    /// happened.
    pub fn tick(&mut self, host: &mut Host, persist: &mut Persist) -> bool {
        let tick = host.world.tick as u64;
        let mut changed = false;
        for line in host.log.drain(..) {
            self.log.add(tick, line);
            changed = true;
        }
        // Deaths, once each (a respawned player can die again).
        self.dead.retain(|id| host.world.player(*id).is_some_and(|p| p.dead));
        for p in host.world.players.iter().filter(|p| p.dead) {
            if !self.dead.contains(&p.id) {
                self.dead.push(p.id);
                let cause = p.run.cause_of_death_with(&host.world.config.weapons).unwrap_or_else(|| "cause unknown".into());
                self.log.add(tick, format!("{} died: {cause}", name_of(host, p.id)));
            }
        }
        let start = Instant::now();
        for _ in 0..JOBS_A_TICK {
            let Some(Ok(job)) = self.jobs.as_ref().map(|j| j.try_recv()) else { break };
            let _ = job.reply.send(self.handle(host, persist, &job.msg));
            if start.elapsed() > JOB_TIME {
                break;
            }
        }
        changed
    }

    /// One JSON-RPC message from the operator's agent, answered.
    pub fn handle(&mut self, host: &mut Host, persist: &mut Persist, msg: &Value) -> Option<Value> {
        let mut call = |name: &str, args: &Value| {
            if !TOOLS.iter().any(|t| t.0 == name) && !LOOK.contains(&name) {
                return Err(format!("Unknown tool: {name}"));
            }
            let result = self.call(host, persist, name, args).map(mcp::text_content);
            Ok(Called { result, structured: matches!(name, "status" | "players" | "settings") })
        };
        let info = json!({"name": "zombye-server-admin", "title": "Zombye server admin", "version": env!("CARGO_PKG_VERSION")});
        mcp::answer(msg, &Service { info, instructions: &instructions, tools: &tool_list }, &mut call)
    }

    /// Log something the operator's agent did, and mark the world as changed.
    fn did(&mut self, host: &Host, persist: &mut Persist, what: String) {
        self.log.add(host.world.tick as u64, format!("admin: {what}"));
        persist.dirty = true;
    }

    fn call(&mut self, host: &mut Host, persist: &mut Persist, name: &str, args: &Value) -> Result<String, String> {
        let num = |k: &str| args.get(k).and_then(Value::as_f64);
        match name {
            "status" => Ok(pretty(&self.status(host, persist))),
            "players" => Ok(pretty(&players(host))),
            "logs" => {
                let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(50).clamp(1, 500) as usize;
                let after = args.get("after").and_then(Value::as_u64);
                let contains = args.get("contains").and_then(Value::as_str).map(str::to_lowercase);
                let matching: Vec<&Entry> = self.log.lines.iter().filter(|e| after.is_none_or(|a| e.seq > a) && contains.as_ref().is_none_or(|c| e.text.to_lowercase().contains(c))).collect();
                // Following the log reads on from `after`; otherwise the newest.
                let shown: Vec<&Entry> = if after.is_some() { matching.iter().take(limit).copied().collect() } else { matching.iter().skip(matching.len().saturating_sub(limit)).copied().collect() };
                let mut out: String = shown.iter().map(|e| format!("#{} {} tick {}: {}\n", e.seq, stamp(e.unix), e.tick, e.text)).collect();
                out += &format!("{} of {} matching lines; the newest line is #{}", shown.len(), matching.len(), self.log.last);
                if after.is_some() && shown.len() < matching.len() {
                    out += &format!(" (more: call again with after: {})", shown.last().map_or(0, |e| e.seq));
                }
                Ok(out)
            }
            "saves" => mcp::saves_tool(&mut host.world, persist, "saves", args),
            "save" => {
                let path = match args.get("name").and_then(Value::as_str) {
                    Some(n) => save_file::path_for(&persist.dir, n)?,
                    None => persist.path.clone().ok_or("give a name: this server has no --save file")?,
                };
                let said = mcp::saves_tool(&mut host.world, persist, "save", args)?;
                crate::multiplayer::write_names(&path, host);
                self.log.add(host.world.tick as u64, format!("admin: {said}"));
                Ok(said)
            }
            "kick" => {
                let id = who(host, args, "player", "name")?.ok_or("say who: player (an id) or name")?;
                let reason = args.get("reason").and_then(Value::as_str).unwrap_or("").trim().chars().filter(|c| !c.is_control()).take(200).collect::<String>();
                let ban = args.get("ban").and_then(Value::as_bool).unwrap_or(false);
                let because = if reason.is_empty() { String::new() } else { format!(": {reason}") };
                match host.kick(id, &reason, ban) {
                    Some(name) => {
                        let what = format!("{} {name} (player {id}){because}", if ban { "banned" } else { "kicked" });
                        self.did(host, persist, what.clone());
                        Ok(format!("{what}. Their survivor is left as when anyone leaves{}", if ban { "; their name is refused until unban" } else { "" }))
                    }
                    // Not connected: a ban still keeps the name out.
                    None if ban => {
                        let name = host.names().iter().find(|(_, p)| **p == id).map(|(n, _)| n.clone()).ok_or(format!("player {id} has no name to ban"))?;
                        host.ban(zombye_net::host::Ban { name: name.clone(), ip: None, why: reason });
                        self.did(host, persist, format!("banned {name} (player {id}, not online){because}"));
                        Ok(format!("{name} isn't online; their name is banned until unban"))
                    }
                    None => Err(format!("{} isn't online (see players)", name_of(host, id))),
                }
            }
            "unban" => {
                let name = args.get("name").and_then(Value::as_str).ok_or("name is required")?;
                match host.unban(name.trim()) {
                    0 => Err(format!("no ban on {name:?} (see players)")),
                    n => {
                        self.did(host, persist, format!("lifted the ban on {name}"));
                        Ok(format!("lifted {n} ban{} on {name}", if n == 1 { "" } else { "s" }))
                    }
                }
            }
            "settings" => {
                let changed = change_settings(host, persist, args)?;
                for c in &changed {
                    self.did(host, persist, format!("settings: {c}"));
                }
                Ok(pretty(&json!({"changed": changed, "settings": settings(host, persist)})))
            }
            "spawn_zombies" | "make_noise" | "spawn_item" => {
                let (pos, at) = target(host, args)?;
                let mut args = args.clone();
                args["x"] = json!(pos.x);
                args["y"] = json!(pos.y);
                if name == "spawn_zombies" {
                    let count = args.get("count").and_then(Value::as_u64).ok_or("count is required")?;
                    if !(1..=MAX_SPAWN).contains(&count) {
                        return Err(format!("count is 1 to {MAX_SPAWN} a call"));
                    }
                }
                debug_assert!(EVENTS.contains(&name));
                let said = mcp::world_tool(&mut host.world, name, &args)?;
                self.did(host, persist, format!("{name} {at}: {said}"));
                Ok(said)
            }
            "skip_time" => {
                let now = host.world.time_of_day();
                let hours = match (num("hours"), num("to_hour")) {
                    (Some(h), _) => h as f32,
                    (None, Some(to)) if (0.0..=24.0).contains(&to) => (to as f32 - now).rem_euclid(24.0),
                    (None, Some(_)) => return Err("to_hour is 0 to 24".into()),
                    (None, None) => return Err("hours or to_hour is required".into()),
                };
                if !(hours > 0.0 && hours <= 24.0) {
                    return Err("between 0 and 24 hours a call".into());
                }
                let said = mcp::world_tool(&mut host.world, "skip_time", &json!({"hours": hours}))?;
                self.did(host, persist, format!("skipped {hours:.1} h from {}: {said}", mcp::clock(now)));
                Ok(format!("skipped {hours:.1} h: {said}"))
            }
            "horde" => {
                let (pos, at) = target(host, args)?;
                let count = args.get("count").and_then(Value::as_u64).unwrap_or(30);
                if !(1..=300).contains(&count) {
                    return Err("count is 1 to 300".into());
                }
                let distance = num("distance").unwrap_or(60.0) as f32;
                if !(20.0..=150.0).contains(&distance) {
                    return Err("distance is 20 to 150 m".into());
                }
                const SIDES: [&str; 8] = ["e", "se", "s", "sw", "w", "nw", "n", "ne"];
                let side = match args.get("from").and_then(Value::as_str) {
                    Some(s) => SIDES.iter().position(|c| *c == s).ok_or("from is a compass point: n, ne, e, se, s, sw, w or nw")?,
                    // One for us: by the tick, so it is no secret from a replay.
                    None => (host.world.tick as usize / 7) % 8,
                };
                let size = Vec2::new(host.world.map.width() as f32, host.world.map.height() as f32);
                let from = (pos + Vec2::from_angle(side as f32 * std::f32::consts::FRAC_PI_4) * distance).clamp(Vec2::splat(3.0), size - Vec2::splat(3.0));
                let spawned = host.world.spawn_zombies(count as u32, from, 12.0);
                if spawned == 0 {
                    return Err(format!("no room for zombies round ({:.0}, {:.0}): try another side or distance", from.x, from.y));
                }
                // Loud enough to reach where they stand, and lasting: they come, and so does whatever else hears it.
                host.world.make_noise(pos, (from.distance(pos) + 30.0).min(500.0), true);
                let said = format!("a horde of {spawned} for {at}: they start round ({:.0}, {:.0}), {:.0} m to the {}, and a lasting noise draws them in", from.x, from.y, from.distance(pos), SIDES[side]);
                self.did(host, persist, said.clone());
                Ok(said)
            }
            "teleport" => {
                let id = who(host, args, "player", "name")?.ok_or("say who: player (an id) or name")?;
                let to = match who(host, args, "to_player", "to_name")? {
                    Some(other) if other == id => return Err("that is the same player".into()),
                    Some(other) => host.world.player(other).expect("checked").state.pos + Vec2::new(0.8, 0.0),
                    None => match (num("x"), num("y")) {
                        (Some(x), Some(y)) => Vec2::new(x as f32, y as f32),
                        _ => return Err("say where to: x and y, or to_player / to_name".into()),
                    },
                };
                let map = &host.world.map;
                if !to.is_finite() || !map.in_bounds(to.floor().as_ivec2()) {
                    return Err(format!("({:.1}, {:.1}) is off the map ({} x {} m)", to.x, to.y, map.width(), map.height()));
                }
                let at = map.resolve_circle(to, PLAYER_RADIUS);
                if map.tile_blocked(at.floor().as_ivec2()) {
                    return Err(format!("({:.1}, {:.1}) is inside something solid (a tree, furniture): pick a free tile (observe shows them)", to.x, to.y));
                }
                let p = host.world.players.iter_mut().find(|p| p.id == id).expect("checked");
                if p.dead {
                    return Err(format!("player {id} is dead"));
                }
                let was = p.state.pos;
                p.state.pos = at;
                let said = format!("moved {} from ({:.1}, {:.1}) to ({:.1}, {:.1}), {}", name_of(host, id), was.x, was.y, at.x, at.y, mcp::place(&host.world, at));
                self.did(host, persist, said.clone());
                Ok(said)
            }
            _ => {
                debug_assert!(LOOK.contains(&name));
                let mut args = args.clone();
                // A name where the game's tools take a player id.
                if args.get("player").is_none() {
                    if let Some(id) = who(host, &args, "player", "name")? {
                        args["player"] = json!(id);
                    }
                }
                // The operator sees everyone, whatever the survivors see of each other.
                if name == "observe" && args.get("all").is_none() {
                    args["all"] = json!(true);
                }
                mcp::world_tool(&mut host.world, name, &args)
            }
        }
    }

    fn status(&self, host: &Host, persist: &Persist) -> Value {
        let (w, s) = (&host.world, &host.stats);
        json!({
            "name": host.options.name,
            "version": env!("CARGO_PKG_VERSION"),
            "address": host.local_addr().map_or(String::new(), |a| a.to_string()),
            "uptime_secs": self.started.elapsed().as_secs(),
            "tick": w.tick,
            "time_of_day": mcp::clock(w.time_of_day()),
            "daylight": round1(w.daylight()),
            "tick_ms": {"last": round1(s.tick_ms), "sim": round1(s.sim_ms), "worst": round1(s.worst_tick_ms), "budget": round1(zombye_sim::TICK_DT * 1000.0)},
            "online": host.clients().len(),
            "max_players": host.options.max_players,
            "survivors": w.players.len(),
            "survivors_alive": w.players.iter().filter(|p| !p.dead).count(),
            "zombies": {"total": w.zombies.len(), "active": w.stats.active, "near": w.stats.near, "virtual": w.stats.virtual_},
            "map": [w.map.width(), w.map.height()],
            "seed": w.config.generation.seed,
            "network": {"kb_out": s.bytes_out / 1000, "kb_in": s.bytes_in / 1000, "joined": s.joined, "left": s.left, "refused": s.refused, "refused_dev_intents": s.refused_dev, "updates_held_back": s.held},
            "rules": {"pvp": w.config.combat.pvp, "fair_sight": host.fair_sight(), "allow_dev": host.options.allow_dev, "password_set": !host.options.password.is_empty()},
            "save": {
                "path": persist.path.as_ref().map(|p| p.display().to_string()),
                "autosave_mins": persist.autosave.map_or(0.0, |d| d.as_secs_f64() / 60.0),
                "saved_secs_ago": persist.path.as_ref().map(|_| persist.last_save.elapsed().as_secs()),
                "unsaved_changes": persist.dirty,
                "saves_folder": persist.dir.display().to_string(),
            },
            "bans": host.bans().len(),
            "log_newest": self.log.last,
        })
    }
}

fn players(host: &Host) -> Value {
    let w = &host.world;
    let clients = host.clients();
    let survivor = |id: u32| {
        let p = w.player(id)?;
        Some(json!({
            "player": id,
            "pos": [round1(p.state.pos.x), round1(p.state.pos.y)],
            "place": mcp::place(w, p.state.pos),
            "hp": round1(p.hp),
            "dead": p.dead,
            "cause_of_death": p.run.cause_of_death_with(&w.config.weapons),
            "kills": p.run.kills,
            "player_kills": p.run.player_kills,
            "alive_game_hours": round1(p.run.hours as f32),
        }))
    };
    let online: Vec<Value> = clients
        .iter()
        .filter_map(|c| {
            let mut v = survivor(c.player)?;
            let secs = c.secs.max(1.0);
            v["name"] = json!(c.name);
            v["address"] = json!(c.addr);
            v["online_secs"] = json!(c.secs as u64);
            v["kb_per_s_down"] = json!(round1(c.bytes_out as f32 / secs / 1000.0));
            v["kb_per_s_up"] = json!(round1(c.bytes_in as f32 / secs / 1000.0));
            v["zombies_in_view"] = json!(c.zombies);
            v["view_lag_ms"] = json!((w.view_lag_secs(c.player) * 1000.0).round());
            Some(v)
        })
        .collect();
    let offline: Vec<Value> = host
        .names()
        .iter()
        .filter(|(_, id)| !clients.iter().any(|c| c.player == **id))
        .filter_map(|(name, id)| {
            let mut v = survivor(*id)?;
            v["name"] = json!(name);
            Some(v)
        })
        .collect();
    let bans: Vec<Value> = host.bans().iter().map(|b| json!({"name": b.name, "address": b.ip.map(|ip| ip.to_string()), "reason": b.why})).collect();
    json!({"online": online, "offline": offline, "bans": bans, "max_players": host.options.max_players})
}

fn settings(host: &Host, persist: &Persist) -> Value {
    let o = &host.options;
    let spawn = |s: Spawn| if s == Spawn::Spread { "spread" } else { "together" };
    json!({
        "name": o.name,
        "max_players": o.max_players,
        "password_set": !o.password.is_empty(),
        "pvp": host.world.config.combat.pvp,
        "fair_sight": host.fair_sight(),
        "fair_sight_setting": o.fair_sight.map_or(json!("auto"), |b| json!(b)),
        "allow_dev": o.allow_dev,
        "spawn": o.spawn.map_or("auto", spawn),
        "budget_kb": o.budget / 1000,
        "autosave_mins": persist.autosave.map_or(0.0, |d| d.as_secs_f64() / 60.0),
        "zombie_respawn": host.world.config.population.respawn.enabled,
    })
}

/// Check every setting asked for, then apply them all (or none). What changed, in words.
fn change_settings(host: &mut Host, persist: &mut Persist, args: &Value) -> Result<Vec<String>, String> {
    const KEYS: [&str; 10] = ["name", "max_players", "password", "pvp", "fair_sight", "allow_dev", "spawn", "budget_kb", "autosave_mins", "zombie_respawn"];
    let Some(asked) = args.as_object() else { return Ok(Vec::new()) };
    if let Some(unknown) = asked.keys().find(|k| !KEYS.contains(&k.as_str()) && k.as_str() != "_meta") {
        return Err(format!("no setting called {unknown:?}: the settings are {}", KEYS.join(", ")));
    }
    let boolean = |k: &str| match args.get(k) {
        None => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(v) => Err(format!("{k} is true or false, not {v}")),
    };
    let (mut o, mut autosave) = (host.options.clone(), persist.autosave);
    let (mut pvp, mut respawn) = (host.world.config.combat.pvp, host.world.config.population.respawn.enabled);
    let mut changed = Vec::new();
    if let Some(v) = args.get("name") {
        let name = v.as_str().map(str::trim).filter(|n| !n.is_empty() && n.chars().count() <= 48 && !n.chars().any(char::is_control)).ok_or("name is 1 to 48 letters")?;
        if name != o.name {
            changed.push(format!("name {:?} (was {:?})", name, o.name));
            o.name = name.to_string();
        }
    }
    if let Some(v) = args.get("max_players") {
        let n = v.as_u64().filter(|n| (1..=wire::MAX_PLAYERS as u64).contains(n)).ok_or(format!("max_players is 1 to {}", wire::MAX_PLAYERS))? as usize;
        if n != o.max_players {
            changed.push(format!("max_players {n} (was {})", o.max_players));
            o.max_players = n;
        }
    }
    if let Some(v) = args.get("password") {
        let p = v.as_str().filter(|p| p.len() <= 64 && !p.chars().any(char::is_control)).ok_or("password is text of at most 64 characters (\"\" for none)")?;
        if p != o.password {
            // Never the password itself: the log is read by more than the operator.
            changed.push(if p.is_empty() { "password removed".to_string() } else { "password changed".to_string() });
            o.password = p.to_string();
        }
    }
    if let Some(b) = boolean("pvp")? {
        if b != pvp {
            changed.push(format!("pvp {}", if b { "on" } else { "off (co-op)" }));
            pvp = b;
        }
    }
    if let Some(v) = args.get("fair_sight") {
        let f = match v {
            Value::Bool(b) => Some(*b),
            Value::String(s) if s == "auto" => None,
            v => return Err(format!("fair_sight is true, false or \"auto\", not {v}")),
        };
        if f != o.fair_sight {
            changed.push(format!("fair_sight {}", f.map_or("auto (on with pvp)", |b| if b { "on" } else { "off" })));
            o.fair_sight = f;
        }
    }
    if let Some(b) = boolean("allow_dev")? {
        if b != o.allow_dev {
            changed.push(format!("allow_dev {}", if b { "on" } else { "off" }));
            o.allow_dev = b;
        }
    }
    if let Some(v) = args.get("spawn") {
        let s = match v.as_str() {
            Some("spread") => Some(Spawn::Spread),
            Some("together") => Some(Spawn::Together),
            Some("auto") => None,
            _ => return Err(format!("spawn is \"spread\", \"together\" or \"auto\", not {v}")),
        };
        if s != o.spawn {
            changed.push(format!("spawn {}", v.as_str().unwrap_or("")));
            o.spawn = s;
        }
    }
    if let Some(v) = args.get("budget_kb") {
        let (least, most) = (BUDGET_RANGE.0 / 1000, BUDGET_RANGE.1 / 1000);
        let kb = v.as_u64().filter(|kb| (least as u64..=most as u64).contains(kb)).ok_or(format!("budget_kb is {least} to {most}"))? as u32;
        if kb * 1000 != o.budget {
            changed.push(format!("budget {kb} KB/s per client (was {})", o.budget / 1000));
            o.budget = kb * 1000;
        }
    }
    if let Some(v) = args.get("autosave_mins") {
        let mins = v.as_f64().filter(|m| m.is_finite() && (0.0..=1440.0).contains(m)).ok_or("autosave_mins is 0 (off) to 1440")?;
        let every = (mins > 0.0).then(|| Duration::from_secs_f64(mins * 60.0));
        if every != autosave {
            changed.push(if mins > 0.0 { format!("autosave every {mins} min") } else { "autosave off".to_string() });
            autosave = every;
        }
    }
    if let Some(b) = boolean("zombie_respawn")? {
        if b != respawn {
            changed.push(format!("zombie_respawn {}", if b { "on" } else { "off" }));
            respawn = b;
        }
    }
    host.options = o;
    persist.autosave = autosave;
    host.world.config.combat.pvp = pvp;
    host.world.config.population.respawn.enabled = respawn;
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    use zombye_net::content;
    use zombye_net::host::HostOptions;
    use zombye_net::remote::Remote;
    use zombye_sim::World;

    const TOKEN: &str = "an-operators-token-123";

    fn strings(line: &str) -> Vec<String> {
        line.split_whitespace().map(String::from).collect()
    }

    struct Scratch(PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn scratch(name: &str) -> Scratch {
        let dir = std::env::temp_dir().join(format!("zombye-admin-test-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }

    fn server(dir: &Path) -> (Host, Persist, Admin) {
        let mut config = content::load_config(&content::default_data_dir(), 7).unwrap().with_map_chunks(8);
        config.zombies = Some(30);
        let host = Host::bind(World::new(config), "127.0.0.1:0", HostOptions::default()).unwrap();
        let persist = Persist { path: Some(dir.join("server.zsave")), dir: dir.to_path_buf(), content: host.world.config.clone(), autosave: Some(Duration::from_secs(300)), last_save: Instant::now(), dirty: false, benchmark: None };
        let mut admin = Admin::default();
        admin.log.quiet = true;
        (host, persist, admin)
    }

    fn join(host: &mut Host, name: &str) -> Result<Remote, String> {
        let (addr, name) = (host.local_addr().unwrap().to_string(), name.to_string());
        let client = std::thread::spawn(move || Remote::connect(&addr, &name, "", Duration::from_secs(10)));
        while !client.is_finished() {
            host.pump();
            std::thread::sleep(Duration::from_millis(1));
        }
        client.join().unwrap()
    }

    /// A few server ticks, with the admin's share after each.
    fn ticks(host: &mut Host, persist: &mut Persist, admin: &mut Admin, n: usize) {
        for _ in 0..n {
            host.tick();
            // As the server's loop does.
            persist.dirty |= admin.tick(host, persist);
            persist.dirty |= !host.clients().is_empty();
        }
    }

    /// One `tools/call`: the text, and whether it is a tool error.
    fn call(host: &mut Host, persist: &mut Persist, admin: &mut Admin, name: &str, args: Value) -> (String, bool) {
        let msg = json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": name, "arguments": args}});
        let reply = admin.handle(host, persist, &msg).expect("a response");
        let result = &reply["result"];
        (result["content"][0]["text"].as_str().unwrap_or_else(|| panic!("no text: {reply}")).to_string(), result.get("isError") == Some(&json!(true)))
    }

    fn ok(host: &mut Host, persist: &mut Persist, admin: &mut Admin, name: &str, args: Value) -> String {
        let (text, error) = call(host, persist, admin, name, args);
        assert!(!error, "{name}: {text}");
        text
    }

    fn err(host: &mut Host, persist: &mut Persist, admin: &mut Admin, name: &str, args: Value) -> String {
        let (text, error) = call(host, persist, admin, name, args);
        assert!(error, "{name} should fail: {text}");
        text
    }

    #[test]
    fn the_endpoint_always_needs_a_token() {
        assert_eq!(parse(&strings("--host"), Some(TOKEN.into())), Ok(None));
        let a = parse(&strings("--host --admin-mcp"), Some(TOKEN.into())).unwrap().unwrap();
        assert_eq!((a.addr.as_str(), a.token.as_deref(), a.dev), ("127.0.0.1:8788", Some(TOKEN), true));
        assert_eq!(parse(&strings("--host --admin-mcp 9100 --admin-token"), Some(TOKEN.into())).unwrap().unwrap().addr, "127.0.0.1:9100");
        let a = parse(&strings(&format!("--host --admin-mcp 0.0.0.0:9100 --admin-token {TOKEN}x --admin-origin https://ops.example/")), Some(TOKEN.into())).unwrap().unwrap();
        assert_eq!((a.addr.as_str(), a.token.unwrap(), a.origins), ("0.0.0.0:9100", format!("{TOKEN}x"), vec!["https://ops.example".to_string()]));
        assert_eq!(parse(&strings("--host --admin-mcp 10.0.0.5"), Some(TOKEN.into())).unwrap().unwrap().addr, "10.0.0.5:8788");
        // No token, an empty one, a short one: it doesn't start. `--mcp-no-auth` opens nothing here.
        for (line, env) in [("--host --admin-mcp", None), ("--host --admin-mcp", Some("  ")), ("--host --admin-mcp --mcp-no-auth", None), ("--host --admin-mcp --admin-token short", None), ("--host --admin-mcp", Some("has a space in it, which no header carries"))] {
            let e = parse(&strings(line), env.map(String::from)).unwrap_err();
            assert!(e.contains("token"), "{line}: {e}");
            assert!(!e.contains("--mcp-"), "its own flags are named: {e}");
        }
    }

    #[test]
    fn times_are_stamped_in_utc() {
        assert_eq!(stamp(0), "1970-01-01 00:00:00Z");
        assert_eq!(stamp(951_782_400), "2000-02-29 00:00:00Z");
        assert_eq!(stamp(1_794_330_249), "2026-11-10 17:04:09Z");
    }

    /// Every tool listed can be called, has a title and says whether it only reads; the ones that
    /// only read leave the world exactly as it was.
    #[test]
    fn tools_are_listed_described_and_the_read_only_ones_change_nothing() {
        let dir = scratch("list");
        let (mut host, mut persist, mut admin) = server(&dir.0);
        let _ada = join(&mut host, "ada").unwrap();
        ticks(&mut host, &mut persist, &mut admin, 5);
        let list = admin.handle(&mut host, &mut persist, &json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"})).unwrap();
        let tools = list["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), TOOLS.len() + LOOK.len());
        let init = admin.handle(&mut host, &mut persist, &json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18"}})).unwrap();
        assert_eq!(init["result"]["serverInfo"]["name"], "zombye-server-admin");
        assert!(init["result"]["instructions"].as_str().unwrap().len() < 2200);
        for t in tools {
            let name = t["name"].as_str().unwrap();
            assert!(t["title"].is_string() && t["description"].as_str().is_some_and(|d| d.len() > 20) && t["inputSchema"]["type"] == "object", "{name}");
            assert_eq!(t["annotations"]["openWorldHint"], false, "{name}");
            // None of the game's tools that play or debug: only the ones named here.
            assert!(TOOLS.iter().any(|a| a.0 == name) || LOOK.contains(&name), "{name}");
            let msg = json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": name, "arguments": {}}});
            if t["annotations"]["readOnlyHint"] == true {
                let (hash, tick, lines) = (host.world.state_hash(), host.world.tick, admin.log.last);
                persist.dirty = false;
                let reply = admin.handle(&mut host, &mut persist, &msg).unwrap();
                assert!(reply["error"].is_null(), "{name}: {reply}");
                assert_eq!((host.world.state_hash(), host.world.tick, admin.log.last, persist.dirty), (hash, tick, lines, false), "{name} only reads");
            } else {
                // With nothing said, each asks for what it needs (or, `settings`, shows them).
                let reply = admin.handle(&mut host, &mut persist, &msg).unwrap();
                assert!(reply["error"].is_null(), "{name} is a tool: {reply}");
            }
        }
        for name in ["step_ticks", "load", "dev", "add_player", "attack", "screenshot", "nonsense"] {
            let reply = admin.handle(&mut host, &mut persist, &json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": name, "arguments": {}}})).unwrap();
            assert_eq!(reply["error"]["code"], -32602, "{name} isn't offered: {reply}");
        }
    }

    #[test]
    fn players_are_listed_kicked_and_banned() {
        let dir = scratch("kick");
        let (mut host, mut persist, mut admin) = server(&dir.0);
        let _ada = join(&mut host, "ada").unwrap();
        let mut bob = join(&mut host, "bob").unwrap();
        ticks(&mut host, &mut persist, &mut admin, 3);
        let list: Value = serde_json::from_str(&ok(&mut host, &mut persist, &mut admin, "players", json!({}))).unwrap();
        let names: Vec<&str> = list["online"].as_array().unwrap().iter().map(|p| p["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["ada", "bob"]);
        assert!(list["online"][1]["address"].as_str().unwrap().starts_with("127.0.0.1:") && list["online"][1]["hp"] == 100.0 && list["online"][1]["dead"] == false, "{list}");
        // Structured too, for clients that take it.
        let reply = admin.handle(&mut host, &mut persist, &json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "status", "arguments": {}}})).unwrap();
        let status = &reply["result"]["structuredContent"];
        assert_eq!((status["online"].as_u64(), status["survivors"].as_u64(), status["rules"]["pvp"].as_bool(), status["rules"]["password_set"].as_bool()), (Some(2), Some(2), Some(false), Some(false)), "{status}");

        assert!(err(&mut host, &mut persist, &mut admin, "kick", json!({})).contains("say who"));
        assert!(err(&mut host, &mut persist, &mut admin, "kick", json!({"name": "mallory"})).contains("nobody called"));
        assert!(err(&mut host, &mut persist, &mut admin, "kick", json!({"player": 9})).contains("no player 9"));
        let said = ok(&mut host, &mut persist, &mut admin, "kick", json!({"name": "Bob", "reason": "blocking\nthe door"}));
        assert!(said.starts_with("kicked bob (player 2): blockingthe door"), "{said}");
        ticks(&mut host, &mut persist, &mut admin, 2);
        let start = Instant::now();
        while bob.closed.is_none() && start.elapsed() < Duration::from_secs(10) {
            bob.poll();
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(bob.closed.as_deref().is_some_and(|w| w.contains("kicked: blockingthe door")), "{:?}", bob.closed);
        let list: Value = serde_json::from_str(&ok(&mut host, &mut persist, &mut admin, "players", json!({}))).unwrap();
        assert_eq!((list["online"].as_array().unwrap().len(), list["offline"][0]["name"].as_str()), (1, Some("bob")), "his survivor stays: {list}");
        assert!(err(&mut host, &mut persist, &mut admin, "kick", json!({"name": "bob"})).contains("isn't online"));

        // Banned while away: the name is refused; lifted: back as the same survivor.
        ok(&mut host, &mut persist, &mut admin, "kick", json!({"player": 2, "ban": true, "reason": "griefing"}));
        assert!(join(&mut host, "bob").unwrap_err().contains("banned from this server: griefing"));
        ticks(&mut host, &mut persist, &mut admin, 1);
        let list: Value = serde_json::from_str(&ok(&mut host, &mut persist, &mut admin, "players", json!({}))).unwrap();
        assert_eq!(list["bans"], json!([{"name": "bob", "address": null, "reason": "griefing"}]));
        assert!(err(&mut host, &mut persist, &mut admin, "unban", json!({"name": "ada"})).contains("no ban"));
        ok(&mut host, &mut persist, &mut admin, "unban", json!({"name": "bob"}));
        assert_eq!(join(&mut host, "bob").unwrap().player(), 2);
        ticks(&mut host, &mut persist, &mut admin, 1);

        // All of it is in the log, in order, and the log can be followed and searched.
        let log = ok(&mut host, &mut persist, &mut admin, "logs", json!({"limit": 500}));
        let order = ["ada joined", "bob joined", "admin: kicked bob (player 2): blockingthe door", "bob left: kicked: blockingthe door", "admin: banned bob (player 2, not online): griefing", "refused bob: you are banned", "admin: lifted the ban on bob", "bob joined"];
        let mut from = 0;
        for want in order {
            from += log[from..].find(want).unwrap_or_else(|| panic!("{want:?} after byte {from} of:\n{log}"));
        }
        assert!(log.lines().next().unwrap().starts_with("#1 20") && log.contains("Z tick "), "{log}");
        let admin_only = ok(&mut host, &mut persist, &mut admin, "logs", json!({"contains": "ADMIN:"}));
        assert!(admin_only.lines().filter(|l| l.starts_with('#')).all(|l| l.contains("admin: ")) && admin_only.contains("3 of 3 matching"), "{admin_only}");
        let newest = admin.log.last;
        assert!(ok(&mut host, &mut persist, &mut admin, "logs", json!({"after": newest})).starts_with("0 of 0"));
        let two = ok(&mut host, &mut persist, &mut admin, "logs", json!({"after": 0, "limit": 2}));
        assert!(two.starts_with("#1 ") && two.contains("\n#2 ") && two.contains("more: call again with after: 2"), "{two}");
    }

    #[test]
    fn settings_show_and_change() {
        let dir = scratch("settings");
        let (mut host, mut persist, mut admin) = server(&dir.0);
        let shown: Value = serde_json::from_str(&ok(&mut host, &mut persist, &mut admin, "settings", json!({}))).unwrap();
        assert_eq!(shown["changed"], json!([]));
        assert_eq!(shown["settings"], json!({"name": "Zombye server", "max_players": 32, "password_set": false, "pvp": false, "fair_sight": false, "fair_sight_setting": "auto", "allow_dev": false, "spawn": "auto", "budget_kb": 32, "autosave_mins": 5.0, "zombie_respawn": false}));
        assert!(!persist.dirty);

        let said = ok(&mut host, &mut persist, &mut admin, "settings", json!({"name": "Dead Commons", "max_players": 1, "password": "brains-brains", "pvp": true, "spawn": "together", "budget_kb": 64, "autosave_mins": 0.5, "zombie_respawn": true, "allow_dev": false}));
        let after: Value = serde_json::from_str(&said).unwrap();
        assert_eq!(after["changed"].as_array().unwrap().len(), 8, "allow_dev didn't change: {said}");
        assert!(!said.contains("brains-brains"), "the password is never shown back");
        assert_eq!(after["settings"], json!({"name": "Dead Commons", "max_players": 1, "password_set": true, "pvp": true, "fair_sight": true, "fair_sight_setting": "auto", "allow_dev": false, "spawn": "together", "budget_kb": 64, "autosave_mins": 0.5, "zombie_respawn": true}));
        assert!(persist.dirty && host.world.config.combat.pvp && host.rules().pvp && host.rules().fair_sight && host.world.config.population.respawn.enabled);
        assert_eq!((host.options.budget, persist.autosave, host.options.spawn), (64_000, Some(Duration::from_secs(30)), Some(Spawn::Together)));
        let log = ok(&mut host, &mut persist, &mut admin, "logs", json!({}));
        assert!(log.contains("admin: settings: password changed") && log.contains("admin: settings: pvp on") && !log.contains("brains-brains"), "{log}");

        // They hold at the door: the password, then the limit.
        let addr = host.local_addr().unwrap().to_string();
        assert!(join(&mut host, "ada").unwrap_err().contains("wrong password"));
        let with = |host: &mut Host, name: &'static str| {
            let addr = addr.clone();
            let client = std::thread::spawn(move || Remote::connect(&addr, name, "brains-brains", Duration::from_secs(10)));
            while !client.is_finished() {
                host.pump();
                std::thread::sleep(Duration::from_millis(1));
            }
            client.join().unwrap()
        };
        let ada = with(&mut host, "ada").unwrap();
        assert!(ada.welcome.rules.pvp && ada.welcome.rules.name == "Dead Commons");
        assert!(with(&mut host, "bob").unwrap_err().contains("full (1 players)"));

        // One bad value and nothing is changed.
        for (bad, why) in [
            (json!({"pvp": false, "max_players": 65}), "1 to 64"),
            (json!({"pvp": "off"}), "true or false"),
            (json!({"budget_kb": 2}), "8 to 1000"),
            (json!({"spawn": "nowhere"}), "spread"),
            (json!({"fair_sight": "dim"}), "auto"),
            (json!({"name": ""}), "1 to 48"),
            (json!({"autosave_mins": -1}), "0 (off)"),
            (json!({"gravity": 2}), "no setting called"),
        ] {
            assert!(err(&mut host, &mut persist, &mut admin, "settings", bad.clone()).contains(why), "{bad}");
        }
        assert!(host.world.config.combat.pvp && host.options.max_players == 1);
        let off: Value = serde_json::from_str(&ok(&mut host, &mut persist, &mut admin, "settings", json!({"password": "", "fair_sight": false, "autosave_mins": 0}))).unwrap();
        assert_eq!((off["settings"]["password_set"].as_bool(), off["settings"]["fair_sight"].as_bool(), persist.autosave), (Some(false), Some(false), None));
    }

    #[test]
    fn saving_writes_the_world_and_whose_survivor_is_whose() {
        let dir = scratch("save");
        let (mut host, mut persist, mut admin) = server(&dir.0);
        let _ada = join(&mut host, "ada").unwrap();
        ticks(&mut host, &mut persist, &mut admin, 10);
        assert!(persist.dirty);
        let said = ok(&mut host, &mut persist, &mut admin, "save", json!({}));
        assert!(said.starts_with("saved tick 10 to "), "{said}");
        assert!(!persist.dirty, "the session's file is up to date");
        assert_eq!(std::fs::read_to_string(dir.0.join("server.zsave.players")).unwrap(), "1\tada\n");
        let loaded = save_file::load(&dir.0.join("server.zsave"), &persist.content, Default::default()).unwrap().0.world;
        assert_eq!((loaded.state_hash(), loaded.players.len()), (host.world.state_hash(), 1));

        ok(&mut host, &mut persist, &mut admin, "save", json!({"name": "before-the-horde"}));
        assert!(dir.0.join("before-the-horde.zsave").exists() && dir.0.join("before-the-horde.zsave.players").exists());
        assert!(err(&mut host, &mut persist, &mut admin, "save", json!({"name": "../elsewhere"})).contains("name"));
        let saves = ok(&mut host, &mut persist, &mut admin, "saves", json!({}));
        assert!(saves.contains("before-the-horde") && saves.contains("server") && saves.contains("autosave every 5 min"), "{saves}");
        assert!(ok(&mut host, &mut persist, &mut admin, "logs", json!({"contains": "admin: saved"})).contains("2 of 2"));
        // Nowhere to save to without a name or a --save file.
        persist.path = None;
        assert!(err(&mut host, &mut persist, &mut admin, "save", json!({})).contains("give a name"));
    }

    #[test]
    fn events_happen_in_the_world_people_play_in() {
        let dir = scratch("events");
        let (mut host, mut persist, mut admin) = server(&dir.0);
        let _ada = join(&mut host, "ada").unwrap();
        let _bob = join(&mut host, "bob").unwrap();
        ticks(&mut host, &mut persist, &mut admin, 3);
        let at = |host: &Host, id: u32| host.world.player(id).unwrap().state.pos;
        let zombies = host.world.zombies.len();

        // Zombies at a player; too many, or nowhere, is refused.
        let said = ok(&mut host, &mut persist, &mut admin, "spawn_zombies", json!({"name": "ada", "count": 12, "radius": 8}));
        assert!(said.starts_with("spawned 12 zombies"), "{said}");
        assert_eq!(host.world.zombies.len(), zombies + 12);
        assert!(err(&mut host, &mut persist, &mut admin, "spawn_zombies", json!({"name": "ada", "count": 501})).contains("1 to 500"));
        assert!(err(&mut host, &mut persist, &mut admin, "spawn_zombies", json!({"count": 5})).contains("say where"));
        assert!(err(&mut host, &mut persist, &mut admin, "spawn_zombies", json!({"count": 5, "x": -4, "y": 10})).contains("off the map"));

        // A horde: zombies a way off, and a lasting pull on them at the target.
        let attractors = host.world.population().attractors.len();
        let said = ok(&mut host, &mut persist, &mut admin, "horde", json!({"player": 1, "count": 20, "distance": 40, "from": "e"}));
        assert!(said.starts_with("a horde of 20 for ") && said.contains("to the e"), "{said}");
        assert_eq!((host.world.zombies.len(), host.world.population().attractors.len()), (zombies + 32, attractors + 1));
        assert!(err(&mut host, &mut persist, &mut admin, "horde", json!({"player": 1, "distance": 5})).contains("20 to 150"));
        assert!(err(&mut host, &mut persist, &mut admin, "horde", json!({"player": 1, "from": "up"})).contains("compass"));

        // Items at someone's feet.
        let said = ok(&mut host, &mut persist, &mut admin, "spawn_item", json!({"name": "bob", "item": "gauze", "count": 3}));
        assert!(said.contains("Gauze roll"), "{said}");
        let bob_at = at(&host, 2);
        assert!(host.world.items().iter().any(|i| i.pos.distance(bob_at) < 0.1 && i.stack.count == 3));
        assert!(err(&mut host, &mut persist, &mut admin, "spawn_item", json!({"name": "bob", "item": "unobtainium"})).contains("unknown item"));

        // A noise, the clock, and a player moved beside another.
        assert!(ok(&mut host, &mut persist, &mut admin, "make_noise", json!({"x": 40, "y": 40, "radius": 30})).contains("noise at (40.0, 40.0)"));
        let hour = host.world.time_of_day();
        let said = ok(&mut host, &mut persist, &mut admin, "skip_time", json!({"to_hour": 22}));
        assert!((host.world.time_of_day() - 22.0).abs() < 0.01 && said.contains("it is now 2"), "{said} (from {hour})");
        assert!(err(&mut host, &mut persist, &mut admin, "skip_time", json!({"hours": 48})).contains("0 and 24"));
        let said = ok(&mut host, &mut persist, &mut admin, "teleport", json!({"name": "bob", "to_name": "ada"}));
        assert!(said.starts_with("moved bob (player 2) from "), "{said}");
        assert!(at(&host, 2).distance(at(&host, 1)) < 1.5);
        assert!(err(&mut host, &mut persist, &mut admin, "teleport", json!({"name": "bob", "x": 9999, "y": 1})).contains("off the map"));
        assert!(err(&mut host, &mut persist, &mut admin, "teleport", json!({"name": "bob", "to_player": 2})).contains("same player"));

        // The world goes on ticking with all of it in, and the operator can look at it.
        ticks(&mut host, &mut persist, &mut admin, 20);
        let map = ok(&mut host, &mut persist, &mut admin, "observe", json!({"name": "ada"}));
        assert!(map.contains('P') && map.contains("zombies in view"), "{map}");
        assert!(ok(&mut host, &mut persist, &mut admin, "inventory", json!({"name": "bob"})).contains("player 2"));
        let log = ok(&mut host, &mut persist, &mut admin, "logs", json!({"contains": "admin:", "limit": 100}));
        for what in ["spawn_zombies (", "a horde of 20", "spawn_item (", "make_noise (40.0, 40.0)", "skipped ", "moved bob"] {
            assert!(log.contains(what), "{what} in:\n{log}");
        }
    }

    /// A death is logged once, with its cause, under the player's name.
    #[test]
    fn deaths_are_logged() {
        let dir = scratch("deaths");
        let (mut host, mut persist, mut admin) = server(&dir.0);
        let _ada = join(&mut host, "ada").unwrap();
        ticks(&mut host, &mut persist, &mut admin, 2);
        host.world.inflict_wound(1, zombye_sim::Part::Torso, zombye_sim::WoundKind::Bite, Some(false)).unwrap();
        ok(&mut host, &mut persist, &mut admin, "skip_time", json!({"hours": 24}));
        ticks(&mut host, &mut persist, &mut admin, 5);
        let log = ok(&mut host, &mut persist, &mut admin, "logs", json!({"contains": ") died"}));
        assert!(log.contains("ada (player 1) died: Bled out from a bite") && log.contains("1 of 1 matching"), "{log}");
    }
}
