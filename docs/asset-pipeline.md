# Asset pipeline: from reference to approved

How every model in Zombye is made, whether it's built in Blender or starts in Meshy: it begins
from an approved reference, passes review gates where the owner approves screenshots, and only
then goes into the game. Then the shared rules: the numbers every asset has to fit.

- **The look** (style sentence, palette, mood board, prompt prefix) is the art-direction bible,
  [art-direction.md](art-direction.md). Colours live only in `tools/blender/palette.py`.
- **The tooling** (build, render, validate, export, promote) is #15 and two skills:
  [blender-assets](../.claude/skills/blender-assets/SKILL.md) (`tools/blender/zasset.py`, `artrepo.py`) and
  [meshy-assets](../.claude/skills/meshy-assets/SKILL.md) (Meshy over its MCP, then `zchar.py` clean-up
  and `zvat.py` crowd bake).
- **This doc** is the process and the numbers both of them follow.

## Two routes, one standard

| Route | For | Starts with |
|---|---|---|
| **Blender** | props, furniture, kit pieces, simple vehicles | a spec and a reference, then a build script of primitives (`tools/blender/zasset.py`) |
| **Meshy** | characters and other detailed assets | a spec and a reference image, then Meshy (Pro plan only) for the model, rig and base animations; the untouched output goes in `raw/` and is reworked in Blender |

Both end in the same place: a Blender scene we can edit, held to the same rules, passing the same
gates. Walls, floors, roofs, roads and terrain are **not assets**: the game generates them from the
building templates so they snap to the grid. Their numbers are below because assets must fit them.

## Where the work happens: the art repo

