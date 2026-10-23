# Zombye — Project Plan (v0.1)

A ground-up, true-3D, isometric zombie survival game.
All code and assets are our own; see the clean-room rules in [CONTRIBUTING.md](../CONTRIBUTING.md).

---

## 1. Goals and non-goals

**Prototype (v1) delivers**
- Wake up in a house in a small town ringed by forest. Zombies everywhere.
- Real 3D world viewed from an isometric-style camera; rotate the camera, see through walls near you.
- Hundreds of zombies on screen, thousands simulated across the map.
- Sneak or fight: melee weapons, empty-hand shove, stomp on downed zombies.
- Zombies that see, hear, lose track of you, follow noise and break doors/windows.
- Basic survival: health and wounds, bandages, hunger, thirst, fatigue, endurance.
- Basic crafting only: rip clothing into rags, bandages, sterilise, patch clothing, a handful of tools.
- Loot in containers driven by room type.
- Save and load.

**Designed in from day one, built later**
- Multiplayer (dedicated server, co-op). The single-player game already runs a local server.
- Cars, farming, electricity/water shut-off, advanced crafting, NPCs.

**Simplicity rule for v1 art:** low-poly, flat-ish textures, easy to read from the isometric camera. Detail comes later.

**Project principles**
- **Open source, for everyone.** Code, data, tools and our generated assets live in a public GitHub repo.
- **Agent-first.** Built from the ground up so AI agents can read the code, connect to a running game
  or server, play, test, administer and build mods (§3a).
- **Moddable by default.** Every gameplay value is in a data file; behaviour can be extended with
  sandboxed scripts; servers push mods to clients.
- **Income from hosting.** Official one-click hosted servers are featured in the repo, the launcher and
  the in-game server browser. Anyone can still self-host for free.

---

## 2. Engine and technology

### Recommendation: Rust + Bevy (0.20)

| Need | Why Bevy fits |
|---|---|
| Hundreds–thousands of zombies | Native ECS with parallel systems; zombies are plain data in tight arrays. |
| MP-friendly from the start | Same Rust crate runs headless as a server — no engine fork, no editor dependency. |
| AI writes the code | 100% code and text files; `cargo test` runs the simulation without a window. No binary scene files. |
| Mac M3 dev box | wgpu → Metal is first class. Also builds for Windows/Linux. |
| Meshy assets | glTF/GLB is Bevy's native format, and Meshy exports GLB (rigged + animated). |
| Open source, no royalties | MIT/Apache, so anyone can clone, build and mod with no licence, account or seat fee. Unity's licence and closed editor clash with "open source for everyone". |

**Risks and mitigations**
- *No mature visual editor.* The town is authored as data (grid-based lot files plus building
  templates) and built by tools. We add an in-game debug
  editor for placing props when needed.
- *Bevy breaks APIs each release (~every 4 months).* Pin the version; upgrade deliberately between
  milestones. Keep the simulation crate mostly engine-independent (see §3) so upgrades touch
  rendering code only.
- *Skinned-mesh cost.* Zombies use vertex-animation textures (VAT) with GPU instancing, not
  per-zombie skeletons (§5).

### Alternatives considered

| Option | Verdict |
|---|---|
| **Unity 6 + DOTS/ECS + Netcode for Entities** | Proven for hordes and has a good editor, but it's a poor fit now that the game is open source: contributors need the proprietary editor, scene/prefab files are hard for agents to work with, and Synty packs can't be redistributed in a public repo anyway. |
| Godot 4 | Great text-based workflow, but large crowds need custom GDExtension (C++/Rust) code anyway, and its 3D renderer is the weakest of the options. |
| Unreal 5 (Mass) | Powerful, but heavy C++ build loop, weaker on macOS, overkill for low-poly isometric. |
| Custom engine | Most control, most time. Bevy already gives us 80% of a custom engine with a clean ECS. |

### Supporting stack
- **Networking:** `lightyear` or `bevy_replicon` + `renet` over UDP/QUIC (decided in the M6 spike;
  the sim is written against our own message types so the transport is swappable).
- **Physics:** no general rigid-body physics for zombies. Custom spatial-hash collision on the tile
  grid (cheap, deterministic, server-friendly). `avian3d` only for loose props/ragdoll later.
