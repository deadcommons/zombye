// World material: StandardMaterial lighting + fog of war + camera-side cutaway (only in
// materials built with CUTAWAY; see crates/client/src/materials.rs).
// See crates/client/src/materials.rs.

import bevy_pbr::render::{
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::alpha_discard,
    mesh_functions,
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
    tree_count: vec4<f32>,
    lights: vec4<f32>,
    sight: array<vec4<f32>, 3>,
    trees: array<vec4<f32>, 32>,
};

@group(constants::MATERIAL_BIND_GROUP) @binding(100) var<storage, read> world_params: WorldParams;
@group(constants::MATERIAL_BIND_GROUP) @binding(101) var fog_tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(102) var fog_sampler: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(103) var rays_tex: texture_2d<f32>;
@if(CUTAWAY)
@group(constants::MATERIAL_BIND_GROUP) @binding(105) var cut_tex: texture_2d<f32>;

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
// At night a surface lit more brightly than LIT_LO (its colour over its albedo) starts to show
// outside what the player sees unaided, in full from LIT_HI. Full moonlight is about 0.1.
const LIT_LO: f32 = 0.16;
const LIT_HI: f32 = 0.4;
// Above the ground-floor walls: upper storeys and roofs, which never drop (the whole of them
// fades out while you're inside that building: `world_render::hide_upper_floors`).
const UPPER_Y: f32 = 3.05;

fn ground_camera_dir() -> vec2<f32> {
    let c = world_params.camera_dir.xz;
    return c / max(length(c), 1e-4);
}

// The cut texture (crates/client/src/cutaway.rs): one texel a tile, addressed by the tile's
// coordinates modulo its size; red = how far the tile's north edge is cut, green = its west edge.
const CUT_TEXELS: i32 = 128;
// Only walls this near the player (metres, along x and z) are ever cut: beyond it the texture's
// wrap would show.
const CUT_NEAR: f32 = 56.0;

// How far the wall piece with this midpoint (pieces are 1 m long, along x or z) is cut down to a
// stub: 0 standing, 1 a stub, eased. Decided on the CPU per building and per edge, never by a
// radius round the player. `Cutaway::amount` is the same.
@if(CUTAWAY)
fn wall_cut(mid: vec2<f32>) -> f32 {
    if (any(abs(mid - world_params.player.xz) > vec2(CUT_NEAR))) {
        return 0.0;
    }
    // A north edge (along x) has its midpoint half-way along a tile in x and on a tile line in z.
    let north = abs(fract(mid.x) - 0.5) < 0.25;
    var tile = vec2<i32>(i32(round(mid.x)), i32(floor(mid.y)));
    if (north) {
        tile = vec2<i32>(i32(floor(mid.x)), i32(round(mid.y)));
    }
    let v = textureLoad(cut_tex, vec2<i32>(tile.x & (CUT_TEXELS - 1), tile.y & (CUT_TEXELS - 1)), 0);
    let a = select(v.g, v.r, north);
    return a * a * (3.0 - 2.0 * a);
}

// The player's body for the fades: half its width.
const BODY_HALF: f32 = 0.3;
// The height the tallest fading detail reaches, the width of the rule's soft edge, and how much
// of a detail is left out at most.
const FADE_TOP: f32 = 2.8;
const FADE_SOFT: f32 = 0.5;
const FADE_MOST: f32 = 0.72;

