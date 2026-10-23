"""Wardrobe: walnut carcass, two doors with a dark seam, chrome handles, plinth and cornice."""


def build(a):
    m = a.mesh()
    back = a.y1
    f = back - 0.58
    m.box((-0.4, f + 0.06, 0.0), (0.4, back, 0.08), "plastic_black")
    m.box((-0.42, f + 0.035, 0.08), (0.42, back, 1.88), "wood_walnut")
    m.box((-0.42, f, 1.88), (0.42, back, 1.95), "wood_walnut", top_inset=(0.01, 0.01))
    for x0, x1 in ((-0.41, -0.006), (0.006, 0.41)):
        m.box((x0, f + 0.015, 0.1), (x1, f + 0.035, 1.86), "wood_walnut")
    m.box((-0.006, f + 0.02, 0.1), (0.006, f + 0.035, 1.86), "plastic_black")
    for x in (-0.05, 0.05):
        m.block(x, f + 0.0075, 0.95, 0.02, 0.015, 0.16, "chrome")
    return m
