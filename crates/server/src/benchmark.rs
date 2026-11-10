//! The agent survival benchmark (#29, docs/benchmark.md): fixed scenarios
//! (`assets/data/scenarios.ron`, `zombye_sim::scenario`), played over MCP or by a built-in
//! baseline, each run ending in a results file.
//!
//! - `zombye-server --benchmark list` prints the scenarios.
//! - `zombye-server --mcp --benchmark ID [--agent NAME] [--results DIR]` (or `--mcp-http`) is the
//!   MCP server on that scenario's world: only what a player can do (no debug tools, no saves, no
//!   getting up again after dying), a `benchmark` tool that says how the run stands, and time that
//!   stops for good when the survivor dies or the scenario's hours are up ([`step`]). The result is
//!   written then, or as `unfinished` when the agent goes away first.
//! - `zombye-server --benchmark ID|all --baseline idle|brawler|runner` plays it with a script, headless:
//!   reference scores, and a check that the same run gives the same result.
//! - `zombye-server --benchmark table [--results DIR]` ranks the results in a folder.
//!
//! Results are JSON, one file a run ([`FORMAT`], [`Run::result`]). No leaderboard is hosted.

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use glam::Vec2;
use serde_json::{Value, json};
use zombye_net::{Intent, apply_intent};
use zombye_sim::scenario::{ScenarioDef, Standing};
use zombye_sim::{World, WorldConfig, ZombieState};

/// The results format: bumped when a field changes meaning or goes.
pub const FORMAT: &str = "zombye-benchmark/1";
/// Where results go unless `--results DIR` says otherwise.
pub const DEFAULT_DIR: &str = "saves/benchmark";
/// The built-in baselines (`--baseline NAME`).
pub const BASELINES: [&str; 3] = ["idle", "brawler", "runner"];

thread_local! {
    /// While a benchmark run's tool call is being answered: when its world stops (game hours) and
    /// whose death stops it. The MCP tools step the world through [`step`].
    static LIMIT: Cell<Option<(f64, u32)>> = const { Cell::new(None) };
}

/// One tick of `world`, for the MCP tools: always, unless a benchmark run is being played and it
/// is over (its survivor dead, or its hours up), from when its world stands still. Returns whether
/// the world stepped; a tool's loop stops when it didn't.
pub fn step(world: &mut World) -> bool {
    if let Some((hours, player)) = LIMIT.get() {
        if world.game_hours() >= hours || world.player(player).is_none_or(|p| p.dead) {
            return false;
        }
    }
    world.step();
    true
}

/// A scenario being played over MCP.
pub struct Run {
    pub def: ScenarioDef,
    /// The survivor's player id.
    pub player: u32,
    /// Who plays (`--agent NAME`): a label in the results.
    pub agent: String,
    /// The folder the result is written to.
    pub dir: PathBuf,
    fingerprint: u64,
    /// Tool calls so far, and those of them that acted or stepped time.
    calls: u64,
    actions: u64,
    started: u64,
    clock: Instant,
    /// The result, once the run is over and written.
    done: Option<Value>,
}

impl Run {
    pub fn new(def: ScenarioDef, world: &World, player: u32, agent: String, dir: PathBuf) -> Self {
        let fingerprint = def.fingerprint(world);
        Run { def, player, agent, dir, fingerprint, calls: 0, actions: 0, started: unix_now(), clock: Instant::now(), done: None }
    }

    /// Before a tool call: counted, and for one that plays (`play`), refused once the run is over
    /// (and getting up again after dying always), else the world's stop is set for [`step`].
    pub fn before(&mut self, name: &str, play: bool) -> Result<(), String> {
        self.calls += 1;
        if !play {
            return Ok(());
        }
        if let Some(done) = &self.done {
            return Err(format!("the benchmark run is over ({}): nothing more can be done. The benchmark tool has the result.", done["summary"].as_str().unwrap_or("")));
        }
        if name == "respawn" {
            return Err("a benchmark run is one life: there is no getting up again".to_string());
        }
        self.actions += 1;
        LIMIT.set(Some((self.def.hours as f64, self.player)));
        Ok(())
    }

