"""Toilet: porcelain bowl on a tapered pedestal, beige seat and lid down, cistern against the wall."""


def build(a):
    m = a.mesh()
    back = a.y1
    f = back - 0.68
    m.box((-0.1, f + 0.12, 0.0), (0.1, back - 0.16, 0.3), "porcelain", top_inset=(-0.04, -0.03))
    m.box((-0.18, f, 0.3), (0.18, back - 0.17, 0.4), "porcelain", top_inset=(-0.0, 0.0))
    m.box((-0.185, f, 0.4), (0.185, back - 0.19, 0.44), "plastic_beige", top_inset=(0.02, 0.03))
    m.box((-0.22, back - 0.18, 0.3), (0.22, back, 0.74), "porcelain")
    m.box((-0.22, back - 0.19, 0.74), (0.22, back, 0.78), "porcelain", top_inset=(0.01, 0.01))
    m.box((-0.18, back - 0.19, 0.65), (-0.11, back - 0.18, 0.675), "chrome")
    return m
