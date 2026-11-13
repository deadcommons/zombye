"""zombie_03: the gaunt old shambler, a zombie base body (Meshy body, rig and clips; everything else in Blender).

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
    "walk_step": 1.9,                                   # the walk's leg swing scaled up: stride about 0.9 m a loop
    "no_stoop": ("lunge",),                             # hunched already in the text-to-motion clip (the walk takes the stoop)
    "tone": {"saturation": 0.92, "contrast": 0.94, "lift": 0.04, "tint": (0.94, 1.04, 0.92)},  # Gate 3: greener, lighter for the night
}


def build(c):
    c.extras = {
        "ai_generated": True,
        "generator": "Meshy (Pro): nano-banana-pro image, meshy-t2.5 image-to-3D, auto-rig, library animations and "
                     "text-to-motion; cleaned, stooped, LOD and VAT in Blender",
        "meshy_tasks": "image 01a1c539-feea-7262-9b24-bb19e19c33e5, model 01a1c53d-4a56-75c6-b9bf-3dcbf70ad86a, rig 01a1c53f-0772-753d-931f-dad1b3ca2a3d, library clips 01a1c544-925c-75cb-b17a-7c0bbfd865d7, "
                       "motions 01a1c544-9550-7646-9bbf-1d2acfad16df 01a1c544-997d-7327-895b-9cb1cf881c2b "
                       "01a1c544-9cc2-7522-a420-6fc58195c81c on this rig 01a1c545-566e-72ca-8ff8-f7e884de8fc0, 01a1c545-5a61-7120-8338-aad82626e32b, 01a1c545-5cec-728e-96bf-a29a14ed58b8",
        "author": "Dead Commons",
    }
    return zb.build(c, PARAMS)
