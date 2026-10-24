"""Build assets from a shell, headless:

    /Applications/Blender.app/Contents/MacOS/Blender --background --factory-startup \
        --python tools/blender/build.py -- furniture/bed furniture/sofa
    ... --python tools/blender/build.py -- furniture          # every furniture asset, in either repo
    ... --python tools/blender/build.py -- --check furniture  # validate only: no renders, no files
    ... --python tools/blender/build.py -- ~/games/zombye-art/characters/zombie-01   # a character (zchar)
    ... --python tools/blender/build.py -- ~/games/zombye-art/characters/survivor-01  # body + garments (zgarment)
    ... --python tools/blender/build.py -- --g3 furniture/bed   # also the Gate 3 sheet (renders/bed_g3.png)
    ... --python tools/blender/build.py -- --refs a.png,b.png furniture/bed   # Gate 3 beside these instead
    ... --python tools/blender/build.py -- --out /tmp/scratch furniture/bed   # write nothing in either repo

Each asset is built where it's being worked on: its folder in the art repo when it has one, else the
game repo's copy (tools/blender/artrepo.py). A character is given as its art-repo folder (a directory
holding `<folder name>.ron`, built by zchar); with `--check` it is validated from its saved `.blend`.
Every review sheet has the scale figure and the door frame. `--g3` adds the Gate 3 sheet: the review
sheet beside the asset's approved reference and one or two approved sheets of its category
(`zasset.gate3_refs`); `--refs A,B` puts those images beside it instead (implies `--g3`). `--out DIR`
writes the renders, `.blend` and exports to DIR (a folder per asset) and only reads the work folder.
Prints each asset's validation report and exits 1 if any failed.
"""

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import zasset  # noqa: E402
import zchar  # noqa: E402
import zgarment  # noqa: E402

argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
check_only, g3, refs, out_dir, args = False, False, None, None, []
while argv:
    a = argv.pop(0)
    if a == "--check":
        check_only = True
    elif a == "--g3":
        g3 = True
    elif a == "--refs" and argv:
        refs, g3 = [r for r in argv.pop(0).split(",") if r], True
    elif a == "--out" and argv:
        out_dir = Path(argv.pop(0)).expanduser()
    else:
        args.append(a)
assets = []
for a in args:
    folder = Path(a).expanduser()
    if folder.is_dir() and (folder / f"{folder.name}.ron").exists():
        assets.append(folder)
    elif "/" in a:
        assets.append(a)
    else:
        assets += zasset.artrepo.list_assets(a)
if not assets:
    print(__doc__)
    sys.exit(2)

failed = []
for asset in assets:
    try:
        out = out_dir / (asset.name if isinstance(asset, Path) else asset.replace("/", "-")) if out_dir else None
        if isinstance(asset, Path):
            builder = zgarment.build if zgarment.is_modular(asset) else zchar.build  # garments in the spec
            report = builder(asset, render=not check_only, export=not check_only, save=not check_only,
                                 from_blend=check_only, out_dir=out, g3=g3, refs=refs)
        else:
            report = zasset.build(asset, render=not check_only, export=not check_only, save=not check_only,
                                  out_dir=out, g3=g3, refs=refs)
    except Exception as e:  # a broken script fails its asset, not the run
        report = {"asset": str(asset), "ok": False, "checks": [{"check": "build script runs", "ok": False, "detail": repr(e)}]}
    print(zasset.summary(report))
    for note in report.get("notes", []):
        print(f"  note: {note}")
    for key in ("sheet", "g3", "glb", "blend"):
        if key in report:
            print(f"  {key}: {zasset.rel(report[key])}")
    if "vat" in report:
        v = report["vat"]
        print(f"  vat: {zasset.rel(v['path'])}: {v['vertices']} vertices, {v['rows']} rows, {v['bytes']} bytes; "
              + ", ".join(f"{c['clip']} {c['frames']} frames over {c['seconds']} s, stride {c['stride_m']} m" for c in v["clips"]))
    if not report["ok"]:
        failed.append(str(asset))
print(f"{len(assets) - len(failed)}/{len(assets)} assets passed" + (f"; failed: {', '.join(failed)}" if failed else ""))
sys.exit(1 if failed else 0)
