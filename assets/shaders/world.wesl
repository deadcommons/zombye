// World material: StandardMaterial lighting + fog of war + camera-side cutaway (only in
// materials built with CUTAWAY; see crates/client/src/materials.rs).
// See crates/client/src/materials.rs.

import bevy_pbr::render::{
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::alpha_discard,
    view_transformations::{position_world_to_ndc, ndc_to_frag_coord},
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
    tree_count: vec4<f32>,
    lights: vec4<f32>,
    sight: array<vec4<f32>, 3>,
    trees: array<vec4<f32>, 32>,
};

@group(constants::MATERIAL_BIND_GROUP) @binding(100) var<uniform> world_params: WorldParams;
@group(constants::MATERIAL_BIND_GROUP) @binding(101) var fog_tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(102) var fog_sampler: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(103) var rays_tex: texture_2d<f32>;

// 4×4 ordered dither threshold in [0, 1).
fn bayer4(p: vec2<u32>) -> f32 {
    let m = array<f32, 16>(0.0, 8.0, 2.0, 10.0, 12.0, 4.0, 14.0, 6.0, 3.0, 11.0, 1.0, 9.0, 15.0, 7.0, 13.0, 5.0);
    return (m[(p.y % 4u) * 4u + (p.x % 4u)] + 0.5) / 16.0;
}

const STUB_HEIGHT: f32 = 0.4;
// How bright a lit window is at night (HDR, before the tone map).
const LIT_GLOW: f32 = 1.8;
// At night the palest things on the ground (road paint, kerb tops) keep this much of their colour
// as if catching the moon, so the street's shapes still read outside the player's light (#81).
const PAINT_NIGHT: f32 = 0.05;
// In unseen areas, surfaces keep (albedo / UNSEEN_MID)^UNSEEN_CONTRAST of their difference in
// lightness.
const UNSEEN_MID: f32 = 0.18;
const UNSEEN_CONTRAST: f32 = 0.6;
// Above the ground-floor walls: roofs and upper storeys, which open a hole round the player
// instead of dropping (the whole of them goes only while you're inside that building, on the CPU).
const UPPER_Y: f32 = 3.05;
// The hole round the player on screen, in metres: half its width, and how far it reaches below
// and above the chest (feet to a little over the head).
const HOLE_HALF_W: f32 = 1.0;
const HOLE_DOWN: f32 = 1.3;
const HOLE_UP: f32 = 1.1;

// 0 outside the see-through hole round the player, 1 in the middle of it, for a fragment at `wp`
// (pixel `frag`): only things nearer the camera than the player open up.
fn hole_at(wp: vec3<f32>, frag: vec2<f32>) -> f32 {
    let chest = world_params.player.xyz;
    if (dot(wp - chest, world_params.camera_dir.xyz) < 0.5) {
        return 0.0;
    }
    let centre = ndc_to_frag_coord(position_world_to_ndc(chest).xy);
    let up = ndc_to_frag_coord(position_world_to_ndc(chest + vec3(0.0, 1.0, 0.0)).xy);
    let metre = max(length(up - centre), 1e-3);
    // In metres on screen from the chest, y up.
    let o = vec2(frag.x - centre.x, centre.y - frag.y) / metre;
    let reach = select(HOLE_DOWN, HOLE_UP, o.y > 0.0);
    let d = length(vec2(o.x / HOLE_HALF_W, o.y / reach));
    return 1.0 - smoothstep(0.75, 1.0, d);
}

fn ground_camera_dir() -> vec2<f32> {
    let c = world_params.camera_dir.xz;
    return c / max(length(c), 1e-4);
}

// How far behind a wall (away from the camera, on the ground) it can hide things: a 3 m wall seen
// from 30° up covers about 5.2 m of floor behind it.
const WALL_REACH: f32 = 5.5;

