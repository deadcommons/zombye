"""Bathroom vanity: oak cupboard, almond laminate top with a sunk porcelain basin, chrome tap."""


def build(a):
    m = a.mesh()
    back = a.y1
    f = back - 0.52
    bx0, bx1, by0, by1 = -0.2, 0.2, f + 0.09, back - 0.12  # basin hole
    m.box((-0.34, f + 0.05, 0.0), (0.34, back, 0.08), "plastic_black")
    m.box((-0.35, f + 0.02, 0.08), (0.35, back, 0.72), "cabinet_oak")
    m.box((-0.3, f + 0.01, 0.14), (0.3, f + 0.02, 0.68), "wood_oak")
    m.block(0.22, f + 0.005, 0.6, 0.03, 0.01, 0.04, "chrome")
    # Basin: a porcelain floor and porcelain sides, the cupboard and the top round it.
    m.box((bx0, by0, 0.72), (bx1, by1, 0.74), "porcelain")
    m.box((-0.35, f + 0.02, 0.72), (0.35, by0, 0.82), "cabinet_oak", faces={"back": "porcelain"})
    m.box((-0.35, by1, 0.72), (0.35, back, 0.82), "cabinet_oak", faces={"front": "porcelain"})
    m.box((-0.35, by0, 0.72), (bx0, by1, 0.82), "cabinet_oak", faces={"right": "porcelain"})
    m.box((bx1, by0, 0.72), (0.35, by1, 0.82), "cabinet_oak", faces={"left": "porcelain"})
    m.box((-0.36, f, 0.82), (0.36, by0, 0.86), "laminate_almond", faces={"back": "porcelain"})
    m.box((-0.36, by1, 0.82), (0.36, back, 0.86), "laminate_almond", faces={"front": "porcelain"})
    m.box((-0.36, by0, 0.82), (bx0, by1, 0.86), "laminate_almond", faces={"right": "porcelain"})
    m.box((bx1, by0, 0.82), (0.36, by1, 0.86), "laminate_almond", faces={"left": "porcelain"})
    m.prism((0.0, back - 0.06, 0.74), 0.02, 0.003, "steel", sides=8)
    m.prism((0.0, back - 0.06, 0.86), 0.02, 0.12, "chrome", sides=6)
    m.box((-0.015, by1 - 0.06, 0.94), (0.015, back - 0.05, 0.97), "chrome")
    for x in (-0.09, 0.09):
        m.prism((x, back - 0.06, 0.86), 0.018, 0.04, "chrome", sides=6)
    return m
