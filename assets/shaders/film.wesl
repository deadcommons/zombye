// The film pass (Linear DCM-59, crates/client/src/film.rs): after the tone map, a soft focus
// toward the top and bottom of the screen (a little tilt-shift, so the town reads as a small
// model) and fine film grain. Either can be off (its strength 0). The old look, when on, makes a faded print
// of the picture: blacks lifted to a warm brown, whites washed to cream, a little colour gone and
// a soft glow round the brightest parts. With nothing on the pass isn't run.

import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput;

struct Film {
    // Grain strength (0 = off), tilt-shift blur radius at the screen's edge in 1/1080ths of its
    // height (0 = off), the clock in seconds, the old look's glow (0 = off).
    grain: f32,
    tilt: f32,
    time: f32,
    glow: f32,
    // The old look: rgb what black becomes (as the screen shows it), w how much colour goes.
    black: vec4<f32>,
    // rgb what white becomes; w 1 with the old look on, 0 without.
    white: vec4<f32>,
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

    if (film.white.w > 0.0) {
        // The glow: what is bright round this pixel, from two rings (6 and 13 pixels of a
        // 1080-high picture), laid over it as light through old paper.
        if (film.glow > 0.0) {
            var bright = vec3(0.0);
            for (var i = 0; i < 12; i++) {
                let a = f32(i) * 0.5235988;
                let r = select(6.0, 13.0, i % 2 == 1) * size.y / 1080.0;
                let tap = textureSampleLevel(screen, screen_sampler, in.uv + vec2(cos(a), sin(a)) * r / size, 0.0).rgb;
                bright += tap * smoothstep(0.35, 0.8, dot(tap, vec3(0.299, 0.587, 0.114)));
            }
            let g = bright / 12.0 * film.glow;
            color = 1.0 - (1.0 - min(color, vec3(1.0))) * (1.0 - min(g, vec3(1.0)));
        }
        // The fade, worked out as the eye sees it (not in linear light): black to ink, white to paper.
        var seen = pow(clamp(color, vec3(0.0), vec3(1.0)), vec3(1.0 / 2.2));
        seen = mix(seen, vec3(dot(seen, vec3(0.299, 0.587, 0.114))), film.black.w);
        seen = film.black.rgb + seen * (film.white.rgb - film.black.rgb);
        color = pow(seen, vec3(2.2));
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
