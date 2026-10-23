//! Minimal MCP (Model Context Protocol) server over stdio: newline-delimited JSON-RPC 2.0.
//!
//! Every tool goes through the same [`Intent`] path a game client uses, so an agent can't do
//! anything a client couldn't (admin tools will be gated separately).

use std::io::{BufRead, Write};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use glam::{IVec2, Vec2};
use serde_json::{Value, json};
use zombye_net::{Intent, apply_intent, save_file, shutdown};
use zombye_sim::map::{Edge, EdgeRef, Feature, Floor, OwnedEdge, Side};
use zombye_sim::senses::{self, Cone, Target};
use zombye_sim::vision::line_of_sight;
use zombye_sim::{ActionKind, HitEffect, LoadOptions, TICK_DT, Tier, World, ZombieState};

use crate::screenshot::{self, Renderer};
use crate::Persist;

const MAX_STEP: u64 = 20 * 60 * 60; // one hour of game time per call
const MAX_MOVE_TICKS: u64 = 20 * 60 * 5; // five minutes of walking per move_player call

/// Compass directions on the map: north is up (−y), east is +x.
const COMPASS: [(&str, [f32; 2]); 8] =
    [("e", [1.0, 0.0]), ("se", [1.0, 1.0]), ("s", [0.0, 1.0]), ("sw", [-1.0, 1.0]), ("w", [-1.0, 0.0]), ("nw", [-1.0, -1.0]), ("n", [0.0, -1.0]), ("ne", [1.0, -1.0])];

/// Tools that only look: they don't make the world need saving.
const READ_ONLY: [&str; 11] = ["observe", "zombie", "edge", "weapons", "buildings", "regions", "stats", "saves", "save", "load", "screenshot"];

/// The agent plays player 1, who wakes up in the bedroom like a single-player client does. A
/// loaded world keeps its players.
pub fn add_agent_player(world: &mut World) {
    if world.players.is_empty() {
        world.add_player(world.player_spawn);
    }
}

pub fn serve(mut world: World, persist: &mut Persist, renderer: &Renderer) {
    add_agent_player(&mut world);
    // Read stdin on its own thread, so the main loop can wake up for autosaves and Ctrl-C.
    let (tx, rx) = mpsc::channel::<Option<String>>();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            if tx.send(Some(line)).is_err() {
                return;
            }
        }
        let _ = tx.send(None);
    });
    let mut stdout = std::io::stdout();
    let why = loop {
        let line = match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(Some(line)) => Some(line),
            Ok(None) | Err(RecvTimeoutError::Disconnected) => break "stdin closed",
            Err(RecvTimeoutError::Timeout) => None,
        };
        if let Some(line) = line.filter(|l| !l.trim().is_empty()) {
            let response = match serde_json::from_str::<Value>(&line) {
                Ok(msg) => handle(&mut world, persist, renderer, &msg),
                Err(e) => Some(json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": e.to_string()}})),
            };
            if let Some(response) = response {
                let _ = writeln!(stdout, "{response}");
                let _ = stdout.flush();
            }
        }
        if shutdown::requested() {
            break "interrupted";
        }
        if persist.autosave_due() {
            persist.save_now(&world, "autosave");
        }
    };
    if persist.dirty {
        persist.save_now(&world, why);
    }
    shutdown::finished();
}

