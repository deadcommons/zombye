// The map screen (crates/client/src/map_ui.rs): the survivor's paper map, drawn per pixel from the
// tile map, so it is sharp at any zoom.
//
// `tiles` has one texel a tile (what the ground is, the edges it owns, what stands on it, whether
// its building has been entered); `seen` one texel an explored cell (`sim::explore`), sampled
// smoothly. Everywhere the sheet shows the printed street plan; where the survivor has been,
// gardens, trees, fences and buildings are inked in over it, and a building's rooms once it has
// been inside. All colours are worked in sRGB, as picked in `map.ron`, and made linear at the end.

import bevy_ui_render::ui_vertex_output::UiVertexOutput;

// How many survivors and markers the sheet can show besides our own.
const PINS: i32 = 16;

struct MapLook {
    // The world point at the middle of the node (metres), device pixels a metre, UI scale.
    view: vec4<f32>,
    // The map's size in tiles, an explored cell's side in tiles, 1 = everything shown as seen.
    size: vec4<f32>,
    // The explored grid's size in cells.
    grid: vec4<f32>,
    // Our survivor: x, y, facing (radians, 0 = +x, turning toward +y), 1 = shown.
    me: vec4<f32>,
    // Where we woke up: x, y, 1 = shown.
    home: vec4<f32>,
    // Others: x, y, kind (1 a survivor of our group, 2 a marker, 0 none).
    pins: array<vec4<f32>, PINS>,
    paper: vec4<f32>,
    ink: vec4<f32>,
    table: vec4<f32>,
    grass: vec4<f32>,
    road: vec4<f32>,
    walk: vec4<f32>,
    path: vec4<f32>,
    dirt: vec4<f32>,
    house: vec4<f32>,
    shop: vec4<f32>,
    office: vec4<f32>,
    civic: vec4<f32>,
    tree: vec4<f32>,
    pine: vec4<f32>,
    marker: vec4<f32>,
    mate: vec4<f32>,
    pin: vec4<f32>,
};

@group(1) @binding(0) var<uniform> look: MapLook;
@group(1) @binding(1) var tiles: texture_2d<f32>;
@group(1) @binding(2) var seen: texture_2d<f32>;
@group(1) @binding(3) var seen_sampler: sampler;

// Ground classes (the red channel), as map_ui.rs writes them.
const GRASS: u32 = 0u;
const DIRT: u32 = 1u;
const ROAD: u32 = 2u;
const WALK: u32 = 3u;
const PATH: u32 = 4u;
const HOUSE: u32 = 8u;
const SHOP: u32 = 9u;
const OFFICE: u32 = 10u;
const CIVIC: u32 = 11u;
// Edge kinds (three bits each in the green channel: north, then west).
const E_OUTER: u32 = 1u;
const E_INNER: u32 = 2u;
const E_FENCE: u32 = 3u;

fn hash(p: vec2<f32>) -> f32 {
    var q = fract(vec3(p.xyx) * vec3(0.1031, 0.1030, 0.0973));
    q += dot(q, q.yzx + 33.33);
    return fract((q.x + q.y) * q.z);
}

fn noise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    return mix(mix(hash(i), hash(i + vec2(1.0, 0.0)), u.x), mix(hash(i + vec2(0.0, 1.0)), hash(i + vec2(1.0, 1.0)), u.x), u.y);
}

fn fbm(p: vec2<f32>) -> f32 {
    return noise(p) * 0.5 + noise(p * 2.03 + 17.0) * 0.3 + noise(p * 4.1 + 43.0) * 0.2;
}

// A tile's texel as whole numbers: class, edges, feature, entered. Off the map: open grass.
fn tile(t: vec2<i32>) -> vec4<u32> {
    if t.x < 0 || t.y < 0 || t.x >= i32(look.size.x) || t.y >= i32(look.size.y) {
        return vec4(0u);
    }
    return vec4<u32>(textureLoad(tiles, t, 0) * 255.0 + 0.5);
}

fn is_road(c: u32) -> bool {
    return c == ROAD;
}

fn is_building(c: u32) -> bool {
    return c >= HOUSE;
}

fn segment(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>) -> f32 {
    let pa = p - a;
    let ba = b - a;
    let h = clamp(dot(pa, ba) / dot(ba, ba), 0.0, 1.0);
    return length(pa - ba * h);
}

// How much of a pixel a line of half-width `hw` covers, `d` from its middle, a pixel being `aa` wide.
fn line(d: f32, hw: f32, aa: f32) -> f32 {
    return 1.0 - smoothstep(hw - aa * 0.6, hw + aa * 0.6, d);
}

fn ground(c: u32) -> vec3<f32> {
    switch c {
        case 1u: { return look.dirt.rgb; }
        case 2u: { return look.road.rgb; }
        case 3u: { return look.walk.rgb; }
        case 4u: { return look.path.rgb; }
        case 8u: { return look.house.rgb; }
        case 9u: { return look.shop.rgb; }
        case 10u: { return look.office.rgb; }
        case 11u: { return look.civic.rgb; }
        default: { return look.grass.rgb; }
    }
}

