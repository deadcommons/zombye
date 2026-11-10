"""The shared clean-up for the zombie base bodies with a Meshy body and rig of their own (zombie-03 on).

A body folder's script calls `build(c, params)`: the steps are the same for every body, the numbers in
`params` are its own. Clip helpers (cut, ease, in place, yaw, stoop, ground, LOD) are zombie-02's, imported
from its script unchanged, so both bodies' clips are cleaned the same way.

Steps:
1. Meshy's rigged GLB with the library clips (raw/animations.glb), the three text-to-motion clips merged onto
   the same armature (raw/motion-*.glb), then centimetres -> metres and the hips onto the origin.
2. Bone lengths: the importer leaves Meshy's bones metres long after the bake (heads and directions are
   right); each bone is set to a real length along its own direction, checked to move no vertex in any pose.
3. Mesh: seams welded, decimated to the budget, at most 4 bones per vertex, texture toned, body regions,
   shading as the spec.
4. Clips (names as the client's crowd slots, plus lunge and crawl), then the stoop and grounding.
5. The crowd LOD (zombie-02's `make_lod`), kept in the LOD collection for `build.py -- --crowd`.
"""

import importlib.util
import math
import sys
from pathlib import Path

import bpy
import numpy as np
from mathutils import Matrix

import zchar
import zgarment as g

_Z2 = Path(__file__).resolve().parent / "zombie-02" / "zombie-02.py"
_spec = importlib.util.spec_from_file_location("zombie02_helpers", _Z2)
z2 = importlib.util.module_from_spec(_spec)
_dont, sys.dont_write_bytecode = sys.dont_write_bytecode, True
try:
    _spec.loader.exec_module(z2)
finally:
    sys.dont_write_bytecode = _dont

FPS = 24
MOTIONS = {"shamble_walk": "motion-shamble_walk.glb", "lunge_grab": "motion-lunge_grab.glb", "crawl": "motion-crawl.glb",
           "climb_motion": "motion-climb.glb", "vault_motion": "motion-vault.glb",
           # The crowd's walk (2026-11-10, owner chose "A"): Meshy's Injured Walk (in place), clear planted steps,
           # so the client can play it at the zombie's speed / stride. The dragging shamble had no stride to measure
           # (0 or 0.135 m): zombies glided or the loop raced.
           "injured_walk": "walk-injured.glb"}
# Climbing in through a window and over a fence: the body really goes up and down, so only the travel across the
# floor comes out (the game moves the zombie through the opening); not grounded, no stoop.
CLIMBS = {"climb": "climb_motion", "vault": "vault_motion"}
LIBRARY = {"Idle": "Idle", "Punch_Forward_with_Both_Fists": "Punch_Forward_with_Both_Fists", "Hit_Reaction": "Hit_Reaction",
           "Hit_Reaction_1": "Hit_Reaction_1", "Shot_and_Slow_Fall_Backward": "Shot_and_Slow_Fall_Backward",
           "Stand_Up1": "Stand_Up1", "Shot_and_Fall_Backward": "Shot_and_Fall_Backward",
           "Shot_and_Fall_Forward": "Shot_and_Fall_Forward", "Zombie_Scream": "Zombie_Scream"}
LEGS = ["LeftUpLeg", "LeftLeg", "LeftFoot", "LeftToeBase", "RightUpLeg", "RightLeg", "RightFoot", "RightToeBase"]
LOOPS = ("idle", "walk", "attack", "thump", "down", "crawl")


def _short(name):
    """'Armature|Punch_Forward_with_Both_Fists|baselayer' -> 'Punch_Forward_with_Both_Fists'."""
    parts = name.split("|")
    return parts[1] if len(parts) >= 3 else parts[-1]


def import_all(c):
    arm, mesh = c.import_glb(c.raw)
    for a in list(bpy.data.actions):
        a.name = _short(a.name)
    missing = [k for k in LIBRARY if k not in bpy.data.actions]
    assert not missing, f"{c.raw.name}: no clips {missing} (it has {sorted(a.name for a in bpy.data.actions)})"
    for name, f in MOTIONS.items():
        g.merge_clip(arm, c.paths["raw"] / f, name)
    for a in bpy.data.actions:
        for slot in a.slots:
            slot.name_display = arm.name
    return arm, mesh