// Does the ground segment from `p` to `p - c * WALL_REACH` pass through the rectangle (min, max)?
fn behind_hits(p: vec2<f32>, c: vec2<f32>, lo: vec2<f32>, hi: vec2<f32>) -> bool {
    let d = -c * WALL_REACH;
    var t0 = 0.0;
    var t1 = 1.0;
    for (var i = 0; i < 2; i++) {
        if (abs(d[i]) < 1e-5) {
            if (p[i] < lo[i] || p[i] > hi[i]) {
                return false;
            }
        } else {
            let a = (lo[i] - p[i]) / d[i];
            let b = (hi[i] - p[i]) / d[i];
            t0 = max(t0, min(a, b));
            t1 = min(t1, max(a, b));
        }
    }
    return t0 <= t1;
}

// The square the player stands in, for the outdoor wall rule: half a side in metres.
const BODY_HALF: f32 = 0.3;

// Is the wall piece with this midpoint (pieces are 1 m long, along x or z) cut down to a stub?
// Only if it hides something that matters: the floor behind it on screen runs into the player's
// room (indoors) or the square the player stands in (outdoors). So the room's far walls, the rest
// of the building, and walls beside you stand. Must match `materials::wall_is_cut`.
fn wall_cut(mid: vec2<f32>) -> bool {
    let c = ground_camera_dir();
    let r = world_params.inside_rect;
    var lo = world_params.player.xz - vec2(BODY_HALF);
    var hi = world_params.player.xz + vec2(BODY_HALF);
    if (r.x <= r.z) {
        lo = r.xy + vec2(0.03);
        hi = r.zw - vec2(0.03);
    }
    // A piece along x has its midpoint half-way along a tile.
    var half = vec2(0.0, 0.45);
    if (abs(fract(mid.x) - 0.5) < 0.25) {
        half = vec2(0.45, 0.0);
    }
    return behind_hits(mid, c, lo, hi) || behind_hits(mid - half, c, lo, hi) || behind_hits(mid + half, c, lo, hi);
}

// How far the foliage of the tree whose trunk foot is `anchor` has faded (0 shown, 1 gone).
fn tree_fade(anchor: vec2<f32>) -> f32 {
    let n = u32(world_params.tree_count.x + 0.5);
    for (var i = 0u; i < n; i++) {
        let t = world_params.trees[i];
        if (all(abs(t.xy - anchor) < vec2(0.01))) {
            let f = t.z;
            return f * f * (3.0 - 2.0 * f);
        }
    }
    return 0.0;
}

// Fog mask at a ground point: x = how visible, y = indoors.
fn fog_sample(xz: vec2<f32>) -> vec2<f32> {
    let uv = (xz - world_params.fog_rect.xy) / world_params.fog_rect.z;
    if (any(uv <= vec2(0.0)) || any(uv >= vec2(1.0))) {
        return vec2(0.0);
    }
    return textureSampleLevel(fog_tex, fog_sampler, uv, 0.0).rg;
}

