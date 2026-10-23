"""Steel utility shelving with boxes, detergent bottles and a paint can on it."""


def build(a):
    m = a.mesh()
    back = a.y1
    f = back - 0.38
    p = 0.035
    for x in (-0.42 + p / 2, 0.42 - p / 2):
        for y in (f + p / 2, back - p / 2):
            m.block(x, y, 0.0, p, p, 1.8, "steel")
    levels = (0.1, 0.62, 1.14, 1.66)
    for z in levels:
        m.box((-0.42, f, z), (0.42, back, z + 0.025), "steel")
    s0, s1, s2, s3 = (z + 0.025 for z in levels)
    m.box((0.0, f + 0.04, s0), (0.36, back - 0.03, s0 + 0.3), "cardboard")
    for k, x in enumerate((-0.32, -0.18)):
        m.box((x - 0.05, f + 0.1, s1), (x + 0.05, f + 0.26, s1 + 0.26), "plastic_blue")
        m.prism((x, f + 0.16, s1 + 0.26), 0.025, 0.04, "plastic_red", sides=6)
    m.box((0.02, f + 0.06, s1), (0.34, back - 0.04, s1 + 0.18), "cardboard")
    m.box((-0.38, f + 0.05, s2), (-0.05, back - 0.03, s2 + 0.28), "cardboard")
    m.prism((0.2, f + 0.19, s2), 0.09, 0.18, "steel", sides=10, cap="plastic_red")
    m.box((-0.3, f + 0.06, s3), (0.1, back - 0.04, s3 + 0.1), "cardboard")
    return m
