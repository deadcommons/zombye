"""Zombye character helpers, run inside Blender (5.x): bring a rigged, animated GLB from a generator
(Meshy) to our standard, validate it, render it and export it. See `.claude/skills/meshy-assets/SKILL.md`.

A character is worked on in the private art repo (deadcommons/zombye-art), one folder per asset:

    characters/zombie-01/
      zombie-01.ron   the spec (id, size, tris, texture, style; `source` = the raw GLB in raw/)
      zombie-01.py    the clean-up script: defines `build(c)`, gets a `Character`, returns the mesh
      raw/            generator output exactly as downloaded
      zombie-01.blend the cleaned scene (saved by the build; the script inside as a text block)
      textures/       the toned base colour
      renders/        the four game angles, a walk strip
      export/         the GLB (mesh, skin, actions)

    import sys, importlib; sys.path.insert(0, "<zombye>/tools/blender")
    import zchar; importlib.reload(zchar)
    report = zchar.build("<zombye-art>/characters/zombie-01")

The build runs the script on the raw GLB when it is there, else opens the saved `.blend` (generator API
output expires, so the `.blend` is the lasting source). Conventions as `zasset`: 1 unit = 1 m, Z up,
facing south (-Y), feet at the origin; the glTF exporter makes it Y up, facing +Z (the game's model space).
"""

import importlib.util
import math
import sys
from pathlib import Path

import bmesh
import bpy
from mathutils import Vector

sys.path.insert(0, str(Path(__file__).resolve().parent))
import zasset  # noqa: E402

# Height must match the spec within this; width (arm span) and depth are upper limits.
HEIGHT_TOLERANCE = 0.02
# The body's centre (the hips at rest) may be this far off the origin, across.
CENTRE_TOLERANCE = 0.03
MAX_INFLUENCES = 4


# ---------------------------------------------------------------- paths and specs

def paths(folder):
    folder = Path(folder)
    name = folder.name
    return {
        "folder": folder,
        "spec": folder / f"{name}.ron",
        "script": folder / f"{name}.py",
        "blend": folder / f"{name}.blend",
        "raw": folder / "raw",
        "textures": folder / "textures",
        "renders": folder / "renders",
        "export": folder / "export",
    }


def load_spec(folder):
    spec = zasset.read_ron(paths(folder)["spec"].read_text())
    for field in ("id", "category", "name", "size", "tris", "texture", "source", "style", "description"):
        if field not in spec:
            raise ValueError(f"{folder}: spec has no `{field}`")
    spec.setdefault("footprint", [1, 1])
    return spec


# ---------------------------------------------------------------- clean-up steps

def action_fcurves(action):
    """F-curves of an action: the slotted layers of Blender 4.4+ (`action.fcurves` is gone in 5)."""
    try:
        return list(action.fcurves)
    except AttributeError:
        return [fc for layer in action.layers for strip in layer.strips
                for bag in strip.channelbags for fc in bag.fcurves]


def import_glb(path):
    """Imports a rigged GLB into the open file. Returns (armature, mesh); drops the importer's bone
    display shapes. Expects one armature and one skinned mesh."""
    before = set(bpy.data.objects)
    bpy.ops.import_scene.gltf(filepath=str(path))
    new = [o for o in bpy.data.objects if o not in before]
    arms = [o for o in new if o.type == "ARMATURE"]
    meshes = [o for o in new if o.type == "MESH" and o.find_armature() is not None]
    for o in new:
        if o not in arms and o not in meshes:
            bpy.data.objects.remove(o)
    if len(arms) != 1 or len(meshes) != 1:
        raise ValueError(f"{path}: expected one armature and one skinned mesh, got {len(arms)} and {len(meshes)}")
    return arms[0], meshes[0]


def _select(objs, active):
    if bpy.context.object and bpy.context.object.mode != "OBJECT":
        bpy.ops.object.mode_set(mode="OBJECT")
    for o in bpy.context.view_layer.objects:
        o.select_set(False)
    for o in objs:
        o.select_set(True)
    bpy.context.view_layer.objects.active = active


