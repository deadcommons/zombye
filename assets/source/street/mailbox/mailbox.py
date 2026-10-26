"""Mailbox: an off-white rounded-top box on a leaning oak post, the door to the street, the red flag up."""

import math


def extrude_y(m, profile, y0, y1, mat, front=None):
    """A closed shape: the (x, z) outline `profile` (convex, anticlockwise from the front) run from y0 to y1."""
    f = [m.bm.verts.new((x, y0, z)) for x, z in profile]
    b = [m.bm.verts.new((x, y1, z)) for x, z in profile]
    cx = sum(x for x, _ in profile) / len(profile)
    cz = sum(z for _, z in profile) / len(profile)
    n = len(profile)
    for k in range(n):
        k2 = (k + 1) % n
        mx = (profile[k][0] + profile[k2][0]) / 2 - cx
        mz = (profile[k][1] + profile[k2][1]) / 2 - cz
        m._face([f[k], f[k2], b[k2], b[k]], mat, (mx, 0, mz))
    m._face(f, front or mat, (0, -1, 0))
    m._face(b, mat, (0, 1, 0))


def box_profile(w, wall, r, z0, inset=0.0):
    """A mailbox end: straight sides `wall` high, then a half-octagon roof of radius r."""
    hw = w / 2 - inset
    rr = r - inset
    pts = [(-hw, z0 + inset), (hw, z0 + inset), (hw, z0 + wall)]
    for a in (45, 90, 135):
        t = math.radians(a)
        pts.append((rr * math.cos(t), z0 + wall + rr * math.sin(t)))
    pts.append((-hw, z0 + wall))
    return pts


def build(a):
    m = a.mesh()
    # Post and the plank the box sits on.
    m.block(0.0, 0.04, 0.0, 0.10, 0.10, 0.86, "wood_oak", top_inset=(0.005, 0.005))
    m.block(0.0, 0.0, 0.86, 0.16, 0.38, 0.04, "wood_oak")
    # The box: straight sides, a rounded (faceted) roof, the door a little proud at the front.
    z0, w, wall, r = 0.90, 0.22, 0.13, 0.11
    extrude_y(m, box_profile(w, wall, r, z0), -0.26, 0.26, "enamel_white")
    extrude_y(m, box_profile(w, wall, r, z0, inset=0.012), -0.275, -0.26, "enamel_white")
    # Latch tab on the door, at the top.
    m.block(0.0, -0.285, z0 + wall + 0.035, 0.06, 0.02, 0.05, "iron_black")
    # The flag, raised: an arm on the right side with the plate at its top, on a dark pivot.
    fx = w / 2
    m.block(fx + 0.015, 0.10, z0 + 0.02, 0.03, 0.06, 0.06, "iron_black")
    m.block(fx + 0.035, 0.10, z0 + 0.03, 0.02, 0.04, 0.22, "brand_red")
    m.box((fx + 0.025, 0.08, z0 + 0.16), (fx + 0.045, 0.26, z0 + 0.25), "brand_red")
    # Wear by shape: the whole thing leans a couple of degrees to the left.
    lean = math.tan(math.radians(-2.0))
    for v in m.bm.verts:
        v.co.x += v.co.z * lean
    return m
