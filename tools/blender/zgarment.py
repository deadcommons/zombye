"""Modular characters and wearables, run inside Blender (5.x): a base body cleaned up with `zchar`, and
garments and accessories ("wearables") as separate meshes on the same skeleton, interchangeable on any
character built on that body (survivors, and later the zombies that reuse it). See
`.claude/skills/meshy-assets/SKILL.md` (Wearables).

## The wearable format

Every wearable is authored against the base body's rest pose and is one of two kinds:

- **Skinned** (clothes, scarves, gloves): a mesh skinned to the rig, weights transferred from the base
  body (Data Transfer, nearest face interpolated, then cleaned, ≤ 4 bones). Hoodie, t-shirt, jeans.
- **Rigid** (glasses, hats, held things, bags): a mesh made in the frame of a named **socket** and bound
  wholly (weight 1) to the socket's bone, so it moves with that bone and can be re-placed on another
  body's socket.

**Sockets** are named attachment points on the rig, in the body's spec: a bone, an `offset` in metres from
the bone's head and a `rotation` in degrees (XYZ Euler), both on the armature's axes in the rest pose
(Blender: +X the character's left, -Y forward, +Z up). `socket_matrix` gives the rest-pose frame.
The standard set: `eyes` (glasses), `head_top` (hats), `neck` (scarves), `hand_r` / `hand_l` (held
items), `back` (bags).

A body's spec lists its `sockets` and its `wearables`; each wearable entry gives

    (id: "glasses", item: None, kind: Rigid, socket: Some("eyes"), layer: None,
     occupies: [face], hides: [], tris: 100)

- `id` (the mesh is `<body id>_<id>`, the file `export/wearables/<id>.glb`), `item`: the id in items.ron
  (None: no item yet);
- `kind`: `Skinned` or `Rigid`; `socket`: the socket a rigid one sits on;
- `layer`: the clothing layer (items::Layer: Base, Mid, Outer; None for accessories);
- `occupies`: the slots it takes (body regions Head, Torso, Arms, Hands, Legs, Feet, or the accessory
  slots face, head_top, neck, hand_r, hand_l, back): two wearables can't share a slot on one layer;
- `hides`: the body regions the game hides while it's worn (what it visibly covers);
- `tris`: its triangle budget.

**Exports:** `export/<id>_body.glb` (the skeleton, the body and every clip), `export/wearables/<id>.glb`
per wearable (the skeleton and that mesh alone, bound to the joints by bone name, no clips; its spec entry,
the socket's bone and offset and the mesh's provenance in the mesh node's glTF extras), and
`export/<id>.glb`, everything together as a preview. The body carries a `_REGION` vertex attribute (the
index in Head, Torso, Arms, Hands, Legs, Feet) so the game can hide what a wearable covers.

## What this adds to `zchar`

- clips from other GLBs of the same rig merged onto the one armature (`merge_clip`), root motion
  taken out (`strip_root_motion`), a clip cut to one cycle (`one_cycle`) or rebuilt from a calm stretch
  (`resample_clip`);
- shading by the spec (`shade`: "flat", or smooth with edges sharper than an angle hard);
- body regions (`region_groups`): one vertex group per `items::BodyPart` (`region_Head`, ...) and the
  `_region` vertex attribute;
- wearables: a shell over some regions of the body (`shell_from_regions`), boundary loops (`lip`),
  surface helpers (`on_surface`, `nearest_on`, `tube`), weights from the body (`transfer_weights`),
  sockets (`socket_matrix`, `bind_to_socket`), the poke-through check over the clips (`poke_through`);
- validation of the body, the sockets and every wearable (`validate`), renders with and without the
  wearables (`render_sheet`, `render_strip`), the exports (`export_glb`, `export_body_glb`,
  `export_wearable_glb`), and the whole build (`build`, which `build.py` calls for a character whose
  spec lists `wearables`).

A modular character's spec adds to a zchar spec:

    shading: 30.0,                       // "flat", or auto-smooth: edges sharper than this (degrees) hard
    regions: (Head: ["Head", "neck"], Torso: [...], ...),   // BodyPart -> the bones whose vertices it holds
    sockets: (eyes: (bone: "Head", offset: (0.0, -0.14, 0.11), rotation: (0.0, 0.0, 0.0)), ...),
    wearables: [(id: "hoodie", item: Some("hoodie"), kind: Skinned, socket: None, layer: Some(Mid), ...), ...],

Conventions as `zasset`/`zchar`: 1 unit = 1 m, Z up, facing south (-Y), feet at the origin.
"""

import importlib.util
import math
import sys
from pathlib import Path

import bmesh
import bpy
from mathutils import Vector
from mathutils.bvhtree import BVHTree

sys.path.insert(0, str(Path(__file__).resolve().parent))
import palette  # noqa: E402
import zasset  # noqa: E402
import zchar  # noqa: E402

# The game's body regions (crates/sim/src/items.rs, BodyPart), in order: `_region` holds the index.
REGIONS = ("Head", "Torso", "Arms", "Hands", "Legs", "Feet")
REGION_PREFIX = "region_"
REGION_ATTRIBUTE = "_region"  # glTF exports custom attributes whose names start with "_"
MAX_INFLUENCES = zchar.MAX_INFLUENCES


def region_group(region):
    return f"{REGION_PREFIX}{region}"


SLOTS = REGIONS + ("face", "head_top", "neck", "hand_r", "hand_l", "back")
KINDS = ("Skinned", "Rigid")
LAYERS = ("Base", "Mid", "Outer")


def wearables(spec):
    """The spec's wearables (garments and accessories)."""
    return list(spec.get("wearables") or spec.get("garments") or [])


def is_modular(folder):
    """A character folder whose spec lists wearables (built here rather than by `zchar.build`)."""
    try:
        return bool(wearables(zchar.load_spec(folder)))
    except Exception:
        return False


def socket_matrix(arm, spec, name):
    """The rest-pose frame of socket `name` in world space (= armature space: transforms are applied):
    its bone's head plus `offset`, turned by `rotation` (degrees, XYZ), on the armature's axes."""
    from mathutils import Euler, Matrix
    s = spec["sockets"][name]
    bone = arm.data.bones[s["bone"]]
    pos = arm.matrix_world @ (bone.head_local + Vector(s["offset"]))
    rot = Euler([math.radians(a) for a in s.get("rotation", (0, 0, 0))], "XYZ").to_matrix().to_4x4()
    return Matrix.Translation(pos) @ rot


def bind_to_socket(obj, arm, spec, name):
    """Binds a rigid wearable to socket `name`: every vertex to the socket's bone (weight 1), and the
    socket kept on the object (`socket`, `socket_bone`, `socket_offset`, `socket_rotation`) for the
    export's extras."""
    s = spec["sockets"][name]
    bind_rigid(obj, arm, s["bone"])
    obj["socket"] = name
    obj["socket_bone"] = s["bone"]
    obj["socket_offset"] = [float(v) for v in s["offset"]]
    obj["socket_rotation"] = [float(v) for v in s.get("rotation", (0, 0, 0))]


# ---------------------------------------------------------------- clips

def _purge_orphans():
    for coll in (bpy.data.meshes, bpy.data.materials, bpy.data.images, bpy.data.armatures):
        for d in list(coll):
            if d.users == 0:
                coll.remove(d)


