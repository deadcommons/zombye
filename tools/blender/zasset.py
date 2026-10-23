"""Zombye asset helpers, run inside Blender (5.x).

Builds an asset from its spec (`<id>.ron`) and its build script (`<id>.py`, which defines
`build(a)`), then validates it, renders the game's four isometric angles, saves the `.blend` beside
the script and exports the GLB. Work happens in the private art repo's folder for the asset
(`<kind>/<category>-<id>/`: spec, script, .blend, renders/, export/) when there is one, else on the
game repo's copy (assets/specs, assets/source, assets/models); `artrepo.py` makes work folders and
promotes finished assets. See `.claude/skills/blender-assets/SKILL.md`.

Live (Blender open, MCP add-on server running) or headless (`execute_blender_code_for_cli` on any
`.blend`, or `tools/blender/build.py` from a shell):

    import sys, importlib; sys.path.insert(0, "<repo>/tools/blender")
    import zasset; importlib.reload(zasset)
    result = zasset.build("furniture/bed")

Conventions: 1 Blender unit = 1 m = 1 tile, Z up. Template (x, y) -> Blender X = x, Y = -y, so the
template's north (row 0, the back) is +Y. An asset faces south (-Y, toward the default camera); the
origin is the centre of its footprint at floor level. The glTF exporter turns Z-up into Y-up, which
gives the game's axes directly (x east, y up, z = template y).
"""

import importlib.util
import json
import math
import os
import random
import tempfile
from pathlib import Path

import sys

import bmesh
import bpy
from mathutils import Vector

sys.path.insert(0, str(Path(__file__).resolve().parent))
import artrepo  # noqa: E402
import palette  # noqa: E402

# A live session picks up palette and path changes with importlib.reload(zasset).
importlib.reload(palette)
importlib.reload(artrepo)

REPO = Path(__file__).resolve().parents[2]
ASSET_COLLECTION = "Asset"
REVIEW_COLLECTION = "Review"
# Walls sit on tile edges and reach this far into the tile (client `WALL_T`): nothing goes nearer.
WALL_MARGIN = 0.08
SIZE_TOLERANCE = 0.02
ELEVATION = math.radians(30.0)
RENDER_PX = 400


# ---------------------------------------------------------------- paths and specs

def paths(asset):
    """`"furniture/bed"` -> the spec, script, `.blend`, review sheet and GLB paths: in the art repo's
    work folder when the asset has one, else the game repo's copy (`artrepo.paths`)."""
    return artrepo.paths(asset)


def read_ron(text):
    """A small RON reader for specs: structs (named or not) become dicts, tuples and lists lists,
    `Some(x)` x, `None` None, other bare words strings."""
    i = 0
    n = len(text)

    def skip():
        nonlocal i
        while i < n:
            if text[i] in " \t\r\n,":
                i += 1
            elif text.startswith("//", i):
                i = text.find("\n", i)
                i = n if i < 0 else i
            elif text.startswith("/*", i):
                i = text.index("*/", i) + 2
            else:
                break

    def word():
        nonlocal i
        s = i
        while i < n and (text[i].isalnum() or text[i] == "_"):
            i += 1
        return text[s:i]

    def value():
        nonlocal i
        skip()
        c = text[i]
        if c == '"':
            j = i + 1
            out = []
            while text[j] != '"':
                if text[j] == "\\":
                    j += 1
                    out.append({"n": "\n", "t": "\t"}.get(text[j], text[j]))
                else:
                    out.append(text[j])
                j += 1
            i = j + 1
            return "".join(out)
        if c == "[":
            i += 1
            items = []
            while True:
                skip()
                if text[i] == "]":
                    i += 1
                    return items
                items.append(value())
        if c == "(":
            return group()
        if c in "-+.0123456789":
            s = i
            i += 1
            while i < n and (text[i] in "0123456789.eE_+-"):
                i += 1
            t = text[s:i].replace("_", "")
            return float(t) if any(ch in t for ch in ".eE") else int(t)
        w = word()
        if not w:
            raise ValueError(f"RON: unexpected {text[i]!r} at {i}")
        skip()
        if i < n and text[i] == "(":
            inner = group()
            if w == "Some":
                return inner[0] if isinstance(inner, list) and len(inner) == 1 else inner
            return inner
        return {"true": True, "false": False, "None": None}.get(w, w)

    def group():
        nonlocal i
        i += 1  # '('
        skip()
        save = i
        key = word()
        skip()
        if key and i < n and text[i] == ":":
            i = save
            d = {}
            while True:
                skip()
                if text[i] == ")":
                    i += 1
                    return d
                k = word()
                skip()
                assert text[i] == ":", f"RON: expected ':' at {i}"
                i += 1
                d[k] = value()
        i = save
        items = []
        while True:
            skip()
            if text[i] == ")":
                i += 1
                return items
            items.append(value())

    v = value()
    return v


