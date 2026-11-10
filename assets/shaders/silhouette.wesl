// The player's silhouette through whatever still covers them (crates/client/src/silhouette.rs):
// the survivor's own meshes drawn once more in the transparent pass, in one flat tint, with the
// depth test turned round, so only the pixels where something stands in front of the body are
// drawn. The depth compared is the body's moved toward the camera by `look.depth.x` (the camera is
// orthographic, so depth is linear in metres), so the body never counts as covering itself: an
// arm in front of the chest is not something to show the chest through.

import bevy_pbr::render::forward_io::VertexOutput;

struct SilhouetteLook {
    // The tint (linear, straight alpha).
    color: vec4<f32>,
    // x = how far toward the camera the body's depth is moved, in depth-buffer units.
    depth: vec4<f32>,
};

@group(constants::MATERIAL_BIND_GROUP) @binding(0) var<uniform> look: SilhouetteLook;

struct SilhouetteOutput {
    @location(0) color: vec4<f32>,
    @builtin(frag_depth) depth: f32,
};

@fragment
fn fragment(in: VertexOutput) -> SilhouetteOutput {
    var out: SilhouetteOutput;
    out.color = look.color;
    out.depth = in.position.z + look.depth.x;
    return out;
}
