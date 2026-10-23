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
| `crates/sim` | Pure simulation: chunked tile map with edge walls, building templates, town generator, line of sight, players (`players.rs`: shared movement + endurance), zombies (`zombies.rs` storage, `senses.rs` sight + hearing, `ai.rs` state machine + `ZombieTuning`, `tiers.rs` active/near/virtual simulation tiers + `TierTuning`), population (`population.rs`: density per zone from data, respawn, migration toward noise), flow fields + cache (`flow.rs`), weapons and ground items (`items.rs`), melee combat and player health (`combat.rs` + `CombatTuning`), saving and loading the world to and from bytes (`save.rs`; content-change and value checks on load in `save_check.rs`). No engine, no I/O. Deterministic. | MIT OR Apache-2.0 |
| `crates/net` | `Intent` (client → server) and `Snapshot` (server → client) types, `LocalServer` (with `join`), content loading (`content.rs`), save files on disk (`save_file.rs`), Ctrl-C/SIGTERM/SIGHUP as a request to save and stop, for the server and the client (`shutdown.rs`). | MIT OR Apache-2.0 |
| `crates/server` | `zombye-server` headless binary: bench mode (`--bench`, with `--players`/`--horde` for the tiers) and the MCP endpoint; `--save` with autosave; the MCP `screenshot` tool runs the client offscreen (`screenshot.rs`). | MIT OR Apache-2.0 |
| `crates/client` | `zombye` Bevy client: camera, chunk streaming, building/tree meshes, fog of war, cutaways, day/night, input + player prediction (`player.rs`), grey-box characters (`rig.rs` bones + boxes; `zombie_render.rs` VAT crowd; `survivor.rs` skinned, CPU-posed players), hit feedback (`fx.rs`), ground items (`items_render.rs`), HUD, `--autoplay` (`autoplay.rs`), quicksave / quickload / autosave (`saving.rs`), offscreen rendering to a PNG with no window (`--render-to`, `offscreen.rs`). | MIT OR Apache-2.0 |
| `assets/` | Shaders, data files (`data/buildings/*.ron` templates, `data/items/weapons.ron`, `data/population.ron`), later generated models/textures/audio. | Code, data and shaders: MIT OR Apache-2.0. Art and audio: CC BY-NC-SA 4.0 ([assets/LICENSE](assets/LICENSE)); each asset listed in [ATTRIBUTION.md](ATTRIBUTION.md) |
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

## Commands

```bash
cargo test -p zombye-sim --release          # sim tests incl. determinism + perf smoke test
cargo run -p zombye-server --release -- --bench --zombies 5000
# Issue #2's target: ~1,000 active + 20,000 virtual on a 1 km map (prints a per-tier breakdown)
cargo run -p zombye-server --release -- --bench --ticks 400 --map-chunks 64 --zombies 21500 --players 1 --horde 1000
# ...and time saving and loading that world (size, ms, and that the hash matches after load and 100 more ticks)
cargo run -p zombye-server --release -- --bench --ticks 200 --map-chunks 64 --zombies 21500 --players 1 --horde 1000 --save-bench
cargo run -p zombye-client --release -- --zombies 1000          # the game window
cargo run -p zombye-client --release -- --zombies 2000 --novsync --screenshot shot.png --exit-after 8
cargo run -p zombye-client --release -- --fight-demo --autoplay --zoom 9 --screenshot fight.png --screenshot-after 2.5,4,6 --exit-after 7
# One picture of a save, rendered offscreen: no window opens, nothing takes focus (about 0.5 s plus loading)
cargo run -p zombye-client --release -- --render-to shot.png --load saves/autosave.zsave --size 1280x720 --angle 1 --zoom 30
# Tests: the offscreen render test needs a GPU; it skips itself if there is none, or set ZOMBYE_SKIP_GPU_TESTS=1
cargo test --release --workspace
```