def load_spec(asset):
    spec = read_ron(paths(asset)["spec"].read_text())
    for field in ("id", "category", "name", "footprint", "size", "tris", "style", "description"):
        if field not in spec:
            raise ValueError(f"{asset}: spec has no `{field}`")
    return spec


# ---------------------------------------------------------------- scene

def reset_scene():
    """Empty the open file: objects, meshes, materials, lights, cameras, worlds, images, texts and
    collections. Metric units, 1 unit = 1 m."""
    for o in list(bpy.data.objects):
        bpy.data.objects.remove(o)
    for coll in (bpy.data.meshes, bpy.data.materials, bpy.data.cameras, bpy.data.lights,
                 bpy.data.worlds, bpy.data.images, bpy.data.texts, bpy.data.collections):
        for d in list(coll):
            coll.remove(d)
    scene = bpy.context.scene
    scene.unit_settings.system = "METRIC"
    scene.unit_settings.scale_length = 1.0
    return scene


def collection(name):
    c = bpy.data.collections.get(name) or bpy.data.collections.new(name)
    if c.name not in bpy.context.scene.collection.children:
        bpy.context.scene.collection.children.link(c)
    return c


def material(name):
    """The palette material `pal.<name>` (made on first use). Unknown names are an error."""
    if name not in palette.PALETTE:
        raise KeyError(f"{name!r} isn't in tools/blender/palette.py")
    full = f"pal.{name}"
    m = bpy.data.materials.get(full)
    if m:
        return m
    hex_colour, rough, metal = palette.PALETTE[name]
    c = (*palette.srgb_to_linear(hex_colour), 1.0)
    m = bpy.data.materials.new(full)
    try:
        m.use_nodes = True  # deprecated in Blender 5 (always on), needed before it
    except Exception:
        pass
    nt = m.node_tree
    bsdf = next((nd for nd in nt.nodes if nd.type == "BSDF_PRINCIPLED"), None)
    if bsdf is None:
        bsdf = nt.nodes.new("ShaderNodeBsdfPrincipled")
        out = next((nd for nd in nt.nodes if nd.type == "OUTPUT_MATERIAL"), None) or nt.nodes.new("ShaderNodeOutputMaterial")
        nt.links.new(bsdf.outputs[0], out.inputs["Surface"])
    bsdf.inputs["Base Color"].default_value = c
    bsdf.inputs["Roughness"].default_value = rough
    bsdf.inputs["Metallic"].default_value = metal
    m.diffuse_color = c
    m.roughness = rough
    m.metallic = metal
    return m


# ---------------------------------------------------------------- mesh builder

SIDES = ("right", "left", "back", "front", "top", "bottom")  # +X -X +Y -Y +Z -Z
_SIDE_NORMALS = {"right": (1, 0, 0), "left": (-1, 0, 0), "back": (0, 1, 0), "front": (0, -1, 0),
                 "top": (0, 0, 1), "bottom": (0, 0, -1)}


