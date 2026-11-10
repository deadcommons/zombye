# Hosting your own Zombye server

Anyone can run a Zombye server, for free, modified or not ([PLAN.md](PLAN.md), "Hosting"). The
server is one binary, `zombye-server`, plus the game data folder; its settings are one file. It
needs no GPU and no display. One world per server process; up to 64 players (32 by default).

## With Docker

There is no published image yet: build it from a checkout (a few minutes; it compiles the server
only, not the game client).

```bash
git clone https://github.com/deadcommons/zombye && cd zombye
docker build -t zombye-server .
docker run -d --name zombye -p 7777:7777 -v zombye-data:/data zombye-server
docker logs -f zombye
```

The log says `Zombye server: listening on 0.0.0.0:7777 (...)`. Players join with the game:

```bash
zombye --connect YOUR-HOST --name ada          # cargo run -p zombye-client --release -- --connect ...
```

Open TCP port 7777 in your firewall. Clients must run the same version of the game with the same
game data as the server: one that differs is refused with a message saying what differs.

- **`/data`** (the `zombye-data` volume) is the server's folder: `server.ron` (the config, written
  with the defaults on the first start), `world.zsave` (the world) and `world.zsave.players` (whose
  survivor is whose). Back up the two `world.zsave*` files together.
- **Change a setting:** edit `server.ron` in the volume and `docker restart zombye`, or pass flags
  after the image name; a flag wins over the file:
  `docker run ... zombye-server --config /data/server.ron --max-players 8 --pvp off`.
- **Stop it:** `docker stop zombye` (SIGTERM). The server tells its players, saves the world and
  exits; give it the default 10 s. It also autosaves every 5 minutes while anyone plays.
- **A folder of your own instead of a volume:** the server runs as uid 10001, so either
  `chown 10001 ./zombye-data` or run it as yourself:
  `docker run -d -p 7777:7777 --user "$(id -u):$(id -g)" -v "$PWD/zombye-data:/data" zombye-server`.
- **Your own game data** (a modded `assets/data`): mount it and point the server at it,
  `-v "$PWD/my-data:/mods:ro" -e ZOMBYE_DATA=/mods`. Your players need the same folder
  (`zombye --data DIR`).

With Compose:

```yaml
services:
  zombye:
    build: .                # the checkout
    restart: unless-stopped
    ports: ["7777:7777"]
    volumes: ["zombye-data:/data"]
volumes:
  zombye-data:
```

## Without Docker

```bash
cargo build --release -p zombye-server
./target/release/zombye-server --config my-server/server.ron
```

To run it somewhere else, copy `target/release/zombye-server` and `assets/data` (as a folder named
`data` beside the binary, or anywhere with `ZOMBYE_DATA=DIR` or `data:` in the config). Stop it
with Ctrl-C or SIGTERM; it saves on the way out.

## The config file

`--config FILE` reads the server's settings from one [RON](https://github.com/ron-rs/ron) file. If
the file isn't there, the server writes it with every default, commented
([crates/server/server.example.ron](../crates/server/server.example.ron) is that file), and starts.
Every line is optional. A mistake (an unknown setting, a value out of range) stops the server
before it makes or loads a world, naming the file and the mistake. Paths in it are relative to
the file's folder. It is read once, at start.

