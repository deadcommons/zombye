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

# The scale set on every gate render (docs/asset-pipeline.md, "The review sheet"): a survivor-height
# figure standing in a door opening the size of the game's, beside the asset on the camera's left
# (the sun's shadows fall that way, so neither shades the asset). In the doorway the figure takes no
# width of its own, so small props stay big in the frame.
FIGURE_HEIGHT = 1.78
DOOR_WIDTH, DOOR_HEIGHT = 1.0, 2.1
WALL_T = 0.16
DOOR_JAMB, DOOR_LINTEL = 0.12, 0.2
SCALE_GAP = 0.3  # across the screen, between the asset and the door frame (metres)
SCALE_COLOUR = "plastic_grey"


def scale_figure():
    """A 1.78 m survivor silhouette from faceted boxes and prisms, feet on the origin, facing -Y.
    Ours: no mesh from anywhere else. `render_angles` moves it to the Review collection."""
    m = Mesh("review_figure")
    g = SCALE_COLOUR
    for sx in (-1, 1):
        m.block(sx * 0.085, -0.03, 0.0, 0.11, 0.26, 0.07, g)                                # boot
        m.block(sx * 0.085, 0.0, 0.07, 0.13, 0.15, 0.80, g, top_inset=(-0.01, 0.0))         # leg
        m.block(sx * 0.27, 0.0, 0.80, 0.085, 0.11, 0.62, g, top_inset=(-0.005, -0.005))     # arm
        m.block(sx * 0.27, 0.0, 0.68, 0.075, 0.09, 0.12, g)                                 # hand
    m.block(0.0, 0.0, 0.82, 0.32, 0.18, 0.12, g)                                            # hips
    m.box((-0.2, -0.11, 0.92), (0.2, 0.11, 1.44), g, top_inset=(-0.03, 0.0))                # torso
    m.prism((0.0, 0.0, 1.43), 0.055, 0.11, g, sides=6)                                      # neck
    m.prism((0.0, -0.01, 1.53), 0.105, FIGURE_HEIGHT - 1.53, g, sides=8, radius2=0.095)     # head
    return m


def door_frame():
    """A door opening as the game makes them, 1 m wide and 2.1 m high in a 0.16 m wall: two jambs
    and a lintel, centred on the origin, the wall along X."""
    m = Mesh("review_door")
    hw, t = DOOR_WIDTH / 2, WALL_T / 2
    for sx in (-1, 1):
        x0, x1 = sorted((sx * hw, sx * (hw + DOOR_JAMB)))
        m.box((x0, -t, 0.0), (x1, t, DOOR_HEIGHT), "sheet_cream")
    m.box((-hw - DOOR_JAMB, -t, DOOR_HEIGHT), (hw + DOOR_JAMB, t, DOOR_HEIGHT + DOOR_LINTEL), "sheet_cream")
    return m


def _to_review(m):
    obj = m.finish()
    collection(ASSET_COLLECTION).objects.unlink(obj)
    collection(REVIEW_COLLECTION).objects.link(obj)
    return obj


def _wall_stub(spec):
    """(lo, hi) of the low wall stub along the back of a thing that stands against a wall (the
    footprint's width, on the wall line), else None."""
    if not spec.get("against_wall"):
        return None
    fw, fd = spec["footprint"]
    return (-fw / 2, fd / 2 - WALL_MARGIN, 0.0), (fw / 2, fd / 2 + WALL_MARGIN, 0.25)