class Mesh:
    """Low-poly geometry in one bmesh: closed boxes, prisms and blobs, a palette material per
    face, outward normals. `finish()` turns it into the asset object."""

    def __init__(self, name):
        self.name = name
        self.bm = bmesh.new()
        self.mats = []
        self.open_faces = set()

    def _mat(self, name):
        if name not in palette.PALETTE:
            raise KeyError(f"{name!r} isn't in tools/blender/palette.py")
        if name not in self.mats:
            self.mats.append(name)
        return self.mats.index(name)

    def _face(self, verts, mat, outward=None):
        f = self.bm.faces.new(verts)
        f.material_index = self._mat(mat)
        f.normal_update()
        if outward is not None and f.normal.dot(Vector(outward)) < 0:
            f.normal_flip()
        return f

    def box(self, lo, hi, mat, faces=None, top_inset=(0.0, 0.0), top_shift=(0.0, 0.0)):
        """Axis-aligned box from corner `lo` to `hi`. `faces` maps a side (top, bottom, front (-Y),
        back (+Y), left (-X), right (+X)) to another material. `top_inset` shrinks the top face by
        (x, y) on each side (a taper), `top_shift` moves it (a lean)."""
        (x0, y0, z0), (x1, y1, z1) = lo, hi
        assert x1 > x0 and y1 > y0 and z1 > z0, f"empty box {lo} {hi}"
        ix, iy = top_inset
        sx, sy = top_shift
        bottom = [(x0, y0, z0), (x1, y0, z0), (x1, y1, z0), (x0, y1, z0)]
        top = [(x0 + ix + sx, y0 + iy + sy, z1), (x1 - ix + sx, y0 + iy + sy, z1),
               (x1 - ix + sx, y1 - iy + sy, z1), (x0 + ix + sx, y1 - iy + sy, z1)]
        v = [self.bm.verts.new(p) for p in bottom + top]
        quads = {
            "bottom": [v[0], v[3], v[2], v[1]], "top": [v[4], v[5], v[6], v[7]],
            "front": [v[0], v[1], v[5], v[4]], "back": [v[2], v[3], v[7], v[6]],
            "left": [v[3], v[0], v[4], v[7]], "right": [v[1], v[2], v[6], v[5]],
        }
        faces = faces or {}
        for side, q in quads.items():
            self._face(q, faces.get(side, mat), _SIDE_NORMALS[side])
        return self

    def block(self, x, y, z, w, d, h, mat, **kw):
        """A box `w` × `d` × `h` centred on (x, y), its bottom at height z."""
        return self.box((x - w / 2, y - d / 2, z), (x + w / 2, y + d / 2, z + h), mat, **kw)

    def prism(self, base, radius, length, mat, sides=8, axis="Z", cap=None, phase=0.0, radius2=None):
        """A closed n-sided prism (or a frustum, with `radius2` at the far end) from `base` along
        +`axis`: legs, knobs, pipes. `cap` gives the end faces another material."""
        base = Vector(base)
        a = {"X": Vector((1, 0, 0)), "Y": Vector((0, 1, 0)), "Z": Vector((0, 0, 1))}[axis]
        u = Vector((0, 0, 1)) if axis != "Z" else Vector((1, 0, 0))
        u = (u - a * u.dot(a)).normalized()
        w = a.cross(u)
        r2 = radius if radius2 is None else radius2
        ring0, ring1 = [], []
        for k in range(sides):
            t = phase + 2 * math.pi * k / sides
            off = u * math.cos(t) + w * math.sin(t)
            ring0.append(self.bm.verts.new(base + off * radius))
            ring1.append(self.bm.verts.new(base + a * length + off * r2))
        for k in range(sides):
            k2 = (k + 1) % sides
            mid = (ring0[k].co + ring0[k2].co + ring1[k].co + ring1[k2].co) / 4
            out = mid - (base + a * a.dot(mid - base))
            self._face([ring0[k], ring0[k2], ring1[k2], ring1[k]], mat, out)
        self._face(list(reversed(ring0)), cap or mat, -a)
        self._face(ring1, cap or mat, a)
        return self

    def blob(self, centre, radii, mat, seed=0, subdiv=1, jitter=0.08, flatten_below=None):
        """A lumpy low-poly ellipsoid (icosphere): pillows, cushions, bags. `flatten_below` clamps
        its bottom to that height so it sits flat."""
        tmp = bmesh.new()
        bmesh.ops.create_icosphere(tmp, subdivisions=subdiv, radius=1.0)
        rng = random.Random(seed)
        c = Vector(centre)
        index = {}
        for v in tmp.verts:
            j = 1.0 + rng.uniform(-jitter, jitter)
            p = Vector((v.co.x * radii[0], v.co.y * radii[1], v.co.z * radii[2])) * j + c
            if flatten_below is not None:
                p.z = max(p.z, flatten_below)
            index[v] = self.bm.verts.new(p)
        for f in tmp.faces:
            verts = [index[v] for v in f.verts]
            mid = sum((v.co for v in verts), Vector()) / len(verts)
            self._face(verts, mat, mid - c)
        tmp.free()
        return self

    def poly(self, points, mat, outward):
        """One flat face (left open: not part of a closed shell)."""
        f = self._face([self.bm.verts.new(p) for p in points], mat, outward)
        self.open_faces.add(f)
        return self

    def finish(self, spec=None):
        """Recalculate normals, unwrap, make the object in the Asset collection, return it."""
        bm = self.bm
        closed = [f for f in bm.faces if f not in self.open_faces]
        bmesh.ops.recalc_face_normals(bm, faces=closed)
        box_unwrap(bm)
        me = bpy.data.meshes.new(self.name)
        bm.to_mesh(me)
        bm.free()
        for name in self.mats:
            me.materials.append(material(name))
        obj = bpy.data.objects.new(self.name, me)
        collection(ASSET_COLLECTION).objects.link(obj)
        if spec:
            obj["footprint"] = [int(v) for v in spec["footprint"]]
            obj["against_wall"] = bool(spec.get("against_wall", False))
        return obj


