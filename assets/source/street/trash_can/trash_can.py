"""Trash can: galvanised steel, two ribs and a rim, dark handles; the lid knocked askew by a bulging black bag."""

import math

from mathutils import Matrix, Vector

STEEL, BAG, DARK, PAPER = "steel", "plastic_black", "iron_black", "paper"
SIDES = 10
PH = math.pi / SIDES


def radius_at(z):
    """The body tapers out from 0.235 m at the foot to 0.27 m at the rim."""
    return 0.235 + (z - 0.04) / 0.76 * 0.035


def transform_new(m, n0, matrix):
    """Moves every vertex made since there were `n0`."""
    for v in list(m.bm.verts)[n0:]:
        v.co = matrix @ v.co


def build(a):
    m = a.mesh()
    # Foot ring, body, two ribs and the rim.
    m.prism((0, 0, 0.0), 0.245, 0.04, STEEL, sides=SIDES, phase=PH)
    m.prism((0, 0, 0.04), 0.235, 0.76, STEEL, sides=SIDES, phase=PH, radius2=0.27)
    for z in (0.24, 0.50):
        m.prism((0, 0, z), radius_at(z) + 0.014, 0.045, STEEL, sides=SIDES, phase=PH)
    m.prism((0, 0, 0.78), 0.285, 0.04, STEEL, sides=SIDES, phase=PH)
    # Handles, left and right, under the rim.
    for s in (-1, 1):
        m.block(s * 0.295, 0.0, 0.66, 0.05, 0.15, 0.06, DARK)
    # The bag bulging out of the top, a crumpled newspaper caught in the gap at the front.
    m.blob((0.0, -0.02, 0.84), (0.25, 0.25, 0.13), BAG, seed=7, subdiv=1, jitter=0.1, flatten_below=0.80)
    m.blob((-0.10, -0.20, 0.88), (0.09, 0.07, 0.04), PAPER, seed=3, subdiv=1, jitter=0.15)
    # The lid, made flat at the origin with a dent knocked into its rim, then turned, pushed back and
    # tipped up at the front onto the bag.
    n0 = len(m.bm.verts)
    m.prism((0, 0, 0.0), 0.30, 0.03, STEEL, sides=SIDES, phase=PH, radius2=0.295)
    m.prism((0, 0, 0.03), 0.29, 0.05, STEEL, sides=SIDES, phase=PH, radius2=0.17)
    m.block(0.0, 0.0, 0.08, 0.18, 0.05, 0.05, DARK)
    for v in list(m.bm.verts)[n0:]:
        r = math.hypot(v.co.x, v.co.y)
        if r > 0.25 and abs(math.atan2(v.co.y, v.co.x) - math.radians(18)) < math.radians(25):
            v.co.x *= 0.9
            v.co.y *= 0.9
            v.co.z -= 0.03
    tip = (Matrix.Translation(Vector((0.04, 0.03, 0.0)))
           @ Matrix.Translation(Vector((0, 0.27, 0.82))) @ Matrix.Rotation(math.radians(-19), 4, "X")
           @ Matrix.Translation(Vector((0, -0.27, 0))) @ Matrix.Rotation(math.radians(25), 4, "Z"))
    transform_new(m, n0, tip)
    return m
