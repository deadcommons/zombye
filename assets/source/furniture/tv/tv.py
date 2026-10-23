"""TV on a stand: walnut cabinet with an open shelf and a VCR, a grey CRT set with a deep back and
rabbit ears."""


def build(a):
    m = a.mesh()
    back = a.y1
    f = back - 0.5
    m.box((-0.4, f, 0.0), (0.4, back, 0.06), "wood_walnut")
    m.box((-0.4, f, 0.46), (0.4, back, 0.5), "wood_walnut")
    for x0, x1 in ((-0.4, -0.37), (0.37, 0.4)):
        m.box((x0, f, 0.06), (x1, back, 0.46), "wood_walnut")
    m.box((-0.37, back - 0.03, 0.06), (0.37, back, 0.46), "plastic_black")
    m.box((-0.37, f + 0.02, 0.24), (0.37, back - 0.03, 0.26), "wood_walnut")
    m.box((-0.22, f + 0.06, 0.26), (0.22, back - 0.08, 0.34), "plastic_black")
    m.box((0.08, f + 0.055, 0.29), (0.18, f + 0.06, 0.31), "screen")
    m.box((-0.3, f + 0.08, 0.06), (-0.1, back - 0.1, 0.16), "plastic_black")
    # The set: bezel, deep back, screen, knobs and antenna.
    m.box((-0.3, f + 0.03, 0.5), (0.3, f + 0.2, 0.98), "plastic_grey")
    m.box((-0.22, f + 0.2, 0.56), (0.22, back - 0.04, 0.92), "plastic_grey", top_inset=(0.03, 0.0))
    m.box((-0.24, f + 0.02, 0.58), (0.18, f + 0.03, 0.92), "screen")
    for z in (0.86, 0.78):
        m.prism((0.24, f + 0.01, z), 0.018, 0.02, "plastic_black", sides=8, axis="Y")
    m.box((-0.05, f + 0.08, 0.98), (0.05, f + 0.16, 1.0), "plastic_black")
    for s in (-1, 1):
        m.box((s * 0.02 - 0.004, f + 0.115, 1.0), (s * 0.02 + 0.004, f + 0.125, 1.12), "steel", top_shift=(s * 0.12, 0.03))
    return m
