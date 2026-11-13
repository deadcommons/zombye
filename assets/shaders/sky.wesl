// The sky (crates/client/src/sky.rs): drawn behind everything, on the far plane, by one triangle
// that covers the screen. Seen only in the first-person view (from above the camera looks down
// and the ground fills the screen; there it gives the clear colour, `look.clear`, so nothing
// changes). Its colours follow the hour from light.ron's `sky`: a gradient from the horizon to
// the zenith, a glow round the sun or moon worked out exactly as the haze's glow (Bevy's distance
// fog), so the far edge of the drawn world melts into the sky behind it, the sun's and the moon's
// discs, and stars at night.

import bevy_pbr::render::mesh_view_bindings::{view, lights, globals};

struct SkyLook {
    // The zenith (linear rgb), a = how quickly the sky turns from the horizon's colour to it.
    zenith: vec4<f32>,
    // The horizon (linear rgb; the haze's colour too), a = how dark below the horizon (0..1).
    horizon: vec4<f32>,
    // What the view from above shows (linear rgb, the clear colour), a = how much of the sky
    // shows (0 from above, 1 in first person, in between on the way).
    clear: vec4<f32>,
    // The glow round the sun or moon (linear rgb), a = its strength (Bevy's fog glow).
    glow: vec4<f32>,
    // x = the glow's exponent, y = the sun's disc's angular size (radians), z = its brightness
    // against the sun's light, w = daylight (0 night, 1 day).
    sun: vec4<f32>,
    // x = the moon's disc's angular size, y = its brightness, z = how many stars (0..1),
    // w = how bright they are.
    night: vec4<f32>,
};

@group(constants::MATERIAL_BIND_GROUP) @binding(0) var<uniform> look: SkyLook;

struct SkyVertex {
    @builtin(position) clip: vec4<f32>,
    @location(0) ndc: vec2<f32>,
};

@vertex
fn vertex(@location(0) position: vec3<f32>) -> SkyVertex {
    var out: SkyVertex;
    // The triangle is given in clip space: (-1,-1), (3,-1), (-1,3). Depth 0 is the far plane
    // (reversed z), so anything drawn is in front of it.
    out.clip = vec4<f32>(position.xy, 0.0, 1.0);
    out.ndc = position.xy;
    return out;
}

fn hash3(p: vec3<f32>) -> f32 {
    var q = fract(p * vec3<f32>(0.1031, 0.1030, 0.0973));
    q += dot(q, q.yxz + 33.33);
    return fract((q.x + q.y) * q.z);
}

// Stars: a point in some of the cells of a grid round the camera, as seen along `dir`.
fn stars(dir: vec3<f32>) -> f32 {
    let cells = 120.0;
    let p = dir * cells;
    let cell = floor(p);
    let h = hash3(cell);
    if h > look.night.z * 0.25 {
        return 0.0;
    }
    // Kept off the cell's edges, so no star is cut in half by its neighbour's cell.
    let at = normalize(cell + 0.25 + 0.5 * vec3<f32>(hash3(cell + 7.1), hash3(cell + 3.7), hash3(cell + 1.3)));
    let angle = acos(clamp(dot(dir, at), -1.0, 1.0));
    let size = 0.0016 + 0.0024 * hash3(cell + 9.9);
    let twinkle = 0.75 + 0.25 * sin(globals.time * (1.5 + 3.0 * h * 4.0) + h * 40.0);
    return (1.0 - smoothstep(size * 0.4, size, angle)) * twinkle * (0.4 + 0.6 * hash3(cell + 5.5));
}

@fragment
fn fragment(in: SkyVertex) -> @location(0) vec4<f32> {
    let amount = look.clear.a;
    if amount <= 0.0 {
        return vec4<f32>(look.clear.rgb, 1.0);
    }
    // The ray through this pixel. An orthographic camera's rays all go the way it looks.
    var dir_view = vec3<f32>(0.0, 0.0, -1.0);
    if view.clip_from_view[3].w != 1.0 {
        let near = view.view_from_clip * vec4<f32>(in.ndc, 1.0, 1.0);
        dir_view = normalize(near.xyz / near.w);
    }
    let dir = normalize((view.world_from_view * vec4<f32>(dir_view, 0.0)).xyz);

    // The gradient, and darker below the horizon (seen only past the edge of the drawn world).
    let up = max(dir.y, 0.0);
    var sky = mix(look.horizon.rgb, look.zenith.rgb, 1.0 - pow(1.0 - up, look.zenith.a));
    sky *= mix(1.0, 1.0 - look.horizon.a, smoothstep(0.0, -0.08, dir.y));

    // The glow and the disc of the one light in the sky (the sun by day, the moon at night).
    if lights.n_directional_lights > 0u {
        let light = lights.directional_lights[0];
        let to_light = light.direction_to_light;
        let along = max(dot(dir, to_light), 0.0);
        // As the haze's glow (pbr_functions::apply_fog), so the two meet without a seam.
        sky += pow(along, look.sun.x) * light.color.rgb * view.exposure * look.glow.rgb * look.glow.a;
        let angle = acos(clamp(dot(dir, to_light), -1.0, 1.0));
        let day = look.sun.w;
        let size = mix(look.night.x, look.sun.y, day);
        let w = max(fwidth(angle), 1e-5);
        let disc = 1.0 - smoothstep(size * 0.5 - w, size * 0.5 + w, angle);
        let colour = light.color.rgb / max(max(light.color.r, max(light.color.g, light.color.b)), 1e-4);
        let bright = mix(look.night.y, look.sun.z, day);
        // Behind the hills the disc is gone (it sits on the far plane, so the world covers it).
        sky += disc * colour * bright * smoothstep(-0.02, 0.01, to_light.y);
    }

    // Stars, at night, fading toward the horizon.
    if look.night.z > 0.0 && look.sun.w < 0.6 {
        let fade = (1.0 - look.sun.w / 0.6) * smoothstep(0.0, 0.25, dir.y);
        sky += vec3<f32>(0.9, 0.93, 1.0) * stars(dir) * look.night.w * fade;
    }

    return vec4<f32>(mix(look.clear.rgb, sky, amount), 1.0);
}
