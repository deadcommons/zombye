"""survivor_01 clean-up and wearables: Meshy's rigged, animated GLB (raw/animations.glb) to our standard,
plus Meshy's free run and walk (raw/running.glb, raw/walking.glb) on the same armature, then the
wearables built in Blender over the body: a hoodie (skinned, hood down), glasses (rigid, on the eyes socket),
and the everyday basics: a t-shirt, jeans and trainers (skinned).

Run by zombye's tools/blender/zgarment.py (`build.py -- <zombye-art>/characters/survivor-01`, which sees
`wearables` in the spec), which then validates, renders and exports. Every number below is ours.
"""

import math

import bmesh
import bpy
from mathutils import Vector
from mathutils.bvhtree import BVHTree

import zchar
import zgarment as g

# Meshy action names -> ours. The walk is the in-place casual walk (calmer, closer to a weary man
# than the free walk); the free walk is kept as walk_alt for comparison.
ACTIONS = {"Idle": "idle", "Casual_Walk_inplace": "walk", "Cautious_Crouch_Walk_Forward_inplace": "sneak"}
EXTRA_CLIPS = [("running.glb", "run"), ("walking.glb", "walk_alt")]
# Directional clips (the owner: "he needs strafe animations"): Meshy's library on the same rig (one file)
# and a walk left from text-to-motion; walk right is walk left mirrored.
DIRECTIONAL = ("directional.glb", {
    "Walk_Backward_inplace": "walk_back",
    "Cautious_Crouch_Walk_Backward_inplace": "sneak_back",
    "Cautious_Crouch_Walk_Left_inplace": "sneak_left",
    "Cautious_Crouch_Walk_Right_inplace": "sneak_right",
    "BackLeft_run": "run_back_left",
    "BackRight_Run": "run_back_right",
})
WALK_LEFT = ("walk_left.glb", "walk_left")

# The hips' and thighs' vertices go by the colour under them: the shirt is light, the trousers dark.
SHIRT_ABOVE, TROUSERS_BELOW = 0.42, 0.35

# Idle: the calm stretch of Meshy's Idle (frames, wrapping past its 96), and the new loop's length.
IDLE_CALM, IDLE_FRAMES = (81, 102), 63

# Slimming the body's waist and belly (round 1: "he looks fat"): between WAIST_Z (fading in and out),
# the front beyond FRONT_SOFT and the back beyond BACK_SOFT are pressed in to KEEP of what stuck out.
WAIST_Z = (0.80, 0.90, 1.18, 1.30)
FRONT_SOFT, FRONT_KEEP, BACK_SOFT, BACK_KEEP = 0.138, 0.35, 0.098, 0.5

# Hoodie: how far the shell stands off the body, the hem, the colours.
HOODIE_COLOURS = ("paint_green", "sheet_cream")  # muted olive body; cream drawstrings
TORSO_EASE, ARM_EASE = 0.012, 0.016
HEM_Z, HEM_OUT, BAND_OUT, BAND_H = 0.86, 0.014, 0.01, 0.04
CUFF_LEN = 0.03
GLASSES_COLOUR = "plastic_black"
# Glasses, in the eyes socket's frame (metres): pupils at ±EYE_X; rims W × H outside, bars BAR wide
# and DEPTH deep; the front STAND_OFF in front of the socket (the nose bridge), its outer ends WRAP
# further back; temples TEMPLE thick, at most TEMPLE_END behind the socket.
GLASSES_EYE_X, GLASSES_RIM_W, GLASSES_RIM_H = 0.046, 0.066, 0.046
GLASSES_BAR, GLASSES_DEPTH, GLASSES_TEMPLE = 0.005, 0.005, 0.004
GLASSES_STAND_OFF, GLASSES_WRAP, GLASSES_TEMPLE_END = 0.013, 0.006, 0.15
GLASSES_END, GLASSES_TEMPLE_TUCK = 0.006, 0.003  # end pieces past the rims; temples into the hair

# T-shirt: a faded red over the base layer's grey; sleeves to SLEEVE of the way down the upper arm.
TSHIRT_COLOURS = ("cotton_red",)
TSHIRT_EASE, TSHIRT_SLEEVE_EASE, TSHIRT_SLEEVE = 0.007, 0.010, 0.62
TSHIRT_HEM_DROP = 0.03  # the hem hangs a little below where the base layer's shirt ends
# Below TSHIRT_LOOSE_Z the shirt stands off by TSHIRT_HEM_EASE: worn out, over the jeans' waistband.
TSHIRT_LOOSE_Z, TSHIRT_LOOSE_SPAN, TSHIRT_HEM_EASE = 1.08, 0.10, 0.024
# Jeans: denim, a darker waistband, hems that fall over the top of the shoe, back pockets.
JEANS_COLOURS = ("denim", "denim_dark")
JEANS_EASE, JEANS_HEM_DROP, JEANS_HEM_OUT = 0.009, 0.025, 0.007
# Snug at the waist like a real waistband: over the top JEANS_SNUG metres the ease goes down to
# JEANS_TOP_EASE, so a shirt worn out over the jeans clears them in every pose (Gate 3 round 1:
# "i can see the trousers poking through the bottom of his shirt").
JEANS_SNUG, JEANS_TOP_EASE = 0.10, 0.003
# The jeans above JEANS_HIPS_Z are tagged Torso (their `_region`): under a shirt or hoodie worn out over
# them the game cuts them away above that garment's hem (its `hem_z`), so they can't show through it
# whatever the pose (Gate 3 round 1).
JEANS_HIPS_Z = 0.70
# Trainers: off-white uppers, grey soles up to SOLE_Z.
TRAINER_COLOURS = ("trainer_white", "plastic_grey")
TRAINER_EASE, TRAINER_SOLE_Z = 0.008, 0.028
# Batch 2. Jumper: navy wool, long sleeves with cuffs, a ribbed hem a little below the waist.
JUMPER_COLOURS = ("fabric_navy",)
JUMPER_EASE, JUMPER_SLEEVE_EASE, JUMPER_HEM_Z = 0.012, 0.015, 0.90
# Leather jacket: the outer layer, roomy enough to close over a hoodie or jumper; collar, zip, cuffs.
JACKET_COLOURS = ("leather_brown", "steel")
JACKET_EASE, JACKET_SLEEVE_EASE, JACKET_HEM_Z = 0.026, 0.028, 0.93
# Work boots: ankle-high over the shin up to BOOT_TOP, the shaft roomy enough to take jeans tucked in,
# thick dark soles up to BOOT_SOLE_Z, a lace strip down the front.
BOOT_COLOURS = ("boot_brown", "rubber")
BOOT_EASE, BOOT_SHAFT_EASE, BOOT_TOP, BOOT_SOLE_Z = 0.012, 0.024, 0.17, 0.035
# Work gloves: a snug shell over the hands, a cuff over the wrist.
GLOVE_COLOURS = ("glove_tan",)
GLOVE_EASE, GLOVE_CUFF = 0.005, 0.04