def evaluated_points(mesh, step=97):
    """Every `step`-th vertex of the evaluated (posed) mesh in world space: to check a clean-up step
    changed nothing visible."""
    dg = bpy.context.evaluated_depsgraph_get()
    ev = mesh.evaluated_get(dg)
    me = ev.to_mesh()
    pts = [ev.matrix_world @ me.vertices[i].co for i in range(0, len(me.vertices), step)]
    ev.to_mesh_clear()
    return pts


def apply_transforms(arm, mesh, offset=(0.0, 0.0)):
    """Moves the body by `offset` (x, y) and bakes every object transform into the data: the armature's
    scale (a generator's armature is often in centimetres under a 0.01 scale) into its bones, and the
    action's bone translations by the same factor, so it animates exactly as before. Returns the scale."""
    s = arm.scale.x
    if max(abs(arm.scale[k] - s) for k in range(3)) > 1e-6:
        raise ValueError(f"{arm.name}: non-uniform scale {tuple(arm.scale)}")
    arm.location.x += offset[0]
    arm.location.y += offset[1]
    _select([arm, mesh], arm)
    bpy.ops.object.transform_apply(location=True, rotation=True, scale=True)
    if abs(s - 1.0) > 1e-9:
        for action in bpy.data.actions:
            for fc in action_fcurves(action):
                if fc.data_path.endswith(".location"):
                    for k in fc.keyframe_points:
                        k.co.y *= s
                        k.handle_left.y *= s
                        k.handle_right.y *= s
                    fc.update()
    return s


def weld_seams(mesh, dist=1e-5):
    """Merges vertices split at UV seams (glTF stores one vertex per UV, so an import has open seams
    everywhere). UVs, weights and normals are per loop or kept, so nothing visible changes.
    Returns how many were merged."""
    me = mesh.data
    bm = bmesh.new()
    bm.from_mesh(me)
    n = len(bm.verts)
    bmesh.ops.remove_doubles(bm, verts=bm.verts, dist=dist)
    merged = n - len(bm.verts)
    bm.to_mesh(me)
    bm.free()
    me.update()
    return merged


def tris(mesh):
    return sum(len(p.vertices) - 2 for p in mesh.data.polygons)


def decimate_to(mesh, budget):
    """Collapses edges until the mesh is within `budget` triangles (weights and UVs interpolate).
    Returns (before, after)."""
    before = tris(mesh)
    ratio = budget / before
    for _ in range(4):
        if tris(mesh) <= budget:
            break
        mod = mesh.modifiers.new("decimate", "DECIMATE")
        mod.decimate_type = "COLLAPSE"
        mod.ratio = min(1.0, ratio * 0.998)
        mod.use_collapse_triangulate = True
        _select([mesh], mesh)
        while mesh.modifiers[0] != mod:
            bpy.ops.object.modifier_move_up(modifier=mod.name)
        bpy.ops.object.modifier_apply(modifier=mod.name)
        ratio = budget / tris(mesh)
    return before, tris(mesh)


def limit_weights(mesh, limit=MAX_INFLUENCES):
    """At most `limit` bones per vertex, weights normalised (what glTF and the VAT bake take)."""
    _select([mesh], mesh)
    bpy.ops.object.vertex_group_limit_total(group_select_mode="ALL", limit=limit)
    bpy.ops.object.vertex_group_normalize_all(group_select_mode="ALL", lock_active=False)


def base_color_image(mesh):
    """The image feeding the Principled BSDF's Base Color of the mesh's (single) material."""
    mat = mesh.data.materials[0]
    bsdf = next(nd for nd in mat.node_tree.nodes if nd.type == "BSDF_PRINCIPLED")
    link = bsdf.inputs["Base Color"].links[0]
    node = link.from_node
    while node.type != "TEX_IMAGE":  # through any mix or colour node the importer added
        node = node.inputs[0].links[0].from_node
    return node.image


