"""Sofa: mauve upholstery, two seat cushions, two soft back cushions, rolled arms, dark feet."""


def build(a):
    m = a.mesh()
    back = a.y1
    f = back - 0.82
    for x in (-0.85, 0.85):
        for y in (f + 0.06, back - 0.06):
            m.block(x, y, 0.0, 0.06, 0.06, 0.06, "wood_walnut")
    m.box((-0.74, f + 0.02, 0.06), (0.74, back, 0.4), "fabric_mauve")
    for s in (-1, 1):
        x0, x1 = sorted((s * 0.74, s * 0.92))
        m.box((x0, f, 0.06), (x1, back, 0.58), "fabric_mauve")
        m.prism((s * 0.83, f, 0.58), 0.09, 0.82, "fabric_mauve", sides=8, axis="Y", phase=0.39)
    m.box((-0.74, back - 0.2, 0.4), (0.74, back, 0.86), "fabric_mauve", top_inset=(0.0, 0.04))
    for x0, x1 in ((-0.74, -0.005), (0.005, 0.74)):
        m.box((x0, f + 0.02, 0.4), (x1, back - 0.2, 0.52), "fabric_mauve", top_inset=(0.02, 0.02))
    for k, x in enumerate((-0.37, 0.37)):
        m.blob((x, back - 0.27, 0.66), (0.34, 0.09, 0.16), "fabric_mauve", seed=k + 3, subdiv=2, jitter=0.04)
    return m
