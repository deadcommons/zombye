---
title: Command-line options
description: Every option of the zombye game and the zombye-server.
---

## The game: `zombye`

```bash
cargo run -p zombye-client --release -- [options]
```

### The world

| Option | What it does |
|---|---|
| `--seed S` | Which town. The same seed always makes the same town. |
| `--zombies N` | How many zombies. Without it, the number `population.ron` gives for the map (about 2,950 on the default map). |
| `--map-chunks N` | Map size: N × 16 m a side. Default 30 (480 m); 64 is about 1 km. More map is more forest round the same town. |
| `--time HOUR` | The starting hour, 0 to 24. |
| `--at X,Y` | Start somewhere other than the bedroom. |
| `--data DIR` | Read game data from another folder (a [mod](/docs/modding/)). |

### Saves

| Option | What it does |
|---|---|
| `--load NAME\|PATH` | Start from a save: a name in the saves folder (`--load autosave`) or a path. Its own seed and map apply. |
| `--load-anyway` | Load a save whose game data changed in a way it can't simply take. Says what it dropped. |
| `--saves DIR` | The saves folder (default `saves/`). |
| `--player ID` | Play as that player of a loaded save. |

### Camera and sound

| Option | What it does |
|---|---|
| `--zoom METRES` | Metres of world visible top to bottom, 8 to 90 (default 26). |
| `--angle N` | Start at one of the four camera angles, 0 to 3. |
| `--rotate` | Allow turning the camera with `,` and `.`. |
| `--novsync` | Don't wait for the display. |
| `--mute`, `--volume V` | No sound, or a volume from 0 to 2. |

### Testing and pictures

| Option | What it does |
|---|---|
| `--fight-demo` | Start outside with a bat and six zombies coming. |
| `--siege` | Start outside a house with a bat and five zombies battering its door. |
| `--autoplay` | The player fights by itself (for watching and testing). |
| `--sneak`, `--god`, `--ignored`, `--inventory` | Start sneaking, in god mode, ignored by zombies, or with the inventory open. |
| `--screenshot PATH`, `--screenshot-after SECS[,SECS...]`, `--exit-after SECS` | Save screenshots from the window, then quit. |
| `--render-to PATH`, `--size WxH`, `--center X,Y` | Render one picture offscreen, with no window, and exit. Usually with `--load`. |

## The server: `zombye-server`

```bash
./target/release/zombye-server (--mcp | --bench ...) [options]
```

| Option | What it does |
|---|---|
| `--mcp` | Speak MCP over standard input and output, for an [agent](/docs/agents/). |
| `--seed S`, `--zombies N`, `--map-chunks N`, `--data DIR` | The world, as for the game. |
| `--save PATH` | Load that save if it exists, else make a new world and save it there. Autosaves, and saves on exit (closing stdin, Ctrl-C, SIGTERM). |
| `--autosave-mins N` | Minutes of real time between autosaves (default 5; 0 is off). |
| `--load-anyway` | As for the game. |
| `--saves DIR` | The folder for the `save` and `load` tools. |
| `--client PATH` | The game binary the `screenshot` tool runs (default: `zombye` next to the server, or `$ZOMBYE_CLIENT`). |
| `--screenshot-timeout SECS` | How long a screenshot may take (default 60). |
| `--no-debug-tools` | With `--mcp`: leave out the [debug tools](/docs/agents/tools/#debug). They aren't listed and can't be called. |

### Benchmarks

```bash
./target/release/zombye-server --bench --zombies 5000
```

| Option | What it does |
|---|---|
| `--bench` | Run the simulation flat out and report timings. |
| `--ticks N` | How many ticks (default 1,000). |
| `--players N` | Players running in circles making noise. Without it, every zombie heads for one point. |
| `--horde N` | N more zombies round the first player. |
| `--save-bench` | Also time saving and loading the world. |

A big one: a 1 km map with 21,500 zombies and a horde of 1,000 on a player.

```bash
./target/release/zombye-server --bench --ticks 400 --map-chunks 64 --zombies 21500 --players 1 --horde 1000
```