def build(c):
    c.extras = {
        "ai_generated": True,
        "generator": "Meshy (Pro): nano-banana-pro image, meshy-t2.5 image-to-3D, auto-rig, animation, text-to-motion; "
                     "cleaned in Blender; garments built in Blender over the body",
        "meshy_tasks": "image 01a16ce1-c617-761c-b4f2-2a283e4646f1, model 01a16ce2-f3ae-74b0-ac00-467749581ab6, "
                       "rig 01a16ce5-1792-7245-b97e-9ca41b7aa2dd, animation 01a16ce5-b344-7075-9734-5fd40440002e, "
                       "directional animation 01a174b0-b8b3-7104-8408-77c8982ab53a, "
                       "text-to-motion 01a174b0-8d1f-7552-bdf8-8f82ecac98c4 (on the rig 01a174b0-d41a-73a5-8ec4-46a7c55e0e7c)",
        "author": "Dead Commons",
    }
    spec = c.spec
    arm, mesh = c.import_glb(c.raw)
    actions = dict(ACTIONS)
    for file, name in EXTRA_CLIPS:
        g.merge_clip(arm, c.paths["raw"] / file, name)
        actions[name] = name
    directional = list(g.merge_clips(arm, c.paths["raw"] / DIRECTIONAL[0], DIRECTIONAL[1]))
    g.merge_clip(arm, c.paths["raw"] / WALK_LEFT[0], WALK_LEFT[1])
    g.mirror_clip(bpy.data.actions["walk_left"], "walk_right")
    # Meshy's back-right run strides much shorter than its back-left (1.9 m a cycle against 2.7 m: it
    # would patter at the game's speed): the back-left one mirrored instead, so the two match.
    bpy.data.actions.remove(bpy.data.actions["run_back_right"])
    g.mirror_clip(bpy.data.actions["run_back_left"], "run_back_right")
    directional += ["walk_left", "walk_right"]
    for name in directional:
        actions[name] = name
    c.to_asset_collection(arm, mesh)

    # Centimetres under a 0.01 armature scale -> metres; the body's centre (hips) onto the origin.
    hx, hy = c.hips_xy(arm)
    before = c.evaluated_points(mesh)
    scale = c.apply_transforms(arm, mesh, offset=(-hx, -hy))
    after = c.evaluated_points(mesh)
    shift = Vector((-hx, -hy, 0.0))
    moved = max((a - (b + shift)).length for a, b in zip(after, before))
    assert moved < 1e-4, f"applying transforms moved the posed mesh by {moved:.5f} m"
    c.note(f"armature scale {scale} baked into the bones and keys; hips moved ({-hx:+.3f}, {-hy:+.3f}) m onto the origin")

    # Clips: in place, one cycle each.
    for file, name in EXTRA_CLIPS:
        metres, speed = g.strip_root_motion(arm, bpy.data.actions[name])
        c.note(f"{name} (from {file}): travels {metres * 100:.1f} cm over the clip"
               + (" (already in place)" if metres < 0.01 else f", taken out ({speed:.2f} m/s)"))
    # The directional clips: in place, one cycle each.
    # Meshy's library clips are one cycle each; the text-to-motion walk holds a few (cut to one).
    for name in directional:
        a = bpy.data.actions[name]
        metres, speed = g.strip_root_motion(arm, a)
        if name.startswith("walk_") and name != "walk_back":
            frames, err = g.one_cycle(arm, a)
        else:
            g.set_cyclic(a)
            frames, err = a.frame_range[1] - a.frame_range[0], g.pose_error(arm, a, *a.frame_range)
        c.note(f"{name}: " + ("in place" if metres < 0.01 else f"travel taken out ({speed:.2f} m/s)")
               + f"; one cycle of {frames:.0f} frames (closes within {err * 100:.1f} cm)")
    frames, err = g.one_cycle(arm, bpy.data.actions["Casual_Walk_inplace"])
    c.note(f"walk = Meshy's in-place casual walk, cut to one cycle of {frames} frames "
           f"(it holds three; closes within {err * 100:.1f} cm)")
    # Idle: Meshy's turns right round to look behind. Keep only its calm stretch (frames 81 to 102,
    # wrapping past the end: standing, breathing, a little sway, facing forward), played there and back
    # with the turns eased, at two-thirds speed: a seamless loop.
    a, b, frames = IDLE_CALM[0], IDLE_CALM[1], IDLE_FRAMES
    g.resample_clip(arm, bpy.data.actions["Idle"], lambda u: a + (b - a) * (1 - math.cos(2 * math.pi * u)) / 2, frames)
    c.note(f"idle = Meshy's Idle frames {a}-{b} (the calm stretch before it turns to look behind), there and back, eased: "
           f"{frames} frames ({frames / 24:.2f} s)")
    for meshy in ("Idle", "Cautious_Crouch_Walk_Forward_inplace", "run", "walk_alt"):
        a = bpy.data.actions[meshy]
        g.set_cyclic(a)
        c.note(f"{ACTIONS.get(meshy, meshy)}: the whole clip is one cycle ({a.frame_range[1] - a.frame_range[0]:.1f} frames, "
               f"closes within {g.pose_error(arm, a, *a.frame_range) * 100:.2f} cm)")

    merged = c.weld_seams(mesh)
    c.note(f"welded {merged} vertices split at UV seams")
    before, after = c.decimate_to(mesh, spec["tris"] - 10)
    c.note(f"decimated {before} -> {after} triangles")
    c.limit_weights(mesh)
    n = slim_waist(mesh)
    c.note(f"waist and belly slimmed (Gate 3 round 1: \"he looks fat\"): {n} vertices moved in, weights kept")

    # Toward the palette: a little less saturation and contrast, blacks lifted a touch. The reference
    # is already muted; the skin stays a natural tone.
    c.tone_texture(mesh, c.paths["textures"] / "survivor-01_base_color.jpg",
                   saturation=0.88, contrast=0.94, lift=0.02, tint=(1.0, 1.0, 1.0))
    c.note("base colour toned: saturation 0.88, contrast 0.94, lift 0.02")

    c.name_things(arm, mesh, spec["id"], actions)
    g.fix_slots(arm)
    g.shade(mesh, spec["shading"])
    c.note(f"shading: {shading_words(spec['shading'])}")

    dom = g.dominant_bones(mesh)

    def split(i, colour, region):
        lum = float(colour.mean())
        if region == "Legs" and dom[i].endswith("UpLeg") and lum > SHIRT_ABOVE:
            return "Torso"
        if region == "Torso" and dom[i] == "Hips" and lum < TROUSERS_BELOW:
            return "Legs"
        return region

    counts = g.region_groups(mesh, spec["regions"], split)
    c.note("regions (vertices): " + ", ".join(f"{r} {n}" for r, n in counts.items()))

    made = [(make_hoodie(c, mesh, arm), "hoodie"), (make_glasses(c, mesh, arm), "glasses"),
            (make_tshirt(c, mesh, arm), "t_shirt"), (make_jeans(c, mesh, arm), "jeans"), (make_trainers(c, mesh, arm), "trainers"),
            (make_jumper(c, mesh, arm), "jumper"), (make_jacket(c, mesh, arm), "leather_jacket"),
            (make_boots(c, mesh, arm), "work_boots"), (make_gloves(c, mesh, arm), "work_gloves")]
    for o, _ in made:
        o.data.update()

    garments = {gm["id"]: gm for gm in g.wearables(spec)}
    clips = list(actions.values())
    c.poke_through = {}
    for obj, gid in made:
        c.poke_through[gid] = g.poke_through(mesh, obj, arm, clips, set(garments[gid].get("hides") or []))
        c.note(f"{obj.name}: {zchar.tris(obj)} triangles; poke-through, worst frame per clip: "
               + g.describe_poke(c.poke_through[gid]))
    return mesh


