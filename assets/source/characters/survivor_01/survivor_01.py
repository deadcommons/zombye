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
# The shove (2026-11-10, owner: "fix shove animation. looks wrong"): Meshy's "Step Forward and Push" (library 259,
# task 01a1c5bb-4f32-7520-aab1-32b4760520a6) instead of the grey-box poses: in place, the push landing at SHOVE_HIT
# of the clip (the game's blow lands at 0.45 of the action), eased from and back to the idle's first pose.
SHOVE_GLB, SHOVE_FRAMES, SHOVE_HIT, SHOVE_EASE = "shove.glb", 22, 0.45, (3, 6)
CLIMB_CLIPS = ["climb", "vault"]  # motion-climb.glb, motion-vault.glb: text-to-motion 01a1c558-393a-71bc-8986-6222bdaa02cb, 01a1c558-3bca-722c-be3c-24f2d632b987
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
# Accessories. Hats: a crown cast onto the head (hair and all) and eased off it, down to a rim that
# sits lower behind than in front (angles from the top of the head: front, sides, back), on the head_top
# socket. The cap has a brim; the beanie and the woolly hat a turned-up cuff, the woolly hat a bobble.
CAP_COLOURS, CAP_EASE, CAP_RIM = ("cap_red",), 0.014, (62.0, 82.0, 94.0)
CAP_BRIM = 0.075  # how far the brim reaches out in front
BEANIE_COLOURS, BEANIE_EASE, BEANIE_RIM = ("wool_charcoal",), 0.013, (66.0, 94.0, 104.0)
WOOLLY_COLOURS, WOOLLY_EASE, WOOLLY_RIM = ("wool_ochre",), 0.016, (70.0, 100.0, 110.0)
HAT_CUFF, BOBBLE = 0.03, 0.035
# Scarf: wound round the base of the neck (a fat tube), one end hanging down the chest.
SCARF_COLOURS, SCARF_RADIUS, SCARF_TAIL = ("wool_maroon",), 0.019, 0.26
# Bags (#91), from the body as built (T-pose, metres, facing -y; his left is +x): the back's surface is about
# 0.11 m behind the spine from the waist to the shoulders, the shoulder tops 1.45 m up, the hips 0.17 m out
# at 0.95 m. Packs: (width, height, depth, top z, bevel). Straps are webbing 6 cm wide, 1.2 cm off the body.
BACKPACK_COLOURS, BACKPACK = ("pack_blue", "plastic_black"), (0.30, 0.40, 0.14, 1.43, 0.04)
HIKING_COLOURS, HIKING = ("pack_rust", "plastic_black", "pack_rust_dark"), (0.32, 0.58, 0.20, 1.56, 0.05)
STRAP_W, STRAP_LIFT, STRAP_X = 0.055, 0.012, 0.095
# Tote: a canvas bag at the left hip, a little behind it, its two handles over the left shoulder.
TOTE_COLOURS, TOTE = ("canvas_natural", "canvas_natural_dark"), (0.34, 0.34, 0.06, 1.06)  # w, h, d, top z
# Hand bags hang from the right hand's grip (hand_r); in the T-pose that is out along -x.
PLASTIC_COLOURS, PLASTIC = ("plastic_white",), (0.28, 0.36, 0.11)  # width (y), length (down), thickness (z)
DUFFEL_COLOURS, DUFFEL = ("canvas_olive", "plastic_black"), (0.56, 0.135, 0.17)  # length (y), radius, handle drop
# Masks (#92), from the eyes socket (the nose bridge): the dust mask a cup over the nose and mouth with a
# strap round the head; the bandana a cloth from the nose bridge to a point below the chin, knotted behind;
# the balaclava cast over the head and neck like the hats, an opening for the eyes.
DUST_COLOURS, BANDANA_COLOURS, BALACLAVA_COLOURS = ("mask_white", "plastic_grey"), ("bandana_red",), ("wool_black",)
BALACLAVA_EASE, BALACLAVA_NECK = 0.012, 0.03  # metres off the head (the game draws a hat over it a little bigger); where it stops above the neck's base