def merge_clip(arm, path, name):
    """Imports the GLB at `path` (the same rig with another clip, e.g. Meshy's free walk and run), keeps
    its action as `name` on `arm`'s slot (so it plays on `arm`), and drops everything else it brought.
    Returns the action."""
    before_objs, before_actions = set(bpy.data.objects), set(bpy.data.actions)
    bpy.ops.import_scene.gltf(filepath=str(path))
    new_actions = [a for a in bpy.data.actions if a not in before_actions]
    if len(new_actions) != 1:
        raise ValueError(f"{path}: expected one action, got {[a.name for a in new_actions]}")
    action = new_actions[0]
    action.name = name
    for slot in action.slots:
        slot.name_display = arm.name  # the identifier becomes OB<arm name>: assignable to `arm`
    action.use_fake_user = True
    for o in [o for o in bpy.data.objects if o not in before_objs]:
        bpy.data.objects.remove(o)
    _purge_orphans()
    return action


def _hips(arm):
    return next(b for b in arm.pose.bones if b.parent is None)


def strip_root_motion(arm, action, threshold=0.01):
    """Takes the root bone's travel across the floor out of `action`: its horizontal move from the
    first key to the last, spread evenly over the clip, so the clip ends where it began and loops on
    the spot. The sway and the bob up and down stay. A clip that travels less than `threshold`
    metres is already in place and left alone. Returns (metres travelled, metres per second)."""
    root = _hips(arm)
    path = f'pose.bones["{root.name}"].location'
    fcs = {fc.array_index: fc for fc in zchar.action_fcurves(action) if fc.data_path == path}
    if len(fcs) != 3:
        return 0.0, 0.0
    # Bone-local location -> world: the root has no parent, so the armature's and its rest matrix map it.
    world = arm.matrix_world.to_3x3() @ root.bone.matrix_local.to_3x3()
    t0, t1 = action.frame_range
    at = lambda t: world @ Vector([fcs[i].evaluate(t) for i in range(3)])  # noqa: E731
    travel = at(t1) - at(t0)
    travel.z = 0.0
    fps = bpy.context.scene.render.fps / bpy.context.scene.render.fps_base
    metres = travel.length
    speed = metres / max((t1 - t0) / fps, 1e-6)
    if metres < threshold:
        return metres, speed
    local = world.inverted() @ travel
    for i, fc in fcs.items():
        for k in fc.keyframe_points:
            c = local[i] * (k.co.x - t0) / (t1 - t0)
            k.co.y -= c
            k.handle_left.y -= c
            k.handle_right.y -= c
        fc.update()
    return metres, speed


def _bind_action(arm, action):
    ad = arm.animation_data or arm.animation_data_create()
    ad.action = action
    if getattr(ad, "action_slot", None) is None and action.slots:
        ad.action_slot = action.slots[0]
    return ad


def pose_error(arm, action, fa, fb):
    """Mean distance (armature units) between every bone's head, relative to the root, at frames `fa`
    and `fb` of `action`: how well a loop closes."""
    ad = _bind_action(arm, action)
    scene = bpy.context.scene
    root = _hips(arm)
    pts = []
    for f in (fa, fb):
        scene.frame_set(int(f), subframe=f - int(f))
        pts.append([(b.head - root.head).copy() for b in arm.pose.bones])
    ad.action = None
    return sum((a - b).length for a, b in zip(*pts)) / len(pts[0])


def one_cycle(arm, action, frames=None):
    """Plays only one cycle of `action`: its manual frame range set to the first `frames` (by
    default `zchar.loop_frames`) and marked cyclic; the exporter samples that range. The keys are kept.
    Returns (frames, how well that cycle closes in metres)."""
    if frames is None:
        frames, _ = zchar.loop_frames(arm, action.name)
    f0 = action.frame_range[0]
    action.use_frame_range = True
    action.frame_start, action.frame_end = f0, f0 + frames
    action.use_cyclic = True
    return frames, pose_error(arm, action, f0, f0 + frames)


def set_cyclic(action):
    """Marks a clip that already is one cycle as such (its own frame range, cyclic)."""
    f0, f1 = action.frame_range
    action.use_frame_range = True
    action.frame_start, action.frame_end = f0, f1
    action.use_cyclic = True


def resample_clip(arm, action, source, frames):
    """Rebuilds `action` from a stretch of itself: frame n of `frames` plays source time
    `source(n / frames)` (in the action's own frames; times past its end wrap round, for a clip that
    loops). With a source that comes back to where it started (e.g. there and back along a calm
    stretch, eased at the turns) the result loops seamlessly. Every channel is keyed each frame."""
    f0, f1 = action.frame_range
    span = f1 - f0
    fcs = zchar.action_fcurves(action)

    def wrap(t):
        return f0 + ((t - f0) % span)

    values = {fc: [fc.evaluate(wrap(source(n / frames))) for n in range(frames + 1)] for fc in fcs}
    action.use_frame_range = False
    for fc, vals in values.items():
        fc.keyframe_points.clear()
        fc.keyframe_points.add(len(vals))
        for n, v in enumerate(vals):
            k = fc.keyframe_points[n]
            k.co = (n, v)
            k.interpolation = "LINEAR"
        fc.update()
    set_cyclic(action)
    return action


def fix_slots(arm):
    """Every NLA strip on `arm` plays its action's slot (Blender 5's slotted actions)."""
    ad = arm.animation_data
    for t in (ad.nla_tracks if ad else []):
        for s in t.strips:
            if s.action and getattr(s, "action_slot", None) is None and s.action.slots:
                s.action_slot = s.action.slots[0]


# ---------------------------------------------------------------- pose layers (a zombie's stoop)

def layer_quaternion(arm, bone, degrees):
    """A pose layer's rotation for `bone`: `degrees` (XYZ Euler, on the armature's axes, about the
    bone's head, as sockets are given) as a rotation in the bone's own rest frame, the form a pose
    bone's quaternion takes."""
    from mathutils import Euler
    rest = arm.data.bones[bone].matrix_local.to_3x3()
    world = Euler([math.radians(a) for a in degrees], "XYZ").to_matrix()
    return (rest.inverted() @ world @ rest).to_quaternion()


def apply_pose_layer(arm, action, layer, weight=None):
    """Adds the pose layer `layer` ({bone: (x, y, z) degrees}, as `layer_quaternion`) over `action`:
    each bone's rotation key q becomes slerp(1, layer, w) @ q, so the layer turns the bone relative
    to its (animated) parent whatever the clip does with it. `weight(frame) -> 0..1` scales it per
    key (e.g. fading a stoop out while the body lies on the ground); None: 1 everywhere. The keys keep
    their times; the four channels are keyed together at every time any of them has a key."""
    from mathutils import Quaternion
    by_path = {}
    for fc in zchar.action_fcurves(action):
        by_path.setdefault(fc.data_path, {})[fc.array_index] = fc
    for bone, degrees in layer.items():
        fcs = by_path.get(f'pose.bones["{bone}"].rotation_quaternion')
        if not fcs or len(fcs) != 4:
            raise ValueError(f"{action.name}: {bone} has no rotation keys to layer onto")
        add = layer_quaternion(arm, bone, degrees)
        times = sorted({round(k.co.x, 4) for fc in fcs.values() for k in fc.keyframe_points})
        out, prev = [], None
        for t in times:
            q = Quaternion([fcs[i].evaluate(t) for i in range(4)])
            w = 1.0 if weight is None else max(0.0, min(1.0, float(weight(t))))
            r = Quaternion().slerp(add, w) @ q
            if prev is not None and r.dot(prev) < 0:
                r.negate()
            out.append(r)
            prev = r
        for i in range(4):
            fc = fcs[i]
            fc.keyframe_points.clear()
            fc.keyframe_points.add(len(times))
            for k, (t, r) in zip(fc.keyframe_points, zip(times, out)):
                k.co = (t, r[i])
                k.interpolation = "LINEAR"
            fc.update()


