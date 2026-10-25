"""Where an asset's files live, and moving finished assets from the art repo into the game repo.
Plain Python (no Blender), so it runs from a shell:

    python3 tools/blender/artrepo.py start furniture/lamp     # make its work folder in the art repo
    python3 tools/blender/artrepo.py status [furniture/lamp]  # where each asset stands
    python3 tools/blender/artrepo.py promote furniture/lamp   # copy a ready asset into the game repo
    python3 tools/blender/artrepo.py promote characters/survivor_01 --wearables hoodie,glasses   # only the approved wearables
    python3 tools/blender/artrepo.py record <commit> furniture/lamp   # after committing it there

Work in progress lives in the private art repo (deadcommons/zombye-art, `$ZOMBYE_ART`, default
~/games/zombye-art), one folder per asset: `<kind>/<category>-<id>/` (e.g. props/furniture-bed/)
holding meta.yml, `<id>.ron` (spec), `<id>.py` (build script), `<id>.blend`, renders/ and export/.
Only a finished asset is promoted into this (public) repo: specs in assets/specs/<category>/, the
script, .blend and review sheet in assets/source/<category>/<id>/, the GLB in assets/models/<category>/,
and an ATTRIBUTION.md row. Without the art repo (or before an asset has a folder there) the tools
work on the game repo's copy directly.

Characters (`characters/survivor_01`) live in the art repo's `characters/<id with dashes>/` (e.g.
characters/survivor-01/), files named after the folder (survivor-01.ron, .py, .blend). A modular one
(zgarment.py, #63) promotes its body export (`export/<id>_body.glb`) as `assets/models/characters/<id>.glb`,
every wearable in `export/wearables/` into `assets/models/wearables/`, its textures beside the source, and
an ATTRIBUTION.md row per file with the generator its GLB records (`ai_generated`, `generator`, task ids).
Its review sheet is the in-game check (`renders/ingame*.png`, the newest).
"""

import json
import struct

import datetime
import os
import re
import shutil
import sys
from pathlib import Path

GAME = Path(__file__).resolve().parents[2]
# Game asset category -> the art repo's top-level folder.
ART_KINDS = {"furniture": "props", "street": "props"}
STATUSES = ("wip", "review", "ready", "promoted")
META_FIELDS = ("name", "status", "tool", "plan_owns_output", "generated", "prompt", "author", "notes", "promoted")


def art_root():
    """The art repo checkout, or None when it isn't here (public contributors don't have it)."""
    p = Path(os.environ.get("ZOMBYE_ART") or Path.home() / "games" / "zombye-art").expanduser()
    return p if (p / "PROMOTED.md").exists() else None


def split(asset):
    category, name = asset.split("/")
    return category, name


def is_character(asset):
    return split(asset)[0] == "characters"


def art_dir(asset):
    root = art_root()
    if root is None:
        return None
    category, name = split(asset)
    if is_character(asset):
        return root / "characters" / name.replace("_", "-")
    return root / ART_KINDS.get(category, category) / f"{category}-{name.replace('_', '-')}"


def game_paths(asset):
    category, name = split(asset)
    src = GAME / "assets" / "source" / category / name
    return {
        "where": "game",
        "spec": GAME / "assets" / "specs" / category / f"{name}.ron",
        "script": src / f"{name}.py",
        "blend": src / f"{name}.blend",
        "sheet": src / f"{name}_iso.png",
        "glb": GAME / "assets" / "models" / category / f"{name}.glb",
    }


def art_paths(asset):
    d = art_dir(asset)
    _, name = split(asset)
    if is_character(asset):
        stem = d.name
        ingame = sorted((d / "renders").glob("ingame*.png"))
        return {
            "where": "art",
            "dir": d,
            "meta": d / "meta.yml",
            "spec": d / f"{stem}.ron",
            "script": d / f"{stem}.py",
            "blend": d / f"{stem}.blend",
            "renders": d / "renders",
            "sheet": ingame[-1] if ingame else d / "renders" / "ingame.png",
            "glb": d / "export" / f"{name}_body.glb",
            "wearables": d / "export" / "wearables",
            "textures": d / "textures",
        }
    return {
        "where": "art",
        "dir": d,
        "meta": d / "meta.yml",
        "spec": d / f"{name}.ron",
        "script": d / f"{name}.py",
        "blend": d / f"{name}.blend",
        "renders": d / "renders",
        "sheet": d / "renders" / f"{name}_iso.png",
        "glb": d / "export" / f"{name}.glb",
    }


