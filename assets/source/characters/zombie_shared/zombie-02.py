"""zombie_02: the crowd's zombie, made from survivor-01's approved base body and rig.

- Body, rig, the calm idle and the hoodie come from ../survivor-01/survivor-01.blend unchanged (the same
  mesh, regions, sockets and UVs, so every survivor wearable fits the zombie).
- The texture is the survivor's, repainted: skin recoloured grey-green (by body region and colour), sunken
  eye sockets, a slack open mouth, a few big muted stains, grime at the hems, a tear at the left knee.
- Clips: Meshy's ten zombie clips bought on the survivor's rig (raw/zombie-animations.glb), cleaned: in
  place, cut to one cycle or looped, turned to face south, eased in from and out to the idle where they join,
  a stoop layered over every one (faded out lying down), and every frame put on the ground.
- A crowd LOD (same rig, weights, UVs, texture) for #53.

Run by zombye's tools/blender/zgarment.py (`build.py -- <zombye-art>/characters/zombie-02`), which then
validates, renders and exports. Every number below is ours.
"""

import math
from pathlib import Path

import bmesh
import bpy
import numpy as np
from mathutils import Matrix, Quaternion, Vector

import zasset
import zchar
import zgarment as g

FPS = 24

# ---------------------------------------------------------------- clips
# Meshy's clip -> ours: source frames (Meshy's, at 24 fps), what to do. Verdicts in meta.yml / review.md.
WALK = ("Injured_Walk_inplace", 0, 35)          # one cycle of six
ATTACK = ("Punch_Forward_with_Both_Fists", 21, 60)  # arms flung wide, then both forward together: a grab
THUMP = ("Punch_Forward_with_Both_Fists", 25, 37)  # both fists forward and back: pounding (there and back)
STAGGER = ("Hit_Reaction", 0, 39)
STAGGER_2 = ("Hit_Reaction_1", 0, 28)
# The fall: Meshy's Shot and Slow Fall Backward (bought for it, round 2: raw/zombie-fall.glb), from where the
# reel turns into the fall to where he lies still on his back (where Stand Up 1 starts).
FALL_GLB, FALL = "zombie-fall.glb", ("Shot_and_Slow_Fall_Backward", 44, 98)
DIE_BACK = ("Shot_and_Fall_Backward", 8, 84)
DIE_FRONT = ("Shot_and_Fall_Forward", 0, 52)
GET_UP = ("Stand_Up1", 20, 150)                 # lying still before, standing still after: cut
GET_UP_FRAMES = 48                              # ... and played faster, evenly: 2.0 s (the game's get-up, #53)
GROAN = ("Zombie_Scream", 0, 67)
# Not used: Knock_Down (launches him 1.3 m into the air) and Heavy_Hammer_Swing (a sideways sledgehammer swing,
# not a door being battered); round 1 renders keep the evidence.

# Turned to face south (degrees about Z): these clips stand side-on in a fighting stance.
# The fall turns a little on the floor: -8° lays him down along the line Stand Up 1 gets up from.
YAW = {"stagger": 57.0, "stagger_2": 45.0, "fall": -8.0}
# Frames eased in from the idle's first pose / out to it (or from the down pose for get_up, die_down).
EASE_IN = {"stagger": 8, "stagger_2": 8, "fall": 10, "die_back": 10, "die_front": 14, "groan": 8, "get_up": 8}
EASE_OUT = {"stagger": 10, "stagger_2": 10, "groan": 10, "get_up": 8}
# A loop's end eased back into its start (frames), so it plays round seamlessly.
LOOP_EASE = {"attack": 12}
ARMS = ["LeftShoulder", "LeftArm", "LeftForeArm", "LeftHand", "RightShoulder", "RightArm", "RightForeArm", "RightHand"]
DOWN_FRAMES, DOWN_BREATH = 48, {"Spine02": (1.5, 0.0, 0.0), "Spine01": (1.0, 0.0, 0.0)}
DIE_DOWN_FRAMES = 40
# Dying on the ground: the chest heaves once (its weight rises and falls over frames 0-14), then the body goes
# limp (frames 8-28): the head rolls to one side, the arms and knees settle a little.
DIE_DOWN_HEAVE = {"Spine02": (9.0, 0.0, 0.0), "Spine01": (6.0, 0.0, 0.0), "neck": (-8.0, 0.0, 0.0)}
DIE_DOWN_LIMP = {"Head": (0.0, 0.0, 50.0), "neck": (0.0, 0.0, 20.0), "LeftForeArm": (0.0, -15.0, 0.0),
                 "RightForeArm": (0.0, 15.0, 0.0), "LeftLeg": (-6.0, 0.0, 0.0), "RightLeg": (-6.0, 0.0, 0.0)}
# The stoop fades with how upright the torso is (hips to neck, z of the unit vector): full above UP_FULL.
UP_NONE, UP_FULL = 0.45, 0.85

# ---------------------------------------------------------------- texture
ZOMBIE_SKIN = (0.541, 0.604, 0.486)  # #8A9A7C display: brand_zombie_green toward grey (art-direction.md)
SKIN_LEVEL = 0.85                    # the survivor's median skin tone lands a little under it (lit, it reads lighter)
SKIN_CONTRAST = 0.85                 # how much of the survivor skin's light and shade the zombie keeps
SOCKET_DARK, SOCKET_STRENGTH = (0.17, 0.19, 0.16), 0.75
MOUTH_DARK, MOUTH_HALF = (0.13, 0.10, 0.09), (0.026, 0.008)  # half width, half height (metres)
STAIN = (0.43, 0.37, 0.30)           # muted dark brown, as a multiply over the cloth
STAIN_ALPHA, STAIN_EDGE, STAIN_RAGGED = 0.75, 0.006, 0.014  # opacity; soft edge and ragged edge (metres)
# Stains: each a merged chain of spheres on the rest body (x left, -y front, z up), clothes only.
STAINS = [
    [((0.03, -0.14, 1.41), 0.05), ((0.05, -0.15, 1.35), 0.055), ((0.06, -0.15, 1.28), 0.045),
     ((0.07, -0.15, 1.22), 0.035), ((0.065, -0.15, 1.17), 0.025)],                     # down the chest from the collar
    [((0.13, -0.12, 1.04), 0.06), ((0.17, -0.09, 0.99), 0.045)],                      # belly, left side
    [((-0.10, -0.10, 0.67), 0.055), ((-0.09, -0.11, 0.60), 0.04), ((-0.085, -0.11, 0.55), 0.03)],  # right thigh
    [((-0.11, 0.13, 1.31), 0.075), ((-0.05, 0.14, 1.24), 0.045)],                     # back, right shoulder blade
    [((0.30, -0.02, 1.39), 0.045), ((0.34, -0.02, 1.36), 0.03)],                      # left sleeve
]
GRIME = (0.30, 0.24, 0.18)
GRIME_Z, GRIME_ALPHA = (0.46, 0.08), 0.45     # trouser legs: from clean at 0.46 m to most at 0.08 m
# Tears: a ragged rip (skin showing, a dark frayed rim) on the front of the clothes: ((x, z), half width,
# half height, phase). The left knee; the tee's hem on the right (its bottom edge runs off the hem).
TEARS = [((0.095, 0.50), 0.055, 0.028, 1.0, "Legs"), ((-0.15, 0.935), 0.035, 0.035, 2.0, "Torso")]  # ..., region
RIM, RIM_DARK = 0.007, (0.17, 0.15, 0.14)