    /// After a tool call that played: the stop is lifted, and if that call ended the run the result
    /// is written and the tool's answer says so.
    pub fn after(&mut self, world: &World, result: Result<Value, String>) -> Result<Value, String> {
        LIMIT.set(None);
        let standing = self.def.standing(world, self.player);
        if self.done.is_some() || !standing.outcome.over() {
            return result;
        }
        let over = format!("BENCHMARK OVER: {}. {}", standing.line(), self.finish(world, &standing, standing.outcome.name()));
        match result {
            Ok(mut content) => {
                if let Some(list) = content.as_array_mut() {
                    list.push(json!({"type": "text", "text": over}));
                }
                Ok(content)
            }
            Err(text) => Err(format!("{text}\n{over}")),
        }
    }

    /// The server is stopping: a run still going is written as `unfinished`, with its score so far.
    pub fn close(&mut self, world: &World) {
        if self.done.is_none() {
            let standing = self.def.standing(world, self.player);
            let outcome = if standing.outcome.over() { standing.outcome.name() } else { "unfinished" };
            let said = self.finish(world, &standing, outcome);
            eprintln!("benchmark {}: {}. {said}", self.def.id, standing.line());
        }
    }

    /// Keep the result and write its file; says where it went (or why it couldn't be written).
    fn finish(&mut self, world: &World, standing: &Standing, outcome: &str) -> String {
        let result = self.result(world, standing, outcome);
        let said = match write_result(&self.dir, &result) {
            Ok(path) => format!("Result written to {}", path.display()),
            Err(e) => format!("The result could not be written: {e}"),
        };
        self.done = Some(result);
        said
    }

    /// The results file's contents (docs/benchmark.md has the fields).
    pub fn result(&self, world: &World, standing: &Standing, outcome: &str) -> Value {
        let mut r = result_json(&self.def, self.fingerprint, world, standing, outcome, &self.agent);
        r["tool_calls"] = json!(self.calls);
        r["actions"] = json!(self.actions);
        r["started"] = json!(stamp(self.started));
        r["wall_secs"] = json!((self.clock.elapsed().as_secs_f64() * 10.0).round() / 10.0);
        r
    }

    /// The `benchmark` tool's answer: the scenario, the rules, and how the run stands (or ended).
    pub fn report(&self, world: &World) -> String {
        let standing = self.def.standing(world, self.player);
        let d = &self.def;
        let mut out = json!({
            "scenario": d.id,
            "name": d.name,
            "brief": d.brief,
            "agent": self.agent,
            "rules": format!(
                "One life, {} game hours (a game hour is {:.0} s of game time, {:.0} ticks). The run ends when you die or the hours are up; time stops then. \
                 Score = {} x (hours survived / {}) + {} x buildings searched + {} x zombies killed. A building counts once you have searched a container in it.",
                d.hours,
                world.config.day_length_secs / 24.0,
                world.config.day_length_secs / 24.0 / zombye_sim::TICK_DT,
                d.points.survive,
                d.hours,
                d.points.house,
                d.points.kill
            ),
            "status": standing.outcome.name(),
            "hours_survived": round(standing.hours, 3),
            "hours_left": round((standing.limit - standing.hours).max(0.0), 3),
            "score": standing.score(),
            "points": {"survive": standing.points[0], "houses": standing.points[1], "kills": standing.points[2]},
            "houses_searched": standing.houses,
            "kills": standing.kills,
            "hp": round(standing.hp as f64, 1),
            "summary": standing.line(),
        });
        if let Some(done) = &self.done {
            out["result"] = done.clone();
        }
        serde_json::to_string_pretty(&out).unwrap_or_default()
    }

    /// Added to the server's instructions: what is being played and what counts.
    pub fn instructions(&self) -> String {
        format!(
            "BENCHMARK RUN: scenario \"{}\" ({}). {} One life, {} game hours; the run ends when you die or the time is up, and time stops then. \
             Call the benchmark tool for the rules, the time left and your score.",
            self.def.id, self.def.name, self.def.brief, self.def.hours
        )
    }
}

/// The `benchmark` tool as `tools/list` gives it (only on a server playing a scenario).
pub fn tool_def() -> Value {
    json!({
        "name": "benchmark",
        "title": "Benchmark: the scenario and your score",
        "description": "This server is running a benchmark scenario. Returns JSON: the scenario and its brief, the rules and how the score is counted, \
                        game hours survived and left, the score so far with its parts, and once the run is over (you died, or the time is up) the final result. \
                        Takes no game time and changes nothing.",
        "inputSchema": {"type": "object", "properties": {}},
        "annotations": {"title": "Benchmark: the scenario and your score", "readOnlyHint": true, "openWorldHint": false},
    })
}