- **Pathfinding:** tile-grid flow fields for crowds plus hierarchical A* for individuals (§4.3).
- **Save data:** per-region binary files (`bincode`/`postcard`) with a version header; SQLite for metadata.
- **Data files:** RON/TOML for items, weapons, recipes, loot tables, building templates — hot-reloadable.
- **Audio:** `bevy_kira_audio` (Kira) with positional sound and occlusion.
- **Assets:** Meshy API (models, rigging, animation), AI image generation for textures/UI,
  ElevenLabs-style SFX generation for sound effects, Suno for music. No third-party asset packs.
- **Scripting for mods:** Luau (sandboxed Lua, via `mlua`). Lua is familiar to most game modders, and
  Luau's sandbox keeps untrusted server mods safe. WASM mods are a possible later addition.

---

## 3. Architecture

```
zombye/
  crates/
    sim/        # Pure game simulation. No rendering, no windowing. Deterministic tick.
                #   world grid, chunks, zombies, players, combat, items, survival, AI
    net/        # Message types, snapshot/delta encoding, interest management
    server/     # Headless binary: sim + net. Runs the authoritative world.
    client/     # Bevy app: rendering, input, audio, UI, prediction/interpolation
    tools/      # asset pipeline (Meshy fetch, VAT bake, LODs), map builder, debug viewers
  assets/       # generated GLB, textures, audio, data files (RON)
  docs/
```

**Authority model (the key MP decision):** the simulation always runs inside a *server*, even in
single-player — the client talks to an in-process server through the same message API it would use
over the network, as many survival games do. This means MP isn't a retrofit.

- Fixed simulation tick: **20 Hz** on the server. Clients render at 60–120 fps and interpolate.
- Clients send *inputs/intents* (move vector, sneak, attack, shove, interact with object X).
  The server validates and applies them. The local player is predicted client-side for movement.
- Every entity has a stable network ID. Components are split into *replicated* (position, facing,
  animation state, health bucket) and *server-only* (AI memory, target, path).
- **Interest management:** each client receives only entities inside its area of interest
  (≈ its loaded chunks). Off-screen zombies stay on the server — exactly the "server knows where
  everything is, client renders it when you get close" requirement.
