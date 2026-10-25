---
title: 'Tutorial: an agent survives'
description: A first session with an AI agent playing Zombye over MCP, prompt by prompt.
---

This tutorial walks you through a first session with an agent: it wakes up, looks around, loots the house, arms
itself, and deals with the first zombie. It uses Claude Code, but the prompts work in any MCP client.

You need the server [connected](/docs/agents/connect/). Start it with a save file, so the run survives restarts and
you can go back to it:

```bash
claude mcp add zombye -- ./target/release/zombye-server --mcp --zombies 1000 --save saves/agent.zsave
```

## 1. Set the rules

The MCP server is a developer tool, so it has debug tools: spawning items and zombies, god mode, skipping time. Tell
the agent what's fair before it starts.

> You're playing Zombye through the zombye MCP tools, as player 1. Play fair: don't use `spawn_item`,
> `spawn_zombies`, `set_goal`, `dev`, `wound`, `set_needs`, `skip_time` or `respawn`, and don't `load` a save after
> you die. Time only moves when you call a tool that steps it, so take your time to think. Your goal is to survive
> as long as you can. Start by calling `observe` and telling me where you are.

## 2. Reading the map

`observe` returns a text map at double resolution: tiles sit between wall lines. The agent will see something like
this, with itself as `P` in the main bedroom:

```text
                +-+-+-+=+-+=+=+-+-+-+-+D+-+
 , , , , : : . .|c f f _|c c c c c|c c _ _|
                +       +         +       +
 , , , , : : . .|_ f f _|c _ _ _ _/_ _ _ _!
                +       +         +       +
 , , , , : : . .!_ _ P _|c _ f f _|_ _ _ c|
                +       +         +-+/+-+-+
 , , , , : : . .|_ _ _ _|_ _ _ _ _|c _ c c|
                +-+-+-+/+-+ + +-+-+       +
```

- `-` `|` walls, `=` `!` shut windows, `D` shut doors, `/` open doors.
- `_` indoor floor, `.` grass, `,` road, `:` sidewalk.
- `c` a container nobody has searched, `f` other furniture, `w` a weapon on the floor, `i` another item.
- Zombies by what they're doing: `Z` wandering, `I` investigating a noise, `C` chasing, `L` lunging, `A` attacking,
  `B` battering a door or window.

Under the map come the players, items on the floor and the nearest zombies, with positions in metres. The
[tool reference](/docs/agents/tools/) has the full legend.

## 3. Loot the house

> Find every container in this house with `containers_near` and search them, one room at a time. Take water, food that
> keeps, anything medical and the best weapon you find. Tell me what you skipped and why.

What to expect:

- **Reach means the same room.** `containers_near` lists the kitchen cupboard 0.4 m away, through the wall. Searching
  it from the bedroom is refused (`refused: the kitchen cupboard is out of reach`), so the agent has to walk round
  through the doors. A good agent works this out from the map; if yours keeps trying, point at the doors.
- **Not every cupboard has something in it.** `searched the wardrobe: empty (41 ticks, 2.0 s)`.
- **Moving is in compass directions and ticks.** There are 20 ticks to a second and walking is 3 m a second, so
  `move_player {direction: "s", ticks: 20}` walks about three metres south. Agents tend to overshoot at first, and
  learn quickly from where the reply says they ended up.

When it's done, ask for the inventory: `inventory` shows hands, bags and load.

## 4. Get ready

> Hold your best weapon. If you found a bag, put it on. Then tell me your plan for leaving the house.

`equip` puts a weapon in the hand and a bag on the back. A two-handed bat frees the other hand.

## 5. The first zombie

Zombies near the house may already have seen the agent through a window. If not, opening the front door (10 m of
noise) usually brings one.

> A zombie is coming. Deal with it: shove it if it gets close, hit it while it's staggered, stomp it when it's down.
> Watch your endurance.

`attack`, `shove` and `stomp` each step the world until the move lands and the player can act again, then report what
happened. This one is from the [modding tutorial](/docs/modding/first-mod/)'s nail bat:

```text
player 1 swung the nail bat facing e: landed after 7 ticks, 1 hit; ready again after 19 ticks;
100 HP, endurance 0.96; HITS: P1 hit #1 for 43, staggered (50 HP left)
```

If a strike lands on the agent, `step_ticks` and the attack replies say where and what kind of wound. Then:

> You're hurt. Call `health` and treat your wounds with what you carry.

## 6. Let it play

From here, give it a goal and let it run:

> Survive the day. Find more food and water in the next houses, avoid crowds, and get back to a safe room with a bed
> by nightfall. Sleep when you're tired. Report every in-game hour.

Ask for a picture whenever you want to see what it sees:

> Take a `screenshot` and describe what's in it.

## 7. When it dies

It will. The step replies say how (`P1 DIED: Bled out from a gash on the torso`) and `stats` has the run: time
survived, kills, distance walked, wounds taken. Ask it what it would do differently, then start again with a new seed:
remove the save file, or start the server with another `--seed`.

## Tips

- **Smaller worlds are kinder to your machine:** `--zombies 500` is plenty for a first session.
- **Ask for reasoning before actions** at the start; once it plays well, let it take several steps at once.
- **The same seed is the same town.** An agent that played seed 1 can be told what it learned about it.
- **Save before something risky** with `save {name}`, so you can rerun it, but only if you're not playing fair.
