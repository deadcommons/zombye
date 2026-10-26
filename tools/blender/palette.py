"""The asset palette: every colour a Blender-built asset may use, read from the game's one palette file.

Muted, slightly desaturated early-1990s small-town colours (docs/PLAN.md §6.6): stained pine and
oak, almond and off-white enamel, laminate counters, mauve, teal and rust upholstery. Materials are
plain colours (no procedural shaders: they don't survive GLB export), so the game reads the exact
values back. How to use them is in the art-direction bible (docs/art-direction.md).

The colours live in `assets/data/palette.ron` (or the file `ZOMBYE_PALETTE` names), which the game's
client reads too for the world it generates: change a colour there, never here. Each entry:
name -> (sRGB hex, roughness, metallic). Materials are named `pal.<name>`. `SETS` holds the named
sets (lists of colour names) the client draws from.
"""

import os
import re

_HERE = os.path.dirname(os.path.abspath(__file__))
PALETTE_FILE = os.environ.get("ZOMBYE_PALETTE") or os.path.join(_HERE, "..", "..", "assets", "data", "palette.ron")

_COLOUR = re.compile(r'^"([a-z0-9_]+)":\s*\(\s*"(#[0-9a-fA-F]{6})"\s*,\s*([0-9.]+)\s*,\s*([0-9.]+)\s*\),?$')
_SET = re.compile(r'^"([a-z0-9_]+)":\s*\[([^\]]*)\],?$')


def load(path=PALETTE_FILE):
    """Read the palette file: (colours, sets). One entry to a line, `//` comments (see its header)."""
    colours, sets, section = {}, {}, None
    with open(path, encoding="utf-8") as f:
        for n, raw in enumerate(f, 1):
            line = raw.split("//", 1)[0].strip()
            if not line or line in ("(", ")", "},", "}", "{"):
                continue
            if line.startswith("colours:"):
                section = colours
                continue
            if line.startswith("sets:"):
                section = sets
                continue
            m = _COLOUR.match(line) if section is colours else _SET.match(line) if section is sets else None
            if not m:
                raise ValueError(f"{path}:{n}: can't read {raw.strip()!r}")
            name = m.group(1)
            if name in section:
                raise ValueError(f"{path}:{n}: {name!r} twice")
            if section is colours:
                section[name] = (m.group(2).lower(), float(m.group(3)), float(m.group(4)))
            else:
                section[name] = [s.strip().strip('"') for s in m.group(2).split(",") if s.strip()]
    for name, members in sets.items():
        for c in members:
            if c not in colours:
                raise ValueError(f"{path}: set {name!r} names {c!r}, which isn't a colour")
    return colours, sets


PALETTE, SETS = load()


def srgb_to_linear(hex_colour):
    """`#rrggbb` -> linear RGB floats, as Blender and glTF store base colours."""
    h = hex_colour.lstrip("#")
    out = []
    for i in (0, 2, 4):
        v = int(h[i:i + 2], 16) / 255
        out.append(((v + 0.055) / 1.055) ** 2.4 if v > 0.04045 else v / 12.92)
    return tuple(out)
