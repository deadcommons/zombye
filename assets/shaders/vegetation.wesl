// Instanced vegetation and litter (Linear DCM-59, crates/client/src/vegetation.rs): the vertex
// stage of a material whose fragment stage is world.wesl, so grass is lit, shadowed and fogged
// exactly like the ground it stands on. Every patch is one instance of a small shared mesh; this
// shader gives each its colour (from its MeshTag), sways it in the wind, bends it away from the
// player's feet and shrinks it to nothing toward the edge of the drawn radius.

import bevy_pbr::render::{
    mesh_functions,
    forward_io::{Vertex, VertexOutput, decompress_vertex},
    view_transformations::position_world_to_clip,
};

struct VegParams {
    // x, y = the camera's focus on the ground (world x, z), z = the radius vegetation is drawn to,
    // w = seconds.
    focus: vec4<f32>,
    // x, y = the player's feet (world x, z), z = wind strength, w unused.
    player: vec4<f32>,
    // Blade colours (linear): fresh and dry. A blade's baked colour is a brightness times these.
    green: vec4<f32>,
    dry: vec4<f32>,
};

@group(constants::MATERIAL_BIND_GROUP) @binding(104) var<uniform> veg: VegParams;

@vertex
fn vertex(vertex_in: Vertex) -> VertexOutput {
    var out: VertexOutput;
    let v = decompress_vertex(vertex_in, vertex_in.instance_index);
    let world_from_local = mesh_functions::get_world_from_local(vertex_in.instance_index);
    let tag = mesh_functions::get_tag(vertex_in.instance_index);
    var world = mesh_functions::mesh_position_local_to_world(world_from_local, vec4<f32>(v.position, 1.0));
    let root = world_from_local[3].xyz;
    // How far up the blade this vertex is (0 at the root): what moves it.
    let h = max(world.y - root.y, 0.0);

    // Wind: gusts rolling across the field, and each patch a little out of step with the next.
    let t = veg.focus.w;
    let along = world.x * 0.31 + world.z * 0.17;
    let gust = sin(along - t * 1.3) * 0.6 + sin(along * 2.7 + root.x * 1.9 - t * 2.9) * 0.3 + 0.35;
    let sway = vec2(0.88, 0.47) * gust * veg.player.z * h;

    // Trodden: blades within a step of the player lean away and lie lower.
    let from_player = world.xz - veg.player.xy;
    let near = 1.0 - smoothstep(0.25, 0.95, length(from_player));
    let push = normalize(from_player + vec2(1e-4, 0.0)) * near * 0.5 * h;

    // Shrunk to nothing toward the edge of what's drawn, so patches never pop in.
    let reach = veg.focus.z;
    let shown = 1.0 - smoothstep(reach * 0.84, reach, length(root.xz - veg.focus.xy));
    world = vec4(world.x + sway.x + push.x, root.y + h * shown * (1.0 - 0.55 * near), world.z + sway.y + push.y, 1.0);

    out.world_position = world;
    out.position = position_world_to_clip(world.xyz);
    // Lit as the ground is, whichever way a blade faces: no dark or flickering blades.
    out.world_normal = vec3(0.0, 1.0, 0.0);

@if(VERTEX_UVS_A)
    out.uv = v.uv;
@if(VERTEX_UVS_B)
    out.uv_b = v.uv_b;
@if(VERTEX_TANGENTS)
    out.world_tangent = vec4(1.0, 0.0, 0.0, 1.0);

@if(VERTEX_COLORS) {
    // Alpha 1: a blade, coloured here (a brightness baked along it, times this patch's mix of
    // fresh and dry). Alpha 0: its own colour (petals, paper, stones).
    let dryness = f32(tag & 255u) / 255.0;
    let bright = 0.86 + 0.28 * f32((tag >> 8u) & 255u) / 255.0;
    let blade = mix(veg.green.rgb, veg.dry.rgb, dryness) * bright;
    out.color = vec4(v.color.rgb * mix(vec3(1.0), blade, v.color.a), 1.0);
}

@if(VERTEX_OUTPUT_INSTANCE_INDEX)
    out.instance_index = vertex_in.instance_index;

@if(VISIBILITY_RANGE_DITHER)
    out.visibility_range_dither = mesh_functions::get_visibility_range_dither_level(vertex_in.instance_index, world_from_local[3]);

    return out;
}