fn to_linear(c: vec3<f32>) -> vec3<f32> {
    return pow(max(c, vec3(0.0)), vec3(2.2));
}

@fragment
fn fragment(in: UiVertexOutput) -> @location(0) vec4<f32> {
    let scale = look.view.z;
    let px = (in.uv - 0.5) * in.size;
    let p = look.view.xy + px / scale;
    // A pixel in metres, and a UI pixel in device pixels.
    let aa = 1.0 / scale;
    let ui = look.view.w;
    let ink = look.ink.rgb;
    let map = look.size.xy;

    // The sheet: the map and a margin round it, on a dark veil the game shows through.
    let margin = 14.0;
    let out_d = max(max(-p.x, p.x - map.x), max(-p.y, p.y - map.y)) - margin;
    if out_d > 0.0 {
        // The sheet's shadow on the table.
        let shade = exp(-out_d / (9.0 * ui * aa));
        return vec4(to_linear(look.table.rgb * (1.0 - 0.5 * shade)), look.table.a + (1.0 - look.table.a) * 0.6 * shade);
    }

    // Paper: soft blotches, fine grain, a few fibres.
    let grain = (fbm(p * 0.045) - 0.5) * 0.07 + (hash(floor(p * scale / ui)) - 0.5) * 0.035;
    let fibre = smoothstep(0.82, 1.0, noise(vec2(p.x * 0.9 + p.y * 0.2, p.y * 0.06))) * 0.03;
    var paper = look.paper.rgb * (1.0 + grain - fibre);

    let t = vec2<i32>(floor(p));
    let own = tile(t);
    let cls = own.x;

    // What the survivor has seen here, with a ragged edge like a wash of ink.
    var known = textureSampleLevel(seen, seen_sampler, p / (look.grid.xy * look.size.z), 0.0).r;
    known = smoothstep(0.42, 0.58, known + (fbm(p * 0.16) - 0.5) * 0.3);
    known = max(known, look.size.w);

    // The printed plan, on every part of the sheet: streets paler than the paper, a fine line along
    // their sides.
    var print = paper;
    if is_road(cls) {
        print = mix(paper, look.road.rgb * (1.0 + grain), 0.75);
    } else if cls == WALK {
        print = mix(paper, look.road.rgb * (1.0 + grain), 0.25);
    }

    // Inked in: the ground by what it is.
    var drawn = mix(paper, ground(cls) * (1.0 + grain * 1.6), 0.9);
    let entered = own.w > 0u;
    if is_building(cls) && !entered {
        // A building not been inside yet: hatched across.
        let h = abs(fract((p.x + p.y) * 0.55) - 0.5);
        drawn = mix(drawn, ink, 0.2 * line(h, 0.09, aa * 0.55) * smoothstep(1.5, 4.0, scale / ui));
    }

    // Everything drawn as lines and blots, from this tile and the eight round it.
    var d_outer = 9.0;
    var d_inner = 9.0;
    var d_fence = 9.0;
    var d_kerb = 9.0;
    var canopy = 0.0;
    var canopy_edge = 0.0;
    var canopy_col = look.tree.rgb;
    for (var dy = -1; dy <= 1; dy += 1) {
        for (var dx = -1; dx <= 1; dx += 1) {
            let n = t + vec2(dx, dy);
            let o = tile(n);
            let c = vec2<f32>(n);
            let north = o.y & 7u;
            let west = (o.y >> 3u) & 7u;
            if north != 0u {
                let d = segment(p, c, c + vec2(1.0, 0.0));
                if north == E_OUTER { d_outer = min(d_outer, d); }
                if north == E_INNER && o.w > 0u { d_inner = min(d_inner, d); }
                if north == E_FENCE { d_fence = min(d_fence, d); }
            }
            if west != 0u {
                let d = segment(p, c, c + vec2(0.0, 1.0));
                if west == E_OUTER { d_outer = min(d_outer, d); }
                if west == E_INNER && o.w > 0u { d_inner = min(d_inner, d); }
                if west == E_FENCE { d_fence = min(d_fence, d); }
            }
            // The side of a street: where road meets what isn't, along this tile's north and west.
            let road = is_road(o.x);
            if road != is_road(tile(n + vec2(0, -1)).x) {
                d_kerb = min(d_kerb, segment(p, c, c + vec2(1.0, 0.0)));
            }
            if road != is_road(tile(n + vec2(-1, 0)).x) {
                d_kerb = min(d_kerb, segment(p, c, c + vec2(0.0, 1.0)));
            }
            // Trees and bushes: a blot with an inked rim.
            let f = o.z;
            if f != 0u {
                let h = hash(c + 3.7);
                let h2 = hash(c + 91.3);
                var r = 0.78;
                if f == 2u { r = 0.62; }
                if f == 3u { r = 0.4; }
                r *= 0.8 + 0.4 * h;
                let at = c + 0.5 + (vec2(h2, fract(h * 7.31)) - 0.5) * 0.3;
                let wob = 1.0 + 0.1 * sin(atan2(p.y - at.y, p.x - at.x) * 5.0 + h * 40.0);
                let d = length(p - at) - r * wob;
                let cover = 1.0 - smoothstep(-aa * 0.6, aa * 0.6, d);
                if cover > canopy {
                    canopy = cover;
                    if f == 2u { canopy_col = look.pine.rgb; } else { canopy_col = look.tree.rgb; }
                    if f == 3u { canopy_col = mix(look.tree.rgb, look.grass.rgb, 0.4); }
                }
                canopy_edge = max(canopy_edge, line(abs(d), 0.045, aa));
            }
        }
    }
    // A hand's line is never quite even.
    let wobble = 0.8 + 0.4 * noise(p * 2.7);
    let near = smoothstep(0.8, 3.0, scale / ui);

    drawn = mix(drawn, look.ink.rgb, 0.35 * line(d_fence, 0.07 * wobble, aa) * near);
    drawn = mix(drawn, canopy_col * (1.0 + grain), canopy * 0.92);
    drawn = mix(drawn, ink, 0.4 * canopy_edge * near);
    drawn = mix(drawn, ink, 0.45 * line(d_kerb, 0.06, aa) * near);
    drawn = mix(drawn, ink, 0.55 * line(d_inner, 0.07 * wobble, aa) * near);
    drawn = mix(drawn, ink, 0.92 * line(d_outer, max(0.15 * wobble, 0.7 * aa), aa));

    print = mix(print, ink, 0.2 * line(d_kerb, 0.05, aa) * near);

    var col = mix(print, drawn, known);

    // The sheet's own marks: a border line, folds, darker toward its edges.
    col = mix(col, ink, 0.5 * line(abs(out_d + margin - 5.0), 0.25 + 0.6 * aa, aa));
    let fold = min(min(abs(p.x - map.x / 3.0), abs(p.x - map.x * 2.0 / 3.0)), abs(p.y - map.y / 2.0));
    col *= 1.0 - 0.07 * exp(-fold / (1.2 * ui * aa)) + 0.04 * exp(-abs(fold - 2.0 * ui * aa) / (1.0 * ui * aa));
    let edge = clamp(-out_d / (margin * 2.5), 0.0, 1.0);
    col *= 0.9 + 0.1 * smoothstep(0.0, 1.0, edge);

    // Marks drawn at a fixed size on screen, whatever the zoom (sizes in UI pixels).
    let unit = scale / ui;
    // Where we woke up: a cross.
    if look.home.z > 0.5 {
        let q = (p - look.home.xy) * unit;
        let d = min(segment(q, vec2(-6.0, -6.0), vec2(6.0, 6.0)), segment(q, vec2(-6.0, 6.0), vec2(6.0, -6.0)));
        col = mix(col, look.marker.rgb, line(d, 1.6, 1.0 / ui) * 0.9);
    }
    for (var i = 0; i < PINS; i += 1) {
        let pin = look.pins[i];
        if pin.z < 0.5 { continue; }
        let q = (p - pin.xy) * unit;
        if pin.z < 1.5 {
            // A survivor of our group: a dot with an inked ring.
            let d = length(q);
            col = mix(col, look.mate.rgb, line(d, 5.0, 1.0 / ui));
            col = mix(col, ink, line(abs(d - 5.0), 0.9, 1.0 / ui));
        } else {
            // A marker: a diamond.
            let d = abs(q.x) + abs(q.y);
            col = mix(col, look.pin.rgb, line(d, 7.0, 1.4 / ui));
            col = mix(col, ink, line(abs(d - 7.0), 1.1, 1.4 / ui));
        }
    }
    // Us: an arrowhead the way we face, in a ring.
    if look.me.w > 0.5 {
        let q = (p - look.me.xy) * unit;
        let fwd = vec2(cos(look.me.z), sin(look.me.z));
        let side = vec2(-fwd.y, fwd.x);
        let l = vec2(dot(q, fwd), dot(q, side));
        // A triangle: tip 9 ahead, barbs 6 back and 6 out, notched at the tail.
        let d_tri = max(max(l.x - 9.0, (-6.0 + abs(l.y) * 0.5) - l.x), abs(l.y) - (9.0 - l.x) * 0.42);
        let ring = line(abs(length(q) - 15.0), 1.1, 1.0 / ui);
        col = mix(col, look.marker.rgb, ring * 0.9);
        col = mix(col, ink, line(d_tri, 1.4, 1.0 / ui));
        col = mix(col, look.marker.rgb, 1.0 - smoothstep(-1.0, 0.0, d_tri));
    }
    return vec4(to_linear(col), 1.0);
}