def set_pose_layer(arm, layer):
    """Poses `arm` (no action) in its rest pose with `layer` on: for renders of the rest pose with a
    stoop. `layer` None or empty clears every bone back to the rest pose."""
    from mathutils import Quaternion
    ad = arm.animation_data
    if ad:
        ad.action = None
    for pb in arm.pose.bones:
        pb.rotation_mode = "QUATERNION"
        pb.location = (0.0, 0.0, 0.0)
        pb.scale = (1.0, 1.0, 1.0)
        pb.rotation_quaternion = layer_quaternion(arm, pb.name, layer[pb.name]) if layer and pb.name in layer else Quaternion()
    bpy.context.view_layer.update()


# ---------------------------------------------------------------- shading

def shade(obj, shading):
    """`"flat"`: every face one plane. A number: smooth, with edges sharper than that many degrees
    hard (auto-smooth). Imported custom normals are dropped either way, so the rule decides."""
    me = obj.data
    if me.has_custom_normals:
        zchar._select([obj], obj)
        bpy.ops.mesh.customdata_custom_splitnormals_clear()
    if shading == "flat":
        me.shade_flat()
    else:
        me.shade_smooth()
        me.set_sharp_from_angle(angle=math.radians(float(shading)))
    me.update()


def shading_ok(obj, shading):
    """(ok, detail): the mesh follows the spec's shading."""
    me = obj.data
    smooth = sum(1 for p in me.polygons if p.use_smooth)
    if me.has_custom_normals:
        return False, "custom normals left"
    if shading == "flat":
        return smooth == 0, f"{smooth} smooth faces of {len(me.polygons)}"
    sharp_attr = me.attributes.get("sharp_edge")
    sharp = sum(1 for d in sharp_attr.data if d.value) if sharp_attr else 0
    # Re-derive the hard edges by angle and compare.
    want = 0
    limit = math.radians(float(shading))
    bm = bmesh.new()
    bm.from_mesh(me)
    for e in bm.edges:
        if len(e.link_faces) == 2 and e.calc_face_angle(0.0) > limit + 1e-4:
            want += 1
    bm.free()
    ok = smooth == len(me.polygons) and sharp >= want
    return ok, f"smooth, {sharp} hard edges (sharper than {float(shading):g}°: {want})"


# ---------------------------------------------------------------- body regions

def _image_sampler(img):
    import numpy as np
    w, h = img.size
    a = np.empty(w * h * 4, dtype=np.float32)
    img.pixels.foreach_get(a)
    a = a.reshape(h, w, 4)

    def sample(uv):
        x = min(w - 1, max(0, int(uv[0] % 1.0 * w)))
        y = min(h - 1, max(0, int(uv[1] % 1.0 * h)))
        return a[y, x, :3]
    return sample


def vertex_colours(obj):
    """Each vertex's base colour (display values, the mean over its corners' UVs)."""
    import numpy as np
    me = obj.data
    sample = _image_sampler(zchar.base_color_image(obj))
    uv = me.uv_layers.active.data
    acc = np.zeros((len(me.vertices), 3))
    cnt = np.zeros(len(me.vertices))
    for loop in me.loops:
        acc[loop.vertex_index] += sample(uv[loop.index].uv)
        cnt[loop.vertex_index] += 1
    return acc / np.maximum(cnt, 1)[:, None]


def dominant_bones(obj):
    """Each vertex's heaviest bone (by name), or None."""
    names = {g.index: g.name for g in obj.vertex_groups}
    out = []
    for v in obj.data.vertices:
        gs = [g for g in v.groups if not names[g.group].startswith(REGION_PREFIX)]
        out.append(names[max(gs, key=lambda g: g.weight).group] if gs else None)
    return out


def region_groups(obj, regions, split=None):
    """One vertex group per body region, every vertex in exactly one (weight 1): by its heaviest bone
    (`regions`: region -> bone names), then `split(vertex index, colour, region)` may move it (e.g. the
    hips' vertices: shirt colour to Torso, trouser colour to Legs). Also writes the `_region` point
    attribute (the index in `REGIONS`, as a float: glTF's custom attributes are floats) that the GLB
    carries. Returns {region: vertex count}."""
    bone_region = {b: r for r, bones in regions.items() for b in bones}
    dom = dominant_bones(obj)
    cols = vertex_colours(obj) if split else None
    assign = []
    for i, b in enumerate(dom):
        r = bone_region.get(b)
        if r is None:
            raise ValueError(f"vertex {i}: bone {b} is in no region")
        if split:
            r = split(i, cols[i], r)
        assign.append(r)
    for g in [g for g in obj.vertex_groups if g.name.startswith(REGION_PREFIX)]:
        obj.vertex_groups.remove(g)
    groups = {r: obj.vertex_groups.new(name=region_group(r)) for r in REGIONS}
    for r in REGIONS:
        idx = [i for i, a in enumerate(assign) if a == r]
        if idx:
            groups[r].add(idx, 1.0, "REPLACE")
    me = obj.data
    if REGION_ATTRIBUTE in me.attributes:
        me.attributes.remove(me.attributes[REGION_ATTRIBUTE])
    attr = me.attributes.new(REGION_ATTRIBUTE, "FLOAT", "POINT")
    attr.data.foreach_set("value", [float(REGIONS.index(a)) for a in assign])
    return {r: assign.count(r) for r in REGIONS}


def region_of_vertices(obj):
    """Vertex index -> region name, from the region groups."""
    names = {g.index: g.name[len(REGION_PREFIX):] for g in obj.vertex_groups if g.name.startswith(REGION_PREFIX)}
    out = {}
    for v in obj.data.vertices:
        for g in v.groups:
            if g.group in names and g.weight > 0.5:
                out[v.index] = names[g.group]
    return out


# ---------------------------------------------------------------- garments

def rest_mesh_copy(obj, name):
    """A new mesh object holding `obj`'s rest-pose geometry (no modifiers, weights or materials),
    in the Asset collection."""
    me = obj.data.copy()
    me.name = name
    new = bpy.data.objects.new(name, me)
    zasset.collection(zasset.ASSET_COLLECTION).objects.link(new)
    new.vertex_groups.clear()
    me.materials.clear()
    for a in [a.name for a in me.attributes if a.name == REGION_ATTRIBUTE]:
        me.attributes.remove(me.attributes[a])
    return new


def shell_from_regions(body, regions, name, keep=0.5):
    """A copy of the body's faces whose vertices are mostly (`keep`) in `regions`: the start of a
    garment shell. Returns the new object (no weights, materials or UVs to speak of yet)."""
    reg = region_of_vertices(body)
    shell = rest_mesh_copy(body, name)
    bm = bmesh.new()
    bm.from_mesh(shell.data)
    drop = [f for f in bm.faces if sum(1 for v in f.verts if reg.get(v.index) in regions) / len(f.verts) < keep]
    bmesh.ops.delete(bm, geom=drop, context="FACES")
    loose = [v for v in bm.verts if not v.link_faces]
    bmesh.ops.delete(bm, geom=loose, context="VERTS")
    bm.to_mesh(shell.data)
    bm.free()
    for uv in list(shell.data.uv_layers):
        shell.data.uv_layers.remove(uv)
    return shell


