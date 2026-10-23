"""Bathtub: a porcelain tub along the wall (+Y), hollow inside, chrome tap at the left end."""


def build(a):
    m = a.mesh()
    back = a.y1
    f = back - 0.74
    x0, x1 = -0.85, 0.85
    rim, top, t = 0.18, 0.55, 0.07
    m.box((x0, f, 0.0), (x1, back, rim), "porcelain")
    m.box((x0, f, rim), (x1, f + t, top), "porcelain")
    m.box((x0, back - t, rim), (x1, back, top), "porcelain")
    m.box((x0, f + t, rim), (x0 + t + 0.08, back - t, top), "porcelain", faces={"right": "porcelain"})
    m.box((x1 - t, f + t, rim), (x1, back - t, top), "porcelain")
    m.prism((x0 + 0.2, back - 0.035, top), 0.02, 0.07, "chrome", sides=6)
    m.box((x0 + 0.185, back - 0.13, top + 0.035), (x0 + 0.215, back - 0.03, top + 0.06), "chrome")
    for dx in (-0.06, 0.06):
        m.prism((x0 + 0.2 + dx, back - 0.035, top), 0.018, 0.035, "chrome", sides=6)
    m.prism((x0 + 0.32, f + 0.37, rim), 0.025, 0.004, "steel", sides=8)
    return m
