"""Top-freezer fridge: off-white enamel, two doors with a dark seam, grey handles, kick plate."""


def build(a):
    m = a.mesh()
    back = a.y1
    f = back - 0.72
    m.box((-0.36, f + 0.04, 0.0), (0.36, back, 0.08), "plastic_black")
    m.box((-0.38, f + 0.03, 0.08), (0.38, back, 1.75), "enamel_white", top_inset=(0.01, 0.01))
    m.box((-0.375, f + 0.01, 1.2), (0.375, f + 0.03, 1.73), "enamel_white", top_inset=(0.005, 0.0))
    m.box((-0.375, f + 0.01, 0.1), (0.375, f + 0.03, 1.18), "enamel_white")
    m.box((-0.37, f + 0.015, 1.18), (0.37, f + 0.03, 1.2), "plastic_black")
    m.box((0.27, f, 1.26), (0.31, f + 0.01, 1.5), "plastic_grey")
    m.box((0.27, f, 0.75), (0.31, f + 0.01, 1.12), "plastic_grey")
    return m
