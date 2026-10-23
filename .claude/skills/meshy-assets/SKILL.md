---
name: meshy-assets
description: Make Zombye characters and other detailed assets with Meshy over its MCP (reference image, image-to-3D, rig, animations), then clean them up in Blender with tools/blender/zchar.py and bake them for the crowd with zvat.py. Use when generating a character, creature or detailed model with Meshy, or spending Meshy credits. Not for simple props and furniture, which the blender-assets skill builds directly.
---

# Making assets with Meshy

**Use Meshy for** characters (zombies, survivors) and detailed organic things a box builder can't do well.
**Use plain Blender** ([blender-assets](../blender-assets/SKILL.md)) for props, furniture and kit pieces:
cheaper, exact, palette colours, editable. Walls, floors, roofs, roads and terrain stay code-generated.

**First read [docs/asset-pipeline.md](../../../docs/asset-pipeline.md):** every asset starts from an
approved reference and stops at four gates (reference, blockout, look, in game) for the owner to approve
screenshots before going on, and it lists the sizes every asset must fit (character heights, rigs,
budgets). This skill is how to do the Meshy part of it.

## Rules first

- **Credits are money.** Check `meshy_check_balance` (free) before every paid call and keep a running
  total. Agree a budget with the owner first (a character took 35; plan ~60 with one retry) and stop to
  ask before going past it. **Never retry a create call blindly:** after a timeout or error, look for the
  task with `meshy_list_tasks` (it may have run and billed); a wait that times out just polls again.
- **Paid plan only** (Pro or above): Meshy ToS §3.2 gives paid customers ownership of their output; free
  output is CC BY 4.0 and can't ship. Record the plan as `Meshy (Pro)`.
- **All art is ours, clean room.** Prompts in our own words describing our own design; never name,
  reference or imitate another game or its art, never feed it screenshots or other people's images.
  Brand: stylised, playful-spooky, not photoreal, not gory (blood as muted stains).
- **Work in progress is private.** Everything goes to the art repo (`~/games/zombye-art`, deadcommons/
  zombye-art; read its AGENTS.md): `<kind>/<asset>/` with `meta.yml`, `raw/`, the spec and clean-up
  script, `.blend`, `textures/`, `renders/`, `export/`. Nothing about it (files, renders, descriptions)
  goes in this repo, its issues or PRs until it's **promoted** (reviewed in game, exported, `meta.yml`
  complete): then `export/` → `assets/models/`, the `.blend` and script → `assets/source/`, an
  ATTRIBUTION.md row (`Meshy (Pro)` + the image model + Blender), `promoted:` set there, a PROMOTED.md line.
- **Keep AI identifiers** (ToS §2.4): commit Meshy's PNGs byte-for-byte (they carry XMP AI-generation
  tags); `zchar` writes `ai_generated`, the generator and the task ids into the GLB's extras.
- **Download at once:** API output is deleted after 3 days (ToS §2.5). `raw/` is then the only copy, and
  the cleaned `.blend` is the lasting source.

## Steps and costs (as of 2026-10)

| Step | Tool and settings that worked | Credits |
|---|---|---|
| 1. Spec | `<asset>.ron` in the art folder: id, size (rest-pose box; height exact), tris, texture, `source` (raw GLB), `animations`, `vat` clips, style. ~3,000 tris for a crowd of hundreds. | 0 |
| 2. Reference image | `meshy_text_to_image`, `nano-banana-pro` (GPT Image models cost the same), `pose_mode: t-pose`, `remove_background: true`. Front view, full body, flat lighting, "low-poly, hand-painted, broad flat colour". Look at it before going on. | 9 |
| 3. Image to 3D | `meshy_image_to_3d` with `input_task_id` = the image, `model_type: smart-topology`, `ai_model: meshy-t2.5`, `target_polycount` ≈ the budget, `texture_resolution: 2k`, `pose_mode: t-pose`, `target_formats: [glb]`. ~1 min. Lands within ~5% of the target. | 15 |
| 4. Rig | `meshy_rig` with `input_task_id` = the model, `height_meters`. 24-bone humanoid; includes a free walk and run (not in place). | 5 |
| 5. Animations | `meshy_list_animations` (free; search "walk", "idle", "injur", "limp", "knock", "dead") → one `meshy_animate` with `action_ids` (one file, one clip per id). Use the `_inplace` variants for crowd clips. Text-to-motion for anything the library lacks (3 swift / 10 prime). | 3 per clip |
| 6. Download | Images: `meshy_download_model` with `save_to`. Rig and animation tasks only return URLs: `curl` them. Into `meshy_output/<asset>/` (git-ignored), then copy what's used into the art folder's `raw/` untouched. | 0 |
| 7. Clean up | `<asset>.py` (`build(c)`) with the `zchar.Character` steps; `build.py -- <art folder>` | 0 |

Show the owner each result as it comes (reference image, model, rig, renders); it's cheap to redirect early.

## Clean-up (`tools/blender/zchar.py`)

A character script (see the zombie in the art repo for one) runs, in order: `import_glb` (one armature +
one skinned mesh, importer bone shapes dropped), `apply_transforms` (Meshy's armature is centimetres under
a 0.01 scale: baked into bones *and* the bone translation keys; also moves the hips onto the origin;
check `evaluated_points` before and after), `weld_seams` (glTF splits vertices at UV seams),
`decimate_to(budget)`, `limit_weights` (4 bones), `tone_texture` (toward the palette: saturation,
contrast, lift, tint), `name_things` (`<id>`, `<id>_rig`, actions renamed, each an NLA track).

`build.py -- <art folder>` then validates (one armature + mesh, transforms applied, feet at 0, height
±2 cm, width/depth within the spec, centred, facing south by the toes, budget, no loose geometry, every
vertex weighted ≤ 4 bones, one textured material, UVs, the spec's animations), renders the four game
angles (`renders/<asset>_iso.png`) and a six-pose strip of one cycle per animation, saves the `.blend`
(script inside) and exports `export/<id>.glb` (skin + clips, JPEG texture, provenance extras) and, with
`vat` in the spec, `export/<id>.zvat`. `--check` validates the saved `.blend` alone.

## The crowd: VAT, not retargeting

Zombies draw as one instanced VAT batch (`zombie_render.rs`), today a procedural 7-bone grey-box.
Don't retarget Meshy's 24 bones onto those 7 (knees and elbows lost, rest pose differs, rigid seams
tear): `zvat.py` bakes the skinned mesh's positions per frame instead, any rig, in game model space,
one cycle per loop (`zchar.loop_frames`: Meshy bakes several cycles into a clip), with the walk's stride
for speed-matched playback. Loading `.zvat` in the client, texture sampling and the other crowd clips
are #53.

## Gotchas

- `image_to_3d` normalises the model to ~1 unit tall; the rig step's `height_meters` sets real size.
- Meshy's in-place walk clip is several cycles long (208 frames = 6 × ~35): sample one cycle, or every
  strip frame shows the same pose.
- Blender 5 actions are slotted: `action.fcurves` is gone (`zchar.action_fcurves`).
- `render_angles` frames the mesh's local bounds: apply the armature scale first or the character is a
  speck.
- The importer adds an `Icosphere` per bone shape; `import_glb` drops them.
- The raw import shows ~2,500 "open edges": seam splits, not holes. After welding a few remain.
- In a worktree-isolated session, chained shell commands with `$VARS` or heredocs are refused: write a
  script file and run it, use literal paths.