def fix_bone_lengths(arm, mesh, length=0.08):
    """Every bone `length` long along its own direction (head, direction and roll kept): no vertex moves."""
    def posed():
        bpy.context.view_layer.update()
        return z2.eval_body(mesh)
    z2._bind(arm, bpy.data.actions["Punch_Forward_with_Both_Fists"])
    bpy.context.scene.frame_set(30)
    before = posed()
    longest = max(b.length for b in arm.data.bones)
    zchar._select([arm], arm)
    bpy.ops.object.mode_set(mode="EDIT")
    for eb in arm.data.edit_bones:
        eb.length = length
    bpy.ops.object.mode_set(mode="OBJECT")
    bpy.context.scene.frame_set(30)
    after = posed()
    moved = float(np.abs(after - before).max())
    arm.animation_data.action = None
    assert moved < 1e-4, f"setting bone lengths moved the posed mesh by {moved:.6f} m"
    return longest, moved


def _hips_heading(arm, data):
    """Per frame: the heading (radians, in the floor plane) of the hips' own Y axis (up the spine)."""
    rest = np.array(arm.data.bones["Hips"].matrix_local.to_3x3())
    q = z2.quat(data, "Hips")
    out = []
    for w, x, y, z in q:
        m = np.array([[1 - 2 * (y * y + z * z), 2 * (x * y - w * z), 2 * (x * z + w * y)],
                      [2 * (x * y + w * z), 1 - 2 * (x * x + z * z), 2 * (y * z - w * x)],
                      [2 * (x * z - w * y), 2 * (y * z + w * x), 1 - 2 * (x * x + y * y)]])
        v = rest @ m @ np.array([0.0, 1.0, 0.0])
        out.append(math.atan2(v[1], v[0]))
    return np.unwrap(np.array(out))


def yaw_per_frame(arm, data, degrees):
    """As zombie-02's `yaw`, but a different turn each frame (`degrees`: one per frame)."""
    n = z2.frames_of(data)
    hq = z2.quat(data, "Hips")
    w, rest = z2.hips_world(arm, data)
    out_q = np.empty_like(hq)
    for i in range(n):
        q = np.array(g.layer_quaternion(arm, "Hips", (0.0, 0.0, float(degrees[i])))[:])
        w1, x1, y1, z1 = q
        w2, x2, y2, z2_ = hq[i]
        out_q[i] = [w1 * w2 - x1 * x2 - y1 * y2 - z1 * z2_, w1 * x2 + x1 * w2 + y1 * z2_ - z1 * y2,
                    w1 * y2 - x1 * z2_ + y1 * w2 + z1 * x2, w1 * z2_ + x1 * y2 - y1 * x2 + z1 * w2]
        rz = np.array(Matrix.Rotation(math.radians(float(degrees[i])), 3, "Z"))
        w[i] = rz @ w[i]
    z2.set_quat(data, "Hips", out_q)
    z2.set_hips_world(data, w, rest)


def towards_rest(data, bones, weight):
    """Those bones' rotations eased toward their rest (straight, as in the T-pose) by `weight`."""
    ident = np.array([1.0, 0.0, 0.0, 0.0])
    for b in bones:
        if (z2.rot_path(b), 0) not in data:
            continue
        q = z2.quat(data, b)
        p = np.where((q @ ident)[:, None] < 0, -ident, ident)
        z2.set_quat(data, b, q * (1 - weight) + p * weight)


def cycle_frames(arm, action, min_lag=16, bones=("LeftFoot", "RightFoot", "LeftHand", "RightHand")):
    """One cycle's length in frames: the lag (at least `min_lag`) at which the feet's and hands' paths,
    relative to the hips, best match themselves (zchar.loop_frames takes the shortest near-best lag,
    which on a 3 s generated clip can be a fraction of a step). Returns (frames, mean error in metres)."""
    z2._bind(arm, action)
    f0, f1 = (int(round(v)) for v in action.frame_range)
    pb = arm.pose.bones
    path = []
    for f in range(f0, f1 + 1):
        bpy.context.scene.frame_set(f)
        path.append([pb[b].head - pb["Hips"].head for b in bones])
    n = len(path)
    errs = {lag: sum((a - b).length for i in range(n - lag) for a, b in zip(path[i], path[i + lag])) / ((n - lag) * len(bones))
            for lag in range(min_lag, n - 8)}
    lag = min(errs, key=errs.get)
    arm.animation_data.action = None
    return lag, errs[lag]


