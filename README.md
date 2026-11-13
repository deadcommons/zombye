# Zombye

**Open-source zombie survival in real 3D.** Isometric, with hundreds of zombies on screen, built in
Rust so that anyone (and any AI agent) can read it, mod it and host it.

> **Status: pre-alpha, in playtesting.** Single-player and multiplayer both run; plenty is still
> missing. The roadmap lives on the
> [Game development board](https://github.com/orgs/deadcommons/projects/2) and the
> [milestones](https://github.com/deadcommons/zombye/milestones); the design is in
> [docs/PLAN.md](docs/PLAN.md).

## What it is

- **Classic slow-burn survival:** wake up in a house in a small town, scavenge, sneak or fight,
  patch your wounds and see how long you last. [VISION.md](VISION.md) explains what we keep and
  what we change.
- **Real 3D**, viewed from a fixed isometric camera, with an optional 4-angle snap.
- **Fast:** thousands of zombies simulated, hundreds on screen, and a headless deterministic sim.
- **Moddable by default:** every gameplay value lives in a data file, behaviour is added with
  sandboxed Luau scripts ([docs/modding.md](docs/modding.md)), and servers push mods to players.
- **Agent-first:** the server has a built-in MCP endpoint, so AI agents can play, test, administer
  and build mods.
- **Host it anywhere:** self-hosting is free and documented. Official one-click hosting by
  Dead Commons is coming, and it pays for development.

## This repo and the official build

Everything in this repo is open, and building it gives you the whole game: every system, the server,
the MCP tools and mods, with its own models, placeholder sounds and the name as plain text.
**The official build** from Dead Commons adds art and audio bought under licences that don't allow
publishing the files, plus signed installers, updates and official servers. Buying it is how you pay
for the game's development; building it yourself is always free.

## Build and run

Needs a current stable Rust toolchain.

```bash
cargo run --release --bin zombye
```

It opens on the main menu. In game: WASD to move, Shift to run, right mouse to aim and left to
swing, E to pick up or search, Tab for the inventory, Esc for the pause menu (its Controls tab lists
every key), mouse wheel to zoom, `,` and `.` to turn the camera. Options such as `--zombies N` and
`--seed S` start a world straight away; [AGENTS.md](AGENTS.md) lists them all.

The headless server (benchmark mode, or MCP over stdio for agents):

```bash
cargo run --release --bin zombye-server -- --bench --zombies 1000
```

```bash
cargo run --release --bin zombye-server -- --mcp
```

Agents can connect through [.mcp.json](.mcp.json) (build the server first). See [AGENTS.md](AGENTS.md)
for the tools and repo rules.

To compare agents, the **survival benchmark** plays fixed scenarios over MCP and writes each run's
score to a file ([docs/benchmark.md](docs/benchmark.md)):

```bash
cargo run --release --bin zombye-server -- --benchmark list
cargo run --release --bin zombye-server -- --mcp --benchmark first_night --agent my-agent
```

### Host a server

Multiplayer runs on a dedicated server anyone can host, for free: one binary, one config file, or
a Docker image you build from this repo. See [docs/self-hosting.md](docs/self-hosting.md).

```bash
docker build -t zombye-server .
docker run -d -p 7777:7777 -v zombye-data:/data zombye-server
cargo run --release --bin zombye -- --connect localhost        # join it
```

## Contributing

Contributions from people and their agents are welcome. Read [CONTRIBUTING.md](CONTRIBUTING.md)
first, especially the **clean-room rules**, and follow the [code of conduct](CODE_OF_CONDUCT.md).
Sign off every commit (`git commit -s`): that's the [DCO](https://developercertificate.org/), and
there's no CLA.

Come and talk to us on **[Discord](https://discord.gg/y7NsmVG2br)**: follow the devlog, share
ideas, or find people to mod and build with.

## License

Zombye's code (the game, the server, tools, data, shaders and docs) is dual-licensed under
[MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option. Our art and audio (models,
textures, animations, images, sounds and music) are licensed under
[CC BY-NC-SA 4.0](assets/LICENSE): use them in mods, fan work and non-commercial projects;
commercial use needs our permission.

**Videos and streams:** you're welcome to record, stream and monetise videos and screenshots of
Zombye without asking.

Copyright (c) 2026 Grillo Ltd (trading as Dead Commons) and the Zombye contributors. Required notices are in
[NOTICE](NOTICE), and every asset is listed in [ATTRIBUTION.md](ATTRIBUTION.md). The Zombye and Dead Commons
names and logos are trademarks, not open source: a fork you publish takes its own name. See
[docs/brand/LICENSE-brand.txt](docs/brand/LICENSE-brand.txt).
