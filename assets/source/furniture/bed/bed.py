"""Double bed: oak frame, headboard at the back (+Y, the wall), blanket folded back, two pillows."""


def build(a):
    m = a.mesh()
    back, foot = a.y1, a.y0
    w = 0.75
    for x in (-w + 0.04, w - 0.04):
        m.block(x, foot + 0.07, 0.0, 0.07, 0.07, 0.14, "wood_walnut")
    m.box((-w, foot + 0.05, 0.12), (w, back - 0.06, 0.32), "wood_oak")
    m.box((-w, back - 0.06, 0.0), (w, back, 0.95), "wood_oak", top_inset=(0.04, 0.0))
    m.box((-w, foot, 0.0), (w, foot + 0.05, 0.46), "wood_oak")
    m.box((-0.71, foot + 0.07, 0.32), (0.71, back - 0.08, 0.5), "sheet_cream")
    m.box((-0.73, foot + 0.06, 0.5), (0.73, 0.18, 0.56), "blanket_blue",
          faces={"front": "blanket_blue"})
    m.box((-0.73, 0.18, 0.5), (0.73, 0.32, 0.58), "sheet_cream", top_inset=(0.0, 0.03))
    for k, x in enumerate((-0.35, 0.35)):
        m.blob((x, back - 0.32, 0.56), (0.31, 0.17, 0.08), "pillow_white", seed=k + 1, flatten_below=0.5)
    return m