def slim_waist(mesh):
    """Presses the belly and the small of the back in (rest pose; the weights stay): the torso's
    vertices between the hips and the chest that stick out beyond FRONT_SOFT in front or BACK_SOFT
    behind keep only part of the excess. Returns how many moved."""
    z0, z1, z2, z3 = WAIST_Z
    moved = 0
    dom = g.dominant_bones(mesh)
    torso = {"Hips", "Spine02", "Spine01", "Spine", "LeftUpLeg", "RightUpLeg"}
    for v in mesh.data.vertices:
        z = v.co.z
        if not (z0 < z < z3) or dom[v.index] not in torso or abs(v.co.x) > 0.24:
            continue
        w = min(1.0, (z - z0) / (z1 - z0), (z3 - z) / (z3 - z2))
        y = v.co.y
        if y < -FRONT_SOFT:
            target = -FRONT_SOFT - (-y - FRONT_SOFT) * FRONT_KEEP
        elif y > BACK_SOFT:
            target = BACK_SOFT + (y - BACK_SOFT) * BACK_KEEP
        else:
            continue
        v.co.y = y + w * (target - y)
        moved += 1
    mesh.data.update()
    return moved


def shading_words(shading):
    return "flat" if shading == "flat" else f"smooth, edges sharper than {float(shading):g}° hard"


# ---------------------------------------------------------------- hoodie

def _flip_piece_if(bm, faces, wrong):
    """Recalculates a piece's normals, then flips them all if `wrong(faces)` says they face in."""
    bmesh.ops.recalc_face_normals(bm, faces=faces)
    if wrong(faces):
        bmesh.ops.reverse_faces(bm, faces=faces)