def boundary_loops(bm):
    """The open boundaries of a bmesh as ordered vertex loops (lists of BMVert)."""
    edges = {e for e in bm.edges if e.is_boundary}
    loops = []
    while edges:
        e = edges.pop()
        loop = [e.verts[0], e.verts[1]]
        while True:
            v = loop[-1]
            nxt = next((x for x in v.link_edges if x in edges), None)
            if nxt is None:
                break
            edges.discard(nxt)
            w = nxt.other_vert(v)
            if w is loop[0]:
                break
            loop.append(w)
        loops.append(loop)
    return loops


def lip(bm, loop, target, inset=None):
    """Extrudes boundary `loop` into a band: each new vertex at `target(vertex) -> Vector`. A hem,
    a cuff or a neckband that closes the gap to the body. Returns the new loop."""
    new = [bm.verts.new(target(v)) for v in loop]
    n = len(loop)
    for i in range(n):
        a, b = loop[i], loop[(i + 1) % n]
        na, nb = new[i], new[(i + 1) % n]
        try:
            bm.faces.new((a, b, nb, na))
        except ValueError:
            pass
    return new


def surface_tree(obj):
    """A BVH tree of `obj`'s rest-pose mesh (object space = world: transforms are applied), to put
    details on its surface."""
    bm = bmesh.new()
    bm.from_mesh(obj.data)
    tree = BVHTree.FromBMesh(bm)
    bm.free()
    return tree


def on_surface(tree, origin, direction, lift=0.0):
    """Where a ray from `origin` along `direction` first meets the surface, moved `lift` back toward
    the ray's origin (out of the surface); None if it misses."""
    d = Vector(direction).normalized()
    hit, normal, _, _ = tree.ray_cast(Vector(origin), d)
    return None if hit is None else hit - d * lift


def nearest_on(tree, point, lift=0.0):
    """The nearest surface point to `point`, moved `lift` out along the surface normal."""
    loc, normal, _, _ = tree.find_nearest(Vector(point))
    return loc + normal * lift


def tube(bm, path, radius, sides, mat, up=Vector((0.0, 0.0, 1.0)), cap=True):
    """A low-poly tube along `path` (points), each ring of `sides` vertices around it at
    `radius(index)`, rings square to the path and turned so a vertex points `up`. Returns the faces."""
    rings, faces = [], []
    n = len(path)
    for i, p in enumerate(path):
        a = path[min(i + 1, n - 1)] - path[max(i - 1, 0)]
        a.normalize()
        u = (up - a * up.dot(a)).normalized()
        w = a.cross(u)
        r = radius(i)
        rings.append([bm.verts.new(p + (u * math.cos(2 * math.pi * k / sides) + w * math.sin(2 * math.pi * k / sides)) * r)
                      for k in range(sides)])
    for i in range(n - 1):
        for k in range(sides):
            k2 = (k + 1) % sides
            f = bm.faces.new((rings[i][k], rings[i][k2], rings[i + 1][k2], rings[i + 1][k]))
            f.material_index = mat
            faces.append(f)
    if cap:
        for ring, rev in ((rings[0], True), (rings[-1], False)):
            f = bm.faces.new(list(reversed(ring)) if rev else ring)
            f.material_index = mat
            faces.append(f)
    # Outward: each side face away from the path.
    for f in faces[:(n - 1) * sides]:
        c = f.calc_center_median()
        i = min(range(n), key=lambda j: (path[j] - c).length)
        if f.normal.dot(c - path[i]) < 0:
            f.normal_flip()
    return faces


def fix_normals_outward(obj, centre_of):
    """Turns each face of a shell to face away from `centre_of(face centre) -> Vector` (the body's
    axis there). Returns how many it flipped."""
    bm = bmesh.new()
    bm.from_mesh(obj.data)
    flip = [f for f in bm.faces if f.normal.dot(f.calc_center_median() - centre_of(f.calc_center_median())) < 0]
    bmesh.ops.reverse_faces(bm, faces=flip)
    bm.to_mesh(obj.data)
    bm.free()
    return len(flip)


def skin(obj, arm):
    """Parents `obj` to `arm` with an Armature modifier (the weights are its vertex groups)."""
    obj.parent = arm
    obj.matrix_parent_inverse.identity()
    mod = next((m for m in obj.modifiers if m.type == "ARMATURE"), None) or obj.modifiers.new("Armature", "ARMATURE")
    mod.object = arm


def transfer_weights(body, garment, arm, clean=0.01):
    """The body's bone weights onto `garment` (Data Transfer, nearest face interpolated) in the rest
    pose, then cleaned: weights under `clean` dropped, at most 4 bones, normalised. Skins it to `arm`."""
    pose = arm.data.pose_position
    arm.data.pose_position = "REST"
    bpy.context.view_layer.update()
    garment.vertex_groups.clear()
    for g in body.vertex_groups:
        if not g.name.startswith(REGION_PREFIX):
            garment.vertex_groups.new(name=g.name)
    mod = garment.modifiers.new("weights", "DATA_TRANSFER")
    mod.object = body
    mod.use_vert_data = True
    mod.data_types_verts = {"VGROUP_WEIGHTS"}
    mod.vert_mapping = "POLYINTERP_NEAREST"
    mod.layers_vgroup_select_src = "ALL"  # "BONE_DEFORM" isn't offered here; region groups go after
    mod.layers_vgroup_select_dst = "NAME"
    zchar._select([garment], garment)
    while garment.modifiers[0] != mod:
        bpy.ops.object.modifier_move_up(modifier=mod.name)
    bpy.ops.object.modifier_apply(modifier=mod.name)
    for grp in [x for x in garment.vertex_groups if x.name.startswith(REGION_PREFIX)]:
        garment.vertex_groups.remove(grp)
    bpy.ops.object.vertex_group_clean(group_select_mode="ALL", limit=clean, keep_single=True)
    zchar.limit_weights(garment)
    for g in [g for g in garment.vertex_groups if not any(v for v in garment.data.vertices for x in v.groups if x.group == g.index)]:
        garment.vertex_groups.remove(g)
    arm.data.pose_position = pose
    skin(garment, arm)


def bind_rigid(garment, arm, bone):
    """Every vertex of `garment` to one bone (an accessory that moves with it: glasses on the head)."""
    garment.vertex_groups.clear()
    g = garment.vertex_groups.new(name=bone)
    g.add([v.index for v in garment.data.vertices], 1.0, "REPLACE")
    skin(garment, arm)


def palette_material(obj, *names):
    """Gives `obj` the palette materials `names` in order (slot index = position); faces keep their
    material indices (clearing the slots would reset them)."""
    me = obj.data
    idx = [0] * len(me.polygons)
    me.polygons.foreach_get("material_index", idx)
    me.materials.clear()
    for n in names:
        me.materials.append(zasset.material(n))
    me.polygons.foreach_set("material_index", idx)
    me.update()