def build(c):
    c.extras = {
        "ai_generated": True,
        "generator": "Meshy (Pro): nano-banana-pro image, meshy-t2.5 image-to-3D, auto-rig, animation, text-to-motion; "
                     "cleaned in Blender; garments built in Blender over the body",
        "meshy_tasks": "image 01a16ce1-c617-761c-b4f2-2a283e4646f1, model 01a16ce2-f3ae-74b0-ac00-467749581ab6, "
                       "rig 01a16ce5-1792-7245-b97e-9ca41b7aa2dd, animation 01a16ce5-b344-7075-9734-5fd40440002e, "
                       "directional animation 01a174b0-b8b3-7104-8408-77c8982ab53a, "
                       "text-to-motion 01a174b0-8d1f-7552-bdf8-8f82ecac98c4 (on the rig 01a174b0-d41a-73a5-8ec4-46a7c55e0e7c), "
                       "climb and vault 01a1c558-393a-71bc-8986-6222bdaa02cb, 01a1c558-3bca-722c-be3c-24f2d632b987 "
                       "(on the rig 01a1c558-c572-75e5-a06c-a014ffccef20, 01a1c558-c7fb-74ea-995d-0df6d30bc0a1)",
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
    # Climbing in through a window and over a fence (2026-11-10, text-to-motion on this rig): one-shots.
    for name in CLIMB_CLIPS:
        g.merge_clip(arm, c.paths["raw"] / f"motion-{name}.glb", name)
        actions[name] = name
    g.merge_clip(arm, c.paths["raw"] / SHOVE_GLB, "shove_src")
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
    # The climbs: the travel across the floor out (the game moves the body through the opening); the rise and
    # drop over the sill or rail stay. One-shots.
    for name in CLIMB_CLIPS:
        a = bpy.data.actions[name]
        metres, speed = g.strip_root_motion(arm, a)
        a.use_cyclic = False
        c.note(f"{name} (text-to-motion, raw/motion-{name}.glb): {metres * 100:.0f} cm of travel across the floor taken out; one-shot")
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

    make_shove(c, arm)
    actions["shove"] = "shove"

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
            (make_boots(c, mesh, arm), "work_boots"), (make_gloves(c, mesh, arm), "work_gloves"),
            (make_hat(c, mesh, arm, "baseball_cap"), "baseball_cap"), (make_hat(c, mesh, arm, "beanie"), "beanie"),
            (make_hat(c, mesh, arm, "woolly_hat"), "woolly_hat"), (make_scarf(c, mesh, arm), "scarf"),
            (make_pack(c, mesh, arm, "school_backpack"), "school_backpack"), (make_pack(c, mesh, arm, "hiking_pack"), "hiking_pack"),
            (make_tote(c, mesh, arm), "tote_bag"), (make_plastic_bag(c, mesh, arm), "plastic_bag"),
            (make_duffel(c, mesh, arm), "duffel_bag"),
            (make_face_mask(c, mesh, arm, "dust_mask"), "dust_mask"), (make_face_mask(c, mesh, arm, "bandana"), "bandana"),
            (make_balaclava(c, mesh, arm), "balaclava")]
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


# ---------------------------------------------------------------- accessories (#63)

def _new_object(c, gid):
    import zasset
    obj = bpy.data.objects.new(f"{c.spec['id']}_{gid}", bpy.data.meshes.new(f"{c.spec['id']}_{gid}"))
    zasset.collection(zasset.ASSET_COLLECTION).objects.link(obj)
    return obj


def make_hat(c, body, arm, gid):
    """A hat on the head_top socket: a crown cast onto the head and eased off it, down to a rim lower
    behind than in front; then the cap's brim, or the beanie's and woolly hat's cuff (and bobble)."""
    spec = c.spec
    colours, ease, (front, side, back) = {"baseball_cap": (CAP_COLOURS, CAP_EASE, CAP_RIM),
                                          "beanie": (BEANIE_COLOURS, BEANIE_EASE, BEANIE_RIM),
                                          "woolly_hat": (WOOLLY_COLOURS, WOOLLY_EASE, WOOLLY_RIM)}[gid]
    tree = g.surface_tree(body)
    crown = g.socket_matrix(arm, spec, "head_top").to_translation()
    centre = crown - Vector((0.0, -0.01, 0.105))
    rows, segs = 7, 16

    def rim(phi):
        cf = math.cos(phi)
        return math.radians(side + (front - side) * max(0.0, cf) + (back - side) * max(0.0, -cf))

    def on_head(theta, phi, extra=0.0):
        d = Vector((math.sin(theta) * math.sin(phi), -math.sin(theta) * math.cos(phi), math.cos(theta)))
        hit = g.on_surface(tree, centre + d * 0.4, -d)
        p = hit if hit is not None and (hit - centre).dot(d) > 0 else centre + d * 0.1
        return p + d * (ease + extra)

    bm = bmesh.new()
    part = bm.faces.layers.int.new(g.PART_ATTRIBUTE)
    top = bm.verts.new(on_head(0.0, 0.0))
    grid = [[bm.verts.new(on_head(rim(2 * math.pi * j / segs) * (i + 1) / rows, 2 * math.pi * j / segs)) for j in range(segs)] for i in range(rows)]
    faces = [bm.faces.new((top, grid[0][j], grid[0][(j + 1) % segs])) for j in range(segs)]
    for i in range(rows - 1):
        for j in range(segs):
            k = (j + 1) % segs
            faces.append(bm.faces.new((grid[i][j], grid[i + 1][j], grid[i + 1][k], grid[i][k])))
    edge = grid[-1]
    if gid == "baseball_cap":
        # The brim: out from the front of the rim (within 70° of straight ahead), tipped a little down.
        front_js = [j for j in range(segs) if math.cos(2 * math.pi * j / segs) > math.cos(math.radians(70))]
        front_js.sort(key=lambda j: ((2 * math.pi * j / segs + math.pi) % (2 * math.pi)))
        tips = {}
        for j in front_js:
            phi = 2 * math.pi * j / segs
            out = Vector((math.sin(phi) * 0.8, -math.cos(phi), -0.18)).normalized()
            reach = CAP_BRIM * (0.55 + 0.45 * math.cos(phi))
            tips[j] = bm.verts.new(edge[j].co + out * reach)
        for a, b in zip(front_js, front_js[1:]):
            faces.append(bm.faces.new((edge[a], tips[a], tips[b], edge[b])))
    else:
        # The cuff: a turned-up band hugging the crown above the rim, a few millimetres proud of it.
        lift = HAT_CUFF / 0.1  # radians of crown the band covers (the head is about 10 cm round)
        up = [bm.verts.new(on_head(rim(2 * math.pi * j / segs) - lift, 2 * math.pi * j / segs, 0.006)) for j in range(segs)]
        down = [bm.verts.new(v.co + (v.co - centre).normalized() * 0.006) for v in edge]
        for j in range(segs):
            k = (j + 1) % segs
            faces.append(bm.faces.new((down[j], down[k], up[k], up[j])))
            faces.append(bm.faces.new((edge[j], edge[k], down[k], down[j])))
        if gid == "woolly_hat":
            # The bobble: a little ball on the top.
            ball = bmesh.ops.create_icosphere(bm, subdivisions=1, radius=BOBBLE)["verts"]
            for v in ball:
                v.co += top.co + Vector((0, 0, BOBBLE * 0.7))
            faces += list({f for v in ball for f in v.link_faces})
    bmesh.ops.recalc_face_normals(bm, faces=faces)
    for f in faces:
        if f.normal.dot(f.calc_center_median() - centre) < 0:
            f.normal_flip()
    for f in bm.faces:
        f[part] = 1  # an accessory: only checked for sinking into the head
    obj = _new_object(c, gid)
    bm.to_mesh(obj.data)
    bm.free()
    g.palette_material(obj, *colours)
    g.finish_garment(obj, spec["shading"])
    g.bind_to_socket(obj, arm, spec, "head_top")
    obj["ai_generated"] = False
    obj["generator"] = "Blender (Dead Commons), from primitives cast onto the head"
    budget = next(x for x in g.wearables(spec) if x["id"] == gid)["tris"]
    t = zchar.tris(obj)
    assert t <= budget, f"{gid}: {t} triangles over {budget}"
    c.note(f"{gid}: {t} triangles (budget {budget})")
    return obj


