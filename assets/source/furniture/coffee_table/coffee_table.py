"""Coffee table: oak top on four legs, a lower shelf with magazines, a shallow drawer."""


def build(a):
    m = a.mesh()
    m.box((-0.4, -0.25, 0.38), (0.4, 0.25, 0.42), "wood_oak")
    m.box((-0.36, -0.21, 0.3), (0.36, 0.21, 0.38), "wood_oak")
    for x in (-0.36, 0.36):
        for y in (-0.21, 0.21):
            m.block(x, y, 0.0, 0.05, 0.05, 0.38, "wood_oak")
    m.box((-0.34, -0.19, 0.08), (0.34, 0.19, 0.1), "wood_oak")
    m.box((-0.18, -0.245, 0.31), (0.18, -0.235, 0.37), "wood_walnut")
    m.block(0.0, -0.245, 0.33, 0.04, 0.01, 0.02, "chrome")
    m.box((-0.26, -0.12, 0.1), (-0.02, 0.08, 0.108), "paper")
    m.box((-0.22, -0.1, 0.108), (0.0, 0.12, 0.114), "plastic_red")
    return m