def finish_garment(obj, shading):
    """Box-projected UVs (1 m per unit, as zasset) and the spec's shading."""
    bm = bmesh.new()
    bm.from_mesh(obj.data)
    zasset.box_unwrap(bm)
    bm.to_mesh(obj.data)
    bm.free()
    shade(obj, shading)


# ---------------------------------------------------------------- poke-through

def _evaluated(obj, dg):
    ev = obj.evaluated_get(dg)
    me = ev.to_mesh()
    verts = [ev.matrix_world @ v.co for v in me.vertices]
    polys = [tuple(p.vertices) for p in me.polygons]
    ev.to_mesh_clear()
    return verts, polys


PART_ATTRIBUTE = "garment_part"  # face attribute: 0 the cover (the shell over the body), else a detail


def poke_through(body, garment, arm, actions, hidden, samples=12, near=0.05, tolerance=0.004):
    """Checks a garment against the body over the clips, at `samples` frames of each action, against
    the rest pose (where the garment was made to fit):

    - each body vertex near the garment's cover (its faces with `garment_part` 0, or all faces
      without that attribute) is under it at rest or not. In a pose, a vertex that was under the cover
      and is now outside it by more than `tolerance` (nearest to a face that isn't on an open edge)
      *pokes out* (a thigh through the hem); one that wasn't and is now inside *clips in* (the chin
      into the collar, a hand into the belly). Counted by region, split into regions the garment
      hides (only matter if the game shows the body under it) and visible ones (always matter);
    - `sunk`: garment vertices inside the body by more than `tolerance` (the glasses into the face).

    Returns {action: {"out_visible": (n, frame, {region: n}), "in_visible": (n, frame, {region: n}),
    "out_hidden": n, "sunk": (n, frame)}}, each the worst frame of the clip, plus "rest" (sunk only)."""
    reg = region_of_vertices(body)
    me = garment.data
    part = me.attributes.get(PART_ATTRIBUTE)
    cover = [p.index for p in me.polygons if part is None or part.data[p.index].value == 0]
    bm = bmesh.new()
    bm.from_mesh(me)
    bm.faces.ensure_lookup_table()
    edge_faces = {f.index for f in bm.faces if any(e.is_boundary for e in f.edges)}
    bm.free()
    scene = bpy.context.scene
    ad = arm.animation_data or arm.animation_data_create()
    saved = ad.action

    def sides():
        """Per body vertex: +1 outside the cover, -1 under it, 0 not near an inner face; and the
        garment vertices sunk into the body."""
        dg = bpy.context.evaluated_depsgraph_get()
        gv, gp = _evaluated(garment, dg)
        bv, bp = _evaluated(body, dg)
        side = [0] * len(bv)
        if cover:
            tree = BVHTree.FromPolygons(gv, [gp[i] for i in cover])
            for i, p in enumerate(bv):
                loc, normal, fi, dist = tree.find_nearest(p, near)
                if loc is None or cover[fi] in edge_faces:
                    continue
                d = (p - loc).dot(normal)
                side[i] = 1 if d > tolerance else (-1 if d < -tolerance else 0)
        btree = BVHTree.FromPolygons(bv, bp)
        sunk = 0
        for p in gv:
            loc, normal, fi, dist = btree.find_nearest(p, near)
            if loc is not None and (p - loc).dot(normal) < -tolerance:
                sunk += 1
        return side, sunk

    report = {}
    pose = arm.data.pose_position
    arm.data.pose_position = "REST"
    bpy.context.view_layer.update()
    rest, sunk = sides()
    report["rest"] = {"sunk": (sunk, 0)}
    under = [s < 0 for s in rest]
    arm.data.pose_position = "POSE"
    for name in actions:
        action = bpy.data.actions[name]
        _bind_action(arm, action)
        f0, f1 = action.frame_range
        worst = {"out_visible": (0, f0, {}), "in_visible": (0, f0, {}), "out_hidden": 0, "sunk": (0, f0)}
        for k in range(samples):
            f = f0 + (f1 - f0) * k / samples
            scene.frame_set(int(f), subframe=f - int(f))
            side, sunk = sides()
            out_v, in_v, out_h = {}, {}, 0
            for i, s in enumerate(side):
                r = reg.get(i, "?")
                if under[i] and s > 0:
                    if r in hidden:
                        out_h += 1
                    else:
                        out_v[r] = out_v.get(r, 0) + 1
                elif not under[i] and s < 0 and r not in hidden:
                    in_v[r] = in_v.get(r, 0) + 1
            for key, d in (("out_visible", out_v), ("in_visible", in_v)):
                if sum(d.values()) > worst[key][0]:
                    worst[key] = (sum(d.values()), round(f, 1), d)
            worst["out_hidden"] = max(worst["out_hidden"], out_h)
            if sunk > worst["sunk"][0]:
                worst["sunk"] = (sunk, round(f, 1))
        report[name] = worst
    ad.action = saved
    arm.data.pose_position = pose
    scene.frame_set(0)
    return report


def describe_poke(report):
    """One line per clip from `poke_through`."""
    out = []
    for k, v in report.items():
        if k == "rest":
            out.append(f"rest: {v['sunk'][0]} sunk")
            continue
        bits = []
        for key, words in (("out_visible", "poke out"), ("in_visible", "clip in")):
            n, f, where = v[key]
            bits.append(f"{n} {words}" + (f" {where} at frame {f}" if n else ""))
        bits.append(f"{v['out_hidden']} covered poke out")
        bits.append(f"{v['sunk'][0]} sunk" + (f" at frame {v['sunk'][1]}" if v["sunk"][0] else ""))
        out.append(f"{k}: " + ", ".join(bits))
    return "; ".join(out)


# ---------------------------------------------------------------- objects and validation

def objects(spec):
    """(armature, body, {garment id: object}) in the Asset collection."""
    arms, meshes = zchar.character_objects()
    by_name = {o.name: o for o in meshes}
    garments = {g["id"]: by_name.get(f"{spec['id']}_{g['id']}") for g in wearables(spec)}
    return (arms[0] if len(arms) == 1 else None), by_name.get(spec["id"]), garments


def _weights_check(obj, bone_names):
    names = {g.index: g.name for g in obj.vertex_groups}
    unweighted, over, unknown = 0, 0, set()
    for v in obj.data.vertices:
        gs = [g for g in v.groups if g.weight > 1e-4 and not names[g.group].startswith(REGION_PREFIX)]
        if not gs:
            unweighted += 1
        if len(gs) > MAX_INFLUENCES:
            over += 1
        unknown |= {names[g.group] for g in gs if names[g.group] not in bone_names}
    ok = unweighted == 0 and over == 0 and not unknown
    return ok, f"{unweighted} unweighted, {over} over {MAX_INFLUENCES} bones" + (f", groups with no bone: {sorted(unknown)}" if unknown else "")


def _geometry_check(me):
    bm = bmesh.new()
    bm.from_mesh(me)
    loose_v = sum(1 for v in bm.verts if not v.link_faces)
    loose_e = sum(1 for e in bm.edges if not e.link_faces)
    degenerate = sum(1 for f in bm.faces if f.calc_area() < 1e-8)
    bm.free()
    return loose_v == 0 and loose_e == 0 and degenerate == 0, f"{loose_v} loose vertices, {loose_e} loose edges, {degenerate} zero-area faces"


def _identity(o):
    return (o.location.length < 1e-6 and Vector(o.rotation_euler).length < 1e-6
            and o.rotation_quaternion.angle < 1e-6 and (o.scale - Vector((1, 1, 1))).length < 1e-6)