def build(c):
    c.extras = {
        "ai_generated": True,
        "generator": "Meshy (Pro): survivor-01's body (nano-banana-pro image, meshy-t2.5 image-to-3D, auto-rig) and "
                     "animation; zombie texture, stoop, clip clean-up and LOD in Blender",
        "meshy_tasks": "body: image 01a16ce1-c617-761c-b4f2-2a283e4646f1, model 01a16ce2-f3ae-74b0-ac00-467749581ab6, "
                       "rig 01a16ce5-1792-7245-b97e-9ca41b7aa2dd, idle 01a16ce5-b344-7075-9734-5fd40440002e; "
                       "zombie clips 01a16cea-acc2-706c-931e-5c002c4d37be",
        "author": "Dead Commons",
    }
    spec = c.spec
    folder = c.paths["folder"]

    # 1. The zombie clips, in metres, the hips on the origin (as survivor-01's were).
    zarm, zmesh = c.import_glb(c.raw)
    g.merge_clip(zarm, c.paths["raw"] / FALL_GLB, FALL[0])  # the bought fall, same rig
    hx, hy = c.hips_xy(zarm)
    c.apply_transforms(zarm, zmesh, offset=(-hx, -hy))
    zrest = {b.name: b.matrix_local.copy() for b in zarm.data.bones}
    meshy = {a.name for a in bpy.data.actions}
    for o in (zmesh, zarm):
        bpy.data.objects.remove(o)
    g._purge_orphans()

    # 2. survivor-01's body, rig, hoodie and idle, as they are.
    arm, body, hoodie = append_survivor(folder / spec["body_from"])
    diff = max((arm.data.bones[n].matrix_local.to_translation() - m.to_translation()).length for n, m in zrest.items())
    assert diff < 1e-3, f"the clips' rig differs from survivor-01's by {diff * 1000:.1f} mm"
    c.note(f"body, rig, idle and hoodie from {spec['body_from']} (the clips' rig matches it within {diff * 1000:.2f} mm)")

    # 3. Clips.
    clips = make_clips(c, arm, body, meshy)

    # 4. Texture.
    paint_texture(c, body)

    # 5. Names, NLA, shading as the survivor (already on the mesh), the hoodie checked over every clip.
    register_clips(arm, clips)
    g.fix_slots(arm)
    worn = {"hoodie"}
    hidden = set(next(x for x in g.wearables(spec) if x["id"] == "hoodie")["hides"])
    c.poke_through = {"hoodie": g.poke_through(body, hoodie, arm, clips, hidden)}
    c.note("hoodie (survivor-01's, unmodified): poke-through, worst frame per clip: " + g.describe_poke(c.poke_through["hoodie"]))
    c.note(f"heights: rest {height(body, arm, None):.3f} m; rest with the stoop {stooped_rest_height(body, arm, spec):.3f} m; "
           f"idle with the stoop {height(body, arm, 'idle'):.3f} m (mean over the loop {height(body, arm, 'idle', mean=True):.3f}; "
           f"spec {spec['stooped_height']})")

    # 6. Crowd LOD, kept out of the Asset collection (validated, rendered and exported on its own).
    lod = make_lod(c, body, arm)
    c.after_build = lambda report, render=True, export=True: after_build(c, report, arm, body, hoodie, lod, render, export)
    return body


# ---------------------------------------------------------------- the survivor's parts

def append_survivor(blend):
    want = ["survivor_01_rig", "survivor_01", "survivor_01_hoodie"]
    before = set(bpy.data.actions)
    with bpy.data.libraries.load(str(blend), link=False) as (src, dst):
        dst.objects = list(want)
    arm, body, hoodie = dst.objects  # the appended objects, in that order
    coll = zasset.collection(zasset.ASSET_COLLECTION)
    for o in (arm, body, hoodie):
        coll.objects.link(o)
    ad = arm.animation_data
    for t in list(ad.nla_tracks):
        ad.nla_tracks.remove(t)
    ad.action = None
    for a in [a for a in bpy.data.actions if a not in before]:
        if a.name == "idle":
            a.name = "idle_survivor"
        else:
            bpy.data.actions.remove(a)
    arm.name = arm.data.name = "zombie_02_rig"
    body.name = body.data.name = "zombie_02"
    body.data.materials[0].name = "zombie_02"
    hoodie.name = "zombie_02_hoodie"  # the object; its mesh stays survivor_01_hoodie, unmodified
    return arm, body, hoodie


# ---------------------------------------------------------------- clip data (keys at whole frames)

def rot_path(bone):
    return f'pose.bones["{bone}"].rotation_quaternion'


def loc_path(bone):
    return f'pose.bones["{bone}"].location'


def remake(action, times, name=None):
    """A new action (or `action` itself, `name` None) keyed at frames 0..n-1 with `action`'s values at
    source `times`: a cut, a stretch played there and back, every channel keyed every frame, linear."""
    src = action
    out = action.copy() if name else action
    if name:
        out.name = name
    vals = {(fc.data_path, fc.array_index): [fc.evaluate(t) for t in times] for fc in zchar.action_fcurves(src)}
    for fc in zchar.action_fcurves(out):
        v = vals[(fc.data_path, fc.array_index)]
        fc.keyframe_points.clear()
        fc.keyframe_points.add(len(v))
        for i, (k, y) in enumerate(zip(fc.keyframe_points, v)):
            k.co = (i, y)
            k.interpolation = "LINEAR"
        fc.update()
    out.use_frame_range = True
    out.frame_start, out.frame_end = 0, len(times) - 1
    out.use_fake_user = True
    return out


def read(action):
    return {(fc.data_path, fc.array_index): np.array([k.co.y for k in fc.keyframe_points]) for fc in zchar.action_fcurves(action)}


def write(action, data):
    for fc in zchar.action_fcurves(action):
        v = data[(fc.data_path, fc.array_index)]
        for k, y in zip(fc.keyframe_points, v):
            k.co.y = float(y)
            k.handle_left.y = k.handle_right.y = float(y)
        fc.update()


