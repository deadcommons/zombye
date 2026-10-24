---
title: Build and run
description: Build Zombye from source and start the game.
---

Zombye is written in Rust with the [Bevy](https://bevyengine.org) engine. There are no prebuilt downloads yet, so you
build it yourself. It takes a few minutes the first time and seconds after that.

## What you need

- **Rust**, the current stable toolchain, from [rustup.rs](https://rustup.rs).
- **git**.
- A GPU that can run Metal (macOS), Vulkan (Linux, Windows) or DirectX 12 (Windows).
- On Linux, the usual Bevy system packages: on Debian and Ubuntu,
  `sudo apt install g++ pkg-config libx11-dev libasound2-dev libudev-dev libxkbcommon-x11-0 libwayland-dev`.

## Get the code and play

```bash
git clone https://github.com/deadcommons/zombye.git
```

```bash
cd zombye
```

```bash
cargo run -p zombye-client --release
```

The first build compiles the engine and takes a while. When the window opens you are standing in a bedroom. The
[controls](/docs/play/controls/) are on the next page.

:::tip[Rebuilding often?]
If you're changing things and playing again and again, the `play` profile is as fast as release and rebuilds in
seconds:

```bash
cargo run -p zombye-client --profile play
```
:::

## Useful options

```bash
cargo run -p zombye-client --release -- --seed 7 --time 20 --zombies 1500
```

- `--seed S` picks the town. The same seed always makes the same town.
- `--time HOUR` sets the starting hour (0 to 24). Night is properly dark.
- `--zombies N` sets how many zombies the world holds. Without it you get the number the population file gives
  for the map, about 2,950 on the default map.
- `--map-chunks N` makes the map bigger (N × 16 m a side; the default 30 is 480 m, 64 is about 1 km).
- `--load autosave` carries on from where you quit.

Every option is in the [command-line reference](/docs/reference/cli/).

## The server, for agents

The headless server is a separate program. Build it in release mode before connecting an agent:

```bash
cargo build --release -p zombye-server -p zombye-client
```

Then follow [Connect your agent](/docs/agents/connect/). (The client is built too because the server's `screenshot`
tool runs it offscreen to draw pictures.)

## Run the tests

```bash
cargo test --release --workspace
```

The simulation tests include a determinism check (the same seed and inputs always give the same world) and a
performance smoke test.
