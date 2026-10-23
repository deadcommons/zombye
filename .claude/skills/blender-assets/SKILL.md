---
name: blender-assets
description: Build Zombye game assets (furniture, props, characters) in Blender over the Blender MCP, from a RON spec to a validated GLB the client draws. Use when adding or changing a model in assets/models/ or its source in assets/source/. Not for walls, floors, roofs, roads or terrain, which stay code-generated.
---

# Building assets in Blender

**Use it for** props, furniture and characters (characters and other detailed assets start from a
ChatGPT reference image through Meshy, then come here to be brought to standard: docs/PLAN.md §6.5).
**Not for** walls, floors, roofs, roads or terrain: they stay code-generated so they snap to the grid
(PLAN §6.8). Stairs are part of the building kit, not furniture.

## Files

| What | Where |
|---|---|
| Helpers (run inside Blender) | `tools/blender/zasset.py` (builder, palette materials, UVs, renders, validation, export), `tools/blender/palette.py` (the only place colours live), `tools/blender/build.py` (shell runner) |
| Spec | `assets/specs/<category>/<id>.ron`: id, name, footprint (tiles, w × d facing south), size (m, x × y × z), against_wall, tris (budget), texture, style, description |
| Source | `assets/source/<category>/<id>/<id>.py` (defines `build(a)`), `<id>.blend` (saved by the build, script inside as a text block), `<id>_iso.png` (the four angles) |
| Output | `assets/models/<category>/<id>.glb`; furniture GLBs are loaded by `crates/client/src/furniture_models.rs` by kind name |

## Workflow

1. **Spec first.** Write the RON spec: real-world size that fits the footprint less 0.08 m per side
   (walls), a triangle budget, style notes in the palette's terms.
2. **Build** with a script of `Mesh` calls (`box`, `block`, `prism`, `blob`, `poly`; a palette name per
   part, `faces={"top": ...}` per side). Run it over the MCP, headless:
   ```python
   # execute_blender_code_for_cli(blend_file=<any existing .blend>, code=...)
   import sys, importlib; sys.path.insert(0, "<repo>/tools/blender")
   import zasset; importlib.reload(zasset)
   r = zasset.build("furniture/bed"); result = {"summary": zasset.summary(r)}
   ```
   or live (`execute_blender_code`, Blender open) the same way, to look around the scene. From a
   shell: `Blender --background --factory-startup --python tools/blender/build.py -- furniture/bed`
   (`-- furniture` builds a whole category, `--check` validates only).
3. **Render and critique** `<id>_iso.png`: the game's four angles (orthographic, 30° elevation, yaw
   45° + 90°·k, as `camera.rs`), on floor tiles with the footprint lighter and a wall stub at the back.
   Ask: does it read from high up at game zoom? Silhouette, the one or two shapes that say what it is,
   contrast between parts. `zasset.montage(files, out, cols, shrink)` tiles many sheets to review at once.
4. **Edit, don't regenerate**: change the lines of the script that are wrong, rebuild, compare.
5. **Validate** (every build does; nothing is exported on a failure): one mesh named after the spec,
   transform applied, pivot at the base, size within 2 cm of the spec, inside the footprint less the
   wall margin, back on the wall line if `against_wall`, triangles within budget, no loose vertices,
   edges or zero-area faces, palette materials only with unchanged colours, a UV map.
6. **Export** (the build does): GLB, Y up, the asset mesh only, footprint as glTF extras.
7. **Check in game** offscreen (no window, no focus stolen):
   `cargo run -p zombye-client --release -- --render-to shot.png --at X,Y --zoom 12 --time 12`
   (`buildings` over the zombye MCP lists houses; `--angle 0..3`), or the MCP `screenshot` tool.
8. **Commit** the spec, script, `.blend`, sheet and GLB together with an ATTRIBUTION.md row each.

## Conventions

- 1 unit = 1 m = 1 tile, Z up. Template (x, y) → Blender X = x, Y = −y (template row 0, the back
  or north, is +Y). The glTF exporter's Y-up gives the game's axes directly (x east, y up, z = template y).
- An asset faces **south** (−Y in Blender, toward the default camera); things that stand against a
  wall have their back on +Y. Origin at the footprint's centre on the floor. The client turns it to the
  template's `facing` and stretches it to the placement's tiles if they differ from the footprint,
  so give the spec the footprint the templates mostly use.
- Names: asset id = file stem = the template's furniture kind; materials `pal.<name>`; collections
  `Asset` (exported) and `Review` (floor, wall stub, sun, camera; never exported).
- Budgets: small props 100–400 triangles, furniture 400–1,000. Low-poly, flat-shaded, no textures
  (`texture: None`); UVs are box-projected at 1 m per UV unit for later tiling textures.
- Style (until `docs/art-direction.md`, #14): muted, slightly desaturated early-1990s small-town
  America; readable silhouettes from high up; colours only from `palette.py` (add one there, never inline).

## Rules

- **All art is ours.** No third-party models, textures, kits or packs, not even as placeholders;
  never trace or copy another game's art, and never name other games in the repo or commits.
- Every asset gets an **ATTRIBUTION.md row** in the same commit (author Dead Commons, made in
  Blender over MCP by an agent from the committed script; CC BY-NC-SA 4.0).
- Reference the GitHub issue in commits (CLAUDE.md).

## Gotchas

- `execute_blender_code_for_cli` opens an **existing** `.blend` (it fails on a missing file; make an
  empty one with `Blender --background --factory-startup --python-expr "import bpy;
  bpy.ops.wm.read_factory_settings(use_empty=True); bpy.ops.wm.save_as_mainfile(filepath='/tmp/empty.blend')"`)
  and returns the dict assigned to `result`. The live tools need Blender open with the MCP add-on's
  server started (port 9876).
- After editing `zasset.py` in a live session, `importlib.reload(zasset)`; `palette` is re-read too.
- Blender 5: `material.use_nodes` / `world.use_nodes` are deprecated (nodes are always on); find the
  Principled BSDF defensively (`zasset.material` does). The EEVEE engine id is `BLENDER_EEVEE`; renders
  work headless and take well under a second at 400 px.
- Procedural shader colours (noise, brick, ramps) **don't survive GLB export**: use palette colours,
  or bake to a texture. Validation rejects node-driven base colours. A brick texture needs coordinates
  (x + y, z) to lay its courses horizontally on walls.
- Keep the review renders on the `Standard` view transform (AgX shifts the palette) with dither off
  (noise makes the PNGs ten times bigger).
- `prism` runs along Z unless you pass `axis="X"`/`"Y"` (knobs, round doors, rolled arms).
- Things must stay 0.08 m inside every footprint edge (walls take that), or the validation fails.