def frames_of(data):
    return len(next(iter(data.values())))


def quat(data, bone):
    return np.stack([data[(rot_path(bone), i)] for i in range(4)], axis=1)


def set_quat(data, bone, q):
    q = q / np.linalg.norm(q, axis=1, keepdims=True)
    for i in range(1, len(q)):  # no sign flips between frames
        if np.dot(q[i], q[i - 1]) < 0:
            q[i] = -q[i]
    for i in range(4):
        data[(rot_path(bone), i)] = q[:, i]


def bones_of(data):
    return sorted({p.split('"')[1] for p, _ in data if p.endswith("rotation_quaternion")})


def pose_at(data, i):
    return {k: float(v[i]) for k, v in data.items()}


def smooth(u):
    u = np.clip(u, 0.0, 1.0)
    return u * u * (3 - 2 * u)


def blend(data, pose, frames, at_start=True):
    """Eases the first (or last) `frames` frames from (to) `pose`: smoothstep, quaternions aligned in sign."""
    n = frames_of(data)
    k = np.arange(n, dtype=float)
    w = 1.0 - smooth(k / frames) if at_start else smooth((k - (n - 1 - frames)) / frames)  # weight of `pose`
    for b in bones_of(data):
        if (rot_path(b), 0) not in pose:  # a leaf bone the pose's clip doesn't key (e.g. head_end): left as it is
            continue
        q = quat(data, b)
        p = np.array([pose[(rot_path(b), i)] for i in range(4)])
        p = np.where((q @ p)[:, None] < 0, -p, p)
        set_quat(data, b, q * (1 - w)[:, None] + p * w[:, None])
        for i in range(3):
            key = (loc_path(b), i)
            if key in data and key in pose:
                data[key] = data[key] * (1 - w) + pose[key] * w


def hips_world(arm, data):
    """The hips' offsets in armature space per frame ((n, 3)): rest frame @ location."""
    rest = arm.data.bones["Hips"].matrix_local.to_3x3()
    loc = np.stack([data[(loc_path("Hips"), i)] for i in range(3)], axis=1)
    return loc @ np.array(rest).T, np.array(rest)


def set_hips_world(data, world, rest):
    loc = world @ np.linalg.inv(rest).T
    for i in range(3):
        data[(loc_path("Hips"), i)] = loc[:, i]


def centre(arm, data):
    """The hips over the origin at the first frame (the clip's own drift is kept)."""
    w, rest = hips_world(arm, data)
    w[:, :2] -= w[0, :2]
    set_hips_world(data, w, rest)


def pin_travel(arm, data, threshold=0.05):
    """In place: every move of the hips along the clip's travel (first to last frame, across the floor)
    taken out; sway across it and up and down kept. Returns the metres taken out."""
    w, rest = hips_world(arm, data)
    d = w[-1, :2] - w[0, :2]
    m = float(np.linalg.norm(d))
    if m < threshold:
        return m
    u = d / m
    along = (w[:, :2] - w[0, :2]) @ u
    w[:, :2] -= along[:, None] * u[None, :]
    set_hips_world(data, w, rest)
    return m


def yaw(arm, data, degrees):
    """The whole clip turned about the vertical axis through the origin."""
    q = g.layer_quaternion(arm, "Hips", (0.0, 0.0, degrees))
    qa = np.array(q[:])
    hq = quat(data, "Hips")
    # quaternion product q @ hq, per frame
    w1, x1, y1, z1 = qa
    w2, x2, y2, z2 = hq.T
    set_quat(data, "Hips", np.stack([w1 * w2 - x1 * x2 - y1 * y2 - z1 * z2, w1 * x2 + x1 * w2 + y1 * z2 - z1 * y2,
                                     w1 * y2 - x1 * z2 + y1 * w2 + z1 * x2, w1 * z2 + x1 * y2 - y1 * x2 + z1 * w2], axis=1))
    w, rest = hips_world(arm, data)
    rz = np.array(Matrix.Rotation(math.radians(degrees), 3, "Z"))
    set_hips_world(data, w @ rz.T, rest)


def arms_from(data, pose, bones=ARMS):
    """The arms held in `pose` the whole clip (a fighter's guard dropped for hanging arms)."""
    n = frames_of(data)
    for b in bones:
        p = np.array([pose[(rot_path(b), i)] for i in range(4)])
        set_quat(data, b, np.tile(p, (n, 1)))


def ease_loop(data, frames):
    """A loop's last `frames` frames eased into its first pose, so it plays round without a jump."""
    blend(data, pose_at(data, 0), frames, at_start=False)


# ---------------------------------------------------------------- evaluating poses

def _bind(arm, action):
    return g._bind_action(arm, action)


def eval_body(body):
    dg = bpy.context.evaluated_depsgraph_get()
    ev = body.evaluated_get(dg)
    me = ev.to_mesh()
    co = np.empty(len(me.vertices) * 3)
    me.vertices.foreach_get("co", co)
    ev.to_mesh_clear()
    return co.reshape(-1, 3)


def uprightness(arm, action):
    """Per frame: how upright the torso is (z of the unit vector from the hips to the neck)."""
    _bind(arm, action)
    pb = arm.pose.bones
    out = []
    for f in range(int(action.frame_start), int(action.frame_end) + 1):
        bpy.context.scene.frame_set(f)
        v = pb["neck"].head - pb["Hips"].head
        out.append(v.normalized().z)
    return out


def stoop(arm, action, layer):
    """The spec's stoop over `action`, faded out as the torso leaves upright. Returns the weights."""
    up = uprightness(arm, action)
    w = [float(smooth((u - UP_NONE) / (UP_FULL - UP_NONE))) for u in up]
    g.apply_pose_layer(arm, action, layer, weight=lambda t: w[min(len(w) - 1, max(0, int(round(t))))])
    return w


def ground(arm, body, action):
    """Every frame's lowest point of the body put on the floor (z = 0) by moving the hips up or down.
    Returns (lowest, highest) of the lowest points before: sinking below 0, floating above it."""
    _bind(arm, action)
    data = read(action)
    w, rest = hips_world(arm, data)
    lows = []
    for f in range(frames_of(data)):
        bpy.context.scene.frame_set(f)
        lows.append(float(eval_body(body)[:, 2].min()))
    w[:, 2] -= np.array(lows)
    set_hips_world(data, w, rest)
    write(action, data)
    return min(lows), max(lows)


def bone_heads(arm, action, frame):
    _bind(arm, action)
    bpy.context.scene.frame_set(int(frame))
    return [b.head.copy() for b in arm.pose.bones]


def join_error(arm, a, fa, b, fb):
    """Mean distance (m) between every bone head in `a` at `fa` and `b` at `fb` (armature space)."""
    pa, pb_ = bone_heads(arm, a, fa), bone_heads(arm, b, fb)
    return sum((x - y).length for x, y in zip(pa, pb_)) / len(pa)