fn handle(world: &mut World, persist: &mut Persist, renderer: &Renderer, msg: &Value) -> Option<Value> {
    let id = msg.get("id").cloned();
    let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
    // Notifications (no id) get no response.
    let id = id?;
    let result = match method {
        "initialize" => Ok(json!({
            "protocolVersion": msg["params"]["protocolVersion"].as_str().unwrap_or("2025-06-18"),
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "zombye-server", "version": env!("CARGO_PKG_VERSION")},
            "instructions": "Zombye headless game server. The world is a tile map (1 tile = 1 m) with a \
                generated town and forest. Time is paused between calls: use step_ticks to advance (20 ticks = 1 s). \
                You control player 1 (P on the map), who wakes up in a bedroom: move_player walks, runs or sneaks it. \
                Zombies notice players by sight (a view cone; shorter at night, against sneaking or standing still) and \
                sound (footsteps: sneak quiet, run loud; doors; groans of other zombies), then investigate, chase, lunge \
                and attack. Zombies can't open doors, but they batter shut doors and windows between them and what they're \
                after until they break (windows go quickly, doors take a while; every thump is a noise that draws more). \
                interact opens and closes the door or window you face; open or broken windows can be climbed through, slowly; \
                smash_window breaks one loudly. Fight or flee: you have 100 HP and zombie strikes hurt (more of them on you, \
                more land). Weapons (w on the map) lie in houses: pick_up equips the nearest one you face. attack swings it \
                (or punches), shove pushes zombies back and staggers them, stomp finishes one that's down (v on the map). \
                Each costs endurance; tired swings are slower and weaker; hits from behind crit. weapons lists their numbers. \
                Use observe to see an ASCII map with walls, doors, windows, items and zombies \
                (letters show what each is doing), zombie to inspect one, edge for a door's or window's state and hit points, \
                and buildings to list houses. Zombies far from every player are simulated in less detail (tiers: active \
                within about 60 m, near out to 150 m, virtual beyond; stats has the counts): only active ones fight, see and \
                show their state on the map, but every zombie keeps heading where it was going. Loud noises also draw the \
                far horde in slowly, over minutes (regions shows zombies per region, targets and the pull of loud events). \
                save and load keep and restore the whole world (saves lists them); with --save the server also autosaves. \
                screenshot returns a rendered PNG of what a player sees (fog of war included), to check visuals; it takes a \
                second or two and changes nothing."
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({"tools": tool_list()})),
        "tools/call" => {
            let name = msg["params"]["name"].as_str().unwrap_or("");
            let args = msg["params"].get("arguments").cloned().unwrap_or(json!({}));
            if name == "screenshot" {
                return Some(json!({"jsonrpc": "2.0", "id": id, "result": match screenshot_tool(world, renderer, &args) {
                    Ok(content) => json!({"content": content}),
                    Err(text) => json!({"content": [{"type": "text", "text": text}], "isError": true}),
                }}));
            }
            let result = match name {
                "save" | "load" | "saves" => save_tool(world, persist, name, &args),
                _ => call_tool(world, name, &args),
            };
            if !READ_ONLY.contains(&name) {
                persist.dirty = true;
            }
            Ok(match result {
                Ok(text) => json!({"content": [{"type": "text", "text": text}]}),
                Err(text) => json!({"content": [{"type": "text", "text": text}], "isError": true}),
            })
        }
        _ => Err(json!({"code": -32601, "message": format!("unknown method {method}")})),
    };
    Some(match result {
        Ok(r) => json!({"jsonrpc": "2.0", "id": id, "result": r}),
        Err(e) => json!({"jsonrpc": "2.0", "id": id, "error": e}),
    })
}

fn tool_list() -> Value {
    json!([
        {
            "name": "observe",
            "description": "Look at the world around a point (default: player 1). ASCII map at double resolution: \
                tiles sit between wall lines. Walls - |, shut windows = !, open windows o, broken windows x, \
                shut doors D (d when damaged), open doors /, broken doors #, fences ~, \
                floors: . grass  , road  : sidewalk/path  _ indoors; T tree, * bush, f furniture, w weapon on the floor, \
                P player (X dead), G debug goal, @ spawn. \
                Active zombies by state: Z wandering, I investigating a sound, C chasing a player, L lunging, A attacking, \
                B battering a door or window, S staggered, v down on the ground, % corpse; zombies on the cheaper tiers \
                far from players: n near (updated a few times a second), ? virtual (moved in bulk every few seconds). \
                Lists players (HP, weapon, whether they're climbing through a window or dead), weapons on the floor, damaged doors \
                and windows in view with their hit points, and the nearest zombies with HP (use zombie {id} for one zombie's details).",
            "inputSchema": {"type": "object", "properties": {
                "x": {"type": "number", "description": "Centre x in metres"},
                "y": {"type": "number", "description": "Centre y in metres"},
                "radius": {"type": "integer", "minimum": 2, "maximum": 30, "default": 10}
            }}
        },
        {
            "name": "zombie",
            "description": "One zombie's hit points, simulation tier (active, near or virtual) and AI: state (including staggered, down and dead), target player or point, \
                memory/interest timer, cooldowns, and for each player the distance, line of sight and how far this zombie could see them right now.",
            "inputSchema": {"type": "object", "required": ["id"], "properties": {"id": {"type": "integer"}}}
        },
        {
            "name": "make_noise",
            "description": "Debug: a noise at (x, y). Zombies within radius metres (40% of that through walls) come to investigate; \
                heard on the next tick. For scale: walking 6 m, running 14 m, a door 10 m, a groan 9 m, a zombie thumping a door 15 m, breaking glass 22 m, \
                breaking a door 26 m. A loud one (25 m or more, see population.ron) also leaves an attractor: over the next minutes far (virtual) \
                zombies from up to 4x the radius away (less from indoors) drift in toward it. lasting: true makes that pull last 30 minutes \
                instead of 10, like an alarm left ringing. See regions.",
            "inputSchema": {"type": "object", "required": ["x", "y", "radius"], "properties": {
                "x": {"type": "number"}, "y": {"type": "number"},
                "radius": {"type": "number", "minimum": 0, "maximum": 500},
                "lasting": {"type": "boolean", "default": false, "description": "Leave a long-lived attractor"}
            }}
        },
        {
            "name": "step_ticks",
            "description": "Advance the simulation. 20 ticks = 1 second of game time. Reports zombie attacks on players (and \
                how many landed, for how much), player hits on zombies, kills, deaths, thumps on doors and windows, and anything that broke.",
            "inputSchema": {"type": "object", "properties": {
                "n": {"type": "integer", "minimum": 1, "maximum": MAX_STEP, "default": 20}
            }}
        },
        {
            "name": "move_player",
            "description": "Drive a player: hold a movement input for some ticks (the world advances), then stop. \
                Walk 3 m/s, run 5.5 m/s (drains endurance; needs some left), sneak 1.4 m/s. Faces the way it moves unless facing is given.",
            "inputSchema": {"type": "object", "properties": {
                "player": {"type": "integer", "default": 1},
                "direction": {"type": "string", "enum": ["n", "ne", "e", "se", "s", "sw", "w", "nw", "stop"], "description": "Compass direction; north is up (-y). Or give dx/dy."},
                "dx": {"type": "number", "description": "Ground-plane direction x (east), used when direction is omitted"},
                "dy": {"type": "number", "description": "Ground-plane direction y (south)"},
                "ticks": {"type": "integer", "minimum": 1, "maximum": MAX_MOVE_TICKS, "default": 20},
                "run": {"type": "boolean", "default": false},
                "sneak": {"type": "boolean", "default": false},
                "facing": {"type": "number", "description": "Facing in radians (0 = east, pi/2 = south)"}
            }}
        },
        {
            "name": "set_goal",
            "description": "Debug override: every zombie within 120 m (path distance) heads for this point, ignoring sight and sound. Omit x/y to go back to normal senses.",
            "inputSchema": {"type": "object", "properties": {"x": {"type": "number"}, "y": {"type": "number"}}}
        },
        {
            "name": "spawn_zombies",
            "description": "Spawn zombies on open tiles in a circle.",
            "inputSchema": {"type": "object", "required": ["count", "x", "y"], "properties": {
                "count": {"type": "integer", "minimum": 1, "maximum": 20000},
                "x": {"type": "number"}, "y": {"type": "number"},
                "radius": {"type": "number", "default": 10}
            }}
        },
        {
            "name": "interact",
            "description": "A player opens or closes the door or window it faces, if one is within 1.6 m of it (face it with \
                move_player's facing first if needed). Doors are noisy (10 m), windows quiet (2.5 m). Broken ones can't be shut.",
            "inputSchema": {"type": "object", "properties": {"player": {"type": "integer", "default": 1}}}
        },
        {
            "name": "smash_window",
            "description": "A player smashes the window it faces (shut or open), within 1.6 m. Loud (22 m); it stays broken \
                and can be climbed through.",
            "inputSchema": {"type": "object", "properties": {"player": {"type": "integer", "default": 1}}}
        },
        {
            "name": "attack",
            "description": "A player swings its weapon (bare hands: a punch) the way it faces, then the world steps until the \
                swing has landed and the player can act again. Hits zombies within the weapon's reach and arc, nearest first up to its target count, with a clear \
                line (not through glass or fences); damage, knockback and a chance to stagger or knock down. Downed zombies are \
                below the swing: stomp them. Costs endurance; makes noise. Give facing (radians) or target (a zombie id) to turn first.",
            "inputSchema": {"type": "object", "properties": {
                "player": {"type": "integer", "default": 1},
                "facing": {"type": "number", "description": "Turn to this angle first (0 = east, pi/2 = south)"},
                "target": {"type": "integer", "description": "Turn to face this zombie first"}
            }}
        },
        {
            "name": "shove",
            "description": "A player shoves the zombies in front (short reach, wide arc, up to 3): no damage, pushes them back and \
                always staggers them, with a chance to knock down (better against a staggered zombie or from behind). Cheap, \
                quick, quiet. Steps until it lands and the player can act again. facing / target as for attack.",
            "inputSchema": {"type": "object", "properties": {
                "player": {"type": "integer", "default": 1},
                "facing": {"type": "number"},
                "target": {"type": "integer"}
            }}
        },
        {
            "name": "stomp",
            "description": "A player stamps on a downed zombie at its feet in front (within about 1 m): solid damage. A miss if \
                none is down there when it lands. Steps until it lands and the player can act again. facing / target as for attack.",
            "inputSchema": {"type": "object", "properties": {
                "player": {"type": "integer", "default": 1},
                "facing": {"type": "number"},
                "target": {"type": "integer"}
            }}
        },
        {
            "name": "pick_up",
            "description": "A player picks up the weapon on the floor nearest to where it faces, within 1.3 m, and equips it; \
                the one it held is left in its place.",
            "inputSchema": {"type": "object", "properties": {"player": {"type": "integer", "default": 1}}}
        },
        {
            "name": "weapons",
            "description": "The weapon table (from assets/data/items/weapons.ron): damage, reach, arc, swing time, targets, \
                knockback, stagger/knockdown/crit chances, endurance cost and noise.",
            "inputSchema": {"type": "object", "properties": {}}
        },
        {
            "name": "spawn_item",
            "description": "Debug: put a weapon on the ground at (x, y), by id (see weapons).",
            "inputSchema": {"type": "object", "required": ["weapon", "x", "y"], "properties": {
                "weapon": {"type": "string"}, "x": {"type": "number"}, "y": {"type": "number"}
            }}
        },
        {
            "name": "respawn",
            "description": "A dead player gets up again in the bedroom with full health and empty hands.",
            "inputSchema": {"type": "object", "properties": {"player": {"type": "integer", "default": 1}}}
        },
        {
            "name": "edge",
            "description": "The four edges of tile (x, y): what each is (wall, window shut/open/broken, door shut/open/broken, \
                arch, fence) and, for shut or open doors and windows, hit points left of full.",
            "inputSchema": {"type": "object", "required": ["x", "y"], "properties": {"x": {"type": "integer"}, "y": {"type": "integer"}}}
        },
        {
            "name": "skip_time",
            "description": "Move the clock forward (0-24 hours).",
            "inputSchema": {"type": "object", "required": ["hours"], "properties": {"hours": {"type": "number"}}}
        },
        {
            "name": "buildings",
            "description": "List buildings: index, template, bounds, storeys and ground-floor rooms. Marks the one the player wakes up in.",
            "inputSchema": {"type": "object", "properties": {}}
        },
        {
            "name": "regions",
            "description": "Population by 64 m region: ASCII heat maps of living zombies per region (all tiers) and of each region's \
                target (from the density in population.ron), the pull of attractors (loud events drawing the far horde in) on each region \
                with players marked, and the zone with the most open ground; then the attractors (where, reach, how far the pull has \
                spread, strength, time left) and the regions furthest below and above their targets.",
            "inputSchema": {"type": "object", "properties": {}}
        },
        {
            "name": "save",
            "description": "Save the whole world (map edits, players, every zombie on every tier, the clock, the RNGs) to a named save \
                in the saves folder, or with no name to this session's --save file. Loading it gives back exactly this state: the same \
                state hash, and the same future for the same calls.",
            "inputSchema": {"type": "object", "properties": {"name": {"type": "string", "description": "Save name: letters, digits, - _ . (no folders)"}}}
        },
        {
            "name": "load",
            "description": "Replace the world with a save from the saves folder (see saves), or with no name the session's --save file. \
                Fails, leaving the world as it is, if the file is damaged, holds impossible values, or is from another save format. \
                Game data changed since the save was made: compatible changes (weapon numbers, weapons added, population.ron) load \
                with a note; a building template that changes the map, or a removed weapon the save uses, fails unless force is true, \
                which loads it anyway, dropping what no longer fits, and says what it changed.",
            "inputSchema": {"type": "object", "properties": {
                "name": {"type": "string"},
                "force": {"type": "boolean", "description": "Load anyway when the game data changed in a way the save can't simply take. Default false"}
            }}
        },
        {
            "name": "saves",
            "description": "List the saves in the saves folder: size, age, tick, time of day, seed, map size, zombies and players; and \
                where and how often this session autosaves.",
            "inputSchema": {"type": "object", "properties": {}}
        },
        {
            "name": "screenshot",
            "description": "A rendered picture (PNG) of the world around a player, as the game client draws it: 3D isometric view, \
                buildings with the walls and roofs between the camera and the player cut away, zombies, items, day/night lighting, \
                and fog of war from that player's view (what they can't see is darkened, and zombies there aren't drawn). No HUD. \
                Takes a second or two (the server runs the game client offscreen on a copy of the world; no window opens) and \
                changes nothing: no time passes. Use observe for exact positions and states; use this to check how things look.",
            "inputSchema": {"type": "object", "properties": {
                "player": {"type": "integer", "description": "Whose view (fog of war, cutaways), and the centre unless x/y are given. Default 1"},
                "x": {"type": "number", "description": "Centre on this point instead (metres; give y too). Fog is still the player's: zombies far from players aren't drawn"},
                "y": {"type": "number"},
                "zoom": {"type": "number", "description": "Metres of world visible top to bottom, 8 to 90. Default 26 (the game's default)"},
                "angle": {"type": "integer", "description": "Camera angle, one of the four fixed ones: 0 (default, looking north-west), 1, 2, 3 (each a quarter turn)"},
                "width": {"type": "integer", "description": "Pixels, 64 to 1920. Default 1280"},
                "height": {"type": "integer", "description": "Pixels, 64 to 1080. Default 720"}
            }}
        },
        {
            "name": "stats",
            "description": "Map size, tick, time of day and daylight, players (HP, weapon, dead), zombie counts by state (corpses too) and by simulation tier (active / near / virtual, with the ranges and last tick's promotions and demotions), weapons on the floor, last tick timing (tier pass, senses, flow fields, active AI, near and virtual updates), sounds heard and the state hash (same seed + same calls = same hash).",
            "inputSchema": {"type": "object", "properties": {}}
        }
    ])
}

/// `save`, `load` and `saves`: named saves in the saves folder (`--saves`, or the `--save` file's).
fn save_tool(world: &mut World, persist: &mut Persist, name: &str, args: &Value) -> Result<String, String> {
    let named = args.get("name").and_then(Value::as_str);
    match name {
        "saves" => {
            let list = save_file::list(&persist.dir).map_err(|e| format!("can't read {}: {e}", persist.dir.display()))?;
            let mut out = format!("saves in {}: {}\n", persist.dir.display(), list.len());
            for e in &list {
                let ago = e.modified.and_then(|m| std::time::SystemTime::now().duration_since(m).ok()).map_or("?".to_string(), |d| format!("{} min ago", d.as_secs() / 60));
                out += &match &e.info {
                    Ok(i) => format!(
                        "  {}: {}, saved {ago}; tick {} ({}), seed {}, map {}x{} m, {} zombies ({} alive), {} players, game {}\n",
                        e.name,
                        save_file::size(e.bytes),
                        i.tick,
                        clock(i.time_of_day),
                        i.seed,
                        i.map_size.x,
                        i.map_size.y,
                        i.zombies,
                        i.alive,
                        i.players,
                        i.game_version
                    ),
                    Err(err) => format!("  {}: {}, saved {ago}; unreadable: {err}\n", e.name, save_file::size(e.bytes)),
                };
            }
            out += &match &persist.path {
                Some(p) => format!(
                    "this session saves to {} ({}; and on shutdown)",
                    p.display(),
                    persist.autosave.map_or("no autosave".to_string(), |d| format!("autosave every {} min", d.as_secs() / 60))
                ),
                None => "this session has no --save file: nothing is saved unless you call save".to_string(),
            };
            Ok(out)
        }
        "save" => {
            let path = match named {
                Some(n) => save_file::path_for(&persist.dir, n)?,
                None => persist.path.clone().ok_or("give a name: this session has no --save file")?,
            };
            let saved = save_file::save(world, &path)?;
            if persist.path.as_ref() == Some(&path) {
                persist.dirty = false;
                persist.last_save = Instant::now();
            }
            Ok(format!("saved tick {} to {saved}; hash {:016x}", world.tick, world.state_hash()))
        }
        _ => {
            let path = match named {
                Some(n) => save_file::path_for(&persist.dir, n)?,
                None => persist.path.clone().ok_or("give the name of a save (see saves)")?,
            };
            let force = args.get("force").and_then(Value::as_bool).unwrap_or(false);
            let (loaded, ms) = save_file::load(&path, &persist.content, LoadOptions { force }).map_err(|e| e.hint("load with force: true"))?;
            let mut loaded_world = loaded.world;
            add_agent_player(&mut loaded_world);
            *world = loaded_world;
            // Loading another file than the session's own: the session's file is now behind.
            persist.dirty = persist.path.as_ref() != Some(&path);
            let notes: String = loaded.notes.iter().map(|n| format!("\nnote: {n}")).collect();
            Ok(format!(
                "loaded {} in {ms:.0} ms: tick {}, {}; {}; {} players; hash {:016x}{notes}",
                path.display(),
                world.tick,
                clock(world.time_of_day()),
                counts(world),
                world.players.len(),
                world.state_hash()
            ))
        }
    }
}

/// `screenshot`: the PNG and a line saying what it shows. Only reads the world.
fn screenshot_tool(world: &World, renderer: &Renderer, args: &Value) -> Result<Value, String> {
    let req = screenshot::Request::from_args(args)?;
    let Some(p) = world.player(req.player) else {
        let ids: Vec<String> = world.players.iter().map(|p| p.id.to_string()).collect();
        return Err(format!("no player {}: the players are [{}]", req.player, ids.join(", ")));
    };
    let pos = p.state.pos;
    let (png, ms) = screenshot::render(world, renderer, &req)?;
    let centre = match req.center {
        Some([x, y]) => format!(", centred on ({x:.1}, {y:.1}) {}", place(world, Vec2::new(x, y))),
        None => String::new(),
    };
    let text = format!(
        "screenshot {}x{} of player {}'s view: player at ({:.1}, {:.1}) {}{}{centre}; tick {}, {} (daylight {:.0}%); zoom {} m, angle {}; PNG {} KB, rendered in {ms:.0} ms",
        req.size[0],
        req.size[1],
        req.player,
        pos.x,
        pos.y,
        place(world, pos),
        if p.dead { " (dead)" } else { "" },
        world.tick,
        clock(world.time_of_day()),
        world.daylight() * 100.0,
        req.zoom,
        req.angle,
        png.len().div_ceil(1024),
    );
    Ok(screenshot::content(&png, &text))
}

fn call_tool(world: &mut World, name: &str, args: &Value) -> Result<String, String> {
    let num = |k: &str| args.get(k).and_then(Value::as_f64);
    let point = || match (num("x"), num("y")) {
        (Some(x), Some(y)) => Some([x as f32, y as f32]),
        _ => None,
    };
    let player_id = || args.get("player").and_then(Value::as_u64).unwrap_or(1) as u32;
    match name {
        "observe" => {
            let centre = point().map(Vec2::from).or(world.players.first().map(|p| p.state.pos)).unwrap_or(world.player_spawn);
            let radius = args.get("radius").and_then(Value::as_i64).unwrap_or(10).clamp(2, 30) as i32;
            Ok(observe(world, centre, radius))
        }
        "step_ticks" => {
            let n = args.get("n").and_then(Value::as_u64).unwrap_or(20).clamp(1, MAX_STEP);
            let start = Instant::now();
            let mut attacks = Events::default();
            for _ in 0..n {
                world.step();
                attacks.add(world);
            }
            world.edge_changes.clear();
            world.combat_events.clear();
            let ms = start.elapsed().as_secs_f32() * 1000.0;
            Ok(format!(
                "advanced {n} ticks ({:.1} s game time) in {ms:.1} ms; now tick {}, {}; {}{}; hash {:016x}",
                n as f32 * TICK_DT,
                world.tick,
                clock(world.time_of_day()),
                counts(world),
                attacks.summary(),
                world.state_hash()
            ))
        }
        "move_player" => {
            let id = player_id();
            let start = world.player(id).ok_or_else(|| format!("no player {id}"))?.state;
            let dir = match args.get("direction").and_then(Value::as_str) {
                Some("stop") => Vec2::ZERO,
                Some(d) => COMPASS.iter().find(|(n, _)| *n == d).map(|(_, v)| Vec2::from(*v).normalize()).ok_or_else(|| format!("unknown direction {d}"))?,
                None => Vec2::new(num("dx").unwrap_or(0.0) as f32, num("dy").unwrap_or(0.0) as f32).clamp_length_max(1.0),
            };
            let facing = num("facing").map(|f| f as f32).unwrap_or(if dir == Vec2::ZERO { start.facing } else { dir.y.atan2(dir.x) });
            let (run, sneak) = (args.get("run").and_then(Value::as_bool).unwrap_or(false), args.get("sneak").and_then(Value::as_bool).unwrap_or(false));
            let ticks = args.get("ticks").and_then(Value::as_u64).unwrap_or(20).clamp(1, MAX_MOVE_TICKS);
            let input = |world: &World, move_dir: Vec2| Intent::PlayerInput {
                player: id,
                seq: world.player(id).map_or(1, |p| p.last_input_seq + 1),
                move_dir: move_dir.to_array(),
                facing,
                run,
                sneak,
            };
            let mut attacks = Events::default();
            for _ in 0..ticks {
                let intent = input(world, dir);
                apply_intent(world, &intent);
                world.step();
                attacks.add(world);
            }
            // Stop, so later step_ticks calls don't keep walking.
            let intent = input(world, Vec2::ZERO);
            apply_intent(world, &intent);
            world.edge_changes.clear();
            world.combat_events.clear();
            let p = world.player(id).expect("player exists").state;
            Ok(format!(
                "player {id} moved from ({:.1}, {:.1}) to ({:.1}, {:.1}), {:.1} m in {ticks} ticks; {:?}{}, endurance {:.2}, facing {}; \
                 in {}; now tick {}, {}; {}{}; hash {:016x}",
                start.pos.x,
                start.pos.y,
                p.pos.x,
                p.pos.y,
                p.pos.distance(start.pos),
                p.stance,
                if world.map.in_window_opening(p.pos) { " (climbing through a window)" } else { "" },
                p.endurance,
                compass(p.facing),
                place(world, p.pos),
                world.tick,
                clock(world.time_of_day()),
                counts(world),
                attacks.summary(),
                world.state_hash()
            ))
        }
        "set_goal" => {
            apply_intent(world, &Intent::SetGoal { pos: point() });
            Ok(match world.goal() {
                Some(g) => format!("goal set to tile ({}, {})", g.x, g.y),
                None => "goal cleared; zombies chase the players again".into(),
            })
        }
        "spawn_zombies" => {
            let pos = point().ok_or("x and y are required")?;
            let count = args.get("count").and_then(Value::as_u64).ok_or("count is required")?.min(20000) as u32;
            let before = world.zombies.len();
            apply_intent(world, &Intent::SpawnZombies { count, center: pos, radius: num("radius").unwrap_or(10.0) as f32 });
            Ok(format!("spawned {} zombies; total {}", world.zombies.len() - before, world.zombies.len()))
        }
        // `toggle_door` is the old name of `interact`.
        "interact" | "toggle_door" => {
            let id = player_id();
            world.player(id).ok_or_else(|| format!("no player {id}"))?;
            let before = world.edge_changes.len();
            apply_intent(world, &Intent::Interact { player: id });
            match world.edge_changes.get(before) {
                Some(c) => Ok(format!("{} at tile ({}, {}) {:?} edge is now {}", noun(c.kind), c.tile.x, c.tile.y, c.edge, edge_name(c.kind))),
                None => Err(format!("no door or window that opens or shuts within reach of player {id}")),
            }
        }
        "smash_window" => {
            let id = player_id();
            world.player(id).ok_or_else(|| format!("no player {id}"))?;
            let before = world.edge_changes.len();
            apply_intent(world, &Intent::SmashWindow { player: id });
            match world.edge_changes.get(before) {
                Some(c) => Ok(format!(
                    "smashed the window at tile ({}, {}) {:?} edge (heard {} m away)",
                    c.tile.x,
                    c.tile.y,
                    c.edge,
                    world.config.zombie.glass_break_noise
                )),
                None => Err(format!("no window within reach of player {id}")),
            }
        }
        "attack" | "shove" | "stomp" => {
            let id = player_id();
            let p = world.player(id).ok_or_else(|| format!("no player {id}"))?;
            if p.dead {
                return Err(format!("player {id} is dead (respawn to play on)"));
            }
            let (pos, seq, sneak, weapon) = (p.state.pos, p.last_input_seq, p.input.sneak, p.weapon);
            let facing = match args.get("target").and_then(Value::as_u64) {
                Some(t) => {
                    let i = world.zombies.index_of(t as u32).ok_or_else(|| format!("no zombie {t}"))?;
                    let to = world.zombies.pos[i] - pos;
                    Some(to.y.atan2(to.x))
                }
                None => num("facing").map(|f| f as f32),
            };
            if let Some(facing) = facing {
                apply_intent(world, &Intent::PlayerInput { player: id, seq: seq + 1, move_dir: [0.0, 0.0], facing, run: false, sneak });
            }
            let (intent, verb) = match name {
                "attack" => (Intent::Attack { player: id }, format!("swung the {}", world.wielded(weapon).name.to_lowercase())),
                "shove" => (Intent::Shove { player: id }, "shoved".to_string()),
                _ => (Intent::Stomp { player: id }, "stomped".to_string()),
            };
            apply_intent(world, &intent);
            let p = world.player(id).expect("player exists");
            if p.action.is_none() && p.queued.is_none() {
                return Err(format!("player {id} is busy: it can act again in {:.2} s", p.ready_in));
            }
            // Step until it lands and the player can act again, so the next call isn't refused.
            let mut events = Events::default();
            let (mut ticks, mut landed_at, mut hits) = (0, None, 0);
            loop {
                world.step();
                events.add(world);
                ticks += 1;
                if let Some(a) = world.action_events.iter().find(|a| a.player == id) {
                    hits = a.hits;
                    landed_at = Some(ticks);
                }
                let p = world.player(id).expect("player exists");
                if ticks >= 200 || p.dead || (landed_at.is_some() && p.action.is_none() && p.ready_in <= 0.0) {
                    break;
                }
            }
            world.edge_changes.clear();
            world.combat_events.clear();
            let p = world.player(id).expect("player exists");
            Ok(format!(
                "player {id} {verb} facing {}: landed after {} ticks, {}; ready again after {ticks} ticks; {:.0} HP, endurance {:.2}{}; now tick {}{}; hash {:016x}",
                compass(p.state.facing),
                landed_at.map_or("never".to_string(), |t| t.to_string()),
                if hits == 0 { "nothing hit".to_string() } else { format!("{hits} hit") },
                p.hp,
                p.state.endurance,
                if p.state.winded { " (winded)" } else { "" },
                world.tick,
                events.summary(),
                world.state_hash()
            ))
        }
        "pick_up" => {
            let id = player_id();
            let before = world.player(id).ok_or_else(|| format!("no player {id}"))?.weapon;
            let floor = world.items().to_vec();
            apply_intent(world, &Intent::PickUp { player: id });
            if world.items() == floor.as_slice() {
                return Err(format!("no weapon within reach of player {id} (1.3 m; the one nearest to where it faces wins)"));
            }
            let def = world.wielded(world.player(id).and_then(|p| p.weapon));
            let dropped = before.map(|b| format!(", left the {} on the floor", world.config.weapons[b as usize].name.to_lowercase())).unwrap_or_default();
            Ok(format!("player {id} picked up the {}{dropped} ({})", def.name.to_lowercase(), weapon_line(def)))
        }
        "weapons" => {
            let mut out = String::from("id: name, category | damage | reach m | arc deg | windup/swing s | targets | knockback m | stagger/knockdown/crit (x mult) | endurance | noise m\n");
            for w in &world.config.weapons {
                out.push_str(&format!("{}: {}, {:?} | {}\n", w.id, w.name, w.category, weapon_line(w)));
            }
            let c = &world.config.combat;
            out.push_str(&format!(
                "shove: reach {:.2} m, arc {:.0} deg, up to {} zombies, knockback {:.2} m, always staggers, knockdown {:.0}% (x{} if staggered, x{} from behind), {:.1} s, endurance {:.3}\n\
                 stomp: downed zombie within {:.1} m centre to centre, damage {:.0}-{:.0}, {:.1} s, endurance {:.3}\n\
                 zombie hp {:.0}-{:.0}; from behind +{:.0}% crit chance; endurance below {:.0}% weakens (down to x{:.2} damage) and slows (up to x{:.1}) actions\n\
                 zombie strikes land {:.0}% (+{:.0}% per extra zombie on you, max {:.0}%) for {:.0}-{:.0} of your {:.0} HP",
                c.shove_range, c.shove_arc_degrees, c.shove_targets, c.shove_knockback, c.shove_knockdown_chance * 100.0, c.staggered_knockdown_mult, c.behind_knockdown_mult, c.shove_seconds, c.shove_endurance,
                c.stomp_range, c.stomp_damage.0, c.stomp_damage.1, c.stomp_seconds, c.stomp_endurance,
                c.zombie_hp.0, c.zombie_hp.1, c.behind_crit_bonus * 100.0, c.tired_below * 100.0, c.tired_power, c.tired_slow,
                c.zombie_hit_chance * 100.0, c.zombie_crowd_bonus * 100.0, c.zombie_hit_max * 100.0, c.zombie_damage.0, c.zombie_damage.1, c.player_hp
            ));
            Ok(out)
        }
        "spawn_item" => {
            let pos = point().ok_or("x and y are required")?;
            let weapon = args.get("weapon").and_then(Value::as_str).ok_or("weapon is required")?.to_string();
            let before = world.items().len();
            apply_intent(world, &Intent::SpawnItem { weapon: weapon.clone(), pos });
            if world.items().len() == before {
                return Err(format!("can't place {weapon:?} there (unknown weapon id, bare hands, or off the map); see weapons"));
            }
            let it = world.items().last().expect("just placed");
            Ok(format!("item #{} ({}) on the floor at ({:.1}, {:.1})", it.id, world.config.weapons[it.weapon as usize].name, pos[0], pos[1]))
        }
        "respawn" => {
            let id = player_id();
            world.player(id).ok_or_else(|| format!("no player {id}"))?;
            apply_intent(world, &Intent::Respawn { player: id });
            let p = world.player(id).expect("player exists");
            if p.dead {
                return Err(format!("player {id} is dead and couldn't respawn"));
            }
            Ok(format!("player {id} is at ({:.1}, {:.1}) in {} with {:.0} HP", p.state.pos.x, p.state.pos.y, place(world, p.state.pos), p.hp))
        }
        "edge" => {
            let (x, y) = (num("x").ok_or("x is required")?, num("y").ok_or("y is required")?);
            let t = IVec2::new(x.floor() as i32, y.floor() as i32);
            if !world.map.in_bounds(t) {
                return Err(format!("tile ({}, {}) is off the map", t.x, t.y));
            }
            let mut out = format!("tile ({}, {}) in {}:\n", t.x, t.y, place(world, t.as_vec2() + Vec2::splat(0.5)));
            for side in Side::ALL {
                let e = EdgeRef::of(t, side);
                let kind = world.map.owned_edge(e.tile, e.edge);
                out.push_str(&format!("  {side:?}: {}{}\n", edge_name(kind), hp_text(world, e)));
            }
            Ok(out)
        }
        "make_noise" => {
            let pos = point().ok_or("x and y are required")?;
            let radius = num("radius").ok_or("radius is required")? as f32;
            let lasting = args.get("lasting").and_then(Value::as_bool).unwrap_or(false);
            apply_intent(world, &Intent::MakeNoise { pos, radius, lasting });
            let m = &world.config.population.migration;
            let pull = if lasting {
                format!("; it leaves a lasting attractor ({:.0} min)", m.lasting_secs / 60.0)
            } else if m.enabled && radius >= m.min_radius {
                format!("; loud enough to leave an attractor ({:.0} min) once heard", m.life_secs / 60.0)
            } else {
                String::new()
            };
            Ok(format!("noise at ({:.1}, {:.1}) with radius {:.1} m; zombies hear it on the next tick{pull}", pos[0], pos[1], radius.clamp(0.0, 500.0)))
        }
        "regions" => Ok(regions(world)),
        "zombie" => {
            let id = args.get("id").and_then(Value::as_u64).ok_or("id is required")? as u32;
            let i = world.zombies.index_of(id).ok_or_else(|| format!("no zombie {id}"))?;
            Ok(zombie_detail(world, i))
        }
        "skip_time" => {
            let hours = num("hours").ok_or("hours is required")? as f32;
            apply_intent(world, &Intent::SkipTime { hours });
            Ok(format!("it is now {}", clock(world.time_of_day())))
        }
        "buildings" => {
            let mut out = String::new();
            for (i, b) in world.map.buildings.iter().enumerate() {
                let mut rooms: Vec<&str> = world
                    .map
                    .rooms
                    .iter()
                    .skip(1)
                    .filter(|r| r.building as usize == i)
                    .map(|r| r.name.as_str())
                    .collect();
                rooms.dedup();
                out.push_str(&format!(
                    "#{i} {} at ({}, {}) size {}x{}, {} storey(s): {}{}\n",
                    b.template,
                    b.min.x,
                    b.min.y,
                    b.size.x,
                    b.size.y,
                    b.levels,
                    rooms.join(", "),
                    if i == world.spawn_building { "  <- player wakes up here" } else { "" }
                ));
            }
            Ok(out)
        }
        "stats" => Ok(json!({
            "map": [world.map.width(), world.map.height()],
            "seed": world.config.seed(),
            "buildings": world.map.buildings.len(),
            "player_spawn": world.player_spawn.to_array(),
            "players": world.players.iter().map(|p| json!({
                "id": p.id,
                "pos": p.state.pos.to_array(),
                "facing": p.state.facing,
                "stance": format!("{:?}", p.state.stance),
                "endurance": p.state.endurance,
                "winded": p.state.winded,
                "last_input_seq": p.last_input_seq,
                "hp": p.hp,
                "dead": p.dead,
                "weapon": world.wielded(p.weapon).id,
                "action": p.action.map(|a| format!("{:?}", a.kind)),
                "ready_in": p.ready_in,
            })).collect::<Vec<_>>(),
            "items_on_floor": world.items().len(),
            "tick": world.tick,
            "time_of_day": clock(world.time_of_day()),
            "daylight": world.daylight(),
            "zombies": world.zombies.len(),
            "zombie_tiers": {
                "active": world.stats.active,
                "near": world.stats.near,
                "virtual": world.stats.virtual_,
                "promoted_last_tick": world.stats.promoted,
                "demoted_last_tick": world.stats.demoted,
                "near_updated_last_tick": world.stats.near_updated,
                "virtual_moved_last_tick": world.stats.virtual_updated,
                "ranges_m": {
                    "active": world.config.tiers.active_range,
                    "near": world.config.tiers.near_range,
                    "hysteresis": world.config.tiers.hysteresis,
                    "region": world.regions().size(),
                },
            },
            "population": {
                "target": world.population().total_target().round(),
                "alive": world.zombies.state.iter().filter(|s| **s != ZombieState::Dead).count(),
                "attractors": world.population().attractors.iter().map(|a| json!({
                    "id": a.id,
                    "pos": a.pos.to_array(),
                    "reach_m": a.reach,
                    "felt_m": a.felt_radius(world.tick, world.config.population.migration.spread_speed),
                    "strength": a.strength(world.tick),
                })).collect::<Vec<_>>(),
                "migration": world.config.population.migration.enabled,
                "respawn": world.config.population.respawn.enabled,
                "respawned": world.population().respawned,
            },
            "zombie_states": {
                "wander": state_count(world, ZombieState::Wander),
                "investigate": state_count(world, ZombieState::Investigate),
                "chase": state_count(world, ZombieState::Chase),
                "lunge": state_count(world, ZombieState::Lunge),
                "attack": state_count(world, ZombieState::Attack),
                "thump": state_count(world, ZombieState::Thump),
                "staggered": state_count(world, ZombieState::Staggered),
                "down": state_count(world, ZombieState::Down),
                "dead": state_count(world, ZombieState::Dead),
            },
            "last_tick_hits": world.hit_events.len(),
            "last_tick_thumps": world.stats.thumps,
            "damaged_edges": world.damaged_edges().count(),
            "chasing": world.stats.chasing,
            "goal": world.goal().map(|g| [g.x, g.y]),
            "last_tick_ms": world.stats.tick_ms,
            "last_tick_tier_ms": world.stats.tier_ms,
            "last_tick_senses_ms": world.stats.senses_ms,
            "last_tick_flow_ms": world.stats.flow_ms,
            "last_tick_active_ms": world.stats.active_ms,
            "last_tick_near_ms": world.stats.near_ms,
            "last_tick_virtual_ms": world.stats.virtual_ms,
            "last_tick_flow_builds": world.stats.flow_builds,
            "flow_fields_cached": world.flow_cache_len(),
            "last_tick_sight_checks": world.stats.sight_checks,
            "last_tick_sounds": world.sounds_heard().iter().map(|s| json!({
                "pos": [s.pos.x, s.pos.y], "radius": s.radius, "source": format!("{:?}", s.source)
            })).collect::<Vec<_>>(),
            "state_hash": format!("{:016x}", world.state_hash()),
        })
        .to_string()),
        _ => Err(format!("unknown tool {name}")),
    }
}

/// Zombie attacks on players, thumps on doors and windows, and breakages, accumulated over several ticks.
#[derive(Default)]
struct Events {
    total: u32,
    /// Per player: strikes, strikes that landed, damage taken.
    per_player: Vec<(u32, u32, u32, f32)>,
    thumps: u32,
    broken: Vec<String>,
    /// Player hits on zombies (the first few spelled out), and kills.
    hits: u32,
    hit_lines: Vec<String>,
    kills: u32,
    deaths: Vec<u32>,
}

impl Events {
    fn add(&mut self, world: &World) {
        for a in &world.attack_events {
            self.total += 1;
            let i = match self.per_player.iter().position(|e| e.0 == a.player) {
                Some(i) => i,
                None => {
                    self.per_player.push((a.player, 0, 0, 0.0));
                    self.per_player.len() - 1
                }
            };
            let e = &mut self.per_player[i];
            e.1 += 1;
            if a.landed {
                e.2 += 1;
                e.3 += a.damage;
            }
            if a.landed && world.player(a.player).is_some_and(|p| p.dead) && !self.deaths.contains(&a.player) {
                self.deaths.push(a.player);
            }
        }
        for h in &world.hit_events {
            self.hits += 1;
            if h.effect == HitEffect::Killed {
                self.kills += 1;
            }
            if self.hit_lines.len() < 12 {
                let how = match h.kind {
                    ActionKind::Swing => "hit",
                    ActionKind::Shove => "shoved",
                    ActionKind::Stomp => "stomped",
                };
                let effect = match h.effect {
                    HitEffect::Hit => "",
                    HitEffect::Staggered => ", staggered",
                    HitEffect::KnockedDown => ", KNOCKED DOWN",
                    HitEffect::Killed => ", KILLED",
                };
                let hp = world.zombies.index_of(h.zombie).map(|i| world.zombies.hp[i]).unwrap_or(0.0);
                self.hit_lines.push(format!(
                    "P{} {how} #{}{}{}{effect} ({hp:.0} HP left)",
                    h.player,
                    h.zombie,
                    if h.damage > 0.0 { format!(" for {:.0}", h.damage) } else { String::new() },
                    match (h.crit, h.behind) {
                        (true, true) => " (crit, from behind)",
                        (true, false) => " (crit)",
                        (false, true) => " (from behind)",
                        _ => "",
                    },
                ));
            }
        }
        self.thumps += world.stats.thumps;
        for c in &world.break_events {
            self.broken.push(format!("{} at ({}, {}) {:?}", noun(c.kind), c.tile.x, c.tile.y, c.edge));
        }
    }

    fn summary(&self) -> String {
        let mut out = String::new();
        if self.hits > 0 {
            let more = if self.hits as usize > self.hit_lines.len() { format!(" and {} more", self.hits as usize - self.hit_lines.len()) } else { String::new() };
            out.push_str(&format!("; HITS: {}{more}", self.hit_lines.join(", ")));
        }
        if self.kills > 0 {
            out.push_str(&format!("; {} zombie(s) killed", self.kills));
        }
        if self.thumps > 0 {
            out.push_str(&format!("; {} thumps on doors/windows", self.thumps));
        }
        if !self.broken.is_empty() {
            out.push_str(&format!("; BROKEN: {}", self.broken.join(", ")));
        }
        if self.total > 0 {
            let list: Vec<String> = self.per_player.iter().map(|(p, n, l, d)| format!("P{p} x{n}, {l} landed for {d:.0} HP")).collect();
            out.push_str(&format!("; ZOMBIE ATTACKS: {} ({})", self.total, list.join("; ")));
        }
        for p in &self.deaths {
            out.push_str(&format!("; P{p} DIED"));
        }
        out
    }
}

/// One weapon's numbers, for tool replies.
fn weapon_line(w: &zombye_sim::WeaponDef) -> String {
    format!(
        "{:.0}-{:.0} dmg | {:.2} m | {:.0} deg | {:.2}/{:.2} s | {} | {:.2} m | {:.0}/{:.0}/{:.0}% (x{:.1}) | {:.3} | {:.1} m",
        w.damage.0,
        w.damage.1,
        w.range,
        w.arc_degrees,
        w.windup_seconds,
        w.swing_seconds,
        w.max_targets,
        w.knockback,
        w.stagger_chance * 100.0,
        w.knockdown_chance * 100.0,
        w.crit_chance * 100.0,
        w.crit_multiplier,
        w.endurance_cost,
        w.noise_radius
    )
}

fn noun(kind: Edge) -> &'static str {
    if kind.is_window() { "window" } else { "door" }
}

