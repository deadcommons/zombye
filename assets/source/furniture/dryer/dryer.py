"""Tumble dryer: matches the washer; a round front door with a dark window and chrome ring."""


def build(a):
    m = a.mesh()
    back = a.y1
    f = back - 0.66
    body = f + 0.03
    m.box((-0.33, body + 0.03, 0.0), (0.33, back - 0.02, 0.03), "plastic_black")
    m.box((-0.34, body, 0.03), (0.34, back, 0.92), "enamel_white")
    m.box((-0.34, back - 0.12, 0.92), (0.34, back, 1.08), "enamel_white", top_inset=(0.0, 0.03))
    m.box((-0.32, back - 0.125, 0.95), (0.32, back - 0.12, 1.05), "plastic_beige")
    for x in (-0.22, 0.22):
        m.prism((x, back - 0.145, 1.0), 0.03, 0.02, "plastic_grey", sides=10, axis="Y")
    m.prism((0.0, body - 0.025, 0.5), 0.21, 0.025, "chrome", sides=16, axis="Y")
    m.prism((0.0, f, 0.5), 0.15, 0.01, "screen", sides=16, axis="Y")
    m.box((0.17, f, 0.46), (0.2, f + 0.005, 0.54), "plastic_grey")
    return m