def height(body, arm, action, mean=False):
    ad = arm.animation_data
    if action is None:
        arm.data.pose_position = "REST"
        bpy.context.view_layer.update()
        h = float(eval_body(body)[:, 2].max())
        arm.data.pose_position = "POSE"
        return h
    a = bpy.data.actions[action]
    _bind(arm, a)
    hs = []
    for f in range(int(a.frame_start), int(a.frame_end) + 1):
        bpy.context.scene.frame_set(f)
        hs.append(float(eval_body(body)[:, 2].max()))
        if not mean:
            break
    ad.action = None
    return sum(hs) / len(hs)


def stooped_rest_height(body, arm, spec):
    g.set_pose_layer(arm, spec["stoop"])
    h = float(eval_body(body)[:, 2].max())
    g.set_pose_layer(arm, None)
    return h


# ---------------------------------------------------------------- the clips

def make_clips(c, arm, body, meshy):
    spec = c.spec
    A = bpy.data.actions
    out = {}

    def cut(src, f0, f1, name):
        return remake(A[src], list(range(f0, f1 + 1)), name)

    # Idle: the survivor's approved calm idle, as it is (its keys are whole frames already).
    idle = A["idle_survivor"]
    idle.name = "idle"
    out["idle"] = idle
    idle_raw = read(idle)
    idle0 = pose_at(idle_raw, 0)

    walk = cut(*WALK, "walk")
    d = read(walk)
    centre(arm, d)
    ease_loop(d, 4)
    write(walk, d)
    out["walk"] = walk
    c.note(f"walk = Meshy's Injured Walk (in place), one cycle of six: frames {WALK[1]}-{WALK[2]} ({(WALK[2] - WALK[1]) / FPS:.2f} s), "
           "last 4 frames eased into the first")

    attack = cut(*ATTACK, "attack")
    d = read(attack)
    centre(arm, d)
    ease_loop(d, LOOP_EASE["attack"])
    write(attack, d)
    out["attack"] = attack
    c.note(f"attack = Meshy's Punch Forward with Both Fists, frames {ATTACK[1]}-{ATTACK[2]} (arms flung wide, then both "
           f"forward together, held, drawn back; the boxer's guard before and after cut), looped ({LOOP_EASE['attack']} frames eased)")

    a, b, n = THUMP[1], THUMP[2], 20
    thump = remake(A[THUMP[0]], [a + (b - a) * (1 - math.cos(2 * math.pi * k / n)) / 2 for k in range(n + 1)], "thump")
    d = read(thump)
    centre(arm, d)
    write(thump, d)
    out["thump"] = thump
    c.note(f"thump = Meshy's Punch Forward with Both Fists, frames {a}-{b} played there and back ({n / FPS:.2f} s): "
           "both fists driven forward together and drawn back, a pounding loop")

    for name, (src, f0, f1) in (("stagger", STAGGER), ("stagger_2", STAGGER_2)):
        clip = cut(src, f0, f1, name)
        d = read(clip)
        yaw(arm, d, YAW[name])
        centre(arm, d)
        moved = pin_travel(arm, d)
        if name == "stagger_2":
            arms_from(d, idle0)
        blend(d, idle0, EASE_IN[name])
        blend(d, idle0, EASE_OUT[name], at_start=False)
        write(clip, d)
        out[name] = clip
        c.note(f"{name} = Meshy's {src.replace('_', ' ')}, frames {f0}-{f1}, turned {YAW[name]:+.0f}° to face the front, "
               f"{moved:.2f} m of travel taken out" + (", the boxer's guard replaced by the idle's hanging arms" if name == "stagger_2" else "")
               + f", eased in from and out to the idle ({EASE_IN[name]}/{EASE_OUT[name]} frames)")

    fall = cut(*FALL, "fall")
    d = read(fall)
    yaw(arm, d, YAW["fall"])
    centre(arm, d)
    moved = pin_travel(arm, d)
    blend(d, idle0, EASE_IN["fall"])
    write(fall, d)
    out["fall"] = fall
    down_raw = pose_at(d, frames_of(d) - 1)
    c.note(f"fall = Meshy's Shot and Slow Fall Backward (bought for it), frames {FALL[1]}-{FALL[2]} ({(FALL[2] - FALL[1]) / FPS:.2f} s: "
           f"the reel turning into the topple, onto his back, lying still), turned {YAW['fall']:+.0f}°, {moved:.2f} m of travel "
           f"taken out, eased in from the idle ({EASE_IN['fall']} frames)")

    # Down: the fall's last pose held, breathing.
    down = remake(fall, [FALL[2] - FALL[1]] * (DOWN_FRAMES + 1), "down")
    out["down"] = down
    g.apply_pose_layer(arm, down, DOWN_BREATH, weight=lambda t: (1 - math.cos(2 * math.pi * t / DOWN_FRAMES)) / 2)
    c.note(f"down = the fall's last pose held, {DOWN_FRAMES} frames ({DOWN_FRAMES / FPS:.1f} s), the chest rising and falling a little")

    for name, (src, f0, f1) in (("die_back", DIE_BACK), ("die_front", DIE_FRONT)):
        clip = cut(src, f0, f1, name)
        d = read(clip)
        centre(arm, d)
        moved = pin_travel(arm, d)
        blend(d, idle0, EASE_IN[name])
        write(clip, d)
        out[name] = clip
        c.note(f"{name} = Meshy's {src.replace('_', ' ')}, frames {f0}-{f1}, {moved:.2f} m of travel taken out, eased in from the idle "
               f"({EASE_IN[name]} frames)")

    src, g0, g1 = GET_UP
    get_up = remake(A[src], [g0 + (g1 - g0) * k / GET_UP_FRAMES for k in range(GET_UP_FRAMES + 1)], "get_up")
    d = read(get_up)
    centre(arm, d)
    moved = pin_travel(arm, d)
    write(get_up, d)
    raw_join = join_error(arm, fall, fall.frame_end, get_up, 0)
    blend(d, down_raw, EASE_IN["get_up"])
    blend(d, idle0, EASE_OUT["get_up"], at_start=False)
    write(get_up, d)
    out["get_up"] = get_up
    c.note(f"get_up = Meshy's Stand Up 1, frames {g0}-{g1} ({(g1 - g0) / FPS:.1f} s) played evenly in {GET_UP_FRAMES} frames "
           f"({GET_UP_FRAMES / FPS:.1f} s, x{(g1 - g0) / GET_UP_FRAMES:.2f}); it starts lying on its back, {raw_join * 100:.1f} cm (mean "
           f"bone head) from where the fall ends before easing; {moved:.2f} m of travel taken out, eased in from the down pose "
           f"({EASE_IN['get_up']} frames) and out to the idle ({EASE_OUT['get_up']})")

    # Die while down: from the down pose, a last heave of the chest, then limp: the head rolls aside.
    die_down = remake(fall, [FALL[2] - FALL[1]] * (DIE_DOWN_FRAMES + 1), "die_down")
    out["die_down"] = die_down
    g.apply_pose_layer(arm, die_down, DIE_DOWN_HEAVE, weight=lambda t: math.sin(math.pi * min(t, 14) / 14))
    g.apply_pose_layer(arm, die_down, DIE_DOWN_LIMP, weight=lambda t: float(smooth((t - 8) / 20)))
    c.note(f"die_down = authored from the down pose: a last heave of the chest, then limp, the head rolling to one side "
           f"({DIE_DOWN_FRAMES} frames)")

    groan = cut(*GROAN, "groan")
    d = read(groan)
    centre(arm, d)
    blend(d, idle0, EASE_IN["groan"])
    blend(d, idle0, EASE_OUT["groan"], at_start=False)
    write(groan, d)
    out["groan"] = groan
    c.note(f"groan = Meshy's Zombie Scream, frames {GROAN[1]}-{GROAN[2]}, eased in from and out to the idle")

    # The Meshy originals go (raw/ keeps them).
    for nm in meshy:
        if nm in A:
            A.remove(A[nm])

    # The stoop over every clip, then every frame on the ground.
    layer = spec["stoop"]
    for name, a in out.items():
        w = stoop(arm, a, layer)
        lo, hi = ground(arm, body, a)
        a.use_cyclic = name in ("idle", "walk", "attack", "thump", "down")
        c.note(f"{name}: {int(a.frame_end) + 1} frames ({a.frame_end / FPS:.2f} s, {'loop' if a.use_cyclic else 'one-shot'}); "
               f"stoop weight {min(w):.2f}-{max(w):.2f}; before grounding the lowest point was {lo * 100:+.1f} to {hi * 100:+.1f} cm")

    # Joins: where clips meet (mean bone-head distance).
    joins = []
    for name in ("stagger", "stagger_2", "fall", "die_back", "die_front", "groan"):
        joins.append(f"idle→{name} {join_error(arm, out['idle'], 0, out[name], 0) * 100:.1f} cm")
    for name in ("stagger", "stagger_2", "groan", "get_up"):
        joins.append(f"{name}→idle {join_error(arm, out[name], out[name].frame_end, out['idle'], 0) * 100:.1f} cm")
    joins.append(f"fall→down {join_error(arm, out['fall'], out['fall'].frame_end, out['down'], 0) * 100:.1f} cm")
    joins.append(f"down→get_up {join_error(arm, out['down'], 0, out['get_up'], 0) * 100:.1f} cm")
    joins.append(f"down→die_down {join_error(arm, out['down'], 0, out['die_down'], 0) * 100:.1f} cm")
    for name in ("walk", "attack", "thump", "idle"):
        joins.append(f"{name} loop {join_error(arm, out[name], out[name].frame_end, out[name], 0) * 100:.1f} cm")
        joins.append(f"idle→{name} {join_error(arm, out['idle'], 0, out[name], 0) * 100:.1f} cm")
    c.note("joins (mean bone-head distance): " + ", ".join(joins))
    arm.animation_data.action = None
    return list(spec["animations"])


