// Opening hints (crates/client/src/opening_hints.rs): the outline of a door or window on a wall
// that faces away from the camera, drawn through its building. As the player's silhouette
// (silhouette.wesl): a flat tint in the transparent pass with the depth test turned round, so
// only where something stands in front of it, its depth moved toward the camera a little so the
// wall it is in doesn't count. Every outline is an instance of one of two small meshes; its
// MeshTag says its tint and how far it has faded in.

import bevy_pbr::render::{
    forward_io::VertexOutput,
    mesh_functions,
};

struct HintLook {
    // The tints (linear, straight alpha): a shut door, a shut window, an open or broken one.
    door: vec4<f32>,
    window: vec4<f32>,
    open: vec4<f32>,
    // x = how far toward the camera the outline's depth is moved (depth-buffer units), y = a
    // barred opening's share of the alpha, z = brightness (dim at night).
    params: vec4<f32>,
};

@group(constants::MATERIAL_BIND_GROUP) @binding(0) var<uniform> look: HintLook;

struct HintOutput {
    @location(0) color: vec4<f32>,
    @builtin(frag_depth) depth: f32,
};

@fragment
fn fragment(in: VertexOutput) -> HintOutput {
    // Bits 0-7: how much shows. 8-9: the tint. 10: barred.
    let tag = mesh_functions::get_tag(in.instance_index);
    let which = (tag >> 8u) & 3u;
    var tint = look.door;
    if (which == 1u) {
        tint = look.window;
    } else if (which == 2u) {
        tint = look.open;
    }
    var alpha = tint.a * f32(tag & 255u) / 255.0;
    if ((tag & 1024u) != 0u) {
        alpha *= look.params.y;
    }
@if(VERTEX_COLORS) {
    // The bars are whole, the fill between them faint.
    alpha *= in.color.a;
}
    var out: HintOutput;
    out.color = vec4<f32>(tint.rgb * look.params.z, alpha);
    out.depth = in.position.z + look.params.x;
    return out;
}
