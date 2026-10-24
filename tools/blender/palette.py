"""The asset palette: every colour a Blender-built asset may use, in one place.

Muted, slightly desaturated early-1990s small-town colours (docs/PLAN.md §6.6): stained pine and
oak, almond and off-white enamel, laminate counters, mauve, teal and rust upholstery. Materials are
plain colours (no procedural shaders: they don't survive GLB export), so the game reads the exact
values back. How to use them is in the art-direction bible (docs/art-direction.md); keep this file the
only place colours are defined.

Each entry: name -> (sRGB hex, roughness, metallic). Materials are named `pal.<name>`.
"""

PALETTE = {
    # Brand anchors (Dead Commons style: stylised, friendly, playful-spooky; not photoreal, not
    # gory). Accents, used sparingly: a sign, a toy, a stripe, a zombie's skin.
    "brand_zombie_green": ("#8db06b", 0.8, 0.0),
    "brand_red": ("#d7262b", 0.5, 0.0),
    "brand_bone_white": ("#f2e6c9", 0.8, 0.0),
    "brand_near_black": ("#141414", 0.6, 0.0),
    # Wood
    "wood_pine": ("#a8875c", 0.75, 0.0),     # light stained pine: shelves, desks
    "wood_oak": ("#86643f", 0.7, 0.0),       # mid oak: tables, bed frames
    "wood_walnut": ("#5c4130", 0.7, 0.0),    # dark veneer: wardrobes, TV cabinets
    "wood_raw": ("#b69a72", 0.9, 0.0),       # unfinished boards: workbench top
    # Kitchen and bathroom
    "laminate_almond": ("#cdbf98", 0.45, 0.0),   # counter tops
    "cabinet_oak": ("#9a7650", 0.65, 0.0),       # kitchen cupboard doors
    "enamel_white": ("#dad6ca", 0.35, 0.0),      # fridge, washer, dryer
    "enamel_almond": ("#d3c6a6", 0.35, 0.0),     # stove body
    "porcelain": ("#e4e1d8", 0.2, 0.0),          # toilet, bathtub, basin
    "water": ("#8fa4a6", 0.1, 0.0),              # still water in a bowl
    # Metal and plastic
    "steel": ("#8b8e90", 0.4, 0.8),
    "chrome": ("#bfc2c2", 0.2, 0.9),
    "iron_black": ("#2a2a2c", 0.6, 0.3),     # stove top, grates
    "plastic_black": ("#242426", 0.5, 0.0),
    "plastic_grey": ("#76787a", 0.55, 0.0),
    "plastic_beige": ("#c2b79e", 0.5, 0.0),  # TV and appliance trim
    "screen": ("#2c3836", 0.15, 0.0),        # CRT glass, oven door glass
    "rubber": ("#363634", 0.9, 0.0),
    "paint_green": ("#5d6a52", 0.7, 0.0),    # workbench frame
    "plastic_red": ("#8c3f33", 0.5, 0.0),    # tool handles, bottle caps
    "plastic_blue": ("#4f6d8c", 0.5, 0.0),   # detergent bottles
    "cardboard": ("#a3825a", 0.95, 0.0),
    "paper": ("#d8d2c0", 0.95, 0.0),
    # Fabric
    "fabric_mauve": ("#8a6d78", 0.95, 0.0),
    "fabric_teal": ("#4e7170", 0.95, 0.0),
    "fabric_rust": ("#955a3d", 0.95, 0.0),
    "fabric_navy": ("#3e4961", 0.95, 0.0),
    "fabric_beige": ("#b5a487", 0.95, 0.0),
    "sheet_cream": ("#d6cdb8", 0.95, 0.0),
    "pillow_white": ("#e0dbcd", 0.95, 0.0),
    "blanket_blue": ("#5d728c", 0.95, 0.0),
}


def srgb_to_linear(hex_colour):
    """`#rrggbb` -> linear RGB floats, as Blender and glTF store base colours."""
    h = hex_colour.lstrip("#")
    out = []
    for i in (0, 2, 4):
        v = int(h[i:i + 2], 16) / 255
        out.append(((v + 0.055) / 1.055) ** 2.4 if v > 0.04045 else v / 12.92)
    return tuple(out)