def register_clips(arm, names):
    ad = arm.animation_data or arm.animation_data_create()
    for t in list(ad.nla_tracks):
        ad.nla_tracks.remove(t)
    ad.action = None
    for name in names:
        a = bpy.data.actions[name]
        a.use_fake_user = True
        track = ad.nla_tracks.new()
        track.name = name
        track.strips.new(name, int(a.frame_range[0]), a)
        track.mute = True


# ---------------------------------------------------------------- texture

def texel_map(obj, w, h):
    """Every texel the body's UV triangles cover: its rest-pose position (h, w, 3) and region index
    (h, w; -1 none), by rasterising each triangle in UV space."""
    me = obj.data
    me.calc_loop_triangles()
    uv = me.uv_layers.active.data
    reg = g.region_of_vertices(obj)
    ridx = {r: i for i, r in enumerate(g.REGIONS)}
    pos = np.zeros((h, w, 3), np.float32)
    region = np.full((h, w), -1, np.int8)
    for t in me.loop_triangles:
        uvs = np.array([uv[li].uv[:] for li in t.loops]) * [w, h]
        ps = np.array([me.vertices[v].co[:] for v in t.vertices])
        rs = [reg.get(v) for v in t.vertices]
        r = max(set(rs), key=rs.count)
        x0, y0 = np.floor(uvs.min(0)).astype(int) - 1
        x1, y1 = np.ceil(uvs.max(0)).astype(int) + 1
        x0, y0, x1, y1 = max(x0, 0), max(y0, 0), min(x1, w - 1), min(y1, h - 1)
        if x1 < x0 or y1 < y0:
            continue
        xs, ys = np.meshgrid(np.arange(x0, x1 + 1) + 0.5, np.arange(y0, y1 + 1) + 0.5)
        a, b, cc = uvs
        den = (b[1] - cc[1]) * (a[0] - cc[0]) + (cc[0] - b[0]) * (a[1] - cc[1])
        if abs(den) < 1e-12:
            continue
        l1 = ((b[1] - cc[1]) * (xs - cc[0]) + (cc[0] - b[0]) * (ys - cc[1])) / den
        l2 = ((cc[1] - a[1]) * (xs - cc[0]) + (a[0] - cc[0]) * (ys - cc[1])) / den
        l3 = 1 - l1 - l2
        e = -0.6 / max(np.linalg.norm(b - a), np.linalg.norm(cc - a), 1.0)
        inside = (l1 >= e) & (l2 >= e) & (l3 >= e)
        p = l1[..., None] * ps[0] + l2[..., None] * ps[1] + l3[..., None] * ps[2]
        sp, sr = pos[y0:y1 + 1, x0:x1 + 1], region[y0:y1 + 1, x0:x1 + 1]
        sp[inside] = p[inside]
        sr[inside] = ridx[r]
    return pos, region


def wobble(p, k=1.0):
    """Low-frequency 3D noise in about -1..1: ragged edges for big simple shapes."""
    x, y, z = p[..., 0], p[..., 1], p[..., 2]
    return (np.sin(41 * x * k + 13 * z * k + 1.3) + np.sin(29 * z * k - 23 * y * k + 0.4) + np.sin(37 * y * k + 31 * x * k + 2.1)) / 3