- Melee hit detection runs on the server with short lag compensation (rewind zombie positions to
  the client's view time).

---

## 3a. Agent-first, modding and hosting

### Agents can connect to the game
- **Agent API = the player API.** Everything a player can do is an *intent* message (move, attack,
  shove, pick up, craft…), and everything a player can see is a structured *observation*
  (nearby tiles, entities, inventory, status). Agents use the same server interface as human
  clients, with the same visibility rules, so they can't cheat.
- **MCP server built in.** `zombye-server --mcp` (and the client) exposes MCP tools:
  - *play:* `observe`, `act`, `inventory`, `map_around`, as a player or as a survivor NPC
  - *admin:* spawn/teleport/kick, sandbox settings, time/weather, save/restore, server stats
  - *dev:* `screenshot`, `step_ticks`, `query_entities`, `profile_frame`, `reload_data`, `run_script`
  - *mods:* list/enable mods, validate a data pack, hot-reload a script
- **Headless and deterministic.** The sim runs without a window, with seeded RNG and a fixed tick,
  so agents (and CI) can run scenarios ("500 zombies vs a barricaded door for 2 minutes"), record
  replays and assert on results.
- **Agent-readable repo:** `AGENTS.md`/`CLAUDE.md` at the root and per crate, a JSON schema for every
  data file, small focused modules, `cargo xtask` commands for common jobs (run scenario, bake assets,
  build map, package mod).

### Modding
- **Data packs:** items, weapons, recipes, loot tables, zombie types, sandbox presets, building
  templates, map lots — all RON with schemas. The base game itself is just the `core` data pack.
- **Scripts:** Luau scripts hook into documented events (on_tick, on_hit, on_craft, on_zombie_spawn…)
  and run server-side (authoritative); client-side scripts only for UI.
- **Assets:** mods ship GLB/PNG/OGG; the asset pipeline tools work for modders as well.
- **Distribution:** a server lists its mods by content hash; clients download them on join. A mod
  index (a GitHub repo of manifests) works as the "workshop".
- **Agent modding:** "make me a mod that adds a nail bat" → an agent writes the data entry,
  generates the model via the Meshy tool, validates it via MCP, and hot-reloads it into a running game.

### Hosting (income)
- `zombye-server` ships as a single binary and a Docker image; the config is one file.
- **Official hosting** (featured): one-click servers, automatic backups, mod sync, admin dashboard,
  and an agent/MCP admin endpoint. It's promoted in the README, the launcher and the server browser.
- Self-hosting stays free and documented. That's what makes the open-source community trust it,
  and our income comes from convenience, not lock-in.
- **Licence:** **MIT OR Apache-2.0 for all code, server included**, like the ArtCraft apps. No CLA
  or sign-off: contributions come in under the same licence. Art and audio are CC BY-NC-SA 4.0
  (contributed art comes in under CC BY 4.0), so nobody can sell our art in a clone; videos and
  streams are always allowed. Anyone may run or resell Zombye
  servers, even modified ones. The hosting business is protected by the brand, not the licence:
  the Zombye and Dead Commons names and logos are trademarks
  ([docs/brand/LICENSE-brand.txt](brand/LICENSE-brand.txt)), so only Dead Commons can offer
  "official" hosting, and it's the one featured in the repo, launcher and server browser.

### Clean-room rule (because the repo is public)
- The full rules are in [CONTRIBUTING.md](../CONTRIBUTING.md#clean-room-rules-non-negotiable).
  In short: other games are observed only by playing them, never decompiled or opened up, and
  none of their code, data files, text, art, sounds or map data go into the repo. All numbers in our data files
  are our own tuning.
- Generated assets must have licences that allow open redistribution (check Meshy/Suno/SFX plan
  terms) and get a row in [ATTRIBUTION.md](../ATTRIBUTION.md) with the generator and plan.

---

## 4. World and simulation design

### 4.1 Spatial units
- **1 tile = 1 m × 1 m**, floor-to-floor height **3 m**. Characters are ~1.8 m.
- **Chunk = 16 × 16 tiles** (streaming/save unit). **Region = 16 × 16 chunks = 256 m** (file unit,
  population cell).
- Vertical levels: ground 0, upper floors 1–3, basement −1 (reserved).
- v1 map: ~**1.5 km × 1.5 km** — a procedural town core of ~40–60 buildings, then forest out to the
  edges.

### 4.1a Procedural town
The town is generated from a **seed** by placing a fixed catalogue of hand-designed building
templates. The same seed always produces the same town (it matters for MP and for agent test runs).

**Building catalogue (v1)**

| Type | Count | Notes |
|---|---|---|
| Houses | 6 | e.g. small 1-storey bungalow, 1-storey with garage, 2-storey family house, L-shaped ranch, trailer/mobile home, duplex |
| Offices | 2 | small 1-storey office, 2-storey office block |
| Others (later or v1-optional) | — | gas station, corner store/diner, school, police station, warehouse, church |

**Template = data, not a mesh.** Each template is a RON file: footprint on the tile grid, floors,
rooms (name + `room_type` + tiles), edges (wall/door/window/opening), stairs, roof type, plus
furniture *slots* by room type (bed, fridge, counter, shelf…). The modular 3D kit is assembled from
it at load time.

**Generator steps**
1. **Roads:** main street plus a grid or loose-grid of side streets inside the town boundary,
   tapering into a couple of roads that leave through the forest.
2. **Zoning:** blocks near the centre → offices and shops; outer blocks → houses; edges → houses
   with bigger lots.
3. **Lots:** split blocks into lots facing the street; pick a template that fits each lot (rotated in
   90° steps and/or mirrored for variety), with setbacks, driveway, yard and fence.
4. **Variation:** per-building seeded choice of exterior colour, roof colour, siding/brick material,
   open/locked/barricaded doors, broken windows, interior furniture variant and wear.
5. **Population and loot:** zombie density per zone (centre high, houses medium, forest low);
   containers in furniture slots get loot from `room_type × container_type` tables when first opened.
6. **Forest:** noise-based tree/bush density from the town boundary outward, clearings, a creek,
   trails. Trees are instanced.
7. **Player spawn:** pick a house, put the player in the bedroom, in bed, with the door closed.

The same templates power **mods** (add a house type = add one RON file plus its kit pieces) and an
optional future "hand-placed lots" mode.

### 4.2 Zombie simulation tiers (handles hundreds on screen, thousands in the world)

| Tier | Where | Update rate | What runs |
|---|---|---|---|
| **Active** | ≤ ~60 m from any player (in a loaded chunk) | 20 Hz | Full AI state machine, senses, local avoidance, combat, animation state |
| **Near** | 60–150 m | 2–5 Hz, staggered | Movement along flow fields, hearing, simple state. Not rendered. |
| **Virtual** | Everything else | ~0.2 Hz per region | Zombies are records (`pos`, `type`, `outfit`, `state`) stored per region; wander/migration only, moved in bulk. |

- Promotion/demotion happens per chunk as players' interest areas move, so zombies "are still
  there" when you come back. A zombie that was chasing you keeps heading where it last heard you.
- Population density per region is authored (town core high, forest low) with optional respawn and
  slow migration toward noise "events" (gunshots, alarms, helicopter later).
- Performance budget target (M3 MacBook): **500 active zombies + player at 60 fps**, server
  **1,000 active + 20,000 virtual at 20 Hz** on one core-group.

### 4.3 Movement and pathfinding
- **Flow fields** per goal (player position, noise source) on the tile grid, shared by every zombie
  heading to that goal — one field serves a 300-zombie horde. Rebuilt incrementally, capped per tick.
- **Hierarchical A*** (chunk portal graph) for individual long trips and for virtual-tier migration.
- **Local avoidance:** spatial hash + simple separation; zombies can push each other and pile up on
  fences/doors (crowd pressure increases door damage).
- Doors, windows, fences, stairs are graph edges with costs (closed door = "thump to open").

### 4.4 Zombie AI (per active zombie)
States: `Idle → Wander → Investigate(sound/sight) → Chase → Lunge/Attack → Thump(door/window/barricade)`
plus `Staggered`, `KnockedDown`, `Crawler`, `GetUp`, `FakeDead`, `Eat`, `Dead`.

Senses:
- **Sight:** view cone + range scaled by light level, player stance (sneaking/crouched), movement
  speed, and line of sight through the tile grid (walls, windows, doors).
- **Hearing:** every action emits a sound event `{pos, radius, volume}` (footsteps, combat, breaking
  glass, doors). Walls attenuate. Zombies investigate the loudest recent event.
- **Memory:** last known position, forgotten after N seconds (sandbox option).
- Sandbox options: speed mix (shamblers/fast), strength, toughness, sight, hearing,
  memory, population multiplier.

### 4.5 Player combat
- **Melee:** swing arc test against active zombies in a spatial hash (range + arc angle), multi-hit
  for wide weapons, knockback, stagger chance, crits from behind, weapon condition, endurance cost.
- **Shove (empty hand or weapon push):** short cone, staggers/knocks down; costs endurance; the main
  tool for crowd control.
- **Stomp:** on knocked-down/crawler zombies at your feet; several stomps to kill; cheap but slow.
- **Getting hurt:** zombies scratch/lacerate/bite specific body parts; clothing reduces chance per
  part; being grabbed when surrounded. Infection on bites (sandbox-configurable).
- Weapon/item stats are data files.

### 4.6 Survival (v1 scope)
- Health per body part, bleeding, bandage (dirty/clean/sterilised), wound healing over time.
- Hunger, thirst, fatigue/sleep, endurance, panic, pain, weight/encumbrance.
- Status icons for hunger, pain, fatigue and the like.
- Crafting v1 recipes: rip clothing → rags/denim strips, rags → bandage, boil/alcohol → sterilised
  bandage, patch clothing hole with thread + needle + fabric, spear from branch + knife, barricade
  window with planks + nails + hammer.

### 4.7 Loot
- Buildings are made of rooms with a `room_type` (kitchen, bedroom, bathroom, garage, shop…).
- Containers get rolled from loot tables per `room_type` × `container_type` on first visit (server-side,
  seeded), so loot is consistent and MP-safe.

---

## 5. Rendering

- **Camera:** fixed isometric view: a true-3D scene, rendered with an orthographic
  (or very narrow-FOV) camera at the classic 2:1 isometric angle, following the player, with zoom.
  **Settings option:** "Camera rotation" (off by default). When on, Q/E snap between the four fixed
  angles (45°, 135°, 225°, 315°) with a short eased turn. Never free rotation. Because the scene is
  real 3D, rotation costs nothing extra; cutaways, LOS and the UI just use the current angle.
- **Visibility ("what you can see"):** compute a 2D line-of-sight visibility polygon from the player
  on the tile grid each frame (cheap). Zombies/items outside it are not drawn; terrain outside it is
  darkened and desaturated (fog of war). Behind you is dimmer (view cone).
- **Cutaways:** walls whose tiles are between the camera and the player (and those of the room you're
  in) are lowered or dithered; floors above the player's level and roofs of the building you're in
  are hidden. Done per-instance with a shader parameter, not by rebuilding meshes.
- **Zombie crowds:** one shared low-poly mesh per body type, outfits via texture atlas variants and
  tint, **GPU instancing + vertex animation textures** (walk, shamble, run, lunge, attack, stagger,
  fall, crawl, eat, death). Thousands of animated zombies for the cost of a few draw calls. The
  player and a handful of zombies in special interactions (grapple, stomp kill) can use full skeletal
  animation.
- **Buildings:** each tile owns its north and west edges (wall, door, window, opening). A modular
  kit snaps to those edges (wall segments, corners, door/window frames, floors, stairs, roofs) —
  code-generated or Meshy-generated pieces, merged into one mesh per chunk and level and rebuilt
  only when something changes. The same edge data drives collision, pathfinding and vision.
- **Foliage:** instanced trees/bushes with 2–3 LODs; billboards at distance. Trees block sight.
- **Lighting:** day/night cycle with a directional sun/moon, a few dynamic lights (flashlight,
  lamps) with shadows only near the player. Darkness matters for stealth.
- Only chunks in the client's interest radius exist on the client at all.

---

## 6. Asset pipeline (AI-generated)

1. **Spec files:** each asset is a small RON/TOML entry (prompt, style tags, target size in metres,
  polycount budget, category).
2. **`tools/meshy`:** batch submits text/image-to-3D → remesh (low-poly, `resize_height` to target
  size) → for humanoids: rigging → animations from Meshy's library → downloads GLB.
3. **`tools/bake`:** headless step (Blender CLI or Rust) that normalises scale/pivot, makes LODs, and
  bakes zombie animations into VAT textures + atlases.
4. **Art direction (v1): our own style, made entirely by us.** Low-poly, readable
  silhouettes from high up; muted, slightly desaturated palette; simple hand-painted-looking textures
  without fine detail; early-1990s small-town rural America (wood siding, brick, chain-link fences,
  boxy cars later); zombies in everyday clothes with blood and grime as texture variants. It's
  original work: we never copy, trace or reference another game's art. A style bible (`docs/art-direction.md`) holds the fixed style
  prompt prefix, palette, scale rules and our own reference renders, which feed every Meshy request.
  Higher detail can come later without changing the pipeline.
5. **All art is ours.** No Synty or other third-party asset packs, not even as placeholders: until
   an asset is generated, the game uses simple grey-box shapes. This also keeps the open-source
   repo free of licence problems.
6. **What Meshy is good for:** characters, furniture, props, weapons, vehicles (later), trees.
   **What we generate procedurally instead:** walls, floors, roofs, roads, terrain — they must snap
   exactly to the grid. Their textures come from AI image generation using the same style bible.
7. **Audio:** SFX generated per category (footsteps per surface, swings, hits, shove, stomp, glass,
  doors, zombie groans in many variants), music from Suno. Stored with a manifest for random variant
  selection.

---

## 7. Milestones

Milestones and their tasks live on GitHub, not in this file:
- [Game development project](https://github.com/orgs/deadcommons/projects/2): the board, by status
- [Milestones](https://github.com/deadcommons/zombye/milestones): M2 to M7, plus a backlog of
  later ideas with no milestone

Done so far: **M0** (tech spike) and **M1** (world and visibility). **M2** (player and combat) is in
progress. Pick up an issue there, or open one to propose work.

---

## 8. Decisions

**Decided (2026-10-23)**
- **Camera:** fixed isometric; optional setting to snap between 4 fixed angles.
- **Town:** procedural from a seed, built from a catalogue of standard templates (6 house types,
  2 office types to start).
- **Art:** our own low-poly, readable style for v1 (improved later), all 3D art made by us with Meshy
  plus procedural kit pieces; no Synty or other third-party packs.
- **Studio and licence:** published by Dead Commons (GitHub org `deadcommons`). MIT OR Apache-2.0
  for code, CC BY-NC-SA 4.0 for art and audio, server included, no CLA (§3a).

- **Engine:** Rust + Bevy 0.20.
- **Scale:** 1 tile = 1 m.

---

## 9. Reference material

Private design notes from playing other games are kept out of the public repository.
They describe mechanics in our own words only. See the clean-room rules in
[CONTRIBUTING.md](../CONTRIBUTING.md).