def validate(spec):
    """`zchar.validate`'s checks for the body (its region groups aside), the regions, the shading,
    and for each garment: named, skinned, transforms applied, within its budget, weighted to bones
    only (≤ 4), palette materials with their colours, a UV map, no loose geometry. Returns
    {ok, checks, warnings, tris: {mesh: n}, size}."""
    checks, warnings = [], []

    def check(name, ok, detail):
        checks.append({"check": name, "ok": bool(ok), "detail": detail})

    arm, body, garments = objects(spec)
    extra = [o.name for o in zchar.character_objects()[1] if o is not body and o not in garments.values()]
    check("one armature, the body and the wearables", arm is not None and body is not None and all(garments.values()) and not extra,
          f"armature {'yes' if arm else 'no'}, body {'yes' if body else 'no'}, wearables "
          + ", ".join(f"{k} {'yes' if v else 'no'}" for k, v in garments.items()) + (f", unexpected {extra}" if extra else ""))
    if arm is None or body is None or not all(garments.values()):
        return {"ok": False, "checks": checks, "warnings": warnings}
    me = body.data
    check("named after the spec", arm.name == f"{spec['id']}_rig", f"{body.name}, {arm.name}")
    every = [arm, body, *garments.values()]
    check("skinned to the armature", all(o.find_armature() == arm and o.parent == arm for o in every[1:]),
          ", ".join(f"{o.name} -> {o.parent and o.parent.name}" for o in every[1:]))
    check("transforms applied (1 unit = 1 m)", all(_identity(o) for o in every),
          "; ".join(f"{o.name} loc {tuple(round(v, 4) for v in o.location)} scale {tuple(round(v, 4) for v in o.scale)}" for o in every if not _identity(o)) or "all identity")

    pose_position = arm.data.pose_position
    arm.data.pose_position = "REST"
    bpy.context.view_layer.update()
    co = [v.co for v in me.vertices]
    arm.data.pose_position = pose_position
    lo = Vector((min(c.x for c in co), min(c.y for c in co), min(c.z for c in co)))
    hi = Vector((max(c.x for c in co), max(c.y for c in co), max(c.z for c in co)))
    size = hi - lo
    want = Vector(spec["size"])
    check("feet on the floor", abs(lo.z) < 1e-3, f"lowest point z = {lo.z:.4f}")
    check("height matches the spec", abs(size.z - want.z) <= zchar.HEIGHT_TOLERANCE,
          f"{size.z:.3f} m vs {want.z:.3f} (±{zchar.HEIGHT_TOLERANCE})")
    check("within the spec's width and depth", size.x <= want.x + 1e-3 and size.y <= want.y + 1e-3,
          f"{size.x:.3f} × {size.y:.3f} m within {want.x:.2f} × {want.y:.2f}")
    hx, hy = zchar.hips_xy(arm)
    check("centred on the origin", math.hypot(hx, hy) <= zchar.CENTRE_TOLERANCE, f"hips at ({hx:.3f}, {hy:.3f})")
    bones = arm.data.bones
    toes = [b for b in bones if "toe" in b.name.lower()]
    forward = [b.head_local.y - b.parent.head_local.y for b in toes if b.parent]
    check("facing south (-Y)", forward and all(f < 0 for f in forward),
          f"toes {', '.join(f'{f:+.3f}' for f in forward)} m along y from the ankles")

    tris = {body.name: zchar.tris(body)}
    check("body triangles within budget", tris[body.name] <= spec["tris"], f"{tris[body.name]} / {spec['tris']}")
    ok, detail = _geometry_check(me)
    check("body: no loose geometry", ok, detail)
    bone_names = {b.name for b in bones}
    ok, detail = _weights_check(body, bone_names)
    check("body: every vertex weighted, at most 4 bones", ok, detail)
    try:
        img = zchar.base_color_image(body)
    except (StopIteration, IndexError, AttributeError):
        img = None
    check("body: one textured material", len(me.materials) == 1 and img is not None,
          f"{len(me.materials)} materials, base colour {img.name if img else 'none'} {tuple(img.size) if img else ''}")
    check("body: UV map", len(me.uv_layers) > 0, f"{len(me.uv_layers)} UV maps")

    reg = region_of_vertices(body)
    counts = {r: sum(1 for v in reg.values() if v == r) for r in REGIONS}
    attr = me.attributes.get(REGION_ATTRIBUTE)
    attr_ok = attr is not None and all(REGIONS[int(round(d.value))] == reg.get(i) for i, d in enumerate(attr.data))
    check("body: every vertex in one region (groups and _region agree)",
          len(reg) == len(me.vertices) and all(counts.values()) and attr_ok,
          ", ".join(f"{r} {n}" for r, n in counts.items()) + f"; {len(me.vertices) - len(reg)} in none" + ("" if attr_ok else "; _region differs"))

    shading = spec.get("shading", "flat")
    for o in [body, *garments.values()]:
        ok, detail = shading_ok(o, shading)
        check(f"{o.name}: shading {shading}", ok, detail)

    for g in wearables(spec):
        o = garments[g["id"]]
        t = zchar.tris(o)
        tris[o.name] = t
        check(f"{o.name}: triangles within budget", t <= g["tris"], f"{t} / {g['tris']}")
        ok, detail = _geometry_check(o.data)
        check(f"{o.name}: no loose geometry", ok, detail)
        ok, detail = _weights_check(o, bone_names)
        check(f"{o.name}: every vertex weighted, at most 4 bones", ok, detail)
        bad = []
        for m in o.data.materials:
            name = m.name[4:] if m and m.name.startswith("pal.") else None
            if name not in palette.PALETTE:
                bad.append(m.name if m else "empty slot")
                continue
            want_c = palette.srgb_to_linear(palette.PALETTE[name][0])
            bsdf = next(nd for nd in m.node_tree.nodes if nd.type == "BSDF_PRINCIPLED")
            if max(abs(a - b) for a, b in zip(bsdf.inputs["Base Color"].default_value[:3], want_c)) > 1e-4:
                bad.append(f"{m.name} (colour changed)")
        check(f"{o.name}: palette materials", o.data.materials and not bad,
              ", ".join(m.name for m in o.data.materials) + (f"; not palette: {bad}" if bad else ""))
        check(f"{o.name}: UV map", len(o.data.uv_layers) > 0, f"{len(o.data.uv_layers)} UV maps")
        hides = g.get("hides") or []
        check(f"{o.name}: hides known regions", all(h in REGIONS for h in hides), ", ".join(hides) or "none")
        occ = g.get("occupies") or []
        layer = g.get("layer")
        check(f"{o.name}: wearable entry (kind, layer, slots)",
              g.get("kind") in KINDS and (layer is None or layer in LAYERS) and occ and all(x in SLOTS for x in occ),
              f"kind {g.get('kind')}, layer {layer}, occupies {', '.join(occ) or 'nothing'}")
        if g.get("kind") == "Rigid":
            sock = (spec.get("sockets") or {}).get(g.get("socket") or "")
            bones_used = {o.vertex_groups[x.group].name for v in o.data.vertices for x in v.groups if x.weight > 1e-4}
            check(f"{o.name}: rigid on its socket's bone", sock is not None and bones_used == {sock["bone"]}
                  and all(abs(x.weight - 1.0) < 1e-4 for v in o.data.vertices for x in v.groups),
                  f"socket {g.get('socket')} ({sock and sock['bone']}), weighted to {sorted(bones_used)}")

    sockets = spec.get("sockets") or {}
    check("sockets on known bones", all(v.get("bone") in bone_names and len(v.get("offset", ())) == 3 for v in sockets.values()),
          ", ".join(f"{k} on {v.get('bone')}" for k, v in sockets.items()) or "none")

    ad = arm.animation_data
    clips = [s.action.name for t in (ad.nla_tracks if ad else []) for s in t.strips if s.action]
    want_clips = list(spec.get("animations", []))
    check("animations", all(c in clips for c in want_clips) and clips,
          f"{', '.join(clips) or 'none'}" + (f" (spec: {', '.join(want_clips)})" if want_clips else ""))

    return {"ok": all(c["ok"] for c in checks), "checks": checks, "warnings": warnings,
            "tris": tris, "size": [round(v, 3) for v in size]}