fn edge_name(kind: Edge) -> &'static str {
    match kind {
        Edge::None => "open ground",
        Edge::Wall => "wall",
        Edge::Window => "window (shut)",
        Edge::WindowOpen => "window (open)",
        Edge::WindowBroken => "window (broken)",
        Edge::DoorClosed => "door (shut)",
        Edge::DoorOpen => "door (open)",
        Edge::DoorBroken => "door (broken)",
        Edge::Arch => "archway",
        Edge::Fence => "fence",
    }
}

/// ", 23/40 HP" for a door or window that isn't broken.
fn hp_text(world: &World, e: EdgeRef) -> String {
    match world.edge_hp(e) {
        Some((hp, max)) => format!(", {hp:.0}/{max:.0} HP"),
        None => String::new(),
    }
}

/// Heat-map character for a zombie count.
fn heat(n: f32) -> char {
    match n.round() as u32 {
        0 => ' ',
        1..=4 => '.',
        5..=14 => ':',
        15..=39 => '-',
        40..=99 => '=',
        100..=249 => '+',
        250..=499 => '*',
        _ => '#',
    }
}

/// The `regions` tool: per-region population (actual vs target), attractor pull and zones.
fn regions(world: &World) -> String {
    let regions = world.regions();
    let pop = world.population();
    let m = &world.config.population.migration;
    let (cols, rows) = (regions.cols(), regions.rows());
    let mut actual = vec![0u32; regions.regions()];
    for i in 0..world.zombies.len() {
        if world.zombies.state[i] != ZombieState::Dead {
            actual[regions.region_of(world.zombies.pos[i])] += 1;
        }
    }
    let alive: u32 = actual.iter().sum();
    let r = &world.config.population.respawn;
    let mut out = format!(
        "{cols} x {rows} regions of {:.0} m (x east, y south). {alive} zombies alive, target {:.0}; {} attractor(s); migration {}; respawn {}{}.\n",
        regions.size(),
        pop.total_target(),
        pop.attractors.len(),
        if m.enabled { "on" } else { "off" },
        if r.enabled { "on" } else { "off" },
        if r.enabled { format!(" ({} added so far)", pop.respawned) } else { String::new() },
    );
    let w = cols as usize;
    out.push_str(&format!("{:<w$}  {:<w$}  {:<w$}  {:<w$}\n", "zombies", "target", "pull", "zone"));
    let player_regions: Vec<usize> = world.players.iter().map(|p| regions.region_of(p.state.pos)).collect();
    let attractor_regions: Vec<usize> = pop.attractors.iter().map(|a| regions.region_of(a.pos)).collect();
    for y in 0..rows {
        let mut line = String::new();
        let row = |f: &dyn Fn(usize) -> char| (0..cols).map(|x| f((y * cols + x) as usize)).collect::<String>();
        line.push_str(&row(&|k| heat(actual[k] as f32)));
        line.push_str("  ");
        line.push_str(&row(&|k| heat(pop.target[k])));
        line.push_str("  ");
        line.push_str(&row(&|k| {
            if player_regions.contains(&k) {
                return 'P';
            }
            if attractor_regions.contains(&k) {
                return '@';
            }
            let pull = pop.pull_on(regions, k, m, world.tick);
            if pull.strength <= 0.0 {
                return ' ';
            }
            let idx = pop.attractors.iter().position(|a| a.id == pull.attractor).unwrap_or(0) as u8 % 26;
            if pull.strength >= 0.3 { (b'A' + idx) as char } else { (b'a' + idx) as char }
        }));
        line.push_str("  ");
        line.push_str(&row(&|k| pop.zone[k].letter()));
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out.push_str(
        "counts: ' ' 0, . 1-4, : 5-14, - 15-39, = 40-99, + 100-249, * 250-499, # 500+. pull: the attractor pulling hardest \
         (a, b, ... in the list below; capital = strong, 0.3+), @ an attractor's own region, P a player. zones: C town core, \
         S street, Y yard, I indoors, R forest road, e forest edge, f deep forest.\n",
    );
    for (k, a) in pop.attractors.iter().enumerate() {
        out.push_str(&format!(
            "attractor {} (#{}) at ({:.0}, {:.0}): reach {:.0} m, pull spread out to {:.0} m, strength {:.2}, {:.0} s left\n",
            (b'a' + (k % 26) as u8) as char,
            a.id,
            a.pos.x,
            a.pos.y,
            a.reach,
            a.felt_radius(world.tick, m.spread_speed),
            a.strength(world.tick),
            (a.life_secs - a.age_secs(world.tick)).max(0.0),
        ));
    }
    let mut diff: Vec<(f32, usize)> = (0..actual.len()).map(|k| (actual[k] as f32 - pop.target[k], k)).collect();
    diff.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    let line = |k: usize| {
        let c = regions.corner(k);
        format!("  ({}, {}) at {:.0},{:.0}: {} vs target {:.0} ({})\n", k as i32 % cols, k as i32 / cols, c.x, c.y, actual[k], pop.target[k], pop.zone[k].name())
    };
    out.push_str("furthest below target:\n");
    for &(_, k) in diff.iter().take(5).filter(|(d, _)| *d < -0.5) {
        out.push_str(&line(k));
    }
    out.push_str("furthest above target:\n");
    for &(_, k) in diff.iter().rev().take(5).filter(|(d, _)| *d > 0.5) {
        out.push_str(&line(k));
    }
    out
}

fn state_count(world: &World, state: ZombieState) -> usize {
    world.zombies.state.iter().filter(|s| **s == state).count()
}

/// "chasing N, investigating M, battering K, ...; tiers A / N / V" for tool replies.
fn counts(world: &World) -> String {
    let s = &world.stats;
    format!(
        "chasing {}, investigating {}, battering {}, reeling {} (active zombies); tiers {} active / {} near / {} virtual",
        s.chasing, s.investigating, s.thumping, s.reeling, s.active, s.near, s.virtual_
    )
}

/// "active (12.3 m from the nearest player)" and the like, for `zombie`.
fn tier_text(world: &World, i: usize) -> String {
    let p = world.zombies.pos[i];
    let d = world.players.iter().map(|pl| pl.state.pos.distance(p)).fold(f32::INFINITY, f32::min);
    let tiers = &world.config.tiers;
    let what = match world.zombies.tier[i] {
        Tier::Active => "active (full AI every tick)".to_string(),
        Tier::Near => format!("near (no sight or combat; updated every {} ticks)", tiers.near_interval_ticks),
        Tier::Virtual => format!("virtual (moved in bulk with its {:.0} m region every {} ticks)", world.regions().size(), tiers.virtual_interval_ticks),
    };
    if d.is_finite() { format!("{what}, {d:.1} m from the nearest player") } else { what }
}

/// Map letter for a zombie's state.
fn state_char(state: ZombieState) -> char {
    match state {
        ZombieState::Wander => 'Z',
        ZombieState::Investigate => 'I',
        ZombieState::Chase => 'C',
        ZombieState::Lunge => 'L',
        ZombieState::Attack => 'A',
        ZombieState::Thump => 'B',
        ZombieState::Staggered => 'S',
        ZombieState::Down => 'v',
        ZombieState::Dead => '%',
    }
}

/// Short description of what zombie `i` is after.
fn zombie_goal(world: &World, i: usize) -> String {
    let z = &world.zombies;
    let t = z.target_pos[i];
    match z.state[i] {
        ZombieState::Wander => "wandering".into(),
        ZombieState::Dead => "dead".into(),
        ZombieState::Staggered | ZombieState::Down => format!(
            "{} for {:.1} s{}",
            if z.state[i] == ZombieState::Down { "down on the ground" } else { "staggered" },
            z.timer[i],
            if z.target_player[i] != 0 { format!(", then after P{}", z.target_player[i]) } else { String::new() }
        ),
        ZombieState::Investigate => format!("investigating ({:.1}, {:.1})", t.x, t.y),
        ZombieState::Thump => {
            let e = z.thump_edge[i];
            let after = if z.target_player[i] != 0 { format!("to get at P{}", z.target_player[i]) } else { format!("to get to ({:.1}, {:.1})", t.x, t.y) };
            let kind = world.map.owned_edge(e.tile, e.edge);
            format!("battering the {} at ({}, {}) {:?}{} {after}", noun(kind), e.tile.x, e.tile.y, e.edge, hp_text(world, e))
        }
        s => format!(
            "{} P{}{}",
            format!("{s:?}").to_lowercase(),
            z.target_player[i],
            if z.sees[i] > 0 { String::new() } else { format!(", lost sight, last seen at ({:.1}, {:.1})", t.x, t.y) }
        ),
    }
}

fn zombie_detail(world: &World, i: usize) -> String {
    let z = &world.zombies;
    let t = &world.config.zombie;
    let p = z.pos[i];
    let timer = match z.state[i] {
        ZombieState::Chase => format!("memory {:.1} s left", z.timer[i]),
        ZombieState::Investigate => format!("interest {:.1} s left", z.timer[i]),
        ZombieState::Lunge => format!("lunge {:.2} s left", z.timer[i]),
        ZombieState::Thump => format!("{} {:.1} s left (paused while battering)", if z.target_player[i] != 0 { "memory" } else { "interest" }, z.timer[i]),
        ZombieState::Staggered => format!("staggered {:.2} s left", z.timer[i]),
        ZombieState::Down => format!("gets up in {:.2} s", z.timer[i]),
        _ => "no timer".into(),
    };
    let mut out = format!(
        "zombie #{} at ({:.1}, {:.1}) in {}, {:.0} HP, facing {}, top speed {:.2} m/s, moving {:.2} m/s, anim {:?}\n\
         tier {}\n\
         state {:?}: {}\n\
         target point ({:.1}, {:.1}), {:.1} m away; {timer}; cooldown {:.2} s{}{}\n",
        z.id[i],
        p.x,
        p.y,
        place(world, p),
        z.hp[i],
        compass(z.facing[i]),
        z.speed[i],
        z.vel[i].length(),
        z.anim[i],
        tier_text(world, i),
        z.state[i],
        zombie_goal(world, i),
        z.target_pos[i].x,
        z.target_pos[i].y,
        z.target_pos[i].distance(p),
        z.cooldown[i],
        if z.state[i].is_hunting() { format!("; next groan {:.1} s", z.groan_timer[i]) } else { String::new() },
        match z.state[i] {
            _ if z.tier[i] != Tier::Active => "",
            ZombieState::Investigate | ZombieState::Chase if z.direct[i] => "; straight path to target",
            ZombieState::Investigate | ZombieState::Chase => "; no straight path (uses a flow field if one is cached)",
            _ => "",
        },
    );
    let (cone, daylight) = (Cone::new(t), world.daylight());
    for pl in &world.players {
        let to = pl.state.pos - p;
        let d = to.length();
        let target = Target { id: pl.id, pos: pl.state.pos, stance: pl.state.stance, moving: pl.input.move_dir.length_squared() > 1e-4 };
        let range = senses::sight_range(t, cone, daylight, &target, z.facing[i], to.normalize_or_zero());
        out.push_str(&format!(
            "  P{}: {:.1} m {}, line of sight {}, could be seen out to {:.1} m from here ({:?}{})\n",
            pl.id,
            d,
            compass(to.y.atan2(to.x)),
            if line_of_sight(&world.map, p, pl.state.pos) { "yes" } else { "no" },
            range,
            pl.state.stance,
            if target.moving { ", moving" } else { ", still" }
        ));
    }
    out
}

/// Nearest compass direction of a facing angle (0 = east, +y = south).
fn compass(angle: f32) -> &'static str {
    let i = (angle / std::f32::consts::FRAC_PI_4).round().rem_euclid(8.0) as usize;
    COMPASS[i].0
}

