"""Electric range: almond enamel body, black cooktop with four coil burners, backguard with knobs,
oven door with a dark window."""


def build(a):
    m = a.mesh()
    back = a.y1
    f = back - 0.68
    m.box((-0.36, f + 0.06, 0.0), (0.36, back, 0.04), "plastic_black")
    m.box((-0.38, f + 0.02, 0.04), (0.38, back, 0.9), "enamel_almond")
    m.box((-0.38, f + 0.02, 0.9), (0.38, back, 0.92), "iron_black")
    for x, y, r in ((-0.17, f + 0.2, 0.09), (0.17, f + 0.2, 0.07), (-0.17, f + 0.46, 0.07), (0.17, f + 0.46, 0.09)):
        m.prism((x, y, 0.92), r, 0.012, "steel", sides=10, cap="steel")
        m.prism((x, y, 0.932), r * 0.35, 0.002, "iron_black", sides=6)
    m.box((-0.38, back - 0.08, 0.92), (0.38, back, 1.08), "enamel_almond", top_inset=(0.0, 0.02))
    for x in (-0.27, -0.15, 0.15, 0.27):
        m.prism((x, back - 0.1, 1.0), 0.022, 0.02, "plastic_black", sides=8, axis="Y")
    m.box((-0.36, f + 0.01, 0.12), (0.36, f + 0.02, 0.72), "enamel_almond")
    m.box((-0.22, f + 0.005, 0.32), (0.22, f + 0.01, 0.6), "screen")
    m.box((-0.28, f, 0.65), (0.28, f + 0.005, 0.68), "chrome")
    m.box((-0.36, f + 0.01, 0.76), (0.36, f + 0.02, 0.86), "plastic_beige")
    return m