def make_scarf(c, body, arm):
    """A scarf wound round the base of the neck (a fat tube following the neck's surface), one end
    hanging down the chest; skinned, so it follows the neck and chest."""
    spec = c.spec
    c.body, c.arm = body, arm
    tree = g.surface_tree(body)
    neck = g.socket_matrix(arm, spec, "neck").to_translation()
    centre = neck + Vector((0.0, 0.0, 0.03))
    path, n = [], 12
    for k in range(n + 1):
        phi = 2 * math.pi * k / n
        d = Vector((math.sin(phi), -math.cos(phi), 0.0))
        hit = g.on_surface(tree, centre + d * 0.3, -d)
        p = hit if hit is not None else centre + d * 0.07
        path.append(p + d * (SCARF_RADIUS * 0.9))
    bm = bmesh.new()
    part = bm.faces.layers.int.new(g.PART_ATTRIBUTE)
    faces = g.tube(bm, path, lambda i: SCARF_RADIUS, 6, 0)
    # The tail: from the front of the loop down the chest, a flat band on the surface.
    start = path[0]
    rows = []
    for k in range(6):
        z = start.z - 0.02 - SCARF_TAIL * k / 5
        x = start.x + 0.03 + 0.01 * k
        p = g.on_surface(tree, (x, -1.0, z), (0, 1, 0), lift=0.014 + 0.003 * k)
        if p is None:
            break
        rows.append((bm.verts.new(p + Vector((-0.035, 0, 0))), bm.verts.new(p + Vector((0.035, 0, 0)))))
    tail = [bm.faces.new((rows[i][0], rows[i][1], rows[i + 1][1], rows[i + 1][0])) for i in range(len(rows) - 1)]
    if sum(f.normal.y for f in tail) > 0:
        bmesh.ops.reverse_faces(bm, faces=tail)
    for f in bm.faces:
        f[part] = 1
    obj = _new_object(c, "scarf")
    bm.to_mesh(obj.data)
    bm.free()
    g.palette_material(obj, *SCARF_COLOURS)
    g.finish_garment(obj, spec["shading"])
    g.transfer_weights(body, obj, arm)
    obj["ai_generated"] = False
    obj["generator"] = "Blender (Dead Commons), from primitives on the body"
    budget = next(x for x in g.wearables(spec) if x["id"] == "scarf")["tris"]
    t = zchar.tris(obj)
    assert t <= budget, f"scarf: {t} triangles over {budget}"
    c.note(f"scarf: {t} triangles (budget {budget})")
    return obj


# ---------------------------------------------------------------- bags (#91)

def _linspace(a, b, n):
    return [a + (b - a) * k / (n - 1) for k in range(n)]