fn place(world: &World, p: Vec2) -> String {
    let room = world.map.room_at(p);
    if room == 0 { "outdoors".to_string() } else { world.map.rooms[room as usize].name.clone() }
}

fn clock(hours: f32) -> String {
    let h = hours.floor() as u32;
    let m = ((hours - h as f32) * 60.0).floor() as u32;
    format!("{h:02}:{m:02}")
}

/// Double-resolution ASCII map: tile (i, j) is at (2i+1, 2j+1), its north edge at (2i+1, 2j) and
/// its west edge at (2i, 2j+1).
fn observe(world: &World, centre: Vec2, radius: i32) -> String {
    let map = &world.map;
    let c = centre.floor().as_ivec2();
    let n = (radius * 2 + 1) as usize;
    let size = 2 * n + 1;
    let mut grid = vec![vec![' '; size]; size];
    let origin = c - IVec2::splat(radius);
    for j in 0..=n as i32 {
        for i in 0..=n as i32 {
            let t = origin + IVec2::new(i, j);
            let tile = map.tile(t);
            let (gx, gy) = (2 * i as usize, 2 * j as usize);
            if (i as usize) < n && (j as usize) < n {
                grid[gy + 1][gx + 1] = match tile.feature {
                    Feature::Tree | Feature::PineTree => 'T',
                    Feature::Bush => '*',
                    Feature::Furniture => 'f',
                    Feature::None if tile.room != 0 => '_',
                    Feature::None => match tile.floor {
                        Floor::Asphalt | Floor::RoadLine => ',',
                        Floor::Sidewalk | Floor::Path | Floor::Concrete => ':',
                        _ => '.',
                    },
                };
            }
            let damaged = |which| world.edge_hp(EdgeRef::new(t, which)).is_some_and(|(hp, max)| hp < max);
            if (i as usize) < n {
                grid[gy][gx + 1] = edge_char(tile.north, true, damaged(OwnedEdge::North));
            }
            if (j as usize) < n {
                grid[gy + 1][gx] = edge_char(tile.west, false, damaged(OwnedEdge::West));
            }
            let corner = tile.north != Edge::None || tile.west != Edge::None || map.tile(t - IVec2::X).north != Edge::None || map.tile(t - IVec2::Y).west != Edge::None;
            grid[gy][gx] = if corner { '+' } else { ' ' };
        }
    }
    let to_cell = |p: Vec2| {
        let d = p.floor().as_ivec2() - origin;
        (d.x >= 0 && d.y >= 0 && d.x < n as i32 && d.y < n as i32).then(|| (2 * d.x as usize + 1, 2 * d.y as usize + 1))
    };
    for it in world.items() {
        if let Some((x, y)) = to_cell(it.pos) {
            grid[y][x] = 'w';
        }
    }
    let mut near: Vec<(f32, usize)> = Vec::new();
    let mut corpses = 0;
    // Corpses first, so the living are drawn on top of them.
    for dead in [true, false] {
        for i in 0..world.zombies.len() {
            let p = world.zombies.pos[i];
            if world.zombies.is_dead(i) != dead {
                continue;
            }
            if let Some((x, y)) = to_cell(p) {
                grid[y][x] = match world.zombies.tier[i] {
                    Tier::Active => state_char(world.zombies.state[i]),
                    Tier::Near => 'n',
                    Tier::Virtual => '?',
                };
                if dead {
                    corpses += 1;
                } else {
                    near.push((p.distance(centre), i));
                }
            }
        }
    }
    if let Some((x, y)) = to_cell(world.player_spawn) {
        grid[y][x] = '@';
    }
    if let Some((x, y)) = world.goal().and_then(|g| to_cell(g.as_vec2() + Vec2::splat(0.5))) {
        grid[y][x] = 'G';
    }
    for p in &world.players {
        if let Some((x, y)) = to_cell(p.state.pos) {
            grid[y][x] = if p.dead { 'X' } else { 'P' };
        }
    }
    near.sort_by(|a, b| a.0.total_cmp(&b.0));

    let centre_place = place(world, centre);
    let mut out = format!(
        "tick {} | {} (daylight {:.0}%) | centre ({:.1}, {:.1}) in {centre_place} | {} zombies in view{} (and {corpses} corpses), {} total\n(north = top row; top-left tile is ({}, {}))\n",
        world.tick,
        clock(world.time_of_day()),
        world.daylight() * 100.0,
        centre.x,
        centre.y,
        near.len(),
        {
            let far = near.iter().filter(|(_, i)| world.zombies.tier[*i] != Tier::Active).count();
            if far > 0 { format!(", {far} of them on the near/virtual tiers") } else { String::new() }
        },
        world.zombies.len(),
        origin.x,
        origin.y
    );
    for row in grid {
        out.extend(row);
        out.push('\n');
    }
    out.push_str("players:\n");
    for p in &world.players {
        let s = &p.state;
        out.push_str(&format!(
            "  P{} at ({:.1}, {:.1}) in {}, facing {}, {:?}, {:.0} HP{}, {}, endurance {:.2}{}{}{}\n",
            p.id,
            s.pos.x,
            s.pos.y,
            place(world, s.pos),
            compass(s.facing),
            s.stance,
            p.hp,
            if p.dead { " DEAD (respawn to play on)" } else { "" },
            world.wielded(p.weapon).name.to_lowercase(),
            s.endurance,
            if s.winded { " (winded)" } else { "" },
            if map.in_window_opening(s.pos) { ", climbing through a window" } else { "" },
            match p.action {
                Some(a) => format!(", mid-{:?} ({:.2} s to land)", a.kind, a.timer).to_lowercase(),
                None if p.ready_in > 0.0 => format!(", recovering {:.2} s", p.ready_in),
                None => String::new(),
            }
        ));
    }
    let lo_items = origin.as_vec2();
    let hi_items = lo_items + Vec2::splat(n as f32);
    let items: Vec<String> = world
        .items()
        .iter()
        .filter(|it| it.pos.x >= lo_items.x && it.pos.y >= lo_items.y && it.pos.x < hi_items.x && it.pos.y < hi_items.y)
        .map(|it| format!("  #{} {} at ({:.1}, {:.1}), {:.1} m away\n", it.id, world.config.weapons[it.weapon as usize].name, it.pos.x, it.pos.y, it.pos.distance(centre)))
        .collect();
    if !items.is_empty() {
        out.push_str("weapons on the floor:\n");
        items.iter().for_each(|l| out.push_str(l));
    }
    let lo = origin.as_vec2();
    let hi = lo + Vec2::splat(n as f32);
    let damaged: Vec<String> = world
        .damaged_edges()
        .filter(|(e, _)| {
            let m = e.mid();
            m.x >= lo.x && m.y >= lo.y && m.x <= hi.x && m.y <= hi.y
        })
        .map(|(e, _)| format!("  {} at ({}, {}) {:?}{}\n", noun(map.owned_edge(e.tile, e.edge)), e.tile.x, e.tile.y, e.edge, hp_text(world, e)))
        .collect();
    if !damaged.is_empty() {
        out.push_str("damaged doors and windows:\n");
        damaged.iter().for_each(|l| out.push_str(l));
    }
    out.push_str("nearest zombies:\n");
    for (d, i) in near.iter().take(8) {
        let p = world.zombies.pos[*i];
        let tier = match world.zombies.tier[*i] {
            Tier::Active => "",
            Tier::Near => " [near tier]",
            Tier::Virtual => " [virtual tier]",
        };
        out.push_str(&format!("  #{} at ({:.1}, {:.1}) dist {:.1} m, {:.0} HP, {}{tier}\n", world.zombies.id[*i], p.x, p.y, d, world.zombies.hp[*i], zombie_goal(world, *i)));
    }
    out
}

