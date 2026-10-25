---
title: Modding with an agent
description: Have an AI agent write a mod, check it, play it and show you the result.
---

Zombye was built so that "make me a mod that adds a nail bat" can be a request to an agent. The data files are plain
text with their fields explained at the top, the game checks them and says what's wrong, and the MCP server lets the
agent play what it made.

## Set up

Open Claude Code (or another coding agent) in the repository. It reads [`AGENTS.md`](https://github.com/deadcommons/zombye/blob/main/AGENTS.md)
for the repo's rules and layout. Make sure the binaries are built:

```bash
cargo build --release -p zombye-server -p zombye-client
```

## Ask for the mod

Be clear about where it goes and how it should be checked:

> Make a mod in `mods/scarce` that makes the world harder: half the loot, twice the zombies in town, and no water in
> fridges. Copy `assets/data` there, change only what's needed, and check it loads with
> `zombye-server --bench --ticks 1 --zombies 0 --data mods/scarce/data`. Then tell me exactly what you changed.

A good agent reads the header comments of `loot.ron` and `population.ron`, finds `scale` in both, edits the fridge
tables, and runs the check.

## Let it play the mod

Add a second MCP server for the mod, so the agent can try it:

```bash
claude mcp add scarce -- ./target/release/zombye-server --mcp --data mods/scarce/data
```

Then:

> Using the scarce server, compare loot: run `loot` for a kitchen fridge and a kitchen cupboard with 2000 samples,
> and tell me how often water turns up. Then search every container in the starting house and list what you found.

For anything visual, a building template for example, ask for screenshots:

> Find a building made from my new template with `buildings`, and take a `screenshot` centred on it from all four
> angles.

## Ideas to try

- **A new house:** "Add a two-bedroom duplex template. Keep it under 13 × 15 tiles. Check every room is reachable and
  screenshot it in town."
- **A new recipe chain:** "Let survivors make a spear from a wooden plank and a kitchen knife with duct tape. Add the
  weapon, the item and the recipe, and test it on a zombie."
- **A sandbox preset:** "A 'lone wolf' mod: zombies respawn, migration reaches twice as far, loot is scarce."
- **Balance work:** "Is the frying pan better than the claw hammer? Spawn ten zombies for each, fight them with god
  mode on from the same save, and compare kills per minute and endurance."

## Keep it honest

- Ask the agent to say what it changed, file by file. The changes are small text diffs you can read.
- The game's checks catch mistakes in the data, not in the idea: play it yourself before sharing it.
- If the mod uses art or sounds, they must be yours to share. Share mods in the [Discord](https://discord.gg/y7NsmVG2br).