| Setting | Default | What |
|---|---|---|
| `name` | `"Zombye server"` | What players see when they join. |
| `listen` | `"0.0.0.0:7777"` | Address and TCP port for game clients. |
| `max_players` | `32` | 1 to 64. |
| `password` | `""` (none) | Keeps strangers out. Sent in the clear: not a secret. |
| `pvp` | on for a new world | `true`: open PvP. `false`: co-op. Left out, a saved world keeps its rule. |
| `spawn` | by `pvp` | `Spread` round the edge of town (PvP's default) or `Together` outside one house. |
| `fair_sight` | by `pvp` | Clients learn of other players only while their survivor sees or hears them. On with PvP. |
| `logout_secs`, `logout_safe_secs` | `30`, `5` | How long a survivor lies in the world after its player leaves; the same when left somewhere safe. |
| `spawn_protection_secs` | `10` | A new survivor can't be hurt by players for this long (PvP). |
| `start_kit` | the data's `start_kit.ron` | What a new survivor starts with: `[(item: "water_bottle"), (item: "rag", count: 2)]`, item ids from `items.ron`, at most 12 entries of 1 to 20. `[]`: nothing. |
| `camera` | `Isometric` | The camera everyone plays with: `Isometric` or `FirstPerson`. No game client has first person yet, so only headless clients can join a first-person server. |
| `agents` | `Marked` | Programs (bots, AI agents: clients that say they are one): `Marked` lets them in and shows players which survivors they are, `Allowed` lets them in unmarked, `Refused` keeps them out. |
| `budget_kb` | `32` | KB a second down to each client at most (8 to 1000). |
| `allow_dev` | `false` | Take god mode and the debug intents from clients. For testing. |
| `save` | `"world.zsave"` | The world's file: loaded if there, else made. `None`: never saved. |
| `autosave_mins` | `5` | Minutes between autosaves; `0`: only on shutdown. |
| `seed`, `map_chunks`, `zombies` | `1`, `30`, by the data | A new world only. The map is `map_chunks` × 16 m each way (64: about 1 km). |
| `load_anyway` | `false` | Load a save whose game data changed incompatibly, dropping what no longer fits. |
| `data` | `$ZOMBYE_DATA`, else shipped | The game data folder. |
| `admin` | none | `(listen: "127.0.0.1:8788", token: "...", origins: [])`: the operator's MCP admin endpoint. |

The same settings are flags (`zombye-server --host --max-players 8 ...`; the list is at the top of
`crates/server/src/main.rs`), and a flag on the command line wins over the file.

**What players are told.** The rules (players, PvP, the camera, agents, what they're shown of other
players, logging out, how new survivors start and with what, the map and its zombies) go to every
client as it joins, which shows them in a panel (F6 brings it back). Anyone can also ask before
joining, with no password: `zombye --server-info HOST[:PORT]` prints them, and the server prints
the same lines when it starts.

## Administering it with an agent

The server has an MCP endpoint for its operator's AI agent: list, kick and ban players, save,
change settings while it runs, set off events, read the log (AGENTS.md, "Administering a
server"). It always needs a token of at least 16 characters, and has no TLS. In Docker it must
listen on the container's own address to be reachable, so publish it to this machine only:

```bash
# in /data/server.ron:   admin: (listen: "0.0.0.0:8788"),
docker run -d --name zombye -p 7777:7777 -p 127.0.0.1:8788:8788 \
  -e ZOMBYE_ADMIN_TOKEN="$(openssl rand -hex 24)" -v zombye-data:/data zombye-server
```

The agent connects to `http://127.0.0.1:8788/mcp` with `Authorization: Bearer TOKEN`. To reach it
from elsewhere put an HTTPS reverse proxy or a tunnel in front; never publish the port as it is.

## Getting found: the server browser

Players press F7 in the game for the server browser. It shows the servers a **list** names, and
there is no central list: nobody runs one for the game, and the game asks no service of ours. A
list is a small text file that anyone can keep and hand round, as a file or behind a plain
`http://` address (https can't be read yet):

```ron
(
    servers: [
        (address: "play.example.org", name: "Example co-op", featured: true, note: "EU, restarts at 04:00"),
        (address: "203.0.113.7:7778"),
    ],
)
```

`address` is `host[:port]` (7777 without); `name` shows until your server answers with its own;
`note` is one line of yours; `featured: true` puts the server in the section at the top. Players
use a list with `zombye --server-list FILE|http://HOST/PATH`, or `server_list: Some("...")` in
their `settings.ron`, or by replacing `servers.ron` in the game's data folder; they can also type
your address into the browser's Address field, which keeps it.

Each server in the list is asked what it is without joining: its name, players on, PvP or co-op,
whether it wants a password, its mods (`--mod`) and the rest of its rules. So the port must be
reachable, and what the browser shows is what your server says. A client needs the same mods as
the server (it is told which, and they aren't downloaded yet).

## What to know

- **Nothing is encrypted**, the join password included. Names are not accounts: anyone who knows
  the name of a survivor nobody is playing can take it up.
- **Size:** the default world (480 m, about 2,950 zombies) with 32 players ticks in 4-6 ms of a
  50 ms budget on one core and sends each client about 5 KB/s; a 1 km world with 21,500 zombies
  and 32 players about 7 ms (measured on a shared 32-core VM; the world runs on one thread).
- **Updating:** `git pull`, build the image again, `docker stop`, `docker rm` and `docker run` as
  before: the volume keeps the world. If the new version's game data changed in a way the save
  can't take, the server says so and stops; `load_anyway: true` loads it, dropping what no longer
  fits.
