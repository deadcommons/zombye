"""Kitchen counter: oak base cupboards and a drawer row under an almond laminate worktop."""


def build(a):
    m = a.mesh()
    back = a.y1
    f = back - 0.64
    face = f + 0.04  # door fronts
    m.box((-0.92, f + 0.1, 0.0), (0.92, back, 0.1), "plastic_black")
    m.box((-0.92, face + 0.015, 0.1), (0.92, back, 0.88), "wood_walnut",
          faces={"left": "cabinet_oak", "right": "cabinet_oak"})
    n = 4
    w = 1.84 / n
    for k in range(n):
        x0 = -0.92 + k * w + 0.005
        x1 = x0 + w - 0.01
        m.box((x0, face, 0.12), (x1, face + 0.015, 0.66), "cabinet_oak")
        m.box((x0, face, 0.68), (x1, face + 0.015, 0.86), "cabinet_oak")
        hx = x1 - 0.06 if k % 2 == 0 else x0 + 0.06
        m.block(hx, face - 0.005, 0.5, 0.02, 0.01, 0.1, "chrome")
        m.block((x0 + x1) / 2, face - 0.005, 0.76, 0.1, 0.01, 0.02, "chrome")
    m.box((-0.92, f, 0.88), (0.92, back, 0.92), "laminate_almond")
    return m
