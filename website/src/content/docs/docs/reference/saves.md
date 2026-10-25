---
title: Saves
description: Where saves go, what they hold, and how they cope with changed game data.
---

## In the game

| Key or option | What it does |
|---|---|
| **F5** | Quicksave to `quicksave.zsave`. |
| **F9** | Quickload it, like rejoining. |
| Closing the window | Autosaves to `autosave.zsave`. So does Ctrl-C in the terminal. |
| `--load autosave` | Start from a save by name, or `--load path/to/file.zsave`. |
| `--saves DIR` | Where saves go (default `saves/` in the folder you run from). |

A quickload of a different town (another seed or map size) is refused: start with `--load` for that.

## On the server

`--save PATH` loads that file if it's there, or makes a new world and saves it there. It autosaves every
`--autosave-mins` minutes of real time when something changed, and on shutdown. Over MCP, `save {name}` and
`load {name}` use named saves in the `--saves` folder, and `saves` lists them.

## What's in a save

Everything the future depends on: every zombie on every tier with its memory, players with their wounds, needs,
skills and inventories, doors and windows and their damage, items on the floor, opened containers, the clock and
both random streams. A loaded world carries on exactly as the saved one would have: the same state hash, and the same
future for the same moves.

The town itself isn't stored. It's regenerated from the seed and the building templates, and the doors and windows
changed since are applied on top. That keeps a save small: about 270 KB for the default map with nearly 3,000
zombies, and 2 MB for a 1 km map with 22,500.

## Saves and changed data

A save remembers the game data it was made with. If the data has changed since (a new version, or a
[mod](/docs/modding/)):

- **Compatible changes load with a note:** weapon and item numbers, new items, weapons and recipes, loot tables
  (unopened containers fill from the new ones), population settings.
- **Incompatible ones are refused:** a building template that changes the map, or a removed item or weapon the save
  still uses. `--load-anyway` (or `load {force: true}` over MCP) loads it regardless: it drops what no longer fits,
  moves players and zombies off ground that's now blocked, and says what it did.

Damaged and tampered files are refused too, with a reason, never a crash. Saves from much older versions of the game
may not load while Zombye is in pre-alpha.