def make_hoodie(c, body, arm):
    spec = c.spec
    gid = f"{spec['id']}_hoodie"
    budget = next(x for x in g.wearables(spec) if x["id"] == "hoodie")["tris"]
    body_tree = g.surface_tree(body)
    shell = g.shell_from_regions(body, {"Torso", "Arms"}, gid)
    bm = bmesh.new()
    bm.from_mesh(shell.data)
    part = bm.faces.layers.int.new(g.PART_ATTRIBUTE)  # 0: the cover over the body; details get 1
    bm.normal_update()
    for f in bm.faces:
        f.material_index = 0

    # Ease: a loose garment, not a second skin. Smooth the body's detail out first (not the edges).
    inner = [v for v in bm.verts if not v.is_boundary]
    bmesh.ops.smooth_vert(bm, verts=inner, factor=0.5, use_axis_x=True, use_axis_y=True, use_axis_z=True)
    bm.normal_update()
    for v in bm.verts:
        ease = ARM_EASE if abs(v.co.x) > 0.26 else TORSO_EASE
        v.co += v.normal * ease
    bm.normal_update()

    loops = g.boundary_loops(bm)
    centre = lambda loop: sum((v.co for v in loop), Vector()) / len(loop)  # noqa: E731
    cuffs = [l for l in loops if abs(centre(l).x) > 0.4]
    rest = [l for l in loops if abs(centre(l).x) <= 0.4]
    hem = min(rest, key=lambda l: centre(l).z)
    neck = max(rest, key=lambda l: centre(l).z)
    assert len(cuffs) == 2 and len(loops) == 4, f"hoodie shell: {len(loops)} open edges loops, expected neck, hem, two cuffs"
    axis = Vector((0.0, -0.03, 0.0))

    def out_from_body(z, d, off):
        """The body's surface at height z in direction d (xy) from the axis, `off` further out."""
        o = Vector((axis.x + d.x, axis.y + d.y, z))
        hit = g.on_surface(body_tree, o, -d)
        return (hit if hit is not None else Vector((axis.x, axis.y, z))) + d * off

    # Hem: down to HEM_Z, bloused a little, then a snug band, then a lip onto the body.
    for v in hem:
        d = Vector((v.co.x - axis.x, v.co.y - axis.y, 0.0)).normalized()
        v.co = out_from_body(HEM_Z + BAND_H, d, HEM_OUT)
    band = g.lip(bm, hem, lambda v: out_from_body(HEM_Z, Vector((v.co.x - axis.x, v.co.y - axis.y, 0.0)).normalized(), BAND_OUT))
    g.lip(bm, band, lambda v: g.nearest_on(body_tree, v.co, 0.004))

    # Cuffs: a short band past the sleeve's end, snug on the wrist, then a lip.
    for loop in cuffs:
        side = 1.0 if centre(loop).x > 0 else -1.0
        band = g.lip(bm, loop, lambda v: g.nearest_on(body_tree, v.co + Vector((side * CUFF_LEN, 0, 0)), 0.012))
        g.lip(bm, band, lambda v: g.nearest_on(body_tree, v.co, 0.004))

    # Neckline: onto the neck.
    g.lip(bm, neck, lambda v: g.nearest_on(body_tree, v.co, 0.004))
    shell_faces = list(bm.faces)
    _flip_piece_if(bm, shell_faces, lambda fs: sum(f.normal.dot(f.calc_center_median() - Vector((0, -0.03, f.calc_center_median().z)))
                                                   for f in fs if abs(f.calc_center_median().x) < 0.2) < 0)
    shell_tree = BVHTree.FromBMesh(bm)
    shell_set = set(shell_faces)
    nc = centre(neck)

    # Hood, down: a roll round the back of the neck, fattest at the back, open at the front...
    path, radii = [], []
    for k in range(11):
        th = math.radians(-130 + 26 * k)
        back = (1 + math.cos(th)) / 2
        reach = 0.10 + 0.02 * back
        x, y = nc.x + reach * math.sin(th), nc.y + reach * math.cos(th)
        hit = g.on_surface(shell_tree, (x, y, 2.0), (0, 0, -1))
        r = 0.024 + 0.028 * back
        z = max(hit.z if hit is not None else 1.42, 1.40) + r * 0.55
        path.append(Vector((x, y, z)))
        radii.append(r)
    roll = g.tube(bm, path, lambda i: radii[i], 5, 0)

    # ...and the rest of it lying flat on the upper back, a rounded flap.
    rows, cols = 4, 5
    grid = []
    for vi in range(rows):
        v = vi / (rows - 1)
        z = path[5].z - radii[5] * 0.6 - 0.24 * v
        half = 0.13 * (1 - 0.45 * v * v)
        row = []
        for ui in range(cols):
            u = -1 + 2 * ui / (cols - 1)
            x = nc.x + u * half
            s = g.on_surface(shell_tree, (x, 1.0, z), (0, -1, 0))
            s = s if s is not None else Vector((x, 0.13, z))
            t = 0.016 + 0.05 * (1 - u * u) * (1 - v * v)
            row.append(bm.verts.new(s + Vector((0, t, 0))))
        grid.append(row)
    flap = []
    for vi in range(rows - 1):
        for ui in range(cols - 1):
            flap.append(bm.faces.new((grid[vi][ui], grid[vi][ui + 1], grid[vi + 1][ui + 1], grid[vi + 1][ui])))
    # A step down to the back round its sides and bottom: a hard edge, so the flap reads from above.
    edge = [grid[vi][0] for vi in range(rows)] + grid[-1][1:] + [grid[vi][-1] for vi in range(rows - 2, -1, -1)]
    foot = [bm.verts.new(g.on_surface(shell_tree, (v.co.x, 1.0, v.co.z), (0, -1, 0), lift=0.002) or v.co.copy())
            for v in edge]
    for i in range(len(edge) - 1):
        flap.append(bm.faces.new((edge[i], foot[i], foot[i + 1], edge[i + 1])))
    _flip_piece_if(bm, flap, lambda fs: sum(f.normal.y for f in fs) < 0)

    # Kangaroo pocket on the belly: a raised panel with slanted hand openings at the sides.
    zb, zt = HEM_Z + BAND_H + 0.02, HEM_Z + BAND_H + 0.19
    outline = [(-0.16, zb), (0.16, zb), (0.16, zb + 0.05), (0.11, zt), (-0.11, zt), (-0.16, zb + 0.05)]
    inset = [(x * 0.9, z + (0.012 if z < (zb + zt) / 2 else -0.012)) for x, z in outline]
    rim = [bm.verts.new(g.on_surface(shell_tree, (x, -1.0, z), (0, 1, 0), lift=0.002)) for x, z in outline]
    top = [bm.verts.new(g.on_surface(shell_tree, (x, -1.0, z), (0, 1, 0), lift=0.009)) for x, z in inset]
    pocket = [bm.faces.new(top)]
    for i in range(len(rim)):
        j = (i + 1) % len(rim)
        pocket.append(bm.faces.new((rim[i], rim[j], top[j], top[i])))
    _flip_piece_if(bm, pocket, lambda fs: fs[0].normal.y > 0)

    # Drawstrings from the neckline, cream, with knotted ends.
    for side in (-1, 1):
        x = nc.x + side * 0.045
        pts = [g.on_surface(shell_tree, (x, -1.0, z), (0, 1, 0), lift=0.008) for z in (1.45, 1.38, 1.31, 1.25)]
        pts = [p for p in pts if p is not None]
        left = [bm.verts.new(p + Vector((-0.012, 0, 0))) for p in pts]
        right = [bm.verts.new(p + Vector((0.012, 0, 0))) for p in pts]
        ribbon = [bm.faces.new((left[i], right[i], right[i + 1], left[i + 1])) for i in range(len(pts) - 1)]
        end = pts[-1] + Vector((0, -0.004, -0.025))
        knot = bmesh.ops.create_cube(bm, size=1.0)["verts"]
        for v in knot:
            v.co = end + Vector((v.co.x * 0.03, v.co.y * 0.026, v.co.z * 0.045))
        knot_faces = list({f for v in knot for f in v.link_faces})
        _flip_piece_if(bm, knot_faces, lambda fs: sum(f.normal.dot(f.calc_center_median() - end) for f in fs) < 0)
        if sum(f.normal.y for f in ribbon) > 0:
            bmesh.ops.reverse_faces(bm, faces=ribbon)
        for f in ribbon + knot_faces:
            f.material_index = 1

    for f in bm.faces:
        if f not in shell_set:
            f[part] = 1
    bm.to_mesh(shell.data)
    bm.free()
    g.palette_material(shell, *HOODIE_COLOURS)
    g.finish_garment(shell, spec["shading"])
    g.transfer_weights(body, shell, arm)
    shell["ai_generated"] = True  # its cover is the Meshy body's faces, eased out and reshaped
    shell["generator"] = "Blender (Dead Commons): shell over the Meshy-generated body; hood, pocket, strings from primitives"
    shell["hem_z"] = HEM_Z  # the game cuts what's under it above this (the jeans' top)
    t = zchar.tris(shell)
    assert t <= budget, f"hoodie: {t} triangles over its budget {budget}"
    return shell


