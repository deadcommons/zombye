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
| `crates/sim` | Pure simulation: chunked tile map with edge walls, building templates, town generator, line of sight, players (`players.rs`: shared movement + endurance), zombies (`zombies.rs` storage, `senses.rs` sight + hearing, `ai.rs` state machine + `ZombieTuning`), flow fields + cache (`flow.rs`), weapons and ground items (`items.rs`), melee combat and player health (`combat.rs` + `CombatTuning`). No engine, no I/O. Deterministic. | MIT OR Apache-2.0 |
| `crates/net` | `Intent` (client → server) and `Snapshot` (server → client) types, `LocalServer` (with `join`), content loading (`content.rs`). | MIT OR Apache-2.0 |
| `crates/server` | `zombye-server` headless binary: bench mode and the MCP endpoint. | MIT OR Apache-2.0 |
| `crates/client` | `zombye` Bevy client: camera, chunk streaming, building/tree meshes, fog of war, cutaways, day/night, input + player prediction (`player.rs`), grey-box characters (`rig.rs` bones + boxes; `zombie_render.rs` VAT crowd; `survivor.rs` skinned, CPU-posed players), hit feedback (`fx.rs`), ground items (`items_render.rs`), HUD, `--autoplay` (`autoplay.rs`). | MIT OR Apache-2.0 |
| `assets/` | Shaders, data files (`data/buildings/*.ron` templates, `data/items/weapons.ron`), later generated models/textures/audio. | MIT OR Apache-2.0 (each asset listed in [ATTRIBUTION.md](ATTRIBUTION.md)) |
| `docs/` | Plan and design docs. | — |

## Rules

1. **The server is the authority.** Clients (human, agent or the single-player UI) only send
   `Intent`s and read `Snapshot`s. Never mutate `World` from client code except through
   `zombye_net::apply_intent`.
2. **Keep `sim` deterministic.** Fixed tick (`TICK_DT`), all randomness from `sim::rng::Rng`, no
   wall-clock time, no `HashMap` iteration order in simulation logic, stable ordering of entities.
   `World::state_hash()` must match for the same seed and inputs. There's a test for it.
3. **Keep `sim` engine-free.** It depends on `glam`, `serde` and `ron` only, and does no I/O (content is
   loaded by `net::content` and passed in). Rendering concerns live in `client`.
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
cargo run -p zombye-client --release -- --zombies 1000          # the game window
cargo run -p zombye-client --release -- --zombies 2000 --novsync --screenshot shot.png --exit-after 8
cargo run -p zombye-client --release -- --fight-demo --autoplay --zoom 9 --screenshot fight.png --screenshot-after 2.5,4,6 --exit-after 7
```

Client flags: `--zombies N --seed S --time HOUR --at X,Y --rotate --novsync --zoom METRES --screenshot PATH --screenshot-after SECS[,SECS...] --exit-after SECS --fight-demo --siege --autoplay --sneak`
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
The client prints one stats line per second to stdout (time, room, stance, hp, endurance, fps, frame ms, zombies, sim tick ms).
In game: WASD move, Shift run, C sneak (toggle), mouse to face, left mouse attack (swing your weapon, or
punch), Space shove (or stomp, when a downed zombie lies right in front of you), E pick up the weapon you
face, F open/close the door or window you face, G smash the window you face, R get up again after dying,
T skip an hour, Z spawn zombies, wheel zoom. Walk into an open or broken window to climb through it.

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
- `set_goal` is a debug override: zombies within 120 m path distance head for that point, senses off.

## Connecting an agent to the game (MCP)

`zombye-server --mcp` speaks MCP over stdio. `.mcp.json` registers it as `zombye` for
Claude Code and other MCP clients. Build it first with `cargo build -p zombye-server --release`.

The agent plays player 1, who wakes up in the bedroom. Tools: `observe` (double-resolution ASCII map
with walls `-` `|`, shut windows `=` `!`, open windows `o`, broken windows `x`, shut doors `D` (`d`
when damaged), open doors `/`, broken doors `#`, fences `~`, furniture, players `P` and zombies by
state: `Z` wander, `I` investigate, `C` chase, `L` lunge, `A` attack, `B` battering a door or window;
`S` staggered, `v` down, `%` corpse, weapons on the floor `w`, dead players `X`; plus players (HP,
weapon, action, whether they're climbing or dead), weapons in view, damaged doors and windows in view
with their HP, and the nearest zombies with HP), `zombie {id}` (one zombie's HP, AI state, target,
memory, timers, the edge it batters, and per player the distance, line of sight and current sight range), `edge {x, y}` (the four edges
of a tile, with HP for doors and windows), `step_ticks` (time only advances when you call it; reports
zombie attacks, thumps and anything broken), `move_player` (player, compass `direction` or `dx`/`dy`,
`ticks`, `run`/`sneak`, `facing`: holds that input while the world steps, then stops), `interact`
(opens/closes the door or window player 1, or `player`, faces; `toggle_door` is an old alias),
`smash_window`, `attack` / `shove` / `stomp` (player, optional `facing` or `target` zombie id to turn
first; steps until the action has landed and the player can act again, and reports what it hit),
`pick_up`, `weapons` (the weapon table and combat numbers), `respawn`, `spawn_item {weapon, x, y}`
(debug), `make_noise {x, y, radius}`
(debug), `set_goal` (debug: zombies head for a point, senses off), `spawn_zombies`, `skip_time`,
`buildings`, `stats` (players with HP/weapon/dead, time of day and daylight, zombies by state
including staggered/down/dead, weapons on the floor, tick timing, thumps, hits and sounds heard last
tick, damaged edges, the state hash). `step_ticks` and `move_player` also report hits, kills, how many
zombie strikes landed for how much, and deaths.
