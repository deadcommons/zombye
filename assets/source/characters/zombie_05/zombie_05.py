"""zombie_05: the woman, a zombie base body (Meshy body, rig and clips; everything else in Blender).

Run by zombye tools/blender/zchar.py (build.py -- <this folder>). The steps are the shared ones in
../zombie_body.py; the numbers below are this body's own.
"""
import importlib.util
import sys
from pathlib import Path

_p = Path(__file__).resolve().parent.parent / "zombie_body.py"
_s = importlib.util.spec_from_file_location("zombie_body", _p)
zb = importlib.util.module_from_spec(_s)
sys.dont_write_bytecode, _d = True, sys.dont_write_bytecode
try:
    _s.loader.exec_module(zb)
finally:
    sys.dont_write_bytecode = _d

PARAMS = {
    "idle_calm": (81, 102), "idle_frames": 63,          # Meshy Idle's calm stretch, there and back (as survivor-01)
    "lunge_max_frames": 48, "lunge_ease_in": 6, "lunge_ease_out": 14,
    "crawl_legs": 1.0,                                  # how far the legs ease toward straight (trailing)
    "walk_step": 1.85,                                  # the walk's leg swing scaled up: stride about 0.9 m a loop
    "no_stoop": ("lunge",),                             # hunched already in the text-to-motion clip (the walk takes the stoop)
    "head_scale": 0.70,                                 # Gate 2: her head in proportion with the men (owner: "Use the fix")
    "tone": {"saturation": 0.92, "contrast": 0.94, "lift": 0.04, "tint": (0.94, 1.04, 0.92)},  # Gate 3: greener, lighter for the night
}


def build(c):
    c.extras = {
        "ai_generated": True,
        "generator": "Meshy (Pro): nano-banana-pro image, meshy-t2.5 image-to-3D, auto-rig, library animations and "
                     "text-to-motion; cleaned, stooped, LOD and VAT in Blender",
        "meshy_tasks": "image 01a1c53a-06b6-7013-a640-d96a2342de12, model 01a1c53d-5182-734c-a4c0-772b24f8f919, rig 01a1c53f-21f8-709d-ae9b-db111def6616, library clips 01a1c55d-6c12-72d9-93a5-0a619a470006, "
                       "motions 01a1c544-9550-7646-9bbf-1d2acfad16df 01a1c544-997d-7327-895b-9cb1cf881c2b "
                       "01a1c544-9cc2-7522-a420-6fc58195c81c on this rig 01a1c54d-3d38-76ac-8e4b-9f773d79e7d7, 01a1c54d-400e-7063-83cf-886dbb510743, 01a1c559-7e51-7030-b0d1-cbf2ee783514; climb, vault 01a1c558-c129-720d-843b-73dc7fd29fa0, 01a1c558-c392-7233-835f-c655e2284bc2",
        "author": "Dead Commons",
    }
    return zb.build(c, PARAMS)