// How much a building detail (a porch roof, an awning, a canopy, a post: its middle on the ground
// `anchor`, its half-width `radius`) is thinned out because it stands between the camera and the
// player: the whole of it, by where it stands, not by where on screen a pixel is. The same rule
// as `cutaway::object_fade`.
fn object_fade(anchor: vec2<f32>, radius: f32) -> f32 {
    let c = ground_camera_dir();
    let tan_e = world_params.camera_dir.y / max(length(world_params.camera_dir.xz), 1e-4);
    let d = anchor - world_params.player.xz;
    let along = dot(d, c);
    let across = abs(d.x * c.y - d.y * c.x);
    let reach = FADE_TOP / max(tan_e, 1e-3) + radius;
    let side = 1.0 - smoothstep(radius + BODY_HALF, radius + BODY_HALF + FADE_SOFT, across);
    let near = smoothstep(-radius - FADE_SOFT, -radius, along) * (1.0 - smoothstep(reach, reach + FADE_SOFT, along));
    return side * near;
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

// Is a ground point in line of sight (1) or behind something (0), by the sight rays alone: no
// range, no view cone. `line_of_sight` in fog.rs is the same.
fn los_at(xz: vec2<f32>) -> f32 {
    let d = xz - world_params.sight[0].xy;
    let n = textureDimensions(rays_tex).x;
    let a = fract(atan2(d.y, d.x) / 6.2831853) * f32(n);
    let i0 = u32(floor(a)) % n;
    let i1 = (i0 + 1u) % n;
    // The nearer of the two rays either side, so light doesn't leak past corners.
    let reach = min(textureLoad(rays_tex, vec2<u32>(i0, 0u), 0).r, textureLoad(rays_tex, vec2<u32>(i1, 0u), 0).r);
    return 1.0 - smoothstep(reach + 0.3, reach + 1.1, length(d));
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
    let los = los_at(xz);
    // The look (assets/data/fog.ron): the far end fades out, and what's behind is a little dimmer
    // than the cone ahead, easing in with distance so there is no ring round the player.
    let range = 1.0 - smoothstep(range_m * min(world_params.lights.w, 0.99), range_m, dist);
    var cone = s2.y + (1.0 - s2.y) * smoothstep(s1.z, s1.w, dot(s1.xy, dir));
    cone = mix(1.0, cone, smoothstep(s2.z, s2.z + max(world_params.lights.z, 0.01), dist));
    return los * range * cone;
}

fn hash21(p: vec2<f32>) -> f32 {
    var q = fract(vec3(p.xyx) * vec3(0.1031, 0.1030, 0.0973));
    q += dot(q, q.yzx + 33.33);
    return fract((q.x + q.y) * q.z);
}

// Smooth value noise, 0..1.
fn vnoise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    return mix(mix(hash21(i), hash21(i + vec2(1.0, 0.0)), u.x), mix(hash21(i + vec2(0.0, 1.0)), hash21(i + vec2(1.0, 1.0)), u.x), u.y);
}

// One flat tone (0..1) per triangle of a grid of `size`-metre triangles, turned so its lines
// never run along the tiles.
fn facet(p: vec2<f32>, size: f32, salt: f32) -> f32 {
    let q = vec2(p.x * 0.8829 - p.y * 0.4695, p.x * 0.4695 + p.y * 0.8829) / size;
    let f = fract(q);
    return hash21(floor(q) + vec2(salt, select(0.0, 0.37, f.x + f.y > 1.0)));
}

// The ground's colour at a point, broken up (DCM-59): broad drifts of lighter and darker, flat
// facets a metre or so across over them, a finer set inside those, and on grass and dirt (not
// asphalt or concrete, which have no green over blue) patches gone dry and yellow.
fn ground_detail(p: vec2<f32>, c: vec3<f32>) -> vec3<f32> {
    let broad = vnoise(p * 0.09 + vec2(3.7, 9.2));
    let mid = vnoise(p * 0.31 + vec2(41.0, 17.0));
    let tone = (broad - 0.5) * 0.16 + (mid - 0.5) * 0.10 + (facet(p, 1.7, 0.0) - 0.5) * 0.11 + (facet(p, 0.55, 5.0) - 0.5) * 0.06;
    let soft = clamp((c.g - c.b) * 14.0, 0.0, 1.0);
    let dry = smoothstep(0.52, 0.78, broad * 0.55 + mid * 0.45) * soft;
    return c * (1.0 + tone * (0.6 + 0.4 * soft)) * mix(vec3(1.0), vec3(1.16, 1.03, 0.78), dry * 0.55);
}