# ---------------------------------------------------------------- renders

def dress(spec, worn):
    """Shows the garments in `worn` (ids) and hides the rest; the body's regions that the worn ones
    hide are masked out, as the game will. Returns the mask modifier (or None)."""
    arm, body, garments = objects(spec)
    hidden = set()
    for g in wearables(spec):
        on = g["id"] in worn
        garments[g["id"]].hide_render = not on
        garments[g["id"]].hide_viewport = not on
        if on:
            hidden |= set(g.get("hides") or [])
    for m in [m for m in body.modifiers if m.type == "MASK"]:
        body.modifiers.remove(m)
    if not hidden:
        return None
    grp = body.vertex_groups.get("hidden_by_garments") or body.vertex_groups.new(name="hidden_by_garments")
    reg = region_of_vertices(body)
    grp.remove([v.index for v in body.data.vertices])
    idx = [i for i, r in reg.items() if r in hidden]
    if idx:
        grp.add(idx, 1.0, "REPLACE")
    mask = body.modifiers.new("hidden_by_garments", "MASK")
    mask.vertex_group = grp.name
    mask.invert_vertex_group = True
    return mask


def undress(spec):
    """Back to the body alone, with no mask (what is saved and validated)."""
    arm, body, garments = objects(spec)
    for m in [m for m in body.modifiers if m.type == "MASK"]:
        body.modifiers.remove(m)
    g = body.vertex_groups.get("hidden_by_garments")
    if g:
        body.vertex_groups.remove(g)
    for o in garments.values():
        o.hide_render = o.hide_viewport = False


def render_sheet(spec, worn, sheet_path):
    """The four game angles (scale figure and door frame) in the rest pose with `worn` on (and the
    spec's `stoop` pose layer, if it has one)."""
    dress(spec, worn)
    arm = objects(spec)[0]
    layer = spec.get("stoop")
    if layer:  # the rest pose stooped, as every clip is
        set_pose_layer(arm, layer)
    try:
        return zchar.render_angles(spec, sheet_path, posed=bool(layer))
    finally:
        undress(spec)
        if layer:
            set_pose_layer(arm, None)


def render_strip(spec, actions, outfits, out_path, frames=6, angle=0):
    """One row per outfit (a list of garment ids each), `frames` poses spread over each action's (one
    cycle) frame range from game angle `angle` (a one-shot's, non-cyclic, from its first frame to its
    last), all at one scale; one PNG per action at `out_path.format(action)`. Returns the paths."""
    arm, body, garments = objects(spec)
    scene = bpy.context.scene
    out = []
    ad = arm.animation_data
    for t in ad.nla_tracks:
        t.mute = True
    for name in actions:
        action = bpy.data.actions[name]
        f0, f1 = action.frame_range
        # A loop's last frame is its first again; a one-shot's last frame is where it ends up.
        span = frames if action.use_cyclic else max(frames - 1, 1)
        poses = [f0 + (f1 - f0) * k / span for k in range(frames)]
        cam, sun = zasset._review_set(spec)
        yaw, d, rot, right, up = zasset._view(angle)
        # Frame every pose of every outfit at one scale: the union of the posed bounds.
        _bind_action(arm, action)
        pts = []
        for f in poses:
            scene.frame_set(int(f), subframe=f - int(f))
            dg = bpy.context.evaluated_depsgraph_get()
            for o in [body, *garments.values()]:
                vs, _ = _evaluated(o, dg)
                pts += vs[::7]
        xs = [p.dot(right) for p in pts]
        ys = [p.dot(up) for p in pts]
        centre = right * (min(xs) + max(xs)) / 2 + up * (min(ys) + max(ys)) / 2
        cam.rotation_euler = rot.to_euler()
        cam.location = centre + d * 20
        cam.data.ortho_scale = max(max(xs) - min(xs), max(ys) - min(ys)) * 1.1
        cam.data.clip_end = 60
        az = yaw + math.radians(55)
        sd = Vector((math.sin(az) * math.cos(math.radians(50)), -math.cos(az) * math.cos(math.radians(50)), math.sin(math.radians(50))))
        sun.rotation_euler = sd.to_track_quat("Z", "Y").to_euler()
        tmp = Path(str(out_path).format(name)).parent / f".strip_{name}"
        tmp.mkdir(parents=True, exist_ok=True)
        files = []
        for row, worn in enumerate(outfits):
            dress(spec, worn)
            for k, f in enumerate(poses):
                scene.frame_set(int(f), subframe=f - int(f))
                scene.render.filepath = str(tmp / f"{row}_{k}.png")
                bpy.ops.render.render(write_still=True)
                files.append(scene.render.filepath)
            undress(spec)
        path = str(out_path).format(name)
        zasset.montage(files, path, cols=frames)
        for f in tmp.glob("*.png"):
            f.unlink()
        tmp.rmdir()
        out.append(path)
    ad.action = None
    scene.frame_set(0)
    return out


def render_head(spec, worn, out_path, scale=0.42):
    """Close-ups of the head with `worn` on, in the rest pose, centred on the `eyes` socket: front,
    side-on (profile, from the character's left), three-quarter from the game's 30° above, and from
    straight above. For checking accessories (glasses standing off the face)."""
    arm, body, garments = objects(spec)
    centre = socket_matrix(arm, spec, "eyes").to_translation() + Vector((0, 0.06, 0.0))
    scene = bpy.context.scene
    cam, sun = zasset._review_set(spec)
    pose = arm.data.pose_position
    arm.data.pose_position = "REST"
    res = scene.render.resolution_x, scene.render.resolution_y
    dress(spec, worn)
    files = []
    tmp = Path(out_path).parent / ".head"
    tmp.mkdir(parents=True, exist_ok=True)
    el = math.radians(30)
    views = [Vector((0, -1, 0.05)), Vector((1, 0, 0.02)), Vector((0.7, -0.7, 0)) * math.cos(el) + Vector((0, 0, math.sin(el))),
             Vector((0.02, -0.25, 1))]
    try:
        for k, d in enumerate(views):
            d = d.normalized()
            cam.rotation_euler = d.to_track_quat("Z", "Y").to_euler()
            cam.location = centre + d * 10
            cam.data.ortho_scale = scale
            sun.rotation_euler = Vector((0.5, -0.6, 0.8)).to_track_quat("Z", "Y").to_euler()
            scene.render.filepath = str(tmp / f"{k}.png")
            bpy.ops.render.render(write_still=True)
            files.append(scene.render.filepath)
        zasset.montage(files, out_path, cols=4)
    finally:
        undress(spec)
        arm.data.pose_position = pose
        scene.render.resolution_x, scene.render.resolution_y = res
        for f in tmp.glob("*.png"):
            f.unlink()
        tmp.rmdir()
    return str(out_path)