Client flags: `--zombies N --seed S --time HOUR --at X,Y --map-chunks N --rotate --angle 0..3 --novsync --zoom METRES --screenshot PATH --screenshot-after SECS[,SECS...] --exit-after SECS --fight-demo --siege --autoplay --sneak --saves DIR --load NAME|PATH --load-anyway --player ID --data DIR --quicksave-after SECS --quickload-after SECS --render-to PATH --size WxH --center X,Y`
(`--rotate` turns the camera with `,` and `.`; `--zoom` is the metres of world visible vertically, 8–90, default 26;
several `--screenshot-after` times save `PATH` numbered: `fight-1.png`, `fight-2.png`, ...; keep the last one
a little before `--exit-after` or it's lost).
For verifying combat without a keyboard: `--fight-demo` starts you on open ground near the house you'd wake up in
(unless `--at` says otherwise) with a baseball bat in hand and six zombies 7 m east, called over by a noise;
`--autoplay` makes the local player fight by itself (`autoplay.rs`): it faces the nearest standing zombie and
closes in, swings in reach, alternates shoves and swings when two or more crowd it, stomps a downed zombie at its
feet, and gets up again 4 s after dying. It only sets the normal movement input and sends the normal intents.
`--siege` starts you outside the house you'd wake up in, 10 m out from one of its shut doors (south or east walls
first, so it faces the camera), with a bat and five zombies between you and the door drawn to a noise just inside
it: they batter it (or the window beside it) with their backs to you, and the door or window shows cracks as it
gives (add `--zombies 0` to keep the rest of the town out of it). `--sneak` starts you sneaking (as if C was
pressed). The game window takes keyboard focus, so typing elsewhere while it runs can move or toggle the player.
`--at` starts the player somewhere other than the bedroom and `--time` sets the start hour, which is handy for screenshots.
`--angle N` starts the camera at one of the four fixed angles (0 = the default, each step a quarter turn), `--player ID`
plays as that player of a `--load`ed save (default its first), `--data DIR` reads game data from another folder (as the
server's `--data`; a save only loads against the content it was made with).
**Offscreen (`--render-to PATH`, `offscreen.rs`):** renders one PNG and exits without ever opening a window: no primary
window, no winit (on macOS that would make it a foreground app and take keyboard focus), no audio or gamepads; the
camera draws into an image (`--size WxH`, default 1280x720, at most 3840x2160) that is read back. Usually with `--load`:
the local server doesn't tick and the player isn't predicted (`crate::live` run condition), so it shows the saved state
exactly, from `--player`'s point of view (fog of war, cutaways, night lamp), with `--zoom`, `--angle` and optionally
`--center X,Y` (the camera, and chunk streaming, centred there; fog is still the player's). No HUD (the UI only draws on
a window's camera), no autosave. It waits for the frame to be stable rather than a fixed time: visual time runs at a fixed
1/30 s a frame for 30 frames (eased poses settle), then stops; once the render world has had no pipelines waiting for a
few frames it reads the image back, and two readbacks in a row that match (all but 1 byte in 10,000) are written. It
prints `rendered PATH: WxH, N frames, N readbacks, MS ms` (plus a warning if it never settled; 40 s limit) and exits 0,
or exits 1 with the reason on stderr. Prefer it to `--screenshot` for checking visuals: the windowed client pops up and
steals focus. Not rendered: client-only effects that aren't in the save (blood splats, hit flashes).
`--map-chunks N` makes the map N × N chunks of 16 m (default 20, 320 m; 64 is about 1 km): the same town in the
middle with more forest round it (a map too small for the town is grown to fit). The server takes it too.
Without `--zombies` (client and server) the world holds as many zombies as `assets/data/population.ron` gives for the
map: about 1,000 on the default map, about 1,560 on a 1 km one (the extra ground is sparse forest); `--zombies N`
sets the total and keeps the file's proportions.
Server bench (`zombye-server --bench`): `--ticks N`, `--zombies N`, `--seed S`, `--map-chunks N`; with no `--players`
every zombie heads for a debug goal (all stay active); `--players N` puts players on the road west of town (in the
forest on a big map) running in circles with a 60 m noise every 10 s, `--horde N` spawns N more zombies round the
first, and the report adds tier counts, per-tier ms/tick and a population line (total target, attractors, respawned);
`--save-bench` adds a line timing a save and a load of the world at the end. Saves (server, both modes): `--save PATH`
loads that save if it exists (the world options are then ignored; content must fit, see Saves; `--load-anyway` loads it
when it doesn't) or makes a new world
and saves it there, then autosaves to it every `--autosave-mins N` minutes of real time (default 5, fractions allowed,
0 = off) when something changed, and on shutdown (stdin closed, Ctrl-C, SIGTERM or SIGHUP, on Windows also closing the
console; a second Ctrl-C stops at once, unsaved; bench: at the end of the run).
`--saves DIR` is the folder for named saves (MCP `save`/`load`/`saves`; default the `--save` file's folder, else `saves`).
The client prints one stats line per second to stdout (time, room, stance, hp, endurance, fps, frame ms, zombies
sent to it, chasing, zombies per tier active/near/virtual, sim tick ms).
In game: WASD move, Shift run, C sneak (toggle), mouse to face, left mouse attack (swing your weapon, or
punch), Space shove (or stomp, when a downed zombie lies right in front of you), E pick up the weapon you
face, F open/close the door or window you face, G smash the window you face, R get up again after dying,
T skip an hour, Z spawn zombies, wheel zoom, F5 quicksave, F9 quickload. Walk into an open or broken window to climb
through it. Quitting (closing the window, `--exit-after`, or Ctrl-C, SIGTERM or SIGHUP in the terminal: `net::shutdown`,
not Bevy's handler; a second Ctrl-C quits unsaved) autosaves; `--render-to` never does.

Saves in the client: `quicksave.zsave` (F5) and `autosave.zsave` (on quit) in the saves folder (`--saves DIR`,
default `saves/` in the working directory, git-ignored; point it elsewhere when testing). `--load NAME|PATH` starts
from a save (a name in the saves folder, e.g. `--load autosave`, or a path): its own seed, map and players, so the
world options and `--fight-demo`/`--siege` don't apply; `--load-anyway` loads one whose game data changed in a way it
can't simply take, for `--load` and F9 alike, with no prompt. F9 swaps the quicksave into the running game like a rejoin:
the doors, windows and cracks that differ arrive as edge changes, the player's prediction restarts from the loaded
state (unconfirmed inputs dropped), zombies and items follow from the first snapshot, the life's kill/time tally
starts again at zero from the loaded tick (a save records neither kills nor when a life began; a dead player stays dead),
and blood splats and particles are cleared (`fx::ClearFx`): they belong to the timeline left behind. A quicksave of another map (seed or size) is refused with a message: start with `--load` for that.
`--quicksave-after SECS` / `--quickload-after SECS` press F5 / F9 by themselves, for verification. A yellow line
under the HUD says what was saved or loaded (or why not), and stdout gets the same line.

HUD: top left, the stats line and the keys. Bottom left, the weapon you hold and your stance, a health bar
(pulses below 30), an endurance bar (orange below `tired_below`, when swings weaken; grey-blue and a WINDED tag
when winded: no running), and this life's kills and time alive. Dead: a "You died" screen with the time survived
and kills (both counted client-side from combat events and sim ticks) and "Press R to get up again".

## The world

- 1 tile = 1 m; chunks are 16 × 16 tiles. Each tile owns its **north and west edges** (wall, window,
  door, arch, fence), so every edge is stored once (`sim::map`).
- Edge states (`map::Edge`): `Wall`, `Window` (shut glass: blocks movement, not sight), `WindowOpen`
  and `WindowBroken` (passable, but crossing is a slow climb: `TileMap::in_window_opening`, within
  `CLIMB_ZONE` of the window line), `DoorClosed` (blocks both), `DoorOpen`, `DoorBroken` (a permanent
  opening; doesn't block sight), `Arch`, `Fence` (blocks movement, not sight). Helpers: `is_door`
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
  bottom row is the street-facing front. `BuildingTemplate::validate` explains mistakes. Add a house
  by adding a file.
- `sim::worldgen` lays out a seeded street grid, fills lots with templates (rotated/mirrored), and
  plants forest around the town. The player wakes up in a ground-floor bedroom.

## Players, intents and prediction

- A player is a server entity (`sim::players::Player`): position, facing, stance (Walk 3 m/s, Run 5.5 m/s,
  Sneak 1.4 m/s; at most `CLIMB_SPEED` 0.7 m/s while climbing through a window) and endurance (0..1; running drains it, and once it runs out you can't run until it
  recovers a bit). `players::step_movement` is the one movement function: the server tick and the
  client's prediction both call it, so keep it pure (map + state + input + dt in, state out).
- `Intent`s (`crates/net`): `PlayerInput { player, seq, move_dir, facing, run, sneak }` every tick
  (the server keeps the newest by `seq` and applies it once per tick; `move_dir` is clamped to
  length 1), `Interact { player }` (opens or closes the door or window that player faces, within
  1.6 m of its server position, otherwise ignored; doors make `door_noise`, windows the quieter
  `window_noise`; broken ones can't be shut), `SmashWindow { player }` (breaks the shut or open window
  it faces; loud, `glass_break_noise`), `Attack`, `Shove`, `Stomp`, `PickUp` (all `{ player }`, see
  Combat), `Respawn { player }` (single-player/debug: a dead player gets up at the spawn point), and the
  debug/admin `SetGoal`, `SpawnZombies`, `SpawnItem { weapon, pos }`, `SkipTime`, `MakeNoise`.
- `LocalServer::join` adds your player and returns its id. Snapshots carry `players` with
  `last_input_seq` (plus `hp`, `dead`, `weapon`, `action`, and `action_left` / `ready_in`: seconds until the
  action lands and until the player can act again, which time the swing animation); the client resets its predicted player to
  that state and replays newer inputs. Swings cost endurance on the server only, so prediction is
  corrected by the next snapshot when you swing (movement itself stays exact). Snapshots also carry
  the full list of ground `items` (small for now). On joining the client also gets the weapon
  definitions; snapshots name weapons by index into them.

## Combat (`sim::combat`, `sim::items`)

- **Weapons are data:** `assets/data/items/weapons.ron`, a list of `items::WeaponDef` (damage
  range, reach to the zombie's body, arc, windup and swing time, max targets, knockback,
  stagger/knockdown/crit chances, crit multiplier, endurance cost, noise, spawn weight, `found_in`
  room types, grey-box length and colour), validated by `items::parse_weapons` and loaded by
  `net::content::load_weapons` into `WorldConfig::weapons`. Exactly one is `Unarmed` (bare hands);
  if the content has none, `WeaponDef::fists()` is added. Add a weapon by adding an entry.
- **Ground items:** `World::items()` (`GroundItem { id, weapon, pos, angle }`). The world places 0–2
  weapons per house on free ground-floor tiles (at least one in the spawn house), weighted by
  `spawn_weight` and ×3 in `found_in` rooms, from its own seeded RNG. `PickUp` takes the item within
  `PICKUP_REACH` (1.3 m) nearest to where the player faces and equips it; what you held is left in its
  place. A player who dies drops their weapon.
- **Actions** (`players::ActionKind`: `Swing`, `Shove`, `Stomp`): start when the intent arrives (or are
  queued if it comes within `action_buffer_secs` of the last one ending), cost endurance at once, and
  land after a windup. At impact the server finds zombies in reach and arc of the player's facing with
  a clear walking line (glass and fences block), nearest first up to `max_targets`, each extra target
  taking `multi_hit_falloff` less. Damage, knockback (a velocity, capped at `MAX_KNOCKBACK_SPEED` so it
  can't push through walls), then seeded rolls (`World::combat_rng`): knockdown, else stagger. From
  behind (the zombie faces away) crits are more likely and knockdowns easier; knockdowns are easier
  on a staggered zombie. Below `tired_below` endurance actions get weaker and slower; running out
  winds you. Each action makes a noise heard that tick. **Shove:** short reach, wide arc, up to 3, no
  damage, always staggers, may knock down (`shove_knockdown_chance` 0.25: about one shove in four floors a lone
  zombie facing you, 0.4 from behind). **Stomp:** hits a downed zombie at your feet in front.
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
  (1.1 s) of being down show `ZombieAnim::GetUp` (still `Down` to the AI, so still stompable). `Dead` zombies are
  corpses: left out of the spatial hash, the AI, senses, separation and swings; their `timer` counts up from death,
  and at the end of each tick the world removes those older than `ZombieTuning::corpse_secs` (900) and the
  oldest beyond `max_corpses` (300), keeping the columns in id order (`Zombies::remove_where`; ids are never
  reused; `World::removed_corpses` lists them). Snapshots just stop listing them and the client despawns them.
  `ZombieAnim`: `Idle`, `Walk`,
  `Attack`, `Stagger`, `Down`, `Dead`, `GetUp`, `Thump` (battering a door or window).
- **Player health:** `hp` (100). Each `ZombieAttack` lands with `zombie_hit_chance`, plus
  `zombie_crowd_bonus` per extra zombie attacking that player, and does `zombie_damage`
  (`landed`/`damage` on the event). At 0 the player is dead: no movement or actions, zombies lose them.
- Every number not in the weapon file is in `CombatTuning` (`WorldConfig::combat`, serde defaults). Ours.

## Characters on screen (`crates/client`)

- Both are grey-box boxes on a small skeleton (`rig.rs`: bones as parent + rest pivot, a pose is a rotation
  per bone plus a root offset; positive X swings a hanging limb backward, positive Z toward the model's left).
- **Zombies** (`zombie_render.rs`, `zombie_vat.wesl`): 7 bones, poses baked into a vertex animation texture,
  one instanced batch for the whole horde. The VAT has more clips than `ZombieAnim`: Idle, Walk (rate from speed
  / stride), Attack, Thump, Stagger, Fall then a Down loop, GetUp, and three deaths (backward, forward when hit
  from behind, and limp when killed on the ground). The client picks the clip and, for one-shots, its position
  (from when the anim changed), so one-shots start at frame 0 and stop on their last frame; clips are authored to
  start and end near each other's poses (`clips_join_up` test). Switches between the looping clips (Idle, Walk,
  Attack, Thump: clips 0-3) crossfade over 0.2 s in the shader. The MeshTag: bits 0-3 clip, 4-9 outfit, 10
  direct position, 27 hit flash, then either (looping) 11-16 phase, 17-24 speed, 25-26 the looping clip it fades
  from and 28-31 that clip's weight (0 = no fade), or (direct) 11-26 a 16-bit clip position.
- **Survivors** (`survivor.rs`): 12 bones (knees, elbows, a right hand), a Bevy skinned mesh with
  StandardMaterial (lit, casts shadows), posed on the CPU each frame with blending: locomotion (idle, walk, run,
  sneak crouch; the cycle advances with distance actually moved), actions on top (punch / one-handed chop /
  two-handed sweep by weapon category, shove, stomp; timed so the blow lands when the server lands it), an
  additive hurt flinch, and a fall onto the back when dead. Every pose is grounded (lowest boot on the floor).
  The held weapon is a child of the hand joint. Your jacket is mustard, other players' teal.

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
  straight meanwhile) and evicts the least recently used field that isn't in use. Fields search a
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
  age on every tier. Crowd separation is capped per tick (`MAX_CROWD_PUSH`), so a pile of promoted zombies spreads
  out instead of being flung through a wall.
- **Snapshots** list only active zombies within `interest_radius` (80 m) of a player (every active zombie when
  there are no players); the client despawns the rest. Full interest management is #22.
- `TickStats` has the tier counts, promotions/demotions and per-tier timings (`tier_ms` for the tier pass plus
  index rebuilds, `active_ms`, `near_ms`, `virtual_ms`); the spatial hash only spans the area its zombies cover.
- Perf (M-series Mac, release): 1 km map, ~1,300 active + ~20,200 virtual (+ ~950 near), a horde of ~900 chasing a
  player: about 1.1 ms/tick average, ~3 ms worst (`tiers_hold_a_thousand_active_and_twenty_thousand_virtual`).

## Population (`sim::population`, `assets/data/population.ron`)

Where zombies live is data (issue #4): `population.ron` (`population::PopulationDef`, validated by
`parse_population`, unknown fields rejected, every field optional with the built-in defaults; loaded by
`net::content::load_population` into `WorldConfig::population`; a data folder without it gets the defaults).
All numbers are ours.

- **Zones** come from the generator (`worldgen::zone_at`, `World::zone_at`): `Indoors` (any room, scaled per room
  type by `rooms`), and outdoors in town the `TownCore` (the middle `core_fraction` of the town's width and height),
  `Street` (roads, sidewalks) and `Yard`; outside town `ForestRoad`, `ForestEdge` (within `forest_edge_width` of
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
  door/window hit points, items, population (targets, zones, last seen, attractors, ids), the clock offset, the debug
  goal, and the flow-field cache (which fields, in order, with their build/use ticks and queued requests; a field is
  rebuilt on load from the map unless that wouldn't reproduce it, e.g. one gone stale after a door changed, which is
  stored whole). Rebuilt on load: walk grid, spatial hash, region index, chunk distances, the goal's field.
  Replication queues (edge changes, edge damage, combat events) and last tick's outputs are not saved: a joining
  client gets the full state. `TickStats` after a load only has the tier counts.
- **Adding state:** `World::save` destructures every `World` field, so a new field doesn't compile until it is saved
  or listed as derived there; zombie columns are listed once in the `zombie_block!` macro in `save.rs`, whose patterns
  name every `Zombies` column. Change the layout of anything saved, and bump `save::FORMAT_VERSION`.
- **The map is regenerated, not stored**: from the saved seed and size plus the content's templates, then the edges
  edited since (`World::map_edits`: doors and windows opened, shut, broken) are applied. A 1 km map is a million tiles
  (several MB even packed) while a game's edits are a few hundred bytes; the price is regenerating on load
  (~0.2 s on 1 km, ~20 ms on the default map). A hash of the generated map (`map_base_hash`) is checked on load
  (`LoadError::MapChanged` if the generator changed). After `World::map_changed` (a hand-edited map: tests,
  editors) the whole map is stored instead.
- **Content vs settings.** The caller passes content (templates, weapons, population, as for `World::new`); the save
  keeps a fingerprint of each (`save::ContentPrint`, hashes plus ids). `World::load_with(bytes, content, LoadOptions)`
  returns `Loaded { world, notes }`, and `save_check::plan_content` sorts changed content. Compatible, loaded with a
  note: a weapon's numbers (the new ones apply), weapons added, reordered or removed when the save uses none of the
  removed ones (saved weapon indices are matched up by id), `population.ron` (respawn and migration settings apply;
  regions keep their saved targets), templates that leave the generated map the same, and any template change for a
  save that stores its map whole. Incompatible, `LoadError::ContentChanged` (`MapChanged` if only the generator
  changed): templates that change the map, a removed weapon the save uses. `LoadOptions { force: true }` (server
  `--load-anyway`, MCP `load {force: true}`, client `--load-anyway`) loads those anyway and notes what it did: items of a
  missing weapon dropped, hands emptied, swings called off; on a new map only the door/window edits that still fit,
  damage only on edges that are still doors or windows, players and zombies on now-blocked tiles moved to free ground,
  flow fields rebuilt as asked for. `World::load` is `load_with` without force, dropping the notes. The
  rest of `WorldConfig` (seed, map size, zombie count, start hour, day length, `ZombieTuning`, `CombatTuning`,
  `TierTuning`) is the save's own, stored as RON so a tuning field added later just takes its default.
- **Format** (`save.rs` docs): `ZOMBYSAV`, a `u32` format version (1), then sections (4-byte tag, `u32` length,
  postcard payload, FNV-1a checksum): `INFO` (`SaveInfo`: game version, tick, time of day, seed, map size, zombies,
  players, content fingerprint), `CONF` (settings, RON), `MAPD` (edits or the full map), `WRLD` (everything else but
  zombies), one `ZOMB` per 64 m region (the tiers' region grid; zombies column by column, ids as gaps), `END`.
  Per-region zombie sections are there so a later streaming save (#3, #22) can rewrite only what changed. Errors are
  `LoadError` values, never panics: `NotASave`, `Version` (older or newer), `Truncated`, `Corrupt` (bad checksum,
  undecodable or inconsistent data), `ContentChanged`, `MapChanged`, `Invalid` (below). No SQLite: a world is one file
  and its metadata is the info section.
- **Validation** (`save_check::validate`, `check_settings`, `zombie_region`): a save with good checksums can still be
  crafted, so values are checked before the world is used, and anything a tick couldn't have made is
  `LoadError::Invalid` naming the field: non-finite numbers (and, in the settings, ones over 1e6), positions off the
  map, zombie speeds or velocities over 50 m/s, zero, repeated, out-of-order or too-new ids, indices out of range
  (weapons, upper-floor rooms, spawn building, hunted players, zombie regions, edge sides), damage on an edge with no
  hit points, negative HP, a living player or zombie with no HP, a dead one with HP or an action under way, a lunge or
  strike off the active tier, a map edit that makes a door or window of something else. The tests mutate a valid world
  and save it (every checksum right) for each case, forge sections, and fuzz section bytes with fixed-up checksums
  (each load fails cleanly or gives a world that steps).
- **Size and speed** (M-series Mac, release, `--save-bench`): the default map with ~980 zombies is about 89 KB, saved
  in ~1-6 ms, loaded in ~22 ms; the 1 km map with 22,500 zombies (21,500 + a 1,000 horde on a player) is 2.0 MB
  (~87 bytes a zombie), saved in ~14 ms and loaded in ~200 ms, nearly all of that regenerating the map.

## Connecting an agent to the game (MCP)

`zombye-server --mcp` speaks MCP over stdio. `.mcp.json` registers it as `zombye` for
Claude Code and other MCP clients. Build it first with `cargo build -p zombye-server --release` (and
`-p zombye-client` too, for `screenshot`).
It takes the common options too (`--zombies N --seed S --map-chunks N`), e.g. a 1 km map with 20,000 zombies, and
`--save PATH` (load-or-create, autosave, save on exit) with `--autosave-mins N` and `--saves DIR`, and for `screenshot`
`--client PATH` and `--screenshot-timeout SECS`. Time is paused
between calls, so an autosave only happens after a call that changed something.

The agent plays player 1, who wakes up in the bedroom. Tools: `screenshot` (below), `observe` (double-resolution ASCII map
with walls `-` `|`, shut windows `=` `!`, open windows `o`, broken windows `x`, shut doors `D` (`d`
when damaged), open doors `/`, broken doors `#`, fences `~`, furniture, players `P` and zombies by
state: `Z` wander, `I` investigate, `C` chase, `L` lunge, `A` attack, `B` battering a door or window;
`S` staggered, `v` down, `%` corpse; zombies on the cheaper tiers `n` near, `?` virtual; weapons on the floor `w`, dead players `X`; plus players (HP,
weapon, action, whether they're climbing or dead), weapons in view, damaged doors and windows in view
with their HP, and the nearest zombies with HP), `zombie {id}` (one zombie's simulation tier and distance to the nearest player, HP, AI state, target,
memory, timers, the edge it batters, and per player the distance, line of sight and current sight range), `edge {x, y}` (the four edges
of a tile, with HP for doors and windows), `step_ticks` (time only advances when you call it; reports
zombie attacks, thumps and anything broken), `move_player` (player, compass `direction` or `dx`/`dy`,
`ticks`, `run`/`sneak`, `facing`: holds that input while the world steps, then stops), `interact`
(opens/closes the door or window player 1, or `player`, faces; `toggle_door` is an old alias),
`smash_window`, `attack` / `shove` / `stomp` (player, optional `facing` or `target` zombie id to turn
first; steps until the action has landed and the player can act again, and reports what it hit),
`pick_up`, `weapons` (the weapon table and combat numbers), `respawn`, `spawn_item {weapon, x, y}`
(debug), `make_noise {x, y, radius, lasting}`
(debug; a loud one leaves an attractor, `lasting: true` a 30-minute one), `regions` (ASCII heat maps per 64 m region of
living zombies and of their targets, the attractors' pull with players marked, and zones; the attractors with reach,
spread, strength and time left; the regions furthest below and above target), `set_goal` (debug: zombies head for a point, senses off), `spawn_zombies`, `skip_time`,
`buildings`, `save {name}` (to `name` in the saves folder, or with no name to the `--save` file), `load {name, force}`
(replaces the world; with no name, the `--save` file; refused, world untouched, if the file is damaged, holds impossible
values or is of another format; changed game data loads with a note when compatible, else is refused unless
`force: true`, which loads it anyway and notes what it changed), `saves` (the saves folder: size, age, tick, time, seed, map size, zombies,
players; and where this session autosaves), `stats` (players with HP/weapon/dead, time of day and daylight, zombies by state
including staggered/down/dead and by tier (`zombie_tiers`: active/near/virtual, last tick's promotions and
demotions, the ranges), `population` (target vs alive, attractors, whether migration and respawn are on, respawned), weapons on the floor, tick timing (tier pass, senses, flow, active, near, virtual), thumps, hits and sounds heard last
tick, damaged edges, the state hash). `step_ticks` and `move_player` also report hits, kills, how many
zombie strikes landed for how much, and deaths.

**`screenshot`** returns a rendered PNG of the world around a player as MCP image content (`{"type": "image",
"data": <base64>, "mimeType": "image/png"}`) followed by a text line: whose view, where the player stands (room or
outdoors, dead or not), the centre if given, tick, time of day and daylight, zoom, angle, PNG size and render time.
Arguments: `player` (default 1: whose fog of war and cutaways, and the centre), `x` and `y` (centre there instead;
fog is still the player's and only zombies within 80 m of a player are sent, so far from players you see terrain and
buildings, darkened, and no zombies), `zoom` (metres visible top to bottom, 8-90, default 26), `angle` (0-3, the four
fixed camera angles), `width` / `height` (default 1280x720, at most 1920x1080). How: the server saves the world to a
temporary file in the system temp folder, runs the game client on it offscreen (`zombye --render-to PNG --load SAVE
--player ... --size ... --data ...`, see Offscreen above; no window opens), waits up to `--screenshot-timeout SECS`
(default 60) and deletes both files. It only reads the world: no ticks, no RNG, the state hash is the same before and
after (and it doesn't mark the world as needing a save). The client binary is `--client PATH`, else `$ZOMBYE_CLIENT`,
else `zombye` next to `zombye-server`: build both (`cargo build --release -p zombye-server -p zombye-client`). A missing
client, a failed render (the client's last log lines are included) and a timeout are tool errors. It takes about
0.4-0.7 s on an M-series Mac (default map or 1 km; the first call after a build can take 1.5-2 s). Use `observe` for exact
positions and states and `screenshot` to check how things look: after changing rendering, meshes or building templates,
or to see a fight, a siege or the fog at night. Each image is large (about 1 MB of base64 at 1280x720), so ask for a
smaller `width`/`height` when the detail isn't needed.