@fragment
fn fragment(
    in: VertexOutput,
    @builtin(front_facing) is_front: bool,
) -> FragmentOutput {
    let wp = in.world_position.xyz;

    // Cutaway (see crates/client/src/cutaway.rs): every vertex of a piece carries the same anchor
    // in UV0 and its kind in UV1, so a whole wall drops to a stub as one piece, with a crisp,
    // grid-aligned top edge.
    var wood_fade = 0.0;
@if(CUTAWAY && VERTEX_UVS_A && VERTEX_UVS_B) {
    let cut_kind = u32(in.uv_b.x + 0.5);
    if (cut_kind == 1u) {
        // The wall sinks to its stub: everything above its top (a straight line) is left out.
        // Only the walls of the storey the player is on (player.w: its floor's height; stairs).
        let rel = wp.y - world_params.player.w;
        if (rel < UPPER_Y && (world_params.player.w == 0.0 || rel > 0.02)) {
            let cut = wall_cut(in.uv);
            if (cut > 0.0 && rel > mix(UPPER_Y, STUB_HEIGHT, cut)) {
                discard;
            }
        }
    } else if (cut_kind == 4u) {
        // The stub's flat top, once the wall is down.
        let rel = wp.y - world_params.player.w;
        if (rel < 0.0 || rel > UPPER_Y || wall_cut(in.uv) < 0.97) {
            discard;
        }
    } else if (cut_kind == 8u) {
        // A detail standing over the player: thinned out whole, in a stipple.
        let thin = object_fade(in.uv, in.uv_b.y) * FADE_MOST;
        if (thin > 0.0 && thin > bayer4(vec2<u32>(in.position.xy))) {
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
@if(!PREPASS_PIPELINE) {
    // The roof and upper storeys of the building the player is inside fade out as a whole (and
    // back in), by the amount in their mesh's tag (`world_render::hide_upper_floors`).
    let gone = f32(mesh_functions::get_tag(in.instance_index) & 255u) / 255.0;
    if (gone > 0.0 && gone > bayer4(vec2<u32>(in.position.xy) + vec2<u32>(2u, 1u))) {
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

@if(GROUND) {
    // Ground detail (DCM-59): the flat colours of the outdoors broken up, in the same faceted
    // style as everything else. Not indoors (floors keep their colour).
    if (wp.y < 0.05 && in.world_normal.y > 0.9) {
        let outdoors = 1.0 - smoothstep(0.3, 0.7, fog_sample(wp.xz).y);
        let c = pbr_input.material.base_color.rgb;
        pbr_input.material.base_color = vec4(mix(c, ground_detail(wp.xz, c), outdoors), pbr_input.material.base_color.a);
    }
}

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
    if (piece_kind == 2u || piece_kind == 8u) {
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
    let albedo_lum = dot(pbr_input.material.base_color.rgb, vec3(0.299, 0.587, 0.114));
    // What's lit is seen (#83): in the dark you see only a few metres by yourself, but a surface a
    // light falls on (a window's pool, someone's flashlight) shows however far off, given a line of
    // sight to it. How brightly it is lit is its colour over what it's made of; moonlight and the
    // ambient stay well under LIT_LO.
    let dark = world_params.lights.x;
    if (dark > 0.0 && world_params.sight[2].w > 0.5 && seen < 1.0) {
        let lit = smoothstep(LIT_LO, LIT_HI, grey / max(albedo_lum, 0.06)) * dark;
        if (lit > 0.0) {
            seen = max(seen, lit * los_at(at));
        }
    }
    // By day what's unseen keeps most of its colour, dimmed and warm; at night a quarter (DCM-59,
    // materials.rs `unseen_colour`).
    var unseen = mix(vec3(grey), out.color.rgb, world_params.lights.y) * world_params.fog_tint.rgb * (1.0 - world_params.fog_tint.w);
    // Greying flattens pale and dark surfaces toward one tone (a driveway vanishes into its lawn):
    // keep some of the difference in what they're made of (#81).
    unseen *= clamp(pow(albedo_lum / UNSEEN_MID, UNSEEN_CONTRAST), 0.6, 1.5);
    // Rooms you can't see into are near-black, as if unlit, so their contents stay hidden. Walls,
    // windows and doors stay readable (just dimmed like everything else unseen), so you can still
    // tell a building's layout and where its ways in are.
    var hide_contents = smoothstep(0.3, 0.7, indoors);
    if (piece_kind == 1u || piece_kind == 4u || piece_kind == 5u || piece_kind == 7u || piece_kind == 8u) {
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
