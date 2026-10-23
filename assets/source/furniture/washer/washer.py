"""Top-loading washing machine: off-white body, lid on top, control backsplash with dials."""


def appliance(m, back, f, lid):
    m.box((-0.33, f + 0.03, 0.0), (0.33, back - 0.02, 0.03), "plastic_black")
    m.box((-0.34, f, 0.03), (0.34, back, 0.92), "enamel_white")
    m.box((-0.34, back - 0.12, 0.92), (0.34, back, 1.08), "enamel_white", top_inset=(0.0, 0.03))
    m.box((-0.32, back - 0.125, 0.95), (0.32, back - 0.12, 1.05), "plastic_beige")
    for x in (-0.22, 0.22):
        m.prism((x, back - 0.145, 1.0), 0.03, 0.02, "plastic_grey", sides=10, axis="Y")
    m.box((-0.08, back - 0.13, 0.98), (0.08, back - 0.125, 1.02), "screen")
    if lid:
        # A dark seam round the lid so it reads from above.
        m.box((-0.31, f + 0.03, 0.92), (0.31, back - 0.15, 0.925), "plastic_black")
        m.box((-0.3, f + 0.04, 0.925), (0.3, back - 0.16, 0.945), "enamel_white", top_inset=(0.01, 0.01))
        m.box((-0.1, f + 0.03, 0.925), (0.1, f + 0.04, 0.94), "plastic_grey")


def build(a):
    m = a.mesh()
    back = a.y1
    appliance(m, back, back - 0.66, lid=True)
    return m
