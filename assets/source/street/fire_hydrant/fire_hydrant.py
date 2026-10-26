"""Fire hydrant: a squat faceted red barrel, a bone-white bonnet and nozzle caps, the pumper nozzle to the street."""

RED, CAP, NUT = "brand_red", "brand_bone_white", "iron_black"
PH = 0.3927  # pi / 8: a flat face, not a corner, toward each side


def build(a):
    m = a.mesh()
    # Base flange with a chamfered top, then the barrel and its upper flange.
    m.prism((0, 0, 0.0), 0.175, 0.05, RED, sides=8, phase=PH)
    m.prism((0, 0, 0.05), 0.175, 0.025, RED, sides=8, phase=PH, radius2=0.14)
    m.prism((0, 0, 0.075), 0.13, 0.40, RED, sides=8, phase=PH, radius2=0.125)
    m.prism((0, 0, 0.465), 0.16, 0.05, RED, sides=8, phase=PH, radius2=0.155)
    # Bonnet (dome as a frustum) and the operating nut on top.
    m.prism((0, 0, 0.52), 0.145, 0.04, CAP, sides=8, phase=PH)
    m.prism((0, 0, 0.56), 0.145, 0.12, CAP, sides=8, phase=PH, radius2=0.065)
    m.prism((0, 0, 0.68), 0.045, 0.07, NUT, sides=5)
    # Side nozzles with caps, left and right.
    for s in (-1, 1):
        x_in, x_cap = 0.10 * s, 0.19 * s
        lo = min(x_in, x_cap)
        m.prism((lo, 0, 0.38), 0.045, 0.09, RED, sides=6, axis="X")
        m.prism((0.19 if s > 0 else -0.22, 0, 0.38), 0.055, 0.03, CAP, sides=6, axis="X")
    # The pumper nozzle, bigger, facing the street, with its cap and nut.
    m.prism((0, -0.20, 0.30), 0.07, 0.10, RED, sides=8, axis="Y", phase=PH)
    m.prism((0, -0.235, 0.30), 0.08, 0.035, CAP, sides=8, axis="Y", phase=PH)
    m.prism((0, -0.26, 0.30), 0.03, 0.025, NUT, sides=5, axis="Y")
    return m