def _review_set(spec, cover=()):
    """Floor tiles under and around the footprint (the footprint lighter; out to every rectangle
    (x0, y0, x1, y1) in `cover`, where the scale set stands), a low wall stub along the back for
    things that stand against a wall, a sun and a camera. Never exported."""
    c = collection(REVIEW_COLLECTION)
    for o in list(c.objects):
        bpy.data.objects.remove(o)
    fw, fd = spec["footprint"]
    tx0, tx1, ty0, ty1 = -1, fw, -1, fd
    for x0, y0, x1, y1 in cover:
        tx0, tx1 = min(tx0, math.floor(x0 + fw / 2)), max(tx1, math.floor(x1 + fw / 2))
        ty0, ty1 = min(ty0, math.floor(y0 + fd / 2)), max(ty1, math.floor(y1 + fd / 2))
    m = Mesh("review_floor")
    for tx in range(tx0, tx1 + 1):
        for ty in range(ty0, ty1 + 1):
            x0, y0 = tx - fw / 2, ty - fd / 2
            in_print = 0 <= tx < fw and 0 <= ty < fd
            m.box((x0 + 0.01, y0 + 0.01, -0.02), (x0 + 0.99, y0 + 0.99, 0.0),
                  "fabric_beige" if in_print else "plastic_grey")
    stub = _wall_stub(spec)
    if stub:
        m.box(*stub, "sheet_cream")
    _to_review(m)

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


def _view(k):
    """Game angle `k`: (yaw, direction to the camera, camera rotation, screen right, screen up)."""
    yaw = math.radians(45 + 90 * k)
    # Client camera offset (sin yaw, cos yaw) in game x/z; game z is Blender -Y.
    d = Vector((math.sin(yaw) * math.cos(ELEVATION), -math.cos(yaw) * math.cos(ELEVATION), math.sin(ELEVATION)))
    rot = d.to_track_quat("Z", "Y")
    return yaw, d, rot, rot @ Vector((1, 0, 0)), rot @ Vector((0, 1, 0))


def render_angles(spec, out_dir=None, sheet_path=None, scale=True):
    """Renders the four fixed game angles (orthographic, 30° elevation, yaw 45° + 90° × k, as the
    client camera) to PNGs and, with `sheet_path`, a 2 × 2 sheet of them (angle 0 top left, then
    1, 2, 3). With `scale` (the default, for every gate render) the scale figure stands in the door
    frame beside the asset, on the camera's left at every angle, the figure facing the camera and the
    door in a wall along X (parallel to the asset's back); the camera frames the asset, its footprint
    and them. Without it, the asset alone as before (animation strips). Returns the paths."""
    obj = asset_objects()[0]
    fw, fd = spec["footprint"]
    asset_pts = [Vector(c) for c in obj.bound_box]
    pts = asset_pts + [Vector((sx * (fw / 2 + 0.3), sy * (fd / 2 + 0.3), 0)) for sx in (-1, 1) for sy in (-1, 1)]
    views = [_view(k) for k in range(4)]
    places, cover = [], []
    stub = _wall_stub(spec)
    if stub:  # the scale set keeps clear of the wall stub too
        asset_pts = asset_pts + [Vector((x, y, 0.0)) for x in (stub[0][0], stub[1][0]) for y in (stub[0][1], stub[1][1])]
    if scale:
        hw, t, top = DOOR_WIDTH / 2 + DOOR_JAMB, WALL_T / 2, DOOR_HEIGHT + DOOR_LINTEL
        door_pts = [Vector((sx * hw, sy * t, z)) for sx in (-1, 1) for sy in (-1, 1) for z in (0.0, top)]
        for yaw, d, rot, right, up in views:
            across = Vector((right.x, right.y, 0.0)).normalized()
            near = min(p.dot(across) for p in asset_pts)
            half = max(p.dot(across) for p in door_pts)
            at = across * (near - SCALE_GAP - half)
            places.append(at)
            cover.append((at.x - hw - 0.3, at.y - hw - 0.3, at.x + hw + 0.3, at.y + hw + 0.3))
    cam, sun = _review_set(spec, cover)
    if scale:
        door, figure = _to_review(door_frame()), _to_review(scale_figure())
        fig_pts = [v.co.copy() for v in figure.data.vertices]
    out_dir = Path(out_dir or tempfile.mkdtemp(prefix=f"zasset_{spec['id']}_"))
    out_dir.mkdir(parents=True, exist_ok=True)
    files = []
    scene = bpy.context.scene
    for k, (yaw, d, rot, right, up) in enumerate(views):
        frame = pts
        if scale:
            at = places[k]
            door.location = figure.location = at
            figure.rotation_euler = (0.0, 0.0, yaw)  # facing the camera
            turn = figure.rotation_euler.to_matrix()
            frame = pts + [at + p for p in door_pts] + [at + turn @ p for p in fig_pts]
        xs = [p.dot(right) for p in frame]
        ys = [p.dot(up) for p in frame]
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