# ---------------------------------------------------------------- glasses

def _face_out_from_line(bm, verts, a, b, z=False):
    """Turns the faces of a bar (its `verts`) to face away from its centre line a-b."""
    faces = {f for v in verts for f in v.link_faces}
    bm.normal_update()
    zc = sum(v.co.z for v in verts) / max(len(verts), 1)
    d = (b - a).normalized()
    for f in faces:
        m = f.calc_center_median()
        p = a + d * d.dot(m - a)
        if z:
            p.z = zc
        if f.normal.dot(m - p) < 0:
            f.normal_flip()

def make_glasses(c, body, arm):
    """Thin dark frames standing off the face: a flat front (two open rims and a bridge, the outer
    edges a touch further back) about 1.3 cm in front of the nose bridge, short end pieces and straight
    temples back into the hair above the ears. Made in the `eyes` socket's frame
    (origin on the nose bridge between the pupils, -Y forward) and bound to its bone: a rigid wearable."""
    spec = c.spec
    sock = g.socket_matrix(arm, spec, "eyes")
    tree = g.surface_tree(body)
    w, h, bar, depth = GLASSES_RIM_W, GLASSES_RIM_H, GLASSES_BAR, GLASSES_DEPTH
    inner_x, outer_x = GLASSES_EYE_X - w / 2, GLASSES_EYE_X + w / 2
    front = -GLASSES_STAND_OFF

    def y_front(x):
        """The front's depth across: flat, the outer ends GLASSES_WRAP further back."""
        return front + GLASSES_WRAP * max(0.0, (abs(x) - inner_x) / (outer_x - inner_x))

    # Temples: from short end pieces just outside the rims straight back until they meet the side of
    # the head (into the hair by TEMPLE_TUCK), as real temples run under the hair to the ears.
    top = h / 2 - bar / 2
    temple_x = outer_x + GLASSES_END
    yo = y_front(outer_x) + depth / 2
    inv = sock.inverted()
    ends = {}
    for side in (-1, 1):
        start = Vector((side * temple_x, yo + 0.006, top))
        hit = g.on_surface(tree, sock @ start, sock.to_3x3() @ Vector((0, 1, 0)))
        ends[side] = min(GLASSES_TEMPLE_END, (inv @ hit).y + GLASSES_TEMPLE_TUCK) if hit is not None else GLASSES_TEMPLE_END

    bm = bmesh.new()
    part = bm.faces.layers.int.new(g.PART_ATTRIBUTE)

    def bar_between(a, b, thick, tall):
        """A square bar from a to b (socket frame), open at its ends."""
        a, b = Vector(a), Vector(b)
        d = (b - a).normalized()
        up = Vector((0, 0, 1)) if abs(d.z) < 0.9 else Vector((0, 1, 0))
        across = d.cross(up).normalized()
        up = across.cross(d).normalized()
        corners = [(-1, -1), (1, -1), (1, 1), (-1, 1)]
        ra = [bm.verts.new(a + across * cx * thick / 2 + up * cz * tall / 2) for cx, cz in corners]
        rb = [bm.verts.new(b + across * cx * thick / 2 + up * cz * tall / 2) for cx, cz in corners]
        faces = []
        for i in range(4):
            j = (i + 1) % 4
            f = bm.faces.new((ra[i], ra[j], rb[j], rb[i]))
            mid = f.calc_center_median()
            axis = a + d * d.dot(mid - a)
            if f.normal.dot(mid - axis) < 0:
                f.normal_flip()
            faces.append(f)
        return faces

    for side in (-1, 1):
        cx = side * GLASSES_EYE_X

        def ring(hw, hh, back):
            pts = [(cx - hw, -hh), (cx + hw, -hh), (cx + hw, hh), (cx - hw, hh)]
            return [bm.verts.new((x, y_front(x) + back, z)) for x, z in pts]

        of, inf = ring(w / 2, h / 2, 0.0), ring(w / 2 - bar, h / 2 - bar, 0.0)
        ob, inb = ring(w / 2, h / 2, depth), ring(w / 2 - bar, h / 2 - bar, depth)
        faces = []
        for i in range(4):
            j = (i + 1) % 4
            faces.append(bm.faces.new((of[i], of[j], inf[j], inf[i])))   # front
            faces.append(bm.faces.new((ob[i], ob[j], of[j], of[i])))     # outer side
            faces.append(bm.faces.new((inf[i], inf[j], inb[j], inb[i])))  # inner side
        for f in faces:
            mid = f.calc_center_median()
            want = Vector((0, -1, 0)) if abs(f.normal.y) > 0.9 else Vector((mid.x - cx, 0, mid.z))
            if f.normal.dot(want) < 0:
                f.normal_flip()
        # End piece out to the temple, then the temple straight back into the hair.
        hinge = (side * temple_x, yo + 0.006, top)
        bar_between((side * (outer_x - bar / 2), yo, top), hinge, GLASSES_TEMPLE, bar)
        bar_between(hinge, (side * temple_x, ends[side], top), GLASSES_TEMPLE, bar)
    bar_between((-inner_x - bar / 2, front + depth / 2, top), (inner_x + bar / 2, front + depth / 2, top), depth, bar)

    for v in bm.verts:
        v.co = sock @ v.co
    bm.normal_update()
    for f in bm.faces:
        f[part] = 1  # no cover: an accessory; only checked for sinking into the face
    obj = bpy.data.objects.new(f"{spec['id']}_glasses", bpy.data.meshes.new(f"{spec['id']}_glasses"))
    bm.to_mesh(obj.data)
    bm.free()
    import zasset
    zasset.collection(zasset.ASSET_COLLECTION).objects.link(obj)
    g.palette_material(obj, GLASSES_COLOUR)
    g.finish_garment(obj, spec["shading"])
    g.bind_to_socket(obj, arm, spec, "eyes")
    obj["ai_generated"] = False
    obj["generator"] = "Blender (Dead Commons), from primitives"
    c.note(f"glasses: front {GLASSES_STAND_OFF * 100:.1f} cm in front of the nose bridge, rims {w * 100:.1f} × {h * 100:.1f} cm, "
           f"bars {bar * 1000:.0f} mm, temples at ±{temple_x * 100:.1f} cm, "
           f"{(ends[1] - yo) * 100:.1f} cm long into the hair")
    budget = next(x for x in g.wearables(spec) if x["id"] == "glasses")["tris"]
    assert zchar.tris(obj) <= budget, f"glasses: {zchar.tris(obj)} triangles over {budget}"
    return obj


