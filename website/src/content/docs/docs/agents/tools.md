---
title: MCP tool reference
description: Every tool the Zombye MCP server offers.
---

Every tool replies in plain text written for a model to read (`stats` replies in JSON). Tools that act take a `player`
id, default 1. Tools that take time step the world until the action is done, then report what happened in the
meantime: hits, strikes on players, thumps on doors, anything broken, deaths and their cause, skill level-ups.

Positions are in metres; one tile is one metre. Facing is in radians, or a compass `direction` where a tool takes one.
There are 20 ticks to a second.

## What the server tells your app

Besides its name, description and arguments, every tool has a `title` (a short name for people) and `annotations`:

- **Read-only tools** (`readOnlyHint: true`): everything under Looking, the tables (`weapons`, `items`, `loot`,
  `recipes`), `inventory`, `containers_near`, `health`, `skills` and `saves`. They take no game time and change
  nothing, so apps that ask before each call can let these through without asking.
- **Debug tools**: their titles start with `Debug:`, their descriptions with "Debug tool, for setting up tests", and
  they carry `_meta: {"zombye/debug": true}`. They are the tools under [Debug](#debug), and `load`.
- Only `save` (it replaces a save of the same name) and `load` (it replaces the world) are marked destructive. No tool
  reaches outside the game (`openWorldHint: false`).

`stats` has an `outputSchema`, and returns its JSON as `structuredContent` as well as text.

The server also sends a few lines of **instructions** when an app connects, which most apps pass on to the model: you
are player 1, time only moves when a tool steps it, look with `observe` first, and the debug tools are for testing.

Start the server with `--no-debug-tools` to leave the debug tools out: they aren't listed and can't be called. That
makes an agent play by the rules, and brings the list down from 47 tools to 37 for apps that cap how many they take.

The server speaks MCP's stateless revision (2026-07-28: no `initialize`, `server/discover` describes the server, each
request names its version) and the older handshake revisions (2024-11-05 to 2025-11-25) side by side, so both new and
old apps connect.

## Looking

