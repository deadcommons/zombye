---
title: Agents and MCP
description: How AI agents play, test and mod Zombye through its built-in MCP server.
---

Zombye is built so that an AI agent can do what a player does. The server has a
[Model Context Protocol](https://modelcontextprotocol.io) endpoint built in: start `zombye-server --mcp` from Claude
Code, Claude Desktop or any other MCP client, and your agent gets a set of tools to look at the world and act in it.

## What an agent can do

- **Play.** Look around, walk, open doors, search cupboards, pick things up, wear and wield them, fight, dress wounds,
  eat, craft and sleep. The player's moves and the agent's moves are the same messages, checked by the same server
  rules, so an agent can't walk through walls or see through doors.
- **See.** `observe` returns a text map of everything around the player with every zombie's state; `screenshot`
  returns a real rendered picture of the game, with the player's fog of war, so a model that reads images sees what
  you'd see.
- **Test.** Set up a scene (spawn zombies, make a noise, give the player a weapon), step time forward, and check what
  happened. Save it and run it again: the simulation is deterministic, so the same save and the same moves give the
  same result.
- **Mod.** An agent can read and change the [data files](/docs/modding/), start a server on the new data, and play
  its own mod to check it.

## How it differs from playing in a window

**Time only moves when the agent says so.** Between tool calls the world is paused. `step_ticks` moves it on by a
number of ticks (20 a second); `move_player` holds a direction for some ticks; `attack` steps until the swing lands
and the player can act again. An agent can think for as long as it likes without a zombie getting closer, which makes
it a turn-based game for the agent.

**The agent is player 1.** It wakes up in the bedroom like you do. `add_player` can bring in more players, for
testing several at once.

**There are debug tools.** Spawning zombies and items, god mode, skipping time and setting needs exist for testing and
mod work. If you want your agent to play fair, tell it not to use them (the [tutorial](/docs/agents/tutorial/) has a
prompt for that).

## Next

- [Connect your agent](/docs/agents/connect/): set it up in Claude Code, Claude Desktop or another client.
- [Tutorial: an agent survives](/docs/agents/tutorial/): a first session, prompt by prompt.
- [Testing and debugging](/docs/agents/testing/): scenarios, screenshots, saves and repeatable runs.
- [MCP tool reference](/docs/agents/tools/): every tool.