def tone_texture(mesh, out_path, saturation=0.85, contrast=0.92, lift=0.0, tint=(1.0, 1.0, 1.0), quality=90):
    """Grades the base colour toward the palette (PLAN §6.6: muted, slightly desaturated): pulls
    saturation and contrast in around the mid grey, lifts the blacks, multiplies a tint. Works in
    display (sRGB) values, like a painter would. Saves it as a JPEG at `out_path` and points the
    material at the new image (packed into the .blend)."""
    import numpy as np
    src = base_color_image(mesh)
    w, h = src.size
    a = np.empty(w * h * 4, dtype=np.float32)
    src.pixels.foreach_get(a)
    a = a.reshape(h, w, 4)
    rgb = a[..., :3]
    lum = (rgb * np.array([0.2126, 0.7152, 0.0722], dtype=np.float32)).sum(axis=-1, keepdims=True)
    rgb = lum + (rgb - lum) * saturation
    rgb = 0.5 + (rgb - 0.5) * contrast
    rgb = lift + rgb * (1.0 - lift)
    rgb = rgb * np.array(tint, dtype=np.float32)
    a[..., :3] = np.clip(rgb, 0.0, 1.0)
    out_path = Path(out_path)
    out_path.parent.mkdir(parents=True, exist_ok=True)
    img = bpy.data.images.new(f"{mesh.name}_base_color", w, h, alpha=False)
    img.pixels.foreach_set(a.ravel())
    img.filepath_raw = str(out_path)
    img.file_format = "JPEG"
    settings = bpy.context.scene.render.image_settings
    settings.file_format, settings.color_mode, settings.quality = "JPEG", "RGB", quality
    img.save_render(str(out_path), scene=bpy.context.scene)
    bpy.data.images.remove(img)
    toned = bpy.data.images.load(str(out_path))
    toned.name = f"{mesh.name}_base_color"
    toned.pack()
    mat = mesh.data.materials[0]
    for nd in mat.node_tree.nodes:
        if nd.type == "TEX_IMAGE" and nd.image == src:
            nd.image = toned
    if src.users == 0:
        bpy.data.images.remove(src)
    return str(out_path)


def name_things(arm, mesh, asset_id, actions):
    """Mesh `<id>`, armature `<id>_rig`, material `<id>`, actions renamed by `actions` (old -> new;
    others removed), every action kept on the armature as an NLA track (the exporter writes each one)."""
    mesh.name = mesh.data.name = asset_id
    arm.name = arm.data.name = f"{asset_id}_rig"
    mesh.data.materials[0].name = asset_id
    base_color_image(mesh).name = f"{asset_id}_base_color"
    for a in list(bpy.data.actions):
        if a.name in actions:
            a.name = actions[a.name]
        else:
            bpy.data.actions.remove(a)
    ad = arm.animation_data or arm.animation_data_create()
    for t in list(ad.nla_tracks):
        ad.nla_tracks.remove(t)
    ad.action = None
    for name in actions.values():
        a = bpy.data.actions[name]
        a.use_fake_user = True
        track = ad.nla_tracks.new()
        track.name = name
        strip = track.strips.new(name, int(a.frame_range[0]), a)
        track.mute = True  # muted: the rest pose stays the rest pose; the exporter writes them anyway


def to_asset_collection(*objs):
    coll = zasset.collection(zasset.ASSET_COLLECTION)
    for o in objs:
        for c in list(o.users_collection):
            c.objects.unlink(o)
        coll.objects.link(o)


def hips_xy(arm):
    """Where the body's centre stands: the root bone's rest head, across."""
    root = next(b for b in arm.data.bones if b.parent is None)
    h = arm.matrix_world @ root.head_local
    return h.x, h.y


# ---------------------------------------------------------------- validation

def character_objects():
    c = bpy.data.collections.get(zasset.ASSET_COLLECTION)
    objs = list(c.objects) if c else []
    return [o for o in objs if o.type == "ARMATURE"], [o for o in objs if o.type == "MESH"]