def _rbox(bm, lo, hi, bevel, mat, rigid=None):
    """A box from `lo` to `hi` with its edges chamfered by `bevel`; its faces get material `mat`, its
    vertices `rigid` = 1 (they go on one bone). Returns the faces."""
    before = set(bm.faces)
    verts = bmesh.ops.create_cube(bm, size=1.0)["verts"]
    for v in verts:
        v.co = Vector(tuple(l + (x + 0.5) * (h - l) for l, h, x in zip(lo, hi, v.co)))
    if bevel > 0:
        edges = list({e for v in verts for e in v.link_edges})
        bmesh.ops.bevel(bm, geom=edges, offset=bevel, segments=1, affect="EDGES", profile=0.5)
    faces = [f for f in bm.faces if f not in before]
    for f in faces:
        f.material_index = mat
        if rigid is not None:
            for v in f.verts:
                v[rigid] = 1
    return faces


def _band(bm, pts, side, width, mat):
    """A flat strip along `pts`, `width` across in the direction `side(point)`. Returns its faces."""
    rows = [(bm.verts.new(p - side(p) * width / 2), bm.verts.new(p + side(p) * width / 2)) for p in pts]
    faces = []
    for a, b in zip(rows, rows[1:]):
        f = bm.faces.new((a[0], a[1], b[1], b[0]))
        f.material_index = mat
        faces.append(f)
    return faces


def _over_shoulder(tree, x, z_front, z_back, lift=STRAP_LIFT, arc=7):
    """Points up the front of the chest at `x` from `z_front`, over the top of the shoulder and down the
    back to `z_back`, `lift` off the body."""
    pts = [g.on_surface(tree, (x, -1.0, z), (0, 1, 0), lift) for z in _linspace(z_front, 1.34, 4)]
    c = Vector((x, 0.0, 1.36))
    for k in range(1, arc):
        th = -math.pi / 2 + math.pi * k / arc
        d = Vector((0.0, math.sin(th), math.cos(th)))
        pts.append(g.on_surface(tree, c + d * 0.6, -d, lift))
    pts += [g.on_surface(tree, (x, 1.0, z), (0, -1, 0), lift) for z in _linspace(1.34, z_back, 3)]
    return [p for p in pts if p is not None]


def _round_side(tree, x_sign, z0, z1, phi1, lift=STRAP_LIFT, n=6):
    """Points round the side of the torso from the front (at height `z0`) to `phi1` degrees round toward
    the back (at `z1`), on the side `x_sign`."""
    pts = []
    for k in range(n):
        phi = math.radians(phi1) * k / (n - 1)
        z = z0 + (z1 - z0) * k / (n - 1)
        d = Vector((x_sign * math.sin(phi), -math.cos(phi), 0.0))
        c = Vector((0.0, -0.02, z))
        pts.append(g.on_surface(tree, c + d * 0.6, -d, lift))
    return [p for p in pts if p is not None]


def _face_out(bm, faces, tree):
    """Turns each face to face away from the body (by the nearest surface's normal)."""
    for f in faces:
        loc, normal, _, _ = tree.find_nearest(f.calc_center_median())
        if f.normal.dot(normal) < 0:
            f.normal_flip()


def _bag_object(c, gid, bm, colours, body, arm, rigid_bone=None, rigid=None, socket=None):
    """The bag `gid` from `bm`: palette materials, UVs and shading, then weights: on `socket`'s bone
    (rigid), or the body's weights with the `rigid` vertices moved wholly onto `rigid_bone`."""
    spec = c.spec
    for f in bm.faces:
        f[bm.faces.layers.int.get(g.PART_ATTRIBUTE)] = 1  # a bag only sits on the body: checked for sinking
    obj = _new_object(c, gid)
    bm.to_mesh(obj.data)
    bm.free()
    g.palette_material(obj, *colours)
    g.finish_garment(obj, spec["shading"])
    if socket:
        g.bind_to_socket(obj, arm, spec, socket)
    else:
        g.transfer_weights(body, obj, arm)
        if rigid_bone:
            marks = obj.data.attributes["rigid"].data
            ids = [i for i in range(len(obj.data.vertices)) if marks[i].value]
            for grp in obj.vertex_groups:
                grp.remove(ids)
            grp = obj.vertex_groups.get(rigid_bone) or obj.vertex_groups.new(name=rigid_bone)
            grp.add(ids, 1.0, "REPLACE")
            for grp in [x for x in obj.vertex_groups if not any(x.index == y.group for v in obj.data.vertices for y in v.groups)]:
                obj.vertex_groups.remove(grp)
    if "rigid" in obj.data.attributes:
        obj.data.attributes.remove(obj.data.attributes["rigid"])
    obj["ai_generated"] = False
    obj["generator"] = "Blender (Dead Commons), from primitives on the body"
    budget = next(x for x in g.wearables(spec) if x["id"] == gid)["tris"]
    t = zchar.tris(obj)
    assert t <= budget, f"{gid}: {t} triangles over {budget}"
    c.note(f"{gid}: {t} triangles (budget {budget})")
    return obj


