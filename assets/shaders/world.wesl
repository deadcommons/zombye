// World material: StandardMaterial lighting + fog of war + camera-side cutaway.
// See crates/client/src/materials.rs.

import bevy_pbr::render::{
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::alpha_discard,
};

@if(PREPASS_PIPELINE)
import bevy_pbr::{
    prepass::io::{VertexOutput, FragmentOutput},
    deferred::functions::deferred_output,
};
@else
import bevy_pbr::render::{
    forward_io::{VertexOutput, FragmentOutput},
    pbr_functions::{apply_pbr_lighting, main_pass_post_lighting_processing},
};

struct WorldParams {
    player: vec4<f32>,
    camera_dir: vec4<f32>,
    fog_rect: vec4<f32>,
    fog_tint: vec4<f32>,
    inside_rect: vec4<f32>,
};

@group(constants::MATERIAL_BIND_GROUP) @binding(100) var<uniform> world_params: WorldParams;
@group(constants::MATERIAL_BIND_GROUP) @binding(101) var fog_tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(102) var fog_sampler: sampler;

// 4×4 ordered dither threshold in [0, 1).
fn bayer4(p: vec2<u32>) -> f32 {
    let m = array<f32, 16>(0.0, 8.0, 2.0, 10.0, 12.0, 4.0, 14.0, 6.0, 3.0, 11.0, 1.0, 9.0, 15.0, 7.0, 13.0, 5.0);
    return (m[(p.y % 4u) * 4u + (p.x % 4u)] + 0.5) / 16.0;
}

const STUB_HEIGHT: f32 = 0.4;

fn ground_camera_dir() -> vec2<f32> {
    let c = world_params.camera_dir.xz;
    return c / max(length(c), 1e-4);
}

// Is the wall with this midpoint on the camera side of the player, close enough to hide them?
// Inside a building, every camera-side wall of that building drops.
fn wall_cut(mid: vec2<f32>) -> bool {
    let c = ground_camera_dir();
    let d = mid - world_params.player.xz;
    let along = dot(d, c);
    let perp = length(d - along * c);
    let r = world_params.inside_rect;
    if (r.x <= r.z && all(mid >= r.xy - vec2(0.1)) && all(mid <= r.zw + vec2(0.1))) {
        return along > 0.0;
    }
    // Outdoors: only walls that actually cover the character. A 3 m wall hides the player's feet
    // up to about 5 m in front of them (the camera looks down at 30°), and a little to either side.
    return along > 0.0 && along < 5.5 && perp < 1.6;
}

// Distance on the ground from `centre` to the line running from the player toward the camera.
fn occluder_distance(centre: vec2<f32>) -> f32 {
    let c = ground_camera_dir();
    let p = world_params.player.xz;
    let s = clamp(dot(centre - p, c), 0.0, 14.0);
    return length(centre - (p + c * s));
}

// Fog mask at a ground point: x = how visible, y = indoors.
fn fog_sample(xz: vec2<f32>) -> vec2<f32> {
    let uv = (xz - world_params.fog_rect.xy) / world_params.fog_rect.z;
    if (any(uv <= vec2(0.0)) || any(uv >= vec2(1.0))) {
        return vec2(0.0);
    }
    return textureSampleLevel(fog_tex, fog_sampler, uv, 0.0).rg;
}

fn fog_at(xz: vec2<f32>) -> f32 {
    return fog_sample(xz).x;
}

@fragment
fn fragment(
    in: VertexOutput,
    @builtin(front_facing) is_front: bool,
) -> FragmentOutput {
    let wp = in.world_position.xyz;

    // Cutaway (see crates/client/src/materials.rs, `wall_is_cut`): every vertex of a
    // piece carries the same anchor in UV0 and its kind in UV1, so a whole wall drops to a stub at
    // once, with a crisp, grid-aligned top edge.
@if(VERTEX_UVS_A && VERTEX_UVS_B) {
    let cut_kind = u32(in.uv_b.x + 0.5);
    if (cut_kind == 1u) {
        if (wp.y > STUB_HEIGHT && wall_cut(in.uv)) {
            discard;
        }
    } else if (cut_kind == 2u) {
        if (occluder_distance(in.uv) < in.uv_b.y) {
            discard;
        }
    } else if (cut_kind == 4u) {
        if (!wall_cut(in.uv)) {
            discard;
        }
    } else if (cut_kind == 3u) {
        // Foliage fades softly instead.
        let keep = smoothstep(in.uv_b.y * 0.5, in.uv_b.y * 1.2, occluder_distance(in.uv));
        if (keep < bayer4(vec2<u32>(in.position.xy))) {
            discard;
        }
    }
}

    var pbr_input = pbr_input_from_standard_material(in, is_front);
    pbr_input.material.base_color = alpha_discard(
        pbr_input.material.flags,
        pbr_input.material.alpha_cutoff,
        pbr_input.material.base_color
    );

    var out: FragmentOutput;
@if(PREPASS_PIPELINE) {
    out = deferred_output(in, pbr_input);
} @else {
    out.color = apply_pbr_lighting(pbr_input);

    // Fog of war. Vertical faces sample the mask just in front of them, so a wall's outside face
    // counts as outdoors and its inside face as the room it faces.
    // (WESL `@if` blocks don't open a scope, so names here must differ from the cutaway block's.)
    var piece_kind = 0u;
@if(VERTEX_UVS_B) {
    piece_kind = u32(in.uv_b.x + 0.5);
}
    let n = normalize(in.world_normal);
    var at = wp.xz;
    if (abs(n.y) < 0.5) {
        at += normalize(n.xz) * 0.3;
    }
    let mask = fog_sample(at);
    var seen = mask.x;
    var indoors = mask.y;
    if (piece_kind == 2u) {
        indoors = 0.0;
    }
    // Roofs and the outside of upper storeys read as "seen" when the wall below them is, rather
    // than showing patches of whatever is visible through the windows underneath.
    if (wp.y > 2.95 && indoors < 0.5) {
        let to_player = world_params.player.xz - wp.xz;
        let dir = to_player / max(length(to_player), 1e-3);
        seen = max(fog_at(wp.xz + dir * 1.5), fog_at(wp.xz + dir * 3.0));
    }
    let grey = dot(out.color.rgb, vec3(0.299, 0.587, 0.114));
    var unseen = mix(vec3(grey), out.color.rgb, 0.25) * world_params.fog_tint.rgb * (1.0 - world_params.fog_tint.w);
    // Rooms you can't see into are near-black, as if unlit, so their contents stay hidden. Walls,
    // windows and doors stay readable (just dimmed like everything else unseen), so you can still
    // tell a building's layout and where its ways in are.
    var hide_contents = smoothstep(0.3, 0.7, indoors);
    if (piece_kind == 1u || piece_kind == 4u || piece_kind == 5u) {
        hide_contents *= 0.25;
    }
    unseen = mix(unseen, out.color.rgb * vec3(0.03, 0.03, 0.04), hide_contents);
    out.color = vec4(mix(unseen, out.color.rgb, seen), out.color.a);

    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
}
    return out;
}