def validate(spec):
    """Checks the Asset collection against a character spec, in the rest pose. Returns
    {ok, checks: [{check, ok, detail}], warnings, tris, size}."""
    checks, warnings = [], []

    def check(name, ok, detail):
        checks.append({"check": name, "ok": bool(ok), "detail": detail})

    arms, meshes = character_objects()
    check("one armature and one mesh", len(arms) == 1 and len(meshes) == 1,
          f"{len(arms)} armatures, {len(meshes)} meshes in '{zasset.ASSET_COLLECTION}'")
    if len(arms) != 1 or len(meshes) != 1:
        return {"ok": False, "checks": checks, "warnings": warnings}
    arm, obj = arms[0], meshes[0]
    me = obj.data
    check("named after the spec", obj.name == spec["id"] and arm.name == f"{spec['id']}_rig", f"{obj.name}, {arm.name}")
    check("skinned to the armature", obj.find_armature() == arm and obj.parent == arm, f"parent {obj.parent and obj.parent.name}")

    identity = all(o.location.length < 1e-6 and Vector(o.rotation_euler).length < 1e-6
                   and o.rotation_quaternion.angle < 1e-6 and (o.scale - Vector((1, 1, 1))).length < 1e-6
                   for o in (arm, obj))
    check("transforms applied (1 unit = 1 m)", identity,
          "; ".join(f"{o.name} loc {tuple(round(v, 4) for v in o.location)} scale {tuple(round(v, 4) for v in o.scale)}" for o in (arm, obj)))

    pose_position = arm.data.pose_position
    arm.data.pose_position = "REST"
    bpy.context.view_layer.update()
    co = [v.co for v in me.vertices]
    arm.data.pose_position = pose_position
    lo = Vector((min(c.x for c in co), min(c.y for c in co), min(c.z for c in co)))
    hi = Vector((max(c.x for c in co), max(c.y for c in co), max(c.z for c in co)))
    size = hi - lo
    check("feet on the floor", abs(lo.z) < 1e-3, f"lowest point z = {lo.z:.4f}")
    want = Vector(spec["size"])
    check("height matches the spec", abs(size.z - want.z) <= HEIGHT_TOLERANCE,
          f"{size.z:.3f} m vs {want.z:.3f} (±{HEIGHT_TOLERANCE})")
    check("within the spec's width and depth", size.x <= want.x + 1e-3 and size.y <= want.y + 1e-3,
          f"{size.x:.3f} × {size.y:.3f} m within {want.x:.2f} × {want.y:.2f}")
    hx, hy = hips_xy(arm)
    check("centred on the origin", math.hypot(hx, hy) <= CENTRE_TOLERANCE, f"hips at ({hx:.3f}, {hy:.3f})")

    # Facing south: the head's front and the toes are toward -Y of the bones they hang from.
    bones = arm.data.bones
    toes = [b for b in bones if "toe" in b.name.lower()]
    forward = [b.head_local.y - b.parent.head_local.y for b in toes if b.parent]
    check("facing south (-Y)", forward and all(f < 0 for f in forward),
          f"toes {', '.join(f'{f:+.3f}' for f in forward)} m along y from the ankles")

    t = sum(len(p.vertices) - 2 for p in me.polygons)
    check("triangles within budget", t <= spec["tris"], f"{t} / {spec['tris']}")

    bm = bmesh.new()
    bm.from_mesh(me)
    loose_v = sum(1 for v in bm.verts if not v.link_faces)
    loose_e = sum(1 for e in bm.edges if not e.link_faces)
    degenerate = sum(1 for f in bm.faces if f.calc_area() < 1e-8)
    nonmanifold = sum(1 for e in bm.edges if not e.is_manifold)
    bm.free()
    check("no loose geometry", loose_v == 0 and loose_e == 0 and degenerate == 0,
          f"{loose_v} loose vertices, {loose_e} loose edges, {degenerate} zero-area faces")
    if nonmanifold:
        warnings.append(f"{nonmanifold} open (non-manifold) edges")

    names = {g.index: g.name for g in obj.vertex_groups}
    bone_names = {b.name for b in bones}
    unweighted, over, unknown = 0, 0, set()
    for v in me.vertices:
        gs = [g for g in v.groups if g.weight > 1e-4]
        if not gs:
            unweighted += 1
        if len(gs) > MAX_INFLUENCES:
            over += 1
        unknown |= {names[g.group] for g in gs if names[g.group] not in bone_names}
    check("every vertex weighted, at most 4 bones", unweighted == 0 and over == 0 and not unknown,
          f"{unweighted} unweighted, {over} over {MAX_INFLUENCES} bones" + (f", groups with no bone: {sorted(unknown)}" if unknown else ""))

    try:
        img = base_color_image(obj)
    except (StopIteration, IndexError, AttributeError):
        img = None
    check("one textured material", len(me.materials) == 1 and img is not None,
          f"{len(me.materials)} materials, base colour {img.name if img else 'none'} {tuple(img.size) if img else ''}")
    check("UV map", len(me.uv_layers) > 0, f"{len(me.uv_layers)} UV maps")

    ad = arm.animation_data
    clips = [s.action.name for t in (ad.nla_tracks if ad else []) for s in t.strips if s.action]
    want_clips = list(spec.get("animations", []))
    check("animations", all(c in clips for c in want_clips) and clips,
          f"{', '.join(clips) or 'none'}" + (f" (spec: {', '.join(want_clips)})" if want_clips else ""))

    return {"ok": all(c["ok"] for c in checks), "checks": checks, "warnings": warnings,
            "tris": t, "size": [round(v, 3) for v in size]}