fn edge_char(e: Edge, horizontal: bool, damaged: bool) -> char {
    match e {
        Edge::None | Edge::Arch => ' ',
        Edge::Wall => if horizontal { '-' } else { '|' },
        Edge::Window => if horizontal { '=' } else { '!' },
        Edge::WindowOpen => 'o',
        Edge::WindowBroken => 'x',
        Edge::DoorClosed => if damaged { 'd' } else { 'D' },
        Edge::DoorOpen => '/',
        Edge::DoorBroken => '#',
        Edge::Fence => '~',
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use zombye_net::content;
    use zombye_sim::WorldConfig;

    fn small_world() -> World {
        let data = content::default_data_dir();
        let config = WorldConfig::new(7, 30, content::load_building_templates(&data).unwrap())
            .with_weapons(content::load_weapons(&data).unwrap())
            .with_population(content::load_population(&data).unwrap())
            .with_map_chunks(8);
        let mut world = World::new(config);
        add_agent_player(&mut world);
        for _ in 0..40 {
            world.step();
        }
        world
    }

    /// A folder of its own for one test, removed when dropped (pass or fail).
    struct Scratch(PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn scratch(name: &str) -> Scratch {
        let dir = std::env::temp_dir().join(format!("zombye-mcp-test-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }

    /// A stand-in for the game client: a shell script (`body` runs with `$out` = the `--render-to`
    /// path and `$save` = the `--load` path, and its arguments in `args.txt`).
    #[cfg(unix)]
    fn fake_client(dir: &Path, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join("fake-zombye");
        let script = format!(
            "#!/bin/sh\necho \"$@\" > '{}/args.txt'\nwhile [ $# -gt 0 ]; do case \"$1\" in --render-to) out=\"$2\"; shift;; --load) save=\"$2\"; shift;; esac; shift; done\n{body}\n",
            dir.display()
        );
        std::fs::write(&path, script).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[cfg(unix)]
    fn renderer(dir: &Path, client: PathBuf, timeout: Duration) -> Renderer {
        let mut r = Renderer::new(Some(client), content::default_data_dir(), timeout);
        r.temp_dir = dir.join("tmp");
        std::fs::create_dir_all(&r.temp_dir).unwrap();
        r
    }

    #[cfg(unix)]
    fn leftovers(r: &Renderer) -> Vec<String> {
        std::fs::read_dir(&r.temp_dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect()
    }

    /// The tool saves the world for the client, passes the request on, returns the PNG as image
    /// content plus a line of text, cleans up, and leaves the world exactly as it was.
    #[cfg(unix)]
    #[test]
    fn screenshot_returns_the_clients_png_and_changes_nothing() {
        let scratch = scratch("ok");
        let dir = scratch.0.clone();
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        png.extend(640u32.to_be_bytes());
        png.extend(360u32.to_be_bytes());
        png.extend([7u8; 40]);
        std::fs::write(dir.join("picture.png"), &png).unwrap();
        // The fake client also keeps a copy of the save it was given.
        let client = fake_client(&dir, &format!("cp \"$save\" '{0}/given.zsave' && cp '{0}/picture.png' \"$out\"", dir.display()));
        let r = renderer(&dir, client, Duration::from_secs(20));
        let mut world = small_world();
        let (hash, tick) = (world.state_hash(), world.tick);
        let mut persist = Persist {
            path: None,
            dir: dir.clone(),
            content: world.config.clone(),
            autosave: None,
            last_save: std::time::Instant::now(),
            dirty: false,
        };
        let msg = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "screenshot", "arguments": {"width": 640, "height": 360, "angle": 1}}});
        let response = handle(&mut world, &mut persist, &r, &msg).unwrap();
        let content = &response["result"]["content"];
        assert_eq!(response["result"].get("isError"), None, "{response}");
        assert_eq!(content[0]["type"], "image");
        assert_eq!(content[0]["mimeType"], "image/png");
        assert_eq!(content[0]["data"], screenshot::base64(&png));
        let text = content[1]["text"].as_str().unwrap();
        assert!(text.starts_with("screenshot 640x360 of player 1's view") && text.contains("angle 1"), "{text}");
        let args = std::fs::read_to_string(dir.join("args.txt")).unwrap();
        assert!(args.contains("--player 1 --size 640x360 --zoom 26 --angle 1"), "{args}");
        // Nothing changed, nothing left behind, and the client got the world as it is.
        assert_eq!((world.state_hash(), world.tick), (hash, tick));
        assert!(!persist.dirty);
        assert!(leftovers(&r).is_empty(), "{:?}", leftovers(&r));
        let (given, _) = save_file::load(&dir.join("given.zsave"), &world.config, LoadOptions::default()).unwrap();
        assert_eq!(given.world.state_hash(), hash);
    }

    #[cfg(unix)]
    #[test]
    fn screenshot_errors_are_reported_and_cleaned_up() {
        let scratch = scratch("errors");
        let dir = scratch.0.clone();
        let world = small_world();
        let call = |r: &Renderer, args: Value| screenshot_tool(&world, r, &args).unwrap_err();

        let failing = renderer(&dir, fake_client(&dir, "echo 'render failed: no GPU here' >&2; exit 3"), Duration::from_secs(20));
        let e = call(&failing, json!({}));
        assert!(e.contains("failed to render") && e.contains("render failed: no GPU here"), "{e}");
        assert!(leftovers(&failing).is_empty());

        let silent = renderer(&dir, fake_client(&dir, "exit 0"), Duration::from_secs(20));
        assert!(call(&silent, json!({})).contains("wrote no picture"));

        let slow = renderer(&dir, fake_client(&dir, "sleep 5"), Duration::from_millis(300));
        let start = std::time::Instant::now();
        let e = call(&slow, json!({}));
        assert!(e.contains("longer than 0.3 s") && start.elapsed() < Duration::from_secs(3), "{e}");
        assert!(leftovers(&slow).is_empty());

        let missing = renderer(&dir, dir.join("no-such-client"), Duration::from_secs(20));
        assert!(call(&missing, json!({})).contains("no game client at"));
        assert!(call(&missing, json!({"player": 9})).contains("no player 9"));
    }

    /// A session with no `--save` file, saving into `dir`.
    fn session(world: &World, dir: &Path) -> Persist {
        Persist { path: None, dir: dir.to_path_buf(), content: world.config.clone(), autosave: None, last_save: std::time::Instant::now(), dirty: false }
    }

    /// One `tools/call` through the JSON-RPC handler: the text and whether it's a tool error.
    fn call(world: &mut World, persist: &mut Persist, name: &str, args: Value) -> (String, bool) {
        let r = Renderer::new(None, content::default_data_dir(), Duration::from_secs(1));
        let msg = json!({"jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": {"name": name, "arguments": args}});
        let response = handle(world, persist, &r, &msg).expect("a response");
        assert_eq!(response["id"], 7);
        let result = &response["result"];
        (result["content"][0]["text"].as_str().expect("text content").to_string(), result.get("isError") == Some(&json!(true)))
    }

    #[test]
    fn save_then_load_restores_the_world() {
        let scratch = scratch("save-load");
        let mut world = small_world();
        let mut persist = session(&world, &scratch.0);
        let (hash, tick) = (world.state_hash(), world.tick);
        let (text, err) = call(&mut world, &mut persist, "save", json!({"name": "first"}));
        assert!(!err && text.contains(&format!("hash {hash:016x}")) && scratch.0.join("first.zsave").exists(), "{text}");
        assert!(!persist.dirty, "saving doesn't need saving");
        let (text, err) = call(&mut world, &mut persist, "step_ticks", json!({"n": 30}));
        assert!(!err, "{text}");
        assert!(persist.dirty && world.state_hash() != hash);
        let (text, err) = call(&mut world, &mut persist, "load", json!({"name": "first"}));
        assert!(!err && text.contains(&format!("hash {hash:016x}")) && !text.contains("note:"), "{text}");
        assert_eq!((world.state_hash(), world.tick), (hash, tick));
        assert_eq!(world.players.len(), 1, "the agent's player came back, no second one added");
        // The loaded world carries on like the saved one did.
        let mut again = small_world();
        for _ in 0..30 {
            world.step();
            again.step();
        }
        assert_eq!(world.state_hash(), again.state_hash());

        // No name and no --save file: nothing to save to or load from.
        let (text, err) = call(&mut world, &mut persist, "save", json!({}));
        assert!(err && text.contains("give a name"), "{text}");
        let (text, err) = call(&mut world, &mut persist, "load", json!({}));
        assert!(err && text.contains("give the name"), "{text}");
        // With one, no name means the session's file, and saving there catches the session up.
        persist.path = Some(scratch.0.join("session.zsave"));
        persist.dirty = true;
        let (text, err) = call(&mut world, &mut persist, "save", json!({}));
        assert!(!err && text.contains("session.zsave"), "{text}");
        assert!(!persist.dirty);
        let (_, err) = call(&mut world, &mut persist, "load", json!({}));
        assert!(!err && !persist.dirty);
        // Loading another file leaves the session's own behind.
        let (_, err) = call(&mut world, &mut persist, "load", json!({"name": "first"}));
        assert!(!err && persist.dirty);
    }

    #[test]
    fn saves_lists_them_with_their_info() {
        let scratch = scratch("list");
        let mut world = small_world();
        let mut persist = session(&world, &scratch.0);
        let (text, err) = call(&mut world, &mut persist, "saves", json!({}));
        assert!(!err && text.starts_with(&format!("saves in {}: 0", scratch.0.display())), "{text}");
        call(&mut world, &mut persist, "save", json!({"name": "early"}));
        world.step();
        call(&mut world, &mut persist, "save", json!({"name": "later.zsave"}));
        std::fs::write(scratch.0.join("broken.zsave"), b"ZOMBYSAV\x01\0\0\0junk").unwrap();
        std::fs::write(scratch.0.join("notes.txt"), b"not a save").unwrap();
        let (text, err) = call(&mut world, &mut persist, "saves", json!({}));
        assert!(!err, "{text}");
        assert!(text.contains(": 3\n"), "three saves, the .txt ignored: {text}");
        let line = |name: &str| text.lines().find(|l| l.trim_start().starts_with(&format!("{name}:"))).unwrap_or_else(|| panic!("no {name} in {text}")).to_string();
        let (map, z) = (format!("map {}x{} m", world.map.width(), world.map.height()), world.zombies.len());
        for (name, tick) in [("early", world.tick - 1), ("later", world.tick)] {
            let l = line(name);
            assert!(l.contains(&format!("tick {tick} ")) && l.contains("seed 7") && l.contains(&map) && l.contains(&format!("{z} zombies")) && l.contains("1 players"), "{l}");
        }
        assert!(line("broken").contains("unreadable"), "{text}");
        assert!(text.ends_with("this session has no --save file: nothing is saved unless you call save"), "{text}");
        persist.path = Some(scratch.0.join("auto.zsave"));
        persist.autosave = Some(Duration::from_secs(300));
        let (text, _) = call(&mut world, &mut persist, "saves", json!({}));
        assert!(text.ends_with("auto.zsave (autosave every 5 min; and on shutdown)"), "{text}");
    }

    #[test]
    fn save_names_cant_leave_the_saves_folder() {
        let scratch = scratch("names");
        let inside = scratch.0.join("saves");
        let mut world = small_world();
        let mut persist = session(&world, &inside);
        let hash = world.state_hash();
        for bad in ["../escaped", "../../etc/passwd", "/tmp/abs", "a/b", "..", ".hidden", "x\\y", "", "a b"] {
            for tool in ["save", "load"] {
                let (text, err) = call(&mut world, &mut persist, tool, json!({"name": bad}));
                assert!(err && text.contains("bad save name"), "{tool} {bad:?}: {text}");
            }
        }
        assert!(!scratch.0.join("escaped.zsave").exists() && !inside.exists(), "nothing written anywhere");
        assert_eq!(world.state_hash(), hash);
    }

    #[test]
    fn a_bad_save_is_a_clear_tool_error_and_changes_nothing() {
        let scratch = scratch("bad");
        let mut world = small_world();
        let mut persist = session(&world, &scratch.0);
        call(&mut world, &mut persist, "save", json!({"name": "good"}));
        let bytes = std::fs::read(scratch.0.join("good.zsave")).unwrap();
        let mut flipped = bytes.clone();
        flipped[bytes.len() / 2] ^= 0x55;
        std::fs::write(scratch.0.join("flipped.zsave"), &flipped).unwrap();
        std::fs::write(scratch.0.join("cut.zsave"), &bytes[..bytes.len() / 3]).unwrap();
        std::fs::write(scratch.0.join("text.zsave"), b"hello").unwrap();
        world.step();
        let hash = world.state_hash();
        for (name, says) in [("flipped", "save file is damaged"), ("cut", "truncated"), ("text", "not a Zombye save"), ("missing", "couldn't read")] {
            let (text, err) = call(&mut world, &mut persist, "load", json!({"name": name}));
            assert!(err && text.contains(says) && text.contains(&format!("{name}.zsave")), "{name}: {text}");
            assert!(!text.contains("force"), "{name}: loading anyway wouldn't help: {text}");
            assert_eq!(world.state_hash(), hash, "{name}: the world is untouched");
        }

        // Game data changed so the map would differ: refused, saying how to load anyway; force does.
        persist.content.templates.truncate(1);
        let (text, err) = call(&mut world, &mut persist, "load", json!({"name": "good"}));
        assert!(err && text.contains("templates") && text.contains("load with force: true loads it anyway"), "{text}");
        assert_eq!(world.state_hash(), hash);
        let (text, err) = call(&mut world, &mut persist, "load", json!({"name": "good", "force": true}));
        assert!(!err && text.contains("note: loaded anyway"), "{text}");
        let (text, err) = call(&mut world, &mut persist, "step_ticks", json!({"n": 20}));
        assert!(!err, "{text}");
    }
}
