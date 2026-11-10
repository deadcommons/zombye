#!/usr/bin/env python3
"""Retarget Universal Animation Library clips (Quaternius, CC0) onto the survivor's skeleton.

Plain Python + numpy, no Blender: reads the library's GLB and the survivor's body GLB, carries each
mapped bone's world-space turn from the library's reference pose over onto the survivor's bone (its rest
direction first lined up with the library's), and writes a copy of the body GLB with the clips added
or replaced.

    retarget.py BODY.glb OUT.glb NAME=LIB.glb:Clip[+Clip...][@HIT] ...

`+` plays clips one after another. `@HIT` (0..1 of the first clip) warps time so that moment falls at
45% of the result, where the game lands a blow; `@auto` takes the moment the right hand moves fastest.
`NAME=...:Clip#F` takes the single frame F as a held pose (two frames).

    retarget.py --clips BODY.glb OUT.glb NAME=... ...

writes only the skeleton's nodes and the clips named, no mesh or texture: a file for the game's
private clips folder (`release_anim.rs`), for clips from libraries whose licence keeps their files
out of the public repo. BODY.glb is the game's own body (its idle is the reference for the grip).

    retarget.py [--clips] --map MAP.json BODY.glb OUT.glb NAME=... ...

reads another library's skeleton from MAP.json instead of the Universal Animation Library's:
`{"bones": {OURS: [THEIRS, THEIR_CHILD, OUR_CHILD], ...}, "reference": CLIP or null (the file's rest
pose, under the clip's own root), "knuckles": [LITTLE_FINGER, FOREFINGER] of the right hand}`, with the same bones of ours as `MAP`.
"""
import json
import struct
import sys

import numpy as np

FPS = 24
U_HIT = 0.45
MAP = {  # ours: (library bone, library child the bone points at, our child)
    "Hips": ("pelvis", None, None),
    "Spine02": ("spine_01", "spine_02", "Spine01"),
    "Spine01": ("spine_02", "spine_03", "Spine"),
    "Spine": ("spine_03", "neck_01", "neck"),
    "neck": ("neck_01", "Head", "Head"),
    "Head": ("Head", None, None),
    "LeftShoulder": ("clavicle_l", "upperarm_l", "LeftArm"),
    "LeftArm": ("upperarm_l", "lowerarm_l", "LeftForeArm"),
    "LeftForeArm": ("lowerarm_l", "hand_l", "LeftHand"),
    "LeftHand": ("hand_l", None, None),
    "RightShoulder": ("clavicle_r", "upperarm_r", "RightArm"),
    "RightArm": ("upperarm_r", "lowerarm_r", "RightForeArm"),
    "RightForeArm": ("lowerarm_r", "hand_r", "RightHand"),
    "RightHand": ("hand_r", None, None),
    "LeftUpLeg": ("thigh_l", "calf_l", "LeftLeg"),
    "LeftLeg": ("calf_l", "foot_l", "LeftFoot"),
    "LeftFoot": ("foot_l", "ball_l", "LeftToeBase"),
    "LeftToeBase": ("ball_l", None, None),
    "RightUpLeg": ("thigh_r", "calf_r", "RightLeg"),
    "RightLeg": ("calf_r", "foot_r", "RightFoot"),
    "RightFoot": ("foot_r", "ball_r", "RightToeBase"),
    "RightToeBase": ("ball_r", None, None),
}
# Bones with no direction of their own take their parent's line-up.
INHERIT = {"LeftHand": "LeftForeArm", "RightHand": "RightForeArm", "LeftToeBase": "LeftFoot", "RightToeBase": "RightFoot", "Head": "neck"}


# Quaternions as (x, y, z, w) arrays.
def qmul(a, b):
    ax, ay, az, aw = np.moveaxis(a, -1, 0)
    bx, by, bz, bw = np.moveaxis(b, -1, 0)
    return np.stack([aw * bx + ax * bw + ay * bz - az * by, aw * by - ax * bz + ay * bw + az * bx,
                     aw * bz + ax * by - ay * bx + az * bw, aw * bw - ax * bx - ay * by - az * bz], -1)