def paths(asset):
    """The art repo's work folder when the asset has one, else the game repo's copy."""
    d = art_dir(asset)
    return art_paths(asset) if d is not None and d.is_dir() else game_paths(asset)


def list_assets(category):
    """Every asset of a category in either repo, sorted."""
    names = {p.stem for p in (GAME / "assets" / "specs" / category).glob("*.ron")}
    root = art_root()
    if root is not None:
        for d in (root / ART_KINDS.get(category, category)).glob(f"{category}-*"):
            names |= {p.stem for p in d.glob("*.ron")}
    return [f"{category}/{n}" for n in sorted(names)]


# ---------------------------------------------------------------- meta.yml (flat key: value)

def read_meta(path):
    meta = {}
    for line in Path(path).read_text().splitlines():
        m = re.match(r"^([a-z_]+):\s*(.*?)\s*(#.*)?$", line)
        if m:
            v = m.group(2)
            meta[m.group(1)] = v[1:-1] if len(v) >= 2 and v[0] == v[-1] == '"' else v
    return meta


def set_meta(path, **values):
    """Rewrites the given top-level fields in place, keeping the rest of the file as it is."""
    text = Path(path).read_text()
    for k, v in values.items():
        line = f"{k}: {v}"
        text, n = re.subn(rf"(?m)^{k}:.*$", line, text)
        if n == 0:
            text = text.rstrip("\n") + f"\n{line}\n"
    Path(path).write_text(text)


def meta_text(name, status, prompt, notes="", promoted="", generated=None):
    return (
        f"name: {name}\n"
        f"status: {status}            # wip → review → ready → promoted\n"
        f"tool: Blender + agent      # built in Blender over the Blender MCP from the build script beside it\n"
        f"plan_owns_output: yes\n"
        f"generated: {generated or datetime.date.today().isoformat()}\n"
        f"prompt: \"{prompt}\"\n"
        f"author: Dead Commons\n"
        f"notes: \"{notes}\"\n"
        f"promoted: {promoted}\n"
    )


# ---------------------------------------------------------------- commands

def start(asset):
    """Makes the asset's work folder in the art repo. An asset already in the game repo is copied
    in (spec, script, .blend, sheet, GLB) to be reworked there."""
    if art_root() is None:
        raise SystemExit("no art repo (set ZOMBYE_ART or clone deadcommons/zombye-art to ~/games/zombye-art)")
    a = art_paths(asset)
    if a["dir"].exists():
        raise SystemExit(f"{a['dir']} already exists")
    g = game_paths(asset)
    _, name = split(asset)
    (a["dir"] / "renders").mkdir(parents=True)
    (a["dir"] / "export").mkdir()
    copied = []
    for key in ("spec", "script", "blend", "sheet", "glb"):
        if g[key].exists():
            shutil.copy2(g[key], a[key])
            copied.append(key)
    a["meta"].write_text(meta_text(a["dir"].name, "wip", f"Spec {name}.ron (our own words)",
                                   notes="reworking the promoted asset" if copied else ""))
    print(f"{a['dir']}: meta.yml" + (f", copied {', '.join(copied)} from the game repo" if copied else
                                     f"; write {name}.ron (spec) and {name}.py (build script) there"))


def check_ready(asset, force=False):
    """Why the asset can't be promoted yet (empty when it can)."""
    a = art_paths(asset)
    problems = []
    if not a["dir"].is_dir():
        return [f"no work folder {a['dir']}"]
    meta = read_meta(a["meta"]) if a["meta"].exists() else {}
    if meta.get("status") != "ready" and not force:
        problems.append(f"status is {meta.get('status')!r}, not 'ready'")
    for k in ("name", "tool", "generated", "prompt", "author"):
        if not meta.get(k):
            problems.append(f"meta.yml has no {k}")
    if meta.get("plan_owns_output") != "yes":
        problems.append("meta.yml: plan_owns_output isn't yes (only tools and plans that give us ownership)")
    for key in ("spec", "script", "blend", "sheet", "glb"):
        if not a[key].exists():
            problems.append(f"missing {a[key].relative_to(a['dir'])}")
    if not list(a["renders"].glob("ingame*.png")):
        problems.append("no in-game check in renders/ (ingame*.png: --render-to or the MCP screenshot tool)")
    return problems


