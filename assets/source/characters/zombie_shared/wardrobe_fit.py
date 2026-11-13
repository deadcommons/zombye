"""Fits survivor-01's wearables (built in Blender on its body) onto another base body on the same
24-bone Meshy rig (bone names identical): the zombie bodies, and any later body.

Every wearable vertex is carried across body to body through the skeleton and the skin:
1. the nearest point p on survivor-01's body (rest pose), the normal there, how far the vertex stands
   off it (d), the body region of that face and the bone that most moves that point;
2. p in that bone's own frame on survivor-01, scaled along the bone by the ratio of the two bodies'
   bone lengths (head to child head) and across it by the ratio of their girths (mean distance of the
   bone's skin from its axis), then placed in the same bone's frame on the new body;
3. that point projected onto the new body's skin in the same region, and the vertex put the same d
   off it along the new normal (never closer than MIN_OFF), so a shirt stays a shirt's thickness off a
   thin old man and a big one alike.
Rigid wearables (glasses, hats, masks, bags in the hand) keep their shape: moved from survivor-01's
socket to the new body's (the spec's sockets, measured with `measure_sockets`) and scaled by the ratio
of the head sizes for the head's sockets (1 for the hands). Skinned ones then take the new body's
weights (zgarment.transfer_weights), rigid ones go on their socket's bone (zgarment.bind_to_socket).
"""

import bpy
import numpy as np
from mathutils import Vector
from mathutils.bvhtree import BVHTree
from mathutils.kdtree import KDTree

import zgarment as g

MIN_OFF = 0.003
SRC_ID = "survivor_01"
HEAD_SOCKETS = ("eyes", "head_top")


def _append(blend, names):
    before_o, before_a = set(bpy.data.objects), set(bpy.data.actions)
    with bpy.data.libraries.load(str(blend), link=False) as (src, dst):
        dst.objects = [n for n in src.objects if n in names]
    coll = bpy.data.collections.new("wardrobe_source")
    bpy.context.scene.collection.children.link(coll)
    for o in dst.objects:
        if o is not None:
            coll.objects.link(o)
    new_actions = [a for a in bpy.data.actions if a not in before_a]
    return {o.name: o for o in bpy.data.objects if o not in before_o}, new_actions, coll


def _regions(obj):
    reg = g.region_of_vertices(obj)
    if not reg:
        attr = obj.data.attributes[g.REGION_ATTRIBUTE]
        reg = {i: g.REGIONS[int(round(d.value))] for i, d in enumerate(attr.data)}
    return reg


def _dominant(obj):
    return g.dominant_bones(obj)


class Body:
    """A body in its rest pose: BVHs of its skin (whole, and per region), each vertex's region and
    bone, and per bone its frame, length and girth."""

    def __init__(self, arm, obj):
        self.arm, self.obj = arm, obj
        me = obj.data
        self.co = [v.co.copy() for v in me.vertices]
        self.reg = _regions(obj)
        self.dom = _dominant(obj)
        polys = [list(p.vertices) for p in me.polygons]
        self.poly_region = []
        for p in polys:
            rs = [self.reg.get(i) for i in p]
            self.poly_region.append(max(set(rs), key=rs.count))
        self.bvh = BVHTree.FromPolygons(self.co, polys)
        self.bvh_region = {}
        for r in g.REGIONS:
            ps = [p for p, pr in zip(polys, self.poly_region) if pr == r]
            if ps:
                self.bvh_region[r] = BVHTree.FromPolygons(self.co, ps)
        self.kd = KDTree(len(self.co))
        for i, c in enumerate(self.co):
            self.kd.insert(c, i)
        self.kd.balance()
        self.frame, self.length, self.girth = {}, {}, {}
        bones = arm.data.bones
        for b in bones:
            head = b.head_local.copy()
            child = b.children[0].head_local if b.children else None
            axis = (child - head) if child is not None else (b.matrix_local.to_3x3() @ Vector((0, 1, 0))) * 0.1
            self.frame[b.name] = (head, b.matrix_local.to_3x3())
            self.length[b.name] = max(axis.length, 1e-3)
            u = axis.normalized()
            ds = []
            for i, bn in enumerate(self.dom):
                if bn == b.name:
                    v = self.co[i] - head
                    ds.append((v - u * v.dot(u)).length)
            self.girth[b.name] = (sum(ds) / len(ds)) if ds else None

    def nearest(self, p, region=None):
        bvh = self.bvh_region.get(region) or self.bvh
        loc, nrm, idx, _ = bvh.find_nearest(p)
        if loc is None:
            loc, nrm, idx, _ = self.bvh.find_nearest(p)
        return loc, nrm


def _ratios(src, dst, bone):
    lr = dst.length[bone] / src.length[bone]
    gs, gd = src.girth.get(bone), dst.girth.get(bone)
    gr = (gd / gs) if gs and gd else lr
    return min(max(lr, 0.6), 1.6), min(max(gr, 0.6), 1.8)


