"""Bakes a skinned character's actions into a vertex animation texture (VAT) for the crowd renderer,
run inside Blender (5.x) on a cleaned character (`zchar`, `zgarment`). Any rig, any number of bones: each
frame is just the deformed vertex positions, which is what `crates/client/src/zombie_render.rs` reads (it
bakes its procedural grey-box zombie the same way at startup). See `.claude/skills/meshy-assets/SKILL.md`.

    import zvat
    info = zvat.bake("<out>/zombie_01.zvat", clips=[("walk", 24, "loop"), ("idle", 24, "loop")])

A clip is `(action, frames, mode)`: mode `"loop"` samples one cycle found by `zchar.loop_frames` (Meshy
bakes several into a clip), `"cycle"` takes the action's frame range as exactly one closed cycle (its last
frame is its first again, as a cleaned-up clip keyed at whole frames is), `"once"` spans the range first to
last. `frames` None: one per source frame (a cycle of n frames gives n, a one-shot n + 1).

**Outfit variants** (`crowd`): one mesh per variant, the body (often its crowd LOD) with the regions a
wearable hides cut away, plus each wearable it wears, all baked together into one `.zvat`, because the
crowd draws one textured mesh per batch. A wearable coloured with flat palette materials (no texture of
its own) gets **swatches**: its colours are painted into a copy of the body's texture where that variant's
visible body never samples (texels only the hidden regions used, or none did), and every vertex of a
material has its UVs on that colour's swatch. So each variant has its own texture, `<variant>.png` beside
`<variant>.zvat` (the client looks for `<stem>.png` there). See `crowd`.

File (little-endian), version 1:
    b"ZVAT", u32 version, u32 vertices V, u32 indices I, u32 clips C
    C × clip: 16-byte name (UTF-8, zero padded), u32 frames, f32 seconds per play,
              f32 stride (metres per loop at walking speed, 0 = fixed rate), u32 one_shot (0/1)
    V × 2 f32 UV (glTF convention: v down), I × u32 triangle indices
    per clip, per frame, V × 3 f32 positions
Positions are game model space: metres, Y up, facing +Z, feet at y = 0 (Blender x, z, -y). Vertices
are split where UVs differ, like a glTF export, so each one has a single UV.
"""

import math
import shutil
import struct
from pathlib import Path

import bpy
import numpy as np

import zchar

VERSION = 1
REGIONS = ("Head", "Torso", "Arms", "Hands", "Legs", "Feet")  # as zgarment.REGIONS (the `_region` attribute)


def _regions(obj):
    """Vertex index -> region name: the `_region` attribute, else the `region_<name>` groups."""
    me = obj.data
    attr = me.attributes.get("_region")
    if attr is not None and attr.domain == "POINT":
        return {i: REGIONS[int(round(d.value))] for i, d in enumerate(attr.data) if 0 <= round(d.value) < len(REGIONS)}
    names = {g.index: g.name[len("region_"):] for g in obj.vertex_groups if g.name.startswith("region_")}
    return {v.index: names[g.group] for v in me.vertices for g in v.groups if g.group in names and g.weight > 0.5}


def _split_vertices(me, hidden=None, uv_of_material=None):
    """One output vertex per (mesh vertex, UV) pair; returns (source vertex per output vertex, UVs,
    triangle indices). Triangles touching a vertex in `hidden` (a set of vertex indices) are left out,
    as Blender's Mask modifier would. `uv_of_material` ({material index: (u, v)}, Blender's v up) puts
    every corner of a material's faces on that one UV (palette swatches) instead of the mesh's UVs."""
    me.calc_loop_triangles()
    uv = me.uv_layers.active.data if me.uv_layers.active else None
    key_to_out, src, uvs, idx = {}, [], [], []
    for tri in me.loop_triangles:
        if hidden and any(me.loops[li].vertex_index in hidden for li in tri.loops):
            continue
        for li in tri.loops:
            vi = me.loops[li].vertex_index
            if uv_of_material is not None:
                u, v = uv_of_material[tri.material_index]
            else:
                u, v = uv[li].uv
            key = (vi, round(u, 6), round(v, 6))
            out = key_to_out.get(key)
            if out is None:
                out = key_to_out[key] = len(src)
                src.append(vi)
                uvs.append((u, 1.0 - v))
            idx.append(out)
    return src, uvs, idx