def swing(data, bones, k):
    """Each bone's rotation away from its mean over the clip scaled by `k` (a longer step): per frame the
    rotation from the mean pose, as axis and angle, with the angle times k."""
    from mathutils import Quaternion
    for b in bones:
        q = z2.quat(data, b)
        m = q.mean(axis=0)
        m /= np.linalg.norm(m)
        mq = Quaternion(m)
        out = []
        for row in q:
            r = Quaternion(row)
            if r.dot(mq) < 0:
                r.negate()
            delta = mq.inverted() @ r
            axis, angle = delta.to_axis_angle()
            out.append(list(mq @ Quaternion(axis, angle * k)))
        z2.set_quat(data, b, np.array(out))


def one_cycle_remake(arm, action, name, ease=4, min_lag=16):
    """`action` cut to one cycle (`cycle_frames`), keyed every frame, the last `ease` frames eased into the first."""
    frames, err = cycle_frames(arm, action, min_lag=min_lag)
    f0 = int(round(action.frame_range[0]))
    clip = z2.remake(action, list(range(f0, f0 + frames + 1)), name)
    d = z2.read(clip)
    z2.centre(arm, d)
    moved = z2.pin_travel(arm, d)
    z2.ease_loop(d, ease)
    z2.write(clip, d)
    return clip, frames, err, moved


def motion_span(arm, action, still=0.004):
    """The frames where `action` moves: first to last frame whose mean bone-head change from the previous
    frame is over `still` metres."""
    z2._bind(arm, action)
    f0, f1 = (int(round(v)) for v in action.frame_range)
    prev, moving = None, []
    for f in range(f0, f1 + 1):
        bpy.context.scene.frame_set(f)
        heads = [b.head.copy() for b in arm.pose.bones]
        if prev is not None and sum((a - b).length for a, b in zip(heads, prev)) / len(heads) > still:
            moving.append(f)
        prev = heads
    return (moving[0] - 1, moving[-1]) if moving else (f0, f1)