def _new_bm():
    bm = bmesh.new()
    bm.faces.layers.int.new(g.PART_ATTRIBUTE)
    return bm, bm.verts.layers.int.new("rigid")


def make_pack(c, body, arm, gid):
    """A pack on the back: a chamfered box clear of the shoulder blades on the upper spine, a front
    pocket, and two shoulder straps over the shoulders and back round under the arms to its lower
    corners (the hiking pack also a lid and a hip belt round the front)."""
    tree = g.surface_tree(body)
    hiking = gid == "hiking_pack"
    colours, (w, h, d, top, bevel) = (HIKING_COLOURS, HIKING) if hiking else (BACKPACK_COLOURS, BACKPACK)
    bottom = top - h
    back = max(g.on_surface(tree, (0.0, 1.0, z), (0, -1, 0)).y for z in _linspace(max(bottom, 0.9), min(top, 1.42), 6))
    y0 = back + 0.012
    bm, rigid = _new_bm()
    _rbox(bm, (-w / 2, y0, bottom), (w / 2, y0 + d, top), bevel, 0, rigid)
    pocket_h = h * (0.32 if hiking else 0.42)
    _rbox(bm, (-w * 0.36, y0 + d - 0.01, bottom + 0.04), (w * 0.36, y0 + d + 0.05, bottom + 0.04 + pocket_h), 0.02,
          2 if hiking else 0, rigid)
    if hiking:
        # The lid: a flatter box over the top, a little proud of the pack.
        _rbox(bm, (-w / 2 - 0.01, y0 - 0.005, top - 0.08), (w / 2 + 0.01, y0 + d + 0.02, top + 0.02), 0.025, 2, rigid)
    straps = []
    for s in (-1, 1):
        x = s * STRAP_X
        over = _over_shoulder(tree, x, 1.16, 1.36)
        straps += _band(bm, over, lambda p: Vector((1.0, 0.0, 0.0)), STRAP_W, 1)
        under = _round_side(tree, s, 1.16, bottom + 0.1 if not hiking else 1.04, 150, n=7)
        straps += _band(bm, under, lambda p: Vector((0.0, 0.0, 1.0)), STRAP_W * 0.7, 1)
    if hiking:
        # The hip belt: from the pack's lower corners round the front of the waist, padded.
        belt = []
        for s in (-1, 1):
            belt.append(_round_side(tree, s, 1.0, 1.0, 120, lift=0.016, n=6))
        path = list(reversed(belt[0])) + belt[1][1:]
        straps += _band(bm, path, lambda p: Vector((0.0, 0.0, 1.0)), 0.07, 1)
    _face_out(bm, straps, tree)
    bm_faces = [f for f in bm.faces if f not in straps]
    centre = Vector((0.0, y0 + d / 2, (top + bottom) / 2))
    for f in bm_faces:
        if f.normal.dot(f.calc_center_median() - centre) < 0:
            f.normal_flip()
    return _bag_object(c, gid, bm, colours, body, arm, rigid_bone="Spine01", rigid=rigid)


def make_tote(c, body, arm):
    """A canvas tote at the left hip, a little behind it, hanging from two handles over the left shoulder."""
    tree = g.surface_tree(body)
    w, h, d, top = TOTE
    side = max(g.on_surface(tree, (1.0, 0.04, z), (-1, 0, 0)).x for z in _linspace(top - h, top, 5))
    x0 = side + 0.012
    y_mid = 0.05
    bm, rigid = _new_bm()
    box = _rbox(bm, (x0, y_mid - w / 2, top - h), (x0 + d, y_mid + w / 2, top), 0.012, 0, rigid)
    centre = Vector((x0 + d / 2, y_mid, top - h / 2))
    for f in box:
        if f.normal.dot(f.calc_center_median() - centre) < 0:
            f.normal_flip()
    # Each handle: from the bag's top (front or back) up to the chest or back at 1.18 m, over the
    # shoulder at x 0.14, and back down the other side to the other end of the bag's top.
    handles = []
    for dy in (-0.07, 0.07):
        over = _over_shoulder(tree, 0.14 + dy * 0.2, 1.2, 1.2, arc=6)
        a = Vector((x0 + d / 2, y_mid - w / 2 + 0.06, top - 0.005))
        b = Vector((x0 + d / 2, y_mid + w / 2 - 0.06, top - 0.005))
        path = [a, a.lerp(over[0], 0.5) + Vector((0.01, 0, 0))] + over + [over[-1].lerp(b, 0.5) + Vector((0.01, 0, 0)), b]
        handles += _band(bm, path, lambda p: Vector((1.0, 0.0, 0.0)), 0.03, 1)
    _face_out(bm, handles, tree)
    return _bag_object(c, "tote_bag", bm, TOTE_COLOURS, body, arm, rigid_bone="Hips", rigid=rigid)


def _hand_frame(arm, spec):
    """The right hand's grip in the T-pose, and the way a bag hangs from it there (out along the arm)."""
    grip = g.socket_matrix(arm, spec, "hand_r").to_translation()
    return grip, Vector((-1.0, 0.0, 0.0))