def box_unwrap(bm):
    """Box-project UVs at 1 UV unit per metre (each face along its main axis), so tiling textures
    added later have the same scale on every asset."""
    uv = bm.loops.layers.uv.verify()
    for f in bm.faces:
        n = f.normal
        ax = max(range(3), key=lambda k: abs(n[k]))
        for loop in f.loops:
            p = loop.vert.co
            loop[uv].uv = {0: (p.y, p.z), 1: (p.x, p.z), 2: (p.x, p.y)}[ax]


# ---------------------------------------------------------------- validation

def asset_objects():
    c = bpy.data.collections.get(ASSET_COLLECTION)
    return [o for o in (c.objects if c else []) if o.type == "MESH"]


def validate(spec):
    """Checks the Asset collection against the spec. Returns {ok, checks: [{check, ok, detail}],
    warnings, tris, size}."""
    checks, warnings = [], []

    def check(name, ok, detail):
        checks.append({"check": name, "ok": bool(ok), "detail": detail})

    objs = asset_objects()
    check("one mesh object", len(objs) == 1, f"{len(objs)} mesh objects in '{ASSET_COLLECTION}'")
    if len(objs) != 1:
        return {"ok": False, "checks": checks, "warnings": warnings}
    obj = objs[0]
    me = obj.data
    check("named after the spec", obj.name == spec["id"], obj.name)

    # Pivot and scale on the grid: an untransformed object, origin at the footprint centre, base at 0.
    identity = (obj.location.length < 1e-6 and Vector(obj.rotation_euler).length < 1e-6
                and (obj.scale - Vector((1, 1, 1))).length < 1e-6)
    check("transform applied", identity, f"loc {tuple(round(v, 4) for v in obj.location)} rot {tuple(round(v, 4) for v in obj.rotation_euler)} scale {tuple(round(v, 4) for v in obj.scale)}")
    co = [v.co for v in me.vertices]
    lo = Vector((min(c.x for c in co), min(c.y for c in co), min(c.z for c in co)))
    hi = Vector((max(c.x for c in co), max(c.y for c in co), max(c.z for c in co)))
    size = hi - lo
    check("pivot at the base", abs(lo.z) < 1e-3, f"lowest point z = {lo.z:.4f}")

    want = Vector(spec["size"])
    off = [abs(size[k] - want[k]) for k in range(3)]
    check("dimensions match the spec", max(off) <= SIZE_TOLERANCE,
          f"size {tuple(round(v, 3) for v in size)} vs spec {tuple(round(v, 3) for v in want)} (±{SIZE_TOLERANCE} m)")

    fw, fd = spec["footprint"]
    half = Vector((fw / 2 - WALL_MARGIN, fd / 2 - WALL_MARGIN))
    inside = lo.x >= -half.x - 1e-4 and hi.x <= half.x + 1e-4 and lo.y >= -half.y - 1e-4 and hi.y <= half.y + 1e-4
    check("inside the footprint", inside,
          f"x {lo.x:.3f}..{hi.x:.3f}, y {lo.y:.3f}..{hi.y:.3f} within ±{half.x:.2f}, ±{half.y:.2f} ({fw}×{fd} tiles less {WALL_MARGIN} m for walls)")
    if spec.get("against_wall"):
        check("back against the wall", hi.y >= half.y - 0.05, f"back at y = {hi.y:.3f}, wall line {half.y:.3f}")

    tris = sum(len(p.vertices) - 2 for p in me.polygons)
    check("triangles within budget", tris <= spec["tris"], f"{tris} / {spec['tris']}")

    bm = bmesh.new()
    bm.from_mesh(me)
    loose_v = sum(1 for v in bm.verts if not v.link_faces)
    loose_e = sum(1 for e in bm.edges if not e.link_faces)
    degenerate = sum(1 for f in bm.faces if f.calc_area() < 1e-7)
    nonmanifold = sum(1 for e in bm.edges if not e.is_manifold)
    bm.free()
    check("no loose geometry", loose_v == 0 and loose_e == 0 and degenerate == 0,
          f"{loose_v} loose vertices, {loose_e} loose edges, {degenerate} zero-area faces")
    if nonmanifold:
        warnings.append(f"{nonmanifold} open (non-manifold) edges")

    bad = []
    for slot in me.materials:
        name = slot.name[4:] if slot and slot.name.startswith("pal.") else None
        if name not in palette.PALETTE:
            bad.append(slot.name if slot else "<empty slot>")
            continue
        want_c = palette.srgb_to_linear(palette.PALETTE[name][0])
        bsdf = next((nd for nd in slot.node_tree.nodes if nd.type == "BSDF_PRINCIPLED"), None) if slot.node_tree else None
        got = tuple(bsdf.inputs["Base Color"].default_value)[:3] if bsdf else None
        linked = bsdf is not None and bsdf.inputs["Base Color"].is_linked
        if got is None or linked or max(abs(a - b) for a, b in zip(got, want_c)) > 1e-4:
            bad.append(f"{slot.name} (colour changed or driven by nodes)")
    check("palette materials only", not bad and len(me.materials) > 0, ", ".join(bad) or f"{len(me.materials)} materials: " + ", ".join(m.name for m in me.materials))
    used = {p.material_index for p in me.polygons}
    unused = [me.materials[k].name for k in range(len(me.materials)) if k not in used]
    if unused:
        warnings.append("unused material slots: " + ", ".join(unused))
    check("UV map", len(me.uv_layers) > 0, f"{len(me.uv_layers)} UV maps")

    return {"ok": all(c["ok"] for c in checks), "checks": checks, "warnings": warnings,
            "tris": tris, "size": [round(v, 3) for v in size]}


