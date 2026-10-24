---
name: blender-assets
description: Build Zombye game assets (furniture, props, characters) in Blender over the Blender MCP, from a RON spec to a validated GLB, worked on in the private art repo and promoted into the game repo when ready. Use when adding or changing a model or its source. Not for walls, floors, roofs, roads or terrain, which stay code-generated.
---

# Building assets in Blender

**Use it for** props and furniture. Characters and other detailed assets start in Meshy and are
cleaned up with `zchar.py`: the [meshy-assets skill](../meshy-assets/SKILL.md) (docs/PLAN.md §6.5).
**Not for** walls, floors, roofs, roads or terrain: they stay code-generated so they snap to the grid
(PLAN §6.8). Stairs are part of the building kit, not furniture.

**First read [docs/asset-pipeline.md](../../../docs/asset-pipeline.md):** every asset starts from an
approved reference and stops at four gates (reference, blockout, look, in game) for the owner to approve
screenshots before going on, and it lists the sizes every asset must fit (walls, doors, windows,
furniture heights, characters, budgets). This skill is how to do the Blender part of it.

## Two repos

- **Work in progress** lives in the private art repo, deadcommons/zombye-art (`~/games/zombye-art`, or
  `$ZOMBYE_ART`; read its AGENTS.md). One folder per asset, `<kind>/<category>-<id>/` (furniture goes in
  `props/`, e.g. `props/furniture-bed/`): `meta.yml`, `<id>.ron` (spec), `<id>.py` (build script),
  `<id>.blend`, `raw/` (untouched generator output, if any), `renders/`, `export/<id>.glb`. Binary files go
  through Git LFS there (`git lfs install --local` once per clone).
- **Finished assets** are promoted into this public repo: `assets/specs/<category>/<id>.ron`,
  `assets/source/<category>/<id>/` (script, `.blend`, review sheet), `assets/models/<category>/<id>.glb`, and
  an ATTRIBUTION.md row. Nothing else about unreleased art goes public: no files, renders or descriptions
  in this repo, its issues or PRs.
- **The pipeline is code** and lives here: `tools/blender/zasset.py` (builder, palette materials, UVs,
  renders, validation, export; runs inside Blender), `palette.py` (the only place colours live),
  `build.py` (shell runner), `artrepo.py` (plain Python: work folders, status, promotion).
- Without the art repo (a public contributor) the same tools work on this repo's copy directly.

## Workflow

1. **Start**: `python3 tools/blender/artrepo.py start furniture/lamp` makes the work folder and its
   `meta.yml` (status `wip`); an asset already in the game is copied in to be reworked.
2. **Spec**: write `<id>.ron` (id, name, footprint in tiles w × d facing south, size in metres x × y × z
   that fits the footprint less 0.08 m per side for walls, against_wall, tris budget, texture, style,
   description). Copy the shape of an existing one, e.g. `assets/specs/furniture/sofa.ron`.
3. **Build** with a script of `Mesh` calls (`box`, `block`, `prism`, `blob`, `poly`; a palette name per
   part, `faces={"top": ...}` per side), `def build(a): m = a.mesh(); ...; return m`. Run it over the MCP:
   ```python
   # execute_blender_code_for_cli(blend_file=<any existing .blend>, code=...), or execute_blender_code live
   import sys, importlib; sys.path.insert(0, "<game repo>/tools/blender")
   import zasset; importlib.reload(zasset)
   r = zasset.build("furniture/lamp"); result = {"summary": zasset.summary(r)}
   ```
   From a shell: `Blender --background --factory-startup --python tools/blender/build.py -- furniture/lamp`
   (`-- furniture` builds a whole category, `--check` validates only). It builds in the work folder.
4. **Render and critique** `renders/<id>_iso.png`: the game's four angles (orthographic, 30° elevation, yaw
   45° + 90°·k, as `camera.rs`) on floor tiles, the footprint lighter, a wall stub at the back. Does it read
   from high up at game zoom? Silhouette, the one or two shapes that say what it is, contrast between parts.
   `zasset.montage(files, out, cols, shrink)` tiles many sheets to review at once.
5. **Edit, don't regenerate**: change the lines of the script that are wrong, rebuild, compare.
6. **Validate** (every build does; nothing is exported on a failure): one mesh named after the spec,
   transform applied, pivot at the base, size within 2 cm of the spec, inside the footprint less the wall
   margin, back on the wall line if `against_wall`, triangles within budget, no loose vertices, edges or
   zero-area faces, palette materials only with unchanged colours, a UV map.