# The review renders' sky as it lands in a PNG (display values): what a reference is laid on.
SHEET_BACKDROP = (0.64, 0.66, 0.69)


def montage(files, out, cols=2, shrink=1, fit=False, line=1):
    """Tiles PNGs (all the same size) into one, `cols` across, in reading order, each shrunk by the
    whole factor `shrink`, with dark lines between them (`line` pixels in from each edge). For the
    2 × 2 review sheet, or a contact sheet of many assets' sheets to critique at once. With `fit`,
    images of any size are scaled to fit the first one's tile (aspect kept) and laid on the review
    backdrop, transparent parts too: a reference image beside a review sheet (`gate3_sheet`)."""
    import numpy as np
    imgs = []
    for f in files:
        im = bpy.data.images.load(str(f), check_existing=False)
        w, h = im.size
        if fit and imgs:
            th, tw = imgs[0].shape[:2]
            k = min(tw / w, th / h)
            w, h = max(1, round(w * k)), max(1, round(h * k))
            if (w, h) != tuple(im.size):
                im.scale(w, h)
        a = np.empty(w * h * 4, dtype=np.float32)
        im.pixels.foreach_get(a)
        a = a.reshape(h, w, 4)
        bpy.data.images.remove(im)
        if fit and imgs:
            tile = np.empty((th, tw, 4), dtype=np.float32)
            tile[..., :3] = SHEET_BACKDROP
            tile[..., 3] = 1.0
            y0, x0 = (th - h) // 2, (tw - w) // 2
            alpha = a[..., 3:4]
            tile[y0:y0 + h, x0:x0 + w, :3] = a[..., :3] * alpha + tile[y0:y0 + h, x0:x0 + w, :3] * (1.0 - alpha)
            imgs.append(tile)  # already the (shrunk) first tile's size
            continue
        if shrink > 1:
            a = a[:h - h % shrink, :w - w % shrink].reshape(h // shrink, shrink, w // shrink, shrink, 4).mean(axis=(1, 3))
        imgs.append(a)
    h, w = imgs[0].shape[:2]
    rows = (len(imgs) + cols - 1) // cols
    sheet = np.full((rows * h, cols * w, 4), 0.15, dtype=np.float32)
    sheet[..., 3] = 1.0
    # Image rows run bottom to top: the first images go at the top.
    for k, a in enumerate(imgs):
        r, c = rows - 1 - k // cols, k % cols
        sheet[r * h + line:(r + 1) * h - line, c * w + line:(c + 1) * w - line] = a[line:-line, line:-line]
    out = Path(out)
    out.parent.mkdir(parents=True, exist_ok=True)
    im = bpy.data.images.new("review_sheet", cols * w, rows * h, alpha=False)
    im.pixels.foreach_set(sheet.ravel())
    settings = bpy.context.scene.render.image_settings
    settings.file_format, settings.color_mode, settings.color_depth, settings.compression = "PNG", "RGB", "8", 100
    im.save_render(str(out), scene=bpy.context.scene)
    bpy.data.images.remove(im)
    return str(out)


IMAGE_SUFFIXES = (".png", ".jpg", ".jpeg")


def gate3_refs(spec, folder):
    """What stands beside an asset at Gate 3 (docs/asset-pipeline.md, References), found in the
    asset's `folder`: its approved reference image, the spec's `reference` (a path, or a list, relative
    to the folder), else the only image in `refs/`, else `raw/reference.png` (Meshy's); then the
    newest one or two approved review sheets of its category (`style/approved/<category>/` in the art
    repo, the spec's category in lower case: furniture, street, character), not its own. Only reads.
    Returns (paths, notes)."""
    folder = Path(folder)
    found, notes = [], []
    named = spec.get("reference")
    if named:
        for r in [named] if isinstance(named, str) else named:
            p = folder / r
            if p.is_file():
                found.append(p)
            else:
                notes.append(f"reference {r} isn't there")
    else:
        refs = sorted(p for p in (folder / "refs").glob("*") if p.suffix.lower() in IMAGE_SUFFIXES)
        if len(refs) == 1:
            found.append(refs[0])
        elif refs:
            notes.append(f"{len(refs)} images in refs/: say which is the approved one (`reference` in the spec, or --refs)")
        elif (folder / "raw" / "reference.png").is_file():
            found.append(folder / "raw" / "reference.png")
        else:
            notes.append("no reference image (refs/ is empty)")
    root = artrepo.art_root()
    category = str(spec.get("category", "")).lower()
    approved = root / "style" / "approved" / category if root else None
    own = {spec["id"], folder.name}
    sheets = sorted((p for p in (approved.glob("*") if approved and approved.is_dir() else [])
                     if p.suffix.lower() in IMAGE_SUFFIXES and not any(p.stem.startswith(o) for o in own)),
                    key=lambda p: (-p.stat().st_mtime, p.name))[:2]
    found += sheets
    if not sheets:
        notes.append(f"no approved {category} sheets yet (style/approved/{category}/)")
    return found, notes


def gate3_sheet(sheet, refs, out):
    """The Gate 3 sheet: the asset's 2 × 2 review sheet, then each of `refs` (its reference image,
    approved neighbours' sheets) beside it in one row, every tile the size of the asset's sheet."""
    return montage([sheet, *refs], out, cols=1 + len(refs), fit=True, line=4)  # wider lines between assets


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


def g3_path(sheet):
    """`renders/<name>_iso.png` -> `renders/<name>_g3.png`."""
    sheet = Path(sheet)
    name = sheet.stem[:-4] if sheet.stem.endswith("_iso") else sheet.stem
    return sheet.with_name(f"{name}_g3.png")


def add_gate3(report, spec, folder, sheet, refs=None):
    """Makes the Gate 3 sheet beside `sheet` (`<name>_g3.png`): the asset's sheet, then `refs` if given
    (paths), else what `gate3_refs` finds. Adds `g3` and notes saying what each tile is."""
    notes = report.setdefault("notes", [])
    if refs is None:
        refs, missing = gate3_refs(spec, folder)
        notes += [f"Gate 3: {m}" for m in missing]
    refs = [Path(r).expanduser() for r in refs]
    for r in refs:
        if not r.is_file():
            raise FileNotFoundError(f"Gate 3 reference {r} isn't there")
    report["g3"] = gate3_sheet(sheet, refs, g3_path(sheet))
    notes.append("Gate 3 sheet, left to right: this asset" + "".join(f", {rel(r)}" for r in refs))
    return report["g3"]


def build(asset, render=True, export=True, save=True, sheet=True, out_dir=None, g3=False, refs=None):
    """Builds `asset` ("category/id") from scratch: reset, run its script, validate, render the four
    angles (with the scale figure and door frame), save the .blend and export the GLB. Returns the
    validation report plus file paths. Nothing is exported when validation fails. `out_dir` writes the
    sheet, .blend and GLB there instead of the work folder (a scratch build; spec and script are only
    read). `g3` (or `refs`, a list of image paths to put beside it instead of the ones found) adds the
    Gate 3 sheet, `<id>_g3.png` beside the review sheet."""
    p = dict(paths(asset))
    if out_dir:
        out = Path(out_dir).expanduser()
        for key in ("sheet", "blend", "glb"):
            p[key] = out / p[key].name
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
    report["notes"] = []
    if render and report["ok"]:
        report["renders"] = render_angles(spec, sheet_path=p["sheet"] if sheet or g3 or refs else None)
        if sheet or g3 or refs:
            report["sheet"] = str(p["sheet"])
        if g3 or refs:
            add_gate3(report, spec, p["spec"].parent, p["sheet"], refs)
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