def paint_texture(c, body):
    img = zchar.base_color_image(body)
    w, h = img.size
    px = np.empty(w * h * 4, np.float32)
    img.pixels.foreach_get(px)
    px = px.reshape(h, w, 4)
    rgb = px[..., :3].copy()
    pos, region = texel_map(body, w, h)
    R = {r: i for i, r in enumerate(g.REGIONS)}
    covered = region >= 0
    lum = rgb @ np.array([0.2126, 0.7152, 0.0722], np.float32)
    warm = rgb[..., 0] - rgb[..., 2]
    x, y, z = pos[..., 0], pos[..., 1], pos[..., 2]

    # Skin: by region and colour (the survivor's skin is pale and warm; the tee neutral grey; hair dark).
    head = region == R["Head"]
    skin = (head & (lum > 0.40) & ((warm > 0.012) | (z > 1.47)))
    skin |= (region == R["Hands"]) & (lum > 0.30)
    skin |= (region == R["Arms"]) & (lum > 0.45) & (warm > 0.022)
    skin |= (region == R["Torso"]) & (lum > 0.50) & (warm > 0.03) & (z > 1.38)
    ref = float(np.median(lum[skin]))
    k = 1.0 + (lum / ref - 1.0) * SKIN_CONTRAST
    zombie = np.clip(np.array(ZOMBIE_SKIN, np.float32)[None, None, :] * (SKIN_LEVEL * k)[..., None], 0, 1)
    # A few big darker blotches on the skin (left forearm, right side of the neck, right cheek).
    blot = np.zeros((h, w), np.float32)
    for (bx, by, bz), r in (((0.50, -0.01, 1.37), 0.09), ((-0.06, -0.02, 1.47), 0.05), ((-0.06, -0.10, 1.58), 0.035)):
        dd = np.sqrt((x - bx) ** 2 + (y - by) ** 2 + (z - bz) ** 2) / r + 0.25 * wobble(pos, 1.5)
        blot = np.maximum(blot, smooth(1.0 - dd) * 1.0)
    zombie *= (1.0 - 0.14 * blot)[..., None]
    rgb[skin] = zombie[skin]
    c.note(f"skin recoloured: {skin.sum()} texels (Head {(skin & head).sum()}, Arms {(skin & (region == R['Arms'])).sum()}, "
           f"Hands {(skin & (region == R['Hands'])).sum()}, Torso {(skin & (region == R['Torso'])).sum()}) to #8A9A7C "
           f"scaled by the survivor's light and shade (median skin luminance {ref:.2f})")

    # The face, found from the texture: pupils (the darkest spots either side of the nose at eye height)
    # and the mouth line (the darkest row under the nose).
    face = head & (y < -0.09)
    eyes = []
    for side in (-1, 1):
        m = face & (side * x > 0.02) & (side * x < 0.075) & (z > 1.60) & (z < 1.67)
        i = np.argmin(np.where(m, lum, 9.0))
        eyes.append(pos.reshape(-1, 3)[i].copy())
    m = face & (np.abs(x) < 0.015) & (z > 1.52) & (z < 1.60)
    zs = z[m]
    ls = lum[m]
    bins = np.round(zs / 0.004) * 0.004
    rows = {b: ls[bins == b].mean() for b in np.unique(bins)}
    mouth_z = float(min(rows, key=rows.get))
    c.note(f"face: pupils at ({eyes[0][0]:+.3f}, {eyes[0][2]:.3f}) and ({eyes[1][0]:+.3f}, {eyes[1][2]:.3f}), mouth line at z {mouth_z:.3f}")

    # Sunken eyes: the sockets darkened, most round the eye and a little under it.
    sock = np.zeros((h, w), np.float32)
    for e in eyes:
        dx = (x - e[0]) / 0.034
        dz = (z - (e[2] - 0.004)) / 0.026
        sock = np.maximum(sock, smooth(1.2 - np.sqrt(dx * dx + dz * dz)))
    sock *= face
    rgb = rgb * (1 - SOCKET_STRENGTH * sock[..., None]) + np.array(SOCKET_DARK) * (SOCKET_STRENGTH * sock[..., None])

    # A slack mouth: a dark opening on the mouth line, its middle hanging lower.
    hw, hh = MOUTH_HALF
    u = x / hw
    zc = mouth_z - 0.003 - 0.004 * np.clip(1 - u * u, 0, 1)
    dm = np.sqrt(u * u + ((z - zc) / hh) ** 2)
    mouth = smooth((1.0 - dm) * 4) * face
    rgb = rgb * (1 - 0.9 * mouth[..., None]) + np.array(MOUTH_DARK) * (0.9 * mouth[..., None])

    # Clothes: everything covered that isn't skin, below the head, but not the shoes.
    clothes = covered & ~skin & np.isin(region, [R["Torso"], R["Arms"], R["Legs"]])
    # Each stain the union of its spheres (a distance field), its edge made ragged by low noise: one
    # big simple shape, flat inside, a narrow soft edge.
    field = np.full((h, w), 9.0, np.float32)
    for chain in STAINS:
        for (sx, sy, sz), r in chain:
            field = np.minimum(field, np.sqrt((x - sx) ** 2 + (y - sy) ** 2 + (z - sz) ** 2) - r)
    field += STAIN_RAGGED * wobble(pos, 1.6)
    st = smooth(0.5 - field / STAIN_EDGE) * clothes
    a = (STAIN_ALPHA * st)[..., None]
    rgb = rgb * (1 - a) + rgb * np.array(STAIN) * a
    legs = clothes & (region == R["Legs"])
    gz = smooth((GRIME_Z[0] - z) / (GRIME_Z[0] - GRIME_Z[1]) + 0.15 * wobble(pos, 2.0)) * legs
    a = (GRIME_ALPHA * gz)[..., None]
    rgb = rgb * (1 - a) + (rgb * 0.5 + np.array(GRIME) * 0.55) * a

    # Tears: a ragged hole in the front of the clothes, skin showing through, a dark frayed rim.
    torn = 0
    for (tx, tz), rx, rz, ph, tr in TEARS:
        u, v = (x - tx) / rx, (z - tz) / rz
        ang = np.arctan2(v, u)
        rad = 1 + 0.14 * np.sin(3 * ang + ph) + 0.12 * np.sin(7 * ang + 2 * ph) + 0.09 * np.sign(np.sin(11 * ang + ph))
        r = rx * rz / np.sqrt((rz * np.cos(ang)) ** 2 + (rx * np.sin(ang)) ** 2)  # the ellipse's radius that way
        rad = rad * r
        dist = np.sqrt((x - tx) ** 2 + (z - tz) ** 2)
        front = clothes & (y < -0.03) & (region == R[tr])
        hole = front & (dist < rad)
        rim = front & (dist >= rad) & (dist < rad + RIM)
        sk = np.array(ZOMBIE_SKIN, np.float32) * 0.82
        rgb[hole] = sk
        rgb[rim] = np.array(RIM_DARK)
        torn += int(hole.sum())
    c.note(f"stains: {len(STAINS)} big shapes ({int((st > 0.5).sum())} texels); grime on the trouser legs below "
           f"{GRIME_Z[0]} m; tears: left knee and tee hem ({torn} texels)")

    # Into the padding round every UV island, so filtering at the seams picks up the new colours.
    rgb = dilate(rgb, covered, 8)
    out = np.concatenate([np.clip(rgb, 0, 1), np.ones((h, w, 1), np.float32)], axis=2)
    new = bpy.data.images.new("zombie_02_painted", w, h, alpha=False)
    new.pixels.foreach_set(out.astype(np.float32).ravel())
    mat = body.data.materials[0]
    for nd in mat.node_tree.nodes:
        if nd.type == "TEX_IMAGE" and nd.image == img:
            nd.image = new
    if img.users == 0:
        bpy.data.images.remove(img)
    c.tone_texture(body, c.paths["textures"] / "zombie-02_base_color.jpg", saturation=0.92, contrast=0.97, lift=0.0)
    zchar.base_color_image(body).name = "zombie_02_base_color"
    c.note("base colour: the survivor's toned texture repainted, then toned again a little (saturation 0.92, contrast 0.97)")


