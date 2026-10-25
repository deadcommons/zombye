---
title: Testing and debugging
description: Use an agent to set up scenes, run them again exactly, take screenshots and measure.
---

Playing is one use for the MCP server. The other is testing: setting up a situation, letting it run, and checking what
happened. Because time only moves when asked and the simulation is deterministic, an agent can do this precisely.

## Set up a scene

The debug tools build any situation in a few calls:

```text
skip_time {hours: 13}                                  → it is now 22:00
dev {player: 1, god: true}                             → player 1: god on
spawn_item {item: "baseball_bat", x: 378.4, y: 290.5}
pick_up {player: 1}                                    → holding the baseball bat
spawn_zombies {count: 8, x: 371, y: 291, radius: 3}
make_noise {x: 377.5, y: 290.5, radius: 20}
step_ticks {n: 70}                                     → 4 battering, 3 thumps on doors/windows
```

That's eight zombies outside a bedroom window at night, drawn by a noise, battering the glass within four seconds. In
plain words, you'd ask:

> It's night. Give player 1 god mode and a bat, put eight zombies just west of the house, make a noise in the bedroom
> and step until they're at the window. Then show me.

## Look at it

`observe` gives exact positions and states; `zombie {id}` gives one zombie's whole mind (state, target, memory,
timers, what it can see); `edge {x, y}` gives a door or window's hit points. `screenshot` renders the scene as the
game draws it, with the player's fog of war:

> Take a screenshot with `angle: 2` and `zoom: 13`.

Screenshots take about a second, change nothing in the world, and are about 1 MB each at 1280 × 720: ask for a smaller
`width` and `height` when detail doesn't matter.

## Run it again, exactly

The simulation is deterministic: the same save and the same calls give the same result, tick for tick. `stats`
reports a `state_hash` of the whole world, and every step reply ends with it.

> `save {name: "window-siege"}`. Now try three ways of holding the window: fighting at it, backing into the hall, and
> leaving by the back door. `load` the save before each, and compare how many strikes land.

This is how to tune something: change a number in the data, start the server on it, load the same save, make the
same calls, and compare.

## Measure

- `stats`: zombies by state and by simulation tier, tick timings (senses, flow fields, each tier), sounds heard last
  tick, the state hash.
- `regions`: zombie population and noise attractors per 64 m region, as heat maps.
- `loot {room, container, n}`: thousands of sample fills of a loot table, without walking anywhere.
- `zombye-server --bench` measures performance outside MCP: see the [command-line reference](/docs/reference/cli/).

## Dev toggles

`dev {player, god, ignored}`:

- **god:** the player takes no damage and can't die. Strikes still land and are reported.
- **ignored:** zombies can't see or hear the player, and those after it lose it.

Both are off in a loaded save. In the game window they're F1 and F2.

## Several players

`add_player {x, y}` brings in another player (default: beside player 1). Give its id as `player` to the other tools.
Useful for screenshots of several survivors and for seeing how zombies split between targets.
