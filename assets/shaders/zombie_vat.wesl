// Zombie crowd shader: vertex animation texture (VAT) + per-instance MeshTag.
// See crates/client/src/zombie_render.rs for the texture layout and tag packing.

import bevy_pbr::render::{
    mesh_functions,
    view_transformations::position_world_to_clip,
    mesh_view_bindings::{globals, view},
};

struct VatSettings {
    clips: array<vec4<f32>, 16>,
    sun: vec4<f32>,
    sun_color: vec4<f32>,
    ambient: vec4<f32>,
};

@group(constants::MATERIAL_BIND_GROUP) @binding(0) var vat: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(1) var<uniform> settings: VatSettings;
// A baked character's base colour; 1 x 1 white for the grey-box.
@group(constants::MATERIAL_BIND_GROUP) @binding(2) var base_color: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(3) var base_sampler: sampler;

struct Vertex {
    @builtin(instance_index) instance_index: u32,
    @location(0) position: vec3<f32>,
    @location(1) vat_info: vec2<f32>,
    @location(2) uv: vec2<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_position: vec3<f32>,
    // The grey-box's body-part colour, or a baked character's outfit tint (times its texture).
    @location(1) color: vec3<f32>,
    @location(2) uv: vec2<f32>,
    // 1 while just hit.
    @location(3) flash: f32,
};

fn hash(n: u32) -> f32 {
    var x = n * 747796405u + 2891336453u;
    x = ((x >> ((x >> 28u) + 4u)) ^ x) * 277803737u;
    return f32((x >> 22u) ^ x) / 4294967295.0;
}

// Muted everyday clothing. All linear RGB.
fn shirt_color(o: u32) -> vec3<f32> {
    let palette = array<vec3<f32>, 8>(
        vec3(0.30, 0.08, 0.07), vec3(0.10, 0.16, 0.30), vec3(0.32, 0.32, 0.30), vec3(0.08, 0.20, 0.10),
        vec3(0.45, 0.40, 0.28), vec3(0.06, 0.06, 0.07), vec3(0.42, 0.20, 0.08), vec3(0.20, 0.10, 0.25),
    );
    return palette[o % 8u];
}

fn pants_color(o: u32) -> vec3<f32> {
    let palette = array<vec3<f32>, 4>(
        vec3(0.07, 0.11, 0.22), vec3(0.28, 0.24, 0.15), vec3(0.05, 0.05, 0.05), vec3(0.16, 0.16, 0.17),
    );
    return palette[(o >> 3u) % 4u];
}

// A baked character's outfit variety: mild washes of colour and brightness over its texture, so a
// crowd of one model doesn't look cloned.
fn outfit_tint(o: u32) -> vec3<f32> {
    let palette = array<vec3<f32>, 8>(
        vec3(1.0, 1.0, 1.0), vec3(1.10, 0.95, 0.86), vec3(0.86, 0.95, 1.10), vec3(0.94, 1.06, 0.90),
        vec3(0.92, 0.92, 0.92), vec3(1.06, 0.90, 0.94), vec3(0.96, 1.0, 1.06), vec3(1.10, 1.02, 0.84),
    );
    return palette[o % 8u] * (0.82 + 0.3 * hash(o + 5u));
}

fn vat_load(vid: u32, clip: vec4<f32>, frame: u32) -> vec3<f32> {
    return textureLoad(vat, vec2<u32>(vid, u32(clip.x) + frame), 0).xyz;
}

// Vertex `vid` of a looping clip at `cycle` in [0, 1), between its two nearest frames.
fn sample_cycle(clip: vec4<f32>, cycle: f32, vid: u32) -> vec3<f32> {
    let frames = u32(clip.y);
    let f = cycle * clip.y;
    let f0 = u32(floor(f)) % frames;
    let f1 = (f0 + 1u) % frames;
    return mix(vat_load(vid, clip, f0), vat_load(vid, clip, f1), fract(f));
}

// Vertex `vid` of looping clip `index` now: at a fixed rate, or (w > 0) at speed / stride.
fn sample_loop(index: u32, phase: f32, speed: f32, vid: u32) -> vec3<f32> {
    let clip = settings.clips[index];
    var rate = clip.z;
    if (clip.w > 0.0) {
        rate = speed / clip.w;
    }
    return sample_cycle(clip, fract(globals.time * rate + phase), vid);
}

