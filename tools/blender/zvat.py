"""Bakes a skinned character's actions into a vertex animation texture (VAT) for the crowd renderer,
run inside Blender (5.x) on a cleaned character (`zchar`). Any rig, any number of bones: each frame is
just the deformed vertex positions, which is what `crates/client/src/zombie_render.rs` reads (it bakes
its procedural grey-box zombie the same way at startup). See `.claude/skills/meshy-assets/SKILL.md`.

    import zvat
    info = zvat.bake("<out>/zombie_01.zvat", clips=[("walk", 24, "loop"), ("idle", 24, "loop")])

File (little-endian), version 1:
    b"ZVAT", u32 version, u32 vertices V, u32 indices I, u32 clips C
    C × clip: 16-byte name (UTF-8, zero padded), u32 frames, f32 seconds per play,
              f32 stride (metres per loop at walking speed, 0 = fixed rate), u32 one_shot (0/1)
    V × 2 f32 UV (glTF convention: v down), I × u32 triangle indices
    per clip, per frame, V × 3 f32 positions
Positions are game model space: metres, Y up, facing +Z, feet at y = 0 (Blender x, z, -y). Vertices
are split where UVs differ, like a glTF export, so each one has a single UV.
"""

import math
import struct
from pathlib import Path

import bpy

import zchar

VERSION = 1


def _split_vertices(me):
    """One output vertex per (mesh vertex, UV) pair; returns (source vertex per output vertex, UVs,
    triangle indices)."""
    me.calc_loop_triangles()
    uv = me.uv_layers.active.data
    key_to_out, src, uvs, idx = {}, [], [], []
    for tri in me.loop_triangles:
        for li in tri.loops:
            vi = me.loops[li].vertex_index
            u, v = uv[li].uv
            key = (vi, round(u, 6), round(v, 6))
            out = key_to_out.get(key)
            if out is None:
                out = key_to_out[key] = len(src)
                src.append(vi)
                uvs.append((u, 1.0 - v))
            idx.append(out)
    return src, uvs, idx


def _stride(arm, action, f0, cycle, fps):
    """Metres the body would cover per loop of an in-place walk: while a foot is planted (in its lowest
    fifth) it slides backward at the walking speed."""
    pb = arm.pose.bones
    feet = [b for b in pb if "foot" in b.name.lower()]
    if not feet:
        return 0.0
    ad = arm.animation_data
    ad.action = bpy.data.actions[action]
    speeds = []
    for foot in feet:
        track = []
        for k in range(int(cycle) + 1):
            bpy.context.scene.frame_set(int(f0) + k)
            track.append((arm.matrix_world @ foot.head).copy())
        zmin = min(p.z for p in track)
        zmax = max(p.z for p in track)
        for a, b in zip(track, track[1:]):
            if a.z < zmin + (zmax - zmin) * 0.2 and b.z < zmin + (zmax - zmin) * 0.2:
                speeds.append((b.y - a.y) * fps)  # planted foot slides toward +Y (backward) as the body walks south
    if not speeds:
        return 0.0
    speeds.sort()
    stride = max(0.0, speeds[len(speeds) // 2]) * cycle / fps
    return stride if stride > 0.05 else 0.0  # feet shifting in an idle: a fixed rate


def bake(out_path, clips):
    """Bakes `clips` [(action, frames, "loop" | "once")] of the Asset collection's character. A looping
    clip samples one cycle (`zchar.loop_frames`) at `frames` evenly spaced times, the last one a step
    before the first again; a one-shot spans its whole range, first to last frame. Returns a summary."""
    arms, meshes = zchar.character_objects()
    arm, obj = arms[0], meshes[0]
    scene = bpy.context.scene
    fps = scene.render.fps / scene.render.fps_base
    src, uvs, idx = _split_vertices(obj.data)
    ad = arm.animation_data or arm.animation_data_create()
    for t in ad.nla_tracks:
        t.mute = True
    saved = ad.action
    header, frames_data, summary = [], [], []
    for name, frames, mode in clips:
        action = bpy.data.actions[name]
        f0, f1 = action.frame_range
        if mode == "loop":
            cycle, err = zchar.loop_frames(arm, name)
            span, times = cycle, [f0 + cycle * k / frames for k in range(frames)]
            stride = _stride(arm, name, f0, cycle, fps)
        else:
            span, err, stride = f1 - f0, 0.0, 0.0
            times = [f0 + span * k / max(frames - 1, 1) for k in range(frames)]
        ad.action = action
        dg = bpy.context.evaluated_depsgraph_get()
        for t in times:
            scene.frame_set(int(math.floor(t)), subframe=t - math.floor(t))
            ev = obj.evaluated_get(dg)
            me = ev.to_mesh()
            mw = ev.matrix_world
            co = [mw @ v.co for v in me.vertices]
            frames_data.append(b"".join(struct.pack("<3f", co[i].x, co[i].z, -co[i].y) for i in src))
            ev.to_mesh_clear()
        secs = span / fps
        header.append(struct.pack("<16sIffI", name.encode()[:16], frames, secs, stride, 0 if mode == "loop" else 1))
        summary.append({"clip": name, "frames": frames, "seconds": round(secs, 3), "stride_m": round(stride, 3),
                        "source_frames": [f0, f1], "cycle_frames": round(span, 2), "loop_error_m": round(err, 4)})
    ad.action = saved
    scene.frame_set(0)
    out_path = Path(out_path)
    out_path.parent.mkdir(parents=True, exist_ok=True)
    with open(out_path, "wb") as f:
        f.write(b"ZVAT" + struct.pack("<4I", VERSION, len(src), len(idx), len(clips)))
        f.write(b"".join(header))
        f.write(b"".join(struct.pack("<2f", *uv) for uv in uvs))
        f.write(struct.pack(f"<{len(idx)}I", *idx))
        f.write(b"".join(frames_data))
    rows = sum(c[1] for c in clips)
    return {"path": str(out_path), "vertices": len(src), "triangles": len(idx) // 3, "rows": rows,
            "bytes": out_path.stat().st_size, "texture_mb_rgba32f": round(len(src) * rows * 16 / 2**20, 2),
            "clips": summary}
