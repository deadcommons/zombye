# The agent survival benchmark

Fixed scenarios to compare AI agents (or people) on, played over the game's MCP server, each run
ending in a results file (#29). Everything runs locally: there is no hosted leaderboard.

A scenario is a seed, a map, a zombie count, an hour, a start point and what the survivor starts
with. The simulation is deterministic and time only moves when a tool steps it, so the same calls
give the same run, down to the world's state hash: a result can be checked by playing it again.

## Running it

```bash
cargo build --release -p zombye-server          # and -p zombye-client, for the screenshot tool

# The scenarios
./target/release/zombye-server --benchmark list

# An agent plays one over MCP (stdio). Register this command with your MCP client:
./target/release/zombye-server --mcp --benchmark first_night --agent my-agent
# e.g. Claude Code:  claude mcp add zombye-bench -- $PWD/target/release/zombye-server --mcp --benchmark first_night --agent claude

# ...or over HTTP, for agents that can't start a local program (see AGENTS.md, Connecting an agent)
ZOMBYE_MCP_TOKEN=a-long-secret-token ./target/release/zombye-server --mcp-http --benchmark siege --agent my-agent

# A built-in script plays it instead, headless (reference scores; a second or so each)
./target/release/zombye-server --benchmark all --baseline runner

# The results so far, best first
./target/release/zombye-server --benchmark table
```

`--agent NAME` is the label in the results (default `unnamed`). `--results DIR` is where results go
and where `table` reads them (default `saves/benchmark`, git-ignored). `--data DIR` plays the
scenarios of another data folder (its `scenarios.ron`).

One process is one run: start the server again for another.

## The rules of a run

- **One life.** The run ends when the survivor dies or the scenario's game hours are up. From then
  on time stands still: tools that act or step time are refused, the read-only ones still answer.
  A tool call that runs past the end (a long `step_ticks`, a walk, a sleep) stops on the tick the
  run ended, so nothing after it counts. `respawn` is refused.
- **A player's tools only.** The debug tools (`skip_time`, `spawn_item`, `dev`, `set_needs`,
  `load`, ...) and the saves folder's `save` / `saves` aren't offered, as on a public server
  (`--mcp-no-dev`). Everything else is as AGENTS.md describes it: `observe`, `move_player`,
  `attack`, `search`, `take`, `use`, `treat`, `sleep`, `screenshot`, ...
- **The `benchmark` tool** (read-only, only on a benchmark server) returns JSON: the scenario and
  its brief, the rules, game hours survived and left, the score so far and its parts, and once the
  run is over the result. The server's instructions tell the agent which scenario it is in and to
  call it.
- **Time** is game hours on the world's clock: one game hour is 150 s of game time, 3,000 ticks.
  Sleeping runs the clock 30 times faster, as in the game, and counts.
- **The answer that ends the run** says so (`BENCHMARK OVER: ...`) and where the result was
  written. An agent that goes away before the end (stdin closed, Ctrl-C) gets a result with
  `outcome: "unfinished"` and its score so far.