# ---------------------------------------------------------------- renders

def render_angles(spec, sheet_path):
    """A 2 × 2 sheet of the four game angles of the rest pose (`zasset.render_angles`, with the scale
    figure and door frame); the single angles go to a temporary folder."""
    arm = character_objects()[0][0]
    arm.data.pose_position = "REST"
    try:
        return zasset.render_angles(spec, sheet_path=sheet_path)[-1]
    finally:
        arm.data.pose_position = "POSE"


def loop_frames(arm, action, bone=None):
    """The length in frames of one cycle of a looping `action`: generators often bake several cycles
    into a clip (Meshy's in-place walk is six). Compares a foot's (or `bone`'s) path, relative to the
    root, against itself shifted by each lag, and takes the shortest lag within 25% of the best.
    Returns (frames, mean error in metres)."""
    ad = arm.animation_data or arm.animation_data_create()
    saved = ad.action
    ad.action = bpy.data.actions[action]
    f0, f1 = (int(round(v)) for v in ad.action.frame_range)
    pb = arm.pose.bones
    root = next(b for b in pb if b.parent is None)
    bone = bone or next((b.name for b in pb if "foot" in b.name.lower()), root.name)
    path = []
    for f in range(f0, f1 + 1):
        bpy.context.scene.frame_set(f)
        path.append(pb[bone].head - root.head)
    ad.action = saved
    n = len(path)
    errs = {}
    for lag in range(8, n - 4):
        errs[lag] = sum((path[i] - path[i + lag]).length for i in range(n - lag)) / (n - lag)
    best = min(errs.values())
    lag = min(l for l, e in errs.items() if e <= best * 1.25)
    return lag, errs[lag]


def render_strip(spec, action, out_path, frames=6, angle=0, cols=6):
    """`frames` poses spread over one cycle of `action` from game angle `angle`, side by side."""
    arm = character_objects()[0][0]
    ad = arm.animation_data
    for t in ad.nla_tracks:
        t.mute = True
    cycle, _ = loop_frames(arm, action)
    ad.action = bpy.data.actions[action]
    f0 = bpy.data.actions[action].frame_range[0]
    f1 = f0 + cycle
    tmp = Path(out_path).parent / f".strip_{action}"
    files = []
    scene = bpy.context.scene
    for k in range(frames):
        scene.frame_set(int(round(f0 + (f1 - f0) * k / frames)))
        cam_files = zasset.render_angles(spec, out_dir=tmp, scale=False)
        files.append(cam_files[angle])
    ad.action = None
    scene.frame_set(0)
    zasset.montage(files, out_path, cols=cols)
    for f in tmp.glob("*.png"):
        f.unlink()
    tmp.rmdir()
    return str(out_path)


# ---------------------------------------------------------------- export

def export_glb(path, extras=None):
    """Exports the armature and mesh with the skin and every action (as named clips), Y up, the base
    colour as JPEG, and `extras` (provenance: generator, plan, task ids) on the mesh as glTF extras."""
    arms, meshes = character_objects()
    arm, obj = arms[0], meshes[0]
    for k, v in (extras or {}).items():
        obj[k] = v
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    _select([arm, obj], arm)
    arm.data.pose_position = "POSE"
    bpy.ops.export_scene.gltf(
        filepath=str(path), export_format="GLB", use_selection=True, export_yup=True,
        export_apply=False, export_skins=True, export_animations=True, export_animation_mode="ACTIONS",
        export_force_sampling=True, export_frame_step=1, export_def_bones=False,
        export_materials="EXPORT", export_image_format="JPEG", export_jpeg_quality=90,
        export_texcoords=True, export_normals=True, export_cameras=False, export_lights=False,
        export_extras=True,
    )
    return str(path)