def dilate(rgb, mask, steps):
    rgb = rgb.copy()
    m = mask.copy()
    for _ in range(steps):
        acc = np.zeros_like(rgb)
        cnt = np.zeros(m.shape, np.float32)
        for dy, dx in ((0, 1), (0, -1), (1, 0), (-1, 0)):
            sm = np.roll(np.roll(m, dy, 0), dx, 1)
            acc += np.roll(np.roll(rgb, dy, 0), dx, 1) * sm[..., None]
            cnt += sm
        grow = ~m & (cnt > 0)
        rgb[grow] = acc[grow] / cnt[grow][:, None]
        m |= grow
    return rgb


# ---------------------------------------------------------------- crowd LOD

def make_lod(c, body, arm):
    spec = c.spec
    want = spec["lod"]["tris"]
    weights = spec["lod"].get("weights") or {}
    lod = body.copy()
    lod.data = body.data.copy()
    lod.name = lod.data.name = f"{spec['id']}_lod1"
    coll = bpy.data.collections.get("LOD") or bpy.data.collections.new("LOD")
    if coll.name not in bpy.context.scene.collection.children:
        bpy.context.scene.collection.children.link(coll)
    coll.objects.link(lod)
    reg = g.region_of_vertices(body)
    # Decimate (collapse) steered by a vertex group: a region weighing less than 1 is collapsed later
    # (the head, hands and feet keep more of their shape; the cut goes on the torso and limbs). The
    # ratio is found by bisection so the result lands on the budget.
    grp = lod.vertex_groups.new(name="lod_decimate")
    for r in g.REGIONS:
        idx = [i for i, x in reg.items() if x == r]
        if idx:
            grp.add(idx, float(weights.get(r, 1.0)), "REPLACE")
    for m in [m for m in lod.modifiers if m.type != "ARMATURE"]:
        lod.modifiers.remove(m)
    before = zchar.tris(lod)

    def decimated(ratio, apply_to=None):
        o = apply_to
        if o is None:
            o = lod.copy()
            o.data = lod.data.copy()
            coll.objects.link(o)
        for m in [m for m in o.modifiers if m.type != "ARMATURE"]:
            o.modifiers.remove(m)
        mod = o.modifiers.new("decimate", "DECIMATE")
        mod.decimate_type = "COLLAPSE"
        mod.ratio = ratio
        mod.use_collapse_triangulate = True
        mod.vertex_group = grp.name
        mod.vertex_group_factor = 1.0
        zchar._select([o], o)
        while o.modifiers[0] != mod:
            bpy.ops.object.modifier_move_up(modifier=mod.name)
        bpy.ops.object.modifier_apply(modifier=mod.name)
        t = zchar.tris(o)
        if apply_to is None:
            me = o.data
            bpy.data.objects.remove(o)
            bpy.data.meshes.remove(me)
        return t

    lo_r, hi_r = 0.02, 1.0
    for _ in range(16):
        mid = (lo_r + hi_r) / 2
        if decimated(mid) > want:
            hi_r = mid
        else:
            lo_r = mid
    decimated(lo_r, apply_to=lod)
    lod.vertex_groups.remove(lod.vertex_groups["lod_decimate"])
    zchar.limit_weights(lod)
    # Regions again, each vertex taking the region of the nearest full-body vertex.
    from mathutils.kdtree import KDTree
    kd = KDTree(len(body.data.vertices))
    for v in body.data.vertices:
        kd.insert(v.co, v.index)
    kd.balance()
    assign = []
    for v in lod.data.vertices:
        _, i, _ = kd.find(v.co)
        assign.append(reg[i])
    for gname in [gr.name for gr in lod.vertex_groups if gr.name.startswith(g.REGION_PREFIX)]:
        lod.vertex_groups.remove(lod.vertex_groups[gname])
    groups = {r: lod.vertex_groups.new(name=g.region_group(r)) for r in g.REGIONS}
    for r in g.REGIONS:
        idx = [i for i, a in enumerate(assign) if a == r]
        if idx:
            groups[r].add(idx, 1.0, "REPLACE")
    attr = lod.data.attributes.get(g.REGION_ATTRIBUTE) or lod.data.attributes.new(g.REGION_ATTRIBUTE, "FLOAT", "POINT")
    attr.data.foreach_set("value", [float(g.REGIONS.index(a)) for a in assign])
    g.shade(lod, spec["shading"])
    lod.hide_render = lod.hide_viewport = True
    by = {r: 0 for r in g.REGIONS}
    for p in lod.data.polygons:
        rs = [assign[v] for v in p.vertices]
        by[max(set(rs), key=rs.count)] += len(p.vertices) - 2
    c.lod_stats = lod_stats(c, body, lod)
    c.note(f"crowd LOD {lod.name}: {before} -> {zchar.tris(lod)} triangles (" + ", ".join(f"{r} {n}" for r, n in by.items())
           + f"); {c.lod_stats}")
    return lod


def split_counts(obj):
    """Vertices as a GPU sees them: split where UVs differ (the VAT bake) and where UVs or normals
    differ (a glTF export with the auto-smooth shading)."""
    import zvat
    src, uvs, idx = zvat._split_vertices(obj.data)
    me = obj.data
    me.calc_loop_triangles()
    uv = me.uv_layers.active.data
    keys = set()
    for t in me.loop_triangles:
        for li, sn in zip(t.loops, t.split_normals):
            keys.add((me.loops[li].vertex_index, round(uv[li].uv[0], 5), round(uv[li].uv[1], 5),
                      round(sn[0], 3), round(sn[1], 3), round(sn[2], 3)))
    return len(me.vertices), len(src), len(keys)