# ---------------------------------------------------------------- the everyday basics (#63, batch 1)

def _shell(c, body, gid, regions, smooth=0.5):
    """A garment's starting shell over `regions`, smoothed (not at its open edges); bmesh, its
    `garment_part` layer (0: the cover) and the object."""
    shell = g.shell_from_regions(body, regions, gid)
    bm = bmesh.new()
    bm.from_mesh(shell.data)
    part = bm.faces.layers.int.new(g.PART_ATTRIBUTE)
    for f in bm.faces:
        f.material_index = 0
    inner = [v for v in bm.verts if not v.is_boundary]
    bmesh.ops.smooth_vert(bm, verts=inner, factor=smooth, use_axis_x=True, use_axis_y=True, use_axis_z=True)
    bm.normal_update()
    return shell, bm, part


def _centre(loop):
    return sum((v.co for v in loop), Vector()) / len(loop)


def _level(loop, axis, value=None):
    """Cuts an opening straight: every vertex of `loop` to one value along `axis` (0 x, 2 z; the
    loop's mean by default), so a hem or cuff doesn't follow the body regions' ragged edge."""
    if value is None:
        value = sum(v.co[axis] for v in loop) / len(loop)
    for v in loop:
        v.co[axis] = value
    return value


def _hang(bm, loop, drop, out):
    """A hem hanging straight down `drop` from a level `loop`, `out` further from its centre."""
    c = _centre(loop)
    def at(v):
        d = Vector((v.co.x - c.x, v.co.y - c.y, 0.0))
        d = d.normalized() if d.length > 1e-6 else Vector()
        return v.co + d * out - Vector((0, 0, drop))
    return g.lip(bm, loop, at)


def _outward(bm, faces, centre_of):
    """Turns each connected piece of `faces` to face away from `centre_of(face)`."""
    bmesh.ops.recalc_face_normals(bm, faces=faces)
    if sum(f.normal.dot(f.calc_center_median() - centre_of(f)) for f in faces) < 0:
        bmesh.ops.reverse_faces(bm, faces=faces)


def _finish(c, shell, bm, part, colours, gid, generator):
    cover = set(bm.faces)
    for f in bm.faces:
        if f not in cover:
            f[part] = 1
    bm.to_mesh(shell.data)
    bm.free()
    g.palette_material(shell, *colours)
    g.finish_garment(shell, c.spec["shading"])
    g.transfer_weights(c.body, shell, c.arm)
    shell["ai_generated"] = True  # its cover is the Meshy body's faces, eased out and reshaped
    shell["generator"] = f"Blender (Dead Commons): shell over the Meshy-generated body; {generator}"
    budget = next(x for x in g.wearables(c.spec) if x["id"] == gid)["tris"]
    t = zchar.tris(shell)
    assert t <= budget, f"{gid}: {t} triangles over its budget {budget}"
    c.note(f"{gid}: {t} triangles (budget {budget})")
    return shell


def make_tshirt(c, body, arm):
    """A plain crew-neck t-shirt: the torso and the upper arms to TSHIRT_SLEEVE of the way to the elbow,
    eased off the body, a hem a little lower than the base layer's, the sleeves and neck closed onto the
    body with a narrow band."""
    c.body, c.arm = body, arm
    gid = f"{c.spec['id']}_t_shirt"
    bones = arm.data.bones
    shoulder, elbow = bones["LeftArm"].head_local.x, bones["LeftForeArm"].head_local.x
    sleeve_x = shoulder + TSHIRT_SLEEVE * (elbow - shoulder)
    shell, bm, part = _shell(c, body, gid, {"Torso", "Arms"})
    cut = [f for f in bm.faces if abs(f.calc_center_median().x) > sleeve_x]
    bmesh.ops.delete(bm, geom=cut, context="FACES")
    bmesh.ops.delete(bm, geom=[v for v in bm.verts if not v.link_faces], context="VERTS")
    bm.normal_update()
    for v in bm.verts:
        if abs(v.co.x) > shoulder:
            ease = TSHIRT_SLEEVE_EASE
        else:
            k = min(1.0, max(0.0, (TSHIRT_LOOSE_Z - v.co.z) / TSHIRT_LOOSE_SPAN))
            ease = TSHIRT_EASE + (TSHIRT_HEM_EASE - TSHIRT_EASE) * k
        v.co += v.normal * ease
    bm.normal_update()
    tree = g.surface_tree(body)
    loops = g.boundary_loops(bm)
    sleeves = [l for l in loops if abs(_centre(l).x) > shoulder]
    rest = [l for l in loops if abs(_centre(l).x) <= shoulder]
    assert len(sleeves) == 2 and len(rest) == 2, f"t-shirt shell: {len(loops)} open edge loops, expected neck, hem, two sleeves"
    hem, neck = min(rest, key=lambda l: _centre(l).z), max(rest, key=lambda l: _centre(l).z)
    _level(hem, 2, min(v.co.z for v in hem))
    _hang(bm, hem, TSHIRT_HEM_DROP, 0.004)
    for loop in sleeves:
        _level(loop, 0, math.copysign(sleeve_x, _centre(loop).x))
    hem_z = min(v.co.z for v in hem) - TSHIRT_HEM_DROP
    g.lip(bm, neck, lambda v: g.nearest_on(tree, v.co, 0.003))
    _outward(bm, list(bm.faces), lambda f: Vector((0, -0.03, f.calc_center_median().z)) if abs(f.calc_center_median().x) < shoulder
             else Vector((f.calc_center_median().x, 0, bones["LeftArm"].head_local.z)))
    shell = _finish(c, shell, bm, part, TSHIRT_COLOURS, "t_shirt", "plain crew-neck t-shirt")
    shell["hem_z"] = round(hem_z, 4)  # the game cuts what's under it above this (the jeans' top)
    return shell