fn round(v: f64, places: i32) -> f64 {
    let m = 10f64.powi(places);
    (v * m).round() / m
}

fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// `2026-11-10 17:04:09Z`.
fn stamp(unix: u64) -> String {
    crate::admin::stamp(unix)
}

/// What every result has, however it was played. Everything here follows from the scenario and
/// what the player did: the same run gives the same values.
fn result_json(def: &ScenarioDef, fingerprint: u64, world: &World, s: &Standing, outcome: &str, agent: &str) -> Value {
    json!({
        "format": FORMAT,
        "scenario": def.id,
        "scenario_name": def.name,
        "fingerprint": format!("{fingerprint:016x}"),
        "game_version": env!("CARGO_PKG_VERSION"),
        "agent": agent,
        "outcome": outcome,
        "score": s.score(),
        "points": {"survive": s.points[0], "houses": s.points[1], "kills": s.points[2]},
        "hours_survived": round(s.hours, 3),
        "hours": round(s.limit, 3),
        "houses_searched": s.houses,
        "kills": s.kills,
        "containers_searched": s.containers,
        "items_taken": s.items_taken,
        "distance_m": round(s.distance_m, 1),
        "wounds": s.wounds,
        "hp_lost": round(s.hp_lost as f64, 1),
        "hp": round(s.hp as f64, 1),
        "cause_of_death": s.cause_of_death,
        "ticks": world.tick,
        "state_hash": format!("{:016x}", world.state_hash()),
        "summary": s.line(),
    })
}

/// Write a result into `dir` as `<scenario>-<agent>-<YYYYMMDD-HHMMSS>.json` (a number added if
/// that file is there already).
pub fn write_result(dir: &Path, result: &Value) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let safe: String = result["agent"].as_str().unwrap_or("").chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '_' }).take(40).collect();
    let when: String = stamp(unix_now()).chars().filter(|c| c.is_ascii_digit() || *c == ' ').map(|c| if c == ' ' { '-' } else { c }).collect();
    let stem = format!("{}-{}-{when}", result["scenario"].as_str().unwrap_or("run"), if safe.is_empty() { "agent" } else { &safe });
    let text = serde_json::to_string_pretty(result).map_err(|e| e.to_string())? + "\n";
    for n in 0..1000 {
        let path = dir.join(if n == 0 { format!("{stem}.json") } else { format!("{stem}-{n}.json") });
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut f) => {
                use std::io::Write;
                f.write_all(text.as_bytes()).map_err(|e| format!("{}: {e}", path.display()))?;
                return Ok(path);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(format!("{}: {e}", path.display())),
        }
    }
    Err(format!("{}: too many results of the same second", dir.display()))
}

/// Play `def` with the built-in baseline `name`, headless: the result (agent `baseline:NAME`).
/// A baseline only sends the intents a client sends; it reads the world to decide, which an agent
/// can't, so it is a reference, not a competitor.
pub fn baseline(def: &ScenarioDef, content: &WorldConfig, name: &str) -> Result<Value, String> {
    if !BASELINES.contains(&name) {
        return Err(format!("no baseline {name:?} (there are: {})", BASELINES.join(", ")));
    }
    let (mut world, id) = def.start(content);
    let fingerprint = def.fingerprint(&world);
    let hours = def.hours as f64;
    while world.game_hours() < hours && world.player(id).is_some_and(|p| !p.dead) {
        match name {
            "brawler" => brawler(&mut world, id),
            "runner" => runner(&mut world, id),
            _ => {}
        }
        world.step();
        world.edge_changes.clear();
        world.combat_events.clear();
    }
    let standing = def.standing(&world, id);
    Ok(result_json(def, fingerprint, &world, &standing, standing.outcome.name(), &format!("baseline:{name}")))
}