Every asset is made in the private art repo,
**[deadcommons/zombye-art](https://github.com/deadcommons/zombye-art)**, checked out at
`~/games/zombye-art` (or wherever `$ZOMBYE_ART` points), with Git LFS for the big files
(`git lfs install --local` once after cloning). Its README and AGENTS.md have the details. One folder
per asset, `<kind>/<category>-<id>/` (e.g. `props/furniture-bed/`, `characters/zombie-01/`):

| In the asset's folder | What |
|---|---|
| `meta.yml` | name, `status` (wip → review → ready → promoted), tool and plan, prompt, author, the promoting commit |
| `<id>.ron`, `<id>.py`, `<id>.blend` | the spec, the build script, the Blender scene |
| `refs/` | reference images (ours) and the prompts that made them |
| `raw/` | generator output exactly as downloaded, never edited (Meshy route) |
| `renders/` | every gate's pictures, by gate and round (`g3-r2_iso.png`), and the in-game shots |
| `review.md` | every review round: date, gate, pictures, verdict, the owner's notes |
| `export/` | the game-ready GLB (and later LODs and baked textures) |

Plus `style/approved/<category>/` (approved review sheets: the references for the next assets) and
`PROMOTED.md` (everything promoted so far). `python3 tools/blender/artrepo.py start|status|promote|record`
in this repo makes the folder, shows where each asset stands, copies a ready one in here with its
ATTRIBUTION.md row, and records the promotion there.

Nothing from the art repo, review screenshots included, goes into this repo, its issues, Discord or
anywhere public until the asset is promoted. Contributors without access work on this repo's copies
directly (`assets/specs/`, `assets/source/`, `assets/models/`); the tools fall back to them.

## The steps and the four gates

Every asset goes through these steps in order. At each **gate** the agent stops, shows the owner
the pictures, and waits. Nothing moves past a gate without the owner's approval in words.
Credits are spent only after the owner says yes (Meshy: say the expected cost first).

1. **Brief.** A spec (`<id>.ron`: footprint, size in metres, triangle budget, style notes, a
   plain-words description) and the asset's folder (`artrepo.py start`).
2. **Reference.** One to four images of what we're making (see References below).
   **Gate 1: reference.** The owner picks or rejects. Nothing 3D is made before this.
3. **Blockout.** Plain grey shapes at the right size, or for Meshy the raw model brought to scale.
   Proportions and silhouette only, no colour.
   **Gate 2: blockout.** Rendered from the four game angles beside the scale figure and a door.
4. **Look.** Palette colours, materials, detail, textures, rig if it moves.
   **Gate 3: look.** The review sheet beside the reference and one or two approved assets of the
   same kind.
5. **In game.** Exported and drawn in the game before promotion (`ZOMBYE_EXTRA_MODELS`, then
   `--render-to` or the MCP `screenshot` tool): default zoom (26 m) and close up (12 m), by day and
   at night, in the room or street it belongs in.
   **Gate 4: in game.** Approved here, `status: ready`.
6. **Promote** (`artrepo.py promote`, then `record`): into this repo with its ATTRIBUTION.md row.

For small, simple props the owner can merge gates 2 and 3 into one. After Gate 2 the asset is
**edited, not regenerated**: change the script lines or the mesh that are wrong, keep what was
approved.

### Asking for review

One message per gate (or one contact sheet for a batch, `zasset.montage`) with:

- the asset and the gate ("bed, Gate 3: look");
- the pictures (saved in the asset's `renders/`, named by gate and round: `g3-r2_iso.png`);
- what changed since the last round, in a line or two;
- the spec numbers that matter (size, budget, triangle count now);
- any specific question ("darker wood, or keep it?").

The owner answers *approved* or with changes. The agent logs every round in the asset's
`review.md`: date, gate, round, pictures, verdict, the owner's notes in their words. `meta.yml`
says `status: review` while a gate waits for an answer and `wip` while work goes on.

## References

What an asset is measured against, in order of strength:

1. **Approved assets.** Once an asset passes Gate 4, its review sheet becomes a reference for
   everything after it in that category (`style/approved/<category>/` in the art repo). New assets
   are always reviewed beside one or two of them. This keeps a set consistent more than anything else.
2. **The mood board** ([art-direction.md](art-direction.md#mood-board)): a few images we generated from
   the fixed style prompt, picked by the owner, standing for the game's look.
3. **Asset references**, made per asset before Gate 1: images generated with Meshy's own image
   tools (on our Pro plan) from our own words, kept in the asset's `refs/` with the prompt that
   made them. Front and side views for anything Meshy will turn into 3D.
4. **Real-world facts:** sizes and shapes of real early-1990s things (a fridge is about 1.7 m tall,
   a counter 0.9 m), from general knowledge.

Never as a reference, and never as an input to a generator: another game's art, models,
screenshots or mods, asset packs, or images we don't own. Reference images are ours.

## The review sheet

Every render for a gate uses the same set, so assets can be compared fairly:

- orthographic camera at the game's angles: 30° elevation, four yaws of 45° + 90° × k (`camera.rs`);
- the same sun and sky, on 1 m floor tiles with the footprint lighter, a wall stub at the back for
  things that stand against a wall;
- a **scale figure** (a 1.78 m survivor silhouette) and a **door frame** (1 m × 2.1 m) beside it;
- at Gate 3, the reference and the approved neighbours in the same sheet.

`zasset.render_angles` does the first two today; the scale figure, door frame and side-by-side
references are still to add (#15).

## Shared rules

**Units and axes.** 1 Blender unit = 1 m = 1 tile; chunks are 16 × 16 tiles. Blender Z up; the GLB
is exported Y up. An asset faces **south** (−Y in Blender, toward the default camera); things that
stand against a wall have their back on +Y. Origin at the centre of the footprint, on the floor.
A building template's tile (x, y) is Blender (x, −y).

### Buildings (generated by the game)

From `crates/client/src/world_render.rs` and `crates/sim/src/map.rs`. Change them there, then here.

| What | Size |
|---|---|
| Storey (floor to floor; wall height) | 3.0 m |
| Wall thickness | 0.16 m, centred on the tile edge (0.08 m into each tile) |
| Cut-away wall (when it hides the player) | drops to a 0.4 m stub |
| Door opening | one tile edge (1 m) × 2.1 m; wall above from 2.1 to 3.0 m |
| Door leaf | 0.92 × 2.05 × 0.05 m, pivot at the hinge, at one end of the edge |
| Arch | 1 m × 2.4 m |
| Window opening | one tile edge, sill 0.9 m, head 2.1 m above the floor; centre mullion |
| Climbing through a window | within 0.35 m of the window line |
| Fence | posts 1.05 m tall, 0.1 m square; rails at 0.35–0.45 m and 0.8–0.9 m |
| Roof | eaves at the top of the last storey, 0.4 m overhang, pitch 1 in 2 (about 27°); flat roofs a 0.2 m slab |
| Tree trunk (blocks walking) | 0.3 m radius |

### Furniture and props

- Fit inside the footprint less **0.08 m on each side that meets a wall** (the wall's half thickness).
- Against-wall pieces sit with their back on the wall face.
- Real early-1990s sizes. Common heights: seat 0.45 m, table 0.75 m, counter, sink and stove 0.9 m,
  bed top 0.5 m, wardrobe and fridge up to 2.0 m.
- Keep furniture **under 2.0 m** unless the real thing is taller: walls facing the camera are cut
  away to 0.4 m, but furniture isn't, so a tall piece by the front wall hides the room.
- Nothing taller than 2.1 m in front of a door.
- Containers (anything that can be searched) show where they open: a door line, a drawer front, a lid.

### Characters

| What | Rule |
|---|---|
| Height | survivors about 1.78 m to the top of the head; zombies about 1.72 m (they stoop) |
| Pivot | between the feet, on the floor; facing south |
| Survivor rig | the 12-bone survivor skeleton (`survivor.rs`: pelvis, torso, head, upper and lower arms, right hand, thighs, shins); held weapons on the hand joint |
| Zombie rig | the 7-bone zombie skeleton baked into a vertex animation texture (`zombie_render.rs`); a Meshy rig is retargeted or baked onto it |
| Outfits | variants on one shared base mesh per character type, not new meshes |

### Vehicles, ground items

- Vehicles: real early-1990s sizes (a sedan about 4.7 × 1.8 × 1.4 m), origin at the centre on the ground.
- Items lying on the ground: real size, origin at the centre of the base.

### Readability

At the default zoom (26 m of world on a 720-pixel-high view) one pixel is about 3.6 cm. So:

- **Silhouette first.** It should say what it is from high up, from all four angles.
- **One or two telling shapes** (a fridge's handle and door line, a stove's burners), not many small ones.
- **Nothing that matters smaller than about 5 cm**: it won't show.
- **Contrast between parts**, so an asset doesn't read as one blob.
- **Night and fog:** the fog of war darkens what the player can't see; check the asset still
  reads dimmed.

### Budgets and materials

| Kind | Triangles |
|---|---|
| Small prop or ground item | up to 300 |
| Furniture | 400–1,000 (each spec sets its own) |
| Vehicle | up to 3,000 |
| Character (the crowd draws hundreds) | set per spec once the base mesh exists (#15) |

Colours come only from the palette (validation refuses others). Plain palette materials for
Blender-built assets; textures (2K at most) for Meshy assets, toned to the palette in Blender.
No procedural shaders: they don't survive GLB export.
