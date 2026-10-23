"""Workbench: green steel frame, raw wood top, a bank of drawers, a pegboard with tools, a vise."""


def build(a):
    m = a.mesh()
    back = a.y1
    f = back - 0.72
    for x in (-0.88, 0.88):
        for y in (f + 0.06, back - 0.06):
            m.block(x, y, 0.0, 0.06, 0.06, 0.86, "paint_green")
    m.box((-0.88, f + 0.06, 0.14), (0.88, back - 0.04, 0.17), "wood_raw")
    m.box((-0.9, f + 0.03, 0.78), (0.9, back - 0.02, 0.86), "paint_green")
    m.box((-0.92, f, 0.86), (0.92, back, 0.92), "wood_raw")
    m.box((0.3, f + 0.04, 0.17), (0.86, back - 0.04, 0.78), "paint_green")
    for k in range(3):
        z0 = 0.2 + k * 0.19
        m.box((0.32, f + 0.03, z0), (0.84, f + 0.04, z0 + 0.17), "plastic_grey")
        m.block(0.58, f + 0.025, z0 + 0.12, 0.12, 0.01, 0.02, "chrome")
    # Pegboard and the tools on it.
    m.box((-0.9, back - 0.02, 0.92), (0.9, back, 1.45), "cardboard")
    t = back - 0.02
    m.box((-0.7, t - 0.02, 1.1), (-0.67, t, 1.36), "wood_oak")
    m.box((-0.76, t - 0.03, 1.33), (-0.6, t, 1.38), "steel")
    m.box((-0.45, t - 0.01, 1.02), (-0.1, t, 1.2), "steel", top_inset=(0.0, 0.0))
    m.box((-0.12, t - 0.025, 1.08), (-0.02, t, 1.24), "wood_oak")
    m.box((0.12, t - 0.015, 1.04), (0.15, t, 1.34), "plastic_red")
    m.box((0.25, t - 0.015, 1.06), (0.28, t, 1.32), "plastic_blue")
    m.box((0.4, t - 0.01, 1.0), (0.44, t, 1.3), "steel")
    m.box((0.55, t - 0.04, 1.2), (0.85, t, 1.24), "wood_raw")
    # Vise at the front left corner, a toolbox on the top.
    m.box((-0.84, f, 0.92), (-0.66, f + 0.16, 1.0), "iron_black")
    m.box((-0.82, f, 1.0), (-0.68, f + 0.03, 1.06), "iron_black")
    m.prism((-0.75, f, 0.96), 0.012, 0.02, "steel", sides=6, axis="Y")
    m.box((-0.35, back - 0.3, 0.92), (0.05, back - 0.06, 1.1), "plastic_red", top_inset=(0.0, 0.03))
    m.box((-0.2, back - 0.19, 1.1), (-0.1, back - 0.17, 1.14), "plastic_black")
    return m
