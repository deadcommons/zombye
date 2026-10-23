"""Build assets from a shell, headless:

    /Applications/Blender.app/Contents/MacOS/Blender --background --factory-startup \
        --python tools/blender/build.py -- furniture/bed furniture/sofa
    ... --python tools/blender/build.py -- furniture          # every furniture asset, in either repo
    ... --python tools/blender/build.py -- --check furniture  # validate only: no renders, no files
    ... --python tools/blender/build.py -- ~/games/zombye-art/characters/zombie-01   # a character (zchar)

Each asset is built where it's being worked on: its folder in the art repo when it has one, else the
game repo's copy (tools/blender/artrepo.py). A character is given as its art-repo folder (a directory
holding `<folder name>.ron`, built by zchar); with `--check` it is validated from its saved `.blend`.
Prints each asset's validation report and exits 1 if any failed.
"""

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import zasset  # noqa: E402
import zchar  # noqa: E402

args = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
check_only = "--check" in args
args = [a for a in args if a != "--check"]
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
        if isinstance(asset, Path):
            report = zchar.build(asset, render=not check_only, export=not check_only, save=not check_only,
                                 from_blend=check_only)
        else:
            report = zasset.build(asset, render=not check_only, export=not check_only, save=not check_only)
    except Exception as e:  # a broken script fails its asset, not the run
        report = {"asset": str(asset), "ok": False, "checks": [{"check": "build script runs", "ok": False, "detail": repr(e)}]}
    print(zasset.summary(report))
    for note in report.get("notes", []):
        print(f"  note: {note}")
    for key in ("glb", "blend"):
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