def make_plastic_bag(c, body, arm):
    """A carrier bag hanging from the right hand: two thin handles from the grip to a bag that widens
    and bulges toward its bottom (rigid on hand_r)."""
    grip, down = _hand_frame(arm, c.spec)
    w, length, t = PLASTIC
    bm, _ = _new_bm()
    top = grip + down * 0.07
    rings = []
    for k, (u, wf, tf) in enumerate(((0.0, 0.62, 0.35), (0.3, 0.92, 0.85), (0.75, 1.0, 1.0), (1.0, 0.86, 0.7))):
        p = top + down * (length * u)
        hw, ht = w * wf / 2, t * tf / 2
        rings.append([bm.verts.new(p + Vector((0, y, z))) for y, z in ((-hw, -ht), (hw, -ht), (hw, ht), (-hw, ht))])
    faces = []
    for a, b in zip(rings, rings[1:]):
        for j in range(4):
            k = (j + 1) % 4
            faces.append(bm.faces.new((a[j], a[k], b[k], b[j])))
    faces.append(bm.faces.new(list(rings[-1])))
    faces.append(bm.faces.new(list(reversed(rings[0]))))
    centre = top + down * (length / 2)
    for f in faces:
        if f.normal.dot(f.calc_center_median() - centre) < 0:
            f.normal_flip()
    # The handles: from the top corners up to the grip, each a thin strip (both sides drawn).
    for s in (-1, 1):
        a = top + Vector((0, s * w * 0.31 * 0.8, 0))
        path = [a, a.lerp(grip, 0.5) + Vector((0, s * 0.015, 0)), grip + Vector((0, s * 0.01, 0))]
        g.tube(bm, path, lambda i: 0.006, 4, 0, cap=False)
    return _bag_object(c, "plastic_bag", bm, PLASTIC_COLOURS, body, arm, socket="hand_r")


def make_duffel(c, body, arm):
    """A duffel bag hanging from the right hand: a long canvas tube along the way he walks, darker ends,
    and two webbing handles up to the grip (rigid on hand_r)."""
    grip, down = _hand_frame(arm, c.spec)
    length, r, drop = DUFFEL
    bm, _ = _new_bm()
    axis_c = grip + down * (drop + r)
    path = [axis_c + Vector((0, y, 0)) for y in _linspace(-length / 2, length / 2, 4)]
    faces = g.tube(bm, path, lambda i: r * (0.86 if i in (0, len(path) - 1) else 1.0), 8, 0, up=Vector((1.0, 0.0, 0.0)))
    for f in faces[-2:]:
        f.material_index = 1  # the round ends
    for s in (-1, 1):
        a = axis_c - down * r + Vector((0, s * 0.11, 0))
        b = grip + Vector((0, s * 0.012, 0))
        g.tube(bm, [a, a.lerp(b, 0.5), b], lambda i: 0.009, 4, 1, cap=False)
    return _bag_object(c, "duffel_bag", bm, DUFFEL_COLOURS, body, arm, socket="hand_r")


# ---------------------------------------------------------------- masks (#92)

def _strap_round_head(bm, tree, centre, z, ends, lift, radius, mat):
    """A thin strap round the back of the head at height `z`, from the mask's edge at `ends[0]` round
    to `ends[1]`."""
    a0 = math.atan2(ends[0].x - centre.x, -(ends[0].y - centre.y))
    a1 = math.atan2(ends[1].x - centre.x, -(ends[1].y - centre.y))
    if a1 < a0:
        a1 += 2 * math.pi
    pts = []
    for k in range(9):
        a = a0 + (a1 - a0) * k / 8
        d = Vector((math.sin(a), -math.cos(a), 0.0))
        p = g.on_surface(tree, Vector((centre.x, centre.y, z)) + d * 0.3, -d, lift)
        if p is not None:
            pts.append(p)
    return g.tube(bm, [ends[0]] + pts[1:-1] + [ends[1]], lambda i: radius, 4, mat, cap=False)