Every agent has the same tools, so runs compare. Note that the MCP tools show more than a player's
screen does (`observe` draws zombies behind walls, `zombie` reads one's mind): a fog-of-war-fair
view for agents is its own piece of work.

## The score

```
score = survive x (hours survived / hours) + house x buildings searched + kill x zombies killed
```

`survive`, `house` and `kill` are the scenario's `points` (defaults 1000, 20, 5). A building counts
once a container in it has been searched (any building: a shop or an office too). Dying keeps the
share of `survive` earned up to then.

## The scenarios

They are data: `assets/data/scenarios.ron` (the file's header has every field), read by
`zombye_sim::scenario`. All four are on the default 480 m map.

| id | What | Start | Zombies | Hours | Points (survive, house, kill) |
|---|---|---|---|---|---|
| `first_night` | The game's first loop: search the house, stay or go, live until 06:00 | seed 1, the bedroom, 14:00, nothing | the map's own (about 2,950) | 16 | 1000, 20, 5 |
| `scavenger` | Search as many buildings as you can in a thinly haunted town | seed 7, the bedroom, 09:00, a bottle of water | 700 | 3 | 400, 50, 0 |
| `last_stand` | In the open in the middle of town with sixty closing in on a noise | seed 3, the middle of town, 12:00, a baseball bat, two rags | the map's own + 60 round the start | 1 | 1000, 0, 10 |
| `siege` | Dark, in the bedroom, and a lasting noise brings the neighbourhood to the house | seed 5, the bedroom, 21:00, a claw hammer, a flashlight, gauze, water | the map's own + 25 outside | 2 | 1000, 20, 5 |

The world's own zombies within 12 m of the start are taken out as a run begins; a scenario's own
hordes are put where it says.

**Changing a scenario changes its fingerprint** (below), and results from before no longer
compare: add a new scenario under a new id instead. To add one, add an entry to the file; `--data`
with your own file needs no rebuild.

## Results

One JSON file a run, `<scenario>-<agent>-<YYYYMMDD-HHMMSS>.json` in the results folder. An
example (the numbers are for show):

```json
{
  "format": "zombye-benchmark/1",
  "scenario": "scavenger",
  "scenario_name": "Scavenger",
  "fingerprint": "c11fb16f3e61aed8",
  "game_version": "0.0.1",
  "agent": "my-agent",
  "outcome": "survived",
  "score": 400,
  "points": { "survive": 400, "houses": 0, "kills": 0 },
  "hours_survived": 3.0,
  "hours": 3.0,
  "houses_searched": 0,
  "kills": 0,
  "containers_searched": 0,
  "items_taken": 0,
  "distance_m": 1234.5,
  "wounds": 0,
  "hp_lost": 0.0,
  "hp": 100.0,
  "cause_of_death": null,
  "ticks": 9000,
  "state_hash": "bf62124dfacd4837",
  "summary": "survived 3.00 of 3.00 game hours, 0 houses searched, 0 kills: 400 points (400 + 0 + 0)",
  "tool_calls": 212,
  "actions": 150,
  "started": "2026-11-10 01:29:11Z",
  "wall_secs": 431.5
}
```

| Field | Meaning |
|---|---|
| `format` | `zombye-benchmark/1`; bumped when a field changes meaning or goes (new fields don't bump it) |
| `scenario`, `scenario_name` | The scenario's id and name |
| `fingerprint` | A hash of the scenario's definition, the game data (templates, street props, weapons, items, containers, loot, population, recipes) and the generated map. **Only results with the same fingerprint and `game_version` compare**; `table` groups by it |
| `agent` | `--agent NAME`; `baseline:NAME` for a built-in script |
| `outcome` | `survived` (alive when the hours were up), `died`, or `unfinished` (the agent left first) |
| `score`, `points` | The score and its three parts |
| `hours_survived`, `hours` | Game hours survived, of the scenario's |
| `houses_searched`, `kills` | What the score counts |
| `containers_searched`, `items_taken`, `distance_m`, `wounds`, `hp_lost`, `hp` | The run's other numbers (the game's run stats); `hp` is 0 dead |
| `cause_of_death` | In plain words, or null |
| `ticks`, `state_hash` | The world's tick and state hash at the end: the same calls give the same pair |
| `summary` | One line for people |
| `tool_calls`, `actions` | MCP runs only: every tool call, and those that acted or stepped time |
| `started`, `wall_secs` | MCP runs only: when the server started (UTC) and how long the run took in real time |

Everything but the last two rows follows from the scenario and what the player did.

## Baselines

Three built-in scripts (`--baseline NAME`, `crates/server/src/benchmark.rs`) give reference
scores. They send only the intents a client sends, but decide by reading the world directly,
which an agent can't: they are a floor to beat, not competitors. `idle` does nothing. `brawler`
stands its ground against anything within 9 m, swings when it is in reach and stamps on what it
floors, and wanders when nothing is near. `runner` keeps away from every zombie within 18 m and
never fights or searches.

Scores with the game data as of this file (run them again after changing any of it):

| Scenario | idle | brawler | runner |
|---|---|---|---|
| `first_night` | 26 (died, 0.41 h) | 48 (died, 0.69 h, 1 kill) | 26 (died, 0.41 h) |
| `scavenger` | 113 (died, 0.85 h) | 38 (died, 0.29 h) | 400 (survived) |
| `last_stand` | 95 (died, 0.10 h) | 98 (died, 0.10 h) | 286 (died, 0.29 h) |
| `siege` | 68 (died, 0.14 h) | 97 (died, 0.17 h, 2 kills) | 73 (died, 0.15 h) |

## Not here

- A public leaderboard, or any way to submit results: a result is a file, to share however you
  like. Since runs are deterministic, a claimed result can be checked by replaying its calls; the
  calls themselves aren't recorded yet.
- A view cut down to what the survivor could see (above).