def attribution_row(asset, name):
    category, ident = split(asset)
    return (f"| `assets/models/{category}/{ident}.glb` ({name}; source, script and renders in "
            f"`assets/source/{category}/{ident}/`) | Dead Commons | Built in Blender 5.2 over MCP by an agent "
            f"(Claude) from the committed script; no generator, no third-party parts | CC BY-NC-SA 4.0 | "
            f"Spec `assets/specs/{category}/{ident}.ron`; palette colours only |")


def glb_extras(path):
    """The extras of a GLB's skinned mesh node (or its first node with extras)."""
    data = Path(path).read_bytes()
    doc = json.loads(data[20:20 + struct.unpack_from("<I", data, 12)[0]])
    nodes = doc.get("nodes", [])
    node = next((n for n in nodes if "mesh" in n and "skin" in n and n.get("extras")), None) or next((n for n in nodes if n.get("extras")), {})
    return node.get("extras", {})


def character_row(model, source, name, extras):
    """An ATTRIBUTION.md row for one of a character's files, from what its GLB records."""
    generator = extras.get("generator") or "Built in Blender by an agent (Claude)"
    tasks = f"; Meshy tasks {extras['meshy_tasks']}" if extras.get("meshy_tasks") else ""
    if extras.get("ai_generated") and extras.get("meshy_tasks"):
        ai = "AI-generated (Meshy (Pro), paid plan: we own the output), cleaned up in Blender"
    elif extras.get("ai_generated"):
        ai = "Built in Blender over the AI-generated body (its row above); no generator of its own"
    else:
        ai = "no generator"
    return (f"| `{model}` ({name}; source, script and renders in `{source}`) | Dead Commons | {generator}{tasks} | "
            f"CC BY-NC-SA 4.0 | {ai} |")


def promote_character(asset, force=False, only=None):
    """A modular character: the body, its wearables (only those named in `only`, when given: the
    approved ones), its source, and a row each (an existing row is brought up to date)."""
    problems = check_ready(asset, force)
    a = art_paths(asset)
    if not a["wearables"].is_dir():
        problems.append("missing export/wearables/")
    if problems:
        raise SystemExit(f"{asset} isn't ready to promote:\n  " + "\n  ".join(problems))
    g = game_paths(asset)
    _, ident = split(asset)
    src = g["script"].parent
    copies = [(a["spec"], g["spec"]), (a["script"], g["script"]), (a["blend"], g["blend"]), (a["sheet"], src / f"{ident}_ingame.png"), (a["glb"], g["glb"])]
    copies += [(t, src / t.name) for t in sorted(a["textures"].glob("*")) if t.is_file()]
    wearables = sorted(w for w in a["wearables"].glob("*.glb") if only is None or w.stem in only)
    if only is not None and len(wearables) != len(only):
        raise SystemExit(f"no wearables {sorted(set(only) - {w.stem for w in wearables})} in {a['wearables']}")
    copies += [(w, GAME / "assets" / "models" / "wearables" / w.name) for w in wearables]
    for frm, to in copies:
        to.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(frm, to)
    m = re.search(r'name:\s*"([^"]+)"', a["spec"].read_text())
    source = f"assets/source/characters/{ident}/"
    rows = [(f"assets/models/characters/{ident}.glb", m.group(1) if m else ident, glb_extras(a["glb"]))]
    rows += [(f"assets/models/wearables/{w.name}", f"{w.stem} for {ident}", glb_extras(w)) for w in wearables]
    attribution = GAME / "ATTRIBUTION.md"
    text = attribution.read_text()
    for model, name, extras in rows:
        row = character_row(model, source, name, extras)
        old = re.search(rf"(?m)^\| `{re.escape(model)}`.*$", text)
        text = text.replace(old.group(0), row) if old else text.rstrip("\n") + "\n" + row + "\n"
    attribution.write_text(text)
    print(f"promoted {asset} into {GAME}:\n  " + "\n  ".join([str(to.relative_to(GAME)) for _, to in copies] + ["ATTRIBUTION.md"]))
    print(f"Commit those in the game repo (Refs the issue), then: python3 {Path(__file__).relative_to(GAME)} record <commit> {asset}")