# ---------------------------------------------------------------- the whole build

class Character:
    """What a clean-up script's `build(c)` gets: the spec, the paths and these steps."""

    def __init__(self, spec, folder):
        self.spec = spec
        self.paths = paths(folder)
        self.raw = self.paths["raw"] / spec["source"]
        self.notes = []
        # Provenance the script sets (generator, plan, task ids): glTF extras on the exported mesh.
        self.extras = {}

    def note(self, text):
        self.notes.append(text)

    import_glb = staticmethod(import_glb)
    apply_transforms = staticmethod(apply_transforms)
    weld_seams = staticmethod(weld_seams)
    decimate_to = staticmethod(decimate_to)
    limit_weights = staticmethod(limit_weights)
    tone_texture = staticmethod(tone_texture)
    name_things = staticmethod(name_things)
    to_asset_collection = staticmethod(to_asset_collection)
    evaluated_points = staticmethod(evaluated_points)
    hips_xy = staticmethod(hips_xy)
    tris = staticmethod(tris)


def build(folder, render=True, export=True, save=True, from_blend=False, out_dir=None, g3=False, refs=None):
    """Builds the character in `folder`: runs its clean-up script on the raw GLB (or, when that's
    gone or `from_blend`, opens the saved `.blend`), validates, renders the four angles (with the
    scale figure and door frame) and a strip per animation, saves the `.blend` and exports the GLB.
    Nothing is exported when validation fails. Provenance (`c.extras`) is kept on the mesh, so a build
    from the `.blend` exports it too. `out_dir` writes everything (renders, textures, `.blend`, export)
    there instead of the folder, which is then only read: a scratch build. `g3` (or `refs`, image
    paths to put beside it instead of the ones found) adds the Gate 3 sheet, `renders/<name>_g3.png`."""
    p = paths(folder)
    blend = p["blend"]
    if out_dir:
        out = Path(out_dir).expanduser()
        p.update(blend=out / p["blend"].name, textures=out / "textures", renders=out / "renders", export=out / "export")
    spec = load_spec(folder)
    c = Character(spec, folder)
    c.paths = p
    report = {"asset": str(p["folder"]), "notes": c.notes}
    if from_blend or not c.raw.exists():
        bpy.ops.wm.open_mainfile(filepath=str(blend))
        report["from"] = "blend"
    else:
        zasset.reset_scene()
        source = p["script"].read_text()
        mod_spec = importlib.util.spec_from_file_location(f"zchar_build_{spec['id']}", p["script"])
        mod = importlib.util.module_from_spec(mod_spec)
        dont_write, sys.dont_write_bytecode = sys.dont_write_bytecode, True
        try:
            mod_spec.loader.exec_module(mod)
        finally:
            sys.dont_write_bytecode = dont_write
        obj = mod.build(c)
        for k, v in c.extras.items():
            obj[k] = v
        report["from"] = "raw"
    report.update(validate(spec))
    if render and report["ok"]:
        sheet = p["renders"] / f"{p['folder'].name}_iso.png"
        report["renders"] = [render_angles(spec, sheet)]
        report["sheet"] = str(sheet)
        if g3 or refs:
            zasset.add_gate3(report, spec, p["folder"], sheet, refs)
        for name in spec.get("animations", []):
            report["renders"].append(render_strip(spec, name, p["renders"] / f"{p['folder'].name}_{name}.png"))
    if save and report["from"] == "raw":
        report["blend"] = zasset.save_blend(p["blend"], p["script"].read_text(), f"{p['folder'].name}.py")
    if export and report["ok"]:
        report["glb"] = export_glb(p["export"] / f"{spec['id']}.glb")
        if spec.get("vat"):
            import zvat
            report["vat"] = zvat.bake(p["export"] / f"{spec['id']}.zvat", [tuple(c) for c in spec["vat"]])
    return report