/// The brawler's tick: with a zombie within 9 m it stands its ground, facing the nearest, swings
/// when it is in reach and stamps on one lying at its feet; with none it walks about, turning
/// every few seconds (by a hash of the tick: the same walk every run).
fn brawler(world: &mut World, id: u32) {
    let Some(p) = world.player(id) else { return };
    let (pos, seq, ready, reach) = (p.state.pos, p.last_input_seq + 1, p.action.is_none() && p.ready_in <= 0.0, world.wielded(p.weapon).range + 0.3);
    let nearest = (0..world.zombies.len())
        .filter(|&i| world.zombies.state[i] != ZombieState::Dead)
        .map(|i| (world.zombies.pos[i].distance_squared(pos), i))
        .filter(|(d, _)| *d < 81.0)
        .min_by(|a, b| a.0.total_cmp(&b.0));
    match nearest {
        Some((d2, i)) => {
            let to = world.zombies.pos[i] - pos;
            let down = world.zombies.state[i] == ZombieState::Down;
            apply_intent(world, &Intent::PlayerInput { player: id, seq, move_dir: [0.0, 0.0], facing: to.y.atan2(to.x), run: false, sneak: false, aim: true });
            if ready && down && d2 < 1.0 {
                apply_intent(world, &Intent::Stomp { player: id });
            } else if ready && !down && d2.sqrt() < reach {
                apply_intent(world, &Intent::Attack { player: id });
            }
        }
        None => {
            let turn = zombye_sim::rng::hash01(world.config.seed(), world.tick / 80, 0xbe9c, id as u64) * std::f32::consts::TAU;
            let dir = Vec2::from_angle(turn);
            apply_intent(world, &Intent::PlayerInput { player: id, seq, move_dir: dir.to_array(), facing: turn, run: false, sneak: false, aim: false });
        }
    }
}

/// The runner's tick: it keeps away from the zombies within 18 m (each pushing it off, the nearer
/// the harder), running when one is within 7 m, and stands still with none about. It never fights
/// or searches.
fn runner(world: &mut World, id: u32) {
    let Some(p) = world.player(id) else { return };
    let (pos, seq, facing) = (p.state.pos, p.last_input_seq + 1, p.state.facing);
    let (mut away, mut nearest) = (Vec2::ZERO, f32::MAX);
    for i in 0..world.zombies.len() {
        let from = pos - world.zombies.pos[i];
        let d = from.length();
        if world.zombies.state[i] != ZombieState::Dead && d < 18.0 {
            away += from / (d * d).max(0.01);
            nearest = nearest.min(d);
        }
    }
    let dir = away.normalize_or_zero();
    let facing = if dir == Vec2::ZERO { facing } else { dir.y.atan2(dir.x) };
    apply_intent(world, &Intent::PlayerInput { player: id, seq, move_dir: dir.to_array(), facing, run: nearest < 7.0, sneak: false, aim: false });
}

/// `--benchmark list`: every scenario, with what a run of it is.
pub fn list(defs: &[ScenarioDef]) -> String {
    let mut out = String::new();
    for d in defs {
        out += &format!(
            "{}: {}\n  seed {}, map {} m, {} zombies{}, starts {:02}:{:02}, {} game hours ({:.0} min of game time awake)\n  score: {} for surviving, {} a building searched, {} a kill\n  {}\n",
            d.id,
            d.name,
            d.seed,
            d.map_chunks * 16,
            d.zombies.map_or("the map's own".to_string(), |n| n.to_string()),
            match d.hordes.iter().map(|h| h.count).sum::<u32>() {
                0 => String::new(),
                n => format!(" + {n} near the start"),
            },
            d.start_hour as u32,
            ((d.start_hour.fract() * 60.0) as u32).min(59),
            d.hours,
            d.hours * 2.5,
            d.points.survive,
            d.points.house,
            d.points.kill,
            d.brief,
        );
    }
    out
}

/// One line of a result, for the terminal.
pub fn line(result: &Value) -> String {
    format!("{} [{}] {}: {}", result["scenario"].as_str().unwrap_or("?"), result["agent"].as_str().unwrap_or("?"), result["outcome"].as_str().unwrap_or("?"), result["summary"].as_str().unwrap_or(""))
}