# ---------------------------------------------------------------- review renders

def _review_set(spec):
    """Floor tiles under and around the footprint (the footprint lighter), a low wall stub along
    the back for things that stand against a wall, a sun and a camera. Never exported."""
    c = collection(REVIEW_COLLECTION)
    for o in list(c.objects):
        bpy.data.objects.remove(o)
    fw, fd = spec["footprint"]
    m = Mesh("review_floor")
    for tx in range(-1, fw + 1):
        for ty in range(-1, fd + 1):
            x0, y0 = tx - fw / 2, ty - fd / 2
            in_print = 0 <= tx < fw and 0 <= ty < fd
            m.box((x0 + 0.01, y0 + 0.01, -0.02), (x0 + 0.99, y0 + 0.99, 0.0),
                  "fabric_beige" if in_print else "plastic_grey")
    if spec.get("against_wall"):
        m.box((-fw / 2 - 1, fd / 2 - WALL_MARGIN, 0.0), (fw / 2 + 1, fd / 2 + WALL_MARGIN, 0.25), "sheet_cream")
    floor = m.finish()
    collection(ASSET_COLLECTION).objects.unlink(floor)
    c.objects.link(floor)

    sun_data = bpy.data.lights.new("review_sun", "SUN")
    sun_data.energy = 3.2
    sun_data.angle = math.radians(8)
    sun = bpy.data.objects.new("review_sun", sun_data)
    c.objects.link(sun)
    cam_data = bpy.data.cameras.new("review_camera")
    cam_data.type = "ORTHO"
    cam = bpy.data.objects.new("review_camera", cam_data)
    c.objects.link(cam)

    scene = bpy.context.scene
    scene.camera = cam
    world = bpy.data.worlds.get("review_world") or bpy.data.worlds.new("review_world")
    try:
        world.use_nodes = True
    except Exception:
        pass
    bg = next((nd for nd in world.node_tree.nodes if nd.type == "BACKGROUND"), None)
    if bg:
        bg.inputs["Color"].default_value = (0.42, 0.44, 0.48, 1.0)
        bg.inputs["Strength"].default_value = 0.9
    world.color = (0.42, 0.44, 0.48)
    scene.world = world
    scene.render.engine = "BLENDER_EEVEE"
    scene.render.resolution_x = scene.render.resolution_y = RENDER_PX
    scene.render.resolution_percentage = 100
    scene.render.image_settings.file_format = "PNG"
    scene.render.film_transparent = False
    scene.render.dither_intensity = 0.0  # noise-free flat colours: smaller PNGs
    try:
        scene.eevee.taa_render_samples = 16
    except Exception:
        pass
    scene.view_settings.view_transform = "Standard"
    scene.view_settings.look = "None"
    return cam, sun


