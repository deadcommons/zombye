# Modding Zombye

A mod is a folder of data and scripts. The data (items, recipes, loot, building templates) joins the game's own; the
scripts are [Luau](https://luau.org) and give things behaviour. Scripts run inside the simulation,
on the server (in single-player, that is the game itself), in a sandbox: they can't read files, the
clock or the network, and they can't make the game stop repeating itself (same seed and inputs,
same world), which replays, saves, multiplayer and the benchmark all rely on.

```bash
# The sample mod: alarm clocks you can wind up and set down as a lure
cargo run -p zombye-client --release -- --mod mods/alarm_clock
cargo run -p zombye-server --release -- --host --mod mods/alarm_clock      # clients download its data on joining
cargo run -p zombye-server --release -- --mcp --mod mods/alarm_clock       # an agent's world with the mod
```

`--mod DIR` can be given several times; mods load in that order. A mod that doesn't load (a
mistake in its data, a script that doesn't compile) stops the game at start-up with the mod, the
file and the reason. A server and its clients play with the same mod data, and a client gets it
by itself: see [Joining a modded server](#joining-a-modded-server).

## A mod's folder

```text
mods/alarm_clock/
  mod.ron            who it is, and which scripts to run
  items.ron          items it adds or replaces                 (optional)
  recipes.ron        recipes it adds                           (optional)
  loot.ron           entries it adds to the game's loot tables (optional)
  buildings/*.ron    building templates it adds or replaces    (optional)
  alarm_clock.luau   a script
```

**`mod.ron`**

```ron
(
    id: "alarm_clock",            // lowercase letters, digits and _; names the mod in saves and logs
    name: "Wind-up alarm clocks",
    version: "1.0",               // optional
    description: "...",           // optional
    scripts: ["alarm_clock.luau"],// files in this folder, run in this order in one VM; optional
)
```

**`items.ron`** is a list in the format of `assets/data/items/items.ron` (that file's header explains
every field). An item whose `id` the game already has **replaces** it, so a mod can change an
existing thing; any other id is a new item. Two tags matter to scripts:

| Tag | Meaning |
|---|---|
| `usable` | Players can use it (right-click, **Use**; `Intent::Use`; MCP `use`). It takes a second and the item is used up. What using it *does* is up to a script's `on_use`. |
| `reusable` | The same, but the item is kept. |

Food and drink are usable anyway, and scripts hear of those uses too.

**`recipes.ron`** is a list in the format of `assets/data/items/recipes.ron`. A recipe id the game
already has is an error. A recipe with `heat: true` needs a lit campfire or a working stove in reach (cooking), and one with `places: Some(Campfire)` builds a campfire from its inputs instead of making items (it may then have no `outputs`).

**`loot.ron`** adds entries to loot tables the game has, so a new item turns up in the world:

```ron
[
    (room: "bedroom", container: "any", entries: [(item: "whistle", weight: 2)]),
]
```

`room` and `container` are a table's key in `assets/data/loot.ron`; `entries` are as there
(`item`, `weight`, `count: (min, max)`, `rare`). Adding to a table changes what its containers
hold everywhere, in containers nobody has opened yet.

**`buildings/*.ron`** are building templates in the format of `assets/data/buildings/*.ron` (file
names of letters, digits, `_` and `-`; read in name order). A template whose `id` the game already
has **replaces** it; any other is one more building the town generator may place. Either way the
town is another town: a save made without the mod no longer fits it.

## Joining a modded server

Nothing to install: a game that joins a server downloads the **data** of the server's mods first
(`items.ron`, `recipes.ron`, `loot.ron`, `buildings/*.ron`, and the mod's name and version).

- **By hash.** The server names each mod by the SHA-256 of that data (`zombye --server-info HOST`
  prints them, and the server prints them when it starts). The game takes a download only if it
  hashes to what the server named, so a damaged or swapped download is refused, not played.
- **Never scripts.** A mod's `.luau` files stay on the server, where they run. A client never
  receives or runs code from a server, and a download holding anything but those data files is
  refused whole.
- **Once.** Downloads are kept in `mods/` in the saves folder (`saves/mods/<id>-<hash>.zmod`) and
  used again while the server's mod is the same; a changed mod has another hash and is fetched
  again. Delete the folder to clear them.
- **Your own copy counts.** Started with `--mod DIR` for a mod whose data is exactly the server's,
  the game uses it and downloads nothing. Mods of yours the server doesn't run are left out while
  you play there (the game says so), and are back in your own world.
- A server with a join password hands its mods only to those who give it. A mod with more than
  2 MB of data isn't handed out: its players need it by hand, with `--mod`.
- Downloaded mods are only for playing on that server; to play a mod in your own world, get its
  folder and use `--mod`.

## Scripts

A script defines **hooks**: global functions with these names. Each is called at the end of the
tick in which the thing happened, with `game` (below) first. Mods run in load order; for each mod
the tick's events come in the order they happened, then `on_tick`.

| Hook | Called when |
|---|---|
| `on_use(game, player, item)` | a player used one of an item: ate, drank, or used a `usable` / `reusable` thing. `item` is the item's id. |
| `on_hit(game, player, zombie, damage, killed)` | a player's swing, shove or stomp hit a zombie. `zombie` is its id, `killed` a boolean. |
| `on_craft(game, player, recipe)` | a player finished a craft. `recipe` is the recipe's id. |
| `on_death(game, player)` | a player died. |
| `on_spawn(game, player)` | a survivor began: a player joined the world, or a dead one started again. |
| `on_tick(game)` | every tick (20 a second), after the events. Keep it cheap. |

`player` is a player id (a number). A hook that fails (an error, or it ran too long) is reported
(below) and the game carries on; what the hook did before failing stands.

### The `game` object

`game` is only valid during the call it was handed to: keep it in a variable and use it later, and
that use is an error. Methods are called with a colon: `game:tick()`. Every method checks what it
is given; a wrong type is a script error, a value out of range is clamped or refused as said.

**Clock**

| Method | Returns |
|---|---|
| `game:tick()` | the tick being finished (0, 1, 2, ...; 20 a second) |
| `game:seconds()` | `tick / 20` |
| `game:hour()` | the hour of the game day, 0 to 24 |
| `game:day()` | the game day, from 1 |
| `game:daylight()` | 0 at night to 1 by day |

**Random numbers**: from the world's own stream for scripts, so the same game gives the same
numbers. There is no `math.random`.

| Method | Returns |
|---|---|
| `game:random()` | a number in `[0, 1)` |
| `game:random(n)` | a whole number from 1 to `n` |
| `game:random(a, b)` | a whole number from `a` to `b` |

**The store**: the only place a script may keep anything between calls (see Rules).

| Method | Does |
|---|---|
| `game:get(key)` | what this mod stored under `key`, or `nil` |
| `game:set(key, value)` | stores it; `nil` forgets the key |

A stored value is a boolean, a finite number, a string (at most 1,024 bytes), or a table of those
that is either a list (keys `1..n`, no gaps) or a record (string keys), nested at most 8 deep.
Keys are 1 to 64 bytes. A mod's whole store is at most about 64 KB. Anything else is an error.
`get` returns a copy: change it, then `set` it again. Each mod has its own store.

**Players**

| Method | Does |
|---|---|
| `game:players()` | a list of the ids of the players in the world (dead ones included) |
| `game:player(id)` | `nil`, or a table: `id`, `x`, `y` (metres; `+y` is south), `facing` (radians), `hp`, `max_hp`, `dead`, `asleep`, `indoors`, `endurance` (0 to 1), `hunger`, `thirst`, `tiredness` (0 fine to 1 worst) |
| `game:heal(id, hp)` | gives hit points back, never above full. Returns whether it did. |
| `game:hurt(id, hp)` | takes hit points (it can kill; god mode holds). Returns whether the player died. |
| `game:set_need(id, need, value)` | sets `"hunger"`, `"thirst"` or `"tiredness"` to `value` (clamped to 0..1) |
| `game:set_endurance(id, value)` | sets endurance (clamped to 0..1) |
| `game:tell(id, text)` | shows the player a line (the yellow line under the HUD; at most 160 characters) |

**Items**

| Method | Does |
|---|---|
| `game:item(item)` | `nil`, or a table about that item id: `id`, `name`, `category` (`"food"`, `"junk"`, ...), `weight_kg`, `tags` |
| `game:count(id, item)` | how many of `item` player `id` carries in pockets and bags |
| `game:give(id, item, count)` | puts `count` (default 1, at most 100) into the player's pockets. Returns whether it did. |
| `game:take(id, item, count)` | takes up to `count` (default 1) out of the player's pockets and bags. Returns how many. |
| `game:spawn_item(item, x, y, count)` | puts `count` (default 1) on the ground there. Returns the ground item's id, or `nil` (unknown item, off the map). |
| `game:ground_item(ground_id)` | `nil` once it is gone (picked up, removed), else a table: `id`, `item`, `count`, `x`, `y` |
| `game:remove_item(ground_id)` | takes a ground item away. Returns whether it was there. |

**The world**

| Method | Does |
|---|---|
| `game:noise(x, y, radius, lasting)` | a noise zombies within `radius` metres hear (at most 150; less through walls) on the next tick, and come to look at. 25 m and more also draws the far horde over the next minutes; `lasting = true` keeps drawing it for half an hour, like an alarm left on. Returns whether it was made. |
| `game:zombies_near(x, y, radius)` | how many living zombies are within `radius` metres |
| `game:spawn_zombies(x, y, count, radius)` | up to `count` (default 1, at most 50 a call) zombies within `radius` metres (default 3). Returns how many were placed. |

`print(...)` writes a line to the game's or server's standard error, with the mod's id.

### What a script has, and hasn't

Luau's `string`, `table`, `math` (without `random` and `randomseed`), `utf8`, `bit32` and `buffer`
libraries and the basic functions (`pairs`, `ipairs`, `pcall`, `tostring`, `type`, ...). Not there:
`os`, `io`, `debug`, `require`, `loadstring`, `getfenv`, `setfenv`, `collectgarbage`, `newproxy`.
The libraries are read-only. Each mod runs in its own VM: mods can't see each other's globals or
stores.

**Limits.** One hook call (and loading a mod's scripts) may use 100,000 "steps": Luau counts one
at every loop iteration and every function call. A call that uses them up is stopped with a "ran
too long" error (catching it with `pcall` doesn't buy more). It is a count, not a clock, so a
script is cut off at the same point on every machine. A mod's VM may hold 16 MB.

**Errors** are printed to standard error (the first five from each mod) and the latest 32 are kept
in the world (`World::script_errors`). A hook that fails is called again next time: one bad call
doesn't switch a mod off.

## Rules for scripts that keep the game deterministic

The sandbox removes the ways to read the outside world. Two things are left to the script:

1. **Keep state in the store, never in script variables.** A save holds the store, not the VM: a
   world that is loaded (or a new run, or a server restart) starts the scripts afresh. A counter in
   a `local` at the top of the file is back at its first value after a load, while the world it
   counted isn't, and from then on the loaded game is not the game that was saved. Constants and
   helper functions at the top of the file are fine.
2. **Don't let table addresses decide anything.** `tostring` of a table or function shows where it
   is in memory, which differs from run to run, and so does the order `pairs` walks a table whose
   *keys* are tables or functions. Use `ipairs` over lists, and string or number keys.

Follow those and a world with your mod repeats exactly: the store, the scripts' random stream and
events not yet delivered are in saves and in the world's state hash.

Also worth knowing:

- Hooks run at the end of the tick. What a hook changes is seen by players in the next snapshot; a
  noise it makes is heard by zombies on the next tick.
- An event your hook causes (a script killing a player) is delivered on the next tick.
- Time skipped at once (the debug `skip_time`) runs no hooks; sleeping does (the clock runs faster,
  ticks still happen).
- Clients predict their own movement. A script can't move players; what it changes about them
  (needs, endurance, hit points) reaches the client with the next snapshot.
- In multiplayer, scripts run on the server only. Clients need the mod for its data.

## The sample mod: `mods/alarm_clock`

The game has an alarm clock: junk, found in bedrooms. The mod gives it a use.

- `items.ron` replaces `alarm_clock` with the same item plus the `usable` tag, and adds
  `alarm_clock_set`, a wound-up clock.
- `alarm_clock.luau`: `on_use` puts a wound-up clock on the ground at the player's feet and stores
  where it is and when it rings; `on_tick` makes a 45 m noise at each stored clock once a second
  from 10 s to 30 s after it was set, forgets a clock that was picked up, and swaps a run-down one
  for an ordinary clock.

In game: find an alarm clock (or `spawn_item` one over MCP), right-click it in the inventory,
**Use**, and walk away. `cargo test -p zombye-mods --release` plays it: a dozen zombies 25 m off
come to the clock, and none do when it stays in the pocket.

## Saves

A save made with mods holds what their scripts stored. Loading it:

- with the same mods: carries on exactly;
- with a mod's script changed: loads, with a note; the new script carries on from the old store;
- without a mod it was made with: its store is kept, its scripts don't run, and if the save holds
  an item only that mod has it is refused (or loaded without those items with `--load-anyway`);
- with a mod it wasn't made with: loads, with a note; the mod's scripts start then.

## How it works (for contributors)

- `crates/sim/src/script.rs`: the hook. `sim` has no Luau in it (it stays engine-free): it defines
  `ScriptEngine` (in `WorldConfig::scripts`; makes a `ScriptHost` for every world), the events a
  tick collects (`ScriptEvent`), the functions scripts change the world through (`World::script_*`)
  and what they keep (`ScriptState`: saved in the optional `MODS` section, hashed once non-empty).
  `World::step` runs the host last, whoever steps the world. A world without mods does none of it
  and hashes and saves as before.
- `crates/mods`: reads mod folders (`Mod::load`), merges their data into a `WorldConfig`
  (`apply`), and runs the scripts (`luau.rs`: `mlua` with Luau built from source; one VM a mod).
- Download on join: `Mod::pack` is a mod's data as it goes over the network
  (`zombye_net::modpack::ModPack`, its hash, the client's cache, `gather`), `Mod::from_pack` /
  `apply_packs` what a client makes of it. A new kind of data file needs a name in
  `modpack::data_file`, and a line each in `Mod::pack`, `Mod::from_pack` and `apply`.
- To add an API function: a checked `script_*` function on `World`, a method in
  `luau::register_game`, a row here, and a line in `crates/mods/tests/mods.rs`.
- To add a hook: a `ScriptEvent`, pushed where the thing happens (`World::script_event`), a name in
  `luau::HOOKS` and `Hooks`, a row here.