def make_clips(c, arm, body, P):
    A = bpy.data.actions
    out = {}
    notes = []

    def cut(src, f0, f1, name):
        return z2.remake(A[src], list(range(f0, f1 + 1)), name)

    # Idle: Meshy's Idle, its calm stretch played there and back (as survivor-01's).
    a, b = P["idle_calm"]
    idle = A["Idle"].copy()
    idle.name = "idle"
    g.resample_clip(arm, idle, lambda u: a + (b - a) * (1 - math.cos(2 * math.pi * u)) / 2, P["idle_frames"])
    d = z2.read(idle)
    z2.centre(arm, d)
    z2.write(idle, d)
    out["idle"] = idle
    idle0 = z2.pose_at(d, 0)
    notes.append(f"idle = Meshy's Idle frames {a}-{b} there and back, eased: {P['idle_frames']} frames")

    # Walk: Meshy's Injured Walk, one cycle (zombie-02's cut), in place, its last frames eased into the first.
    walk = cut("injured_walk", z2.WALK[1], z2.WALK[2], "walk")
    d = z2.read(walk)
    z2.centre(arm, d)
    k = P.get("walk_step", 1.0)
    if k != 1.0:
        swing(d, ("LeftUpLeg", "RightUpLeg", "LeftLeg", "RightLeg"), k)
        notes.append(f"walk: the legs' swing scaled x{k:.2f} about its mean pose (a longer step, so the crowd's speed / stride "
                     "doesn't race the loop)")
    z2.ease_loop(d, 4)
    z2.write(walk, d)
    out["walk"] = walk
    notes.append(f"walk = Meshy's Injured Walk (in place), frames {z2.WALK[1]}-{z2.WALK[2]}: one cycle with planted steps, "
                 "last 4 frames eased into the first")

    attack = cut(*z2.ATTACK, "attack")
    d = z2.read(attack)
    z2.centre(arm, d)
    z2.ease_loop(d, z2.LOOP_EASE["attack"])
    z2.write(attack, d)
    out["attack"] = attack

    ta, tb, tn = z2.THUMP[1], z2.THUMP[2], 20
    thump = z2.remake(A[z2.THUMP[0]], [ta + (tb - ta) * (1 - math.cos(2 * math.pi * k / tn)) / 2 for k in range(tn + 1)], "thump")
    d = z2.read(thump)
    z2.centre(arm, d)
    z2.write(thump, d)
    out["thump"] = thump
    notes.append("attack and thump = Meshy's Punch Forward with Both Fists, cut as zombie-02's")

    for name, (src, f0, f1) in (("stagger", z2.STAGGER), ("stagger_2", z2.STAGGER_2)):
        clip = cut(src, f0, f1, name)
        d = z2.read(clip)
        z2.yaw(arm, d, z2.YAW[name])
        z2.centre(arm, d)
        moved = z2.pin_travel(arm, d)
        if name == "stagger_2":
            z2.arms_from(d, idle0)
        z2.blend(d, idle0, z2.EASE_IN[name])
        z2.blend(d, idle0, z2.EASE_OUT[name], at_start=False)
        z2.write(clip, d)
        out[name] = clip
        notes.append(f"{name}: turned {z2.YAW[name]:+.0f}°, {moved:.2f} m of travel taken out, eased from and to the idle")

    fall = cut(*z2.FALL, "fall")
    d = z2.read(fall)
    z2.yaw(arm, d, z2.YAW["fall"])
    z2.centre(arm, d)
    moved = z2.pin_travel(arm, d)
    z2.blend(d, idle0, z2.EASE_IN["fall"])
    z2.write(fall, d)
    out["fall"] = fall
    down_raw = z2.pose_at(d, z2.frames_of(d) - 1)
    notes.append(f"fall: Shot and Slow Fall Backward {z2.FALL[1]}-{z2.FALL[2]}, {moved:.2f} m of travel taken out")

    down = z2.remake(fall, [z2.FALL[2] - z2.FALL[1]] * (z2.DOWN_FRAMES + 1), "down")
    out["down"] = down
    g.apply_pose_layer(arm, down, z2.DOWN_BREATH, weight=lambda t: (1 - math.cos(2 * math.pi * t / z2.DOWN_FRAMES)) / 2)

    for name, (src, f0, f1) in (("die_back", z2.DIE_BACK), ("die_front", z2.DIE_FRONT)):
        clip = cut(src, f0, f1, name)
        d = z2.read(clip)
        z2.centre(arm, d)
        moved = z2.pin_travel(arm, d)
        z2.blend(d, idle0, z2.EASE_IN[name])
        z2.write(clip, d)
        out[name] = clip
        notes.append(f"{name}: {moved:.2f} m of travel taken out")

    src, g0, g1 = z2.GET_UP
    get_up = z2.remake(A[src], [g0 + (g1 - g0) * k / z2.GET_UP_FRAMES for k in range(z2.GET_UP_FRAMES + 1)], "get_up")
    d = z2.read(get_up)
    z2.centre(arm, d)
    z2.pin_travel(arm, d)
    z2.blend(d, down_raw, z2.EASE_IN["get_up"])
    z2.blend(d, idle0, z2.EASE_OUT["get_up"], at_start=False)
    z2.write(get_up, d)
    out["get_up"] = get_up

    die_down = z2.remake(fall, [z2.FALL[2] - z2.FALL[1]] * (z2.DIE_DOWN_FRAMES + 1), "die_down")
    out["die_down"] = die_down
    g.apply_pose_layer(arm, die_down, z2.DIE_DOWN_HEAVE, weight=lambda t: math.sin(math.pi * min(t, 14) / 14))
    g.apply_pose_layer(arm, die_down, z2.DIE_DOWN_LIMP, weight=lambda t: float(z2.smooth((t - 8) / 20)))

    groan = cut(*z2.GROAN, "groan")
    d = z2.read(groan)
    z2.centre(arm, d)
    z2.blend(d, idle0, z2.EASE_IN["groan"])
    z2.blend(d, idle0, z2.EASE_OUT["groan"], at_start=False)
    z2.write(groan, d)
    out["groan"] = groan

    # Lunge (text-to-motion lunge_grab): cut to where it moves, in place (the game moves the zombie), eased in
    # from the idle and back out to it, so it ends where it began.
    s0, s1 = motion_span(arm, A["lunge_grab"])
    s1 = min(s1, s0 + P["lunge_max_frames"])
    lunge = cut("lunge_grab", s0, s1, "lunge")
    d = z2.read(lunge)
    z2.centre(arm, d)
    moved = z2.pin_travel(arm, d, threshold=0.0)
    z2.blend(d, idle0, P["lunge_ease_in"])
    z2.blend(d, idle0, P["lunge_ease_out"], at_start=False)
    z2.write(lunge, d)
    out["lunge"] = lunge
    notes.append(f"lunge = text-to-motion lunge_grab frames {s0}-{s1} ({(s1 - s0) / FPS:.2f} s), {moved:.2f} m of travel taken out "
                 f"(in place), eased in from the idle ({P['lunge_ease_in']} frames) and back out to it ({P['lunge_ease_out']})")

    # Crawl (text-to-motion crawl): the slow turn taken out (the hips' heading's straight-line drift; its wobble
    # kept), facing south, legs eased toward straight so they trail, one cycle, in place.
    crawl_src = A["crawl"]
    crawl_src.name = "crawl_motion"  # the result takes the name
    d = z2.read(crawl_src)
    head = _hips_heading(arm, d)
    t = np.arange(len(head), dtype=float)
    slope, icpt = np.polyfit(t, head, 1)
    target = -math.pi / 2  # the spine pointing -Y: crawling toward the front
    turn = np.degrees(target - (icpt + slope * t))
    yaw_per_frame(arm, d, turn)
    towards_rest(d, LEGS, P["crawl_legs"])
    z2.write(crawl_src, d)
    crawl, n, err, moved = one_cycle_remake(arm, crawl_src, "crawl", ease=8, min_lag=P.get("crawl_min_frames", 50))
    out["crawl"] = crawl
    notes.append(f"crawl = text-to-motion crawl: a turn of {math.degrees(slope * (len(head) - 1)):+.0f}° over the clip taken out, "
                 f"legs {P['crawl_legs']:.0%} toward straight (trailing), one cycle of {n} frames ({n / FPS:.2f} s, closes within "
                 f"{err * 100:.1f} cm), {moved:.2f} m of travel taken out")

    for name, src in CLIMBS.items():
        c0, c1 = motion_span(arm, A[src])
        clip = cut(src, c0, c1, name)
        d = z2.read(clip)
        z2.centre(arm, d)
        moved = z2.pin_travel(arm, d, threshold=0.0)
        z2.blend(d, idle0, P.get("climb_ease_in", 6))
        z2.blend(d, idle0, P.get("climb_ease_out", 10), at_start=False)
        z2.write(clip, d)
        out[name] = clip
        w_, rest_ = z2.hips_world(arm, d)
        notes.append(f"{name} = text-to-motion {src.split('_')[0]} frames {c0}-{c1} ({(c1 - c0) / FPS:.2f} s), {moved:.2f} m of travel "
                     f"across the floor taken out (the hips rise {(w_[:, 2].max() - w_[0, 2]) * 100:.0f} cm: kept), eased from and to the idle")

    keep = set(out.values())
    for a in list(A):
        if a not in keep:
            A.remove(a)

    layer = c.spec["stoop"]
    for name, a in out.items():
        # The text-to-motion walk and lunge are hunched already: the stoop on top would fold them double.
        if name in CLIMBS:
            a.use_cyclic = False
            notes.append(f"{name}: {int(a.frame_end) + 1} frames ({a.frame_end / FPS:.2f} s, one-shot); no stoop, not grounded")
            continue
        w = [0.0] if name in P.get("no_stoop", ()) else z2.stoop(arm, a, layer)
        lo, hi = z2.ground(arm, body, a)
        a.use_cyclic = name in LOOPS
        notes.append(f"{name}: {int(a.frame_end) + 1} frames ({a.frame_end / FPS:.2f} s, {'loop' if a.use_cyclic else 'one-shot'}); "
                     f"stoop {min(w):.2f}-{max(w):.2f}; lowest point {lo * 100:+.1f} to {hi * 100:+.1f} cm before grounding")
    joins = [f"{n} loop {z2.join_error(arm, out[n], out[n].frame_end, out[n], 0) * 100:.1f} cm" for n in LOOPS]
    joins += [f"idle→{n} {z2.join_error(arm, out['idle'], 0, out[n], 0) * 100:.1f} cm" for n in ("walk", "lunge", "stagger", "groan")]
    joins.append(f"lunge→idle {z2.join_error(arm, out['lunge'], out['lunge'].frame_end, out['idle'], 0) * 100:.1f} cm")
    notes.append("joins (mean bone-head distance): " + ", ".join(joins))
    arm.animation_data.action = None
    for n_ in notes:
        c.note(n_)
    return list(c.spec["animations"])


