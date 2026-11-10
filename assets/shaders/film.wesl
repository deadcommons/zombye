// The film pass (Linear DCM-59, crates/client/src/film.rs): after the tone map, a soft focus
// toward the top and bottom of the screen (a little tilt-shift, so the town reads as a small
// model) and fine film grain. Either can be off (its strength 0); with both off the pass isn't run.

import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput;

struct Film {
    // Grain strength (0 = off), tilt-shift blur radius at the screen's edge in 1/1080ths of its
    // height (0 = off), the clock in seconds, unused.
    grain: f32,
    tilt: f32,
    time: f32,
    pad: f32,
};

@group(0) @binding(0) var screen: texture_2d<f32>;
@group(0) @binding(1) var screen_sampler: sampler;
@group(0) @binding(2) var<uniform> film: Film;

// The band across the middle of the screen (as a share of its height from the centre) that stays
// sharp, and where the blur is at full strength.
const SHARP: f32 = 0.28;
const SOFT: f32 = 0.5;

fn hash(p: vec2<f32>) -> f32 {
    var q = fract(vec3(p.xyx) * vec3(0.1031, 0.1030, 0.0973));
    q += dot(q, q.yzx + 33.33);
    return fract((q.x + q.y) * q.z);
}

@fragment
fn fragment(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let size = vec2<f32>(textureDimensions(screen));
    let centre = textureSampleLevel(screen, screen_sampler, in.uv, 0.0);
    var color = centre.rgb;

    if (film.tilt > 0.0) {
        let k = smoothstep(SHARP, SOFT, abs(in.uv.y - 0.5));
        if (k > 0.0) {
            // Eight taps on a ring and four nearer in, the same number of screen pixels at any size.
            let r = film.tilt * k * size.y / 1080.0;
            var sum = color * 2.0;
            for (var i = 0; i < 8; i++) {
                let a = (f32(i) + 0.5) * 0.7853982;
                sum += textureSampleLevel(screen, screen_sampler, in.uv + vec2(cos(a), sin(a)) * r / size, 0.0).rgb;
            }
            for (var i = 0; i < 4; i++) {
                let a = f32(i) * 1.5707963;
                sum += textureSampleLevel(screen, screen_sampler, in.uv + vec2(cos(a), sin(a)) * r * 0.5 / size, 0.0).rgb;
            }
            color = sum / 14.0;
        }
    }

    if (film.grain > 0.0) {
        // New grain 24 times a second; strongest in the mid-tones, none in pure black, so the night
        // stays clean and dark.
        let frame = floor(film.time * 24.0);
        let n = hash(in.position.xy + vec2(frame * 37.0, frame * 91.0)) + hash(in.position.yx * 1.31 + vec2(frame * 53.0, 17.0)) - 1.0;
        let luma = dot(color, vec3(0.299, 0.587, 0.114));
        let weight = smoothstep(0.0, 0.25, luma) * (1.0 - 0.6 * smoothstep(0.6, 1.0, luma));
        color += color * n * film.grain * weight + vec3(n * film.grain * 0.015 * weight);
    }
    return vec4(max(color, vec3(0.0)), centre.a);
}