def lod_stats(c, body, lod):
    out = []
    for o in (body, lod):
        v, uv, uvn = split_counts(o)
        out.append(f"{o.name}: {zchar.tris(o)} tris, {v} vertices, {uv} split at UV seams (VAT), {uvn} split at UV seams and hard edges (glTF)")
    return "; ".join(out)


# ---------------------------------------------------------------- after the build: the LOD's renders and export

def after_build(c, report, arm, body, hoodie, lod, render, export):
    spec = c.spec
    p = c.paths
    if render:
        report["renders"].append(render_lod_compare(c, arm, body, hoodie, lod, p["renders"] / "g3-r1_lod_26m.png", 26.0))
        report["renders"].append(render_lod_compare(c, arm, body, hoodie, lod, p["renders"] / "g3-r1_lod_12m.png", 12.0))
        report["renders"].append(render_lod_sheet(c, arm, body, lod, p["renders"] / "g3-r1_lod_iso.png"))
    if export:
        lod.hide_render = lod.hide_viewport = False
        for k, v in (c.extras or {}).items():
            lod[k] = v
        lod["lod_of"] = spec["id"]
        lod["regions"] = body.get("regions", "")
        lod["region_attribute"] = body.get("region_attribute", "")
        report["lod_glb"] = g._gltf(p["export"] / f"{spec['id']}_body_lod1.glb", [lod], arm, True)
        lod.hide_render = lod.hide_viewport = True
    report.setdefault("notes", []).append(f"LOD: {getattr(c, 'lod_stats', '')}")


def _mask(obj, hidden):
    for m in [m for m in obj.modifiers if m.type == "MASK"]:
        obj.modifiers.remove(m)
    if not hidden:
        return
    reg = g.region_of_vertices(obj)
    grp = obj.vertex_groups.get("hidden_by_garments") or obj.vertex_groups.new(name="hidden_by_garments")
    grp.remove([v.index for v in obj.data.vertices])
    idx = [i for i, r in reg.items() if r in hidden]
    if idx:
        grp.add(idx, 1.0, "REPLACE")
    m = obj.modifiers.new("hidden_by_garments", "MASK")
    m.vertex_group = grp.name
    m.invert_vertex_group = True


def _show(objs, all_objs):
    for o in all_objs:
        o.hide_render = o.hide_viewport = o not in objs


def render_lod_compare(c, arm, body, hoodie, lod, out, zoom):
    """The full body and the LOD side by side as the game draws them at `zoom` metres of world on a
    720-pixel-high view (game angle 0): idle and mid-walk, bare and in the hoodie, each tile rendered at
    the game's pixel size, then blown up (nearest) to see the pixels."""
    spec = c.spec
    scene = bpy.context.scene
    cam, sun = zasset._review_set(spec)
    yaw_, d, rot, right, up = zasset._view(0)
    px_per_m = 720.0 / zoom
    tile_m = 2.1
    size = int(round(tile_m * px_per_m))
    scene.render.resolution_x = scene.render.resolution_y = size
    cam.rotation_euler = rot.to_euler()
    cam.data.ortho_scale = tile_m
    cam.location = Vector((0, 0, 0.85)) + d * 20
    cam.data.clip_end = 60
    az = yaw_ + math.radians(55)
    sun.rotation_euler = Vector((math.sin(az) * 0.64, -math.cos(az) * 0.64, 0.77)).to_track_quat("Z", "Y").to_euler()
    ad = arm.animation_data
    for t in ad.nla_tracks:
        t.mute = True
    tmp = Path(out).parent / ".lod"
    tmp.mkdir(parents=True, exist_ok=True)
    files = []
    everyone = [body, hoodie, lod]
    hidden = {"Torso", "Arms"}
    for action, frame in (("idle", 0), ("walk", 9)):
        _bind(arm, bpy.data.actions[action])
        scene.frame_set(frame)
        for objs, masked in (([body], False), ([lod], False), ([body, hoodie], True), ([lod, hoodie], True)):
            _show(objs, everyone)
            for o in (body, lod):
                _mask(o, hidden if masked else set())
            f = tmp / f"{len(files)}.png"
            scene.render.filepath = str(f)
            bpy.ops.render.render(write_still=True)
            files.append(str(f))
    for o in (body, lod):
        _mask(o, set())
    _show([body, hoodie], everyone)
    lod.hide_render = lod.hide_viewport = True
    ad.action = None
    scene.frame_set(0)
    # Blow each tile up to about 400 px, nearest neighbour, so the game's pixels show.
    k = max(1, round(400 / size))
    big = []
    for i, f in enumerate(files):
        im = bpy.data.images.load(f, check_existing=False)
        a = np.empty(size * size * 4, np.float32)
        im.pixels.foreach_get(a)
        bpy.data.images.remove(im)
        a = a.reshape(size, size, 4).repeat(k, 0).repeat(k, 1)
        b = bpy.data.images.new(f"lod_big_{i}", size * k, size * k, alpha=False)
        b.pixels.foreach_set(a.ravel())
        bf = tmp / f"big_{i}.png"
        b.save_render(str(bf), scene=scene)
        bpy.data.images.remove(b)
        big.append(str(bf))
    zasset.montage(big, out, cols=4)
    for f in tmp.glob("*.png"):
        f.unlink()
    tmp.rmdir()
    scene.render.resolution_x = scene.render.resolution_y = zasset.RENDER_PX
    c.note(f"LOD compare at {zoom:g} m ({px_per_m:.1f} px/m, a tile {size} px, the figure ~{1.72 * px_per_m:.0f} px tall): "
           f"columns full, LOD, full + hoodie, LOD + hoodie; rows idle, walk: {out}")
    return str(out)


def render_lod_sheet(c, arm, body, lod, out):
    """The LOD's review sheet (four game angles, scale figure and door), in the rest pose stooped."""
    spec = c.spec
    body.hide_render = body.hide_viewport = True
    lod.hide_render = lod.hide_viewport = False
    hoodie = bpy.data.objects[f"{spec['id']}_hoodie"]
    hoodie.hide_render = hoodie.hide_viewport = True
    coll = zasset.collection(zasset.ASSET_COLLECTION)
    coll.objects.link(lod)  # render_angles frames the Asset collection
    g.set_pose_layer(arm, spec["stoop"])
    try:
        return zchar.render_angles(spec, out, posed=True)
    finally:
        g.set_pose_layer(arm, None)
        coll.objects.unlink(lod)
        body.hide_render = body.hide_viewport = False
        hoodie.hide_render = hoodie.hide_viewport = False
        lod.hide_render = lod.hide_viewport = True