def render_angles(spec, out_dir=None, sheet_path=None):
    """Renders the four fixed game angles (orthographic, 30° elevation, yaw 45° + 90° × k, as the
    client camera) to PNGs and, with `sheet_path`, a 2 × 2 sheet of them (angle 0 top left, then
    1, 2, 3). Returns the paths."""
    cam, sun = _review_set(spec)
    obj = asset_objects()[0]
    fw, fd = spec["footprint"]
    pts = [Vector(c) for c in obj.bound_box]
    pts += [Vector((sx * (fw / 2 + 0.3), sy * (fd / 2 + 0.3), 0)) for sx in (-1, 1) for sy in (-1, 1)]
    out_dir = Path(out_dir or tempfile.mkdtemp(prefix=f"zasset_{spec['id']}_"))
    out_dir.mkdir(parents=True, exist_ok=True)
    files = []
    scene = bpy.context.scene
    for k in range(4):
        yaw = math.radians(45 + 90 * k)
        # Client camera offset (sin yaw, cos yaw) in game x/z; game z is Blender -Y.
        d = Vector((math.sin(yaw) * math.cos(ELEVATION), -math.cos(yaw) * math.cos(ELEVATION), math.sin(ELEVATION)))
        rot = d.to_track_quat("Z", "Y")
        right, up = rot @ Vector((1, 0, 0)), rot @ Vector((0, 1, 0))
        xs = [p.dot(right) for p in pts]
        ys = [p.dot(up) for p in pts]
        centre = right * (min(xs) + max(xs)) / 2 + up * (min(ys) + max(ys)) / 2
        cam.rotation_euler = rot.to_euler()
        cam.location = centre + d * 20
        cam.data.ortho_scale = max(max(xs) - min(xs), max(ys) - min(ys)) * 1.12
        cam.data.clip_end = 60
        # Light from the camera's upper left, so every angle reads the same way.
        az = yaw + math.radians(55)
        sd = Vector((math.sin(az) * math.cos(math.radians(50)), -math.cos(az) * math.cos(math.radians(50)), math.sin(math.radians(50))))
        sun.rotation_euler = sd.to_track_quat("Z", "Y").to_euler()
        path = out_dir / f"{spec['id']}_angle{k}.png"
        scene.render.filepath = str(path)
        bpy.ops.render.render(write_still=True)
        files.append(str(path))
    if sheet_path:
        montage(files, sheet_path)
        files.append(str(sheet_path))
    return files


