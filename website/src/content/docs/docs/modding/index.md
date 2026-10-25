---
title: How modding works
description: Zombye's game data, the --data folder, how files are checked, and what's coming.
---

Everything a modder would want to change in Zombye lives in plain text files under
[`assets/data`](https://github.com/deadcommons/zombye/tree/main/assets/data). The game reads them when it starts,
checks every one, and tells you exactly what's wrong and where if something doesn't fit. The code knows the rules; the
data says what there is.

## What's in the data folder

| File | What it holds | Guide |
|---|---|---|
| `items/items.ron` | Everything a survivor can carry: food, drink, medical, tools, clothing, bags, weapons, junk | [Items](/docs/modding/items/) |
| `items/weapons.ron` | How each weapon swings: damage, reach, arc, speed, knockback, crits, noise | [Weapons](/docs/modding/weapons/) |
| `items/recipes.ron` | What can be made from what | [Recipes](/docs/modding/recipes/) |
| `containers.ron` | Which furniture holds things, how much, how long it takes to search | [Loot and containers](/docs/modding/loot/) |
| `loot.ron` | What each container holds, by room type | [Loot and containers](/docs/modding/loot/) |
| `buildings/*.ron` | House, shop and office layouts, one file each | [Buildings](/docs/modding/buildings/) |
| `population.ron` | How many zombies live where, respawning, and how noise draws hordes in | [Zombie population](/docs/modding/population/) |
| `sounds.ron` | Which sound files play for which event, how loud and how far | [Sounds](/docs/modding/sounds/) |

The files are [RON](https://github.com/ron-rs/ron) (Rusty Object Notation): like JSON with named structs, comments
(`//`) and trailing commas. Every file starts with a comment explaining its fields, and those comments are the most
up-to-date reference there is.

## Making a mod today

A mod is a copy of the data folder with your changes, and the game started with `--data` pointing at it.

```bash
mkdir -p mods/my-mod
```

```bash
cp -R assets/data mods/my-mod/data
```

Change what you like in `mods/my-mod/data`, then play it:

```bash
cargo run -p zombye-client --release -- --data mods/my-mod/data
```

The server takes `--data` too, so an agent can [play your mod](/docs/modding/with-agents/) over MCP:

```bash
./target/release/zombye-server --mcp --data mods/my-mod/data
```

:::note[Sounds]
If your data folder has its own `sounds.ron`, the game looks for the sound files in an `audio` folder next to it
(`mods/my-mod/audio`). If you're not changing sounds, delete `sounds.ron` from your copy and the game's own sounds are
used.
:::

## Checking your data

Every file is checked when it loads: unknown fields, missing fields, ids that don't exist, numbers out of range, a
recipe that loops, a loot table naming an item that isn't there, a house with a room you can't reach. The game stops
with a message naming the file, the line and the column:

```text
failed to load content: mods/my-mod/data/items/weapons.ron: 242:5: Unexpected missing field named `damage` in `WeaponDef`
```

To check a mod without opening a window, run the server for one tick. If it prints its bench report, the data loaded:

```bash
./target/release/zombye-server --bench --ticks 1 --zombies 0 --data mods/my-mod/data
```

## Mods and saves

A save remembers what content it was made with. Load it with different data and the game sorts the changes:

- **Fine, loaded with a note:** changed numbers (a weapon hits harder, an item weighs less), new items, weapons and
  recipes, new loot (it only affects containers nobody has opened yet), population changes.
- **Refused:** a building change that changes the map, or removing an item or weapon the save still holds.
  `--load-anyway` loads it regardless, drops what no longer fits, and says what it dropped.

## Models

Items, weapons and furniture without a model are drawn as grey boxes in their `color`, so a new item works the moment
it's in the data. Models are GLB files in `assets/models`; an item's `model` field names the wearable a survivor shows
when wearing it. How we build ours in Blender is in the repository's
[asset pipeline](https://github.com/deadcommons/zombye/blob/main/docs/asset-pipeline.md) doc.

## What's coming

Modding grows with the game. Planned for the **Multiplayer and mods** milestone:

- **Data packs** that hold only your changes and layer on top of the base game (which becomes the `core` pack), so
  several mods can be used at once.
- **Scripts** in Luau (sandboxed Lua) that hook into events like a hit, a craft or a zombie spawning, and run on the
  server.
- **Mods pushed by servers:** join a modded server and your game downloads what it needs.
- **A mod index** to find and share mods.

Follow along on the [Game development board](https://github.com/orgs/deadcommons/projects/2) or in the
[Discord](https://discord.gg/y7NsmVG2br).