# ---------------------------------------------------------------- export

def _gltf(path, objs, arm, animations):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    zchar._select([arm, *objs], arm)
    arm.data.pose_position = "POSE"
    bpy.ops.export_scene.gltf(
        filepath=str(path), export_format="GLB", use_selection=True, export_yup=True,
        export_apply=False, export_skins=True, export_animations=animations, export_animation_mode="ACTIONS",
        export_force_sampling=True, export_frame_step=1, export_def_bones=False,
        export_materials="EXPORT", export_image_format="JPEG", export_jpeg_quality=90,
        export_texcoords=True, export_normals=True, export_attributes=True,
        export_cameras=False, export_lights=False, export_extras=True,
    )
    return str(path)


def tag_for_export(spec, extras=None):
    """Writes the glTF extras onto the objects: on the body the provenance (`extras`), the region map
    and the `_REGION` attribute's meaning, the sockets; on each wearable its spec entry (item, kind,
    socket, layer, occupies, hides), its socket's bone and offset, and its provenance."""
    import json
    arm, body, worn = objects(spec)
    undress(spec)
    for k, v in (extras or {}).items():
        body[k] = v
    body["regions"] = json.dumps({r: list(spec["regions"][r]) for r in REGIONS if r in spec["regions"]})
    body["region_attribute"] = f"{REGION_ATTRIBUTE.upper()} (glTF; {REGION_ATTRIBUTE} in Blender): index into {list(REGIONS)}"
    body["sockets"] = json.dumps(spec.get("sockets") or {})
    for g in wearables(spec):
        o = worn[g["id"]]
        o["wearable"] = g["id"]
        o["item"] = g.get("item") or ""
        o["kind"] = g.get("kind") or ""
        o["layer"] = g.get("layer") or ""
        o["occupies"] = json.dumps(list(g.get("occupies") or []))
        o["hides"] = json.dumps(list(g.get("hides") or []))
        o["body"] = spec["id"]
        if g.get("socket"):
            sock = spec["sockets"][g["socket"]]
            o["socket"] = g["socket"]
            o["socket_bone"] = sock["bone"]
            o["socket_offset"] = json.dumps([float(v) for v in sock["offset"]])
            o["socket_rotation"] = json.dumps([float(v) for v in sock.get("rotation", (0, 0, 0))])
        # Provenance: what the script set (e.g. a shell made from a generated body's faces is derived
        # from generator output), else made in Blender from primitives.
        if "ai_generated" not in o:
            o["ai_generated"] = False
        if "generator" not in o:
            o["generator"] = "Blender (Dead Commons)"
        if extras and "author" in extras:
            o["author"] = extras["author"]
    return arm, body, worn


def export_glb(spec, path, extras=None):
    """The preview: the armature, the body and every wearable as one GLB (separate meshes on one skin),
    every action as a named clip, Y up, the body's texture as JPEG, palette materials as plain colours."""
    arm, body, worn = tag_for_export(spec, extras)
    return _gltf(path, [body, *worn.values()], arm, True)


def export_body_glb(spec, path, extras=None):
    """The body alone: the skeleton, the body mesh (with `_REGION`) and every clip."""
    arm, body, worn = tag_for_export(spec, extras)
    return _gltf(path, [body], arm, True)


def export_wearable_glb(spec, wid, path, extras=None):
    """One wearable alone: the skeleton (for the joints, by bone name) and its mesh, no clips; its spec
    entry, socket and provenance in the mesh node's extras."""
    arm, body, worn = tag_for_export(spec, extras)
    return _gltf(path, [worn[wid]], arm, False)


# ---------------------------------------------------------------- the whole build

def build(folder, render=True, export=True, save=True, from_blend=False, out_dir=None, g3=False, refs=None,
          prefix=None):
    """As `zchar.build` for a modular character: runs the clean-up script (which makes the body and the
    garments) or opens the saved `.blend`, validates, renders the base body's sheet and the dressed
    sheet (every garment on) with Gate 3 sheets beside the reference, a strip per animation (top row
    the body alone, bottom row dressed), saves the `.blend` and exports the GLB. Render names start
    with `prefix` (default the folder name): `<prefix>_base_iso.png`, `<prefix>_<garments>_iso.png`,
    `<prefix>_strip_<clip>.png`. Nothing is exported when validation fails."""
    p = zchar.paths(folder)
    blend = p["blend"]
    if out_dir:
        out = Path(out_dir).expanduser()
        p.update(blend=out / p["blend"].name, textures=out / "textures", renders=out / "renders", export=out / "export")
    spec = zchar.load_spec(folder)
    c = zchar.Character(spec, folder)
    c.paths = p
    report = {"asset": str(p["folder"]), "notes": c.notes}
    if from_blend or not c.raw.exists():
        bpy.ops.wm.open_mainfile(filepath=str(blend))
        report["from"] = "blend"
    else:
        zasset.reset_scene()
        mod_spec = importlib.util.spec_from_file_location(f"zgarment_build_{spec['id']}", p["script"])
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
        report["poke_through"] = getattr(c, "poke_through", None)
    report.update(validate(spec))
    if render and report["ok"]:
        prefix = prefix or p["folder"].name
        worn = [g["id"] for g in wearables(spec)]
        named = "_".join(g["id"] for g in wearables(spec) if g.get("item")) or "dressed"
        report["renders"] = []
        for label, outfit in (("base", []), (named, worn)):
            sheet = p["renders"] / f"{prefix}_{label}_iso.png"
            report["renders"].append(render_sheet(spec, outfit, sheet))
            if g3 or refs:
                r = {"notes": []}
                zasset.add_gate3(r, spec, p["folder"], sheet, refs)
                report["renders"].append(r["g3"])
                report["notes"] += [n for n in r["notes"] if n not in report["notes"]]
        report["renders"] += render_strip(spec, spec.get("animations", []), [[], worn],
                                          p["renders"] / f"{prefix}_strip_{{}}.png")
        if "eyes" in (spec.get("sockets") or {}):
            report["renders"].append(render_head(spec, worn, p["renders"] / f"{prefix}_head.png"))
        report["sheet"] = report["renders"][0]
    if save and report["from"] == "raw":
        report["blend"] = zasset.save_blend(p["blend"], p["script"].read_text(), f"{p['folder'].name}.py")
    if export and report["ok"]:
        report["glb"] = export_glb(spec, p["export"] / f"{spec['id']}.glb", c.extras or None)
        report["body_glb"] = export_body_glb(spec, p["export"] / f"{spec['id']}_body.glb", c.extras or None)
        # A wearable borrowed from another body (`shared_from`) is exported there, not again here.
        report["wearable_glbs"] = [export_wearable_glb(spec, g["id"], p["export"] / "wearables" / f"{g['id']}.glb", c.extras or None)
                                   for g in wearables(spec) if not g.get("shared_from")]
    after = getattr(c, "after_build", None)  # the script's extra steps once built (e.g. a crowd LOD)
    if after and report["ok"] and report["from"] == "raw":
        after(report, render=render, export=export)
    return report