def make_face_mask(c, body, arm, gid):
    """A dust mask (a cup over the nose and mouth, a strap round the head) or a bandana (a cloth from
    the nose bridge to a point below the chin, knotted behind), rigid on the head (the eyes socket)."""
    spec = c.spec
    tree = g.surface_tree(body)
    eyes = g.socket_matrix(arm, spec, "eyes").to_translation()
    dust = gid == "dust_mask"
    half, top, bottom, cols, rows = (0.056, 0.012, 0.105, 6, 5) if dust else (0.07, 0.012, 0.15, 7, 6)
    bm, _ = _new_bm()
    grid = []
    chin = None
    for i in range(rows):
        z = eyes.z - top - (bottom - top) * i / (rows - 1)
        row = []
        for j in range(cols):
            u = -1 + 2 * j / (cols - 1)
            x = eyes.x + u * half
            lift = (0.008 + 0.016 * max(0.0, 1 - u * u) * math.sin(math.pi * (0.25 + 0.75 * i / (rows - 1)))) if dust else 0.014
            p = g.on_surface(tree, (x, eyes.y - 0.5, z), (0, 1, 0), lift)
            if p is None or p.y > eyes.y + 0.09:  # below the jaw the ray reaches the neck: the cloth hangs
                prev = grid[-1][j] if grid else None
                p = (prev.co if prev else Vector((x, eyes.y + 0.02, z))) - Vector((0, 0, (bottom - top) / (rows - 1)))
                if not dust:
                    p.x *= 0.85
            row.append(bm.verts.new(p))
        grid.append(row)
    if not dust:
        # Cloth, not skin: it tents from the most forward points (the nose, the chin) across and down
        # rather than following every hollow of the face.
        for row in grid:
            ys = [v.co.y for v in row]
            for j, v in enumerate(row):
                v.co.y = min(ys[k] + 0.7 * abs(row[j].co.x - row[k].co.x) for k in range(cols))
        for j in range(cols):
            for i in range(1, rows):
                above = grid[i - 1][j].co
                grid[i][j].co.y = min(grid[i][j].co.y, above.y + 0.3 * abs(grid[i][j].co.z - above.z))
            for i in range(rows - 2, -1, -1):  # taut up from the nose tip to the bridge
                below = grid[i + 1][j].co
                grid[i][j].co.y = min(grid[i][j].co.y, below.y + 0.5 * abs(grid[i][j].co.z - below.z))
    faces = []
    for i in range(rows - 1):
        for j in range(cols - 1):
            f = bm.faces.new((grid[i][j], grid[i][j + 1], grid[i + 1][j + 1], grid[i + 1][j]))
            faces.append(f)
    if not dust:
        # The point: the bottom edge drawn down to a tip below the middle of the chin.
        tip = bm.verts.new(grid[-1][cols // 2].co + Vector((0, -0.01, -0.07)))
        for j in range(cols - 1):
            faces.append(bm.faces.new((grid[-1][j], grid[-1][j + 1], tip)))
    for f in faces:
        if f.normal.y > 0:
            f.normal_flip()
    centre = Vector((eyes.x, eyes.y + 0.09, eyes.z))
    z_strap = eyes.z - (0.06 if dust else top + 0.01)
    ends = (min((grid[r][0].co for r in range(rows)), key=lambda p: abs(p.z - z_strap)),
            min((grid[r][-1].co for r in range(rows)), key=lambda p: abs(p.z - z_strap)))
    _strap_round_head(bm, tree, centre, z_strap, (ends[1], ends[0]), 0.006, 0.004 if dust else 0.008, 1 if dust else 0)
    if not dust:
        # The knot at the back of the head: a small lump and two short ends hanging.
        back = g.on_surface(tree, Vector((centre.x, centre.y + 0.4, z_strap)), (0, -1, 0), 0.015)
        if back is not None:
            knot = bmesh.ops.create_icosphere(bm, subdivisions=1, radius=0.018)["verts"]
            for v in knot:
                v.co += back
            for s in (-1, 1):
                g.tube(bm, [back, back + Vector((s * 0.02, 0.01, -0.07))], lambda i: 0.008, 4, 0, cap=True)
    return _bag_object(c, gid, bm, DUST_COLOURS if dust else BANDANA_COLOURS, body, arm, socket="eyes")


def make_balaclava(c, body, arm):
    """A balaclava: knitted wool cast over the head and down the neck (rigid on the head), with an
    opening for the eyes."""
    spec = c.spec
    tree = g.surface_tree(body)
    crown = g.socket_matrix(arm, spec, "head_top").to_translation()
    eyes = g.socket_matrix(arm, spec, "eyes").to_translation()
    centre = crown - Vector((0.0, -0.01, 0.105))
    rows, segs = 9, 16
    bm, _ = _new_bm()

    def on_head(theta, phi):
        d = Vector((math.sin(theta) * math.sin(phi), -math.sin(theta) * math.cos(phi), math.cos(theta)))
        hit = g.on_surface(tree, centre + d * 0.4, -d)
        p = hit if hit is not None and (hit - centre).dot(d) > 0 else centre + d * 0.1
        return p + d * BALACLAVA_EASE * (1.0 + 0.6 * max(0.0, -d.y))

    # The dome: over the crown down to the head's middle, cast out from its centre.
    top = bm.verts.new(on_head(0.0, 0.0))
    dome = [[bm.verts.new(on_head(math.radians(90.0) * (i + 1) / 6, 2 * math.pi * j / segs)) for j in range(segs)] for i in range(6)]
    # Below it, rings cast in level toward an axis that runs from the head's centre down to the neck:
    # round the jaw and chin, then the neck, so it never reaches out to the shoulders.
    neck = g.socket_matrix(arm, spec, "neck").to_translation()
    rings = []
    n = 8
    for i in range(1, n + 1):
        z = centre.z + (neck.z + BALACLAVA_NECK - centre.z) * i / n
        axis = centre.lerp(Vector((neck.x, neck.y, z)), i / n)
        axis.z = z
        ring = []
        for j in range(segs):
            phi = 2 * math.pi * j / segs
            d = Vector((math.sin(phi), -math.cos(phi), 0.0))
            # A little more room over the face (the nose, the stubble) than round the back.
            p = g.on_surface(tree, axis + d * 0.3, -d, BALACLAVA_EASE * (1.0 + 0.6 * max(0.0, math.cos(phi))))
            ring.append(bm.verts.new(p if p is not None else axis + d * 0.06))
        rings.append(ring)
    # Over the face it hangs straight down from the nose and chin rather than tucking in under them.
    for j in range(segs):
        if math.cos(2 * math.pi * j / segs) > 0.3:
            for i in range(1, len(rings)):
                above = rings[i - 1][j].co
                rings[i][j].co.y = min(rings[i][j].co.y, above.y + 0.35 * abs(rings[i][j].co.z - above.z))
    grid = dome + rings
    faces = [bm.faces.new((top, grid[0][j], grid[0][(j + 1) % segs])) for j in range(segs)]
    for i in range(len(grid) - 1):
        for j in range(segs):
            k = (j + 1) % segs
            quad = (grid[i][j], grid[i + 1][j], grid[i + 1][k], grid[i][k])
            mid = sum((v.co for v in quad), Vector()) / 4
            # The eye opening: the front, across the eyes' height.
            if abs(mid.x - eyes.x) < 0.07 and mid.y < eyes.y + 0.05 and abs(mid.z - (eyes.z + 0.004)) < 0.026:
                continue
            faces.append(bm.faces.new(quad))
    bmesh.ops.delete(bm, geom=[v for v in bm.verts if not v.link_faces], context="VERTS")  # inside the eye opening
    for f in faces:
        if f.normal.dot(f.calc_center_median() - centre) < 0:
            f.normal_flip()
    return _bag_object(c, "balaclava", bm, BALACLAVA_COLOURS, body, arm, socket="head_top")


def make_shove(c, arm):
    """The shove clip from Meshy's push (see SHOVE_GLB): the frames where it moves, the push (the hands
    furthest forward of the hips) put at SHOVE_HIT of SHOVE_FRAMES, the travel taken out, eased from and
    back to the idle's first pose. The source action goes."""
    import importlib.util
    spec_ = importlib.util.spec_from_file_location("zombie02_helpers", c.paths["folder"].parent / "zombie-02" / "zombie-02.py")
    z2 = importlib.util.module_from_spec(spec_)
    spec_.loader.exec_module(z2)
    src = bpy.data.actions["shove_src"]
    z2._bind(arm, src)
    pb = arm.pose.bones
    f0, f1 = (int(round(v)) for v in src.frame_range)
    reach, prev, moving = [], None, []
    for f in range(f0, f1 + 1):
        bpy.context.scene.frame_set(f)
        heads = [b.head.copy() for b in pb]
        hips = pb["Hips"].head
        reach.append(-((pb["LeftHand"].head.y + pb["RightHand"].head.y) / 2 - hips.y))  # forward is -y
        if prev is not None and sum((x - y).length for x, y in zip(heads, prev)) / len(heads) > 0.004:
            moving.append(f)
        prev = heads
    arm.animation_data.action = None
    i_hit = max(range(len(reach)), key=lambda i: reach[i])
    hit = f0 + i_hit
    # The push itself: back from the hit while the hands were still coming forward (the reach rising),
    # on from it while they draw back; Meshy's step up to the wall before that stays out.
    i0 = i_hit
    while i0 > 0 and reach[i0 - 1] < reach[i0] - 1e-4:
        i0 -= 1
    i1 = i_hit
    while i1 < len(reach) - 1 and reach[i1 + 1] < reach[i1] + 1e-4 and reach[i1] > reach[i0] + 0.25 * (reach[i_hit] - reach[i0]):
        i1 += 1
    s0, s1 = f0 + max(0, i0 - 2), f0 + min(len(reach) - 1, i1 + 2)
    n = SHOVE_FRAMES - 1
    h = int(round(SHOVE_HIT * n))
    times = [s0 + (hit - s0) * k / h for k in range(h)] + [hit + (s1 - hit) * k / (n - h) for k in range(n - h + 1)]
    shove = z2.remake(src, times, "shove")
    d = z2.read(shove)
    z2.centre(arm, d)
    moved = z2.pin_travel(arm, d, threshold=0.0)
    idle0 = z2.pose_at(z2.read(bpy.data.actions["Idle"]), 0)
    z2.blend(d, idle0, SHOVE_EASE[0])
    z2.blend(d, idle0, SHOVE_EASE[1], at_start=False)
    z2.write(shove, d)
    shove.use_cyclic = False
    bpy.data.actions.remove(src)
    c.note(f"shove = Meshy's Step Forward and Push frames {s0}-{s1}, the push (frame {hit}) at {SHOVE_HIT:.0%} of "
           f"{SHOVE_FRAMES} frames ({(SHOVE_FRAMES - 1) / 24:.2f} s), {moved:.2f} m of travel taken out, eased from and back to the idle")