@vertex
fn vertex(vertex: Vertex) -> VertexOutput {
    let tag = mesh_functions::get_tag(vertex.instance_index);
    // Bits 0-3 clip, 4-9 outfit, 10 direct, 27 hit flash; then (looping) 11-16 phase, 17-24 speed,
    // 25-26 the looping clip it's fading from and 28-31 how much of it still shows, or (direct)
    // 11-26 the clip position chosen by the client (see pack_tag in zombie_render.rs).
    let clip_index = tag & 15u;
    let outfit = (tag >> 4u) & 63u;
    let direct = ((tag >> 10u) & 1u) == 1u;
    let flash = ((tag >> 27u) & 1u) == 1u;
    let vid = u32(vertex.vat_info.x);

    let clip = settings.clips[clip_index];
    var local = vec3(0.0);
    if (direct && clip.w < 0.0) {
        // One-shot: 0..1 spans the first to the last frame, no wrapping.
        let frames = u32(clip.y);
        let pos = f32((tag >> 11u) & 65535u) / 65535.0 * (clip.y - 1.0);
        let f0 = min(u32(floor(pos)), frames - 1u);
        let f1 = min(f0 + 1u, frames - 1u);
        local = mix(vat_load(vid, clip, f0), vat_load(vid, clip, f1), fract(pos));
    } else if (direct) {
        local = sample_cycle(clip, f32((tag >> 11u) & 65535u) / 65535.0, vid);
    } else {
        let phase = f32((tag >> 11u) & 63u) / 64.0;
        let speed = f32((tag >> 17u) & 255u) / 48.0;
        local = sample_loop(clip_index, phase, speed, vid);
        // Crossfade from the looping clip it was playing a moment ago.
        let fade = f32((tag >> 28u) & 15u) / 15.0;
        if (fade > 0.0) {
            local = mix(local, sample_loop((tag >> 25u) & 3u, phase, speed, vid), fade);
        }
    }

    var out: VertexOutput;
    let world_from_local = mesh_functions::get_world_from_local(vertex.instance_index);
    let world = mesh_functions::mesh_position_local_to_world(world_from_local, vec4(local, 1.0));
    out.world_position = world.xyz;
    out.clip_position = position_world_to_clip(world.xyz);

    out.uv = vertex.uv;
    out.flash = select(0.0, 1.0, flash);
    let kind = u32(vertex.vat_info.y);
    if (kind == 4u) {
        out.color = outfit_tint(outfit);
        return out;
    }
    let skin = mix(vec3(0.28, 0.32, 0.24), vec3(0.40, 0.38, 0.30), hash(outfit));
    var color = skin;
    if (kind == 1u) {
        color = shirt_color(outfit);
        // Some shirts are blood-soaked.
        if (hash(outfit + 17u) > 0.65) {
            color = mix(color, vec3(0.18, 0.015, 0.01), 0.55);
        }
    } else if (kind == 2u) {
        color = pants_color(outfit);
    } else if (kind == 3u) {
        color = vec3(0.05, 0.035, 0.025);
    }
    out.color = color;
    return out;
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    // Flat shading from screen-space derivatives: faceted low-poly look, no normals needed.
    var n = normalize(cross(dpdx(in.world_position), dpdy(in.world_position)));
    if (dot(n, view.world_position - in.world_position) < 0.0) {
        n = -n;
    }
    let diffuse = max(dot(n, normalize(settings.sun.xyz)), 0.0) * settings.sun.w;
    let sky = 0.5 + 0.5 * n.y;
    // Always sampled (uniform control flow); the grey-box's texture is white.
    var base = textureSample(base_color, base_sampler, in.uv).rgb * in.color;
    // Just hit: a brief pale flash so the blow reads.
    base = mix(base, vec3(1.0, 0.82, 0.75), 0.7 * in.flash);
    let lit = base * (settings.ambient.rgb * sky * 1.4 + settings.sun_color.rgb * 1.5 * diffuse);
    return vec4(lit, 1.0);
}
