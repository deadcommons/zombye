"""zombie_04: the big-framed man, a zombie base body (Meshy body, rig and clips; everything else in Blender).

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
    "no_stoop": ("lunge",),                             # hunched already in the text-to-motion clip (the walk takes the stoop)
    "tone": {"saturation": 0.92, "contrast": 0.94, "lift": 0.04, "tint": (0.94, 1.04, 0.92)},  # Gate 3: greener, lighter for the night
}


def build(c):
    c.extras = {
        "ai_generated": True,
        "generator": "Meshy (Pro): nano-banana-pro image, meshy-t2.5 image-to-3D, auto-rig, library animations and "
                     "text-to-motion; cleaned, stooped, LOD and VAT in Blender",
        "meshy_tasks": "image 01a1c53a-0352-734f-886f-bfefb8224a99, model 01a1c53d-4de6-77b3-820d-a666305d093e, rig 01a1c53f-1436-7749-847d-960ae644b69a, library clips 01a1c544-93db-704b-a75f-3ba0e43c2b94, "
                       "motions 01a1c544-9550-7646-9bbf-1d2acfad16df 01a1c544-997d-7327-895b-9cb1cf881c2b "
                       "01a1c544-9cc2-7522-a420-6fc58195c81c on this rig 01a1c545-5f13-718f-806f-453b8d952449, 01a1c545-6171-7137-870b-8cdd0ae55860, 01a1c545-6384-711c-93cd-230717d6da57",
        "author": "Dead Commons",
    }
    return zb.build(c, PARAMS)
