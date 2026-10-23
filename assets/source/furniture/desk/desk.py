"""Desk: pine top, a pedestal of drawers on the right, a kneehole, a desk lamp and papers."""


def build(a):
    m = a.mesh()
    back = a.y1
    f = back - 0.56
    m.box((-0.42, f, 0.72), (0.42, back, 0.76), "wood_pine")
    m.box((0.0, f + 0.02, 0.0), (0.4, back - 0.02, 0.72), "wood_pine")
    for k in range(3):
        z0 = 0.04 + k * 0.225
        m.box((0.02, f + 0.01, z0), (0.38, f + 0.02, z0 + 0.21), "wood_oak")
        m.block(0.2, f + 0.005, z0 + 0.15, 0.06, 0.01, 0.015, "chrome")
    m.box((-0.41, f + 0.02, 0.0), (-0.38, back - 0.02, 0.72), "wood_pine")
    m.box((-0.38, back - 0.06, 0.3), (0.0, back - 0.03, 0.72), "wood_pine")
    # Lamp and papers.
    lx, ly = -0.28, back - 0.14
    m.prism((lx, ly, 0.76), 0.07, 0.03, "plastic_beige", sides=8)
    m.prism((lx, ly, 0.79), 0.012, 0.22, "plastic_beige", sides=6)
    m.prism((lx, ly, 1.0), 0.11, 0.18, "fabric_beige", sides=8, radius2=0.065)
    m.box((-0.05, f + 0.08, 0.76), (0.2, f + 0.3, 0.765), "paper")
    m.box((0.02, f + 0.12, 0.765), (0.24, f + 0.36, 0.77), "paper")
    return m