def qinv(a):
    return a * np.array([-1, -1, -1, 1.0])


def qrot(q, v):
    return qmul(qmul(q, np.concatenate([v, np.zeros(v.shape[:-1] + (1,))], -1)), qinv(q))[..., :3]


def qnorm(q):
    return q / np.linalg.norm(q, axis=-1, keepdims=True)


def qarc(a, b):
    a, b = a / np.linalg.norm(a), b / np.linalg.norm(b)
    c = np.cross(a, b)
    w = 1.0 + a.dot(b)
    if w < 1e-6:
        ax = np.cross(a, [1, 0, 0])
        if np.linalg.norm(ax) < 1e-6:
            ax = np.cross(a, [0, 1, 0])
        return np.append(ax / np.linalg.norm(ax), 0.0)
    return qnorm(np.append(c, w))


def qslerp(a, b, t):
    d = (a * b).sum(-1, keepdims=True)
    b = np.where(d < 0, -b, b)
    return qnorm(a + (b - a) * t)  # nlerp: frames are close


COMP = {5120: "b", 5121: "B", 5122: "h", 5123: "H", 5125: "I", 5126: "f"}
WIDTH = {"SCALAR": 1, "VEC2": 2, "VEC3": 3, "VEC4": 4, "MAT4": 16}