def make_jeans(c, body, arm):
    """Straight jeans: the legs eased off the body, a darker waistband closed onto the waist, hems that
    fall over the top of the shoe and back pockets."""
    c.body, c.arm = body, arm
    gid = f"{c.spec['id']}_jeans"
    shell, bm, part = _shell(c, body, gid, {"Legs"})
    top_z = max(v.co.z for v in bm.verts)
    for v in bm.verts:
        k = min(1.0, max(0.0, (v.co.z - (top_z - JEANS_SNUG)) / JEANS_SNUG))
        v.co += v.normal * (JEANS_EASE + (JEANS_TOP_EASE - JEANS_EASE) * k)
    bm.normal_update()
    tree = g.surface_tree(body)
    loops = g.boundary_loops(bm)
    assert len(loops) == 3, f"jeans shell: {len(loops)} open edge loops, expected the waist and two ankles"
    waist = max(loops, key=lambda l: _centre(l).z)
    ankles = [l for l in loops if l is not waist]
    # Hems: down over the top of the shoe, a little flared, then in onto the foot.
    for loop in ankles:
        _level(loop, 2, max(v.co.z for v in loop))
        _hang(bm, loop, JEANS_HEM_DROP, JEANS_HEM_OUT)
    # The waistband: the top cut level and darker.
    _level(waist, 2, max(v.co.z for v in waist))
    # Snug already (JEANS_TOP_EASE off the body): no band to close the gap.
    cover = list(bm.faces)
    _outward(bm, cover, lambda f: Vector((math.copysign(0.09, f.calc_center_median().x) if f.calc_center_median().z < 0.8 else 0, 0.0, f.calc_center_median().z)))
    zt = max(v.co.z for v in waist)
    for f in cover:
        if f.calc_center_median().z > zt - 0.025:
            f.material_index = 1
    # Back pockets: raised patches on the seat (+Y is the back).
    shell_tree = BVHTree.FromBMesh(bm)
    pockets = []
    for side in (-1, 1):
        x0, x1 = side * 0.035, side * 0.135
        zb, zp = zt - 0.20, zt - 0.07
        outline = [(x0, zp), (x1, zp), (x1 * 0.97, zb + 0.02), ((x0 + x1) / 2, zb), (x0 * 1.1, zb + 0.02)]
        rim = [g.on_surface(shell_tree, (x, 1.0, z), (0, -1, 0), lift=0.001) for x, z in outline]
        top_ = [g.on_surface(shell_tree, (x, 1.0, z), (0, -1, 0), lift=0.005) for x, z in outline]
        if any(p is None for p in rim + top_):
            continue
        rv = [bm.verts.new(p) for p in rim]
        tv = [bm.verts.new(p) for p in top_]
        faces = [bm.faces.new(tv)]
        for i in range(len(rv)):
            j = (i + 1) % len(rv)
            faces.append(bm.faces.new((rv[i], rv[j], tv[j], tv[i])))
        _outward(bm, faces, lambda f: Vector((f.calc_center_median().x, 0.0, f.calc_center_median().z)))
        for f in faces:
            f.material_index = 1
            f[part] = 1
        pockets += faces
    shell = _finish(c, shell, bm, part, JEANS_COLOURS, "jeans", "waistband and back pockets from primitives")
    torso, legs = g.REGIONS.index("Torso"), g.REGIONS.index("Legs")
    attr = shell.data.attributes.new(g.REGION_ATTRIBUTE, "FLOAT", "POINT")
    attr.data.foreach_set("value", [float(torso if v.co.z > JEANS_HIPS_Z else legs) for v in shell.data.vertices])
    return shell


def make_trainers(c, body, arm):
    """Off-white trainers: the feet eased off the body, grey soles up to TRAINER_SOLE_Z, flat on the
    ground, the collar closed onto the ankle."""
    c.body, c.arm = body, arm
    gid = f"{c.spec['id']}_trainers"
    shell, bm, part = _shell(c, body, gid, {"Feet"}, smooth=0.3)
    for v in bm.verts:
        v.co += v.normal * TRAINER_EASE
        v.co.z = max(v.co.z, 0.0)
    bm.normal_update()
    tree = g.surface_tree(body)
    loops = g.boundary_loops(bm)
    assert len(loops) == 2, f"trainers shell: {len(loops)} open edge loops, expected two collars"
    for loop in loops:
        _level(loop, 2)
        g.lip(bm, loop, lambda v: g.nearest_on(tree, v.co, 0.003))
    _outward(bm, list(bm.faces), lambda f: Vector((math.copysign(0.09, f.calc_center_median().x), -0.06, 0.05)))
    for f in bm.faces:
        if f.calc_center_median().z < TRAINER_SOLE_Z:
            f.material_index = 1
    return _finish(c, shell, bm, part, TRAINER_COLOURS, "trainers", "soles by height")


# ---------------------------------------------------------------- batch 2 (#63): warmer and tougher things