def build(c, P):
    spec = c.spec
    arm, body = import_all(c)
    c.to_asset_collection(arm, body)
    hx, hy = c.hips_xy(arm)
    scale = c.apply_transforms(arm, body, offset=(-hx, -hy))
    c.note(f"armature scale {scale:.3f} baked into the bones and keys; hips moved ({-hx:+.3f}, {-hy:+.3f}) m onto the origin")
    longest, moved = fix_bone_lengths(arm, body)
    c.note(f"bone lengths: the importer's (up to {longest:.1f} m) set to 0.08 m along each bone; the posed mesh moved {moved * 1000:.3f} mm")

    merged = c.weld_seams(body)
    before, after = c.decimate_to(body, spec["tris"] - 10)
    c.limit_weights(body)
    c.note(f"welded {merged} vertices split at UV seams; decimated {before} -> {after} triangles")
    c.tone_texture(body, c.paths["textures"] / f"{c.paths['folder'].name}_base_color.jpg", **P["tone"])
    c.note("base colour toned: " + ", ".join(f"{k} {v}" for k, v in P["tone"].items()))

    if P.get("head_scale"):
        s = P["head_scale"]
        gi = {gr.name: gr.index for gr in body.vertex_groups}
        heads = {gi[n] for n in ("Head", "head_end", "headfront") if n in gi}
        piv = arm.data.bones["Head"].head_local
        for v in body.data.vertices:
            w = min(1.0, sum(x.weight for x in v.groups if x.group in heads))
            if w > 0:
                v.co = piv + (v.co - piv) * (1 + (s - 1) * w)
        h = max(v.co.z for v in body.data.vertices)
        f = spec["size"][2] / h
        arm.scale = (f, f, f)
        c.apply_transforms(arm, body)
        c.note(f"head and hair scaled {s} about the Head joint (blended by the head bones' weights); the whole body and rig "
               f"then scaled x{f:.3f} back to the spec's height ({h:.3f} -> {spec['size'][2]:.3f} m)")

    counts = g.region_groups(body, spec["regions"])
    c.note("regions: " + ", ".join(f"{r} {n}" for r, n in counts.items()))
    g.shade(body, spec["shading"])

    if g.wearables(spec):
        here = Path(__file__).resolve().parent
        _ws = importlib.util.spec_from_file_location("wardrobe_fit", here / "wardrobe_fit.py")
        wf = importlib.util.module_from_spec(_ws)
        _ws.loader.exec_module(wf)
        if arm.animation_data:
            arm.animation_data.action = None
        for n_ in wf.fit(c, arm, body, spec, here / "survivor-01" / "survivor-01.blend", zchar.load_spec(here / "survivor-01")):
            c.note(n_)

    clips = make_clips(c, arm, body, P)
    zchar.name_things(arm, body, spec["id"], {n: n for n in clips})
    g.fix_slots(arm)
    c.note(f"heights: rest {z2.height(body, arm, None):.3f} m; idle with the stoop {z2.height(body, arm, 'idle'):.3f} m "
           f"(spec stooped {spec.get('stooped_height')})")
    lod = z2.make_lod(c, body, arm)
    # zchar (no wearables yet) counts every vertex group as a bone: the regions stay as the `_region` point
    # attribute (what the GLB and zvat read); the groups come back when the wardrobe is fitted (zgarment).
    if not g.wearables(spec):
        for o in (body, lod):
            for gr in [gr for gr in o.vertex_groups if gr.name.startswith(g.REGION_PREFIX)]:
                o.vertex_groups.remove(gr)
    return body
