"""Dining table: oak top on four legs, a chair tucked in at each end with its back outward."""


def chair(m, side):
    # `side` 1: the chair at +X, its back at the far end.
    def bx(x0, x1, y0, y1, z0, z1, mat, **kw):
        lo, hi = sorted((side * x0, side * x1))
        m.box((lo, y0, z0), (hi, y1, z1), mat, **kw)
    bx(0.52, 0.92, -0.2, 0.2, 0.42, 0.46, "wood_oak")
    bx(0.54, 0.88, -0.18, 0.18, 0.46, 0.49, "fabric_mauve", top_inset=(0.01, 0.01))
    for y in (-0.18, 0.15):
        bx(0.53, 0.56, y, y + 0.03, 0.0, 0.42, "wood_oak")
        bx(0.885, 0.92, y, y + 0.03, 0.0, 0.92, "wood_oak")
    bx(0.89, 0.915, -0.15, 0.15, 0.64, 0.88, "wood_oak")


def build(a):
    m = a.mesh()
    m.box((-0.58, -0.38, 0.72), (0.58, 0.38, 0.76), "wood_oak")
    m.box((-0.54, -0.34, 0.64), (0.54, 0.34, 0.72), "wood_oak")
    for x in (-0.53, 0.53):
        for y in (-0.33, 0.33):
            m.block(x, y, 0.0, 0.05, 0.05, 0.72, "wood_oak")
    chair(m, 1)
    chair(m, -1)
    return m
