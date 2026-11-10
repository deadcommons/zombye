# AGENTS.md: working on Zombye

Zombye is an open-source, true-3D isometric zombie survival game (Rust + Bevy), built so AI agents
can read it, run it, play it and mod it. The design is in [docs/PLAN.md](docs/PLAN.md). Read it
before making architectural changes.

Work is tracked on GitHub, not in the repo: the
[Game development project](https://github.com/orgs/deadcommons/projects/2) board and the
[milestones](https://github.com/deadcommons/zombye/milestones). Reference issues in commits
(`Closes #N`), and keep each issue's board status current (Todo, In Progress, Done).

## Layout

| Path | What | Licence |
|---|---|---|
| `crates/sim` | Pure simulation: chunked tile map with edge walls, building templates, town generator, line of sight, players (`players.rs`: shared movement + endurance), zombies (`zombies.rs` storage, `senses.rs` sight + hearing, `awareness.rs`: which players see and heard which, `ai.rs` state machine + `ZombieTuning`, `tiers.rs` active/near/virtual simulation tiers + `TierTuning`), population (`population.rs`: density per zone from data, respawn, migration toward noise), flow fields + cache (`flow.rs`), item and weapon definitions and ground items (`items.rs`), inventories, carry weight and item jobs (`inventory.rs` + `CarryTuning`), furniture containers (`containers.rs`), loot tables (`loot.rs`), melee combat and player health (`combat.rs` + `CombatTuning`), weapons wearing out and breaking (`wear.rs`, see Weapons in the hand), lag compensation for it (`lagcomp.rs`: a few ticks of positions, so a remote client's blow lands where it saw the target), guns (`ranged.rs`: a shot's flight over the map and through bodies, rounds, magazines and reloading, see Guns), wounds and illness (`health.rs`), first aid (`firstaid.rs` + `FirstAidTuning`), crafting recipes and craft jobs (`crafting.rs`), hunger, thirst, tiredness, food going off and sleep (`needs.rs` + `NeedsTuning`), the status icons a client shows of all that, their levels from data (`status.rs`), skills that improve with practice (`skills.rs` + `SkillTuning`), street props placed by the town generator from data (`street_props.rs`), run stats and the cause of death (`run.rs`), the first night's objectives and outcome (`firstnight.rs`), logging out, spawn protection and the start kit on a server (`logout.rs`), the benchmark's scenarios and score (`scenario.rs`, see The agent survival benchmark), proximity chat and gestures (`speech.rs`: who hears a line, the noise it makes, see Talking and gestures), tying hands, freeing and searching a tied survivor (`bonds.rs`) and optional groups with names and shared markers (`groups.rs`; both: see Hold-ups and groups), saving and loading the world to and from bytes (`save.rs`; content-change and value checks on load in `save_check.rs`), the hook mod scripts run through (`script.rs`: events, the checked `script_*` functions, what scripts keep; see Mods and scripts). No engine, no I/O. Deterministic. | MIT OR Apache-2.0 |
| `crates/net` | `Intent` (client → server) and `Snapshot` (server → client) types, `LocalServer` (with `join`), multiplayer over the network (see Multiplayer: the messages, framing and per-client delta snapshots in `wire.rs`, a client's view cut to what its survivor sees and hears of other players in `visibility.rs`, the server's connections in `host.rs`, a client's in `remote.rs`; the server browser's list and probes in `browser.rs`, see The server browser; a mod's data as a server hands it to joining clients, its hash and the client's cache in `modpack.rs`, see Mods downloaded on joining), content loading (`content.rs`), save files on disk (`save_file.rs`), Ctrl-C/SIGTERM/SIGHUP as a request to save and stop, for the server and the client (`shutdown.rs`). | MIT OR Apache-2.0 |
| `crates/mods` | Mods (see Mods and scripts): reads a mod's folder (`mod.ron`, `items.ron`, `recipes.ron`, `loot.ron`, `buildings/*.ron`, `*.luau`), merges its data into a `WorldConfig` (`lib.rs`: `Mod`, `apply`, `load_into`; `Mod::pack` / `from_pack` / `apply_packs`: the data a server hands to joining clients, #24) and runs its Luau scripts sandboxed, one VM a mod, as the world's script host (`luau.rs`, on `mlua`; Luau is built from source, so a C++ compiler is needed). The one crate with Luau in it. | MIT OR Apache-2.0 |
| `mods/` | Sample mods: `alarm_clock` (an item behaviour by script; the worked example of [docs/modding.md](docs/modding.md)). | MIT OR Apache-2.0 |
| `crates/server` | `zombye-server` headless binary: bench mode (`--bench`, with `--players`/`--horde` for the tiers) and the MCP endpoint (`--mcp` on stdio, `mcp.rs`; `--mcp-http` over streamable HTTP for remote agents, `mcp_http.rs`, see Connecting an agent); the multiplayer server (`--host`) and bot clients (`--bots N --connect ADDR`; `multiplayer.rs`, see Multiplayer); the operator's MCP admin endpoint on a hosting server (`--host --admin-mcp`, `admin.rs`, see Administering a server); the dedicated server from one config file (`--config FILE`, `config.rs`, and the `Dockerfile` at the root that runs it, see The dedicated server under Multiplayer); `--save` with autosave; the MCP `screenshot` tool runs the client offscreen (`screenshot.rs`); the agent survival benchmark (`--benchmark`, `benchmark.rs`, see The agent survival benchmark). | MIT OR Apache-2.0 |
| `crates/client` | `zombye` Bevy client: camera, chunk streaming, building meshes (`world_render.rs`; their details, trim, siding lines, porches, chimneys, shopfronts and lit windows, from `meshgen/building_kit.rs`, see Buildings dressed under The world), the outdoors per chunk from the tile map alone (`meshgen/landscape.rs`: lawns, verges, forest floor and litter, road paint at real scale, kerbs, slab joints, driveways, front beds, fences and hedges; `meshgen/curves.rs`: what's drawn round over the tiles, see Curves under The world; `meshgen/trees.rs`: faceted broadleaves, birches, pines, spruces; colours from `assets/data/palette.ron` through `meshgen/palette.rs`), fog of war, cutaways (`cutaway.rs`: which walls drop, per building and wall edge; `silhouette.rs`: the player's shape through what still covers them; see Cutaways under Light and mood; `opening_hints.rs`: outlines of the doors and windows a building hides from the camera, see Opening hints there), day/night, input + player prediction (`player.rs`), grey-box characters (`rig.rs` bones + boxes; `zombie_render.rs` VAT crowd, or a character baked offline read from a `.zvat` (`zvat.rs`); `survivor.rs` skinned, CPU-posed players; `character.rs` reads a modular character's body and wearable GLBs and `survivor_model.rs` draws players as it, dressed in what they wear), hit feedback (`fx.rs`), lights in the dark (`lights.rs`, see Light and mood), instanced grass, flowers and litter (`vegetation.rs`), the film pass (`film.rs`) and the graphics settings (`graphics.rs`; all three: see Look and feel), ground items (`items_render.rs`), Blender-built furniture models merged into building meshes (`furniture_models.rs`, grey-box for kinds without one), street props merged into each chunk's props mesh from the same models (`meshgen/street_props.rs`, see Street props under The world), HUD (`hud.rs`; the hotbar on keys 1-9, `hotbar.rs`; panels dragged by their titles and remembered, `panels.rs`; the UI's one skin, `theme.rs`: text sizes and colours, and the painted sheets, buttons, slots and bars every panel uses, see The skin), the death screen and run summary (`death.rs`), the first night's objectives, hints and morning screen (`firstnight_ui.rs`), the skills panel and level-up line (`skills_ui.rs`), the chat line, speech bubbles and gesture keys (`chat.rs`, see Talking and gestures), tying, searching and group keys, the search panel, name tags and markers (`holdup.rs`, see Hold-ups and groups), the server browser (`browser_ui.rs`, F7, see The server browser), sound (`audio.rs`, see Sound), `--autoplay` (`autoplay.rs`), quicksave / quickload / autosave (`saving.rs`), offscreen rendering to a PNG with no window (`--render-to`, `offscreen.rs`). | MIT OR Apache-2.0 |
| `assets/` | Shaders, data files (`data/buildings/*.ron` templates, `data/items/items.ron`, `data/items/weapons.ron`, `data/items/recipes.ron`, `data/containers.ron`, `data/loot.ron`, `data/population.ron`, `data/street_props.ron`, `data/scenarios.ron`: the benchmark's scenarios, `data/status.ron`: the status icons' levels, labels and tips, `data/palette.ron`: every colour, for Blender assets and the generated world), models (`models/furniture/*.glb`, street props `models/street/*.glb`; the survivor's body `models/characters/survivor_01.glb` and its wearables `models/wearables/*.glb`) with their specs (`specs/`) and Blender sources (`source/`: build script, `.blend`, renders), sound (`audio/sfx/*.wav`, `audio/ambience/*.wav`, listed in `data/sounds.ron`), the UI's typefaces (`fonts/barlow/`, see The skin), later textures. | Code, data and shaders: MIT OR Apache-2.0. Art and audio: CC BY-NC-SA 4.0 ([assets/LICENSE](assets/LICENSE)); the Barlow fonts: SIL OFL 1.1; each asset listed in [ATTRIBUTION.md](ATTRIBUTION.md) |
| `tools/blender/` | Python to build assets: run inside Blender, mesh builder, palette, renders, validation, GLB export (`zasset.py`, `palette.py`: reads `assets/data/palette.ron`, `build.py`), character clean-up, validation and skinned export (`zchar.py`), VAT bake for the crowd, outfit variants with palette swatches (`zvat.py`, `build.py -- --crowd`); plain Python, art-repo work folders and promotion (`artrepo.py`). | MIT OR Apache-2.0 |
| `tools/audio/` | `synth.py`: the placeholder sounds, synthesised from noise, tones and filters (plain Python, deterministic); `zsound.py`: the real sounds (#78), from a generator's raw takes to game-ready WAVs: slice, clean, audition, promote with ATTRIBUTION.md rows, check (plain Python; see the elevenlabs-sounds skill). | MIT OR Apache-2.0 |
| `tools/remote/` | The remote machines (see Remote machines): `setup.sh` (a build or GPU box), `zgpu` (the client on the GPU box, as if local), `zwork` (an agent per issue, each in its own worktree), `zboard` (the owner's dashboard: a screen of review pictures or the game streamed, the issues' progress, feedback, approve), `dcv.sh` (a GPU machine's desktop shared by Amazon DCV, for remote play). | MIT OR Apache-2.0 |
| `docs/` | Plan and design docs. | — |

## Rules

1. **The server is the authority.** Clients (human, agent or the single-player UI) only send
   `Intent`s and read `Snapshot`s. Never mutate `World` from client code except through
   `zombye_net::apply_intent`.
2. **Keep `sim` deterministic.** Fixed tick (`TICK_DT`), all randomness from `sim::rng::Rng`, no
   wall-clock time, no `HashMap` iteration order in simulation logic, stable ordering of entities.
   `World::state_hash()` must match for the same seed and inputs. There's a test for it.
3. **Keep `sim` engine-free.** It depends on `glam`, `serde`, `ron` and `postcard` (the save encoding)
   only, and does no I/O (content is loaded by `net::content` and passed in; saves are bytes, written and
   read by `net::save_file`). Rendering concerns live in `client`.
4. **Performance budget:** a server tick is 50 ms (20 Hz). Thousands of zombies must stay well under
   it. Use struct-of-arrays and the spatial hash; avoid per-zombie allocation and per-zombie A*.
5. **Clean-room: follow the rules in [CONTRIBUTING.md](CONTRIBUTING.md#clean-room-rules-non-negotiable).**
   In short, other games may only be observed by playing them: no decompiling, disassembling, or
   reading or extracting anything in their install folders, with any tool (REA included). All tuning
   values are our own.
6. **All art is ours.** No third-party asset packs. Placeholder = grey-box geometry. Every asset
   needs a row in [ATTRIBUTION.md](ATTRIBUTION.md).
7. **Data over code** for anything a modder would want to change (items, weapons, recipes, loot,
   building templates), once those systems exist.

## Assets built in Blender

Models (furniture, props, later characters) are built in Blender over the Blender MCP from a spec and
a build script, validated, rendered from the four game angles and exported to GLB: follow the
**[blender-assets skill](.claude/skills/blender-assets/SKILL.md)**. Work in progress lives in the private
art repo (deadcommons/zombye-art); an asset comes here only when it's finished, promoted with
`tools/blender/artrepo.py` (the pipeline code itself lives here). `ZOMBYE_EXTRA_MODELS=DIR[:DIR]` makes the
client draw work-in-progress GLBs to check them in game. Walls, floors, roofs, roads and terrain stay
code-generated. Characters and other detailed assets start in Meshy (reference image, image-to-3D, rig,
animations) and are cleaned up in Blender with `tools/blender/zchar.py` and baked for the crowd with
`zvat.py`: follow the **[meshy-assets skill](.claude/skills/meshy-assets/SKILL.md)** (paid credits, its
rules first).
**[docs/asset-pipeline.md](docs/asset-pipeline.md)** is the process for every asset, Blender
or Meshy (from an approved reference through four review gates the owner approves with screenshots) and
the sizes they all share: storey 3.0 m, walls 0.16 m, doors 1 × 2.1 m, window sill 0.9 m and head 2.1 m,
and the rest. Change a building number in the code, change it there too.

## Remote machines

Development can run on two remote Linux machines, driven from Claude on the owner's Mac:
**[docs/remote-dev.md](docs/remote-dev.md)**. `zombye-build` (many cores, no GPU) holds one git worktree
and one agent per issue (`~/wt/N`, branch `issue-N`, started with `zwork start N`): it builds, tests,
commits with `Refs #N` and opens a draft PR. `zombye-gpu` only renders: `zgpu` runs the client there as if
it ran locally, so `--render-to` and the MCP `screenshot` tool work from the build box. Review pictures go
to `review/` in the worktree and come back to the Mac for the owner to approve; agents never merge, push
to main or force-push. Any issue's build can be played remotely, streamed into the owner's dashboard
(zboard) from a GPU desktop by Amazon DCV. Asset work with Blender, Meshy or ElevenLabs (their keys are only
on the Mac) stays on the Mac. If you are on `zombye-build`, you have no GPU and no display: render with `zgpu`.

## Commands

```bash
cargo test -p zombye-sim --release          # sim tests incl. determinism + perf smoke test
cargo run -p zombye-server --release -- --bench --zombies 5000
# Issue #2's target: ~1,000 active + 20,000 virtual on a 1 km map (prints a per-tier breakdown)
cargo run -p zombye-server --release -- --bench --ticks 400 --map-chunks 64 --zombies 21500 --players 1 --horde 1000
# ...and time saving and loading that world (size, ms, and that the hash matches after load and 100 more ticks)
cargo run -p zombye-server --release -- --bench --ticks 200 --map-chunks 64 --zombies 21500 --players 1 --horde 1000 --save-bench
# Issue #70's target: 32 players spread over the 1 km map (nearly every zombie active), and the same over real TCP with bots
cargo run -p zombye-server --release -- --bench --ticks 600 --map-chunks 64 --zombies 21500 --players 32 --horde 1000 --spread
cargo run -p zombye-server --release -- --bench --bots 32 --spread --ticks 400 --map-chunks 64 --zombies 21500 --horde 1000
# Multiplayer (see Multiplayer): a server on TCP port 7777, a game client on it, and 31 bots to fill it
cargo run -p zombye-server --release -- --host --save saves/server.zsave      # add --pvp off for co-op
# ...with the operator's MCP admin endpoint on http://127.0.0.1:8788/mcp (see Administering a server)
ZOMBYE_ADMIN_TOKEN=a-long-secret-token cargo run -p zombye-server --release -- --host --save saves/server.zsave --admin-mcp
cargo run -p zombye-client --release -- --connect HOST --name NAME
cargo run -p zombye-server --release -- --bots 31 --connect HOST
# The dedicated server: every setting from one file (written with the defaults if it isn't there), and the same in Docker
cargo run -p zombye-server --release -- --config saves/server/server.ron
docker build -t zombye-server . && docker run -d -p 7777:7777 -v zombye-data:/data zombye-server
cargo run -p zombye-client --release -- --zombies 1000          # the game window
cargo run -p zombye-client --profile play -- --zombies 1000     # the same, rebuilding in seconds after a change (for playtesting)
cargo run -p zombye-client --release -- --zombies 2000 --novsync --screenshot shot.png --exit-after 8
cargo run -p zombye-client --release -- --fight-demo --autoplay --zoom 9 --screenshot fight.png --screenshot-after 2.5,4,6 --exit-after 7
# One picture of a save, rendered offscreen: no window opens, nothing takes focus (about 0.5 s plus loading)
cargo run -p zombye-client --release -- --render-to shot.png --load saves/autosave.zsave --size 1280x720 --angle 1 --zoom 30
# Tests: the offscreen render test needs a GPU; it skips itself if there is none, or set ZOMBYE_SKIP_GPU_TESTS=1
cargo test --release --workspace
# The agent survival benchmark (see its section): the scenarios, one played by a built-in script, one served to an agent over MCP
cargo run -p zombye-server --release -- --benchmark list
cargo run -p zombye-server --release -- --benchmark all --baseline runner
cargo run -p zombye-server --release -- --mcp --benchmark first_night --agent NAME
# Mods (see Mods and scripts): the sample mod in the game, on a server (clients download its data on joining), in an agent's world
cargo run -p zombye-client --release -- --mod mods/alarm_clock
cargo run -p zombye-server --release -- --host --mod mods/alarm_clock
cargo run -p zombye-server --release -- --mcp --mod mods/alarm_clock
# Rebuild, validate, render and export every furniture model (Blender 5.x; see the blender-assets skill)
/Applications/Blender.app/Contents/MacOS/Blender --background --factory-startup --python tools/blender/build.py -- furniture
```

Client flags: `--zombies N --seed S --time HOUR --at X,Y --map-chunks N --angle 0..3 --turn N@SECS[,N@SECS...] --opening-hints on|off --novsync --zoom METRES --screenshot PATH --screenshot-after SECS[,SECS...] --exit-after SECS --fight-demo --siege --pvp --duel --autoplay --sneak --saves DIR --load NAME|PATH --load-anyway --player ID --data DIR --mod DIR --quicksave-after SECS --quickload-after SECS --new-run-after SECS --render-to PATH --size WxH --center X,Y --hud --inventory --find TEXT --filter CATEGORY[,CATEGORY...] --tab worn|body|floor --sort name|type|weight --expand --pick N,N --pick-range A,B[@SECS] --drop-picked-after SECS --menu SPEC --menu-pick N[@SECS] --skills --health --morning --graphics-panel --skip-hours H --hover-tag N --debug-stats --god --ignored --crowd-vat PATH[,PATH...]|DIR|none --survivor PATH|DIR|none --pose NAME[:U] --mute --volume V --music V --ui-scale PCT --connect HOST[:PORT] --name NAME --password P --agent --server-info HOST[:PORT] --server-list FILE|URL --browser --browser-join N[@SECS] --hotbar ID[,ID...] --hotbar-press K@SECS[,K@SECS...] --panel-at PANEL:X,Y[;PANEL:X,Y] --drag-panel PANEL:DX,DY[@SECS] --lag MS --say TEXT[@SECS] --shout TEXT[@SECS] --whisper TEXT[@SECS] --gesture NAME[@SECS] --talk-demo --talk-wall --holdup-demo --holdup-auto --graphics low|medium|high|ultra|auto --vegetation off|low|high|N --no-post LIST --msaa N --shadow-map N --render-scale F --frame-bench N --gun ID --walls-up --zombie-silhouettes --walk X,Y;X,Y;...` (the last walks the local player from point to point by itself, opening shut doors its way leads through, for recordings; the two before it: see Cutaways under Light and mood; the one before those: see Guns; the seven before it: see Look and feel; the two before them: see Hold-ups and groups; the six before those: see Talking and gestures) (the camera always turns with `,` and `.`, see Turning the camera under Light and mood: `--turn` does it by itself for recordings, `--opening-hints`: see Opening hints there, and `--rotate`, which turning used to need, is still accepted and does nothing; `--zoom` is the metres of world visible vertically, 8–90, default 26;
several `--screenshot-after` times save `PATH` numbered: `fight-1.png`, `fight-2.png`, ...; keep the last one
a little before `--exit-after` or it's lost). `--size WxH` makes the window exactly that many pixels, at scale factor 1, so a
`--screenshot` is WxH on a high-DPI screen too; without it the window is 1600x900 at the screen's scale.
For verifying combat without a keyboard: `--fight-demo` starts you on open ground near the house you'd wake up in
(unless `--at` says otherwise) with a baseball bat in hand and six zombies 7 m east, called over by a noise;
`--autoplay` makes the local player fight by itself (`autoplay.rs`): it faces the nearest standing zombie and
closes in, swings in reach, alternates shoves and swings when two or more crowd it, stomps a downed zombie at its
feet, and gets up again 4 s after dying. It only sets the normal movement input and sends the normal intents.
`--pvp` turns player against player on for the world the client starts (see Combat). `--duel` is for watching it:
PvP on, you on open ground near the house with a claw hammer, and another survivor 5 m east with a crowbar who comes at you
by itself (`duel.rs`: a player like any other, driven only by the intents a client sends: it closes in, swings, shoves
every third blow, stamps on you when you're down); with `--autoplay` you fight back (autoplay takes other players as
targets when PvP is on). Add `--zombies 0` to keep the town out of it.
`--siege` starts you outside the house you'd wake up in, 10 m out from one of its shut doors (south or east walls
first, so it faces the camera; with no door that has 10 m of open ground straight out, the one with the most, at least
4 m, else a window), with a bat and five zombies between you and the door drawn to a noise just inside
it: they batter it (or the window beside it) with their backs to you, and the door or window shows cracks as it
gives (add `--zombies 0` to keep the rest of the town out of it). Stdout says where (`--siege: 10 m out from the door
of [382, 297] (S wall)`), or that the house has nowhere to stage it and you start inside. `--sneak` starts you sneaking (as if C was
pressed). The game window takes keyboard focus, so typing elsewhere while it runs can move or toggle the player.
`--inventory`, `--skills` and `--health` start with that panel open (for screenshots of it), `--graphics-panel` the graphics panel (F8). `--god` and `--ignored` start with the dev
toggles on (as if F1 / F2 were pressed; see Dev toggles).
`--at` starts the player somewhere other than the bedroom and `--time` sets the start hour, which is handy for screenshots.
`--angle N` starts the camera at one of the four fixed angles (0 = the default, each step a quarter turn; a fraction such as 0.5 is that far through the turn to the next, for a picture of a turn under way), `--player ID`
plays as that player of a `--load`ed save (default its first), `--data DIR` reads game data from another folder (as the
server's `--data`; a save only loads against the content it was made with).
**Offscreen (`--render-to PATH`, `offscreen.rs`):** renders one PNG and exits without ever opening a window: no primary
window, no winit (on macOS that would make it a foreground app and take keyboard focus), no audio or gamepads; the
camera draws into an image (`--size WxH`, default 1280x720, at most 3840x2160) that is read back. Usually with `--load`:
the local server doesn't tick and the player isn't predicted (`crate::live` run condition), so it shows the saved state
exactly, from `--player`'s point of view (fog of war, cutaways, night lamp), with `--zoom`, `--angle` and optionally
`--center X,Y` (the camera, and chunk streaming, centred there; fog is still the player's). No HUD unless `--hud`
(which makes the offscreen camera the UI's too: the HUD and any panel, with `--inventory`, `--skills` or `--health` to
open one, and `--hover-tag N` to show the Nth status icon's line, there being no pointer, and `--debug-stats` the debug stats window), no autosave. For reviewing
the HUD's screens, `crates/net/tests/review_saves.rs` (ignored; `ZOMBYE_REVIEW_DIR=DIR`) writes saves in the states
they show: status icons, things to make with a craft under way, skill XP, tired, asleep. It waits for the frame to be stable rather than a fixed time: visual time runs at a fixed
1/30 s a frame for 30 frames (eased poses settle), then stops; once the render world has had no pipelines waiting for a
few frames it reads the image back, and two readbacks in a row that match (all but 1 byte in 10,000) are written. It
prints `rendered PATH: WxH, N frames, N readbacks, MS ms` (plus a warning if it never settled; 40 s limit) and exits 0,
or exits 1 with the reason on stderr. Prefer it to `--screenshot` for checking visuals: the windowed client pops up and
steals focus. `ZOMBYE_FRAME_BENCH=N` then renders N more frames of the same picture, waiting for the GPU each frame, and
prints `frame cost: M ms mean, W ms worst` (how frame costs in this file are measured: 1920x1080, 2,000 zombies, 120 frames); `--frame-bench N` is the same as a flag. Not rendered: client-only effects that aren't in the save (blood splats, hit flashes).
`--pose NAME[@DEG][+aim][:U]` (`@DEG` moving that many degrees off the aim, `+aim` aiming) holds every survivor in one pose, each the next step through it (players in a row make a strip), for reviewing animation in offscreen pictures where nothing moves:
`walk`, `run`, `sneak` at `U` (0..1) of the locomotion cycle, `swing`, `shove`, `stomp` at `U` of the action (the blow
lands at 0.45), `idle`, `dead`.
`--map-chunks N` makes the map N × N chunks of 16 m (default 30, 480 m; 64 is about 1 km): the same town in the
middle with more forest round it (a map too small for the town is grown to fit). The server takes it too.
Without `--zombies` (client and server) the world holds as many zombies as `assets/data/population.ron` gives for the
map: about 2,950 on the default map, about 3,450 on a 1 km one (the extra ground is sparse forest); `--zombies N`
sets the total and keeps the file's proportions.
Server bench (`zombye-server --bench`): `--ticks N`, `--zombies N`, `--seed S`, `--map-chunks N`; with no `--players`
every zombie heads for a debug goal (all stay active); `--players N` puts players on the road west of town (in the
forest on a big map) running in circles with a 60 m noise every 10 s, `--horde N` spawns N more zombies round the
first, and the report adds tier counts, per-tier ms/tick, a population line (total target, attractors, respawned) and
what the worst tick was made of (each part's ms, promotions, fields and plans built, and how many ticks went over half
the budget and over it); `--spread` (#70) is a realistic server instead: the players on a grid over the whole map, one in
four running and making a noise now and then, the rest walking. **`--bench --bots N`** is the bench over the real
transport (`multiplayer::bench`): a server on a loopback TCP port, the players placed the same way (`--spread`,
`--horde`) and unkillable, N headless bots each on its own connection playing them from another thread, ticking at the
real rate; once all are on it measures `--ticks` ticks and reports the whole tick (intents, world, snapshots) and the
world alone, avg and worst, the active zombies, KB/s down and up per client (and the busiest), the fullest view, and the
worst tick's parts;
`--save-bench` adds a line timing a save and a load of the world at the end. Saves (server, both modes): `--save PATH`
loads that save if it exists (the world options are then ignored; content must fit, see Saves; `--load-anyway` loads it
when it doesn't) or makes a new world
and saves it there, then autosaves to it every `--autosave-mins N` minutes of real time (default 5, fractions allowed,
0 = off) when something changed, and on shutdown (stdin closed, Ctrl-C, SIGTERM or SIGHUP, on Windows also closing the
console; a second Ctrl-C stops at once, unsaved; bench: at the end of the run).
`--saves DIR` is the folder for named saves (MCP `save`/`load`/`saves`; default the `--save` file's folder, else `saves`).
The client prints one stats line per second to stdout (time, room, stance, hp, endurance, fps, frame ms, zombies
sent to it, chasing, zombies per tier active/near/virtual, sim tick ms, and the sounds started that second: `sounds 9 (Groan 4, FootstepHard 3, ...)` or `sound off`).
In game: WASD move (he faces the way he walks), Shift run, C sneak (toggle), right mouse held: aim (he faces the
cursor and strafes or backs off at 2.2 m/s, no running, in a guard stance), and only while aiming left mouse attack
(swing your weapon, or punch; with a gun in hand: shoot at what the cursor is on, see Guns) and Space shove (or stomp, when a downed zombie lies right in front of you), R reload the gun you hold, E pick up the item you
face (with nothing on the floor within reach: search the container you face), 1-9 the hotbar (swap to or use what's on that slot, see Hotbar), Tab or I the inventory panel, H the health panel, K the skills panel (drag any of the three by its title; they remember where they were and whether they were open), F open/close the door or window you face, G smash the window you face, dead: R a new run and Q or Esc quit,
B sleep (B again or WASD to get up), Enter the chat line (Tab there: whisper, talk or shout; Enter says it, Esc drops it), V wave, U hands up (again lowers them), P point (see Talking and gestures), Y tie the hands of the survivor you face or free tied ones, L search a tied survivor, J (held) struggle out of ties, N invite into your group or accept (Shift+N leaves), middle mouse a group marker (see Hold-ups and groups), T skip an hour, Z spawn zombies, wheel zoom (over the inventory, health or skills panel it scrolls that panel instead), F5 quicksave, F9 quickload, F1 god mode, F2 ignored by zombies (dev toggles), F3 the light preview (a time-of-day slider, see Light and mood), F4 the debug stats window (see Needs), F6 the server's rules (on a multiplayer server, see Multiplayer), F7 the server browser (see The server browser), `,` and `.` turn the camera a quarter turn left or right (see Turning the camera under Light and mood), F10 walls up / walls cut, Shift+F10 zombie silhouettes and Ctrl+F10 opening hints (see Cutaways and Opening hints under Light and mood), M mute, O hide or show the objectives and hints (see The first night), Ctrl (or Cmd) + `=` / `-` the UI scale (see UI scale), Ctrl+0 back to 100%. Walk into an open or broken window to climb
through it. Quitting (closing the window, `--exit-after`, or Ctrl-C, SIGTERM or SIGHUP in the terminal: `net::shutdown`,
not Bevy's handler; a second Ctrl-C quits unsaved) autosaves; `--render-to` never does.

Saves in the client: `quicksave.zsave` (F5) and `autosave.zsave` (on quit) in the saves folder (`--saves DIR`,
default `saves/` in the working directory, git-ignored; point it elsewhere when testing). `--load NAME|PATH` starts
from a save (a name in the saves folder, e.g. `--load autosave`, or a path): its own seed, map and players, so the
world options and `--fight-demo`/`--siege` don't apply; `--load-anyway` loads one whose game data changed in a way it
can't simply take, for `--load` and F9 alike, with no prompt. F9 swaps the quicksave into the running game like a rejoin:
the doors, windows and cracks that differ arrive as edge changes, the player's prediction restarts from the loaded
state (unconfirmed inputs dropped), zombies and items follow from the first snapshot, the run stats are the save's (a dead player stays dead, with its
death screen; a save from before #13 has none, so they start empty and the cause shows as unknown),
and blood splats and particles are cleared (`fx::ClearFx`): they belong to the timeline left behind. A quicksave of another map (seed or size) is refused with a message: start with `--load` for that.
`--quicksave-after SECS` / `--quickload-after SECS` press F5 / F9 by themselves, for verification. A yellow line
under the HUD says what was saved or loaded (or why not), and stdout gets the same line.

**UI scale (#109, `ui_scale.rs`):** one setting, 75% to 150%, that every panel follows (the HUD, status tags, inventory,
skills, health, the current action, the death screen, the light preview and the yellow line), through Bevy's `UiScale`.
Ctrl (or Cmd) + `=` / `-` (numpad `+` / `-` too) steps it by 5%, Ctrl+0 puts it back to 100%, and the yellow line says
the new value. The panels are laid out for a screen of at least 1280 x 720 UI pixels, so above 100% the scale used stops
where the window has no more room (`ui_scale::fitted_pct`): 150% is all of it on 1920 x 1080, 125% on 1600 x 900, 100%
on 1280 x 720, and the yellow line says so ("UI scale 150% (125% fits this window)"); the setting itself is kept as
chosen and resizing the window follows it. The panels hung from the top (inventory, health, skills: `hud::TopPanel`) sit
under the stats text and stop above the vitals and the current action, however the text wraps (`hud::fit_top_panels`),
and scroll with the mouse wheel when their content is taller than that (`hud::scroll_panels`; where two overlap, the one
drawn on top; the wheel doesn't zoom the camera then). The yellow line sits under the stats text, drawn over the panels
on a dark backing. **Client settings file:** `settings.ron` in the saves folder (`--saves DIR`, default `saves/`), RON
(`ui_scale::ClientSettings`, every field defaulted: the UI scale, and since #97 each panel's place and open state and
the hotbar's slots, all through `ui_scale::SettingsStore`), written when the
scale changes in game; a missing or damaged file means 100% (a damaged one with a warning on stderr). `--ui-scale PCT`
starts at that scale for the session without saving it (for screenshots and tests). An offscreen render (`--render-to`)
never reads or writes the file: its pictures depend on the flags alone (`--hud --ui-scale PCT`).

**Hotbar (#97, `hotbar.rs`):** nine slots along the bottom, keys 1-9 (or a click). A slot holds a kind of item (its
id), and its key does the likely thing with the one you carry (`hotbar::press`, sending only the intents the inventory
panel sends): a weapon or tool into your hand (`Equip`; what was there goes into the bags), a light into the other hand
when the first holds something one-handed, food and drink eaten or drunk (`Use`), a pill taken, clothing worn, a
dressing or antiseptic opens the health panel to pick the wound. A slot shows its key, the item's colour and name and
how many you have in your hands and bags; the one in your hand is outlined, one you don't carry dimmed (its key says
"no flashlight on you"). Weapons and lights take the first free slot by themselves when they first come into your
hands or bags (`Hotbar::auto_fill`; one taken off the bar stays off while you keep carrying it). With the inventory
open, point at a row and press 1-9 to put that item on that slot (the same key again takes it off, an item already on
another slot swaps with it; `inventory_ui::HoveredItem`); right-click a slot to empty it. While an item's right-click
menu is up the number keys are the menu's, and Ctrl / Cmd with a number is never the hotbar's. The slots are kept in
`settings.ron` (`ClientSettings::hotbar`). `--hotbar ID,ID,...` (`-` an empty slot) sets them for a session (pictures:
offscreen never reads the file), `--hotbar-press K@SECS[,K@SECS...]` presses keys by itself (recordings).

**Panels remember (#97, `panels.rs`):** the inventory, health and skills panels (`panels::PanelId`, beside
`hud::TopPanel` on each root) are dragged by their title (a press on the 40 UI pixels along the top that moves 4 or
more; not while the panel is scrolled down, when its title is out of sight) and stay where they're let go:
`PanelPlaces` keeps the top-left corner in UI pixels and `hud::fit_top_panels` puts the panel there, kept on the screen
and above the vitals whatever the window's size (`panels::placed`). A double-click on a title puts it back where it's
laid out. Each panel's place and whether it's open are written to `settings.ron` when they change
(`ClientSettings::panels`) and come back at the next start (`--inventory` / `--skills` / `--health` still open theirs).
`--panel-at inventory:X,Y[;health:X,Y][;skills:X,Y]` places them for a picture; `--drag-panel PANEL:DX,DY[@SECS]` drags
one by its title by itself (recordings). Everything kept between sessions goes
through one resource, `ui_scale::SettingsStore` (`change` writes the file only when something changed; offscreen it
holds the defaults and writes nothing), so the UI scale, the panels, the hotbar and the graphics settings never lose
each other's values.

**Readable text (#97, `theme.rs`):** no HUD text is smaller than 14 UI pixels (`theme::SMALL`; body 15, titles 22; a pixel up each when Barlow replaced the monospace), and
the text colours (`theme::TEXT`, the quiet `theme::DIM`) keep at least 7:1 contrast on every backing over the brightest
ground the game draws (sunlit concrete), `TEXT` over pure white too (`DIM` 4.5:1 there). A test works it out from the
textures' own pixels, blended in linear light as the UI is drawn (which is why the sheets are 96% opaque: a few per
cent of sunlit ground behind a dark panel is a lot). Checked in offscreen pictures at 1280 x 720 and 1920 x 1080.

**The skin (Linear DCM-61, `theme.rs`; the look is in docs/art-direction.md, UI):** one rough painted skin, and one
module every panel takes it from: the inventory with its right-click menu and Make list, the health and skills panels,
the HUD (weapon panel, status icons and their tooltip, hotbar, current action, the first night's objectives and hint),
the death and morning screens, the server's rules panel, the server browser, and the hold-up bar and search panel. **New panels take their text sizes, colours, backings, buttons, bars and rules from
`theme.rs`; nothing sets a backing colour of its own.** The textures are made in code when the client starts
(`theme::ThemePlugin`, before the UI's plugins; six small images at fixed handles, about 130 KB in all, so the helpers
need no resource and a test app without the plugin just draws nothing there): `theme::sheet()` (a panel: a charcoal
sheet with dry-brushed, ragged sides; give the node `SHEET_PAD` of padding so nothing stands on the ragged part),
`strip()` (the small pieces of the HUD, tooltips, menus; `STRIP_PAD`), `button(on)` (a plain light button with a worn
edge, its text `theme::INK`; `keycap` for a key to press), `slot(tint)` (a hotbar slot: hatched, a worn rim; its tint
is its state), `bar_track()` / `bar_fill(colour)` (a dry-brushed bar), `rule()` / `spawn_rule` (a brushed line),
`dab(colour)` (a blot behind a status glyph), `title(c, text)` (a panel's title: centred, capitals). Each is an
`ImageNode` on the node itself, cut in nine (`TextureSlicer`) with sides and middle repeated at their own size, so the
brush keeps its scale on any panel and follows the UI scale; the textures repeat without a seam (a test). One quad a
node, in Bevy's UI pass. **Rows stay flat tints** (`theme::ROW`, `ROW_HOVER`, `ROW_PICKED`, `INSET`: there are
hundreds), and a line of text that carries its own backing (the stats text, the yellow line, chat) a flat
`theme::HUD_BG`. Hover on a textured node changes its `ImageNode::color`, not a `BackgroundColor`. **Typefaces:** Barlow for
text and the HUD (`theme::font`; `theme::strong`, its Medium, for values, notes and button labels) and Barlow Condensed
SemiBold for titles, tabs, column and section headers, set in capitals (`theme::head`); both SIL OFL 1.1, the three
weights used in `assets/fonts/barlow/` with `OFL.txt`, compiled into the client (`include_bytes!`, at fixed handles
like the textures, so offscreen renders and tests have them). Figures are tabular (the `tnum` feature, on in every
`theme` face), so counts, weights and clocks line up and don't shuffle as they change. Bevy's built-in monospace
(`theme::mono`) is kept only for the developer's read-outs: the stats line and keys at the top left, the debug stats
window (F4) and the light preview's label (F3). **Every text takes its `TextFont` from one of those four functions.**
Nothing may assume a letter's width: lay text out with flex, and where a size must be known before layout (the
inventory's turned tabs, the hotbar's slot test) ask `theme::width(face, text, size)`, which reads the advances from
the face's own tables. Cost: nothing measurable (the inventory open
at 1280 x 720, offscreen on the GPU-less build VM, software rendering under other load, best of three: 135.5 ms a
frame against 131.5 ms before, with runs of either from 130 to 390 ms; about 40 textured quads on screen with the
inventory open). Not yet measured on a GPU.

HUD: top left, the stats line and the keys. Bottom left, the weapon you hold and your stance, your health as one
brushed bar (pale, yellow under half, red under a quarter: `hud::hp_colour`; Linear DCM-61 brought this one bar back),
and this life's kills, time played and distance (the server's run stats). **No other stat bars** (Linear DCM-57): over that panel stands a row of
status icons (`needs_ui.rs`, see Status icons under Needs; the two share `hud::BottomLeft`, a column in the corner),
which appear only when something passes a level; the raw
numbers are in the debug stats window (F4, see there). Bottom right, the hotbar (above). Dead: the death
screen (`death.rs`, #13): the cause of death in plain words, the game day and hour, and the run summary (game time
survived, time played, zombies killed, distance walked, things looted, containers searched, wounds taken, HP lost), all
from our run in the snapshot, so it is right after a quickload or `--load` too. **R starts a new run:** a fresh world
with the same seed, map size and settings as the one being played (the same town from the start: the client's map copy
and meshes are built once for that map, so a new seed would mean rebuilding them; `net::LocalServer::new_run`, taken up
like a quickload through `saving::WorldSwap`); `--at`, `--fight-demo` and `--siege` don't apply again. **Q or Esc quits**
(autosaving). `--new-run-after SECS` presses R by itself once dead. `--autoplay` still gets up again with
`Intent::Respawn` (a new run for that player, same world). A cyan GOD / IGNORED
tag beside the stance (and in the stats line) while a dev toggle is on.

## The world

- 1 tile = 1 m; chunks are 16 × 16 tiles. Each tile owns its **north and west edges** (wall, window,
  door, arch, fence), so every edge is stored once (`sim::map`).
- Edge states (`map::Edge`): `Wall`, `Window` (shut glass: blocks movement, not sight), `WindowOpen`
  and `WindowBroken` (passable, but crossing is a slow climb: `TileMap::in_window_opening`, within
  `CLIMB_ZONE` of the window line), `DoorClosed` (blocks both), `DoorOpen`, `DoorBroken` (a permanent
  opening; doesn't block sight), `Arch`, `Fence` (waist high: doesn't block sight, and like an open window
  crossing it is a slow climb, for players and zombies; blows and hands reach over it). Helpers: `is_door`
  (openable), `is_window`, `is_breakable` (shut door or intact window), `is_climbable`, `broken()`.
- Shut doors and intact windows have **hit points** on the server (`World::edge_hp(EdgeRef)`), stored
  sparsely in a sorted map (only damaged edges; part of the state hash). Full HP and every other
  door/window number is in `ZombieTuning` (`door_hp` 40, `window_hp` 6). Broken edges have none.
  Every edge change (open, shut, break) goes out in snapshots as an `EdgeChange`; clients apply it to
  their map copy, so movement prediction (climbing included) stays exact. Hit points go out as changes too:
  `Snapshot::edge_damage` lists each door or window whose HP changed since the last snapshot once, as a
  fraction of full (`net::EdgeDamage`, from `World::take_edge_damage`; 1 = broken or whole), and a joining
  client gets every damaged edge (`EdgeDamage::all`). The client draws three stages of cracks (dark splits and
  pale splinters on doors, white star cracks on glass) as child overlays of the leaf or pane (`world_render.rs`).
- Buildings are **templates** in `assets/data/buildings/*.ron`: ASCII room layouts per storey, plus
  doors, windows, arches and furniture by `(x, y, side)`. Walls appear wherever rooms differ. The
  bottom row is the street-facing front. `BuildingTemplate::validate` explains mistakes (furniture
  outside, overlapping or across a wall between rooms, two openings on one edge). Add a house by adding
  a file; the template tests walk every shipped one (every room reachable from outside, upstairs from
  the stairs, no doorway blocked, every piece of furniture reachable, containers mapping as meant).
  Shipped (#17): six houses (bungalow, two-storey family house, ranch with garage, narrow townhouse,
  cottage with an open porch, L-shaped house round a covered patio), two offices (`kind: Office`: a
  small one-storey office, a two-storey office block) and two shops (`kind: Shop`: a corner pharmacy,
  a general store and hardware). Houses are at most 13 × 15 tiles to fit a lot. `kind: Civic` (none
  shipped yet) is for landmarks: town hall, courthouse, church, school, up to about 40 × 40 m (see worldgen).
- `sim::worldgen` (#18, #77, #80, PLAN §4.1a) lays the town out **by district** (`GenConfig::blocks` is its nominal size
  in blocks of about 74 × 66 m, default 5 × 4: about 370–420 × 275–320 m, grown to fit its core). A **main street**
  runs east–west across it (9 m of asphalt, 3 m sidewalks, a solid centre line) and on out of town both ways. Its
  middle is the **core**: the **civic district** (north of the street the **town square**, a green with a paved
  middle, paths and a ring of trees, usually with a landmark block beside it, now and then one each side or none;
  south of it one wide block of two **landmark lots** or a single one, about 42–52 × 48–62 m: 1–4 landmark lots in
  all; the south side's civic district starts in line with the north side's or 14–22 m along), with **commercial
  blocks** either side (`commercial_blocks` on each side of the street, default 2, split at random between the two
  ends, so the civic district and its square sit anywhere along the row; serde default, so older settings load;
  50–70 m wide): **shops and offices** facing
  the main street (`kind: Shop` / `Office`, shops twice as likely), packed 1–3 m apart on shallow lots paved up to
  their fronts, now and then a parking lot, half the time an **alley** (4 m of asphalt) behind them, and a row of
  houses behind facing the back street. The blocks south of the street are sized so their side streets either line
  up with the north side's or are clearly offset, so most meet the main street at a T. Each side's row of core blocks
  falls into 1–3 **runs** (`CoreRun`), each 44–62 m deep (the civic district's at least 48) with a back street
  behind it; between runs, and at each end of the row, a street runs on from the main street out to the forest
  (never in line with one across the main street), and neighbouring runs differ in depth by at least 12 m, so the
  back streets step and stop instead of running the length of the core. The
  **residential districts** (beyond the ends of the core, and behind it) are split again and again by narrower side
  streets (6 m of asphalt) into blocks of varied sizes (`TownBuilder::split`: mostly T-junctions, now and then a
  crossing that carries straight on, now and then a long block; a street never ends against the backs of lots), and
  over half the time where one fits, by a **bent street** (`bent_split`, `Plan::bends`): a dog-leg across a strip,
  jogging 14–30 m half way; an L in from one end that turns to a long side (or stops at the forest); or a crescent in
  from a long side and back out to it round a block. The pieces left are rectangles; where two meet with no street
  between, no lots face that way. Each turn's corner square is painted so the asphalt turns and the sidewalk runs
  round the outside (`paint_elbow`). Blocks are filled with rows of house lots facing whichever
  streets run round them (houses turned to face north, south, east or west; corner lots on the side streets) or, in
  up to three blocks (at least one), a **cul-de-sac**: a dead end with a turning circle (asphalt within 8.5 m) and
  houses round it (the generator leaves each one on the map as a `map::TurningCircle`, for drawing it round; not
  saved). Blocks on the edge of town stop short of it by their own few metres and the woods come up to
  them, so streets run on into the trees and stop at the forest (some with a dirt track); lots backing onto the
  forest are **outer** lots, wider, set further back and wilder. The street out north and the one out south are the
  dead ends nearest the middle of town, with a dog-leg in the forest. House lots take any `kind: House` template
  that fits (5–6 m back, a garden path to the street): in inner blocks nearly all houses (95%), by the forest
  more bare yards and overgrown vacant lots; blocks are split when over 72 m deep, so back yards stay shallow; one
  pocket park per town, back fences and yard trees. **Landmark lots** take the biggest `kind: Civic` template that
  fits (a town hall, courthouse, church, school; or a shop or office too big for the strip, over 24 × 16), with a
  paved forecourt and parking beside it; with none (none ship yet), each becomes the biggest office that fits with
  parking, a green, or a parking lot (never the same twice running). Templates are chosen by kind and footprint,
  rotated to face their street and mirrored at random. With no shop or office templates the commercial blocks get
  houses; with no houses, residential lots stay empty. The player wakes up in a ground-floor bedroom of a house. The
  town core (`World::town_core`, from the generator, not saved: two rectangles, the core blocks north of the main
  street with the street, and those south of it) is the civic and commercial blocks. `Generated::plan`
  (`worldgen::Plan`) lists the blocks with their district (`Civic`, `Commercial`, `Residential`, `Outer`) and use, the
  streets, the square, landmark lots, cul-de-sacs, alleys, dead ends and bent streets (tests, tools; not kept by
  the world). The default town holds about 90–130 buildings on a 480 m map, generated in about 50–60 ms (world
  included). Changing the generator changes every map: saves made before #80 load only with `--load-anyway` /
  `load {force: true}`. Lot details use coordinates relative to their lot, so a bigger map gives the same
  town. `print_town` (an ignored test; `ZOMBYE_SEED`, and `ZOMBYE_PPM=PATH` for a one-pixel-a-tile picture) prints it.
- **Curves (#82, `meshgen/curves.rs`):** the tiles stay square (walking, sight and the sim go by them), but what's
  drawn follows real curves, faceted, never stepped. Turning circles are true circles (48 facets) of asphalt, kerb and
  a sidewalk ring with slab joints across it, opening into their street through a 4 m reverse curve each side. Every
  sidewalk corner with road on its three other sides gets a rounded kerb: 4 m at junctions (the back of the sidewalk
  follows at the same width where it meets open lawn), 1.5 m where the sidewalk is all there is (half of its width at
  the end of a strip between two roads, so the curves meet). The tiles have no way from an alley or a parking lot
  through the sidewalk to the street, so one is drawn: asphalt across the sidewalk where an alley ends or a lot's
  aisle meets it (up to 6 m), kerbs turning in. Driveways flare (1.5 m) where they meet the sidewalk, and the
  dog-legs on the roads out are bends of constant width (inside radius up to 6 m, less where a tree stands in the
  way) with the dashed centre line round them. Stop lines sit back where the kerb still runs straight; zebra bars run
  out to a curved kerb as far as it leaves room. How: `Curves::find` works out the shapes reaching a chunk from the
  tiles round it (its eight neighbours, read once) and the turning circles; each is a list of convex regions drawn
  as another surface; a tile one crosses is cut into convex pieces (`Curves::pieces`): its own look clipped to what's
  still its own, plain asphalt, concrete or the neighbouring ground on the rest; straight kerbs and gutter grit along
  tile edges stop where a curve takes over (`Curves::kerb_span`), and curved kerbs, grit, ring joints and bend
  centre lines are drawn by the chunk each stretch's middle is in. Cost: chunk building about 0.24 ms (0.22 without
  curves; `chunk_build_time`, an ignored test, times every chunk of the default map), frames the same.
- **Street props (#61, `sim::street_props`, `assets/data/street_props.ron`):** mailboxes, fire hydrants and bins
  that the generator puts outside. The file lists the kinds (`id`; `model`, the model drawn (`assets/models/street/<model>.glb`, built in Blender: mailbox, fire_hydrant, trash_can), the id when
  left out; grey-box `size` and `color`; `blocks`, default true; `radius`, how far a body is kept from its middle,
  default half its larger side, at most 0.5) and the rules, applied in order: `GardenPath` (on the lawn beside the
  street end of a house's garden path, facing the street; `chance` per house), `HouseKerb` (at the kerb 2-3 m to one
  side of a house's path), `ShopKerb` (at the kerb in front of a shop or office on the main street), `Kerb` (along
  every straight kerb in town, `spacing: (min, max)` metres apart). Validated by `parse_street_props` (unknown fields,
  ids, sizes, colours, radius, chances, spacing, a rule naming a missing kind; errors name the kind or rule), loaded by
  `net::content::load_street_props` into `WorldConfig::street_props`; **a data folder without the file has none**
  (as `WorldConfig::new`). Placed last by `worldgen::generate_with` (`generate` places none), each rule from its own
  seeded stream, so the rest of the town is the same with or without them and the same seed gives the same props. A
  prop stands in the middle of its tile, turned a quarter at a time to face the street; never on a road, path or
  driveway (straight out from a garage door to the first sidewalk and across it, a tile either side, as the client
  draws them), beside a
  door, or next to another prop (each keeps the eight tiles round it free); kerb props only on a straight kerb (5 m
  of it each way: clear of the 4 m kerb curves at junctions), away from turning circles, alley mouths and parking-lot
  entries, and not in front of a garden path. Shipped: all three block; seed 1 has 48 mailboxes (most of the houses
  whose front door has a path), 28 bins and 101 hydrants (both kerbs of every street). The map keeps them in
  `TileMap::street_props` (sorted by tile; `street_prop(t)`) with their kinds in `TileMap::prop_kinds`; a blocking
  one's tile is `Feature::StreetProp`, so the walk grid, flow fields, the nav graph, `clear_path` and every
  `tile_blocked` check go round it like a tree, and `TileMap::resolve_circle` keeps bodies the kind's `radius` from
  its middle (`street_prop_radius`; 0.3 m for one whose kind isn't known), the same function for the server and the
  client's prediction. Not saved (regenerated with the map; a map stored whole gets the generated map's props where
  its tiles still have them). The client draws each with `FurnitureModels::add_at` (the model at its own size, never
  stretched to the tile; `ZOMBYE_EXTRA_MODELS` for ones still in the art repo), a grey-box of its size and colour
  without one, merged into the chunk's props mesh (`meshgen/street_props.rs`, called from `spawn_chunk`): no draw
  call of its own. Cost: town generation the same (~6 ms, the props under 1 ms), chunk building +0.003 ms,
  frames the same.
- **Buildings dressed (#59, `meshgen/building_kit.rs`):** `world_render::build_building` (pure: map in, a building's
  ground-floor, upper and roof meshes out) runs the building kit over every wall and the roof, merged into the same
  meshes (no draw calls of their own). Houses: a concrete foundation and a pale water-table board below the stub
  height, siding board lines every 0.3 m, a belt board between storeys, corner boards on outside corners (trim
  off-white, dark on the palest sidings); window frames with a sill and a cross of muntins, door casings, two steps
  up to every outside door with no porch; fascia along the eaves and up the gables, a gutter, a ridge cap, gable
  walls up to the roof with a small window, a brick chimney with a cap on about half and a TV aerial on about 40%.
  Shops and offices are brick (buff, tan, red by their siding pick) with brick courses, a parapet with coping round
  their flat roof and a rooftop unit; shops add a dark stall riser under the front windows, a blank sign board along
  the front and an awning striped in one muted colour and bone white over the sidewalk; offices a canopy over each
  outside door. **Porches** come from the template's optional `porch` (`template::PorchDef`, serde default, checked
  by `validate`): `Room('P')`, a porch room under the main roof open through arches (the cottage's, the patio
  house's), gets posts along its open sides and a railing across its arches but the way in (an arch onto a garden
  path, else the one nearest the door); `Front(from, to, depth)`, tiles `from..=to` of the bottom row, 1 or 2 deep,
  is built onto the front outside the footprint: a deck 0.22 m up with boards, a step at the way in (in line with
  the door), posts at most 2.6 m apart, a railing and a shed roof of its own (skipped where the yard has a tree, a
  fence or anything else in the way; no front bed under it). **Porches are render only**: posts and railings never
  block walking or sight (the map doesn't change), so you can walk through them. Placing a building records its
  kind, front and porch in `Building::look` (`template::BuildingLook`; derived, not saved, in no hash: a map stored
  whole has none and is drawn as a house facing south, without a porch). Cutaways: wall details carry their wall's
  anchor and drop with it to the stub (vertical trim crossing the stub height gets a cap); porch roofs, awnings,
  canopies and porch posts are `CutKind::Fade` pieces, thinned out in a stipple while they stand over the player (see
  Cutaways under Light and mood), and the roofs among them fade out with the upper storeys while you're inside;
  railings, decks and steps never drop. **Lit windows:** about one
  room in five has a light on (`building_kit::room_lit`, by a hash of the room; never halls, porches or garages),
  and about one gable window in six: their glass (`CutKind::Lit`) glows warm amber at night, seen or not, by
  `WorldParams::lights` (1 at night to 0 by day, from the daylight). Cost: building meshes about 0.27 ms each
  (0.15 ms before; `building_build_time`, an ignored test, times every building of the default map and says where
  one of each template stands), about 9,800 triangles a building (6,000 before); a frame (1920 × 1080 offscreen,
  2,000 zombies, by day, default zoom) about 0.7 ms more: 9.3 against 8.2-8.7 ms mean.

## Players, intents and prediction

- A player is a server entity (`sim::players::Player`): position, facing, stance (Walk 3 m/s, Run 5.5 m/s,
  Sneak 1.4 m/s; at most `CLIMB_SPEED` 0.7 m/s while climbing through a window) and endurance (0..1; running drains it, and once it runs out you can't run until it
  recovers a bit). `players::step_movement` is the one movement function: the server tick and the
  client's prediction both call it, so keep it pure (map + state + input + dt in, state out). What the
  player carries changes movement through `PlayerState::mods` (`MoveMods`: speed, run drain, regen, can run,
  can climb), which the server sets from the load (`World::refresh_carry`) and snapshots carry (`PlayerView::mods`),
  so prediction with the snapshot's modifiers stays exact (Fitness, #46, goes there too).
- `Intent`s (`crates/net`): `PlayerInput { player, seq, move_dir, facing, run, sneak, aim }` every tick (`aim`: the
  right mouse held; `step_movement` walks an aiming player at `AIM_SPEED` 2.2 m/s with no running, `PlayerView::aiming`
  says so, and the server takes `Attack`, `Shove` and `Stomp` only from an aiming player; not saved)
  (the server keeps the newest by `seq` and applies it once per tick; `move_dir` is clamped to
  length 1), `Interact { player }` (opens or closes the door or window that player faces, within
  1.6 m of its server position, otherwise ignored; doors make `door_noise`, windows the quieter
  `window_noise`; broken ones can't be shut), `SmashWindow { player }` (breaks the shut or open window
  it faces; loud, `glass_break_noise`), `Attack`, `Shove`, `Stomp`, `PickUp` (all `{ player }`, see
  Combat and Items), the item intents (`MoveItem`, `Equip`, `Wear`, `Unequip`, `Drop`, `Use`, `Search`, see Items), `Craft { player, recipe }`
  (see Crafting),
  `Sleep { player, sleep }` (lie down or get up, see Needs; an action or item intent from a sleeper only gets it up),
  `Seen { player, tick }` (a remote client, every tick: the snapshot it is showing, see Lag-compensated melee under Combat),
  `Say { player, text, volume }` and `Gesture { player, gesture }` (see Talking and gestures),
  `Restrain`, `Free`, `Frisk` (each `{ player, target }`), `TakeFrom`, `StopHands`, `Struggle { player, on }` and
  `Group { player, op }` (see Hold-ups and groups), `Aim { player, pitch }` (how far up or down of level the player
  aims a gun, see Guns),
  `Respawn { player }` (single-player/debug: a dead player gets up at the spawn point), `Dev { player, god, ignored }`
  (dev toggles, below), and the debug/admin `SetGoal`, `SpawnZombies`, `SpawnItem { item, count, pos }`, `SkipTime`, `MakeNoise`.
- **Dev toggles** (`players::DevFlags { god, ignored }` on the player, `World::set_dev`; `Intent::Dev` with `Option<bool>`s,
  `None` = leave it): **god** takes no damage and can't die: zombie strikes still roll, land and become events and hit
  feedback, HP just doesn't drop (`World::hurt_player_by(pi, damage, DeathCause)` is the one place a player loses HP;
  anything new that hurts goes through it and names its cause, see Run stats). **ignored**: zombies don't see the player (`Target::ignored`, skipped by `sight_pass`) or hear anything it
  makes (its `SoundSource::Player` sounds are dropped in `World::step` before the senses, far tiers and migration listen),
  and turning it on makes every zombie after it (chasing, lunging, striking, battering toward it) investigate where it last
  saw it, then wander; one it hits finds nobody when it recovers. Off again, it is noticed as usual from the next tick.
  Zombies still walk round its body. Not saved (`#[serde(skip)]`; the save format is unchanged and a loaded world has
  them off; the client's F9 sends them again), kept on respawn, in `state_hash` only as bits that are 0 when off (so
  worlds without them hash as before). `PlayerView::dev` carries them. Single-player (`LocalServer::new`/`from_world`)
  takes `Intent::Dev`; a server others join (`LocalServer::hosted(world, allow_dev)`) drops it (counted in `refused_dev`)
  unless `allow_dev`, and the multiplayer server (`zombye-server --host`, see Multiplayer) drops it and the debug intents unless started with `--allow-dev`. The MCP
  server, a dev tool, always allows it.
- `LocalServer::join` adds your player and returns its id. Snapshots carry `players` with
  `last_input_seq` (plus `hp`, `dead`, `weapon`, `action`, and `action_left` / `ready_in`: seconds until the
  action lands and until the player can act again, which time the swing animation); the client resets its predicted player to
  that state and replays newer inputs. Swings cost endurance on the server only, so prediction is
  corrected by the next snapshot when you swing (movement itself stays exact). Snapshots also carry
  the full list of ground `items` (small for now), each player's `busy` (seconds left on its item job) and `jobs`
  (queued), `private` views (`PrivateView`: inventory, load, the open container's contents, the job queue's kinds) only for players whose
  inventory, open container or queue changed since the last snapshot (`World::take_private_dirty`; the client keeps
  the latest, and `Snapshot::for_player` keeps only a client's own), and `item_notes` (what came of item requests:
  done, or why not). On joining the client also gets the weapon and item definitions; snapshots name weapons and
  items by index into them.

## Combat (`sim::combat`, `sim::items`)

- **Weapons are data:** `assets/data/items/weapons.ron`, a list of `items::WeaponDef` (damage
  range, reach to the zombie's body, arc, windup and swing time, max targets, knockback,
  stagger/knockdown/crit chances, crit multiplier, endurance cost, noise, spawn weight, `found_in`
  room types, grey-box length and colour, and `skill`: the skill it trains, see Skills), validated by `items::parse_weapons` and loaded by
  `net::content::load_weapons` into `WorldConfig::weapons`. Exactly one is `Unarmed` (bare hands);
  if the content has none, `WeaponDef::fists()` is added. Add a weapon by adding an entry.
- **Weapons are items** (an item with `weapon: Some(id)`, see Items): the weapon swung is the one the item in the
  player's `Hand` slot names (`Player::weapon`, derived from the hand). The world places 0–2 weapon items per house
  on free ground-floor tiles (at least one in the spawn house), weighted by `spawn_weight` and ×3 in `found_in`
  rooms, from its own seeded RNG (`GroundItem { id, stack, pos, angle }`). A player who dies drops everything it has
  where it fell.
- **Actions** (`players::ActionKind`: `Swing`, `Shove`, `Stomp`): start when the intent arrives (or are
  queued if it comes within `action_buffer_secs` of the last one ending), cost endurance at once, and
  land after a windup. At impact the server finds zombies in reach and arc of the player's facing with
  a clear walking line (glass blocks; a fence doesn't), nearest first up to `max_targets`, each extra target
  taking `multi_hit_falloff` less. Damage, knockback (a velocity, capped at `MAX_KNOCKBACK_SPEED` so it
  can't push through walls), then seeded rolls (`World::combat_rng`): knockdown, else stagger. From
  behind (the zombie faces away) crits are more likely and knockdowns easier; knockdowns are easier
  on a staggered zombie. Below `tired_below` endurance actions get weaker and slower; running out
  winds you. Each action makes a noise heard that tick. **Shove:** short reach, wide arc, up to 3, no
  damage, always staggers, may knock down (`shove_knockdown_chance` 0.25, +0.03 for a new survivor's Strength 1:
  a little over one shove in four floors a lone zombie facing you, 0.45 from behind). **Stomp:** hits a downed zombie at your feet in front.
  Swings and shoves pass over downed zombies. Results are in `World::action_events` and `hit_events`.
- **Combat events (hit feedback):** every hit, kill, shove, stomp and wound is also pushed to
  `World::combat_events` (`combat::CombatEvent { kind, pos, dir, zombie, player, damage, crit }`, kinds
  `ZombieHit`, `ZombieKilled`, `PlayerHurt`, `Shove`, `Stomp`; `pos` is the body hit, `dir` the blow's
  direction). They accumulate until a snapshot drains them into `Snapshot::hits`, like `edge_changes` (capped at
  `MAX_COMBAT_EVENTS` if nobody drains them; not in the state hash). The client turns them into blood spurts
  (short-lived red cubes, at most 400 alive), blood splats on the ground (dark-red decals drawn with the ground
  material so fog darkens them, at most 240, oldest removed first; each one sits on one of 10 height layers,
  never the same as a splat it overlaps, so crossing splats don't flicker), a pale flash and a small kickback on the
  zombie hit, a flinch on the player hurt, and for your own wounds a red screen-edge flash and a little camera
  shake (`fx.rs`).
- **Zombie health and states:** per-zombie `hp` (`CombatTuning::zombie_hp`, from a hash of the id).
  `Staggered` (brief; no attacking, no senses) and `Down` (a few seconds on the ground; no attacking, no
  senses, out of crowd separation) end with the zombie going after whoever hit it; the last `GET_UP_SECS`
  (2.0 s, the crowd character's get-up clip, #53) of being down (`down_secs` 4-6 s: the client's 1.3 s fall, a moment
  lying, the get-up) show `ZombieAnim::GetUp` (still `Down` to the AI, so still stompable). `Dead` zombies are
  corpses: left out of the spatial hash, the AI, senses, separation and swings; their `timer` counts up from death,
  and at the end of each tick the world removes those older than `ZombieTuning::corpse_secs` (900) and the
  oldest beyond `max_corpses` (300), keeping the columns in id order (`Zombies::remove_where`; ids are never
  reused; `World::removed_corpses` lists them). Snapshots just stop listing them and the client despawns them.
  `ZombieAnim`: `Idle`, `Walk`,
  `Attack`, `Stagger`, `Down`, `Dead`, `GetUp`, `Thump` (battering a door or window), `Lunge` (the last couple of
  metres at a player) and `Climb` (moving through an open or broken window's or a fence's climb zone; the client
  plays a climb or a vault by which edge it is). The last two came with `wire::PROTOCOL` 14.
- **Player health:** `hp` (100). Each `ZombieAttack` lands with `zombie_hit_chance`, plus
  `zombie_crowd_bonus` per extra zombie attacking that player, and leaves a wound on a body part (see Health and
  wounds; `landed`/`damage`/`part`/`wound` on the event). At 0 the player is dead: no movement or actions, zombies lose them.
- **Player against player** (#65, PLAN §4.8; `CombatTuning::pvp`, off by default: single-player and co-op; the client's
  `--pvp`, the server's `--pvp on|off`, MCP `pvp {on}`; saved with the settings; `--host` turns it on unless `--pvp off`,
  see Multiplayer): with it on, other
  living players are targets of swings, shoves and stomps **in the same list as zombies** (`combat::land`: same reach,
  arc and clear walking line, nearest first up to `max_targets`, the same falloff); off, players are never targets.
  A swing or stomp that lands (`World::strike_player`) rolls the same crit and damage as against a zombie, times
  `pvp_damage_mult` (0.4: about eight blows of a bat, four of an axe), and leaves a **wound** on a body part (the zombie strikes' `front` / `behind` tables): blades and
  axes gash, blunt weapons bruise or gash (`pvp_blunt_gash` 0.3, always on a crit), fists and boots bruise; worn clothing
  may stop a cut to a bruise (`World::protection` × the wound's `protection`), which takes `pvp_stopped_damage` (0.5) of
  it. No infection (only zombies carry it). HP goes through `hurt_player_by` (god mode: the blow lands, nothing is
  lost), a blow wakes a sleeper and drops queued treatments and crafts. Then knockback (the victim is moved at once, up
  to 1 m, stopped by walls, never into a window opening), a knockdown roll, else a stagger roll, with the same chances
  and behind / staggered multipliers as for zombies. **`Player::knock`** (`players::Knock { down, timer }`): staggered
  `player_stagger_secs` (0.5-0.8 s) or on the ground `player_down_secs` (1.4-2 s, time for two stomps); either way no moving (`MoveMods` speed 0,
  set and cleared in `step_player_actions`, after movement, so prediction stays exact: only the tick of the push itself
  is corrected by the snapshot), no actions, its action and item jobs dropped. Swings and shoves pass over a player on the
  ground; a stomp hits only those (a bruise) and leaves them down. Each hit is a `PlayerHitEvent` in
  `World::player_hit_events` (per tick) and a combat event for feedback: `PlayerHurt` with `by` (the attacker; 0 for a
  zombie's strike) or `PlayerShoved`. A kill is `DeathCause::Survivor { by, weapon, stomp, part, wound }`
  (`describe_with(weapons)`: "Killed by a survivor with a crowbar: a gash to the head", "Beaten to death by a survivor",
  "Stamped to death by a survivor") and counts in the killer's `RunStats::player_kills`, apart from zombie `kills`.
  `PlayerView::knock` carries the state; `knock` is in `state_hash` only while set (worlds without PvP hash as before).
- **Lag-compensated melee** (#23, `sim::lagcomp`, PLAN §3): over the network a client's own survivor is predicted
  and everything else is shown late (the way down, the client's tick and snapshot cushion, the way up, the server's
  tick: about 250 ms at 150 ms of ping), so a blow lands **where its attacker saw the target**. Each tick, before its
  `PlayerInput`, a remote client says which snapshot it is showing (`Intent::Seen { player, tick }`,
  `World::player_saw`: never ahead of the world, never back; `Player::seen_tick`, 0 = never said). The world keeps
  where active zombies and players stood at the end of the last few ticks (`lagcomp::History`, one `Frame` a tick,
  only while some player has reported), and `combat::land` tests a blow's targets (reach, arc, the clear line, which
  is nearest, the way the blow goes) at the frame of the attacker's `seen_tick`, **at most
  `CombatTuning::lag_comp_max_secs` (0.35 s, 7 ticks) back** (`World::rewind_tick`; 0 turns it off). The attacker's
  own place is the server's (its client predicts that). Fair to the one hit: who can be hit goes by now (dead or down
  since: not swung at), the map is as it is now (a door shut since stops the blow), and knockback and the rest happen
  to the target where it really is; the cap bounds how far "round the corner" a hit can come (a client claiming more
  lag gets the cap). **A trade of blows** (`World::strike_player`): a blow that staggers, floors or kills a player
  calls off that player's swing under way, unless it was due within that player's own view lag (same cap): on its own
  screen it had already landed before the other's blow showed, so it lands too, at once, from where the player stood
  (`combat::Trade`). The same rule and cap for both sides, whatever their pings. A player that never reports (single
  player, the duel bot, MCP) has no lag: nothing is kept, nothing rewound, no trade window, so single-player runs
  exactly as before. Derived: not saved, not in `state_hash` (after a load blows are unrewound until clients report
  again, a tick). **Client:** on a server our own swing, shove or stomp begins on the click (`LocalPlayer::act`,
  `player::OwnSwing`; `survivor.rs` plays it and moves the snapshots' timing onto our survivor's clock by the view lag,
  the inputs sent that the snapshot shown hasn't applied), so the blow lands on screen when and where the server lands
  it; the stats line says `view N ms behind`. `--lag MS` (client, and `zombye-server --bots`) plays as if MS of ping
  away (`Remote::set_lag`: half each way), to try it on one machine. The server's report line says how far it is
  rewinding. Tests: `sim::combat::lag_tests`, `net/tests/lagcomp.rs` (a real socket at 150 ms: the punch lands where
  the client saw the zombie; with the cap at 0 it misses).
- Every number not in the weapon file is in `CombatTuning` (`WorldConfig::combat`, serde defaults). Ours.

## Weapons in the hand: wear, cramped blows, one hand (`sim::wear`)

- **Weapons wear out and break.** A weapon in `weapons.ron` with a `durability` (the knocks it takes; 0 or left out:
  never wears, as bare hands and guns) takes a knock on a swing that hits something with chance `wear_chance`, each
  level of its skill making that `CombatTuning::wear_skill_less` (8%) less likely; a swing at the air costs it nothing.
  So it lasts about `durability / wear_chance` hitting swings in new hands (shipped, ours: plank 20, golf club 36,
  kitchen knife 50, bat 80, frying pan 100, fire axe 140, claw hammer 165, crowbar 400). At `durability` knocks it
  **breaks**: gone from the hand (bare hands from then on), with the note "your baseball bat broke". With
  `battered_below` (a quarter) of its life or less left it is **battered**: `battered_damage` (0.85) of its damage,
  and a note when it gets there. `wear::Condition` (`Good`, `Worn` at 60% or less, `Battered`), `wear::condition`,
  `World::condition_of(stack)`.
- **The knocks ride on the stack** (`ItemStack::used`, which for a weapon that wears counts knocks, as it counts
  rounds for a gun and goes had for food: **no save format change**, and saves from before load with every weapon
  sound). So a weapon keeps its wear on the floor, in a cupboard and in other hands. `ranged::max_used` holds a save
  to one under the `durability`. One made now is sound (debug spawns, kits, the floor weapons); one found in furniture
  has had a seeded 0-70% of its life (`CombatTuning::found_wear_max`), more often little (`World::wear_found`, by seed, container and stack).
- **The roll** is `hash01` of the seed, tick, player and item (`World::wear_weapon`, called from `combat::land`), not
  a draw from the combat RNG: fights roll exactly as they did before.
- **Cramped blows:** a weapon's `close_range` (bat and fire axe 0.38 m, golf club and plank 0.4; to the target's
  body: bodies touch 0.3 m apart, and a lone zombie striking you stands 0.4-0.6 m off) is too close for a full
  swing: a target nearer, one pressed onto you by the crowd behind it or that you walked into, takes
  `CombatTuning::cramped_damage` (0.6) of the damage. Shove them back first.
- **One hand:** an item's `one_hand: true` (bat, golf club, plank) makes a `two_handed` weapon one that the other
  hand can hold something beside (a flashlight, a bag): it no longer empties the off hand (`ItemDef::both_hands`),
  and swung that way it hits `one_hand_power` (0.75) as hard and takes `one_hand_slow` (1.2) times as long
  (`combat::begin_action`). The fire axe, rifle and shotgun still take both.
- **Shown:** the inventory row and the weapon panel say "worn" or "battered" after the name (nothing while good);
  MCP `inventory` says the knocks left, `weapons` each weapon's durability and close range, `items` which two-handed
  items one hand will do for. `wire::PROTOCOL` 15 (the definitions and `CombatTuning` changed shape).
- **Not yet:** repairing or sharpening, a separate head and handle, a skill for looking after weapons, guns that
  wear, hitting doors and trees.
- Tests: `sim::wear::tests`.

## Guns (`sim::ranged`; #72)

Guns are for fights between players and a last resort against the dead (docs/PLAN.md §4.8): they kill at a distance
and tell the whole town where you are.

- **Guns are data:** a weapon in `weapons.ron` with `category: Firearm` and a `ranged` block (`ranged::RangedDef`,
  unknown fields refused): `ammo` (the item a round is), `capacity`, `magazine` (the item its detachable magazine is,
  if it has one), `reload_seconds` (a magazine change), `round_seconds` (one loose round pushed in), `range`,
  `spread_degrees` and `move_spread_degrees` (how far off the aim a pellet may go, standing and extra on the move),
  `pellets` and `far_damage` (the share of its damage left at the end of the range). Of the weapon's other numbers a
  gun uses `damage` (a pellet's), `windup_seconds` (trigger to shot) and `swing_seconds` (shot to shot),
  `max_targets` (bodies one bullet goes through), `crit_multiplier` (a hit to the head), `knockback`,
  `stagger_chance`, `knockdown_chance`, `endurance_cost` and `noise_radius` (up to 300 m for a gun); `range`,
  `arc_degrees` and `crit_chance` aren't read. Exactly the `Firearm` weapons have `ranged`; a gun names no `skill`
  (shooting trains none yet, and none makes it better). `parse_weapons` checks the numbers, `validate_items` that a
  gun's item has its rounds and magazine among the items (the magazine not stacking, tagged `magazine`). Shipped
  (ours): `pistol` (12 in a `pistol_magazine`, 34-46 a shot, 45 m, noise 120 m), `hunting_rifle` (5 loose
  `rifle_rounds`, 85-115, through two bodies, 110 m, a slow bolt, noise 180 m), `shotgun` (5 `shotgun_shells`, 8
  pellets of 16-24, 28 m, little past 20, noise 160 m). Add a gun by adding a weapon, its item and its rounds.
- **A shot is the attack:** `Intent::Attack` with a gun in hand pulls the trigger (`ActionKind::Swing`, so everything
  that times, queues, interrupts and replicates a swing holds); when it lands `combat::land` hands over to
  `World::fire`: one round off the gun, then each pellet flies from the shooter, 1.4 m up (`MUZZLE_HEIGHT`), along
  its **facing and pitch** (`Player::pitch`, `World::set_pitch`, `Intent::Aim { player, pitch }`: radians up from
  level, at most 1.3 either way; not saved, not in the hash, like `aim`), off by up to the spread either way sideways
  and up or down (the combat RNG; wider moving, and when tired or hurt). **The sim has no camera in it:** a
  first-person client would send its camera's pitch and nothing else changes.
- **What stops it** (`ranged::flight`, a walk over the tiles the line crosses): walls and shut doors; a window lets
  it through only between sill and head (0.9-2.1 m; a shut one's glass breaks, `World::break_edge`, with its noise),
  a doorway under 2.1 m, a fence stops what flies under 1 m, a tree trunk (0.3 m round), the ground, the edge of the
  map. Furniture and street props don't. **Bodies** are upright cylinders, 1.8 m tall standing and 0.4 m lying
  (`ranged::through`): the nearest the line passes through is hit at the height it is hit at, 1.5 m or more up a
  standing one being the head (`crit_multiplier`); a bullet goes on through `max_targets` of them
  (`multi_hit_falloff` less each) and its glass beyond where it stopped stays whole. A level shot passes over a
  zombie on the ground; one aimed over a head misses. A zombie is struck as by a blow (`World::strike`: damage,
  knockback, the stagger and knockdown rolls, it is after the shooter); all a shotgun's pellets fly together (one
  floored by the first is still standing for the rest). With PvP on other players are in the same list
  (`strike_player`): the wound is on the part at that height (`ranged::part_at`), a gash clothing doesn't stop, the
  damage times `CombatTuning::pvp_shot_mult` (0.7: three or four pistol shots to the body); spawn protection holds;
  a kill is "Shot by a survivor with a pistol: hit in the head". With PvP off bullets pass through other players.
- **Lag compensation (#23):** bodies are tested where the shooter's client showed them (`World::rewind_tick`, the same
  frames and cap as a blow); the map is as it is now. `net/tests/guns.rs` holds it over a socket at 150 ms.
- **Rounds and magazines are items.** How many rounds are in a gun or a magazine is on its stack
  (`ItemStack::used`, which for those two counts rounds, not goes had: no save format change; `ranged::holds` says
  what an item takes, `ranged::max_used` what the save checks hold it to). One made now is full
  (`World::fresh_stack`: debug spawns, kits), one found in a container holds a seeded few (`World::load_found`, by
  seed, container and stack, more often few than many). **Reloading** is the item job `JobKind::Reload { spare }`
  (saved with the queue; the last variant, so older saves read as before): `World::player_reload` swaps in the
  fullest magazine carried if it holds more than the gun's (`reload_seconds`; the one taken out keeps what was in
  it), else pushes loose rounds in one at a time (`round_seconds` each, the next following by itself, one note at
  the end) until the gun is full or they run out; `player_fill_magazine` fills a carried magazine the same way.
  Like eating it doesn't stop you walking, and shooting or swinging drops it (what went in stays in). There is no
  intent of its own: **`Intent::Use` on the gun in hand reloads it, on a carried magazine fills it**
  (`World::use_reloads`), and the trigger pulled on an empty gun reloads instead of shooting
  (`combat::request_action`; the refusal says what's missing).
- **Noise:** a shot is a player's sound of the weapon's `noise_radius`, heard that tick like a swing: every zombie in
  earshot comes (less through walls, as ever), sleepers within 20 m wake, other players who don't see the shooter
  hear it (#66), and being far over the migration's `min_radius` it leaves an attractor that draws the far horde in
  over the following minutes (`a_gunshot_wakes_the_street_and_draws_the_horde`).
- **Loot:** guns, magazines and rounds are `rare` entries in `loot.ron` (wardrobes, desk drawers, coat cupboards,
  sheds, garages, hardware shelves); none lies on a floor (`spawn_weight` 0). A town of four templates holds a
  handful.
- **Replication:** each pellet's flight is a combat event, `CombatEventKind::Shot` (`player` the shooter, `pos` where
  it stopped, `dir` and `damage` the way and how far it flew over the ground), sent like the others (so with fair
  information only when the shooter is seen; the bang is heard either way). The rounds in a client's own gun are in
  its private view (the hand's stack).
- **Client:** aiming (right mouse) with a gun in hand, the survivor holds it out level (`survivor::Style::Gun`,
  with a kick after the shot) and the aim is **the body the cursor is on, at the height it is on it**
  (`player::gun_aim`: the cursor's ray against each body's upright middle line, so the cursor on a head is a head
  shot and on a body on the ground a low one; on nobody, level at chest height toward the ground under the cursor);
  the pitch goes out as `Intent::Aim` each tick it isn't level. Left mouse shoots, R reloads (`Intent::Use` of the
  hand). The weapon panel says `Pistol  9/12  +12  +24` (in the gun, each magazine carried, loose rounds); the
  current-action bar says Reloading; the inventory says what is in a gun or magazine and offers "Fill with rounds"
  on a magazine. Each `Shot` is a bright streak along its line and a flash at the muzzle for a few frames (`fx.rs`:
  the blood's cube mesh in one emissive material, one instanced batch, at most 96 alive, nothing while nobody
  shoots) and a little camera shake for our own; the bang is the `Gunshot` cue, a reload step the `Reload` cue
  (**placeholders made of sounds there already are**, in `sounds.ron`; real ones with #78). `--autoplay` with a gun
  stands off and shoots the nearest it has a clear line to (`ranged::flight` on the client's map), reloading when
  empty. **`--gun ID`** starts a new world with that gun in hand, loaded, a spare magazine if it takes one and three
  loads of loose rounds; with `--fight-demo` there is no bat and the zombies start 16 m off:
  `--fight-demo --gun pistol --autoplay --zombies 0`.
- **MCP:** `attack` with a gun fires a round: `pitch` (radians), or with `target` / `target_player` it aims at
  `aim_height` on it (default 1.15 m, 0.2 for one on the ground); the reply says the rounds left; an empty gun is a
  tool error saying to reload. `use {from: "hand"}` reloads (and says beforehand what is in the gun and carried),
  `use` on a carried magazine fills it. `weapons` lists the guns in a table of their own; `inventory` shows the
  rounds in every gun and magazine. No new tool.
- **Not yet:** a skill for shooting, bows, suppressors, bullets through doors or furniture as cover, wounds of their
  own for bullets (a gash for now), zombies drawn by the light of a muzzle flash, models for the guns (grey-box bars).
- Tests: `sim::ranged::tests`, `net/tests/guns.rs`, `mcp::tests::agents_shoot_and_reload`, `player.rs`'s
  `a_gun_aims_at_what_the_cursor_is_on`, `survivor.rs`'s `a_gun_is_levelled_and_kicks`.

## Health and wounds (`sim::health`; #9)

- **One HP pool, wounds on ten parts** (`health::Part`: head, torso, left/right arm, hand, leg, foot; clothing still
  covers the six coarse `items::BodyPart` regions, `Part::region`). `Player::health` (`Health`: wounds, the infection,
  food poisoning, `rise_in`, first aid's `aid`) is saved with the player (format 5) and in `state_hash`. Every HP loss goes through
  `World::hurt_player`, so god mode holds: strikes and glass on a god still land as events but leave no wound or HP loss
  (the debug `inflict_wound` adds the wound, not the loss).
- **Wound kinds** (`WoundKind`, numbers in `HealthTuning`, `WorldConfig::health`, ours; hours are game hours, 150 s each
  by default): bruise 4–6 HP at once, no bleeding, heals in 4 h; scratch 6–9, bleeds 3/h for 1 h, 10 h; gash 9–13, 10/h,
  40 h; bite 12–18, 14/h, 60 h; glass cut 5–8, 8/h, 18 h (each wound rolls ±25% of its heal time). Gashes, bites and glass
  cuts bleed until dressed (#10), so untreated they kill in hours.
- **Zombie strikes** (`World::roll_zombie_wound`, combat RNG): a part by weight (`front` / `behind` tables: upper body
  likeliest, more head and torso from behind; a zombie on the ground hits legs and feet only), then scratch 50 / gash 30 /
  bite 20 (+15 bite when two or more are on you or from behind), then the worn clothing on that region
  (`World::protection`: layers multiply what gets through) may stop it to a bruise (protection in full against scratches,
  x0.7 gashes, x0.5 bites). **Glass:** smashing a window bare-handed cuts the right hand 60% (gloves 10%); starting to
  climb through a broken window cuts a hand, arm or leg 15% (clothing protects).
- **Effects while open** (`Health::effects`; scratches count half, bruises nothing): arm/hand -12% swing power and +8%
  swing time each (caps 40% / 25%), applied when a swing starts; leg/foot -8% walk and -12% run each (cap 30%), a gash or
  bite on a leg means no running; torso -20% endurance recovery each (cap 50%); pain per wound, easing as it heals, x2 on the
  head (`Health::pain`, for #11). Movement effects go into `MoveMods` (`speed`, the new `run_speed`, `can_run`, `regen`)
  through `World::refresh_carry` whenever wounds change, so the snapshot carries them and prediction stays exact.
- **Over time** (`World::step_health`, every tick; `skip_time` runs it too in 0.1 h steps): bleeding and festering drain
  HP; HP comes back 2/h with nothing bleeding or festering and no fever; wounds heal (a third as fast festering, twice
  stitched, not at all feverish). **Festering:** a seeded `hash01` roll per wound per game hour (undressed 4%, dirty
  dressing 6%, clean 0.5%, halved if cleaned): +0.1 pain, -1 HP/h, slow healing. **Food poisoning** is a hook for #11:
  `World::poison_player(id, chance)`; `Health::can_eat`, `thirst_mult`, endurance x0.7.
- **The infection** ("the turn", `InfectionTuning`, `WorldConfig::infection`): each wound a zombie leaves rolls once (bite
  60%, gash 8%, scratch 0; `mode: Off | Default | Custom { bite, gash, scratch }`). Hidden 6–12 h (not in any view, not to
  the player, the HUD or an agent), Feverish 12–24 h (sick, -1 HP/h, endurance x0.6, no healing), then Turning (-6 HP/h,
  the client halves night sight) until death; the body rises as a zombie 30 s later where it fell (at once if the player
  respawns first). `fatal: false` makes it burn out after the fever, leaving 30 HP; `stage_mult` 0.5–3. Fatal by default.
- **Treatment state** (#10) is `Wound::dressing` (`None | Clean | Dirty`), `plaster`, `clean_for_h`,
  `dressing_age_h`, `cleaned` and `stitched`, and `Health::aid` (`Aid`): painkiller doses, treatments' stings, the
  antibiotics course and the uses gone from each opened medical item.
- **First aid** (`sim::firstaid`, `FirstAidTuning` in `HealthTuning::first_aid`, ours; #10): `Intent::Treat { player,
  wound, action, item }` (`Treatment`: `Dress`, `Clean`, `Stitch`, `Painkiller`, `Antibiotic`, `Undress`; `wound` is the
  wound's id, `None` for pills; `item` an item index, `None` the best carried, `firstaid::best_item`) queues a
  `JobKind::Treat` (`World::player_treat`): checked when asked and when due by one pure rule over the client's view
  (`firstaid::check`, `options`, `needs`), so the health panel offers only what the server does. Times: dress 4 s
  (plaster 2 s, a hand x1.5, a dressed wound +1 s to change it), clean 3 s, stitch 8 s, a pill or taking a dressing off
  1 s. The player stands still while its first job is a treatment: `MoveMods` speed 0, switched only in
  `step_item_jobs` after movement, so snapshots carry what the next tick moves with and prediction stays exact. A strike
  landing (`wound_player`) drops queued treatments; swinging, shoving and stomping drop the whole queue.
  Items by tag: `plaster`/`bandage` dress (`clean` ones and plasters go on clean, a rag dirty; a plaster only on a bruise
  or scratch), `disinfects` cleans (undressed wounds; halves festering until that wound's next dressing comes off; +0.1
  pain for 1 h), `stitches` stitches a gash (no bleeding, heals x2; +0.2 pain for 2 h; a hand's dressing must come off
  first), `painkiller` (-0.4 pain for 4 h; two doses at once: `Health::drowsy`, the hook for #11), `antibiotic` (a
  course of 2, the next dose counting from 12 h and lapsing after 36 h; the last cures every festering wound; each dose
  keeps festering from starting for 24 h; never the zombie infection). A dressing stops a scratch, glass cut or bite
  bleeding for good (`bleeding` false); a dressed gash bleeds 25% until stitched, 100% again undressed. A clean dressing
  turns dirty after 12 h (6 h if the wound bled when it went on, a plaster 24 h). Dressing a dressed wound changes it;
  taking a dressing off gives a dirty `rag` back (not a plaster). Medical uses (`MedicalDef::uses`) are counted per
  player and item kind (`Aid::opened`, the opened one); the last use takes the item. Per-stack uses (that follow a
  dropped box) can come with the food's uses (#11). Ripping clothes into rags is a recipe (see Crafting), and so is
  washing rags into clean rags (a clean dressing).
- **Replication:** `PlayerView::health` is the coarse `HealthTags` everyone sees (bleeding, limping, injured arm, sick);
  `PrivateView::health` (`HealthView`: wounds by part with their id, bleeding, healed in 5% steps, hours to go, dressing
  and hours until a clean one turns dirty, cleaned, stitched, festering; HP lost per hour, pain, the infection once it
  shows; `aid`: painkiller hours, drowsy, the course, opened items) only to the player's own client, sent when it
  changes. `PlayerHurt` combat events carry `part` and `wound`. Saves check every wound, illness and first-aid value
  (`save_check::check_health`; opened items and treatment jobs are remapped like other item indices) and the tunings
  (`HealthTuning::check` with `FirstAidTuning::check`, `InfectionTuning::check`).
- **Client** (`health_ui.rs`): H opens the health panel (wounds by part, head to feet, colour for bleeding and festering,
  the dressing state); click a wound for what it needs and the treatments you can do with what you carry (with times and
  uses left), click one to do it; pills at the foot. Right-click a dressing, antiseptic or sewing kit in the inventory to
  use it on a wound (the panel opens with the wounds it fits marked; right-click cancels); right-click a pill to take it.
  You can aim and swing with the panel open, just not with the pointer on it (or on any panel). A tags line in the vitals panel for what the status tags don't say
  (INJURED LEG / ARM, DROWSY (PAINKILLERS), and for 3 s where the last strike landed); bleeding, the fever, festering and
  food poisoning are status tags (see Needs).
- **MCP:** `health {player}` (as the player's own view, plus wound ids (`#n`), dressings, effects, painkillers and
  antibiotics, what each wound needs next and what it can treat with what it carries), `treat {player, action, wound |
  part, item}` (action follows from the item if left out; the wound from `part`, else the most urgent it fits; steps
  until done and replies with the health), `wound {player, part, kind, infected}` (debug), `stats` per player (wounds,
  bleeding, HP lost per hour, pain, sick, infection or "unknown"), and `step_ticks` / `move_player` name the wounds
  strikes left.

## Run stats and the cause of death (`sim::run`; #13)

- `Player::run` (`RunStats`): `secs` played (ticks; skipped time not counted), `hours` alive in game time (skipped time
  included, counted in `step_health`), `kills`, `distance_m`, `items_taken` (from containers and the floor, each item of a
  stack), `searched` (containers this player searched first), `wounds`, `hp_lost`, and `death: Option<Death>` (`cause`,
  `tick`, game `day` and `hour`, `rises`: the infection will raise the body). Counted where things happen, one line each
  (`combat::strike` kills, `World::step` movement, `step_item_jobs` / `player_pick_up`, `add_wound`, `hurt_player_by`).
  A respawn or a new world starts a fresh record. Not in `state_hash` (nothing reads it back) and `#[serde(skip)]` in
  `Player`, saved in its own `RUNS` section instead (see Saves).
- `RunStats::player_kills` counts other players killed (PvP), apart from `kills`; `DeathCause::Survivor` names the
  killer's weapon through `describe_with(weapons)` / `RunStats::cause_of_death_with` (the client and MCP use those).
- `DeathCause`: `Zombie { part, wound }`, `BledOut { part, wound }` (the wound bleeding most), `Festering`,
  `Infection { wound }` (the fever or the turn; "Bitten and turned"), `Glass { part }`, `Wound` (debug), `Other`, `Survivor { by, weapon, stomp, part, wound }` (another player's swing or stomp, PvP; see Multiplayer).
  `World::hurt_player_by(pi, damage, cause)` records it; plain `hurt_player` says `Other`; `step_health`'s drain picks
  the biggest drain (`run::health_cause`: the turn always wins). **#11 adds** `Starved` / `Dehydrated`: a variant, its words
  in `DeathCause::describe`, and `hurt_player_by` where it hurts; the client and MCP only show `describe`'s text.
- Replication: `Snapshot::runs` (`RunView { player, run }`) every snapshot, a client only its own (`for_player`,
  `Snapshot::run_of`). The HUD's kills / alive / walked line and the death screen read it.

## The first night (`sim::firstnight`, `firstnight_ui.rs`; #19)

The first playable loop: wake up in a bedroom, search the house, stay or go, live until dawn.

- **The record:** every life has a `FirstNight` on the server (`Player::first_night`, begun by `add_player` and a
  respawn; `None` in a save from before it): `began`, `dusk` and `dawn` on the world's clock (`World::clock_hours`:
  start hour plus game hours, never wrapping, so skipped and slept-through time count; a life that begins at night is
  already in its first night), the sticky objective flags, where the night's hours went (home = the house you woke
  in, another building, outdoors; and asleep) and, at the first dawn if alive, an `Outcome` (where most of the night
  was spent, hours slept, kills, wounds and HP lost between nightfall and dawn, HP at dawn, objectives done) with a
  `headline` and a `verdict` in plain words. `World::step_first_night` keeps it after needs each tick and after a
  `skip_time`. `phase()`: `Day`, `Evening` (the last 2 h of light), `Night`, `Morning` (there is an outcome).
- **Objectives** (`Objective::ALL`, guidance only, nothing is refused): search 3 containers (`run.searched`), hold a
  weapon, carry food and a drink (by item category), be indoors at some point after dark, live until dawn.
  `FirstNight::done` / `label` (with what's missing) / `done_count`.
- **Derived:** not in `state_hash`, `#[serde(skip)]` in `Player`, saved in the optional `NITE` section (only players
  with a record; checked on load by `FirstNight::check`), so the format didn't change and older saves load without one.
  Goes out in `RunView::first_night` (`Snapshot::first_night_of`; a client only its own) and MCP `stats`
  (`first_night`: phase, `dark_at`, `dawn_at`, hours to each, the objectives with `done`, the `outcome`).
- **Pacing:** the client starts a new world at 14:00 without `--time` (`server_link::WAKE_HOUR`: six game hours, 15
  minutes, of daylight; the sim's default stays 9:00 for the server and tests). **After dark you can sleep however
  rested you are, and being rested doesn't wake you until sunrise** (`NeedsTuning::sleep_through_night`, default on),
  so a night in bed is about 50 s at x30 unless a noise, a strike, being seen, pain, hunger or thirst wakes you.
- **Client** (`firstnight_ui.rs`): an objectives panel bottom right (hour, time to dark or dawn, the five objectives
  ticked off); a hint line bottom centre, one sentence on what to do now and its key (`firstnight_ui::hint`, a pure
  function: asleep, a zombie within 5 m, bleeding, thirst and hunger first, then waking, searching (E), taking (Tab),
  arming, packing food and drink, the evening's stay or go, the night indoors or out); and at the first dawn the
  **morning screen** (DAWN: where the night was spent, what it cost, Enter carries on, R starts a new run; the world
  doesn't stop behind it; it comes up only for a dawn of this session, not for a loaded save's old one). O hides the
  panel and hints for the session. None of it in `--fight-demo` / `--siege`, once dead, or after the morning screen.
  The death screen adds how far short of the first dawn a life ended and its objectives done. For pictures:
  `--skip-hours H` skips game time before the first frame of a new world, `--morning` starts with the morning screen
  up when there is an outcome (`--render-to shot.png --hud --time 21 --skip-hours 9.5 --morning`).

## Needs (`sim::needs`; #11)

- **Hunger, thirst, tiredness** (`Player::needs`, 0 fine to 1 worst) rise per game hour (`NeedsTuning`,
  `WorldConfig::needs`, serde defaults, saved with the settings; ours): hunger +0.04 (asleep +0.02), thirst +0.06 (+0.03;
  x2 with food poisoning; +0.02/h running, +0.002 a swing), tiredness +0.05 awake, -0.12 / -0.09 / -0.06 asleep on a bed /
  sofa / the floor. Levels at 0.35 / 0.6 / 0.85: Peckish, Hungry (no HP back, endurance recovery x0.85), Starving (-2 HP/h,
  swing power x0.85); Dry, Thirsty (recovery x0.75), Parched (-3 HP/h, no running); Drowsy, Tired (swing time x1.1, the
  client's view cone x0.85), Exhausted (swing time x1.25, walking x0.9). Pain (wounds plus a night on the floor) at 0.2 /
  0.45 / 0.75 weakens swings x0.95 / x0.85 / x0.7. Movement effects go into `MoveMods` (`World::needs_mods`, from
  `refresh_carry` whenever a level or sleep changes); HP loss goes through `hurt_player` (god mode holds). `World::step`
  runs `step_needs` after `step_health`; `skip_time` steps both in 0.1 h steps. `NeedsTuning::off()`: needs never rise
  (other systems' long-skip tests use it).
- **Eating and drinking** is the `Use` job: `2 + 8 x (|hunger| + |thirst|)` s, 2 to 5; one go off the top item
  (`ItemStack::used`; the item goes after its `uses`); the item's amounts come off (negative ones add). Food with hunger
  under 0.1 does nothing for hunger; food poisoning refuses food (`Health::can_eat`).
- **Spoiling:** `ItemStack::born` is the game minute it was made (`World::game_minute`, from `game_hours()`: ticks plus
  skips, never wrapping; 0 for things that keep). Fresh until `spoils_hours`, Stale to twice that (x0.7 of what it gives,
  `HealthTuning::poison_stale` 10%), then Mouldy (x0.4, `poison_rotten` 70%), through `World::poison_player` (combat RNG).
  Food found in furniture (`World::age_found_food` on first search) was made 0-60% of its `spoils_hours` before the world
  began (seeded per container and stack). No per-tick aging; fridges don't slow it yet (no power).
- **Sleep** (`World::player_sleep`, `Intent::Sleep`, B, MCP `sleep`): needs tiredness 0.3+ (after dark any, and then being rested doesn't wake you before sunrise: `sleep_through_night`, #19) and no agony, unless a painkiller dose is
  working (#57, `World::pain_stops_sleep`: you sleep through it, and wake in pain when the last dose wears off if it's
  still agony; the Pain tag's tip says which); on a bed (else a
  sofa) within 1 m of its footprint in your room, else the floor (wakes you sore: +0.1 pain for 2 h). Asleep: `MoveMods`
  speed 0, HP back at `regen_asleep_per_h` (5/h). Wakes (`World::woke`, per tick, `WakeReason`) when tiredness < 0.05, a
  zombie's strike lands (god too), a sound reaches you within 20 m (`heard`, not your own), an active zombie sees you, in
  agony, after an hour starving or parched, or on getting up (B, WASD, or any action or item intent). **Time:** while every
  living player sleeps (`World::time_scale`), game time runs `sleep_time_scale` (30) x as fast: each tick adds 29 extra
  ticks' hours to the clock offset and steps health and needs by 30 ticks' worth. **Zombies while you sleep (#57):** active and
  near zombies (within 150 m: the ones that can reach you) keep real time, so a strike, a noise or being seen still wakes
  you with time to act; the far horde follows game time: the far clock (`World::far_tick`, `tick` plus the saved
  `far_ticks`) takes 30 steps a tick, and the virtual tier's turns, wandering and migration, the attractors' spread and
  fading, and respawn's timers all go by it (see Simulation tiers). So after a night's sleep a horde drawn by an alarm
  across town has arrived, and far regions have refilled. Single-player
  that's whenever you sleep; on a server, only once everyone is asleep (a lone sleeper rests in real time). Snapshots carry
  `time_scale` and `game_minute`; `PlayerView::asleep` (the survivor lies on its back, drawn on the bed or sofa it
  sleeps on: `needs::sleep_furniture`, the server's rule too; the HUD's stance says "asleep").
- **Status icons** (Linear DCM-57; `sim::status`, `assets/data/status.ron`; they replace the status tags and the HUD's
  health and endurance bars): one icon per status, shown only once its value passes a level, each with up to four
  levels (severity 1-4; 0 for a dimmer good state), a label and a one-line tip. **Which icons there are, their order,
  levels, labels and tips are data** (`status::StatusData`, checked by `parse_status`: unknown fields and kinds, 1-4
  levels each starting higher than the last, severities 1-4 never falling, a good state ending before the first level;
  errors name the icon); the code only says what each kind's value is (`status::value`): Health (the share of HP
  lost: Hurt / Wounded / Badly wounded / Near death), Bleeding (HP lost to it per game hour: a little / Bleeding /
  badly / out), Pain (Sore / Hurting / In real pain / Agony), Sick (festering Sick / Feverish / Turning), Queasy (food
  poisoning), Hunger (Peckish / Hungry / Famished / Starving; good: Well fed), Thirst (Dry / Thirsty / Very thirsty /
  Parched), Tiredness (Drowsy / Tired / Worn out / Exhausted; good: Rested), Endurance (the share spent, plus 1 while
  winded: Short of breath / Winded), Load (weight over the limit, passed when over a level as the burden is: Burdened /
  Heavy / Overloaded / Crushed), Asleep (x30). The file's levels sit on the settings' thresholds (a test holds them
  together), with one more display level between the last two. **Display only:** nothing in the sim reads a tag, the
  file is in no save or fingerprint. `status_tags(data, needs_tuning, &StatusInput)` is the one function: the client
  builds the input from its views (`PrivateView::status_tags`, with its HP and predicted endurance) and its data
  folder's file (`net::content::load_status`; none: the shipped one), `World::status_tags` (the MCP tools) from the
  world and the shipped file compiled in (`StatusData::builtin`). Client (`needs_ui.rs`): glyphs in a row, each on a dark dab of paint (`theme::dab`),
  over the weapon panel (eight to a row; the hotbar has the bottom edge beside it, #97), the picture drawn in code (`needs_ui::glyph`: a cross, a drop, a bolt, a thermometer, a
  swirl, a knife and fork, a glass, a moon, a heartbeat, a weight, a Z; white on a 64 px image, tinted), coloured by
  severity (Linear DCM-61: simple white or yellow glyphs: a faint white good, white, pale yellow, strong yellow, and
  only the worst red, on a dark red dab; sleep pale blue) with as
  many pips; an icon that appears or worsens pulses (two rings over 1.2 s; not the ones a world starts with); hover one
  for its line, shown just over the icons. Also the stats line, stale/mouldy and goes left in the
  inventory panel, a narrower cone when tired and a closed-eyes fog asleep (`fog.rs`). `review_saves.rs`'s
  `status_icon_saves` writes `icons_0` to `icons_4` (every status at that severity), `icons_cleared` and `icons_good`.
- **The debug stats window** (Linear DCM-57, `debug_stats.rs`; F4, or `--debug-stats` to start with it open, e.g.
  `--render-to shot.png --hud --debug-stats`; off by default): the raw numbers of our own survivor: HP and what it
  loses per hour, pain, every body part's wounds (bleeding, healed, dressing, festering), hunger, thirst, tiredness,
  endurance, the infection, food poisoning, temperature (not simulated yet, and it says so), the load and what it all
  does to movement, every skill's level and XP, the status icons with their severity, and the last 8 things that cost
  HP with their source (a zombie's or a survivor's strike with the wound and part, from our combat events; slow losses
  added up and put down to bleeding, the infection, a festering wound, thirst or hunger). Client-side only. In
  single-player it also reads our player in the world in this process (needs unrounded, an infection that doesn't show
  yet). On a multiplayer server it opens only when the server was started with `--allow-dev` (`Rules::allow_dev`;
  otherwise F4 says so) and shows our private view as sent: never another player's state, nor a hidden infection.
- **Saves:** needs, sleep, soreness and each stack's `born`/`used` (format 6; 7 with crafting), in `state_hash`; checked on load
  (`save_check::check_needs`, `check_stack_state`: needs 0..1, goes below `uses`, made no later than now) and
  `NeedsTuning::check`. **MCP:** `stats` (needs, asleep, tags, `time_scale`), `health` (a needs line and each tag's tip),
  `observe` (tags per player), `use` (what it will do first), `sleep {player, max_hours}` (steps until awake; why, hours,
  needs and HP before and after), `set_needs` (debug).

## Skills (`sim::skills`; #46)

- **Nine skills, levels 0-5, raised only by practice.** `Player::skills` (`Skills { xp: [f32; 9] }`, by `Skill as usize`)
  holds XP; the level is worked out from it at the cumulative thresholds `SkillTuning::levels` (100, 250, 500, 900, 1,500;
  XP stops at the top). A new survivor (`add_player`, a respawn, a new run) has Fitness and Strength at 1 (100 XP), the
  rest 0 (`Skills::start`). Nothing is lost when a skill isn't used; no skill books. Every number is in `SkillTuning`
  (`WorldConfig::skills`, serde defaults, saved with the settings; ours), except Strength's carry kg
  (`CarryTuning::strength_kg`) and First Aid's speed and stitching (`FirstAidTuning::skill_faster_per_level`,
  `skill_stitch_level`), which stay where those numbers are used.

| Skill | XP from | Each level |
|---|---|---|
| Fitness | running: 0.1 a second | running drains endurance 6% slower, it recovers 6% faster (`MoveMods`) |
| Strength | a swing of a weapon over 1.5 kg (by its item's weight): 1; moving over the carry limit, awake: 1 a game minute | carry limit +1.5 kg, shove knockdown chance +0.03, knockback of swings and shoves +5% |
| Bludgeon / Edged | each zombie hit with a weapon of that skill: 2, a kill 5 instead | damage +6%, endurance cost -5%, windup and swing time -3% |
| Brawl | each punch that lands 2 (a kill 5), each zombie shoved 2, a stomp that kills 5 | punch and stomp damage +10%, shove knockdown chance +0.04 |
| Stealth | sneaking and moving within 15 m of a living zombie that isn't after you (not while ignored): 0.2 a second | sneaking footsteps 8% quieter; every sight range against you 6% shorter (`senses::Target::sight_mult`) |
| First Aid | each treatment done: 10 | treatment 10% faster; from 3, stitching stings half as much |
| Scavenging | each container searched for the first time: 2 | first searches 10% faster and 10% quieter |
| Handiwork | each craft (#12): 5 | crafting 10% faster (`SkillTuning::craft_time_mult`) |

- **Weapon skills follow how a weapon hits:** each entry in `weapons.ron` names its `skill` (`Some(Bludgeon)`, `Some(Edged)`
  or `Some(Brawl)`; left out, `WeaponCategory::skill`: `ShortBlunt`/`LongBlunt` Bludgeon, `ShortBlade`/`Axe` Edged,
  `Unarmed` Brawl). `parse_weapons` refuses a non-weapon skill, bare hands that aren't Brawl, and two weapons of one
  category naming different skills. `WeaponDef::skill()`.
- **Where effects apply:** Fitness in `SkillTuning::move_mods`, inside `World::refresh_carry` (load, then skills, then
  wounds, then needs); Strength in `World::carry` (the limit) and `combat::land` (knockback, shove knockdown); the weapon
  skills in `combat::begin_action` (cost, time) and `land` (damage); Stealth in `World::step` (footsteps, the sight target);
  First Aid in `firstaid` (`first_aid_level`); Scavenging in `player_search` and `do_search`. XP is awarded where the
  activity happens (`World::award_xp(pi, skill, xp)`, one line each), and the per-tick ones (running, overload,
  sneaking) in `World::step_skills` after the senses.
- **Prediction stays exact:** a Fitness or Strength level-up never changes `MoveMods` at once; the player goes on
  `World::skill_mods_due` and `refresh_skill_mods` refreshes it at the end of the tick, after movement (even for a heavy
  swing asked for by an intent before that tick's movement), so the snapshot carries what the next tick moves with
  (`net`'s `prediction_matches_server_with_skills`).
- **Level-ups:** `World::level_ups` (`LevelUp { player, skill, level }`, kept until drained, at most 64; not saved, not in
  the hash) go out in `Snapshot::level_ups` (a client gets only its own); the client shows `LevelUp::text` ("Edged 2") as
  the yellow line under the HUD and prints it (`skills_ui.rs`). `PrivateView::skills` (`SkillView`: level, whole XP,
  the next level's XP, progress) goes to the player's own client when a whole XP point or a level changes.
- **Client:** K opens the skills panel (`skills_ui.rs`): each skill's level, a bar to the next level with XP, what the
  level gives now and what the next gives (`SkillTuning::gives`), and how XP is earned (`earned_by`). The health panel's
  treatment times use our First Aid level.
- **Crafting (#12):** `crafting`'s `on_crafted` gives `SkillTuning::craft_xp` Handiwork per craft done
  (`World::award_xp(pi, skill, xp)`), and a craft takes its recipe's time times `SkillTuning::craft_time_mult(level)`
  (`World::craft_secs`; never below `crafting::HANDIWORK_MIN_MULT`, half). `World::skill_level(pi, skill)` and
  `award_xp` are by player index, as the item jobs are.
- **Saves:** XP per skill with the player (format 8), in `state_hash`; checked on load (`save_check::check_skills`: finite,
  0 to the top level's XP) and `SkillTuning::check`. **MCP:** `skills {player}` (each skill's level, XP and to go, what it
  gives now and at the next level, how it's earned; read-only), `stats` (each player's `skills` levels by id),
  `step_ticks` / `move_player` / `attack` report `LEVEL UP: P1 Edged 2`, and `weapons` shows each weapon's skill.

## Items, inventory and containers (`sim::items`, `sim::inventory`, `sim::containers`; #7)

- **Items are data:** `assets/data/items/items.ron`, a list of `items::ItemDef` (id, name, `category`: Food, Drink,
  Medical, Tool, Material, Clothing, Bag, Weapon, Light, Junk; `weight_kg`, `volume_l`, `stack`, `tags`,
  `two_handed`, `one_hand`, and per category `food`/`drink` (hunger, thirst, uses, spoils_hours), `medical` (uses), `bag`
  (capacity_l, carried Back/Shoulder/Hand, slow), `clothing` (covers body parts, takes accessory places, protection,
  layer, hood), `light`, `weapon` (an id in `weapons.ron`), `model` (the wearable the client draws, #63) and grey-box
  `color`). About 70 ordinary things with real-world weights; validated by
  `items::parse_items` (unknown fields refused), loaded by `net::content::load_items` (all content at once:
  `content::load_config`). A weapon with no item gets a plain 1 kg one (`items::complete_items`), so bare configs and
  tests work. Things are `ItemStack { item, count, born, used }` (index into `WorldConfig::items`; `born`, the game minute
  food was made, and `used`, goes had of the top one, see Needs; stacks merge only when all but the count agree:
  `ItemStack::same_kind`, `merge_into(stacks, like, n, size)`).
- **Accessories** (#63) are clothing that `takes` a place (`items::WearSlot`: `Face`, `HeadTop`, `Neck`): glasses (the
  face), a baseball cap, beanie or woolly hat (the head top; they cover the head too), a scarf (the neck), and masks (#92:
  a dust mask, a bandana, the face on the Mid layer, so over glasses and one at a time; a balaclava, the face and the head
  top, under any hat, which the client then draws 6% bigger about the head's centre so it sits over the knit). A
  `hood` (the hoodie's) covers the head while nothing takes the head top: putting a hat on pulls it down
  (`ClothingDef::covers_with`, used by `World::protection`; the note says so). Each item can name its `model`, the
  wearable GLB the client draws (`assets/models/wearables/<model>.glb`); none, or no file: a grey-box (see Characters
  on screen). A new wearable is a model file plus an `items.ron` entry, no code.
- **Inventory** (`Player::inventory`): `hand` (swings), `off_hand`, `back` and `shoulder` (bags), `worn` (clothing and
  accessories; two can't share a body part or an accessory place on one layer, `ClothingDef::clashes`), `carried` (pockets and bags). Two limits (`CarryTuning`, ours): *room*,
  pockets 3 L plus the bags in the slots (held and worn items take none); *weight*, everything in full (bags add room,
  not lightness). Limit 14 kg (+1.5 kg per Strength level, #46: `CarryTuning::limit(strength)`; a new survivor's Strength 1 makes it 15.5 kg); over 100%
  Burdened (x0.9 speed, running drains x1.3), over 130% Heavy (x0.75, no running, actions cost x1.2), over 160%
  Overloaded (x0.5, no running or climbing); nothing more picked up past 200%. `World::carry(p)` says where a player is.
- **Moving things takes time:** `MoveItem { player, from, index, to, count }` (`Place`: `Carried`, `Slot(Hand |
  OffHand | Back | Shoulder)`, `Worn`, `Container(id)`, `Ground` with the ground item id as `index`; count 0 = all),
  `Equip { index }` (a carried stack where it belongs), `Wear { index }`, `Unequip { from, index }` (into bags, or the
  floor if there's no room; a bag can't come off while its contents wouldn't fit), `Drop { from, index, count }`,
  `Use { from, index }` (eat or drink one go, 2-5 s by the item; see Needs; `World::used_items` lists them per tick) and
  `Search { container }` queue an `ItemJob` on the player (`Player::jobs`, at most 16). The first counts down and
  happens when due (0.3 s a stack, 0.6 s over 3 kg; use 1 s; a first search the container's search time, x1.5
  sneaking; reopening 0.3 s), checked again then: still there (by index, else the first stack of that item), in reach,
  open, fits, not too heavy; slots take one item and push what was in the way into the bags; moves are atomic.
  Swinging, shoving or stomping drops the queue. `PickUp` (E) is the one instant grab: the item nearest to where the
  player faces within `PICKUP_REACH` (1.3 m, clear line): a weapon into the hand (what was held into the bags if it
  fits, else left on the floor), anything else into the bags, as many as fit. Refusals and results become
  `World::item_notes` (to the snapshot).
- **Containers:** `assets/data/containers.ron` (`containers::ContainerData`): container kinds (room in litres,
  search time, noise) and a map from `(furniture kind, room type)`
  to a kind, `room: "any"` the fallback (a sink is a bathroom cabinet in a bathroom, an under-sink cupboard
  elsewhere; a wardrobe in a hall is a coat cupboard). Ground-floor furniture that maps to one is a `ContainerSite`
  (`World::containers()`), id = the furniture's place in the map (buildings in order, each one's furniture in order),
  opened from the same room within `CONTAINER_REACH` (1.6 m of the nearest edge of the furniture's footprint; E takes
  the one faced, within 60 degrees, nearest first, else the nearest in reach). New furniture kinds `shelf` (in the
  family house's laundry) and `workbench`. A container is filled the first time anyone searches it, from its loot
  table (see Loot; a hash of seed and id, never the world RNG, so the same in any order). Opened containers are stored sparsely (`World::opened`, saved, in
  `state_hash`); a searched container stays open (and usable) to that player (`Player::open`) until it is `CONTAINER_KEEP_OPEN`
  (2.2 m) away or leaves the room. The floor has unlimited room; ground items within `PICKUP_REACH` with no wall,
  shut window or door between (a hand reaches over a fence; furniture doesn't block it either: `vision::clear_reach`) are the player's floor
  (`World::floor_of`, the one rule for moves, `PickUp` and the panel); every snapshot carries each player's floor as
  ground item ids (`Snapshot::floor`, `FloorView`; a client gets its own). A request for what is already queued
  (a repeated click) queues nothing and refuses nothing; E with no container and an empty floor says "nothing to pick
  up or search here" (`inventory::NOTHING_HERE`).
- **Client:** the inventory panel (`inventory_ui.rs`, Tab or I; laid out in Linear DCM-56): two panes side by side, one
  title each. Left, what you carry; right, the open container, or the floor in reach. Slim tabs down each pane's outer
  edge pick what it shows: Carried or Worn (what you hold and wear, slot by slot) on the left, the open container or
  Floor (each with its count) on the right; a newly opened container comes to the front (`PanelUi`). Under each title a
  thin bar and one line: your load against the limit (coloured, with the burden stage) and litres used, the container's
  litres and weight, the floor's weight. Every row is one line of the same cells, so the columns line up: a swatch of the
  item's colour, name (then dimmed: stale, goes left, moving), type, count, kg. The headers Name, Type and kg sort both
  panes (`Sort`: a press, again the other way round, a third time as things lie). Several stacks of one item collapse into
  one row with their count and weight together (`arrange`): a click moves them all (`click_intents`, from the back so
  indices don't shift), the `+` at its start opens it to its stacks. `>>` and `<<` between the panes move everything a
  pane shows across (`MoveAll`; never what you hold or wear). Two text sizes in all (`ROW_FONT`, `HEAD_FONT`). Click a row to move it
  (yours into what the right pane shows: the container, or the floor; theirs into your bags; slots off), Alt-click (Option on a Mac) for one, right-click any
  row for its menu (#95, `inventory_ui::menu_entries`): one flat list of what can be done with it, the likeliest first (eat or
  drink, wear, equip, take a pill, use on a wound, put in the open container, drop, take, take in hand, split: one, or half of four or more),
  1-9 to pick, Esc or a click elsewhere to close; the same menu on a ground item in reach in the world (right-click it with
  the panel open, or a quick right tap with it shut; held, right mouse aims). `--menu carried:N|worn:N|hand|container:N|ground[:N]`
  starts with one open, for pictures, and `--menu-pick N[@SECS]` presses its number key N (after SECS), for recordings. A
  left click behind an open menu only closes it (it moves nothing), opening or shutting the panel closes it, and it opens
  at the pointer in UI pixels (the UI scale), kept on screen. The latest note shows at its foot, and under the HUD for a few seconds when
  it's shut (the same note again only keeps it up, and isn't printed again). One click is one request: row observers
  only record presses, `send_clicks` sends at most one per button per frame and none for a stack already on its way
  (`InFlight`, until the snapshot shows the answer, a refusal, or 3 s); a floor item being taken leaves the Floor
  column at once. The Floor column and E use the snapshot's floor, not a distance guess. Left mouse doesn't swing while it's open.
  **Current action** (#98, `action_ui.rs`): while a job is under way a bar at the bottom centre names it ("Wash rags in clean water"), shows progress and seconds left, the number queued, and X or a click cancels it, Shift+X the whole queue (`Intent::CancelJobs { player, all }`, `World::player_cancel_jobs`).
  **Finding and moving several** (#94): a search box (`/` or a click to type; Enter or Esc stops; while
  typing every other key is held back, `inventory_ui::TextEntry`, which key handlers in `PreUpdate` run after) and
  category chips (Food, Drink, Medical, Weapons, Clothing, Tools, Materials, Bags, Other; each a toggle, so several can
  be on, and All turns them off) narrow both panes but your hand and
  bag slots. They are out of sight until wanted: the small "Find" control at the top right, or `/`, brings them out, they
  stay while a search or chip is set (`Find::in_use`), and "Find" again puts them away, cleared. Each pane's title has a "pick all" for
  the stacks it shows (of yours, only what you carry, never what you hold or wear; pressed again, it unpicks them); Ctrl-click picks rows (a picked row always shows; a collapsed row: all its stacks), Shift-click every row from the last one picked to it in that pane
  (only the rows on show, so a range under a search skips what it hides),
  and the top row (always that high, so picking doesn't shift the rows, and in view however long the
  lists are) then says how many are picked and their weight and sends every picked stack to the bags, the open container or
  the floor at once. Names never wrap, and one too long for its cell is cut off at the next.
  `--find TEXT`, `--filter CATEGORY[,CATEGORY...]` and `--pick N,N` (carried stacks) start it that way, for pictures, as do
  `--tab worn|body|floor` (several with a comma; `body`: the Worn tab as the body view), `--sort name|type|weight` and `--expand` (collapsed rows opened);
  `--pick-range A,B[@SECS]` Ctrl-clicks carried row A and Shift-clicks row B (after SECS), and `--drop-picked-after SECS`
  presses the move-to row's "floor", for recordings (`review_saves.rs` writes `panel.zsave`: a pack, a container
  open and things on the floor, and `panel_full.zsave`: the pack full, with several stacks of one item, things held and more on the floor). Ground items are drawn as boxes of their size and colour,
  weapons as bars. Under the panes, the Make list (see Crafting), one line ("3 you can make now") until a click opens it.
  **The body view** (Linear DCM-43, `inventory_ui/body.rs`): the Worn tab's title has "List" (the default) and "Body";
  Body shows what you hold and wear on a simple outline of a person instead of as rows. A column of places either side
  of the figure (`body::Spot`: head, face, neck, torso, hands, legs, feet, and arms when something is listed there; the
  hand, off hand, back and shoulder slots), each listing what is at it, outermost first, or a dash. A worn item is
  listed once (`Spot::of`: the accessory place it takes, else the main part it covers: a hoodie the torso, boots the
  feet), and the figure's parts take the colour of what covers them (`body::tint`: a part's own garment before one that
  only reaches it, then the outermost; a hood up until a hat is on), with a bag behind the shoulders or at the hip and
  a bar in a hand that holds something. Each item is a tile with the list rows' own presses (`inventory_ui::clickable`,
  the same `RowAction`): click to take it off into the bags, Alt-, Ctrl- and Shift-click, the right-click menu and the
  hotbar's keys as on a row. The search and chips don't narrow it. Which view is kept in `settings.ron`
  (`ClientSettings::inventory_body`); `--tab body` starts with it (pictures: `review_saves.rs` writes `body`,
  `body_some` and `body_bare`). A survivor with three layers on is taller than a 1280 x 720 screen leaves room for
  while the objectives are up: the panel then scrolls, as its lists do.

## Crafting (`sim::crafting`, `assets/data/items/recipes.ron`; #12)

- **Recipes are data:** a list of `crafting::RecipeDef` (`id`, `name`, `secs`, `inputs` and `outputs` as
  `(item, count)`, `tools` as `Item("id")` or `Tag("tag")`: any item with that tag in `items.ron`, needed but kept),
  validated by `crafting::parse_recipes` (ids, a name, secs in (0, 600], counts 1..100, at least one input and output,
  nothing listed twice, a tool that is also an input, an output that is also an input (it would loop), a recipe id twice,
  unknown fields) and checked against the items by `crafting::check_recipes` (every input, output and tool item in
  `items.ron`, some item with each tool tag; errors name the recipe), loaded by `net::content::load_recipes` into
  `WorldConfig::recipes` (no file: no recipes). Add a recipe by adding an entry.
- **Shipped** (ours): rip a T-shirt (3 rags), a jumper (4), a hoodie (5), a towel (4), tear a bed sheet (6), cut jeans
  (4, a `cuts` tool: scissors or a kitchen knife), and wash 3 rags in a bottle of water into 3 clean rags (and an empty
  bottle). A rag dresses a wound dirty, a clean rag clean (first aid's `bandage` / `clean` tags).
- **Crafting is an item job** (`JobKind::Craft { recipe }`, `World::player_craft`, `Intent::Craft { player, recipe }`
  with an index into the recipes the client got on joining): checked when asked and again when due by one pure rule
  over an inventory (`crafting::check` / `check_times`, which the inventory panel uses too), taking the recipe's `secs`
  (`World::craft_secs`: Handiwork, #46, `SkillTuning::craft_time_mult`, 10% faster a level down to half; each craft done
  gives 5 Handiwork XP in `World::on_crafted`, see Skills). Inputs and tools are what the player
  carries and holds (`firstaid::usable`), not what it wears (the refusal says to take it off); only whole items count
  as inputs (`crafting::whole`: the part-used top one of a stack doesn't). Like eating, it doesn't stop you walking; a
  strike landing drops queued crafts with the treatments (`interrupt_treatment`), and swinging, shoving or stomping
  drops the whole queue; a sleeper's `Craft` only gets it up. Asking again for a recipe already queued queues another
  only if there's enough for both, else nothing (a repeated click). When done the inputs go and the outputs are made
  new (`World::fresh_stack`: food made now is fresh, `used` 0), into pockets and bags while there's room, the rest at
  the player's feet (`put_on_floor`), and the note says what was made from what.
- **Saves:** craft jobs are saved with the player's queue (format 7); `recipes.ron` is in the content fingerprint
  (`ContentPrint::recipes` / `recipe_ids`): any change loads with a note, queued crafts matched up by id
  (`save_check::remap_recipes`), and those of a recipe that's gone called off (nothing was used up yet). A craft's
  recipe index out of range is `LoadError::Invalid`.
- **Client:** the inventory panel's Make list (`inventory_ui.rs`; shut to one line until clicked): every recipe; the ones you can make (by
  `crafting::check` over your private view) are rows to click (one request until the answer, like moves), with what
  they make and their time; the one under way shows its seconds left, queued ones how many; the rest say what's
  missing. No key of its own.
- **MCP:** `recipes` (the table; read-only), `craft {player, recipe}` (no recipe: what the player can make now, and what
  the rest need; with a recipe id: queues it, steps until done, and says what was made and what changed, carried and on
  the floor).

## Loot (`sim::loot`, `assets/data/loot.ron`; #8)

- **Tables by room and container:** `loot::LootData` (`scale`, `rare_scale`, `tables`), validated by `loot::parse_loot`
  (numbers, empty tables, a key twice, an item listed twice, unknown fields) and checked by `LootData::check` (item ids
  in `items.ron`, container kinds in `containers.ron`; errors name the table, e.g. `(kitchen, fridge)`), loaded by
  `net::content::load_loot` into `WorldConfig::loot` (no file: no tables, every container holds nothing). A
  `LootTable` is `room` (a room type or `any`), `container` (a kind or `any`), `empty` (chance of nothing), `rolls`
  (min, max picks), `entries` (`item`, `weight`, `count` per pick, `rare`) and `groups` (extra lists rolled after
  the picks with their own `chance` and `rolls`: the kitchen's "cutlery drawer", the laundry's "toolbox"). A container
  uses the first of `(room, container)`, `(any, container)`, `(room, any)`, `(any, any)` (`LootData::find`): a fridge
  is a fridge anywhere, and shelves and workbenches (no `any` table of their own) hold what their room holds.
- **Filling** (`loot::fill(seed, id, def, room_type, loot, items)`, called on a container's first search): from a
  stream seeded by the world seed and container id only (`loot::container_rng`), so the same in any opening order and
  across a save; picks add up into stacks of the item's stack size; a pick that doesn't fit the container's litres
  is cut to what fits. **Sandbox:** `scale` multiplies every table's and group's rolls (a fraction rounds by chance;
  0.25 scarce, 2 plenty), `rare_scale` the weight of `rare` entries.
- **Tables shipped** (ours): kitchen (fridge, cupboard with the cutlery-drawer group, under-sink, pantry shelves),
  any-room fridge, cupboard, oven, under-sink, bathroom cabinet, wardrobe, coat cupboard, desk drawers, coffee-table
  drawer, TV shelf, drum; room tables for bathroom, bedroom, living, dining, hall, laundry (with a toolbox group),
  shed, garage, office (and its desk), shop (and its fridge), pharmacy, hardware (a shop's stock is its room type;
  shops keep it on shelves, never counters, which would hold kitchen things); `(any, any)` clutter. On the default town (#18) that is about two months of food for one player at 2.0
  hunger a day (`the_default_town_feeds_one_player_for_weeks`).
- **Weapons** are loot entries too (knives in the cutlery drawer, a pan in the oven, a bat in a wardrobe, a golf club
  in the coat cupboard, hammers and crowbars on laundry, shed and garage shelves, a rare fire axe). The floor weapons
  (`items::place_weapons`, `found_in`, at least one in the spawn house) stay for now, so the spawn house always has
  something to fight with; `found_in` folds into the tables when that is retired.
- **Saves:** `loot.ron` shares the `containers` fingerprint with `containers.ron` (no format change); a change loads
  with a note: opened containers keep what they hold, unopened ones fill from the tables as they are now. Not yet:
  food starting partly spoiled (needs item freshness, #11).

## Sound (`crates/client/src/audio.rs`; #16)

- **Cues are data:** `assets/data/sounds.ron` maps each `audio::Cue` (footsteps hard and soft, zombie shuffle and groan,
  light and heavy swing, blunt and blade hits, punch, body fall, stomp, shove, player hurt, door and window open, shut and
  break, battering a door or window, rummage, pickup, level-up, bird, and the wind, room-tone and cricket loops) to its WAV
  files under `assets/audio/` (one picked at random), `volume`, `pitch` range, `reach` (metres to silence), `muffle` (how
  much gets through a wall) and `voices` (at most that many at once, nearest first); `audio::parse_sounds` refuses a
  missing cue, an unknown field and bad numbers. Change a sound by changing its files. `--data DIR` with its own
  `sounds.ron` uses it, and the files next to it in `../audio`.
- **Placeholders for now:** every file is synthesised by `tools/audio/synth.py` (plain Python, standard library,
  deterministic; `--attribution` prints the ATTRIBUTION.md rows). The real sounds (ElevenLabs SFX, Suno music, groans we
  record) replace them file for file in #78, made with the **[elevenlabs-sounds skill](.claude/skills/elevenlabs-sounds/SKILL.md)**
  and `tools/audio/zsound.py` (`check` lists every cue's files, their loudness and which are still placeholders). Files are 16-bit PCM WAV (mono or stereo, 8-96 kHz; we decode them ourselves,
  `audio::parse_wav`). Every file has an ATTRIBUTION.md row and is used by a cue (a test checks both).
- **Release audio** (`release_audio.rs`; #154): ElevenLabs' terms forbid publishing its output as separate files, so
  ElevenLabs sounds (and any music under such terms) are never in this repo: `zsound.py promote` puts them in the art
  repo's `release-audio/` (`audio/<folder>/*.wav`, its own ATTRIBUTION.md, optionally `data/sounds.ron` and
  `data/music.ron`), and release builds carry that folder. The client reads `$ZOMBYE_RELEASE_AUDIO`, else
  `release-audio` beside its binary: each audio file there replaces the same-named one under `assets/audio/`, and its
  `sounds.ron` / `music.ron` replace the game's (not a `--data` folder's own). Without it the game plays this repo's files.
  Sounds we record ourselves are public as before.
- **What plays when** (client only, from the snapshots; nothing in the sim): footsteps from each player's movement, one per
  stride by stance (sneak quiet), hard or soft by the floor, none while climbing or asleep; active zombies within 12 m
  walking shuffle (a drag every 1.2 m); groans per zombie every 3-7 s attacking or battering, 7-15 s walking, 12-30 s idle (staggered, so a crowd
  doesn't groan together); door and window changes the server sends (`server_link::EdgeTransitionsThisFrame`, the edge
  before and after; a quickload's differences make none); a thump per battered door or window per frame (edge damage);
  combat events (hit sound by the attacker's weapon category, a kill adds a body falling, shoves once per player); a swing's
  whoosh timed to land with the blow; an item gone from the floor next to a player; rummaging while our first job is a
  search; our level-ups; wind (quieter indoors), room tone indoors, crickets at night, birds now and then by day outdoors.
- **How it sounds:** falls off with the square of distance to `reach`; low-passed and down to `muffle` when a wall, shut
  door or window is between (`vision::clear_reach` from our survivor); panned by the camera's right. Our own Bevy audio
  source (`audio::Voice`) does the panning, pitch, low-pass and looping (rodio's spatial sink pans the wrong way close up).
  Asleep, everything is quieter. `--mute` / `--volume V` (0-2) / M; the stats line says `sounds N (...)` each second.
  Offscreen renders have no audio.

- **Music** (`music.rs`, `assets/data/music.ron`; #78): sparse on purpose, since the town is meant to feel empty.
  Each `music::Mood` has its files (OGG Vorbis under `assets/audio/`, through Bevy's own player:
  no panning or muffling), volume, `gap` (seconds of silence after a Day or Night track) and fades. **When:**
  `Death` once when you die; `Danger`, looped, while zombies are on you (`music::in_danger`: one attacking
  within 6 m or three coming within 12 m, held 8 s); `Dawn` once as the night turns to day; else `Night` or
  `Day` by the daylight, a track then a silence. A change of mood crossfades. A mood with no files is silent
  (none ship yet: the tracks come from Suno, by hand). `--music V` (0..2, default 1, 0 none) under the master
  volume and M; asleep, quieter. A test checks the file and that every track exists and has its ATTRIBUTION.md row.

## Light and mood (`daynight.rs`, `camera.rs`, `fog.rs`; #83)

- **The look is isolation and fear** (docs/art-direction.md, "Light and mood"). The camera is HDR with a filmic tone map
  (TonyMcMapface), bloom only above sunlit brightness (light sources), a colour grade and a vignette that `daynight.rs`
  sets from the hour: warm and a little faded by day, warmest and richest at dawn and dusk, cold and closed in at night.
- **Sun:** never more than about 35° up, so shadows stay long; little ambient fill. One shadow cascade fitted round what
  the camera sees at the current zoom (`sun_cascade`; the camera is orthographic and 120 m back, so the default cascades
  put the whole view in the coarsest one), 4096 map: about 3 cm a texel at the default zoom.
  The light's direction follows the clock in steps of 0.3° (`sun_step`, about every 3.5 s), not every tick: each new
  direction turns the shadow map's texel grid, so a sun that crept every tick made every shadow edge flicker.
- **Night:** moonlight and ambient low enough that only edges and silhouettes read; the fog of war's unseen areas go
  nearly black (`fog_tint`); you see 10 m with no light (`fog::NIGHT_SIGHT`). A faint glow round the player lights its
  own feet. The palest things on the ground (road paint, kerb tops) keep a little of their colour as if catching the
  moon (`PAINT_NIGHT` in world.wesl), so the street's shapes still read. Unseen areas keep some of the difference in
  lightness between what things are made of (`UNSEEN_CONTRAST`), so a driveway doesn't vanish into its lawn.
- **Held lights:** an item with `light` in either hand (`daynight::held_light`, from our `PrivateView`): a flashlight is a
  shadow-casting `SpotLight` cone along the facing (`BEAM_OUTER`, aimed 4.5 m ahead), seeing out to its `radius_m` + 2
  along the beam at night; a candle (`needs_fire`) a small warm shadow-casting pool that wavers a little
  (`lights::flicker`). Zombies don't react to light (a sim change of its own).
- **Other lights in the dark** (`lights.rs`, #83 round two): **other survivors' lights**, from what `Snapshot::outfits`
  says they hold: the nearest three within 60 m get the same cone or pool as ours, shadows included (a beam mustn't
  come through a wall). **Window pools:** each ground-floor window of a room with a light on (`building_kit::room_lit`)
  is a warm point light at night (no shadows; `lights::spawn_window_light`, spawned and despawned with its building),
  lighting the ground outside, its room, and whoever walks through. **What's lit is seen:** at night a surface a light
  falls on shows outside your own 10 m, given a line of sight to it, up to `lights::FAR_LIGHT` (45 m, how far the sight
  rays are cast in the dark): world.wesl works it out per pixel from how brightly the surface is lit (its colour over
  its albedo, between `LIT_LO` and `LIT_HI`; moonlight is under both), and `lights::NightLights` lists the same lights
  for the fog mask, so zombies, survivors and items standing in a far pool or someone's beam are drawn. Client only.
  Cost: nothing by day; at night a few unshadowed point lights and, in multiplayer, up to three more shadow maps
  (about 0.8 ms each on an M-series Mac, as ours); not measurable on the GPU-less build machine.
- **Fog of war per pixel:** the sight rays go to the GPU (`FogState::rays`, 1,024 × 1 `R32Float`, with the range and cone in
  `FogState::sight`) and `world.wesl`'s `fog_at` works out what's seen per pixel by the same rule as the mask, so
  line-of-sight edges are smooth, not the mask's half-metre steps. The mask texture now only says what's indoors (its
  green; refilled when its origin moves on by a texel, every half metre walked). Which zombies, items and players to
  draw is `FogState::visibility(point)`: the value the mask's texel there used to hold, worked out when asked (a test
  holds it to the old raster). The rays are cast again only when the eye, their reach or the map changed, and uploaded
  only when they differ: standing still, `update_fog` does nothing.
- **The crowd is in the scene's light** (#85): `zombie_vat.wesl` shades zombies with Bevy's PBR lighting (flat normals
  from screen derivatives, the hit flash as emissive), so the sun, moon, ambient, held lights and shadows reach them,
  and its prepass entry points run the same VAT animation (crossfades included) into the shadow maps, so they cast
  animated shadows too (the shadow pass reads the shader clock from its own view layout, binding 1, see `clock()`).
  Zombies the fog of war hides cast none. Cost with 2,000 zombies round the player (1920x1080): no more than before
  (day 10.5-10.7 against 11.7-14.6 ms at 26 m; night with a flashlight 9.0-10.5 against 10.0-11.4 ms), the per-frame
  crowd lighting update it replaced having cost about as much as the extra shadow pass.
- **Cutaways** (`cutaway.rs`, `world.wesl`, `world_render.rs`, `silhouette.rs`; #84, redone per building): walls drop
  to 0.4 m stubs **per building and per wall edge, never by a radius or a hole round the player**. *Inside a
  building* (`cutaway::room_edges`): every outside wall of that building whose outward side faces the camera (two
  sides at each of the four camera angles; they follow `,` / `.`), and any of its walls that hides part of the room
  you're in (the floor behind it on screen, 5.5 m away from the camera, what a 3 m wall covers from 30° up, runs
  into your room: the room's own near walls and inner walls between it and the camera). Its far walls, inner walls
  behind your room and every other building stand. Its roof and upper storeys (every floor above the player's;
  players are only ever on the ground floor) fade out as one over 0.15 s and are then hidden
  (`world_render::hide_upper_floors`: a stipple by the amount in their `MeshTag`). *Anywhere*
  (`cutaway::body_edges`): a wall piece that covers the player's body from the camera drops, whichever building it
  is part of (the edge crosses the 0.6 m strip of ground between the body and the camera within 5.5 m); the facade
  you walk along, behind you, doesn't. **From outside every building keeps its roof and floors**; a building that
  doesn't cover the player isn't touched. **How it's drawn:** `Cutaway` (a resource) keeps the edges on their way
  down or up, eases each over `EASE_SECS` (0.15 s; the first decision of a session is taken at once, so pictures of
  a save are settled) and writes the amounts to a 128 x 128 Rg8 texture (one texel a tile, by the tile's coordinates
  modulo 128: red its north edge, green its west edge; the world material's binding 105). `wall_cut` in world.wesl
  reads it by a wall piece's anchor and leaves out what is above the wall's top, which sinks from 3.05 m to the stub
  as a straight line. The room's set is worked out again only when the room or the camera's angle changes, the
  body's every frame (a few dozen tiles), the texture is uploaded only on frames where an amount moved, and nothing
  is allocated per frame. **Nothing inside is hidden:** furniture, containers, items and characters never are; a door
  leaf drops to the stub's height with its wall (a shut door in a cut wall doesn't look like an open doorway) and a
  window becomes a low framed lip of glass standing on the stub (`world_render::piece_look`, by `Cutaway::amount`,
  the same eased amount the shader draws the wall by). Cut walls still cast shadows. **Details standing over the
  player from outside** (a porch roof, built a metre at a time; a shop's awning, a metre at a time; a door canopy; a
  porch post: `CutKind::Fade`, anchor its middle on the ground, radius its half-width) are thinned out whole, in a
  screen-space stipple (72% at most), while they stand in the strip between the body and the camera
  (`object_fade` in world.wesl, `cutaway::object_fade` for the tests: soft in space, so smooth as you walk); trees
  fade their foliage as before (#81). **F10** switches the cutaways off and on ("walls up" / "walls cut", the yellow
  line says which; `--walls-up` starts with them off; not kept between sessions): walls up, nothing is cut (the roof
  of the building you're in still goes).
- **Silhouettes** (`silhouette.rs`, `silhouette.wesl`): while something still covers the player from the camera
  (`Cutaway::occluded`: the roof or an upper storey of a building other than the one they're inside,
  `cutaway::bulk_hides`; or, walls up, a wall that would have dropped), the local survivor's own skinned meshes are
  drawn once more in one flat mustard tint (62% opaque, dim at night) with the depth test turned round, so only where
  something stands in front of the body: its shape through the roof or wall. The body's depth is moved 0.7 m toward
  the camera first (`SELF_CLEAR_M`; the camera is orthographic, so depth is linear), so its own limbs never count. Two
  more small draws in the transparent phase of the main pass, and none while the player is in plain view (the twins
  are hidden); it fades in and out over 0.15 s. **Zombies stay hidden behind walls by default.** The "zombie
  silhouettes" setting (**Shift+F10**, `--zombie-silhouettes`, `zombie_silhouettes` in `settings.ron`; off by
  default) gives the zombies the fog of war already draws (the ones your survivor sees; never one it doesn't) the
  same in dull red where a roof, an upper storey or a standing wall covers them from the camera
  (`zombie_render::covered`, looked at every 8th frame per zombie; a second instance of the crowd's mesh with
  `ZombieMaterial::silhouette`). Zombies the survivor only hears aren't in what the client draws, so they get none.
- **Turning the camera** (`camera.rs`): always on. `,` turns it a quarter turn one way and `.` the other, between the
  four fixed angles, never freely (Q and E are taken: quit on the death screen, pick up; while a chat line or a search
  is being typed the keys are its own). A turn takes `camera::TURN_SECS` (0.3 s), slow out and slow in
  (`camera::eased_yaw`), ends exactly on the angle at any frame rate, and one asked for during another carries on
  from where the camera is. Everything that goes by the camera reads `IsoCamera::yaw` or the camera's transform
  every frame, so it all follows a turn as it happens: WASD (`ground_axes`), the cursor's aim and clicks (a ray
  through the camera as drawn), the cutaways and hints (their sets are worked out again while the yaw moves: in the
  middle of a turn only the wall square to the camera counts as facing it or away), the silhouettes, the sound's
  panning. `--angle 0.5` starts part-way through a turn and stays (pictures), `--turn N@SECS[,N@SECS...]` turns N
  quarter turns by itself (recordings). `--rotate`, which turning used to need, is accepted and ignored.
- **Opening hints** (`opening_hints.rs`, `opening_hint.wesl`): a door or window on an outside wall that faces away
  from the camera is behind its own building on screen. While the player is within 10 m of its middle
  (`opening_hints::RANGE`) its outline is drawn through the building: a shut door blue, a shut window pale violet (with
  a cross), an open or broken one of either green (a way in), each fading in and out over 0.2 s, dim at night.
  `State::Barred` (locked, barricaded, boarded) is drawn at 45% of the alpha, but **no edge in the map is any of
  those yet**: `opening_hints::classify` names every `Edge` kind, so a new one doesn't compile until it is given its
  hint. Which openings (`hint_for`, `find`): doors and windows only (never an arch or a fence), on an edge with a
  building's room on one side only (an open porch's room counts as outdoors), whose outward side has more than 0.1
  against the camera's ground direction, and not of the building the player is inside (that one is cut open).
  **Fair information:** only openings, from the client's map copy, which every client holds whole; nothing of
  zombies, players or loot. Drawn as the silhouettes are: one material with the depth test turned round
  (`silhouette::behind_only`), the outline's depth moved 0.5 m toward the camera so its own wall doesn't count, two
  small shared meshes (16 and 28 vertices) and an entity an opening whose `MeshTag` holds its tint and fade (bits 0-7
  how much shows, 8-9 door / window / open, 10 barred), so each mesh's instances are one batch in the transparent
  phase of the main pass: no pass of its own, nothing full-screen. Worked out again only when the player has moved
  half a metre, the camera turned, the map changed or the player went in or out of a building; a frame in which
  nothing fades writes nothing, and nothing is allocated per frame (entities of hints that went are kept for the
  next). **Ctrl+F10** (Cmd on a Mac) switches them off and on, kept as `opening_hints` in `settings.ron` (on by
  default); `--opening-hints on|off` for a session; offscreen they are on unless the flag says off. Cost: not
  measurable (1920 x 1080 offscreen, 2,000 zombies, three hints in view, on the GPU-less build VM: best frame 166.0 ms
  before, 165.8 ms after; `--bench-live` best 128.1 against 128.7 ms).
- **F3, the light preview** (`dev_time.rs`): a slider at the foot of the screen (drag, or hold `[` / `]`) for the hour the
  light is drawn at (sun, moon, ambient, grade, held light, crowd, sight range); the server's clock, zombies' senses and
  the HUD's time don't follow it. F3 again goes back to the server's hour.
- **Cost** (M-series Mac, 1920 × 1080 offscreen, 2,000 zombies, under the same background load): within about 1 ms of
  before (day 9.1 against 7.9 ms, dusk 8.0 against 8.3, 60 m 9.4 against 9.8); a lit flashlight adds about 0.8 ms (its
  shadow map).

## Look and feel (`graphics.rs`, `vegetation.rs`, `film.rs`; Linear DCM-59)

The day is a warm, filmic late afternoon and the ground is grown over; the night is as dark as it was (see Light
and mood, and docs/art-direction.md). All of it is client only: nothing here is in the sim, a save or a snapshot.

- **Graphics settings** (`graphics::Graphics`, a resource): a `preset` (Low, Medium, High, Ultra, Auto, or Custom
  when flags changed one), `vegetation` (density, 0 to 2), a switch each for `grain`, `tilt_shift`, `vignette`,
  `bloom`, `contact_shadows` and `ground_detail`, the sun's `shadow_map` (texels a side), `msaa` (1, 2 or 4) and
  `render_scale` (the 3D's share of the window's pixels on a HiDPI screen; `render_scale.rs` draws the world into an
  image of that size, sharpens it lightly (CAS) and shows it behind the UI, which stays at full resolution; on a
  scale-factor-1 screen the scale is raised a third of the way back to 1; anything projecting a cursor into the
  world or the world onto the screen goes through `render_scale::to_view` / `from_view`).
  **High is the look the game is made to and the default** (3D at 0.75 on Retina); Medium drops tilt-shift, thins
  the grass to 0.6 and halves the shadow map; Low is grass 0.35, every switch off, a 2048 map, 2x MSAA and 0.6
  scale; Ultra is grass 1.5, an 8192 map and full-resolution 3D. **Auto** plays at High, measures 5 s of frames after the first 3, and keeps High up to 14.5 ms a frame,
  Medium up to 22 ms, else Low, then saves that (`graphics_ui.rs`); a game started with no settings file starts on
  Auto. From `graphics` in the client settings file (`settings.ron`, see UI scale; never read offscreen), then
  `--graphics low|medium|high|ultra|auto`, `--vegetation off|low|high|N`, `--no-post LIST` (`grain`, `tilt`,
  `vignette`, `bloom`, `ao`, `ground`, or `all`), `--msaa 1|2|4`, `--shadow-map N` and `--render-scale F`. **F8** opens a panel to pick
  a preset in game (saved); vegetation, the film pass, vignette, bloom, the shadow map and MSAA change at once,
  contact shadows and ground detail on the next start. **Measuring:** `--frame-bench N` (offscreen) prints mean,
  median, best and worst frame times and the mesh instances drawn; `ZOMBYE_FPS_LOG=1` makes a windowed game print
  its frame rate every second and a mean/median/p99 summary on exit (`fps_log.rs`). **A new effect gets a switch there, a place in each preset,
  and goes off in `Graphics::low`.**
- **The day's light** (`daynight::sky_at`, `grade_at`): the sun is golden all day and lower than it was (about 47° up
  at noon, 32° by mid-afternoon), the fill a weak violet, the grade warm with soft contrast and lifted blacks; the
  night's light and grade are unchanged (a test holds them). **Unseen ground by day** keeps three quarters of its
  colour, dimmed and warmed, instead of going grey (`materials::unseen_tint`, `unseen_colour`, in `WorldParams::fog_tint`
  and `lights.y`); at night it is as black as before.
- **The film pass** (`film.rs`, `assets/shaders/film.wesl`): one fullscreen pass after the tone map (Bevy's
  `FullscreenMaterial`): tilt-shift (the top and bottom of the screen soften, up to 2.6 px at 1080p, 13 taps) and grain
  (24 a second, by visual time, so it stands still in an offscreen render; about half strength at night). With both
  off the camera has no `Film` and the pass isn't run.
- **Ground detail** (`world.wesl` `ground_detail`, only in the ground material, `WorldKey::ground` / `GROUND`):
  outdoors, the ground's colour is broken up per pixel: broad drifts, flat triangular facets 1.7 m and 0.55 m across,
  and dry patches on grass and dirt. **Worn ground:** the mesher blends lawn toward `worn_ground` within 2.8 m of an
  outside door (`Land::wear`, from the doors in the chunk's cached tiles, whatever state they're in) and in a few bare
  patches a yard (`bare_patch`).
- **Contact shadows** (`world_render::contact_shadows`): a strip of black fading to clear, 0.6 m wide, on the ground
  and floors along both faces of every ground-floor wall and window: one small blended, unlit mesh a building (16 to
  about 1,000 triangles), spawned and despawned with it. They stand in for ambient occlusion: the screen-space kind
  needs MSAA off, and the window glass is alpha to coverage.
- **Instanced vegetation and litter** (`vegetation.rs`, `assets/shaders/vegetation.wesl`): eight kinds
  (`VegKind`: long grass, lawn grass, thin grass, a weed, red and white flowers, loose paper, small stones), four
  mesh variants each, built in code (12 to 91 triangles; blades are single triangles, dark at the root). **Where**
  is `landscape::scatter`, run per outdoor tile by `build_chunk` (`ChunkMeshes::veg`, a `VegSpot` list: position,
  kind, variant, turn, scale, dryness, rank), from tile hashes and noise alone, so a chunk always grows the same:
  long grass where nobody mows (by walls, fences, verges, overgrown lots), shorter on lawns, thin on dirt, under
  trees, beside paths and over bare patches, none on doorsteps, driveways, beds, curved kerbs or under a porch;
  flowers in drifts; weeds, paper and stones on pavements and in gutters. A patch against a wall keeps to its tile
  and quarter turns (it would poke into the room); in the open it is jittered and turned freely. **Drawing** is the
  crowd's path: each spot is an entity (a child of its chunk's ground entity, which holds the list as `ChunkVeg`)
  with a shared mesh, the one `VegMaterial` and a `MeshTag` (bits 0-7 dryness, 8-15 brightness), so Bevy batches each
  mesh's instances into one draw; no shadows cast. `VegMaterial` is the world material's fragment shader (same
  bindings 100-103: lighting, shadows, fog of war, night) behind its own vertex shader, which colours a blade by the
  tag (a mix of the palette's `grass_blade` and `grass_blade_dry`), sways it (gusts by world position and the clock),
  bends it away from the player's feet, gives it the ground's normal, and shrinks it to nothing over the last 16% of
  the drawn radius. **Culling:** a chunk's spots exist only while it is within `veg_radius(zoom)` of the camera's
  focus (1.34 x zoom + 3 m, 24 to 60 m: past the view's corners up to a zoom of about 42 m), spawned two chunks a
  frame as you walk (all at once for the first view and offscreen), removed 8 m further out. **Density:** a spot
  is spawned when its rank is under the setting: every tile's first patch ranks 0 to 1, a second between them 1 to
  2; `--vegetation off` spawns nothing.
- **Cost** (measured only on the GPU-less build VM, software rendering under other load, so these are upper bounds
  of the CPU-side work and say little about a GPU: best of three, 1920 x 1080 offscreen, 2,000 zombies, a yard by
  day): the default town averages 161 instances and 5,900 triangles of vegetation a chunk, about 5,000 instances and
  180,000 triangles round the camera at the default zoom, in at most 32 batches. A frame at 26 m: 148 ms with
  everything off, 176 ms with vegetation alone, 165 ms with the post effects, ground detail and contact shadows
  alone, 193 ms with it all, 237 ms at density 2; at 60 m 233 ms off and 363 ms on. Chunk building, scatter
  included: 0.53 ms mean on that VM. **Not yet measured on a GPU** (`--frame-bench 120` on the Mac, with and
  without `--graphics low --vegetation off`).

## Keeping a frame cheap on the CPU (`crates/client`)

Bevy keeps what it prepared for a mesh until something about it changes, so the client's rule is: **nothing that
stands still may count as changed.** Each of these cost milliseconds a frame before it was found.

- **Shader parameters that change every frame go in a buffer, never in a material asset.** A changed material asset
  makes Bevy extract, specialise and queue again every mesh drawn with it. The world materials (and the vegetation's)
  hold a handle to `materials::ParamBuffer`, a `ShaderBuffer` asset read as `var<storage, read>`
  (`materials::param_buffer`, `write_params`, which writes only when the values differ; the same size every time, so
  the GPU buffer is written in place). `VegExt` has a second one for its clock and wind. A new per-frame parameter is
  a field of `WorldParams` (keep `floats()` and the struct in `world.wesl` in step).
- **Write a `Transform`, `Visibility`, `MeshTag`, `Text` or `Node` only when its value changes** (`if *x != new`).
  A `Mut<T>` passed on as `&mut T` counts as changed even if nothing is assigned: pass the `Mut` itself. Things that
  ease toward a rest pose (door leaves, panes) are put there when close and then left alone. `sync_zombies` places
  only zombies that are in sight and moved, and skips those that stay out of sight.
- **Resources too:** `tick_server` hands the client's map on as the `ResMut`, so `ClientMap` is changed only when an
  edge is.
- **Measuring:** `--bench-live N` (offscreen, the world running, the player walking a 24 m square; add `--crowd N`
  for N zombies round the player, on a server one started with `--allow-dev`) prints the frame times and, every 150
  frames, the frame's churn: how many meshes moved, changed or came into view, and how many UI texts and nodes
  changed. Built with `--features zombye-client/profile` it also prints CPU ms a frame by part, the main world's
  update and the top 40 spans (`ZOMBYE_PROFILE_TOP=N`). On the GPU-less build VM only these CPU-side numbers mean
  anything. **A change to rendering is checked with pictures:** render the same scene before and after into
  `target/cmp/NAME-a.png` and `NAME-b.png` and run `cargo test --release -p zombye-client compare_pictures -- --ignored
  --nocapture` (pixels that differ, by how much, where).
- Measured on that VM (CPU-side spans, `--bench-live 200 --crowd 2000`, 1280 x 720; before and after these rules):
  the main world's update 7.4 to 5.1 ms, mesh extraction 2.0 to 0.1 ms, `update_fog` 2.2 to 0.05 ms, and material
  specialising, queueing and bind groups (2.3 ms together) to nothing; as a client of a server with 5,300 zombies in
  the snapshot, 8.0 to 5.4 ms and 3.8 to 0.9 ms. The single-threaded sim: 1.0 ms a tick for 5,000 active zombies, 2.5
  ms with a horde of 5,000 after a player (`--bench`). **Not yet:** the sim's tick runs inside the frame it falls in;
  every zombie and every patch of vegetation is still an entity (about 8,000 mesh entities in a yard), which Bevy's
  per-entity systems walk each frame.

## Characters on screen (`crates/client`)

- Both are grey-box boxes on a small skeleton (`rig.rs`: bones as parent + rest pivot, a pose is a rotation
  per bone plus a root offset; positive X swings a hanging limb backward, positive Z toward the model's left).
- **Zombies** (`zombie_render.rs`, `zombie_vat.wesl`): poses baked into a vertex animation texture, one instanced
  batch per outfit variant. The VAT has more clips than `ZombieAnim` (16 slots, `Clip`, the most the tag's 4 bits hold): Idle, Walk (rate from
  speed / stride), Attack, Thump (loops, clips 0-3), Stagger and Stagger2 (a stagger plays one or the other, picked
  per stagger from the zombie id and its count of staggers, so every client agrees), Fall then a Down loop, GetUp,
  three deaths (backward, forward when hit from behind, and limp when killed on the ground) Lunge, Climb (through a window) and Vault (over a fence; the client picks which from its map copy,
  `TileMap::window_opening`), and Groan (baked, not
  played: the sim's groans aren't in the snapshots, and a hunting zombie is moving, which an on-the-spot roar
  can't show). The client picks the clip and, for one-shots, its position (from when the anim changed), so
  one-shots start at frame 0 and stop on their last frame. **Every clip change crossfades over 0.2 s**
  (`FADE_SECS`): loop to loop with both playing on the shader's clock (`Fade::Loop`); into or out of a one-shot or
  the ground loop the old clip is held where it was (`Fade::Held`) while the new one fades in, the new one played
  at a position the client works out (`loop_position`, the shader's own sum on `Time::elapsed_secs_wrapped`, so the
  hand-over to the shader's clock after the fade doesn't jump). Clips are also authored to start and end near each
  other's poses (`clips_join_up`). The MeshTag: bits 0-3 clip, 4-9 outfit, 10 direct, 11 hit flash, then either
  (looping) 12-17 phase, 18-25 speed (1/48 m/s), 26-27 the looping clip it fades from and 28-31 its weight (/15),
  or (direct) 12-19 the clip position (/255), 20-23 the clip it fades from, 24-28 where that one is held (/31) and
  29-31 its weight (/7). The VAT is RGBA16F (one column per vertex, at most 8,192; one row per frame); the mesh's
  bounds are two zero-area triangles at the corners of the box every baked frame fits in (`POSE_BOUNDS` for the
  grey-box, 7 bones of boxes, the fallback).
- **A baked crowd character** (#53) replaces the grey-box when there is one: `--crowd-vat`, else
  `ZOMBYE_CROWD_VAT` (the MCP `screenshot` client inherits it, so a work-in-progress bake from the art repo can be
  reviewed in game), else every `assets/models/characters/*.zvat` by name (where promoted ones go). The value is a
  `.zvat`, a folder (every `.zvat` in it, by name) or several separated by `,` (or `:`); `none` forces the
  grey-box, and a file that won't load is a warning and left out (none left: the grey-box). **Several files are
  outfit variants:** each its own mesh, VAT, texture and material (one draw batch each), and each zombie wears one
  by a hash of its id (`variant_of`). `zvat.rs` reads the file `tools/blender/zvat.py` writes (version 2: half-float positions, the precision
  the VAT holds anyway; version 1's f32 still read; the crowd is baked at 15 frames a second, played by seconds;
  validated: sizes, indices, finite positions, unique clip names) and its texture: `ZOMBYE_CROWD_TEXTURE` (for every variant), else
  `<stem>.png`/`.jpg`/`_base_color.png`/`.jpg` beside it, else the base-colour image in the `<stem>.glb` beside it;
  mip-mapped on load (linear-light averages), at most 1024 px. File clips fill the slots by name (`idle`, `walk`,
  `attack`, `thump`, `stagger`, `stagger2`, `fall`, `down`, `getup`, `dieback`, `diefront`, `diedown`, `groan`, `lunge`, `climb`, `vault`; a file's `crawl` has no slot until the sim has a crawler;
  case, `_` and `-` ignored); loops keep their own rate (the walk its stride, the down loop its length). One-shots
  play over their own length sped up to a cap (`baked_max_secs`: stagger 1.0 s, stagger2 0.9, fall 1.3, deaths
  2.0 / 1.5 / 1.7, groan 3.0, lunge 0.6, climb 2.0, vault 1.6; the sim's stagger lasts 0.6-0.9 s and the crossfade covers the cut) and the get-up
  over exactly `GET_UP_SECS`. Slots the file lacks get stand-ins (`Source`): idle and walk stand in for each other;
  attack and thump are the idle (else the walk) leaned forward 0.3 / 0.12 rad; the one-shots and the Down loop move
  the idle's first frame as one piece by the grey-box clip's root motion (toppling about the feet, getting up,
  reeling back), its lowest point kept on the ground. The shader samples the texture by UV and tints it per outfit
  (`outfit_tint`: 8 washes times a brightness); the grey-box binds a 1 × 1 white texture and keeps its body-part
  colours. Cost, measured with the first crowd character in review (#53: a ~600-triangle crowd LOD, 879 vertices,
  and an outfit variant with a wearable, 1,063) at 1920 × 1080 offscreen with 2,000 zombies in view (M-series Mac):
  at the default zoom (26 m) a frame took ~4.1 ms (one variant) / ~4.3 ms (both mixed) against the grey-box's
  ~3.7 ms (a full-detail 2,970-vertex body: 6.5 ms); at 60 m ~5.9 / ~6.2 ms against ~5.6 ms. Keep crowd meshes to
  about this size: the crowd is vertex-bound.
- **Survivors** (`survivor.rs`): 12 bones (knees, elbows, a right hand), a Bevy skinned mesh with
  StandardMaterial (lit, casts shadows), posed on the CPU each frame with blending: locomotion (idle, walk, run,
  sneak crouch; the cycle advances with distance actually moved), actions on top (punch / one-handed chop /
  two-handed sweep by weapon category, shove, stomp; timed so the blow lands when the server lands it), an
  additive hurt flinch, and a fall onto the back when dead. Every pose is grounded (lowest boot on the floor).
  The held weapon is a child of the hand joint. Your jacket is mustard, other players' teal. This grey-box is the
  fallback when there's no modular character.
- **The modular survivor** (#62, #63; `character.rs` reads, `survivor_model.rs` draws): one base body on one skeleton
  (`tools/blender/zgarment.py`'s body GLB: the skinned body with a `_REGION` attribute per vertex, its clips idle, walk,
  run, sneak, and its sockets `eyes`, `head_top`, `neck`, `hand_r`, `hand_l`, `back` in the body node's extras) and a
  GLB per wearable, skinned to the same skeleton by bone name (rigid ones wholly to their socket's bone), with the body
  regions it `hides` in its extras. Files: `--survivor PATH` (a body GLB, or a folder with a `*_body.glb`, e.g. a
  character's `export/` in the art repo), else `ZOMBYE_SURVIVOR` (the MCP `screenshot` client inherits it, for review
  before promotion), else `assets/models/characters/survivor_01.glb`; `none`, or no file: the grey-box. Wearables: every
  GLB in `assets/models/wearables/`, then `wearables/` beside the body, by file name (an item's `model`).
  **Animation:** locomotion from the body's own clips, sampled on the same distance-driven cycle, each clip's stride
  measured from its planted feet (which way it travels, its phase and stride) after its legs are swung wider to reach `CADENCE` (about 2.9 steps a second at the game's brisk 3 and 5.5 m/s, feet planted; the walk is `walk_alt`), the idle's 40° turn taken out. **Directions** (`STANCE_CLIPS`): by the way he moves off his aim, walking and sneaking blend the clips either side (forward, left, right, back), running (forward and the back diagonals) plays the nearer one with the hips turned the rest of the way and the chest kept on the aim (`--pose run@90:0` holds that); the grey-box's
  poses carried over onto the skeleton for the rest (`Retarget`: each grey-box bone's turn from its rest put on the
  matching bones in the idle's first frame; the spine shares the torso's turn, the neck half the head's): the arm
  holding a weapon at the ready, swing / shove / stomp (timed as before; but a body clip named `punch`, `swing_one`,
  `swing_two`, `shot`, `shove` or `stomp` plays instead of the grey-box poses for that action, by the held weapon's style:
  made in place, the blow at 45%, from and back to the idle's first pose; the whole body standing, the upper body over the
  gait when moving, a stomp always whole; `survivor_model::ACTION_CLIPS`; besides those, `swing_one_b` / `swing_two_b` are second swings picked at random per swing, `stab` is a blade's swing, and `ground_one` / `ground_two` play over a stomp with a one- or two-handed weapon in hand: only the look, the stomp's damage stays the boot's; `action_clips` says which stand in when a body lacks one), the flinch (additive) and the fall onto the
  back when dead or asleep; then grounded on the lowest foot vertex. `step_movement` and prediction are untouched.
  **Dressing:** `Snapshot::outfits` (`net::OutfitView`: worn items, the back and shoulder bags, what's in each hand,
  by item index; every player, every snapshot) says what's drawn, for us and for others, changing the moment it does: the body with the
  regions worn wearables hide cut away, and every worn model merged into one more mesh (both cached per outfit; a garment whose regions are all hidden by a model on a higher layer that reaches at least as low, a t-shirt under a hoodie, isn't drawn; under a shorter leather jacket it is), and a worn model with a hem (`hem_z` in its extras: shirts, hoodies) cuts away, 3 cm above it, the faces other garments have in the regions it hides (a wearable can carry a `_REGION` per vertex: the jeans' top is Torso), so trousers never show through a shirt worn out over them, so a
  survivor is body + outfit + held weapon + ring draws. An item with no model is a grey-box: clothing tints the regions
  it covers (sampling a light texel of the shirt, so the colour shows), an accessory is a box on its socket, a bag a box
  on the back (or the left hip from the shoulder). **A bag in the hand** (#91: a carrier bag, a duffel) hangs straight
  down from the grip whatever the arm does: each hand has a hang joint past the rig's bones, posed every frame at its
  grip with the T-pose's arm turned down (`SurvivorModel::hang_joints`), and the bag is skinned to it; its model is
  built on the right hand and mirrored for the left (`SkinMesh::hung`). The held weapon sits on `hand_r`. Players are told apart by a flat
  ring round their feet, ours mustard, others teal, unlit but dimmed with the daylight to 15% at night (no glow in the dark).
- **Rising in what it wore** (#63): a player who dies infected has what it wore noted (`World::rise_looks`, item ids,
  before it drops everything), the zombie it rises as keeps it (`World::risen_look`, until the corpse is cleared away),
  and `Snapshot::risen` sends it for the zombies listed; the crowd draws the baked variant nearest to it
  (`zombie_render::variant_for`: a variant's wearables are its file name past the plainest one's, `zombie_02_hoodie` is
  "hoodie"; most matches and fewest extras, else by id as usual).

## Zombies

- Struct-of-arrays in `sim::zombies`, including the AI columns (`state`, `target_player`,
  `target_pos`, `timer`, `cooldown`, `groan_timer`, `sees`, `direct`, `thump_edge`). No per-zombie
  allocation in the tick.
- States (`ZombieState`): `Wander` → (hears something) `Investigate` the sound's position →
  (sees a player) `Chase` → (within ~2.4 m, in sight) `Lunge` → (within reach) `Attack` on a
  cooldown. A chaser that loses sight goes to the last place it saw them and searches there until
  its memory runs out (`memory_secs`), then investigates that spot, then wanders.
- **Doors and windows** (`Thump`): zombie flow fields may lead through shut doors and intact windows
  at an extra path cost (`flow::WINDOW_BREACH_COST` < `DOOR_BREACH_COST`; open windows cost
  `CLIMB_COST`), so a chaser or investigator whose target is in a shut house heads for the nearest
  door or window. When its path's next (straight) step crosses one within `thump_reach`, it enters
  `Thump` and batters it every `thump_cooldown` (jittered) as `ZombieThump` events in
  `World::thump_events`; the world applies them in order after the zombie pass, so a crowd stacks
  damage, and breaks the edge at 0 HP (`World::break_events`, walk grid updated, flow fields covering
  it invalidated). Each thump is a `thump_noise` sound at the edge that lures hearers to the far side
  (`Sound::lure`), so the zombies it draws join in; breaking makes a louder one. **Crowd pressure:** a zombie
  within `crowd_reach` (1.6 m) of the edge whose path leads through it (from its tile or the next two along
  the path) and that stands behind someone battering it from within reach pushes in and thumps too, each
  thump doing `crowd_thump_frac` (0.3) of one (`ZombieThump::damage`). A thumper's interest (`timer`, capped
  at `thump_interest_secs`, 20 s, when it starts) runs down unless it sees a player or hears any player's
  noise (which tops it up again); when it runs out it gives up (below). Once the way is open (broken, or a
  player opened it) it goes back to chasing or investigating. Zombies never open doors.
- **Giving up:** a chaser or investigator pushed back by walls (not by the crowd) that hasn't got
  `stuck_move` (1 m) anywhere for `give_up_secs` (12 s) gives up, and so does a chaser whose player's field has
  no way to them at all within 16 m (not even through a door or window). Giving up (and a thumper losing
  interest) means milling about where it stands for `investigate_secs`, then wandering, `bored` for
  `bored_secs` (20 s): it ignores noise that isn't a player's, and only notices a player close up (within
  `sure_sight_frac` of its sight range). A player's noise, being seen or being hit ends it. They climb
  through open/broken windows at `climb_speed`, and need a clear walking line (not just sight
  through glass) to lunge or strike.
- **Sight** (`senses::sight_pass`): full range in a forward cone, less at the sides, ~2 m behind;
  scaled by daylight (`senses::daylight`, the client's sky curve), the player's stance (sneak much
  harder, run easier) and whether they move; needs `vision::line_of_sight` (windows don't block it).
  A seeded per-check chance (`rng::hash01`) makes the edge of sight unreliable. Each zombie looks
  every `sight_interval_ticks`, and only zombies near a player are tested (`SpatialHash::for_each_within`).
- **Hearing** (`senses::hearing_pass`): `World::emit_sound(pos, radius)` queues a sound heard on the
  next tick. Moving players make footstep noise every tick by stance, doors and windows make noise, hunting
  zombies groan now and then (which pulls their neighbours in, so hordes snowball). Through walls
  (no line of sight) only `wall_muffle` of the radius carries. Hearing sends a zombie to the sound's
  position, not to the player.
- **Paths:** `flow::FlowCache` keeps up to 40 local flow fields keyed by `FlowKey::Player(id)` or a
  noise point snapped to a 6 m grid, builds at most 3 per tick (missing ones queue; the zombie walks
  straight meanwhile) and evicts the least recently used field that isn't in use. With many players (#70): of the
  hunted players who moved, the one whose field is oldest is rebuilt first, so each gets its turn when there are more
  than the budget covers; one build a tick is kept back for missing fields while any wait; and players' fields beyond
  the first come on top of the 40 (a full server still has room for noises). One or two players: as before. Fields search a
  precomputed `WalkGrid`; after editing `world.map` directly (tests, editors) call
  `World::map_changed()`. A zombie with a clear straight line to its target (`vision::clear_path`,
  rechecked every 8 ticks) skips the field.
- Attacks are recorded in `World::attack_events` (`ZombieAttack { zombie, player, landed, damage }`,
  cleared each tick) and hurt players (see Combat). Hit zombies stagger, fall `Down` or die.
- Every number is in `ZombieTuning` (`WorldConfig::zombie`, serde with defaults), ready to become a
  data file / sandbox options. Our own values; keep them that way.
- `set_goal` is a debug override: zombies within 120 m path distance head for that point, senses off
  (active and near zombies; virtual ones ignore it).

## Simulation tiers (`sim::tiers`)

Zombies far from every player get less of the simulation, so a world can hold tens of thousands
(issues #2, #3). All tiers live in the one struct-of-arrays with a `tier` column (`zombies::Tier`): a
zombie keeps its id, its place in id order and every AI column whatever its tier, so it keeps what it was
doing (a zombie investigating a noise is still heading there when promoted) and `state_hash` covers all
of them (tier included). Virtual zombies are also indexed per 64 m region (`RegionIndex`, rebuilt each
tick), which their bulk update walks; on its turn a region also feels the pull of loud events and sends some of its
wanderers off toward them (`tiers::step_virtual`, see Population below). Numbers are in `TierTuning` (`WorldConfig::tiers`, serde defaults; ours):

| Tier | Where (chunk distance to the nearest player) | Rate | What runs |
|---|---|---|---|
| Active | ≤ 60 m | every tick | everything: sight, hearing, state machine, flow fields, separation, combat. Only these go to clients. |
| Near | ≤ 150 m | every `near_interval_ticks` (5: 4 Hz), staggered by id | hearing (the normal pass: near zombies are in the spatial hash), heading for what it heard or remembers (along a flow field active zombies already keep built, else straight), collision with walls; no sight, strikes, thumps or separation; chasers keep the spot they remember |
| Virtual | beyond | each 64 m region every `virtual_interval_ticks` (100: 0.2 Hz), regions staggered | slow wander, or a drift toward what it heard; only sounds ≥ `virtual_hearing_min` (30 m) are checked, full radius when both ends are outdoors, else `wall_muffle` of it, no ray; wanderers in a region an attractor pulls set off toward it (migration, below); moves never cross anything that blocks walking or end on a blocked tile |

- **Per chunk, with hysteresis:** each tick (with at least one player; with none, nothing changes tier) every
  chunk's distance from the nearest player (dead ones count) to its nearest point decides the tier of the zombies
  in it (`tiers::next_tier`): promotion at the range, demotion only `hysteresis` (20 m, more than a chunk) beyond
  it, so crossing a chunk border can't flip a zombie back and forth. A big jump goes straight to the far tier.
- **Promotion and demotion** (`tiers::assign`): leaving the virtual tier, a zombie is settled on a free tile it can
  walk to from where it is (`tiers::settle`: its own tile if free, else a breadth-first search over walkable steps),
  so it never lands in a wall, tree, furniture or a sealed building it wasn't in. Demoted, a lunge or strike
  becomes a chase from memory. Either way it forgets what it saw and keeps its state, target and timers. Corpses
  age on every tier.
- **The far clock (#57):** the virtual tier, the attractors and respawn count in `World::far_tick()`, not `tick`. It is
  `tick` until every living player is asleep; then it takes `sleep_time_scale` (30, rounded) steps a tick, each a full
  virtual step (`tiers::step_virtual(.., clock, tick, ..)`: the regions whose turn it is, rolls hashed on the clock),
  attractor expiry and respawn check, so the far horde lives through the night you sleep through. Active and near
  zombies keep real time. `TierTuning::far_follows_sleep` (default on) turns it off. `World::far_ticks` (how far ahead it
  is) is saved in the optional `FARC` section (only when not 0) and in `state_hash` only when not 0, so worlds nobody
  slept in save and hash as before. Cost asleep on a 1 km map with 21,500 zombies and a lasting noise in town: about
  3.8 ms a tick, 8 worst (6,300 virtual zombies moved a tick; `sleeping_through_the_far_clock_stays_in_the_tick_budget`). Crowd separation is capped per tick (`MAX_CROWD_PUSH`), so a pile of promoted zombies spreads
  out instead of being flung through a wall.
- **Snapshots** list only active zombies within `interest_radius` (80 m) of a player (every active zombie when
  there are no players); the client despawns the rest. A multiplayer server cuts them down again for each client (#22, see Multiplayer).
- `TickStats` has the tier counts, promotions/demotions and per-tier timings (`tier_ms` for the tier pass plus
  index rebuilds, `active_ms`, `near_ms`, `virtual_ms`); the spatial hash only spans the area its zombies cover.
- Perf (M-series Mac, release): 1 km map, ~1,300 active + ~20,200 virtual (+ ~950 near), a horde of ~900 chasing a
  player: about 1.1 ms/tick average, ~3 ms worst (`tiers_hold_a_thousand_active_and_twenty_thousand_virtual`).
- **Many spread-out players (#70):** with 32 players over a 1 km map nearly every zombie is active (about 20,000 of
  22,500), so the active tier's cost per zombie is the tick. What keeps it down, all giving exactly the results of the
  plain way (the same `state_hash`): **open ground** (`flow::WalkGrid::open`, a bit per tile kept with the grid: the
  tile and the eight round it hold nothing that blocks and no edge that blocks or is climbed through), where a zombie's
  body can't be pushed back or be in a window, so the active and near updates skip `resolve_circle` and
  `in_window_opening` there (`WalkGrid::resolve_body`; a hand-edited map needs `World::map_changed()` for this too);
  zombies only ask the players' bodies where one is about (`Ctx::near_players`, the chunk distances); **chunk distances**
  are stamped per player over the chunks within `TierTuning::chunk_reach` (exact wherever a tier is decided, "far"
  beyond), not players times every chunk; the spatial hash's rebuild makes one pass less over its table (a million
  cells on a 1 km map) and `for_each_near` reads three row slices; **hearing** (`senses::hearing_pass`) casts no ray for
  a zombie the sound would change nothing about (`would_react`), and idle listeners hear the sounds backward and stop at
  the first, since only the last one they hear counts (one ray instead of one per noise when many fall on a tick).
  Measured on a shared 32-core VM (release; the world is one thread): the 32-player bench (`--bench --players 32
  --spread`, ~17,900 active) about 6.8 ms/tick, worst 11 (9.1 and 13.6 before); over TCP with 32 bots (`--bench --bots
  32 --spread`, 600 ticks, ~20,350 active, up to 2,800 zombies in a view) the whole tick 7.6-7.7 ms, worst 10 (8.7-11.6
  and 11-22 before), of which the world 6 ms, 6.2 KB/s down per client (the one in the horde 25) and 0.7 up; a 2 km map
  with 44,000 zombies and 32 players 7.4 ms, worst 13 (the first tick, when everything far is demoted).
  `thirty_two_spread_out_players_keep_the_worst_tick_within_budget` holds it.

## Navigation (`sim::nav`)

Long trips follow plans over a coarse graph (issue #44; the hierarchical A* of PLAN §4.3), so a far horde drawn
to a noise comes round through streets and gaps instead of piling up behind buildings.

- **Graph** (`NavGraph`, from the `WalkGrid`, built with the world: ~27 ms, ~4,200 nodes and ~17,000 links on a
  1 km map): one node per connected open area of each 16 m chunk (id `chunk << 8 | area`), portals across chunk
  borders (open ground, open or broken doors, arches; open windows as a climb; one per pair of areas and kind),
  and shut doors and intact windows as breach portals (inside chunks too), at the flow fields' costs. An edit
  through the world (`set_edge`: doors and windows opened, shut, broken) rebuilds the chunks around it
  (`Nav::edge_changed`), always giving what a fresh build would; `map_changed` rebuilds it all. Derived, not saved.
- **Plans** (`Nav`): A* from the zombie's node to its target's node, through breach portals only when the target
  can't be walked to. Shared: every node along a plan is indexed with it. A miss queues a request; `Nav::update`
  makes at most 16 a tick (and starts none after 40,000 node expansions) while the zombie goes straight. Edits drop
  the plans through the chunks they touch. Plans and queued requests are saved (format version 2); the index is
  rebuilt on load.
- **Users:** near zombies with no flow field covering them (`Nav::way`: toward the next portal; they stand at a
  door their plan leads through), virtual zombies heading somewhere (`Nav::trace`: tile by tile along small
  in-chunk fields, so never across anything that blocks walking; they wait at doors), and active zombies whose
  target is beyond their flow field (they batter a door their plan leads through). `TierTuning::long_paths`
  (default on) turns it off. Stats: `TickStats::nav_ms`/`nav_plans`/`nav_expanded`, `World::nav()`.
- `a_far_horde_finds_the_way_round_a_row_of_houses`: 300 virtual zombies 250 m off behind a row of shut houses
  with fenced back gardens all arrive within 13 minutes; moving straight, none do in 20.

## Population (`sim::population`, `assets/data/population.ron`)

Where zombies live is data (issue #4): `population.ron` (`population::PopulationDef`, validated by
`parse_population`, unknown fields rejected, every field optional with the built-in defaults; loaded by
`net::content::load_population` into `WorldConfig::population`; a data folder without it gets the defaults).
All numbers are ours.

- **Zones** come from the generator (`worldgen::zone_at`, `World::zone_at`): `Indoors` (any room, scaled per room
  type by `rooms`), and outdoors in town the `TownCore` (the civic and commercial blocks along the main street and the
  street itself, `World::town_core`: one rectangle north of the street, one south; `core_fraction`, default 1, of each
  one's length along the street round its middle), `Street` (roads, parking lots, sidewalks) and `Yard`; outside town `ForestRoad`, `ForestEdge` (within `forest_edge_width` of
  town) and `DeepForest`. `density` is zombies per km² of open (unblocked) ground per zone; `scale` multiplies it.
  `World::populate` places them by rejection sampling on tile density (none within 14 m of the player's start or in
  that house), `WorldConfig::zombies` (`Option`) either sets the total (proportions kept) or, `None`, takes the
  natural number. Speeds: `fast_share` of them at `fast_speed`, the rest at `slow_speed`.
- **Per region** (the virtual tier's 64 m `RegionIndex`) `World::population()` keeps the `target` count (density ×
  area, scaled to the total), the `zone` with the most open ground, and `last_seen` (the tick a player was last
  within `respawn.seen_distance`).
- **Respawn** (`respawn`, off by default): every `interval_secs`, a region whose nearest point is at least
  `min_distance` from every player, not seen for `unseen_secs`, holding fewer than `top_up_below` of its target, is
  topped up to it (`max_per_region` at a time, `max_per_check` in all), placed by density with seeded hashes, on the
  virtual tier, never in or right by the spawn house. `TickStats::respawned`.
- **Migration toward noise** (`migration`): every sound of radius ≥ `min_radius` (25 m: breaking a door, the debug
  `make_noise`, later shots and alarms) leaves an `Attractor` at its lure point, reaching `reach_mult` (4) × its
  radius (at most `max_reach`; `indoor_mult` 0.6 of that when made or luring indoors, so a big noise in a house still
  pulls the far horde, unlike hearing through walls), whose pull spreads outward at `spread_speed` (3 m/s) and fades
  linearly over `life_secs` (600; `lasting_secs` 1800 for `World::make_noise(.., lasting: true)`). Strength felt at
  distance d: `strength × (1 − (d/reach)²)` inside the spread front. A loud sound within `merge_radius` of an
  attractor widens and prolongs that one (its front keeps spreading from when it began); at most `max_attractors`.
  On a virtual region's turn it takes the strongest pull on it (`Population::pull_on`, the per-region field) and each
  of its wandering, not-bored zombies sets off with chance `join_chance × pull`, as an investigator heading for a point
  within `gather_radius` of the attractor with time to walk there, so it keeps going when promoted. Near and active
  zombies only hear as usual. A 120 m noise brings a cluster 410 m away in over 5–15 minutes; nothing moves at once
  (`a_loud_noise_draws_the_far_horde_in_over_minutes`, `an_indoor_loud_noise_still_draws_the_far_horde`).
- Attractors, `last_seen` and the respawn count are in `state_hash`. Cost: a few attractor checks per region update.

## Saves (`sim::save`, `net::save_file`)

`World::save(&self) -> Vec<u8>` and `World::load(bytes, &content) -> Result<World, LoadError>` (issue #5). `sim` only
turns the world into bytes and back; `net::save_file` does the files: `save`/`load`, `list` (a folder's saves, newest
first, from each file's small info section, `save::read_info`), `path_for` (save names are plain file names, so a
request can't write outside the folder) and `write_atomic` (a temporary file in the same folder, flushed, then renamed
over the old one: a crash mid-write leaves the previous save whole).

- **Exact restore.** A loaded world has the same `state_hash` and, given the same inputs, the same future:
  `a_loaded_world_steps_like_the_original` steps a saved-and-loaded world beside an unsaved copy through a fight with
  combat, a battered and a broken door, corpses, respawn, attractors and all three tiers, comparing hashes every tick,
  and saves again mid-run; `net`'s `a_loaded_server_carries_on_like_the_saved_one` compares snapshots and prediction.
  So everything the future depends on is saved, hash or not: both RNG streams, players with their latest input,
  queued and under-way actions and cooldowns, every zombie column on every tier, sounds made but not yet heard,
  door/window hit points, ground items, inventories and item jobs, skills' XP, opened containers, population (targets, zones, last seen, attractors, ids), the clock offset, the debug
  goal, and the flow-field cache (which fields, in order, with their build/use ticks and queued requests; a field is
  rebuilt on load from the map unless that wouldn't reproduce it, e.g. one gone stale after a door changed, which is
  stored whole). Rebuilt on load: walk grid, spatial hash, region index, chunk distances, the goal's field, the
  container sites, each player's weapon in hand and load modifiers.
  Replication queues (edge changes, edge damage, combat events) and last tick's outputs are not saved: a joining
  client gets the full state. `TickStats` after a load only has the tier counts.
- **Adding state:** `World::save` destructures every `World` field, so a new field doesn't compile until it is saved
  or listed as derived there; zombie columns are listed once in the `zombie_block!` macro in `save.rs`, whose patterns
  name every `Zombies` column. Change the layout of anything saved, and bump `save::FORMAT_VERSION`.
- **The map is regenerated, not stored**: from the saved seed and size plus the content's templates, then the edges
  edited since (`World::map_edits`: doors and windows opened, shut, broken) are applied. A 1 km map is a million tiles
  (several MB even packed) while a game's edits are a few hundred bytes; the price is regenerating on load
  (~0.2 s on 1 km, ~45 ms on the default map). A hash of the generated map (`map_base_hash`) is checked on load
  (`LoadError::MapChanged` if the generator changed). After `World::map_changed` (a hand-edited map: tests,
  editors) the whole map is stored instead. **Street props (#61) changed every shipped map:** saves made before
  them no longer match (`ContentPrint::templates` now hashes the templates with `street_props.ron`, and the map has
  props), so they're refused as `ContentChanged` ("building templates or street props changed") and load only with
  `--load-anyway` / `load {force: true}` (onto the new map, door and window edits that still fit). The save format
  didn't change (no version bump).
- **Content vs settings.** The caller passes content (templates, weapons, items, containers, loot, recipes, population, street props, as for `World::new`); the save
  keeps a fingerprint of each (`save::ContentPrint`, hashes plus ids). `World::load_with(bytes, content, LoadOptions)`
  returns `Loaded { world, notes }`, and `save_check::plan_content` sorts changed content. Compatible, loaded with a
  note: a weapon's numbers (the new ones apply), weapons added, reordered or removed when the save uses none of the
  removed ones (saved weapon indices are matched up by id), the same for items (their numbers apply; indices matched
  by id), `containers.ron` while every opened container is still one, `loot.ron` (only unopened containers fill
  from the new tables), `population.ron` (respawn and migration settings apply;
  regions keep their saved targets), `recipes.ron` (queued crafts matched up by id; those of a removed recipe called
  off), templates that leave the generated map the same, and any template change for a
  save that stores its map whole. Incompatible, `LoadError::ContentChanged` (`MapChanged` if only the generator
  changed): templates that change the map, a removed weapon a swing under way uses, a removed item the save holds
  anywhere, an opened container that is no longer a container. `LoadOptions { force: true }` (server
  `--load-anyway`, MCP `load {force: true}`, client `--load-anyway`) loads those anyway and notes what it did: stacks of a
  missing item dropped (and jobs naming it), swings with a missing weapon called off, gone containers' contents dropped; on a new map only the door/window edits that still fit,
  damage only on edges that are still doors or windows, players and zombies on now-blocked tiles moved to free ground,
  flow fields rebuilt as asked for. `World::load` is `load_with` without force, dropping the notes. The
  rest of `WorldConfig` (seed, map size, zombie count, start hour, day length, `ZombieTuning`, `CombatTuning`,
  `TierTuning`, `CarryTuning`, `HealthTuning`, `InfectionTuning`, `NeedsTuning`, `SkillTuning`) is the save's own, stored as RON so a tuning field added later just takes its default.
- **Format** (`save.rs` docs): `ZOMBYSAV`, a `u32` format version (8; 3 added items, inventories and containers, 4 wounds and illness, 5 first aid, 6 needs and sleep and each
stack's `born`/`used`, 7 crafting: craft jobs and the recipes in the content fingerprint, 8 skills: each player's XP; older saves are refused), then sections (4-byte tag, `u32` length,
  postcard payload, FNV-1a checksum): `INFO` (`SaveInfo`: game version, tick, time of day, seed, map size, zombies,
  players, content fingerprint), `CONF` (settings, RON), `MAPD` (edits or the full map), `WRLD` (everything else but
  zombies), one `ZOMB` per 64 m region (the tiers' region grid; zombies column by column, ids as gaps), `RUNS` (each
  player's run stats and cause of death, #13; optional, so it didn't bump the format: a format 4 save without it loads
  with empty records), `MODS` (what mod scripts keep, #25, see Mods and scripts; optional, only for a world with mods), `FARC` (the far clock's lead in ticks, #57; optional, written only when not 0, checked by
  `save_check::check_far_clock`), `LOOK` (what bodies that rise wore, #63: `health::RiseLooks`, item ids by player and zombie id;
  optional too, checked by `save_check::check_looks`), `END`. PvP (#65) didn't bump the format either: the run stats
  gained `player_kills`, so they are written as `RUN2` (a `RUNS` section from before still loads, with none), and
  players reeling or down are an optional `KNOC` section (ids and `Knock`s, only when there are any; checked on load).
  Logged-out survivors and spawn protection (#68) are an optional `AWAY` section the same way (see Multiplayer),
  and tied hands (`BOND`) and groups (`GRPS`, #73) two more (see Hold-ups and groups).
  Per-region zombie sections are there so a later streaming save (#3, #22) can rewrite only what changed. Errors are
  `LoadError` values, never panics: `NotASave`, `Version` (older or newer), `Truncated`, `Corrupt` (bad checksum,
  undecodable or inconsistent data), `ContentChanged`, `MapChanged`, `Invalid` (below). No SQLite: a world is one file
  and its metadata is the info section.
- **Validation** (`save_check::validate`, `check_settings`, `zombie_region`): a save with good checksums can still be
  crafted, so values are checked before the world is used, and anything a tick couldn't have made is
  `LoadError::Invalid` naming the field: non-finite numbers (and, in the settings, ones over 1e6), positions off the
  map, zombie speeds or velocities over 50 m/s, zero, repeated, out-of-order or too-new ids, indices out of range
  (weapons, items in every stack, recipes in queued crafts, upper-floor rooms, spawn building, hunted players, zombie regions, edge sides), damage on an edge with no
  hit points, negative HP, a living player or zombie with no HP, a dead one with HP, an action, jobs or an open container, an empty stack or a slot holding more than one, a lunge or
  strike off the active tier, a map edit that makes a door or window of something else. The tests mutate a valid world
  and save it (every checksum right) for each case, forge sections, and fuzz section bytes with fixed-up checksums
  (each load fails cleanly or gives a world that steps).
- **Size and speed** (M-series Mac, release, `--save-bench`): the default map with ~2,950 zombies is about 267 KB, saved
  in ~1-4 ms, loaded in ~50 ms; the 1 km map with 22,500 zombies (21,500 + a 1,000 horde on a player) is 2.0 MB
  (~87 bytes a zombie), saved in ~14 ms and loaded in ~200 ms, nearly all of that regenerating the map.

## Multiplayer (`net::wire`, `net::host`, `net::remote`; #21)

Single-player still runs the world in the client's own process (`LocalServer`). Multiplayer is the same intents and
snapshots over the network: `zombye-server --host` serves a world and `zombye --connect HOST[:PORT]` plays on it.

- **Transport:** one TCP connection per client (`TCP_NODELAY`), frames of a `u32` length and postcard bytes
  (`wire::encode_frame` / `read_frame`; a client's frame is at most 4 KB, a server's 8 MB). TCP needs only the standard
  library and its ordered, reliable delivery lets every snapshot be a delta with no acknowledgements; the cost is
  head-of-line blocking on a lossy link. The messages (`wire::ClientMsg`: `Hello`, `Intent`, `Ping`, `Bye`;
  `ServerMsg`: `Welcome`, `Refused`, `Snapshot`, `Pong`, `Bye`) don't care what carries them, so UDP or QUIC can go
  behind `host::Host` and `remote::Remote` later. `wire::PROTOCOL` is bumped whenever a message changes shape; a
  server refuses any other. Nothing is encrypted and the join password goes in the clear.
- **Joining:** `Hello { protocol, name, password }`, answered by `Welcome` (the player id, the name, the server's
  `Rules`: name, `pvp`, `max_players`, `allow_dev`, the logout and spawn protection times; the tunings a client predicts and labels with) or `Refused` with the
  reason (another protocol, wrong password, full, the name already playing). **The map isn't sent:** the client builds
  it from the server's `GenConfig` with its own game data (`remote::build_map`, as loading a save does), checks its
  content fingerprint (`save::ContentPrint`: templates and street props, weapons, items, recipes) and the map hash
  (`World::map_base_hash`) against the server's, and applies the doors and windows changed since
  (`World::map_edits_now`) and every cracked one. Other game data, or another version's generator: refused, saying what
  differs.
- **Who is who:** a name (at most 24 letters; none given: `survivor-N`). A name whose survivor nobody is playing takes
  that survivor up again (in the world or out of it, see Logging out), dead or alive: back to a dead one, its player
  sees how it ended and starts a new one with R. With `--save PATH` the names are kept in `PATH.players` (`id<TAB>name` a
  line), so survivors are their players' again after a restart. Names aren't accounts: anyone who knows a name that
  isn't playing can take its survivor (accounts are with #69).
- **Clients only act as themselves:** every intent's `player` is overwritten with the sender's (`Intent::player_mut`);
  `Intent::dev_only` ones (the dev toggles, `SpawnItem`, `SetGoal`, `SpawnZombies`, `SkipTime`, `MakeNoise`) are
  dropped unless `--allow-dev` (`HostStats::refused_dev`). `Respawn` (R on the death screen: "a new survivor") gets a
  dead player up as a new survivor at a new start, in the same world. F5, F9 and the autosave do nothing on a server
  (the yellow line says the world is the server's).
- **Rules:** `--pvp on|off` (default on, docs/PLAN.md §4.8; `CombatTuning::pvp`, saved with the world's settings, off
  in single-player): on, swings, shoves and stomps hit other players as they hit zombies, in one target list (#65, see
  Player against player under Combat: the weapon's damage times `pvp_damage_mult` (0.4) through
  `World::hurt_player_by`, a wound on a body part, stagger and knockdown, a `PlayerHurt` or `PlayerShoved` combat
  event, and a death recorded as `DeathCause::Survivor { by, weapon, stomp, part, wound }`: "Killed by a survivor with
  a crowbar: a gash to the torso"). `--spawn spread|together`: where new
  survivors start, by default spread round the edge of the town with PvP on and together outside the house a single
  player wakes up in with it off. `--max-players N` (default 32, at most 64), `--name`, `--password`, `--budget KB`,
  `--logout-secs S`, `--logout-safe-secs S`, `--spawn-protection S` (below), `--fair-sight on|off` (other players only
  when your survivor sees or hears them, #66, below; on with PvP unless said otherwise).
- **The rules a server sends, and the rest of them (#69; `wire::Rules`, `Host::rules`):** every client that joins gets
  the server's rules in `Welcome::rules`: name, `max_players`, `pvp`, `fair_sight`, `allow_dev`, the logout times and
  spawn protection, and since #69 `camera`, `agents`, `spread_spawns`, `start_kit` (item names and counts), `map_m`
  and `zombies` (living, now). `Rules::lines()` says them in words, a label and a sentence each: the one text for the
  server's start-up lines, the client's stdout on joining, its rules panel and `--server-info`. **`--camera
  isometric|first-person`** (`wire::Camera`, docs/PLAN.md §5; default isometric): a rule clients keep, since a camera
  is the client's. No client has first person yet (#71), so the game client refuses to join a first-person server
  (and says why) and the server says so when it starts; headless clients still join. **`--agents off|on|marked`**
  (`wire::Agents`: `Refused`, `Allowed`, `Marked`; default marked): a client says in its hello whether a program plays
  it (`Hello::agent`; `Remote::connect_as`; the bots always, the game client with `--agent` or `--autoplay`). Off, such
  a hello is refused ("this server's rules don't allow agents"). Marked, the host sets `PlayerView::agent` on the
  survivors programs play (`Host::is_agent`; a bit of `PlayerPacked::bits`), so whoever sees one sees what it is, and
  only then (fair sight still decides who is seen); on, nobody is told. It is the client's own word: a server can't
  tell a program that doesn't say so. Nothing of it is in the sim, a save or single-player. **`--start-kit
  ITEM[:N],...|none`** is the start kit from the command line or the config (`start_kit: [(item: "rag", count: 2)]`)
  instead of the data folder's `start_kit.ron`, held to the same checks (`logout::check_start_kit`). **Asking without
  joining:** `ClientMsg::Query` in place of a hello is answered with `ServerMsg::Info` (`wire::ServerInfo`: protocol,
  game version, the rules, players on, whether a password is needed) and the connection is closed: no password, no
  survivor, not counted as a join (`HostStats::queries`); `remote::query(addr, timeout)`, and **`zombye --server-info
  HOST[:PORT]`** prints it and exits (no window). It is what the server browser asks (below). **Client**
  (`rules_ui.rs`): on a server a panel at the top right lists the rules for the first 20 s after joining, and F6 hides
  or shows it any time; a marked agent's ring is violet where a person's is teal (`survivor_model::mark_agents`), the
  chat log says "An agent says: ...", and the stats line counts them ("3 other survivors in view (2 agents)"). The
  admin `settings` tool shows and changes `agents` and `camera` (they hold at the door: nobody already on is turned
  out). Tests: `net/tests/rules.rs`, `config.rs`'s `the_rules_are_read_from_the_file`.
- **New survivors (#68; `Host::new_spawn`, `outfit_new`):** spread, the places are 64 points round the town's bounds,
  6 m out, each a golden angle on from the last and moved to the nearest open ground outdoors (`Host::spawn_point(k)`);
  a new survivor (a first join, or R after dying) gets the first, counting on from the last one used, with no living
  player within 40 m and no living zombie within 15 m, else (more players than the edge has room for) the one furthest
  from every player. It starts with the **start kit**, `assets/data/start_kit.ron` (`(items: [(item: "water_bottle"),
  (item: "rag", count: 2), ...])`: item ids and counts, at most 12 entries, checked against `items.ron` by
  `logout::parse_start_kit`, loaded by `net::content::load_start_kit` into `HostOptions::kit`; a data folder without
  the file: nothing; shipped: a bottle of water, a chocolate bar, two rags, matches), put straight into its pockets
  (`World::give_kit`), and with PvP on **spawn protection**: `--spawn-protection S` seconds (default 10, 0 none;
  `World::protect_player`, `Player::protected`) in which other players' swings, shoves and stomps pass it by
  (`combat::land` leaves it out of the targets; zombies are not held off), ended at once by starting a swing, shove or
  stomp of its own (`begin_action`). `PlayerView::protected` says so, and the HUD shows a cyan PROTECTED tag.
  **Single-player uses none of it**: you wake in the bedroom with empty pockets, as before.
- **Logging out (#68; `sim::logout`):** a client that leaves or is dropped leaves its survivor in the world
  (`World::player_logout`, `Player::away`): lying asleep where it was (`PlayerView::asleep`), taking no input,
  defenceless, for `--logout-secs S` (default 30), so leaving can't end a fight: zombies see, hear and strike it,
  players can hit, kill and loot it. Left somewhere safe (`World::logout_is_safe`: no living zombie within 25 m or
  after it, no other playing survivor within 40 m, not reeling from a blow) it is `--logout-safe-secs S` (default 5).
  When the time runs out it is taken out of the world whole (`World::parked`, in id order; `World::left_world` names
  it that tick): zombies lose it, snapshots stop listing it. Its player back (`World::player_return`, from
  `Host::hello`): one still lying there gets up, one out of the world is put back where it lay with everything it had.
  Killed while lying there it is a dead player like any other (its things on the floor, the killer in its cause of
  death). A lying survivor doesn't count for the everyone-asleep time scale. A server started on a world with
  survivors nobody plays logs them out (`Host::log_out_unplayed`). **Saves:** survivors out of the world are stored
  among the players in `WRLD` (so every check and content change reaches them) and an optional `AWAY` section says
  which they are, the seconds left to those still lying in the world and spawn protection left (no format bump; in
  `state_hash` only while there is any, so other worlds hash as before). A joining client's first look
  (`Remote::wait_snapshot`) keeps what the snapshots it skips said once only (its private view: the kit in its pockets).
- **Input, one a tick** (`Host::apply_queue`): a client's intents wait in its queue in the order sent, and a tick
  applies them up to and including one `PlayerInput`, so each input moves the player for exactly one tick, as the
  client predicted, however the network bunched them; with none waiting the last one holds; more than 10 waiting (a
  stall) skips to the newest 2. Prediction and reconciliation are single-player's (`player.rs`): the snapshot's
  `last_input_seq` says which inputs to replay. `a_remote_client_joins_and_predicts_exactly` holds the client's
  prediction to the server's state tick for tick over a real socket.
- **Snapshots per client: interest, deltas, a budget** (#22, `Host::send_snapshots`): one `Snapshot::capture` a tick,
  cut down for each client by `Snapshot::for_client(player, radius)`: its own private parts, and the zombies, players,
  ground items and hits within `TierTuning::interest_radius` (80 m) of its survivor. Then a **delta** against what
  that client has (`wire::DeltaEncoder`, rebuilt by the client's `DeltaDecoder`): ground items new, changed and gone,
  and the floor, outfits and run only when they change (the run about once a second).
  - **Zombies are carried on between updates** (dead reckoning, `wire::Tracked`): both ends keep each zombie the client
    has at a position and a speed in fixed point and move it on every tick (integers only, so the same on every
    machine); the server compares that with the truth and sends a correction (`ZombieMove`, about 4 bytes: an id gap,
    how far off, the facing; half of what it says about the speed is taken up) only once the zombie is further off
    than its distance from the client's survivor allows: 1/400 of the distance (2.5 cm at 10 m, 20 cm at 80 m), never
    under 3/4 of a 1/64 m step, and likewise 4° to 40° of facing. A new zombie, or one whose animation changed, goes
    whole (`ZombiePacked`, about 11 bytes, standing). So a crowd costs what its changes of pace and direction cost.
    New ones are taken up 6 m inside the edge of the view and kept to the edge, so one walking along it doesn't come
    and go.
  - **A budget per client** (`HostOptions::budget`, `--budget KB`, default 32 KB/s, 8 to 1000): a bucket filled at
    that rate, up to a second's worth, and spent by every frame. Zombie updates get what is left of it each tick (never
    under a quarter of a tick's share): the due ones go most pressing first (how many times over its allowance a
    zombie is; a new or changed one counts as half a metre off) and the rest wait (`EncodeStats::held`), carried on
    meanwhile. So a horde bigger than the budget gets coarser with distance instead of the connection falling behind.
  - **Other players** go packed (`PlayerPacked`, about 14 bytes: position, facing, stance, action and its timing, HP
    rounded up, weapon, the health tags, reeling or knocked down) and only when they change; a client's own player goes whole every tick
    (prediction needs it exact). A client doesn't get others' endurance, load, input numbers or dev toggles. The
    visibility rules (only players you could see or hear) are the next point.
- **Fair information (#66, `sim::awareness`, `net::visibility`; docs/PLAN.md §4.8):** a client learns about another
  player only through its own survivor's senses, so a modified client is no wallhack. The world keeps, for every pair
  of players, whether one **sees** the other (`World::player_sees(viewer, target)`; `World::awareness()`), by the rule
  zombies see players by (`senses::eyes_range`: view cone, daylight, the other's stance, whether it moves, its Stealth
  `sight_mult`; then `vision::line_of_sight`, so windows don't block it and walls and shut doors do) with a survivor's
  eyes: `PlayerSightTuning` (`WorldConfig::player_sight`, ours; not saved, a server's rule), which follows the client's
  fog of war so that what your screen shows lit is where you'd see someone: 70 m by day, 10 m at night, full range
  within 105° of the facing, half out to 125°, behind that only 2.5 m (never through a wall); sneaking x0.6, running
  x1.2, standing still x0.85; someone holding a light is seen 35 m off at night, and whoever stands in your own
  flashlight's beam (33° either side, its radius + 2 m) is seen; asleep you see nobody. Sight **lasts 1 s after it is
  lost** (`grace_secs`), so nobody flickers at the edge. A player that isn't seen but makes a noise in earshot (the
  zombies' hearing rule: the sound's radius, `wall_muffle` of it with no line of sight) is **heard**: a
  `sim::Heard { listener, pos, radius }`, at most one every 0.4 s per player heard (a louder one at once), kept 4 s
  (`Awareness::heard_lately`). Updated at the end of every tick with two or more players (`World::step_awareness`;
  nothing with one), from the players and the tick's sounds; **derived**: not saved, not in `state_hash`, read by
  nothing in the simulation (`awareness_leaves_the_simulation_alone`). `Snapshot::through_senses(player, world)` then
  cuts a client's view: other players and their outfits only while seen; `Snapshot::heard` (its own; on the wire
  `SnapshotDelta::heard`, position and radius) for those only heard; hits that name a player it doesn't see are left
  out, whoever was hit; ground items only with a line of sight from its survivor (any direction) or in its reach, so
  something dropped or taken behind a wall says nothing until you look. Zombies aren't cut (the client's fog hides
  them), nor doors and windows (the map copy must be right wherever you can walk). **A server's rule:**
  `--fair-sight on|off` (`HostOptions::fair_sight`, `Rules::fair_sight`), by default on with PvP and off in co-op.
  Single-player with PvP on (`--pvp`, `--duel`, a save of such a world) does the same (`Snapshot::for_local`; the duel
  bot goes by `ServerLink::everyone`), and a world that hasn't ticked yet (a loaded save, an offscreen picture) goes by
  the rule alone (`World::sees_now`). **Client** (`heard.rs`): a heard player is a pale ring on the ground where the
  sound came from, spreading and fading over 1.5 s, wider the louder, and a footstep sound there (`audio.rs`); the
  client's fog already hid other players out of sight, so honest pictures don't change. `review_sight.rs` (ignored)
  writes saves to look at it with: `sight_seen`, `sight_behind`, `sight_wall`, `sight_dark`, and `sight_walker` to
  watch live (`--load`): someone walks up behind you, heard then seen. Tests: `sim::awareness::tests`,
  `net/tests/visibility.rs` (over a socket: behind a wall, outside the cone, in the dark, in plain view). Cost: nothing
  measurable (32 clients: the tick as before).
  - **Map changes by chunk** (`wire::MapInterest`): a door or window opening, shutting, breaking or cracking goes to
    the clients whose survivor is within the interest radius of its 16 m chunk. For the others the edge is noted, and
    when its chunk comes into their interest they get it as it is by then, once (the client's map copy is right
    wherever it can walk or see). A joining client still gets every change so far in `Welcome`.
  - The server's report line (every 10 s) gives KB/s per client, the busiest, the fullest view and the updates held
    back a tick; the client's stats line `down N KB/s`.
- **The client** (`server_link.rs`): `ServerLink` holds a `LocalServer` or a `RemoteLink`. A remote one sends the
  tick's intents in one write, and takes up the snapshots that have arrived one a client tick from a short queue (one
  left waiting as a cushion against jitter, two a tick when more have piled up), so rendering interpolates between
  snapshots a tick apart as in single-player; with none it stands still. The stats line says `online: NAME on SERVER
  (pvp on) | N other survivors in view | ping N ms` (the ping includes the wait for the server's next tick), or
  `DISCONNECTED` and why. `--render-to` with `--connect` draws the first snapshot.
- **Threads** (`host.rs`): each connection has a thread reading frames into one channel and a thread writing them from
  a queue of 100 (5 s of snapshots); a client whose queue fills, that floods (400 intents waiting) or that says nothing
  for 20 s (clients ping every second) is dropped. The tick itself never waits on a socket.
- **Melee is lag-compensated** (#23, see Combat): clients say which snapshot they show, blows land where the attacker
  saw the target (at most 0.35 s back), trades of blows go by what each side saw. `wire::PROTOCOL` 13 (12: mods
  downloaded on joining, #24; 7: #68's spawn
  protection and logout times; 8: chat and gestures, #67; 9: the rules of #69, agents, the query; 10: hold-ups and
  groups, #73; 11: the mods in a query's answer, #27; 13: guns, #72).
- **Bots** (`zombye-server --bots N --connect HOST[:PORT] [--secs S] [--bots-hostile] [--lag MS]`): N headless clients, each its
  own connection, that wander, turn when stuck, pick up what lies at their feet, fight zombies that come close (and
  players, hostile) and start again when dead; every 5 s it prints what each costs.
- **Cost** (loopback on a 32-core VM; `thirty_two_clients_share_a_world_within_budget`): 32 clients spread through
  the default town, about 2,900 zombies of which 1,800-2,300 are active and up to 380 in a client's view: a tick 4-6 ms
  (budget 50), about 5 KB/s down per client (the busiest 6.5; 22-26 before #22) against a budget of 32 KB/s, nothing
  held back, and 0.5 KB/s up. The worst case, all 32 in one place with a horde of 1,500 in everyone's view
  (`thirty_two_clients_in_one_horde_keep_to_their_budget`): about 25 KB/s each with nothing held back; with the budget
  set to 20 KB/s, exactly that, every client still with the whole horde and the zombies within 6 m at most 10 cm off.
  One client with 1,500 zombies in view on 10 KB/s of zombie updates: within 8 m at most 5 cm off, beyond at most
  0.3 m (`a_horde_over_budget_gets_coarser_with_distance`). Not yet measured between machines over the internet.

### The dedicated server: one config file, and Docker (`crates/server/src/config.rs`, `Dockerfile`; #20)

For operators the guide is **[docs/self-hosting.md](docs/self-hosting.md)** (Docker, Compose, the settings table,
the admin endpoint in a container). Change a setting or the image, change it there too.

- **`zombye-server --config FILE`** is `--host` with its settings from one RON file (`config::ServerConfig`,
  unknown fields refused, every field optional, options written bare: `pvp: true`): `name`, `listen`, `max_players`,
  `password`, `pvp`, `spawn` (`Spread` / `Together`), `fair_sight`, `logout_secs`, `logout_safe_secs`,
  `spawn_protection_secs`, `start_kit`, `camera` (`Isometric` / `FirstPerson`), `agents` (`Refused` / `Allowed` /
  `Marked`), `budget_kb`, `allow_dev`, `save` (default `world.zsave`; `None`: never saved),
  `autosave_mins`, `seed`, `map_chunks`, `zombies`, `load_anyway`, `data`, `admin: (listen, token, origins)`.
  **The file is turned into the flags it stands for** (`ServerConfig::flags`) and those go behind the command line
  (`config::merge`), so every value is parsed and checked where the flags are (`HostArgs::parse`, `main`), **a flag on
  the command line wins over the file**, and a new `--host` flag needs only a field, a line in `flags` and a line in the
  template to be in the file too. Paths are relative to the file's folder. `ZOMBYE_ADMIN_TOKEN` wins over the file's
  admin token. A file that isn't there is written with the defaults, commented (`config::TEMPLATE`,
  `crates/server/server.example.ron`; a test holds it to the defaults and to naming every field), and the server
  starts; a mistake in one is exit 2 naming the file, before any world is made or loaded.
- **Where the data is** (`net::content::installed_data_dir`, the server's default for `--data`): `$ZOMBYE_DATA`, else
  the checkout's `assets/data` while it exists, else `data` (or `assets/data`) beside the binary. The client still
  uses the checkout's (`default_data_dir`) or `--data`.
- **The image** (`Dockerfile`, `.dockerignore`; never published from here): a `rust:1-bookworm` stage builds
  `-p zombye-server` only (`--locked`; it needs the whole workspace's manifests and `assets/data`, some of which is
  compiled in), and a `debian:bookworm-slim` stage holds the binary, the data in `/usr/share/zombye/data`
  (`ZOMBYE_DATA`) and the licences; it runs as uid 10001 in the volume `/data` with `--config /data/server.ron`, port
  7777. `docker stop` is SIGTERM: `net::shutdown` saves the world and `world.zsave.players`. No health check yet (the
  wire has no query that doesn't join).
- Tests: `config.rs`'s own, and `crates/server/tests/dedicated.rs` (the binary as the image runs it: a config in a
  folder of its own, the data through `ZOMBYE_DATA`, another working folder; a client joins with the password, the
  server is stopped with SIGTERM and started again on the saved world; an empty folder becomes a server; mistakes
  stop it). **The image itself is not built by any test.**

### The server browser (`net::browser`, `crates/client/src/browser_ui.rs`; #27)

F7 in the game lists servers, featured ones first, each with its players, ping, PvP or co-op, whether it wants a
password and its mods, and joins the one you pick. **There is no hosted service:** the game asks nothing of ours.

- **The list is a file anyone can keep** (`browser::parse_list`, RON): `(servers: [(address: "host[:port]", name:
  "...", featured: true, note: "..."), ...])`. Only `address` is needed; `name` shows until the server answers with
  its own; `note` is a line from the list's keeper; `featured` puts it in the section at the top. At most 200
  servers and 256 KB; a server named twice is one row; names and notes are made one line and cut (48 / 120
  characters); a bad address is an error naming the entry. Fields this version doesn't know are ignored (unlike the
  game's other data), so a list can grow fields without breaking older games.
- **Where it comes from** (`browser_ui::list_source`): `--server-list FILE|http://HOST[:PORT]/PATH`, else
  `server_list` in the client's `settings.ron`, else the data folder's `servers.ron` (`assets/data/servers.ron`,
  **shipped empty**, with the format in its comments). A URL is fetched with a plain HTTP/1.0 GET on the standard
  library (`browser::Source`: one request, no redirects, 5 s); **`https://` is refused with the reason**: the game has
  no TLS, and it would be its first dependency for it. A list that can't be read is said in the panel and isn't fatal.
- **Asking** (`browser::Browser`): the list is read on a thread, then every server is asked with the query that
  doesn't join (#69, `remote::probe`), up to 8 at once on worker threads, 5 s each; answers come back over a channel
  and `Browser::poll` takes them up (the client polls once a frame, and redraws the panel only when something
  changed). **Ping** is how long the TCP connection took to open: one trip there and back, without the name lookup
  or the server's wait for its next tick. **Mods:** a server started with `--mod` tells them in its answer
  (`wire::ServerInfo::mods`, `ModInfo { id, name, version, hash, size }`, from `HostOptions::mods`, which `zombye-server` fills
  from the mods it loaded; `--server-info` prints them too). What a server says is someone else's text: it is made
  one line and cut before it is shown (`clean_info`). Rows: featured in the list's order, then the rest, those
  that answered first, quickest first, those that didn't last.
- **Can this game join?** (`browser::can_join(info, the hashes of the mods we have, agent)`, before anything is
  left): the same protocol, the isometric camera, agents allowed if a program plays us, and room. **A server's mods
  needn't be here** (#24): they are downloaded on joining, and the details say how much ("Mods: Wind-up alarm clocks
  1.0 (1 to download, 0.8 KB)", `browser::mods_note` against `browser_ui::LoadedMods`: the hashes of our `--mod`
  folders and of the download cache); only a mod too big for a server to hand out blocks a row, unless we have it. A
  server that fails is shown in orange
  with the reason in its details. A wrong password, a name in use and other game data are only found by joining.
- **Your own servers:** type `host[:port]` in the Address field and Enter adds it (kept in `settings.ron`,
  `ClientSettings::servers`; Delete takes one off; the list's can't be taken off). The Name field is the name you
  join under (`ClientSettings::name`, saved on joining; `--name` wins), Password the join password.
- **Joining starts the game again on that server.** The client builds its map and meshes once per world at start-up
  (as R's new run keeps the same map), so a join from the browser shuts the game down as quitting does
  (single-player: autosaved to `autosave.zsave`) and `main` runs the same program again in its place
  (`browser_ui::relaunch_args`: this run's arguments without what chose its world, plus `--connect ADDR --name NAME
  --from-browser single|server`; on Unix an `exec`, so it is the same process; the password goes in the
  `ZOMBYE_PASSWORD` environment variable, never on the command line). **If the server then refuses** (wrong
  password, name in use, other game data, gone), `--from-browser` keeps the game from stopping: it starts in its own
  world instead (`single`: the autosave the join left) with the browser open, that server picked and the reason on
  it (`browser_ui::BrowserStart`). Without `--from-browser`, `--connect` fails as before (stderr, exit 1).
- **Keys** (while it is open every other key and the mouse buttons are held back, as with the chat line; the wheel
  scrolls the list, not the zoom): F7 or Esc shut it, Up / Down or a click pick, Enter or the Join button joins, R
  asks every server again, Tab goes round the fields (Enter in Address adds, in Password joins, Esc leaves a field),
  Delete takes off a server you added. On a server it works the same (hopping to another).
- **For pictures and recordings:** `--browser` starts with it open, `--browser-join N[@SECS]` picks and joins the
  Nth row by itself once the servers have answered (a row that can't be joined shows why and stays).
- **Not yet:** https lists, joining without the restart, a list service of anyone's, LAN
  discovery, favourites and history beyond the servers you add, sorting and filtering in the panel.
- Tests: `net::browser::tests`, `net/tests/browser.rs` (real hosts on loopback: a list file, a list over HTTP and what
  goes wrong with one, players, ping and mods, an offline server, servers added by hand, joining the row picked),
  `browser_ui::tests`, `crates/server/tests/dedicated.rs` `a_modded_server_is_a_row_in_the_browser` (the binary with
  `--mod`).

## Administering a server (MCP admin, `crates/server/src/admin.rs`; #26)

`zombye-server --host ... --admin-mcp [ADDR[:PORT]]` adds an MCP endpoint for whoever runs the server, so their agent
can administer the world people are playing in. It is **not** the game's MCP server (below: that one plays a world of its
own, time paused between calls) and shares no tools with it but a few read-only ones.

- **Access:** the same streamable HTTP transport as `--mcp-http` (`mcp_http::listen`: `POST /mcp`, `GET /health`, the
  same limits), by default on `127.0.0.1:8788`, and **always** behind a bearer token: `ZOMBYE_ADMIN_TOKEN`, else
  `--admin-token T` (at least 16 characters, never printed). With no token the server refuses to start; there is no
  open mode and `--mcp-no-auth` doesn't apply. `--admin-origin ORIGIN` allows a browser origin. No TLS: a reverse
  proxy or a tunnel in front. Game clients can't reach any of it: it is another port, and nothing in `wire` carries it.
- **Between ticks:** connections pass JSON-RPC messages to the server's main thread, which answers at most 4 after
  each tick (stopping for the next tick once they have taken 10 ms: `Admin::tick`, called from `multiplayer::host`),
  with `mcp::answer`, the protocol half of the game's MCP server (`mcp::Service`: its own name, instructions and
  tool list). The world ticks on at 20 Hz whatever the agent does; a call sees and changes it between two ticks, as a
  client's intents do.
- **Tools** (`admin::TOOLS`; titles, `readOnlyHint` / `destructiveHint` annotations; a test calls every one):
  `status` (JSON, also `structuredContent`: uptime, tick, clock, tick ms against the budget, online, survivors,
  zombies by tier, bytes, joins / leaves / refusals, the rules, the save and when it was written, bans, the newest
  log line), `players` (JSON: `online` with id, name, address, seconds on, position and room, HP, dead, kills, KB/s,
  zombies in view, view lag; `offline` survivors; `bans`), `kick {player | name, reason, ban}` (`Host::kick`: the
  client gets "kicked: REASON" and is dropped, its survivor left as when anyone leaves (the logout rules); `ban: true` also refuses the name, and the address
  unless it is loopback, until `unban {name}`: `Host::ban`, checked before anything else in `Host::hello`; bans last
  until the server restarts), `save {name}` (no name: the `--save` file, as the autosave; a name: that file in the
  saves folder; `PATH.players` beside it either way) and `saves`, `settings {...}` (no arguments: shows them as JSON;
  with: `name`, `max_players`, `password` ("" removes it; never shown back or logged), `pvp`, `fair_sight`
  (true / false / "auto"), `allow_dev`, `spawn` (spread / together / auto), `budget_kb`, `autosave_mins`,
  `zombie_respawn`; all checked first, then all applied, at once; `pvp` is the world's and is saved with it, the rest
  are the server's and come from the command line (`zombie_respawn`: from `population.ron`) again after a restart; clients already on
  keep the name and PvP label they joined with), the events `spawn_zombies {count ≤ 500, radius}`, `horde {count ≤
  300, distance 20-150 m, from: compass}` (a clump that far off, and a lasting noise at the target that brings it and
  whatever else hears), `make_noise {radius, lasting}`, `spawn_item {item, count}` (each at `x`, `y` or at a `player` /
  `name`), `skip_time {hours | to_hour}` (at most 24 h; health and needs move on with it) and `teleport {player | name,
  x, y | to_player | to_name}`, `logs {limit, after, contains}`, and the game's read-only `observe` (everyone shown:
  the operator isn't a survivor), `stats`, `inventory`, `health`, `regions`, `zombie`, `items` on the hosted world
  (`mcp::look_defs`, `mcp::world_tool`; they take `name` for a player here too). Not offered: anything that plays or
  steps time, `load` (clients couldn't follow the world being swapped under them), `screenshot` (a render would hold
  the tick up).
- **The log** (`admin::Log`): every line the server prints (joins, leaves with the reason, refusals, deaths with their
  cause under the player's name, autosaves, the load report every 10 s) and every admin call that changed something
  (`admin: kicked bob (player 2): griefing`) is kept, numbered from 1 with its UTC time and tick, the newest 2,000;
  `logs {after: N}` reads on from line N. A hosting server keeps it with or without the endpoint.
- **Determinism:** admin calls are inputs like intents, applied between ticks through the world's own functions
  (`spawn_zombies`, `make_noise`, `spawn_item`, `skip_time`; a teleport sets the player's position, which the next
  snapshot corrects the client's prediction with). Single-player and the sim are untouched.
- Tests: `admin.rs`'s own (a real `Host` with clients on sockets: every tool, the read-only ones leaving the state
  hash alone, kick / ban / unban, settings holding at the door, saves, events, the log) and
  `crates/server/tests/admin_mcp.rs` (the binary as a process: 401 without the token, no start without one, a client
  joined, the world administered and the client kicked over HTTP); `net/tests/multiplayer.rs`
  `an_operator_kicks_and_bans`.

## The agent survival benchmark (`sim::scenario`, `crates/server/src/benchmark.rs`; #29)

Fixed scenarios to compare agents on over MCP, and a results file a run: **[docs/benchmark.md](docs/benchmark.md)**
(how to run it, the rules, the scenarios, the results format, baseline scores). Local only: no leaderboard is hosted.

- **Scenarios are data:** `assets/data/scenarios.ron`, a list of `scenario::ScenarioDef` (`id`, `name`, `brief`, `seed`,
  `map_chunks`, `zombies`, `start_hour`, `start`: `Bedroom` / `Town` / `At(x, y)`, `hold`, `kit`, `hordes`, `noise`,
  `hours`, `points`), validated by `scenario::parse_scenarios` and checked against the items by `check_scenarios`,
  loaded by `net::content::load_scenarios` (no file: none). Shipped: `first_night`, `scavenger`, `last_stand`, `siege`.
  `ScenarioDef::start(content)` makes the world and its survivor (the world's own zombies within 12 m of the start
  taken out), the same every time; `standing(world, player)` reads the run from the world alone (outcome, hours, houses
  searched: buildings with an opened container, kills, the score's three parts), so **nothing of it is in the world, a
  save or the state hash**; `fingerprint(world)` hashes the scenario with the game data and the map (results compare
  only with the same one). **Changing a shipped scenario breaks comparison with earlier results: add a new id.**
- **Score:** `survive x (hours survived / hours) + house x houses + kill x kills` (`scenario::Points`, per scenario).
- **Over MCP:** `zombye-server --mcp --benchmark ID [--agent NAME] [--results DIR]` (or `--mcp-http`) serves the
  scenario's world (`Persist::benchmark`, a `benchmark::Run`): `Options::PLAY_ONLY` whatever the flags, `respawn`
  refused, a read-only `benchmark` tool (listed only then; not in `mcp::TOOLS`) and a paragraph in the instructions.
  The tools step the world through `benchmark::step`, which stops stepping once the survivor is dead or the hours are
  up (a thread-local set round each playing call: `Run::before` / `Run::after`), so a long `step_ticks` ends on the
  run's last tick; **a new tool that steps time must step through it too.** The call that ends the run gets
  `BENCHMARK OVER: ...` added and the result is written (`<scenario>-<agent>-<time>.json`, format
  `zombye-benchmark/1`, default folder `saves/benchmark`); later playing calls are tool errors. Leaving first writes
  `outcome: "unfinished"` (`Run::close`, from `mcp::serve` and `mcp_http::serve`). Not with `--save`.
- **Baselines:** `--benchmark ID|all --baseline idle|brawler|runner` plays headless with a script (`benchmark::baseline`:
  intents only, but it reads the world, so a floor, not a competitor); `--benchmark list` prints the scenarios,
  `--benchmark table` ranks a folder's results per scenario and fingerprint.
- Tests: `sim::scenario::tests`, `benchmark.rs`'s own (a run over the handler ends on time and on death, the same
  calls give the same result, baselines repeat exactly) and `crates/server/tests/benchmark.rs` (the binary: list, a
  baseline, an agent on stdio, an unfinished run).

## Mods and scripts (`sim::script`, `crates/mods`; #25)

Mods add data and behaviour: **[docs/modding.md](docs/modding.md)** is the modder's guide and the script API
reference (keep it current: a new API function or hook gets a row there). `--mod DIR` (client and server, as often
as wanted, in order) loads a mod's folder; `mods/alarm_clock` is the sample.

- **Data:** a mod's `items.ron` (an id the game has replaces that item), `recipes.ron`, `loot.ron` (entries
  added to tables the game has: `zombye_mods::LootAdd`) and `buildings/*.ron` (#24: building templates, read in file
  name order; an id the game has replaces that template, any other is one more for the town generator, so the town
  changes) are merged into the `WorldConfig` by `zombye_mods::apply`
  and checked as the game's own are; errors name the mod and file, and stop the game at start-up. Mod data is content
  like any other: it is in the save's fingerprint, and a multiplayer client plays with its server's mod data, which
  it downloads on joining (next point).
- **Mods downloaded on joining (#24; `net::modpack`, `Mod::pack`):** joining a modded server needs no installing.
  A **pack** (`modpack::ModPack`: id, name, version, description and the data files as text: `items.ron`,
  `recipes.ron`, `loot.ron`, `buildings/NAME.ron`, nothing else: `modpack::data_file`) is a mod's data **without its
  scripts**, which run on the server alone: a client never receives or runs code from a server. A pack is named by
  `ModPack::hash`, SHA-256 over its fields and files with their lengths (our own implementation in `modpack.rs`,
  held to the standard's test vectors: no dependency for one hash). **Server:** `zombye-server --host --mod DIR`
  packs each mod (`HostOptions::with_mods`), says id, name, version, hash and size in its answer to a query
  (`ModInfo`) and prints the hashes at start-up; a pack over `MAX_PACK_BYTES` (2 MB) is listed but not handed out.
  `ClientMsg::Fetch { protocol, password, mods: hashes }` comes instead of a hello, like a query: the host checks
  it between ticks as it would a hello (protocol, the join password, a ban by address) and a thread of its own
  (counted with the open connections) writes one `ServerMsg::Mod` frame a pack, encoded once at start-up, so a slow
  download never holds the tick; or one `Refused` with the reason (`HostStats::mods_sent`, `mod_bytes`; the log says
  `sent 1 mod (0.8 KB) to ADDR`). **Client** (`server_link::server_content`, before `join_remote`): asks the server
  what it runs (`remote::query`), then `modpack::gather` takes each mod, in the server's order, from our own `--mod`
  folders when one packs to the same hash, else from the cache (`modpack::Cache`: `mods/<id>-<hash>.zmod` in the
  saves folder, one file a pack, taken only if its contents hash to its name; a pack's file names never touch the
  disk), else downloads the missing ones over one connection (`remote::fetch_mods`): each must be a proper pack
  (`ModPack::check`) with the id and **the hash asked for**, or the join stops with the reason and nothing is kept.
  Then `zombye_mods::apply_packs` puts them into the game's own data (no scripts: `config.scripts` stays `None`)
  and the join goes on as before, the content fingerprint and map hash still checked against the server's. **Our own
  mods the server doesn't run are left out** while we play there (stdout says so); a join from the browser that fails falls
  back to our own world with our own mods. Stdout says each mod's source (`mod alarm_clock 1.0 (downloaded, 0.8 KB; sha256
  ...)`, `downloaded before`, `your own copy`), and the rules panel gets a Mods line ("Wind-up alarm clocks 1.0
  (downloaded)"). The bots (`--bots`) build no map and need no mods. Tests: `net::modpack::tests`,
  `net/tests/mod_download.rs` (a real host: downloaded, cached, own copy, password, a server that sends something
  else than it named, a script or a path smuggled in), `crates/mods/tests/mods.rs` (a pack gives a client exactly the
  server's content fingerprint and town), `crates/server/tests/dedicated.rs`
  `a_game_without_the_mods_downloads_them_and_joins` (the binary). **Not yet:** mod assets (models, sounds), a
  server switch to turn handing out off, resuming a broken download, mods bigger than 2 MB, downloaded mods in
  single-player.
- **Items with a use:** an item tagged `usable` (used up) or `reusable` (kept) can be used like food
  (`ItemDef::usable`, `player_use` / `do_use`; the inventory menu offers "Use"); what that does is a script's.
- **The hook:** `sim` has no Luau in it. `WorldConfig::scripts` is an optional `Arc<dyn script::ScriptEngine>`;
  every world made from the config (new, loaded, a new run) gets a `ScriptHost` from it (`World::scripts`), and
  **`World::step` runs it last**, so scripts run the same whoever steps the world (the game, a server, the bench,
  MCP). The tick's `ScriptEvent`s (`Used`, `Hit`, `Crafted`, `Died`, `Spawned`; pushed where they
  happen through `World::script_event`, hits taken from `hit_events`) are handed over in order, then the tick hook.
  A world without scripts keeps no events and hashes and saves exactly as before.
- **Scripts change the world only through `World::script_*`** (`script.rs`: noise, items, needs, hit points
  through `hurt_player_by`, a line for the player, spawning zombies, the store, random numbers), each checking what
  it is given. A new one goes there, then into `luau::register_game`, the docs and the tests.
- **What scripts keep** is in the world, not the VM (`script::ScriptState`): each mod's store (`script_get` /
  `script_set`: booleans, finite numbers, strings, lists and records, at most `MAX_STORE` 64 KB a mod), the scripts'
  own random stream (`script_random`, apart from the AI's and combat's) and events not yet handed over. Saved in the
  optional `MODS` section with the mods' ids and script hashes (no format bump; checked on load by
  `ScriptState::check`; a changed, missing or new mod loads with a note), and in `state_hash` once there is any.
  So a loaded world carries on exactly (`a_loaded_world_carries_on_with_its_scripts`).
- **The sandbox** (`crates/mods/src/luau.rs`): one Luau VM a mod with only `string`, `table`, `math` (no
  `random`), `utf8`, `bit32`, `buffer` and the basic functions; no `os`, `io`, `debug`, `require`, `loadstring`,
  `getfenv`/`setfenv`, `collectgarbage`; libraries read-only (`Lua::sandbox`). The world reaches a hook as `game`, a
  userdata valid for that call only (`Lua::scope`). A call may use `BUDGET` (100,000) interrupts (loop iterations
  and function calls: a count, not a clock, so the cut-off is the same everywhere) and a VM `MEMORY` (16 MB). A
  failed hook is reported (`World::script_errors`, stderr) and the tick carries on.
- **Cost** (`crates/mods/tests/review.rs`, ignored: `script_cost`; `alarm_clock_saves` writes saves to look at the
  sample mod with): the default town with its ~2,950 zombies and one player (release, a shared VM, noisy) ticks in
  0.14-0.15 ms with no mods and 0.16-0.22 ms with the sample mod (idle or with a clock ringing) or with a script
  that queries the world and writes its store every tick: about 0.02-0.07 ms a tick for a mod with an `on_tick`, of
  the 50 ms budget, most of it lending the world to the VM for the call. A mod with no `on_tick` costs nothing on a
  tick with no events for it. `--benchmark` refuses `--mod` (results compare runs of the game as shipped).
- **Not yet:** mods in the dedicated server's config file and Docker image (`--mod` on its command line works),
  client-side UI scripts, MCP tools for mods.

## Talking and gestures (`sim::speech`, `chat.rs`; #67)

Survivors talk to whoever is near them, in text (docs/PLAN.md §4.8: how alliances form). Voice chat comes later,
through the same rule of who hears what.

- **Saying a line:** `Intent::Say { player, text, volume }` (`speech::Volume`: `Whisper`, `Talk`, `Shout`),
  `World::player_say`. The text is made one line (`speech::clean_text`: control characters and line breaks to
  spaces, trimmed, at most `max_chars` 120). It is heard by every other living, waking, played survivor within the
  volume's reach of the speaker (`SpeechTuning`, `WorldConfig::speech`, ours; not saved, like `player_sight`:
  whisper 3 m, talk 12 m, shout 40 m) by the zombies' hearing rule (`speech::hears`): the whole reach with a line
  of sight (glass doesn't block it), `ZombieTuning::wall_muffle` (0.4) of it through a wall or a shut door, and
  then **muffled** (talk gets 4.8 m through a wall, a shout 16 m). Who hears is worked out when it is said: one
  `speech::Said { listener, speaker, seen, pos, volume, muffled, text, tick }` per listener, the speaker's own
  included (its client shows the line when the server took it), in `World::speech` until a snapshot takes them
  (`take_speech`; at most 256 kept; not saved, not in `state_hash`). `seen`: the listener saw the speaker as it
  spoke (`World::player_sees`, the fair-information record). One line a player a tick; dead, asleep or logged-out
  survivors neither speak nor hear.
- **Speaking is a noise:** each line is also a player's sound at the speaker (`whisper_noise` 1.5 m, `talk_noise`
  7 m, a little more than walking, `shout_noise` 30 m), heard on the next tick like a door: zombies in earshot come
  to look, a sleeper within 20 m wakes, other players who don't see the speaker get it as a heard sound (#66), and
  a shout (over the migration's `min_radius`) leaves an attractor that draws the far horde over the following
  minutes. A dev-`ignored` player's voice is dropped for zombies with its other sounds.
- **Replication:** `Snapshot::speech` (a client only what its own survivor heard: `for_player`, `for_client`); with
  fair information on (`through_senses`: PvP servers, single-player with `--pvp`) a line whose speaker the listener
  didn't see comes with `speaker` 0: a voice from a place, not a player. `SnapshotDelta::speech` on the wire. A
  multiplayer server takes one line every half second from a client (`host::SAY_EVERY`) and, as for every intent,
  puts the sender's own player on it.
- **Gestures:** `Intent::Gesture { player, gesture }` (`speech::Gesture`: `Wave`, `HandsUp`, `Point`; `None` stops),
  `World::player_gesture`, `Player::gesture` (`#[serde(skip)]`, not saved, not in the hash: a pose, nothing in the
  simulation reads it). A wave and a point last `gesture_secs` (2.5); hands stay up until lowered (the same gesture
  again, another, or `None`). Starting a swing, shove or stomp, being knocked about, falling asleep or dying ends
  one (`step_gestures`), and none starts mid-swing. `PlayerView::gesture` carries it (others' in two bits of
  `PlayerPacked::bits`), so only those who see the survivor see the gesture.
- **Client** (`chat.rs`): Enter opens the chat line at the bottom left (not over the search box, the morning screen
  or the death screen); while it is open every other key is held back (it runs before `inventory_ui::TextEntry`),
  Tab goes round talk, shout, whisper, Enter says it, Esc drops it. What our survivor heard (`ServerLink::speech`)
  is a line in the chat log above it (6 lines, 14 s each; stdout gets `chat: ...`) and a bubble in the world: over
  the speaker when we were told who, else where the voice came from, and the log then says which way ("A muffled
  voice to the north says: ..."; `chat::log_text`, `compass`). Shouts are bigger and pale yellow, whispers small
  and dim, muffled lines grey. Nobody has a name tag: others are "A survivor". At most 8 bubbles (UI text nodes
  placed with `Camera::world_to_viewport`, in UI pixels, kept on screen), one a speaker. V waves, U puts the hands
  up, P points the way you face; `survivor.rs` poses them (`gesture_now`: the free left arm waves and points, both
  go up; eased in and out; carried onto the modular character by the retarget) under any swing. `--pose wave|handsup|point[:U]`
  holds one for pictures. No sound of its own yet (an unseen speaker's line makes the heard-player footstep): #78.
- **For recordings and pictures:** `--say TEXT[@SECS]`, `--shout`, `--whisper` (each as often as wanted) and
  `--gesture wave|hands_up|point[@SECS]` do it by themselves; offscreen (`--render-to --hud`), where nothing ticks,
  the lines show as if just said. `--talk-demo` puts another survivor 4 m east of you on open ground who turns to
  you and, in a 20 s loop, waves and greets, puts its hands up, points, whispers (too far to hear at 4 m) and
  shouts (`chat::TalkDemo`: only the intents a client sends); `--talk-wall` puts it outside the wall of the room
  you wake up in instead (stdout says how far), so its lines come muffled, and with `--pvp` from "a voice". Add
  `--zombies 0` to keep the town from answering the shout.
- **MCP:** `say {player, text, volume}` (says who heard it, unless PvP is on; time doesn't move), `heard {player,
  secs}` (read-only: what that player heard lately, how long ago, how loud, muffled or not, from where, and from
  whom when known: with PvP on only if it saw the speaker), `gesture {player, gesture}` (`none` stops); `observe`
  tags a gesturing player (`WAVING`, `HANDS UP`, `POINTING`) and `stats` has each player's `gesture`.
- Tests: `sim::speech::tests`, `net/tests/chat.rs` (over sockets, three clients: across a room, through a wall, out
  of range; the half-second limit; gestures), `chat.rs`'s own, `mcp::tests::agents_talk_and_gesture`.

## Hold-ups and groups (`sim::bonds`, `sim::groups`, `holdup.rs`; #73)

Ways to make and test trust between players (docs/PLAN.md §4.8). Hands up (the gesture, see Talking and gestures)
is the surrender; this is what can follow it.

- **Restraints are items:** anything tagged `restrains` in `items.ron` ties hands: `rope` (untied again, and falls at
  the feet of whoever it comes off) and `cable_tie` (also `ties_once`: used up, cut off with something tagged `cuts`,
  scissors or a kitchen knife, or snapped by struggling). Cable ties are loot in laundries, garages and hardware shops.
- **Tying** (`Intent::Restrain { player, target }`, `World::player_restrain`; `target: None` is the survivor faced
  within `reach_m` 1.6 m with nothing that stops a hand between, `World::faced_player`): the other must have their
  **hands up**, or, with PvP on, be **knocked down** or lie **asleep** (a logged-out survivor too); never one under
  spawn protection. With PvP off only raised hands will do, so nobody is tied against their will in co-op. It takes
  `tie_secs` (3) and is called off if they lower their hands, get up or get further than `keep_m` (2.2), or if the
  one tying swings, is knocked about or falls asleep; one restraint is used when it is done.
- **Tied** (`Player::restraint`: the item, who tied it, seconds struggled): walks at `speed` 0.6 of normal, no
  running, no climbing (`MoveMods`, set in `step_bonds` after movement like a knock, so prediction stays exact); no
  swings, shoves or stomps, no gestures, no smashing windows, nothing with its things (`queue_job` and `PickUp`
  refuse: "your hands are tied"). It can still talk, and push a door open or shut. Zombies treat it as anyone.
  Dying frees the hands.
- **Getting out:** struggling (`Intent::Struggle { player, on }`): while it is on and the survivor stands still (no
  movement input), awake and on its feet, seconds add up to `escape_secs` (60, rope) or `escape_tie_secs` (90, a cable
  tie), with a `struggle_noise` (4 m) every 2 s that zombies hear. Or someone frees it (`Intent::Free`, `free_secs`
  2.5; a cable tie needs a `cuts` item on the one freeing, which is kept).
- **Searching a tied survivor** (`Intent::Frisk`, `frisk_secs` 2): its whole inventory is then shown to the searcher
  (`Player::frisking`, `PrivateView::frisk`: what it holds, wears and carries; sent again whenever it changes) until
  the two are more than `keep_m` apart, the hands come free, or `Intent::StopHands`. `Intent::TakeFrom { player, from,
  index, item, count }` takes a stack (`take_secs` 0.6 from pockets and bags, `take_worn_secs` 1.5 for something held
  or worn; by index if that is still `item`, else the first stack of it there) into the searcher's pockets and bags,
  or to the floor at its feet when it doesn't fit or would be too heavy; a bag only comes off once what's left fits
  without it. Both are told (`item_notes`: "Took their 2 x tin of beans" / "A survivor took your ...").
- **One job at a time** (`Player::hands`, `bonds::HandsJob`: tie, free, frisk, take): checked when asked and again
  every tick until due (`hands_check`), like the item jobs; starting one drops the item queue, a swing drops it.
  `PlayerView` carries `restrained`, `struggling`, `struggle` (0 to 1, own client only), `hands` (`HandsAct`; others
  see it in three bits of `PlayerPacked::bits`, 11-15 with tied and struggling) and its seconds left and total (own).
- **Groups** (`World::groups`, `groups::Groups`): `Intent::Group { player, op }` with `GroupOp::Invite { target }`
  (`None`: the nearest survivor in sight within `invite_m` 12 m; the group is made when the other accepts; an
  invitation stands `invite_secs` 60), `Accept { from }`, `Leave` (a group of one is no group), `Mark { pos }` and
  `Unmark { marker, near }` (shared markers, at most `max_markers` 8, oldest going first). At most `max_members` 8.
  Members are told each other's **names** (`World::set_player_name`: a server's join names, set by `Host`; none:
  "Survivor N") and the markers in `Snapshot::groups` (`GroupView`; a client only its own; `SnapshotDelta::group`
  when it changes). A member is still only sent while seen (fair information): the name is a label, not a position.
  **Nothing in combat reads a group: damage between members stays on.** Membership is the player's and outlasts a
  death.
- **Saves and determinism:** tied hands are an optional `BOND` section (the item by id, so changed items need no
  remapping; an item that is gone frees the hands with a note) and groups, markers and invitations an optional `GRPS`
  section (checked on load: known players, nobody in two groups, markers on the map); both are in `state_hash` only
  while there are any, so other worlds save and hash as before. A hands job under way and an open search are not
  saved (a load calls them off, as it lowers raised hands). Tunings: `BondTuning` (`WorldConfig::bonds`) and
  `GroupTuning` (`WorldConfig::groups`), ours, not saved (like `speech`).
- **Client** (`holdup.rs`): Y ties the survivor nearest you if free, frees it if tied; L searches (again: stops); X
  stops what your hands are at; J held struggles; N invites or accepts, Shift+N leaves; the middle mouse button puts a
  marker where you point or takes away the one pointed at. A bar over the current-action bar while your hands are at
  work, and while they are tied (how far out you are); the search panel on the right (click a row to take it,
  Alt-click one of a stack); the group's roster top right; a name under each group member in view; each marker a label
  where it is, with its distance, kept on screen. `survivor.rs` poses tied hands behind the back and working hands in
  front (`bonds_now`; `--pose tied`, `--pose tying`). `--holdup-demo`: PvP on, another survivor 1.2 m in front of you
  with a knife, tins and painkillers who keeps its hands up until it has been tied and freed, and accepts invitations;
  you get cable ties and scissors. `--holdup-auto` plays your side too (`holdup::SCRIPT`, 25 s: tie, search, take, it
  struggles, free, group, marker), for clips.
- **MCP:** `restrain {player, target}`, `free {player, target}` and `search_player {player, target}` (each steps until
  the job is over and says how it went; the search lists what the other holds, wears and carries with indices),
  `take_from {player, from, index, count}` (from the one being searched), `struggle {player, max_secs}` (steps until
  out, dead or out of time), `group {player, action: show | invite | accept | leave | mark | unmark, target, x, y,
  marker}` (time doesn't move; replies with the members' names, markers and invitations) and the debug `restrain_now
  {player, item, by}`. `stats` has each player's `restrained` (the item), `struggling`, `hands`, `searching` and
  `group`; `observe` tags `HANDS TIED` and `STRUGGLING`.
- Tests: `mcp::tests::agents_hold_up_search_and_group`, `sim::bonds::tests`, `sim::groups::tests`, `net/tests/holdup.rs` (over sockets: the hold-up from hands up to
  cut loose; a group's names and markers, and that a third player is told none of it), `holdup.rs`'s own.

## Connecting an agent to the game (MCP)

`zombye-server --mcp` speaks MCP over stdio. `.mcp.json` registers it as `zombye` for
Claude Code and other MCP clients. Build it first with `cargo build -p zombye-server --release` (and
`-p zombye-client` too, for `screenshot`).
It takes the common options too (`--zombies N --seed S --map-chunks N`, and `--pvp on`: player against player on in a new world), e.g. a 1 km map with 20,000 zombies, and
`--save PATH` (load-or-create, autosave, save on exit) with `--autosave-mins N` and `--saves DIR`, and for `screenshot`
`--client PATH` and `--screenshot-timeout SECS`. Time is paused
between calls, so an autosave only happens after a call that changed something.

**Over the network (#87, `mcp_http.rs`):** `zombye-server --mcp-http [ADDR[:PORT]]` (default `127.0.0.1:8787`; a bare
port or a bare address works too) is the same server over MCP's streamable HTTP transport, for agents that can't start a
local program (claude.ai connectors, ChatGPT, the Responses API, a hosted server). **One world per process, shared by
every caller:** connections (a thread each, a small HTTP/1.1 server on the standard library, no new dependencies) only
pass JSON-RPC messages to the main thread, which answers them one at a time with `mcp::handle`, the stdio server's
own handler, so time is still paused between calls; it autosaves and saves on Ctrl-C / SIGTERM like `--mcp` and, a
service, ignores stdin. `POST /mcp` with a message (or a batch of up to 50): a request is answered as
`application/json`, a notification with `202`; `initialize` returns an `Mcp-Session-Id` the server never asks for (it
keeps nothing per session); `GET /mcp` is `405` (no server-to-client stream), `DELETE /mcp` `200`, `GET /health` `ok`
with no token. Limits: 1 MB a body, 32 KB of headers, 64 connections, 30 s of silence. **Access:** a bearer token
(`ZOMBYE_MCP_TOKEN`, else `--mcp-token T`; at least 16 characters; compared in constant time, a wrong one answered
250 ms late; never printed), sent as `Authorization: Bearer T`; `--mcp-token-in-url` also takes `/mcp/T` (for apps that
only take a URL; weaker). With no token the server refuses to start unless `--mcp-no-auth`, which is always without the
debug tools. **Debug tools and saves:** with a token every tool is on unless `--mcp-no-dev` (or the stdio server's
`--no-debug-tools`); off (`mcp::Options::PLAY_ONLY`), the debug tools (below) and the saves folder's `save` and `saves`
are left out: not in `tools/list`, and a call is the unknown-tool error. A new tool must be put on one
side or the other (`every_debug_tool_exists` lists the play tools). A browser's `Origin` must be this machine's
(`http://localhost`, `127.0.0.1`, `[::1]`) or named with `--mcp-origin ORIGIN` (`*`: any), else `403`; CORS headers are
sent for allowed origins. No TLS and no OAuth here: HTTPS comes from a reverse proxy or tunnel in front. Tests:
`mcp_http.rs`'s own (the transport, access, a real world played over a socket) and `crates/server/tests/mcp_http.rs`
(the binary as a process: no stdin, token from the environment, a screenshot's image through whole).

With PvP on (`--pvp on`, `pvp {on: true}`) `observe` is fair (#66): it shows `player`'s view (default 1) by the rule a
multiplayer server tells clients by: other players only while that survivor sees them or did a second ago, those it
only hears as `h` on the map and a "heard, not seen" list (where, how far, how loud, how long ago), ground items only
with a line of sight; `all: true` shows everyone (the default with PvP off). The other tools stay debug tools.

**What clients are told about the tools (#88, `mcp.rs`):** the `TOOLS` table gives every tool a `title` and a kind, and
`tool_list` turns the kind into MCP annotations on the definitions in `tool_defs`: `Look` tools are `readOnlyHint: true`
(`observe`, `screenshot`, `stats`, `zombie`, `edge`, `buildings`, `regions`, `inventory`, `health`, `skills`, `weapons`,
`items`, `loot`, `recipes`, `containers_near`, `saves`, `heard`; clients skip the approval prompt for them, and a test calls each
and checks the state hash, the tick and the session's dirty flag don't move), `Play` tools act as a player or step time,
`save` writes a file, and `Debug` tools (`make_noise`, `set_goal`, `spawn_zombies`, `spawn_item`, `set_needs`, `wound`,
`add_player`, `pvp`, `dev`, `skip_time`, `load`, `restrain_now`) get a title starting `Debug:`, a description starting "Debug tool, for setting
up tests" and `_meta: {"zombye/debug": true}`. Only `save` and `load` are `destructiveHint: true`; every tool is
`openWorldHint: false`. **A new tool needs a row in `TOOLS`** (a test fails without one). `--no-debug-tools` leaves the
debug tools out: not listed (46 tools instead of 58), and calling one is the unknown-tool error. `stats` has an
`outputSchema` and returns its JSON as `structuredContent` beside the text (`output_schema`). The server's `instructions`
(`mcp::instructions`, under 2,200 bytes: every client puts them in the model's context) say you are player 1, time only
moves when a tool steps it, look with `observe` first, and the debug tools are for testing; detail belongs in the tools'
descriptions. **Protocol:** two eras side by side (`mcp::handle`). The handshake revisions: `initialize` answers with the
client's version if it is one of 2024-11-05, 2025-03-26, 2025-06-18, 2025-11-25, else the newest of them. The stateless
2026-07-28 revision: `server/discover` returns the supported versions, capabilities, the server's identity and the
instructions, and any request naming that version in `params._meta["io.modelcontextprotocol/protocolVersion"]` is
answered with no handshake before it, with `resultType: "complete"`, the server's identity in `_meta` and cache hints
(`ttlMs`, `cacheScope`) on `tools/list`; a version we don't speak is error -32022 with the list of those we do. An
unknown tool is the JSON-RPC error -32602 (a tool's own failure is still a result with `isError`).

The agent plays player 1, who wakes up in the bedroom. Tools: `screenshot` (below), `observe` (double-resolution ASCII map
with walls `-` `|`, shut windows `=` `!`, open windows `o`, broken windows `x`, shut doors `D` (`d`
when damaged), open doors `/`, broken doors `#`, fences `~`, furniture `f`, street props `^`, containers `c` (not searched yet) and `o` (searched; on a tile, not a wall line), players `P` and zombies by
state: `Z` wander, `I` investigate, `C` chase, `L` lunge, `A` attack, `B` battering a door or window;
`S` staggered, `v` down, `%` corpse; zombies on the cheaper tiers `n` near, `?` virtual; weapons on the floor `w`, other items `i`, dead players `X`; plus players (HP,
weapon, action, load, whether they're climbing or dead), items in view, damaged doors and windows in view
with their HP, and the nearest zombies with HP), `zombie {id}` (one zombie's simulation tier and distance to the nearest player, HP, AI state, target,
memory, timers, the edge it batters, and per player the distance, line of sight and current sight range), `edge {x, y}` (the four edges
of a tile, with HP for doors and windows), `step_ticks` (time only advances when you call it; reports
zombie attacks, thumps and anything broken), `move_player` (player, compass `direction` or `dx`/`dy`,
`ticks`, `run`/`sneak`, `facing`: holds that input while the world steps, then stops), `interact`
(opens/closes the door or window player 1, or `player`, faces; `toggle_door` is an old alias),
`smash_window`, `attack` / `shove` / `stomp` (player, optional `facing` or `target` zombie id to turn
first, or `target_player` a player id; steps until the action has landed and the player can act again, and reports what it hit: zombies under `HITS`, other players (PvP) under `PLAYER HITS: P1 hit P2 for 18: a gash on the left arm, staggered (82 HP left)`),
`pvp {on}` (player against player on or off for this world; no argument says which; `--pvp on` starts with it on; `stats` has `pvp`, each player's `knocked` (`staggered` / `down`) and `run.player_kills`, and `observe` says who is staggered or knocked down),
`pick_up` (the instant grab, as E), `dev {player, god, ignored}` (the dev toggles; a field left out stays as it is;
`stats` gives each player's `god`/`ignored` and `observe` tags them `[GOD]` / `[IGNORED]`), `weapons` (the weapon table and combat numbers), `items {category}` (the item table
and the carry numbers), `skills {player}` (see Skills), `inventory {player}` (slots, worn and carried stacks with indices, load and burden, the open
container, the floor in reach with ground item ids, containers in reach, the job queue), `containers_near {player, radius}`
(id, kind, furniture, room, distance, searched or not and what's in it, or the loot table it will fill from, in reach),
`loot {room, container, n}` (no arguments: every table; with a room type and a container or furniture kind: the table
it fills from and which fallback, entries with chance per pick, groups, and `n` sample fills, default 200, with how
often each item turns up; read-only), `search {player, container}`, `take
{player, from: container|ground, index, count}`, `move_item {player, from, index, to, count}` (places: carried, hand,
off_hand, back, shoulder, worn, container, ground), `drop {player, from, index, count}`, `equip {player, index}`, `wear
{player, index}`, `unequip {player, from, index}`, `use {player, from, index}` (each queues the job and steps the world
until the player's queue is empty, then reports what happened and the inventory; `use` also says beforehand what it will
do, and the needs before and after), `recipes` (every crafting recipe: inputs, tools, outputs, seconds; read-only) and
`craft {player, recipe}` (see Crafting), `sleep {player, max_hours}` and `set_needs {player, hunger, thirst, tiredness}` (see
Needs), `say`, `heard` and `gesture` (see Talking and gestures), `restrain`, `free`, `search_player`, `take_from`,
`struggle`, `group` and the debug `restrain_now` (see Hold-ups and groups),
`respawn`, `add_player {x, y}` (another player joins there, default beside player 1; for screenshots of several
players and testing with them), `spawn_item {item, count,
x, y}` (debug; any item id, `weapon` still accepted), `make_noise {x, y, radius, lasting}`
(debug; a loud one leaves an attractor, `lasting: true` a 30-minute one), `regions` (ASCII heat maps per 64 m region of
living zombies and of their targets, the attractors' pull with players marked, and zones; the attractors with reach,
spread, strength and time left; the regions furthest below and above target), `set_goal` (debug: zombies head for a point, senses off), `spawn_zombies`, `skip_time`,
`buildings`, `save {name}` (to `name` in the saves folder, or with no name to the `--save` file), `load {name, force}`
(replaces the world; with no name, the `--save` file; refused, world untouched, if the file is damaged, holds impossible
values or is of another format; changed game data loads with a note when compatible, else is refused unless
`force: true`, which loads it anyway and notes what it changed), `saves` (the saves folder: size, age, tick, time, seed, map size, zombies,
players; and where this session autosaves), `stats` (players with HP/weapon/load/burden/busy/dead/god/ignored/needs/asleep/tags, `run` (this life: `survived`, `game_hours`, `played`,
`seconds`, `kills`, `distance_m`, `items_taken`, `containers_searched`, `wounds`, `hp_lost`, and once dead `death`: `cause`
id, `text`, `rises`, `tick`, `day`, `time_of_day`) and `cause_of_death` (plain words, null while alive), time of day, daylight and time scale, zombies by state
including staggered/down/dead and by tier (`zombie_tiers`: active/near/virtual, last tick's promotions and
demotions, the ranges), `population` (target vs alive, attractors, whether migration and respawn are on, respawned), items on the floor, containers and how many are opened, tick timing (tier pass, senses, flow, active, near, virtual), thumps, hits and sounds heard last
tick, damaged edges, the state hash). `step_ticks` and `move_player` also report hits, kills, how many
zombie strikes landed for how much, and deaths with their cause (`P1 DIED: Bled out from a gash on the torso`), from
anything, not only strikes; `skip_time` reports deaths during the skip the same way.

**`screenshot`** returns a rendered PNG of the world around a player as MCP image content (`{"type": "image",
"data": <base64>, "mimeType": "image/png"}`) followed by a text line: whose view, where the player stands (room or
outdoors, dead or not), the centre if given, tick, time of day and daylight, zoom, angle, PNG size and render time.
Arguments: `player` (default 1: whose fog of war and cutaways, and the centre), `x` and `y` (centre there instead;
fog is still the player's and only zombies within 80 m of a player are sent, so far from players you see terrain and
buildings, darkened, and no zombies), `zoom` (metres visible top to bottom, 8-90, default 26), `angle` (0-3, the four
fixed camera angles), `width` / `height` (default 960x540, at most 1920x1080). How: the server saves the world to a
temporary file in the system temp folder, runs the game client on it offscreen (`zombye --render-to PNG --load SAVE
--player ... --size ... --data ...`, see Offscreen above; no window opens), waits up to `--screenshot-timeout SECS`
(default 60) and deletes both files. It only reads the world: no ticks, no RNG, the state hash is the same before and
after (and it doesn't mark the world as needing a save). The client binary is `--client PATH`, else `$ZOMBYE_CLIENT`,
else `zombye` next to `zombye-server`: build both (`cargo build --release -p zombye-server -p zombye-client`). A missing
client, a failed render (the client's last log lines are included) and a timeout are tool errors. It takes about
0.4-0.7 s on an M-series Mac (default map or 1 km; the first call after a build can take 1.5-2 s). Use `observe` for exact
positions and states and `screenshot` to check how things look: after changing rendering, meshes or building templates,
or to see a fight, a siege or the fog at night. Each image is large (about 1 MB of base64 at 1280x720, which is why
the default is 960x540, #88), so ask for a bigger `width`/`height` only when the detail is needed.