| Tool | Arguments | What it does |
|---|---|---|
| `observe` | `radius`, `x`, `y` | A text map around player 1 (or a point), then the players, items on the floor, damaged doors and windows, and the nearest zombies with their state. |
| `screenshot` | `player`, `x`, `y`, `zoom`, `angle`, `width`, `height` | A rendered PNG as the game draws it, with the player's fog of war and the walls between the camera and the player cut away. No HUD. 960 × 540 unless you give `width` and `height` (up to 1920 × 1080). About a second; changes nothing. |
| `stats` | | JSON (as text and as structured content): time, players (health, needs, wounds, skills, this run's numbers, cause of death), zombies by state and tier, population, tick timings, the state hash. |
| `zombie` | `id` | One zombie's tier, health, state, target, memory and timers, and what it can see of each player. |
| `edge` | `x`, `y` | The four edges of a tile: wall, window, door, arch or fence, and hit points for doors and windows. |
| `buildings` | | Every building: template, bounds, storeys, ground-floor rooms. Marks the one you wake up in. |
| `regions` | | Zombie population, targets and noise attractors per 64 m region, as text heat maps. |

### The map legend

| Symbol | Meaning |
|---|---|
| `-` `\|` | Walls |
| `=` `!` | Shut windows |
| `o` `x` | Open and broken windows (on a wall line) |
| `D` `d` | Shut door, damaged shut door |
| `/` `#` | Open door, broken door |
| `~` | Fence |
| `.` `,` `:` `_` | Grass, road, sidewalk or path, indoor floor |
| `T` `*` | Tree, bush |
| `f` `c` `o` | Furniture, an unsearched container, a searched container (on a tile) |
| `w` `i` | A weapon, any other item on the floor |
| `P` `X` | A player, a dead player |
| `@` `G` | Where the player woke up, the debug goal |
| `Z` `I` `C` `L` `A` `B` | Zombies wandering, investigating, chasing, lunging, attacking, battering |
| `S` `v` `%` | Staggered, down, a corpse |
| `n` `?` | Zombies on the cheaper near and far simulation tiers |

## Moving and the world

| Tool | Arguments | What it does |
|---|---|---|
| `step_ticks` | `n` | Let time pass. |
| `move_player` | `player`, `direction` or `dx`/`dy`, `ticks`, `run`, `sneak`, `facing` | Hold a movement input for some ticks, then stop. Walk 3 m/s, run 5.5, sneak 1.4. |
| `interact` | `player` | Open or shut the door or window the player faces, within 1.6 m. |
| `smash_window` | `player` | Smash the window the player faces. Loud. |
| `sleep` | `player`, `max_hours` | Lie down (bed, sofa or floor) and sleep until something wakes the player. Says why it woke. |
| `respawn` | `player` | A dead player gets up in the bedroom, with full health and empty hands. |
| `say` | `player`, `text`, `volume` | Says a line to whoever is near: `whisper` (3 m), `talk` (12 m, the default) or `shout` (40 m); 0.4 of that through a wall. Zombies hear it too (1.5, 7 and 30 m). Time doesn't move. |
| `heard` | `player`, `secs` | Read-only. What the player heard said lately: how long ago, how loud, muffled or not, from where, and from whom when known (with PvP on, only if it saw the speaker). |
| `gesture` | `player`, `gesture` | `wave`, `hands_up` (held until lowered), `point`, or `none`. Others see it; `observe` and `stats` show it. |

## Fighting

| Tool | Arguments | What it does |
|---|---|---|
| `attack` | `player`, `facing` or `target` | Swing what's in hand (or punch). Turns to `target` (a zombie id) first if given. |
| `shove` | `player`, `facing` or `target` | Push back up to three zombies in front. No damage; always staggers. |
| `stomp` | `player`, `facing` or `target` | Stamp on a downed zombie at the player's feet. |
| `weapons` | | The weapon table as the game read it. |

## Things

| Tool | Arguments | What it does |
|---|---|---|
| `inventory` | `player` | Hands, back, shoulder, worn, carried (with indices), load, the open container, the floor in reach, containers in reach, queued jobs. |
| `containers_near` | `player`, `radius` | Containers nearby: id, kind, room, distance, searched or not, and what's in them or the table they'll fill from. |
| `search` | `player`, `container` | Open a container in reach (by id, or the one faced). Lists what's inside. |
| `take` | `player`, `from`, `index`, `count` | Take from the open container (`from: "container"`) or the floor (`from: "ground"`, index = ground item id). |
| `pick_up` | `player` | The instant grab: the item on the floor in front. A weapon goes into the hand. |
| `move_item` | `player`, `from`, `index`, `to`, `count` | Move a stack between `carried`, `hand`, `off_hand`, `back`, `shoulder`, `worn`, `container`, `ground`. |
| `equip` | `player`, `index` | Put a carried stack where it belongs: a bag on, a weapon in hand. |
| `wear` | `player`, `index` | Put on carried clothing. |
| `unequip` | `player`, `from`, `index` | Take off or let go into the bags (or the floor). |
| `drop` | `player`, `from`, `index`, `count` | Drop at the player's feet. |
| `use` | `player`, `from`, `index` | Eat or drink one go. Says beforehand what it will do, then the needs before and after. |
| `items` | `category` | The item table. |
| `loot` | `room`, `container`, `n` | A loot table, and `n` sample fills of it. |

## Health and crafting

| Tool | Arguments | What it does |
|---|---|---|
| `health` | `player` | Health, every wound (with its `#` id), what each needs, pain, illness, needs and status tags. |
| `treat` | `player`, `action`, `wound` or `part`, `item` | First aid: `dress`, `clean`, `stitch`, `remove`, `painkillers`, `antibiotics`. Picks the most urgent wound and the best item if not told. |
| `skills` | `player` | Each skill's level, XP, what it gives now and next, and how it's earned. |
| `recipes` | | Every recipe. |
| `craft` | `player`, `recipe` | Without a recipe: what the player can make and what's missing for the rest. With one: makes it. |

## Saves

| Tool | Arguments | What it does |
|---|---|---|
| `save` | `name` | Save the world to a named save in the saves folder, or with no name to the `--save` file. |
| `load` | `name`, `force` | Replace the world with a save. `force` loads one whose game data changed in a way it can't simply take. |
| `saves` | | The saves folder, and where this session autosaves. |

## Debug

For testing and modding, not for fair play. `--no-debug-tools` leaves them out.

| Tool | Arguments | What it does |
|---|---|---|
| `dev` | `player`, `god`, `ignored` | God mode, and zombies ignoring the player. |
| `spawn_item` | `item`, `count`, `x`, `y` | Put items on the floor. |
| `spawn_zombies` | `count`, `x`, `y`, `radius` | Put zombies on open tiles in a circle. |
| `make_noise` | `x`, `y`, `radius`, `lasting` | A noise. A loud one (25 m or more) also draws far zombies in over minutes. |
| `set_goal` | `x`, `y` | Every zombie within 120 m heads for a point, senses off. No arguments goes back to normal. |
| `skip_time` | `hours` | Move the clock on. |
| `set_needs` | `player`, `hunger`, `thirst`, `tiredness` | Set needs, 0 fine to 1 worst. |
| `wound` | `player`, `part`, `kind`, `infected` | Give the player a wound. |
| `add_player` | `x`, `y` | Another player joins. |
| `load` | `name`, `force` | See [Saves](#saves): it swaps the whole world, so it goes with the debug tools. |

For scale when making noise: walking is 6 m, running 14, a door 10, a zombie groaning 9, a zombie thumping a door 15,
breaking glass 22, breaking a door 26.