7. **Check in game** before promoting: `ZOMBYE_EXTRA_MODELS=<work folder>/export` makes the client use the
   work GLB (folders separated by `:`), e.g.
   `ZOMBYE_EXTRA_MODELS=~/games/zombye-art/props/furniture-lamp/export cargo run -p zombye-client --release -- --render-to ~/games/zombye-art/props/furniture-lamp/renders/ingame.png --zombies 0 --time 12 --zoom 12`
   (`--at X,Y`, `--angle 0..3`; the MCP `screenshot` tool passes the server's environment on too). Save at
   least one as `renders/ingame*.png`; set `status: review`, then `ready` once it's been looked at.
8. **Promote**: `python3 tools/blender/artrepo.py promote furniture/lamp` checks it (status `ready`, meta
   complete, `plan_owns_output: yes`, spec, script, `.blend`, sheet, GLB and an in-game render there), copies
   it into this repo and adds the ATTRIBUTION.md row. Commit here (reference the issue), then
   `python3 tools/blender/artrepo.py record <commit> furniture/lamp` sets `status: promoted` and `promoted:`
   in meta.yml and adds the PROMOTED.md line; commit that in the art repo. `artrepo.py status` lists where
   every asset stands.

## Conventions

- 1 unit = 1 m = 1 tile, Z up. Template (x, y) → Blender X = x, Y = −y (template row 0, the back or
  north, is +Y). The glTF exporter's Y-up gives the game's axes directly (x east, y up, z = template y).
- An asset faces **south** (−Y in Blender, toward the default camera); things that stand against a wall
  have their back on +Y. Origin at the footprint's centre on the floor. The client turns it to the
  template's `facing` and stretches it to the placement's tiles if they differ from the footprint, so give
  the spec the footprint the templates mostly use.
- Names: asset id = file stem = the template's furniture kind; work folders kebab-case
  (`furniture-coffee-table`); materials `pal.<name>`; collections `Asset` (exported) and `Review` (floor,
  wall stub, sun, camera; never exported).
- Budgets: small props 100–400 triangles, furniture 400–1,000. Low-poly, flat-shaded, no textures
  (`texture: None`); UVs are box-projected at 1 m per UV unit for later tiling textures.
- Style: follow [docs/art-direction.md](../../../docs/art-direction.md) (faceted, flat-shaded low-poly, 3–6
  palette colours per asset with one dominant, two or three telling details of at least 5 cm, muted early-1990s
  small town with the `brand_*` colours as sparing accents; its review checklist). Colours only from
  `palette.py` (add one there, never inline).

## Rules

- **All art is ours.** No third-party models, textures, kits or packs, not even as placeholders; our own
  references only, never another game's art, models or screenshots; never name other games in either repo.
- **Generated input** (Meshy, Recraft, ...) only from plans that give us ownership (Meshy Pro or above,
  Recraft paid; record tool and plan in meta.yml, ask the owner before spending credits). Keep the untouched
  output in `raw/` and always rework it in Blender: clean-up, retopology, 1 unit = 1 m with a sensible pivot,
  UVs, LODs, rigging if it moves.
- Every promoted asset has its **ATTRIBUTION.md row** in the same commit (`promote` adds it).
- Reference the GitHub issue in commits (CLAUDE.md).

## Gotchas

- `execute_blender_code_for_cli` opens an **existing** `.blend` (it fails on a missing file; make an empty
  one with `Blender --background --factory-startup --python-expr "import bpy;
  bpy.ops.wm.read_factory_settings(use_empty=True); bpy.ops.wm.save_as_mainfile(filepath='/tmp/empty.blend')"`)
  and returns the dict assigned to `result`. The live tools need Blender open with the MCP add-on's server
  started (port 9876).
- After editing `zasset.py` in a live session, `importlib.reload(zasset)` (it reloads `palette` and `artrepo`).
- Blender 5: `material.use_nodes` / `world.use_nodes` are deprecated (nodes are always on); find the
  Principled BSDF defensively (`zasset.material` does). The EEVEE engine id is `BLENDER_EEVEE`; renders work
  headless and take well under a second at 400 px.
- Procedural shader colours (noise, brick, ramps) **don't survive GLB export**: use palette colours, or bake
  to a texture. Validation rejects node-driven base colours. A brick texture needs coordinates (x + y, z) to
  lay its courses horizontally on walls.
- Keep the review renders on the `Standard` view transform (AgX shifts the palette) with dither off (noise
  makes the PNGs ten times bigger).
- `prism` runs along Z unless you pass `axis="X"`/`"Y"` (knobs, round doors, rolled arms).
- Things must stay 0.08 m inside every footprint edge (walls take that), or the validation fails.