def promote(asset, force=False, only=None):
    """Copies a ready asset into the game repo and adds its ATTRIBUTION.md row. Commit there, then
    run `record <commit>`."""
    if is_character(asset):
        return promote_character(asset, force, only)
    problems = check_ready(asset, force)
    if problems:
        raise SystemExit(f"{asset} isn't ready to promote:\n  " + "\n  ".join(problems))
    a, g = art_paths(asset), game_paths(asset)
    for key in ("spec", "script", "blend", "sheet", "glb"):
        g[key].parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(a[key], g[key])
    spec = a["spec"].read_text()
    m = re.search(r'name:\s*"([^"]+)"', spec)
    attribution = GAME / "ATTRIBUTION.md"
    text = attribution.read_text()
    category, ident = split(asset)
    if f"`assets/models/{category}/{ident}.glb`" not in text:
        row = attribution_row(asset, m.group(1) if m else ident)
        text = text.replace("| _none yet_ | | | | |", row) if "_none yet_" in text else text.rstrip("\n") + "\n" + row + "\n"
        attribution.write_text(text)
    files = [str(g[k].relative_to(GAME)) for k in ("spec", "script", "blend", "sheet", "glb")]
    print(f"promoted {asset} into {GAME}:\n  " + "\n  ".join(files + ["ATTRIBUTION.md"]))
    print(f"Commit those in the game repo (Refs the issue), then: python3 {Path(__file__).relative_to(GAME)} record <commit> {asset}")


def record(commit, asset, date=None):
    """Marks the asset promoted in its meta.yml and adds its line to PROMOTED.md."""
    a = art_paths(asset)
    set_meta(a["meta"], status="promoted", promoted=commit)
    category, ident = split(asset)
    files = f"`assets/models/{category}/{ident}.glb`, `assets/source/{category}/{ident}/`, `assets/specs/{category}/{ident}.ron`"
    if is_character(asset) and a["wearables"].is_dir():
        files += "".join(f", `assets/models/wearables/{w.name}`" for w in sorted(a["wearables"].glob("*.glb")))
    line = f"| {date or datetime.date.today().isoformat()} | {a['dir'].relative_to(art_root())} | {commit} | {files} |"
    promoted = art_root() / "PROMOTED.md"
    text = promoted.read_text()
    if "| _none yet_ | | | |" in text:
        text = text.replace("| _none yet_ | | | |", line)
    else:
        # Newest first: right under the table's header.
        text = re.sub(r"(?m)^(\|---\|.*\|\n)", lambda m0: m0.group(1) + line + "\n", text, count=1)
    promoted.write_text(text)
    print(f"recorded {asset}: promoted {commit}")


def status(assets):
    for asset in assets:
        p = paths(asset)
        if p["where"] == "art":
            meta = read_meta(p["meta"])
            extra = f" ({meta.get('promoted')})" if meta.get("promoted") else ""
            print(f"{asset}: art repo, {meta.get('status')}{extra}  {p['dir']}")
        else:
            print(f"{asset}: game repo only")


if __name__ == "__main__":
    args = sys.argv[1:]
    force = "--force" in args
    only = None
    if "--wearables" in args:
        i = args.index("--wearables")
        only = [w for w in args[i + 1].split(",") if w]
        del args[i:i + 2]
    args = [a for a in args if a != "--force"]
    if not args:
        raise SystemExit(__doc__)
    cmd, rest = args[0], args[1:]
    if cmd == "start" and len(rest) == 1:
        start(rest[0])
    elif cmd == "status":
        status([a for r in (rest or ["furniture"]) for a in (list_assets(r) if "/" not in r else [r])])
    elif cmd == "promote" and len(rest) == 1:
        promote(rest[0], force, only)
    elif cmd == "record" and len(rest) == 2:
        record(rest[0], rest[1])
    else:
        raise SystemExit(__doc__)