// How visible a ground point is, worked out here per pixel from the sight rays (crates/client/src/
// fog.rs: the same rule as its mask, so edges are smooth lines instead of the mask's half-metre steps).
fn fog_at(xz: vec2<f32>) -> f32 {
    let s0 = world_params.sight[0];
    let s1 = world_params.sight[1];
    let s2 = world_params.sight[2];
    if (s2.w < 0.5) {
        return fog_sample(xz).x;
    }
    let d = xz - s0.xy;
    let dist = length(d);
    let dir = d / max(dist, 1e-4);
    // Along a flashlight's beam the sight range is the beam's reach.
    var range_m = s0.z;
    if (s0.w > s0.z && dist > 0.1) {
        range_m = mix(s0.z, s0.w, smoothstep(s2.x - 0.04, s2.x + 0.04, dot(s1.xy, dir)));
    }
    if (dist > range_m + 1.5) {
        return 0.0;
    }
    let n = textureDimensions(rays_tex).x;
    let a = fract(atan2(d.y, d.x) / 6.2831853) * f32(n);
    let i0 = u32(floor(a)) % n;
    let i1 = (i0 + 1u) % n;
    // The nearer of the two rays either side, so light doesn't leak past corners.
    let reach = min(textureLoad(rays_tex, vec2<u32>(i0, 0u), 0).r, textureLoad(rays_tex, vec2<u32>(i1, 0u), 0).r);
    let los = 1.0 - smoothstep(reach + 0.3, reach + 1.1, dist);
    let range = 1.0 - smoothstep(range_m * 0.5, range_m, dist);
    var cone = 1.0;
    if (dist >= s2.z) {
        cone = s2.y + (1.0 - s2.y) * smoothstep(s1.z, s1.w, dot(s1.xy, dir));
    }
    return los * range * cone;
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
    var wood_fade = 0.0;
@if(CUTAWAY && VERTEX_UVS_A && VERTEX_UVS_B) {
    let cut_kind = u32(in.uv_b.x + 0.5);
    if (cut_kind == 1u) {
        if (wp.y > STUB_HEIGHT && wp.y < UPPER_Y && wall_cut(in.uv)) {
            discard;
        }
    } else if (cut_kind == 4u) {
        if (!wall_cut(in.uv)) {
            discard;
        }
    } else if (cut_kind == 3u) {
        // A tree's foliage dithers away while the tree hides the player (materials.rs).
        let fade = tree_fade(in.uv);
        if (fade > 0.0 && 1.0 - fade < bayer4(vec2<u32>(in.position.xy))) {
            discard;
        }
    } else if (cut_kind == 6u) {
        // Its trunk and branches stay, thinned to every other pixel and darkened (below).
        wood_fade = tree_fade(in.uv);
        if (wood_fade > 0.0 && bayer4(vec2<u32>(in.position.xy)) < 0.5 * wood_fade) {
            discard;
        }
    }
    // Roofs and upper storeys (not trees) between the camera and the player: a soft hole round them.
    // Roofs open it however low they are (porch roofs, awnings: building_kit.rs).
    if ((wp.y > UPPER_Y || cut_kind == 2u) && cut_kind != 3u && cut_kind != 6u) {
        let hole = hole_at(wp, in.position.xy);
        if (hole > 0.0 && hole > bayer4(vec2<u32>(in.position.xy))) {
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
    var seen = fog_at(at);
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
    // Greying flattens pale and dark surfaces toward one tone (a driveway vanishes into its lawn):
    // keep some of the difference in what they're made of (#81).
    let albedo_lum = dot(pbr_input.material.base_color.rgb, vec3(0.299, 0.587, 0.114));
    unseen *= clamp(pow(albedo_lum / UNSEEN_MID, UNSEEN_CONTRAST), 0.6, 1.5);
    // Rooms you can't see into are near-black, as if unlit, so their contents stay hidden. Walls,
    // windows and doors stay readable (just dimmed like everything else unseen), so you can still
    // tell a building's layout and where its ways in are.
    var hide_contents = smoothstep(0.3, 0.7, indoors);
    if (piece_kind == 1u || piece_kind == 4u || piece_kind == 5u || piece_kind == 7u) {
        hide_contents *= 0.25;
    }
    unseen = mix(unseen, out.color.rgb * vec3(0.03, 0.03, 0.04), hide_contents);
    out.color = vec4(mix(unseen, out.color.rgb, seen), out.color.a);
    // A window with a light on behind it glows warm at night, seen or not (#59): a light source,
    // so bright enough to bloom a little, and the glass goes nearly opaque.
    if (piece_kind == 7u) {
        let night = world_params.lights.x;
        let glow = vec3(1.0, 0.36, 0.07) * LIT_GLOW * night * mix(0.7, 1.0, seen);
        out.color = vec4(mix(out.color.rgb, glow, 0.85 * night), max(out.color.a, night));
    }
    out.color = vec4(out.color.rgb * (1.0 - 0.55 * wood_fade), out.color.a);
    if (wp.y < 0.1 && n.y > 0.9) {
        let albedo = pbr_input.material.base_color.rgb;
        let pale = smoothstep(0.3, 0.5, dot(albedo, vec3(0.299, 0.587, 0.114)));
        let moon = albedo * pale * world_params.lights.x * PAINT_NIGHT * mix(0.4, 1.0, seen);
        out.color = vec4(out.color.rgb + moon, out.color.a);
    }

    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
}
    return out;
}