class Glb:
    def __init__(self, path):
        d = open(path, "rb").read()
        n = struct.unpack_from("<I", d, 12)[0]
        self.json = json.loads(d[20:20 + n])
        o = 20 + n
        m = struct.unpack_from("<I", d, o)[0]
        self.bin = bytearray(d[o + 8:o + 8 + m])
        g = self.json
        self.nodes = g["nodes"]
        self.parent = {c: i for i, nd in enumerate(self.nodes) for c in nd.get("children", [])}
        self.by_name = {nd.get("name"): i for i, nd in enumerate(self.nodes)}
        # Parents before children.
        self.order = []
        stack = [i for i in range(len(self.nodes)) if i not in self.parent]
        while stack:
            i = stack.pop()
            self.order.append(i)
            stack += self.nodes[i].get("children", [])

    def accessor(self, k):
        a = self.json["accessors"][k]
        v = self.json["bufferViews"][a["bufferView"]]
        w = WIDTH[a["type"]]
        off = v.get("byteOffset", 0) + a.get("byteOffset", 0)
        assert "byteStride" not in v or v["byteStride"] == w * 4
        arr = np.frombuffer(self.bin, COMP[a["componentType"]], a["count"] * w, off).astype(np.float64)
        return arr.reshape(a["count"], w) if w > 1 else arr

    def rest(self):
        t = np.array([nd.get("translation", [0, 0, 0]) for nd in self.nodes], float)
        r = np.array([nd.get("rotation", [0, 0, 0, 1]) for nd in self.nodes], float)
        return t, r

    def clip(self, name):
        """(times, t[frames, nodes, 3], r[frames, nodes, 4]) sampled at FPS."""
        anim = next(a for a in self.json["animations"] if a["name"] == name)
        end = max(self.accessor(s["input"]).max() for s in anim["samplers"])
        times = np.arange(0, end + 1e-6, 1.0 / FPS)
        t0, r0 = self.rest()
        t = np.repeat(t0[None], len(times), 0)
        r = np.repeat(r0[None], len(times), 0)
        for ch in anim["channels"]:
            path = ch["target"]["path"]
            if path not in ("translation", "rotation"):
                continue
            s = anim["samplers"][ch["sampler"]]
            kt, kv = self.accessor(s["input"]), self.accessor(s["output"])
            i = np.clip(np.searchsorted(kt, times, "right") - 1, 0, len(kt) - 1)
            j = np.minimum(i + 1, len(kt) - 1)
            f = np.where(kt[j] > kt[i], (times - kt[i]) / np.maximum(kt[j] - kt[i], 1e-9), 0.0)[:, None]
            f = np.clip(f, 0, 1)
            if s.get("interpolation") == "STEP":
                f = f * 0
            n = ch["target"]["node"]
            if path == "translation":
                t[:, n] = kv[i] + (kv[j] - kv[i]) * f
            else:
                r[:, n] = qslerp(kv[i], kv[j], f)
        return times, t, r

    def globals(self, t, r):
        """World positions and rotations for local t[..., nodes, 3], r[..., nodes, 4]."""
        gp, gr = np.zeros_like(t), np.zeros_like(r)
        for i in self.order:
            if i in self.parent:
                p = self.parent[i]
                gr[..., i, :] = qmul(gr[..., p, :], r[..., i, :])
                gp[..., i, :] = gp[..., p, :] + qrot(gr[..., p, :], t[..., i, :])
            else:
                gr[..., i, :], gp[..., i, :] = r[..., i, :], t[..., i, :]
        return gp, gr

    def add_view(self, arr):
        raw = np.ascontiguousarray(arr, np.float32).tobytes()
        while len(self.bin) % 4:
            self.bin.append(0)
        self.json["bufferViews"].append({"buffer": 0, "byteOffset": len(self.bin), "byteLength": len(raw)})
        self.bin += raw
        return len(self.json["bufferViews"]) - 1

    def add_accessor(self, arr, kind):
        arr = np.asarray(arr, np.float32)
        a = {"bufferView": self.add_view(arr), "componentType": 5126, "count": len(arr), "type": kind}
        if kind == "SCALAR":
            a["min"], a["max"] = [float(arr.min())], [float(arr.max())]
        self.json["accessors"].append(a)
        return len(self.json["accessors"]) - 1

    def set_clip(self, name, joints, times, t, r):
        """Add or replace a clip: rotation for every joint, translation and scale as they were given."""
        tin = self.add_accessor(times, "SCALAR")
        samplers, channels = [], []
        for n in joints:
            for path, vals, kind in (("translation", t[:, n], "VEC3"), ("rotation", r[:, n], "VEC4"),
                                     ("scale", np.repeat(np.array([self.nodes[n].get("scale", [1, 1, 1])]), len(times), 0), "VEC3")):
                samplers.append({"input": tin, "output": self.add_accessor(vals, kind), "interpolation": "LINEAR"})
                channels.append({"sampler": len(samplers) - 1, "target": {"node": n, "path": path}})
        anim = {"name": name, "samplers": samplers, "channels": channels}
        anims = self.json.setdefault("animations", [])
        for k, a in enumerate(anims):
            if a["name"] == name:
                anims[k] = anim
                return
        anims.append(anim)

    def skeleton_only(self):
        """A copy with the nodes (names, rest, children) and nothing else: no mesh, skin, texture or clip."""
        out = Glb.__new__(Glb)
        keep = ("name", "translation", "rotation", "scale", "children")
        out.json = {"asset": {"version": "2.0"}, "scene": 0,
                    "scenes": [{"nodes": [i for i in range(len(self.nodes)) if i not in self.parent]}],
                    "nodes": [{k: nd[k] for k in keep if k in nd} for nd in self.nodes],
                    "buffers": [{"byteLength": 0}], "bufferViews": [], "accessors": [], "animations": []}
        out.bin = bytearray()
        out.nodes, out.parent, out.by_name, out.order = out.json["nodes"], self.parent, self.by_name, self.order
        return out

    def write(self, path):
        self.json["buffers"][0]["byteLength"] = len(self.bin)
        js = json.dumps(self.json, separators=(",", ":")).encode()
        js += b" " * (-len(js) % 4)
        while len(self.bin) % 4:
            self.bin.append(0)
        with open(path, "wb") as f:
            f.write(struct.pack("<4sII", b"glTF", 2, 28 + len(js) + len(self.bin)))
            f.write(struct.pack("<I4s", len(js), b"JSON") + js)
            f.write(struct.pack("<I4s", len(self.bin), b"BIN\0") + bytes(self.bin))