def carry(src, dst, v):
    """A point near survivor-01's skin carried to the same place on the new body (see the module docs)."""
    loc, nrm, idx, _ = src.bvh.find_nearest(v)
    d = (v - loc).dot(nrm)
    _, vi, _ = src.kd.find(loc)
    bone, region = src.dom[vi], src.reg.get(vi)
    hs, rs = src.frame[bone]
    hd, rd = dst.frame[bone]
    lr, gr = _ratios(src, dst, bone)
    local = rs.transposed() @ (loc - hs)
    local = Vector((local.x * gr, local.y * lr, local.z * gr))
    q = hd + rd @ local
    loc2, nrm2 = dst.nearest(q, region)
    off = d if d < 0 else max(d, MIN_OFF)
    # What the vertex stands off sideways (a hem's lip, a cuff past the wrist), carried in the bone's
    # frame and scaled as the point was: without it, neighbours projecting to one spot would collapse.
    t = (v - loc) - nrm * d
    tl = rs.transposed() @ t
    t2 = rd @ Vector((tl.x * gr, tl.y * lr, tl.z * gr))
    t2 -= nrm2 * t2.dot(nrm2)
    return loc2 + nrm2 * off + t2


def unfold(me, orig, rounds=4, area=1e-8):
    """Faces the carry collapsed (zero area) opened again: each of their vertices put at its neighbours'
    mean plus where it stood from them on survivor-01. Returns how many vertices moved."""
    nbrs = {v.index: set() for v in me.vertices}
    for e in me.edges:
        a, b = e.vertices
        nbrs[a].add(b)
        nbrs[b].add(a)
    moved = set()
    for _ in range(rounds):
        bad = {i for p in me.polygons if p.area < area for i in p.vertices}
        if not bad:
            break
        for i in bad:
            ns = list(nbrs[i])
            if not ns:
                continue
            md = sum((me.vertices[j].co for j in ns), Vector()) / len(ns)
            ms = sum((orig[j] for j in ns), Vector()) / len(ns)
            me.vertices[i].co = md + (orig[i] - ms)
            moved.add(i)
        me.update()
    return len(moved)


def head_scale(src_spec, spec):
    """How much bigger the new head is: the crown's height over the Head joint (head_top socket), new
    over survivor-01's. (A bounding box would count hair: wisps, a ponytail.)"""
    return Vector(spec["sockets"]["head_top"]["offset"]).length / Vector(src_spec["sockets"]["head_top"]["offset"]).length


def source(blend, wearable_ids):
    names = {SRC_ID, f"{SRC_ID}_rig"} | {f"{SRC_ID}_{w}" for w in wearable_ids}
    objs, actions, coll = _append(blend, names)
    sarm = objs[f"{SRC_ID}_rig"]
    for a in actions:
        bpy.data.actions.remove(a)
    if sarm.animation_data:
        sarm.animation_data.action = None
        for t in list(sarm.animation_data.nla_tracks):
            sarm.animation_data.nla_tracks.remove(t)
    sarm.data.pose_position = "REST"
    return objs, coll


def measure_sockets(src_spec, src, dst):
    """The spec's sockets for the new body: each of survivor-01's carried across; offsets from the
    same bone's head on the new body."""
    out = {}
    for name, s in src_spec["sockets"].items():
        p = src.frame[s["bone"]][0] + Vector(s["offset"])
        q = carry(src, dst, p)
        off = q - dst.frame[s["bone"]][0]
        out[name] = (s["bone"], tuple(round(x, 3) for x in off), tuple(s.get("rotation", (0, 0, 0))))
    return out


def fit(c, arm, body, spec, src_blend, src_spec):
    """Every wearable in `spec` (ids as survivor-01's) made for this body: named `<id>_<wearable>`, in
    the Asset collection, weighted or bound. Returns notes."""
    wear = g.wearables(spec)
    objs, coll = source(src_blend, [w["id"] for w in wear])
    src = Body(objs[f"{SRC_ID}_rig"], objs[SRC_ID])
    dst = Body(arm, body)
    k_head = head_scale(src_spec, spec)
    notes = [f"wardrobe: head size x{k_head:.3f} of survivor-01's"]
    asset = body.users_collection[0]
    for w in wear:
        so = objs[f"{SRC_ID}_{w['id']}"]
        me = so.data.copy()
        if w["kind"] == "Rigid":
            ss = src_spec["sockets"][w["socket"]]
            ds = spec["sockets"][w["socket"]]
            ps = src.frame[ss["bone"]][0] + Vector(ss["offset"])
            pd = dst.frame[ds["bone"]][0] + Vector(ds["offset"])
            k = k_head if w["socket"] in HEAD_SOCKETS else 1.0
            for v in me.vertices:
                v.co = pd + (v.co - ps) * k
        else:
            orig = [v.co.copy() for v in me.vertices]
            for v in me.vertices:
                v.co = carry(src, dst, v.co.copy())
            fixed = unfold(me, orig)
            if fixed:
                notes.append(f"{so.name}: {fixed} vertices of collapsed faces placed from their neighbours")
        o = bpy.data.objects.new(f"{spec['id']}_{w['id']}", me)
        me.name = o.name
        asset.objects.link(o)
        if w["kind"] == "Rigid":
            g.bind_to_socket(o, arm, spec, w["socket"])
        else:
            g.transfer_weights(body, o, arm)
        g.shade(o, spec["shading"])
        for key in ("item", "kind", "layer"):
            if w.get(key) is not None:
                o[key] = w[key]
        notes.append(f"{o.name}: {len(me.polygons)} faces, {w['kind'].lower()}" + (f" on {w['socket']}" if w["kind"] == "Rigid" else ""))
    for o in list(coll.objects):
        bpy.data.objects.remove(o)
    bpy.data.collections.remove(coll)
    g._purge_orphans()
    return notes