def montage(files, out, cols=2, shrink=1):
    """Tiles PNGs (all the same size) into one, `cols` across, in reading order, each shrunk by the
    whole factor `shrink`, with thin dark lines between them. For the 2 × 2 review sheet, or a
    contact sheet of many assets' sheets to critique at once."""
    import numpy as np
    imgs = []
    for f in files:
        im = bpy.data.images.load(str(f), check_existing=False)
        w, h = im.size
        a = np.empty(w * h * 4, dtype=np.float32)
        im.pixels.foreach_get(a)
        a = a.reshape(h, w, 4)
        if shrink > 1:
            a = a[:h - h % shrink, :w - w % shrink].reshape(h // shrink, shrink, w // shrink, shrink, 4).mean(axis=(1, 3))
        imgs.append(a)
        bpy.data.images.remove(im)
    h, w = imgs[0].shape[:2]
    rows = (len(imgs) + cols - 1) // cols
    sheet = np.full((rows * h, cols * w, 4), 0.15, dtype=np.float32)
    sheet[..., 3] = 1.0
    # Image rows run bottom to top: the first images go at the top.
    for k, a in enumerate(imgs):
        r, c = rows - 1 - k // cols, k % cols
        sheet[r * h + 1:(r + 1) * h - 1, c * w + 1:(c + 1) * w - 1] = a[1:-1, 1:-1]
    out = Path(out)
    out.parent.mkdir(parents=True, exist_ok=True)
    im = bpy.data.images.new("review_sheet", cols * w, rows * h, alpha=False)
    im.pixels.foreach_set(sheet.ravel())
    settings = bpy.context.scene.render.image_settings
    settings.file_format, settings.color_mode, settings.color_depth, settings.compression = "PNG", "RGB", "8", 100
    im.save_render(str(out), scene=bpy.context.scene)
    bpy.data.images.remove(im)
    return str(out)


# ---------------------------------------------------------------- export

def export_glb(path):
    """Exports the Asset collection's mesh alone (no review set, camera or lights) as GLB, Y up,
    with its custom properties (footprint, against_wall) as glTF extras."""
    obj = asset_objects()[0]
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    for o in bpy.context.view_layer.objects:
        o.select_set(False)
    obj.select_set(True)
    bpy.context.view_layer.objects.active = obj
    bpy.ops.export_scene.gltf(
        filepath=str(path), export_format="GLB", use_selection=True, export_apply=True,
        export_yup=True, export_materials="EXPORT", export_texcoords=True, export_normals=True,
        export_cameras=False, export_lights=False, export_extras=True, export_animations=False,
    )
    return str(path)


def save_blend(path, script_text=None, name="build.py"):
    """Saves the open file (compressed), with the build script kept inside it as a text block."""
    if script_text is not None:
        t = bpy.data.texts.get(name) or bpy.data.texts.new(name)
        t.from_string(script_text)
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    bpy.context.preferences.filepaths.save_version = 0  # no .blend1 backups beside the source
    bpy.ops.wm.save_as_mainfile(filepath=str(path), compress=True)
    return str(path)


# ---------------------------------------------------------------- the whole build

class Build:
    """What a build script's `build(a)` gets: the spec and the builder."""

    def __init__(self, spec):
        self.spec = spec
        self.width, self.depth = spec["footprint"]
        self.size = spec["size"]
        # Inner limits of the footprint (walls take WALL_MARGIN off every side).
        self.x0, self.x1 = -self.width / 2 + WALL_MARGIN, self.width / 2 - WALL_MARGIN
        self.y0, self.y1 = -self.depth / 2 + WALL_MARGIN, self.depth / 2 - WALL_MARGIN

    def mesh(self):
        return Mesh(self.spec["id"])


def build(asset, render=True, export=True, save=True, sheet=True):
    """Builds `asset` ("category/id") from scratch: reset, run its script, validate, render the four
    angles, save the .blend and export the GLB. Returns the validation report plus file paths.
    Nothing is exported when validation fails."""
    p = paths(asset)
    spec = load_spec(asset)
    reset_scene()
    source = p["script"].read_text()
    mod_spec = importlib.util.spec_from_file_location(f"zasset_build_{spec['id']}", p["script"])
    mod = importlib.util.module_from_spec(mod_spec)
    dont_write, sys.dont_write_bytecode = sys.dont_write_bytecode, True  # no __pycache__ in assets/
    try:
        mod_spec.loader.exec_module(mod)
    finally:
        sys.dont_write_bytecode = dont_write
    m = mod.build(Build(spec))
    if isinstance(m, Mesh):
        m.finish(spec)
    report = validate(spec)
    report["asset"] = asset
    report["where"] = p["where"]
    if render and report["ok"]:
        report["renders"] = render_angles(spec, sheet_path=p["sheet"] if sheet else None)
    if save:
        report["blend"] = save_blend(p["blend"], source, f"{spec['id']}.py")
    if export and report["ok"]:
        report["glb"] = export_glb(p["glb"])
    return report


def rel(path):
    """A path relative to the game repo, or `art:` and relative to the art repo."""
    p = Path(path)
    art = artrepo.art_root()
    if art is not None and p.is_relative_to(art):
        return "art:" + os.path.relpath(p, art)
    return os.path.relpath(p, REPO) if p.is_relative_to(REPO) else str(p)


def summary(report):
    """One line per check, for printing."""
    lines = [f"{report.get('asset')}: {'OK' if report['ok'] else 'FAILED'}  {report.get('tris')} tris, size {report.get('size')}"]
    for c in report["checks"]:
        lines.append(f"  [{'x' if c['ok'] else ' '}] {c['check']}: {c['detail']}")
    for w in report.get("warnings", []):
        lines.append(f"  warning: {w}")
    return "\n".join(lines)


def to_json(report):
    return json.dumps(report, indent=2, default=str)