class Retarget:
    def __init__(self, body, lib, ref_clip="A_TPose", bones=MAP, knuckles=("pinky_01_r", "index_01_r"), root_clip=None):
        self.body, self.lib, self.map, self.knuckles = body, lib, bones, knuckles
        if ref_clip is None:
            lt, lr = lib.rest()
            # What stands above the hips (a rig's root, turned to another up axis at rest) is as the clip has it.
            _, ct, cr = lib.clip(root_clip)
            n = lib.by_name[bones["Hips"][0]]
            while n in lib.parent:
                n = lib.parent[n]
                lt[n], lr[n] = ct[0][n], cr[0][n]
            self.lref_p, self.lref_r = lib.globals(lt, lr)
        else:
            _, lt, lr = lib.clip(ref_clip)
            self.lref_p, self.lref_r = lib.globals(lt[0], lr[0])
        bt, br = body.rest()
        self.rest_t, self.rest_r = bt, br
        self.brest_p, self.brest_r = body.globals(bt, br)
        b, l = body.by_name, lib.by_name
        # The library must face the way the survivor does (+Z, his left at +X).
        assert self.lref_p[l[bones["LeftArm"][0]]][0] > 0 and self.brest_p[b["LeftArm"]][0] > 0, "the two don't face the same way"
        self.align = {}
        for ours, (theirs, their_child, our_child) in bones.items():
            if their_child:
                want = self.lref_p[l[their_child]] - self.lref_p[l[theirs]]
                have = self.brest_p[b[our_child]] - self.brest_p[b[ours]]
                self.align[ours] = qarc(have, want)
        for ours, src in INHERIT.items():
            self.align[ours] = self.align[src]
        self.align["Hips"] = np.array([0, 0, 0, 1.0])
        self.scale = self.brest_p[b["Hips"]][1] / self.lref_p[l[bones["Hips"][0]]][1]
        self.grip = self.grip_fix()

    def grip_fix(self):
        """The extra turn of the right hand that points the held weapon along the library's grip.

        The game draws the weapon along the hand's +Z as the hand is in the first frame of the body's idle
        (turned to face forward, as the game turns it). The library grips along the knuckles, little
        finger to forefinger."""
        body, lib = self.body, self.lib
        b, l = body.by_name, lib.by_name
        _, t, r = body.clip("idle")
        fwd = qrot(r[:, b["Hips"]], np.array([0, 0, 1.0]))
        yaw = np.arctan2(np.sin(np.arctan2(fwd[:, 0], fwd[:, 2])).sum(), np.cos(np.arctan2(fwd[:, 0], fwd[:, 2])).sum())
        turn = np.array([0, np.sin(-yaw / 2), 0, np.cos(-yaw / 2)])
        ref = qmul(turn, body.globals(t[0], r[0])[1][b["RightHand"]])
        hand = l[self.map["RightHand"][0]]
        knuckles = self.lref_p[l[self.knuckles[1]]] - self.lref_p[l[self.knuckles[0]]]
        want = qrot(qinv(self.lref_r[hand]), knuckles / np.linalg.norm(knuckles))
        plain = qmul(qinv(self.lref_r[hand]), qmul(self.align["RightHand"], self.brest_r[b["RightHand"]]))
        have = qrot(qmul(plain, qinv(ref)), np.array([0, 0, 1.0]))
        fix = qarc(have, want)
        print(f"grip: the hand turned {np.degrees(2 * np.arccos(min(1, abs(fix[3])))):.0f} deg to lay the weapon along the library's grip")
        return fix

    def clip(self, name, frame=None):
        body, lib = self.body, self.lib
        times, lt, lr = lib.clip(name)
        if frame is not None:
            lt, lr, times = lt[[frame, frame]], lr[[frame, frame]], np.array([0, 1.0 / FPS])
        lp, lg = lib.globals(lt, lr)
        n = len(times)
        t = np.repeat(self.rest_t[None], n, 0)
        r = np.repeat(self.rest_r[None], n, 0)
        g = np.zeros_like(r)
        for i in body.order:
            name_i = body.nodes[i].get("name")
            pg = g[:, body.parent[i]] if i in body.parent else np.tile([0, 0, 0, 1.0], (n, 1))
            if name_i in self.map:
                k = lib.by_name[self.map[name_i][0]]
                turn = qmul(lg[:, k], qinv(self.lref_r[k]))
                if name_i == "RightHand":
                    turn = qmul(lg[:, k], qmul(self.grip, qinv(self.lref_r[k])))
                g[:, i] = qnorm(qmul(turn, qmul(self.align[name_i], self.brest_r[i])))
                r[:, i] = qnorm(qmul(qinv(pg), g[:, i]))
            else:
                g[:, i] = qmul(pg, r[:, i])
        hips, pelvis = body.by_name["Hips"], lib.by_name[self.map["Hips"][0]]
        # The hips go where the library's pelvis goes, at the survivor's size, kept in place over the ground.
        move = (lp[:, pelvis] - self.lref_p[pelvis]) * self.scale
        t[:, hips] = self.rest_t[hips] + move
        return times, t, r