/// `--benchmark table`: the results in `dir`, best first, a table per scenario (and per
/// fingerprint: runs of a changed scenario or other game data don't compare).
pub fn table(dir: &Path) -> Result<String, String> {
    let mut results: Vec<Value> = Vec::new();
    let mut skipped = 0;
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().is_some_and(|x| x == "json")).collect();
    paths.sort();
    for path in paths {
        match std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) {
            Some(v) if v["format"] == FORMAT && v["scenario"].is_string() && v["score"].is_u64() => results.push(v),
            _ => skipped += 1,
        }
    }
    let key = |v: &Value| (v["scenario"].as_str().unwrap_or("").to_string(), v["fingerprint"].as_str().unwrap_or("").to_string());
    // Best first; the stable sort keeps equal scores in file (so time) order.
    results.sort_by(|a, b| key(a).cmp(&key(b)).then(b["score"].as_u64().cmp(&a["score"].as_u64())));
    let mut out = String::new();
    let mut last = None;
    let mut rank = 0;
    for r in &results {
        if last.as_ref() != Some(&key(r)) {
            last = Some(key(r));
            rank = 0;
            out += &format!("\n{} (fingerprint {}, game {})\n", r["scenario"].as_str().unwrap_or(""), r["fingerprint"].as_str().unwrap_or(""), r["game_version"].as_str().unwrap_or("?"));
            out += &format!("{:>4} {:>6}  {:<10} {:>7} {:>6} {:>5} {:>6}  {}\n", "rank", "score", "outcome", "hours", "houses", "kills", "calls", "agent");
        }
        rank += 1;
        out += &format!(
            "{rank:>4} {:>6}  {:<10} {:>7.2} {:>6} {:>5} {:>6}  {}\n",
            r["score"].as_u64().unwrap_or(0),
            r["outcome"].as_str().unwrap_or("?"),
            r["hours_survived"].as_f64().unwrap_or(0.0),
            r["houses_searched"].as_u64().unwrap_or(0),
            r["kills"].as_u64().unwrap_or(0),
            r["tool_calls"].as_u64().map_or("-".to_string(), |n| n.to_string()),
            r["agent"].as_str().unwrap_or("?"),
        );
    }
    if results.is_empty() {
        out += &format!("no results in {}\n", dir.display());
    }
    if skipped > 0 {
        out += &format!("\n({skipped} other .json file{} skipped: not {FORMAT})\n", if skipped == 1 { "" } else { "s" });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    use crate::Persist;
    use crate::mcp::{self, Options};
    use crate::screenshot::Renderer;
    use zombye_net::content;
    use zombye_sim::scenario::Outcome;

    fn content_and_defs() -> (WorldConfig, Vec<ScenarioDef>) {
        let data = content::default_data_dir();
        let config = content::load_config(&data, 1).unwrap();
        let defs = content::load_scenarios(&data, &config.items, &config.weapons).unwrap();
        (config, defs)
    }

    /// A short scenario on a small map, for tests: three game minutes.
    fn short() -> ScenarioDef {
        let text = "[(id: \"short\", name: \"Short\", brief: \"Live three minutes.\", seed: 7, map_chunks: 12, zombies: Some(60), start_hour: 12.0, hold: Some(\"baseball_bat\"), \
                    hordes: [(count: 6, distance: 9.0, bearing: 90.0, radius: 3.0)], noise: Some((radius: 30.0)), hours: 0.05)]";
        zombye_sim::scenario::parse_scenarios(text).unwrap().remove(0)
    }

    struct Scratch(PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn scratch(name: &str) -> Scratch {
        let dir = std::env::temp_dir().join(format!("zombye-benchmark-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        Scratch(dir)
    }

    /// A benchmark session over MCP on `def`: the world, and the session with the run in it.
    fn session(def: &ScenarioDef, content: &WorldConfig, dir: &Path) -> (World, Persist) {
        let (world, player) = def.start(content);
        let run = Run::new(def.clone(), &world, player, "test agent".into(), dir.to_path_buf());
        let persist = Persist { path: None, dir: dir.to_path_buf(), content: content.clone(), autosave: None, last_save: Instant::now(), dirty: false, benchmark: Some(run) };
        (world, persist)
    }

    fn rpc(world: &mut World, persist: &mut Persist, method: &str, params: Value) -> Value {
        let r = Renderer::new(None, content::default_data_dir(), Duration::from_secs(1));
        let msg = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
        mcp::handle(world, persist, &r, Options::default(), &msg).expect("a response")
    }

    /// One tool call: every text of the answer joined, and whether it is a tool error.
    fn call(world: &mut World, persist: &mut Persist, name: &str, args: Value) -> (String, bool) {
        let response = rpc(world, persist, "tools/call", json!({"name": name, "arguments": args}));
        let result = &response["result"];
        assert!(result.is_object(), "{response}");
        let text: Vec<&str> = result["content"].as_array().unwrap().iter().filter_map(|c| c["text"].as_str()).collect();
        (text.join("\n"), result.get("isError") == Some(&json!(true)))
    }

    /// The shipped scenarios start: a living survivor, zombies, the hour, what it holds.
    #[test]
    fn every_shipped_scenario_starts() {
        let (content, defs) = content_and_defs();
        assert!(defs.len() >= 4);
        let listed = list(&defs);
        for d in &defs {
            let (world, id) = d.start(&content);
            let p = world.player(id).unwrap();
            assert!(!p.dead && world.zombies.len() > 100, "{}", d.id);
            assert_eq!(world.map.room_at(p.state.pos) == 0, d.start != zombye_sim::scenario::Start::Bedroom, "{}", d.id);
            assert!((world.time_of_day() - d.start_hour).abs() < 0.01, "{}", d.id);
            assert_eq!(p.inventory.hand.is_some(), d.hold.is_some(), "{}", d.id);
            assert_eq!(p.inventory.carried.len(), d.kit.len(), "{}", d.id);
            let s = d.standing(&world, id);
            assert_eq!((s.outcome, s.score()), (Outcome::Running, 0), "{}", d.id);
            assert!(listed.contains(&format!("{}: {}", d.id, d.name)));
            // No zombie on top of the survivor at the start.
            let near = world.zombies.pos.iter().filter(|z| z.distance(p.state.pos) < 6.0).count();
            assert_eq!(near, 0, "{}: {near} zombies within 6 m of the start", d.id);
        }
    }

    /// A baseline run is the same every time, down to the world's hash, and ends by the rules.
    #[test]
    fn a_baseline_run_repeats_exactly() {
        let (content, _) = content_and_defs();
        let def = short();
        for name in BASELINES {
            let (a, b) = (baseline(&def, &content, name).unwrap(), baseline(&def, &content, name).unwrap());
            assert_eq!(a, b, "{name}");
            assert_eq!(a["format"], FORMAT);
            assert_eq!(a["agent"], format!("baseline:{name}"));
            let outcome = a["outcome"].as_str().unwrap();
            assert!(outcome == "survived" || outcome == "died", "{a}");
            if outcome == "survived" {
                // 0.05 game hours: 150 ticks.
                assert_eq!((a["ticks"].as_u64(), a["points"]["survive"].as_u64()), (Some(150), Some(1000)), "{a}");
            }
            assert_eq!(a["score"].as_u64().unwrap(), a["points"]["survive"].as_u64().unwrap() + a["points"]["houses"].as_u64().unwrap() + a["points"]["kills"].as_u64().unwrap());
        }
        assert!(baseline(&def, &content, "genius").unwrap_err().contains("no baseline"));
        // The brawler fights: with six zombies called onto it, it hits back.
        let mut long = short();
        long.hours = 0.4;
        let r = baseline(&long, &content, "brawler").unwrap();
        assert!(r["kills"].as_u64().unwrap() >= 1, "{r}");
    }

    /// Over MCP: only a player's tools and the benchmark tool, time stops when the hours are up,
    /// the result is written once, and nothing can be done after.
    #[test]
    fn a_run_over_mcp_ends_when_the_time_is_up_and_writes_its_result() {
        let (content, _) = content_and_defs();
        let def = short();
        let scratch = scratch("mcp");
        let (mut world, mut persist) = session(&def, &content, &scratch.0);

        // The tools: the benchmark tool, no debug tools, no saves.
        let list = rpc(&mut world, &mut persist, "tools/list", json!({}));
        let names: Vec<&str> = list["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"benchmark") && names.contains(&"observe") && names.contains(&"move_player"), "{names:?}");
        for gone in ["skip_time", "spawn_item", "dev", "load", "save", "saves", "add_player", "set_needs"] {
            assert!(!names.contains(&gone), "{gone} is offered");
        }
        let init = rpc(&mut world, &mut persist, "initialize", json!({"protocolVersion": "2025-06-18"}));
        assert!(init["result"]["instructions"].as_str().unwrap().contains("BENCHMARK RUN: scenario \"short\""), "{init}");
        let refused = rpc(&mut world, &mut persist, "tools/call", json!({"name": "skip_time", "arguments": {"hours": 1}}));
        assert!(refused["error"]["message"].as_str().unwrap().contains("Unknown tool"), "{refused}");
        let (text, err) = call(&mut world, &mut persist, "respawn", json!({}));
        assert!(err && text.contains("one life"), "{text}");

        // Looking takes no time and doesn't end anything.
        let (report, err) = call(&mut world, &mut persist, "benchmark", json!({}));
        let report: Value = serde_json::from_str(&report).unwrap();
        assert!(!err);
        assert_eq!((report["status"].as_str(), report["score"].as_u64(), report["hours_left"].as_f64()), (Some("running"), Some(0), Some(0.05)));
        assert_eq!(world.tick, 0);

        // 0.05 game hours is 150 ticks: 100 pass, then a step of 500 stops at the end.
        let (text, _) = call(&mut world, &mut persist, "step_ticks", json!({"n": 100}));
        assert!(!text.contains("BENCHMARK OVER"), "{text}");
        assert_eq!(world.tick, 100);
        let alive = !world.players[0].dead;
        let (text, err) = call(&mut world, &mut persist, "step_ticks", json!({"n": 500}));
        assert!(!err && text.contains("BENCHMARK OVER: ") && text.contains("Result written to "), "{text}");
        if alive && !world.players[0].dead {
            assert_eq!(world.tick, 150);
            assert!(text.contains("survived 0.05 of 0.05 game hours"), "{text}");
        }
        let (tick, hash) = (world.tick, world.state_hash());

        // One file, with the run in it.
        let files: Vec<PathBuf> = std::fs::read_dir(&scratch.0).unwrap().map(|e| e.unwrap().path()).collect();
        assert_eq!(files.len(), 1, "{files:?}");
        let name = files[0].file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("short-test_agent-") && name.ends_with(".json"), "{name}");
        let result: Value = serde_json::from_str(&std::fs::read_to_string(&files[0]).unwrap()).unwrap();
        assert_eq!(result["format"], FORMAT);
        assert_eq!((result["scenario"].as_str(), result["agent"].as_str()), (Some("short"), Some("test agent")));
        assert_eq!(result["ticks"].as_u64(), Some(tick));
        assert_eq!(result["state_hash"].as_str().unwrap(), format!("{hash:016x}"));
        assert_eq!(result["actions"].as_u64(), Some(2));
        assert!(result["tool_calls"].as_u64().unwrap() >= 4);
        for field in ["fingerprint", "game_version", "outcome", "score", "points", "hours_survived", "hours", "houses_searched", "kills", "containers_searched", "items_taken", "distance_m", "wounds", "hp_lost", "hp", "cause_of_death", "started", "wall_secs", "summary"] {
            assert!(result.get(field).is_some(), "no {field}");
        }

        // Over: nothing moves, playing is refused, looking still works and has the result.
        for (tool, args) in [("step_ticks", json!({"n": 50})), ("move_player", json!({"direction": "n"})), ("attack", json!({})), ("sleep", json!({}))] {
            let (text, err) = call(&mut world, &mut persist, tool, args);
            assert!(err && text.contains("the benchmark run is over"), "{tool}: {text}");
        }
        assert_eq!((world.tick, world.state_hash()), (tick, hash));
        let (report, _) = call(&mut world, &mut persist, "benchmark", json!({}));
        let report: Value = serde_json::from_str(&report).unwrap();
        assert_eq!(report["result"]["score"], result["score"]);
        assert!(!call(&mut world, &mut persist, "observe", json!({})).1);
        // Stopping the server writes nothing more.
        persist.benchmark.as_mut().unwrap().close(&world);
        assert_eq!(std::fs::read_dir(&scratch.0).unwrap().count(), 1);

        // The table ranks it.
        let t = table(&scratch.0).unwrap();
        assert!(t.contains("short (fingerprint ") && t.contains("test agent"), "{t}");
    }

    /// Dying ends the run at that tick, whatever the tool was still going to do; an agent that
    /// leaves mid-run gets an `unfinished` result.
    #[test]
    fn dying_ends_the_run_and_leaving_writes_it_unfinished() {
        let (content, _) = content_and_defs();
        let mut def = short();
        def.hours = 2.0;
        let scratch = scratch("death");
        let (mut world, mut persist) = session(&def, &content, &scratch.0);
        call(&mut world, &mut persist, "step_ticks", json!({"n": 20}));
        // Something kills it between calls (a wound no player could give itself this fast).
        world.inflict_wound(1, zombye_sim::Part::Torso, zombye_sim::WoundKind::Bite, Some(false));
        world.players[0].hp = 0.5;
        let (text, _) = call(&mut world, &mut persist, "step_ticks", json!({"n": 72000}));
        assert!(text.contains("BENCHMARK OVER: died after"), "{text}");
        let died = world.players[0].run.death.expect("dead").tick;
        assert_eq!(world.tick, died + 1, "the world stopped with the tick it died on");
        let (report, _) = call(&mut world, &mut persist, "benchmark", json!({}));
        let report: Value = serde_json::from_str(&report).unwrap();
        assert_eq!(report["result"]["outcome"], "died");
        assert!(report["result"]["cause_of_death"].is_string() && report["result"]["points"]["survive"].as_u64().unwrap() < 1000, "{report}");

        // Another run, left after a few seconds.
        let scratch = super::tests::scratch("left");
        let (mut world, mut persist) = session(&def, &content, &scratch.0);
        call(&mut world, &mut persist, "step_ticks", json!({"n": 40}));
        persist.benchmark.as_mut().unwrap().close(&world);
        let files: Vec<PathBuf> = std::fs::read_dir(&scratch.0).unwrap().map(|e| e.unwrap().path()).collect();
        assert_eq!(files.len(), 1);
        let result: Value = serde_json::from_str(&std::fs::read_to_string(&files[0]).unwrap()).unwrap();
        assert_eq!((result["outcome"].as_str(), result["ticks"].as_u64()), (Some("unfinished"), Some(40)));
    }

    /// The same calls give the same result: what an agent did is all that decides a run.
    #[test]
    fn the_same_calls_give_the_same_result() {
        let (content, _) = content_and_defs();
        let def = short();
        let play = |name: &str| {
            let scratch = scratch(name);
            let (mut world, mut persist) = session(&def, &content, &scratch.0);
            call(&mut world, &mut persist, "move_player", json!({"direction": "w", "ticks": 30}));
            call(&mut world, &mut persist, "attack", json!({"facing": 0.0}));
            call(&mut world, &mut persist, "observe", json!({}));
            call(&mut world, &mut persist, "step_ticks", json!({"n": 400}));
            let (report, _) = call(&mut world, &mut persist, "benchmark", json!({}));
            let mut result = serde_json::from_str::<Value>(&report).unwrap()["result"].take();
            assert!(result.is_object(), "{report}");
            // What the wall clock says is the only thing that may differ.
            for k in ["started", "wall_secs"] {
                result.as_object_mut().unwrap().remove(k);
            }
            result
        };
        assert_eq!(play("same-a"), play("same-b"));
    }

    /// The table ranks results by score within a scenario and skips what isn't one.
    #[test]
    fn the_table_ranks_results() {
        let scratch = scratch("table");
        let result = |scenario: &str, agent: &str, score: u64| json!({"format": FORMAT, "scenario": scenario, "fingerprint": "00ff", "game_version": "0", "agent": agent, "outcome": "survived", "score": score, "hours_survived": 1.0, "houses_searched": 2, "kills": 3, "summary": ""});
        write_result(&scratch.0, &result("b_two", "low", 10)).unwrap();
        write_result(&scratch.0, &result("b_two", "high", 900)).unwrap();
        write_result(&scratch.0, &result("a_one", "only", 5)).unwrap();
        // The same agent in the same second: both kept.
        write_result(&scratch.0, &result("a_one", "only", 7)).unwrap();
        std::fs::write(scratch.0.join("notes.json"), "{\"hello\": 1}").unwrap();
        let t = table(&scratch.0).unwrap();
        let at = |s: &str| t.find(s).unwrap_or_else(|| panic!("no {s} in {t}"));
        assert!(at("a_one (fingerprint 00ff") < at("b_two (fingerprint 00ff"));
        assert!(at("high") < at("low"), "{t}");
        assert_eq!(t.matches("only").count(), 2, "{t}");
        assert!(t.contains("1 other .json file skipped"), "{t}");
        assert!(table(&scratch.0.join("nowhere")).is_err());
    }
}