def _top(c, body, arm, gid, ease, sleeve_ease, hem_z, colours, generator, details=None):
    """A long-sleeved top over the torso and arms (a jumper, a jacket): eased off the body, the hem
    cut level at `hem_z` with a band, cuffs at the wrists, the neck closed onto the body; `details`
    adds to the bmesh (part 1) before it's finished. Records its `hem_z`."""
    c.body, c.arm = body, arm
    shoulder = arm.data.bones["LeftArm"].head_local.x
    shell, bm, part = _shell(c, body, f"{c.spec['id']}_{gid}", {"Torso", "Arms"})
    for v in bm.verts:
        v.co += v.normal * (sleeve_ease if abs(v.co.x) > shoulder else ease)
    bm.normal_update()
    tree = g.surface_tree(body)
    loops = g.boundary_loops(bm)
    cuffs = [l for l in loops if abs(_centre(l).x) > shoulder]
    rest = [l for l in loops if abs(_centre(l).x) <= shoulder]
    assert len(cuffs) == 2 and len(rest) == 2, f"{gid} shell: {len(loops)} open edge loops, expected neck, hem, two cuffs"
    hem, neck = min(rest, key=lambda l: _centre(l).z), max(rest, key=lambda l: _centre(l).z)
    # The hem: down (or up) to hem_z, level, then a band in onto the body.
    for v in hem:
        v.co.z = min(v.co.z, hem_z + 0.03)
    _level(hem, 2, hem_z + 0.03)
    band = _hang(bm, hem, 0.03, -0.003)
    g.lip(bm, band, lambda v: g.nearest_on(tree, v.co, 0.004))
    for loop in cuffs:
        side = 1.0 if _centre(loop).x > 0 else -1.0
        _level(loop, 0)
        band = g.lip(bm, loop, lambda v: g.nearest_on(tree, v.co + Vector((side * 0.025, 0, 0)), 0.008))
        g.lip(bm, band, lambda v: g.nearest_on(tree, v.co, 0.003))
    g.lip(bm, neck, lambda v: g.nearest_on(tree, v.co, 0.004))
    _outward(bm, list(bm.faces), lambda f: Vector((0, -0.03, f.calc_center_median().z)) if abs(f.calc_center_median().x) < shoulder
             else Vector((f.calc_center_median().x, 0, arm.data.bones["LeftArm"].head_local.z)))
    if details:
        details(bm, part, BVHTree.FromBMesh(bm), _centre(neck))
    shell = _finish(c, shell, bm, part, colours, gid, generator)
    shell["hem_z"] = round(hem_z, 4)
    return shell


def make_jumper(c, body, arm):
    """A plain navy crew-neck jumper."""
    return _top(c, body, arm, "jumper", JUMPER_EASE, JUMPER_SLEEVE_EASE, JUMPER_HEM_Z, JUMPER_COLOURS, "plain crew-neck jumper")


def make_jacket(c, body, arm):
    """A worn brown leather jacket, zipped: roomy enough to close over a hoodie or jumper, a stand-up
    collar round the neck, a steel zip line down the front."""
    def details(bm, part, tree, nc):
        new = []
        # The collar: a band standing up round the back and sides of the neck.
        path = []
        for k in range(9):
            th = math.radians(-110 + 27.5 * k)
            x, y = nc.x + 0.085 * math.sin(th), nc.y + 0.075 * math.cos(th)
            hit = g.on_surface(tree, (x, y, 2.0), (0, 0, -1))
            path.append(Vector((x, y, (hit.z if hit is not None else nc.z) + 0.015)))
        new += g.tube(bm, path, lambda i: 0.016, 4, 0)
        # The zip: a thin steel line down the front, from the collar to the hem.
        top, bottom = nc.z - 0.02, JACKET_HEM_Z + 0.02
        pts = [g.on_surface(tree, (nc.x, -1.0, top - (top - bottom) * k / 5), (0, 1, 0), lift=0.003) for k in range(6)]
        pts = [p for p in pts if p is not None]
        left = [bm.verts.new(p + Vector((-0.005, 0, 0))) for p in pts]
        right = [bm.verts.new(p + Vector((0.005, 0, 0))) for p in pts]
        zip_faces = [bm.faces.new((left[i], right[i], right[i + 1], left[i + 1])) for i in range(len(pts) - 1)]
        if sum(f.normal.y for f in zip_faces) > 0:
            bmesh.ops.reverse_faces(bm, faces=zip_faces)
        for f in zip_faces:
            f.material_index = 1
        for f in new + zip_faces:
            f[part] = 1
    return _top(c, body, arm, "leather_jacket", JACKET_EASE, JACKET_SLEEVE_EASE, JACKET_HEM_Z, JACKET_COLOURS,
                "zipped leather jacket; collar and zip from primitives", details)


def make_boots(c, body, arm):
    """Ankle-high work boots: the feet and the shins up to BOOT_TOP, the shaft roomy (jeans tucked in),
    thick dark soles, a darker lace strip down the front of each."""
    c.body, c.arm = body, arm
    gid = f"{c.spec['id']}_work_boots"
    shell, bm, part = _shell(c, body, gid, {"Feet", "Legs"}, smooth=0.3)
    cut = [f for f in bm.faces if f.calc_center_median().z > BOOT_TOP]
    bmesh.ops.delete(bm, geom=cut, context="FACES")
    bmesh.ops.delete(bm, geom=[v for v in bm.verts if not v.link_faces], context="VERTS")
    bm.normal_update()
    for v in bm.verts:
        k = min(1.0, max(0.0, (v.co.z - 0.06) / 0.06))
        v.co += v.normal * (BOOT_EASE + (BOOT_SHAFT_EASE - BOOT_EASE) * k)
        v.co.z = max(v.co.z, 0.0)
    bm.normal_update()
    loops = g.boundary_loops(bm)
    assert len(loops) == 2, f"boots shell: {len(loops)} open edge loops, expected two tops"
    for loop in loops:
        _level(loop, 2, BOOT_TOP)
    _outward(bm, list(bm.faces), lambda f: Vector((math.copysign(0.09, f.calc_center_median().x), -0.04, 0.06)))
    for f in bm.faces:
        if f.calc_center_median().z < BOOT_SOLE_Z:
            f.material_index = 1
    return _finish(c, shell, bm, part, BOOT_COLOURS, "work_boots", "ankle boots, soles by height")


def make_gloves(c, body, arm):
    """Work gloves: a snug shell over the hands (fingers and all), a cuff over the wrist."""
    c.body, c.arm = body, arm
    gid = f"{c.spec['id']}_work_gloves"
    shell, bm, part = _shell(c, body, gid, {"Hands"}, smooth=0.2)
    for v in bm.verts:
        v.co += v.normal * GLOVE_EASE
    bm.normal_update()
    tree = g.surface_tree(body)
    loops = g.boundary_loops(bm)
    assert len(loops) == 2, f"gloves shell: {len(loops)} open edge loops, expected two wrists"
    for loop in loops:
        side = 1.0 if _centre(loop).x > 0 else -1.0
        _level(loop, 0)
        # The cuff: back over the wrist, a little loose.
        g.lip(bm, loop, lambda v: g.nearest_on(tree, v.co - Vector((side * GLOVE_CUFF, 0, 0)), 0.009))
    _outward(bm, list(bm.faces), lambda f: Vector((math.copysign(0.75, f.calc_center_median().x), 0.0, arm.data.bones["LeftHand"].head_local.z)))
    return _finish(c, shell, bm, part, GLOVE_COLOURS, "work_gloves", "gloves")