def resample(times, t, r, at):
    i = np.clip(np.searchsorted(times, at, "right") - 1, 0, len(times) - 2)
    f = np.clip((at - times[i]) / (times[i + 1] - times[i]), 0, 1)[:, None, None]
    return t[i] + (t[i + 1] - t[i]) * f, qslerp(r[i], r[i + 1], f)


def main():
    args = sys.argv[1:]
    clips_only = bool(args) and args[0] == "--clips"
    if clips_only:
        args = args[1:]
    setup = {}
    if args and args[0] == "--map":
        m = json.load(open(args[1]))
        setup = {"ref_clip": m.get("reference"), "bones": {k: tuple(v) for k, v in m["bones"].items()}, "knuckles": tuple(m["knuckles"])}
        args = args[2:]
    body_path, out = args[0], args[1]
    body = Glb(body_path)
    target = body.skeleton_only() if clips_only else body
    joints = body.json["skins"][0]["joints"]
    libs = {}
    for spec in args[2:]:
        name, rest = spec.split("=", 1)
        lib_path, clips = rest.rsplit(":", 1)
        hit = None
        if "@" in clips:
            clips, hit = clips.split("@")
        lib = libs.setdefault(lib_path, Glb(lib_path))
        rt = Retarget(body, lib, root_clip=clips.split("+")[0].split("#")[0], **setup)
        parts = []
        for c in clips.split("+"):
            frame = None
            if "#" in c:
                c, frame = c.split("#")
                frame = int(frame)
            parts.append(rt.clip(c, frame))
        times, t, r = parts[0]
        first_end = times[-1]
        for pt, t2, r2 in parts[1:]:
            times = np.concatenate([times, times[-1] + pt[1:]])
            t, r = np.concatenate([t, t2[1:]]), np.concatenate([r, r2[1:]])
        if hit is not None:
            if hit == "auto":
                hand = body.by_name["RightHand"]
                gp, _ = body.globals(t, r)
                # The blow: while the hand is moving fast, the moment it is straight ahead of the hips.
                n = int(np.searchsorted(times, first_end))
                speed = np.linalg.norm(np.diff(gp[:, hand], axis=0), axis=1)[:n]
                d = (gp[:, hand] - gp[:, body.by_name["Hips"]])[1 : n + 1]
                bearing = np.abs(np.arctan2(d[:, 0], d[:, 2]))
                bearing[speed < 0.5 * speed.max()] = 9.0
                k = int(np.argmin(bearing)) + 1
                at_hit = times[k]
                print(f"{name}: hand {np.degrees(bearing[k - 1]):.0f} deg off straight ahead at the blow")
            else:
                at_hit = float(hit) * first_end
            secs = round(times[-1] * FPS) / FPS
            u = np.arange(0, secs + 1e-6, 1.0 / FPS) / secs
            src = np.where(u < U_HIT, u / U_HIT * at_hit, at_hit + (u - U_HIT) / (1 - U_HIT) * (times[-1] - at_hit))
            t, r = resample(times, t, r, src)
            times = u * secs
            print(f"{name}: the blow at {at_hit:.2f} s of {secs:.2f} s, moved to {U_HIT:.0%}")
        target.set_clip(name, joints, times, t, r)
        print(f"{name}: {clips} -> {len(times)} frames, {times[-1]:.2f} s")
    target.write(out)


if __name__ == "__main__":
    main()