def _stride(arm, action, f0, cycle, fps):
    """Metres the body would cover per loop of an in-place walk: while a foot is planted (in its lowest
    fifth) it slides backward at the walking speed."""
    pb = arm.pose.bones
    feet = [b for b in pb if "foot" in b.name.lower()]
    if not feet:
        return 0.0
    ad = arm.animation_data
    ad.action = bpy.data.actions[action]
    speeds = []
    for foot in feet:
        track = []
        for k in range(int(cycle) + 1):
            bpy.context.scene.frame_set(int(f0) + k)
            track.append((arm.matrix_world @ foot.head).copy())
        zmin = min(p.z for p in track)
        zmax = max(p.z for p in track)
        for a, b in zip(track, track[1:]):
            if a.z < zmin + (zmax - zmin) * 0.2 and b.z < zmin + (zmax - zmin) * 0.2:
                speeds.append((b.y - a.y) * fps)  # planted foot slides toward +Y (backward) as the body walks south
    if not speeds:
        return 0.0
    speeds.sort()
    stride = max(0.0, speeds[len(speeds) // 2]) * cycle / fps
    return stride if stride > 0.05 else 0.0  # feet shifting in an idle: a fixed rate


class Part:
    """A mesh baked into a `.zvat` with others: `hide` the body regions cut away (vertices in them and
    the triangles touching them), `uv_of_material` palette swatches for a mesh without a texture."""

    def __init__(self, obj, hide=(), uv_of_material=None):
        self.obj, self.hide, self.uv_of_material = obj, set(hide), uv_of_material


def bake(out_path, clips, parts=None, arm=None):
    """Bakes `clips` [(action, frames | None, "loop" | "cycle" | "once")] of the Asset collection's
    character (or of `parts`, a list of `Part`s deformed by `arm`, merged into one mesh). Returns a
    summary: vertices, triangles, rows, bytes, and per clip its frames, seconds, stride, how well a loop
    closes, and per frame the highest and lowest point (lying down, standing up: the joins)."""
    if parts is None:
        arms, meshes = zchar.character_objects()
        arm, parts = arms[0], [Part(meshes[0])]
    scene = bpy.context.scene
    fps = scene.render.fps / scene.render.fps_base
    split = []  # per part: (obj, source vertices, first output vertex)
    uvs, idx = [], []
    for part in parts:
        hidden = {i for i, r in _regions(part.obj).items() if r in part.hide} if part.hide else None
        src, puv, pidx = _split_vertices(part.obj.data, hidden, part.uv_of_material)
        split.append((part.obj, src))
        base = len(uvs)
        uvs += puv
        idx += [base + i for i in pidx]
    ad = arm.animation_data or arm.animation_data_create()
    for t in ad.nla_tracks:
        t.mute = True
    saved = ad.action
    header, frames_data, summary = [], [], []
    for name, frames, mode in clips:
        action = bpy.data.actions[name]
        f0, f1 = action.frame_range
        if mode == "loop":
            cycle, err = zchar.loop_frames(arm, name)
        if mode in ("loop", "cycle"):
            if mode == "cycle":
                cycle, err = f1 - f0, None
            frames = frames or int(round(cycle))
            span, times = cycle, [f0 + cycle * k / frames for k in range(frames)]
            stride = _stride(arm, name, f0, cycle, fps)
        else:
            span, err, stride = f1 - f0, 0.0, 0.0
            frames = frames or int(round(span)) + 1
            times = [f0 + span * k / max(frames - 1, 1) for k in range(frames)]
        ad.action = action
        rows, tops, lows = [], [], []
        for t in times + ([f1] if mode == "cycle" else []):
            scene.frame_set(int(math.floor(t)), subframe=t - math.floor(t))
            dg = bpy.context.evaluated_depsgraph_get()
            pts = []
            for obj, src in split:
                ev = obj.evaluated_get(dg)
                me = ev.to_mesh()
                mw = ev.matrix_world
                co = [mw @ v.co for v in me.vertices]
                pts += [(co[i].x, co[i].z, -co[i].y) for i in src]
                ev.to_mesh_clear()
            rows.append(np.array(pts, dtype=np.float32))
        if mode == "cycle":  # how well the cycle closes: its last frame against its first
            err = float(np.linalg.norm(rows[-1] - rows[0], axis=1).mean())
            rows = rows[:-1]
        for r in rows:
            frames_data.append(r.astype("<f4").tobytes())
            tops.append(float(r[:, 1].max()))
            lows.append(float(r[:, 1].min()))
        secs = span / fps
        header.append(struct.pack("<16sIffI", name.encode()[:16], frames, secs, stride, 0 if mode != "once" else 1))
        summary.append({"clip": name, "mode": mode, "frames": frames, "seconds": round(secs, 3), "stride_m": round(stride, 3),
                        "source_frames": [f0, f1], "cycle_frames": round(span, 2),
                        "loop_error_m": None if err is None else round(err, 4),
                        "top_m": [round(min(tops), 3), round(max(tops), 3)], "low_m": [round(min(lows), 3), round(max(lows), 3)],
                        "top_first_last_m": [round(tops[0], 3), round(tops[-1], 3)],
                        "tops": [round(x, 2) for x in tops]})
    ad.action = saved
    scene.frame_set(0)
    out_path = Path(out_path)
    out_path.parent.mkdir(parents=True, exist_ok=True)
    with open(out_path, "wb") as f:
        f.write(b"ZVAT" + struct.pack("<4I", VERSION, len(uvs), len(idx), len(clips)))
        f.write(b"".join(header))
        f.write(b"".join(struct.pack("<2f", *uv) for uv in uvs))
        f.write(struct.pack(f"<{len(idx)}I", *idx))
        f.write(b"".join(frames_data))
    rows = sum(c["frames"] for c in summary)
    return {"path": str(out_path), "vertices": len(uvs), "triangles": len(idx) // 3, "rows": rows,
            "bytes": out_path.stat().st_size, "vat_mb_rgba16f": round((len(uvs) + 6) * rows * 8 / 2**20, 2),
            "parts": {obj.name: len(src) for obj, src in split}, "clips": summary}


# ---------------------------------------------------------------- outfit variants and swatches

def _srgb(c):
    c = max(0.0, min(1.0, c))
    return c * 12.92 if c <= 0.0031308 else 1.055 * c ** (1 / 2.4) - 0.055


def base_color(mat):
    """A material's flat base colour (linear RGB), or its base-colour image."""
    bsdf = next((n for n in mat.node_tree.nodes if n.type == "BSDF_PRINCIPLED"), None) if mat and mat.use_nodes else None
    if bsdf is None:
        return None, None
    inp = bsdf.inputs["Base Color"]
    if inp.links and inp.links[0].from_node.type == "TEX_IMAGE":
        return None, inp.links[0].from_node.image
    return tuple(inp.default_value)[:3], None


def _coverage(obj, hide, size, cell):
    """A (size/cell)² bool grid (row 0 at v = 0, Blender's image rows): cells the visible triangles'
    UVs touch (each triangle sampled densely enough that no cell inside it is missed)."""
    n = size // cell
    grid = np.zeros((n, n), bool)
    me = obj.data
    me.calc_loop_triangles()
    hidden = {i for i, r in _regions(obj).items() if r in hide}
    uv = me.uv_layers.active.data
    for tri in me.loop_triangles:
        if any(me.loops[li].vertex_index in hidden for li in tri.loops):
            continue
        p = np.array([uv[li].uv[:] for li in tri.loops]) * n
        steps = int(max(np.ptp(p[:, 0]), np.ptp(p[:, 1]))) * 2 + 2
        a, b = np.meshgrid(np.linspace(0, 1, steps), np.linspace(0, 1, steps))
        keep = a + b <= 1
        pts = p[0] + np.outer(a[keep], p[1] - p[0]) + np.outer(b[keep], p[2] - p[0])
        ij = np.clip(pts.astype(int), 0, n - 1)
        grid[ij[:, 1], ij[:, 0]] = True
    return grid


def _dilate(grid, k):
    out = grid.copy()
    for _ in range(k):
        g = out.copy()
        g[1:] |= out[:-1]
        g[:-1] |= out[1:]
        g[:, 1:] |= out[:, :-1]
        g[:, :-1] |= out[:, 1:]
        out = g
    return out


def _largest_square(free):
    """(row, col, side) of the largest all-True square in `free` (its top-left corner)."""
    n, m = free.shape
    dp = np.zeros((n + 1, m + 1), int)
    best = (0, 0, 0)
    for i in range(n):
        for j in range(m):
            if free[i, j]:
                dp[i + 1, j + 1] = 1 + min(dp[i, j + 1], dp[i + 1, j], dp[i, j])
                if dp[i + 1, j + 1] > best[2]:
                    s = dp[i + 1, j + 1]
                    best = (i - s + 1, j - s + 1, s)
    return best


def swatch_texture(body, hide, colours, out_png, margin_px=24, cell=8, max_side_px=256):
    """A copy of `body`'s base-colour image with flat `colours` ({material name: linear RGB}) painted
    where the body's visible triangles (those not in the `hide` regions) never sample: further than
    `margin_px` from any of them, so mip levels down to about texture/margin don't bleed. The first colour
    (the wearable's main one) fills all of that free space, so its swatch survives the smallest mips; each
    later colour is a square of up to `max_side_px` in the largest space left. Written as PNG. Returns
    {material name: (u, v)} (Blender's v up), the swatch centres, and a note."""
    _, img = base_color(body.data.materials[0])
    w, h = img.size
    px = np.empty(w * h * 4, np.float32)
    img.pixels.foreach_get(px)
    px = px.reshape(h, w, 4)
    used = _dilate(_coverage(body, hide, w, cell), max(1, margin_px // cell))
    free = ~used
    out, notes = {}, []
    n = w // cell
    names = list(colours)
    for k, name in enumerate(names):
        r, c, s = _largest_square(free)
        if s < 2:
            raise ValueError(f"no room left in {img.name} for the swatch of {name}")
        if k > 0:
            s = min(s, max_side_px // cell)
        rgb = [_srgb(x) for x in colours[name]]
        if k == 0:
            region = free.copy()
        else:
            region = np.zeros_like(free)
            region[r:r + s, c:c + s] = True
        mask = np.repeat(np.repeat(region, cell, 0), cell, 1)[:h, :w]
        px[mask, 0:3] = rgb
        px[mask, 3] = 1.0
        out[name] = ((c + s / 2) / n, (r + s / 2) / n)
        notes.append(f"{name} swatch {s * cell} px square at uv {out[name][0]:.3f},{out[name][1]:.3f}"
                     + (f" (and every free texel: {region.mean() * 100:.0f}% of the texture)" if k == 0 else ""))
        # The next swatch keeps clear of this one's square (its own margin).
        taken = np.zeros_like(free)
        taken[r:r + s, c:c + s] = True
        free &= ~_dilate(taken, max(1, margin_px // cell))
    tex = bpy.data.images.new(Path(out_png).stem, w, h, alpha=False)
    tex.pixels.foreach_set(px.ravel())
    tex.filepath_raw = str(out_png)
    tex.file_format = "PNG"
    Path(out_png).parent.mkdir(parents=True, exist_ok=True)
    tex.save()
    bpy.data.images.remove(tex)
    return out, "; ".join(notes)


def crowd(spec, export_dir, arm=None):
    """Bakes the spec's `crowd` section: one `.zvat` (and its texture) per outfit variant, into
    `export_dir`. In the spec:

        crowd: (
            mesh: "<id>_lod1",                  // the body mesh the crowd draws (e.g. its LOD)
            clips: [("idle", "cycle"), ("walk", "cycle"), ("stagger", "once"), ...],   // every source frame
            variants: [
                (id: "<id>", wear: []),         // the body alone: its own texture, copied as <id>.<ext>
                (id: "<id>_hoodie", wear: ["hoodie"]),   // + the wearable(s), regions they hide cut away
            ],
        ),

    A worn wearable is `<spec id>_<wearable id>`; it hides the regions its spec entry `hides`. A wearable
    with flat palette materials gets swatches in `<variant>.png` (`swatch_texture`). Returns a summary per
    variant."""
    cfg = spec["crowd"]
    body = bpy.data.objects[cfg["mesh"]]
    arm = arm or body.find_armature() or body.parent
    for o in [body] + [bpy.data.objects[f"{spec['id']}_{w}"] for v in cfg["variants"] for w in v.get("wear", [])]:
        o.hide_viewport = o.hide_render = False  # the depsgraph only evaluates what the view layer shows
        if o.name not in bpy.context.view_layer.objects:
            bpy.context.scene.collection.objects.link(o)
    bpy.context.view_layer.update()
    wear_spec = {g["id"]: g for g in spec.get("wearables", [])}
    clips = [(c[0], None, c[1]) for c in cfg["clips"]]
    export_dir = Path(export_dir)
    out = []
    for v in cfg["variants"]:
        wear = list(v.get("wear", []))
        hide = {r for w in wear for r in wear_spec[w].get("hides", [])}
        parts = [Part(body, hide)]
        swatched = {}
        for w in wear:
            obj = bpy.data.objects[f"{spec['id']}_{w}"]
            cols = {m.name: base_color(m)[0] for m in obj.data.materials}
            if all(c is not None for c in cols.values()):
                swatched[obj] = cols
            parts.append(Part(obj))
        note = ""
        if swatched:
            colours = {}
            for cols in swatched.values():
                colours.update(cols)
            uv_of, note = swatch_texture(body, hide, colours, export_dir / f"{v['id']}.png")
            for p in parts:
                if p.obj in swatched:
                    p.uv_of_material = {i: uv_of[m.name] for i, m in enumerate(p.obj.data.materials)}
        else:
            _, img = base_color(body.data.materials[0])
            src = Path(bpy.path.abspath(img.filepath))
            dst = export_dir / f"{v['id']}{src.suffix.lower()}"
            shutil.copyfile(src, dst)
            note = f"texture {src.name} copied as {dst.name}"
        info = bake(export_dir / f"{v['id']}.zvat", clips, parts, arm)
        info["texture"] = note
        info["wear"], info["hidden"] = wear, sorted(hide)
        out.append(info)
    return out
