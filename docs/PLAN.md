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
- Multiplayer on a dedicated server, **open PvP by default** (§4.8): you survive the dead and the
  living, avoiding both or teaming up. Co-op is a server setting (PvP off). The single-player game
  already runs a local server.
- A first-person camera, as a client option and a server rule (§5).
- A huge open world: several towns and the land between, streamed in regions near players, with
  hordes, fires and other survivors carrying on far from anyone (built on the tiers of §4.2).
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
| Our own assets | glTF/GLB is Bevy's native format; Blender (and Meshy) export GLB with rigs and animations. |
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
- **Assets:** Blender driven by agents over MCP for most assets; characters and detailed assets from
  a ChatGPT reference image through Meshy's API, finished in Blender (§6); AI image generation for textures/UI,
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
    tools/      # asset pipeline (Blender MCP build scripts, VAT bake, LODs, Meshy import), map builder, debug viewers
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
- **Other players are sent only when your survivor could see or hear them** (worked out on the
  server, with the same sight and hearing rules as everything else), so a modified client can't see
  through walls. The camera never decides what you know; the server does (§4.8, §5).
- Melee hit detection runs on the server with short lag compensation (rewind zombie and player
  positions to the attacker's view time).

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
  builds the model in Blender over MCP, validates it via MCP, and hot-reloads it into a running game.

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

### 4.8 Players and each other (open PvP)
On a multiplayer server PvP is on by default. Other survivors are the second danger: you can avoid
them, fight them, rob them or team up with them. Nothing in the game decides who is a friend.
- **One combat system.** Swings, shoves and stomps hit players as they hit zombies: the same reach and
  arc, the same wounds on body parts, clothing protection, bleeding and infection. A death names its
  killer in plain words ("killed by a survivor with a crowbar"). A dead player drops everything where
  they fell, as now.
- **Noise is the price of a fight.** Every swing, scream, broken window and (later) gunshot is a sound
  the zombies hear, and loud ones draw the far horde in over minutes. Fighting another survivor is
  always a gamble, which makes avoiding each other, or making a deal, a real choice.
- **Fair information.** You only learn about another player through your survivor's senses: the server
  sends a player to your client only when you could see or hear them, and nothing else (no name tags
  over strangers, no other players on the map). Agents connect through the same rules, so an AI
  survivor is a fair player, not a cheat.
- **Alliances come from play.** Proximity text and voice chat (heard near the speaker, quieter through
  walls) and a few gestures. You recognise people by their clothes. Later: restraints for hold-ups and an
  optional group (shared map markers, seeing your group's names); groups never switch damage off.
- **Spawns and logging out.** New survivors start spread round the edge of the town with little, not
  in one bedroom. A survivor who logs out stays in the world for a short while (so leaving can't end a
  fight), and a fresh spawn gets a few seconds of protection.
- **Bases matter more.** Barricades, locked doors and safehouses protect you from people as well as the
  dead; raiding them is part of the game.
- **Server rules** (the server config, shown in the server browser): `pvp` on or off, the camera (§5),
  whether agents may join and whether they are marked, logout timer, spawn protection.
- **Size.** M6 aims at 32 players on a server and is designed for 64, on maps of 1-2 km. Players are
  cheap; the zombies awake around them are what costs (§4.2), so a server's zombie count and map size
  go with its player count.
- Single-player and co-op are the same game with `pvp` off.

---

## 5. Rendering

- **Camera:** fixed isometric view: a true-3D scene, rendered with an orthographic
  (or very narrow-FOV) camera at the classic 2:1 isometric angle, following the player, with zoom.
  **Settings option:** "Camera rotation" (off by default). When on, Q/E snap between the four fixed
  angles (45°, 135°, 225°, 315°) with a short eased turn. Never free rotation. Because the scene is
  real 3D, rotation costs nothing extra; cutaways, LOS and the UI just use the current angle.
- **First person (later):** because the world is real 3D, a first-person camera is mostly client work:
  mouse look, arms and the held weapon in view, ceilings and close-up detail indoors, better character
  models. The sim stays camera-agnostic: aiming gets a pitch with ranged weapons, and what a player
  knows comes from the server's sight rules, not the camera. The isometric camera sees over walls and
  round corners, so in PvP the camera is a server rule (isometric, or first person only) rather than
  mixed on one server.
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
  code-generated or Blender-built pieces, merged into one mesh per chunk and level and rebuilt
  only when something changes. The same edge data drives collision, pathfinding and vision.
- **Foliage:** instanced trees/bushes with 2–3 LODs; billboards at distance. Trees block sight.
- **Lighting:** day/night cycle with a directional sun/moon, a few dynamic lights (flashlight,
  lamps) with shadows only near the player. Darkness matters for stealth.
- Only chunks in the client's interest radius exist on the client at all.

---

## 6. Asset pipeline (Blender, driven by agents)

The main tool is **Blender, driven by agents over MCP** (a Blender MCP server). Subagents build each
asset as an editable Blender scene (primitives and modifiers, materials, armature, animation actions),
render previews, take feedback and iterate, then export GLB. The `.blend` source and the script or
steps that produced it are committed with the asset (`assets/source/`), so anyone, person or agent,
can reopen it, change it and export again. That gives us control and assets we can keep improving,
where a one-shot generator only gives us a new roll of the dice.

**Characters and other detailed assets** take a second route: a reference image generated with
ChatGPT from the style bible, then Meshy's image-to-3D API (Dead Commons' Meshy Pro account, so we
own the output) for the model, rig and base animations, then into Blender for clean-up and to bring
it to our standard. Simple and modular assets (props, furniture, kit pieces) are built in Blender
directly.

1. **Spec files:** each asset has a small RON entry in `assets/specs/` (name, category, target size in
  metres, polygon and texture budget, style notes, a plain-words description).
2. **Build in Blender over MCP:** a subagent blocks out the model, UV-unwraps it, applies palette
  materials or simple hand-painted textures, and sets pivot and scale to the 1 m grid. Characters
  share one humanoid base mesh and armature; outfits are variants; animations (walk, run, sneak,
  attacks, stagger, falls, deaths) are Blender actions on that rig.
3. **Review loop:** renders from the game's four isometric angles, plus in-game checks with the MCP
  `screenshot` tool, are critiqued by an agent or a person, and the scene is edited, not regenerated.
4. **Export and bake** (`tools/bake`, headless Blender CLI): GLB export, LODs, and the vertex animation
  textures and atlases the zombie crowd renderer uses. The same step takes Meshy output.
5. **Characters and detailed assets via Meshy:** a reference image from ChatGPT (front/side views,
  the style bible's palette and proportions), sent to Meshy's image-to-3D API on our Pro account,
  with Meshy rigging and animation for humanoids. The result is imported into Blender over MCP and
  brought to the same standard (scale, pivot, palette, polygon budget, a rig matching our base
  skeleton) before it ships. The reference image, the Meshy task and the Blender clean-up are all
  recorded with the asset, and its ATTRIBUTION.md row names the tools (ChatGPT image + Meshy Pro +
  Blender).
6. **Art direction (v1): our own style, made entirely by us.** Low-poly, readable
  silhouettes from high up; muted, slightly desaturated palette; simple hand-painted-looking textures
  without fine detail; early-1990s small-town rural America (wood siding, brick, chain-link fences,
  boxy cars later); zombies in everyday clothes with blood and grime as texture variants. It's
  original work: we never copy, trace or reference another game's art. A style bible (`docs/art-direction.md`) holds the fixed style
  prompt prefix, palette, scale rules and our own reference renders, which feed every Blender build
  and every Meshy prompt.
  Higher detail can come later without changing the pipeline.
7. **All art is ours.** No Synty or other third-party asset packs, not even as placeholders: until
   an asset exists, the game uses simple grey-box shapes. This also keeps the open-source repo free of
   licence problems. Blender-built assets are original work by Dead Commons or a contributor
   (see assets/LICENSE and ATTRIBUTION.md).
8. **Procedural in code:** walls, floors, roofs, roads and terrain stay code-generated so they snap
   exactly to the grid. Their textures follow the same style bible.
9. **Audio:** SFX generated per category (footsteps per surface, swings, hits, shove, stomp, glass,
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
- **Art:** our own low-poly, readable style for v1 (improved later). All 3D art is made by us: most of it
  in Blender driven by agents over MCP; characters and detailed assets from a ChatGPT reference image
  through Meshy (Pro), finished in Blender; plus procedural kit pieces; no Synty or other third-party
  packs (decided 2026-10-23).
- **Studio and licence:** published by Dead Commons (GitHub org `deadcommons`). MIT OR Apache-2.0
  for code, CC BY-NC-SA 4.0 for art and audio, server included, no CLA (§3a).

- **Engine:** Rust + Bevy 0.20.
- **Scale:** 1 tile = 1 m.

**Decided (2026-10-24)**
- **Open PvP by default** on multiplayer servers; co-op is `pvp` off (§4.8). M6 aims at 32 players a
  server, designed for 64.
- **First person later**, as a client option and a server rule; the sim stays camera-agnostic (§5).

---

## 9. Reference material

Private design notes from playing other games are kept out of the public repository.
They describe mechanics in our own words only. See the clean-room rules in
[CONTRIBUTING.md](../CONTRIBUTING.md).
