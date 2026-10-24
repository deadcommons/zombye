---
title: Connect your agent
description: Set up the Zombye MCP server in Claude Code, Claude Desktop and other MCP clients.
---

The MCP server is the `zombye-server` program started with `--mcp`. It talks over standard input and output, so your
MCP client starts it and owns it: when the client closes, the server saves (if you gave it a save file) and stops.

## 1. Build it

From the repository's root:

```bash
cargo build --release -p zombye-server -p zombye-client
```

This makes `target/release/zombye-server` and `target/release/zombye`. The server needs the client next to it for the
`screenshot` tool, which runs the game offscreen to draw a picture (no window opens).

## 2. Add it to your client

### Claude Code

The repository already registers the server in [`.mcp.json`](https://github.com/deadcommons/zombye/blob/main/.mcp.json).
Start Claude Code in the repository's root, approve the `zombye` server when it asks, and the tools are there.

To add it yourself, or with other options:

```bash
claude mcp add zombye -- ./target/release/zombye-server --mcp --zombies 1000
```

Check it's connected with `/mcp` in Claude Code.

### Claude Desktop

Add it to `claude_desktop_config.json` (Settings → Developer → Edit Config), with absolute paths:

```json
{
  "mcpServers": {
    "zombye": {
      "command": "/Users/you/zombye/target/release/zombye-server",
      "args": ["--mcp", "--zombies", "1000", "--save", "/Users/you/zombye/saves/agent.zsave"]
    }
  }
}
```

Restart Claude Desktop.

### Any other MCP client

Run `zombye-server --mcp` as a stdio server. It needs no environment variables. If the client starts it from another
folder, use the full path to the binary.

## 3. Choose the world

The server takes the same world options as the game:

| Option | What it does |
|---|---|
| `--seed S` | Which town. The same seed always makes the same town. |
| `--zombies N` | How many zombies. Without it, the population file's number for the map (about 2,950). Fewer is easier, and lighter on your machine. |
| `--map-chunks N` | Map size, N × 16 m a side (default 30). |
| `--save PATH` | Load that save if it exists, else make a new world and save it there. Autosaves every few minutes of real time when something changed, and on exit. |
| `--autosave-mins N` | How often to autosave (default 5; 0 is off). |
| `--saves DIR` | The folder for the `save` and `load` tools' named saves. |
| `--data DIR` | Play on modded data instead of `assets/data`. |
| `--client PATH` | Where the game binary is, for screenshots (default: next to the server). |

## 4. Say hello

Ask your agent something like:

> Use the zombye tools. Call `observe` and tell me where you are and what's around you.

It should describe a bedroom in a house, the rooms around it and what's in reach.

## Troubleshooting

- **The server won't connect, or "connection closed".** The binary isn't built, or the path is relative and the
  client started it from another folder. Build it, or use the absolute path.
- **`screenshot` fails with "client not found".** Build the client too (`-p zombye-client`), or pass `--client PATH`.
- **The world is the same every time.** That's the seed. Pass `--seed` with another number.
- **The agent says it's paused.** It is. Time only moves when a tool steps it; ask the agent to move or step time.
