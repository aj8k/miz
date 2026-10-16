// Shows the phosphor buffer with a soft glow (a cheap, fixed-radius blur
// added on top: the "bloom" of a bright trace on glass) and a faint
// graticule for the analog-instrument feel.

@group(1) @binding(0) var ph_tex: texture_2d<f32>;
@group(1) @binding(1) var ph_smp: sampler;

fn ring_blur(uv: vec2<f32>, radius_px: f32) -> vec3<f32> {
    let px = radius_px / g.resolution;
    var s = vec3<f32>(0.0);
    for (var i = 0; i < 8; i++) {
        let a = f32(i) * (TAU / 8.0);
        s += textureSample(ph_tex, ph_smp, uv + vec2<f32>(cos(a), sin(a)) * px).rgb;
    }
    return s / 8.0;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let base = textureSample(ph_tex, ph_smp, in.uv).rgb;
    let glow = 0.5 * ring_blur(in.uv, 3.0) + 0.35 * ring_blur(in.uv, 9.0);
    var c = base + glow * 0.8;

    // Graticule: a dim circle and cross-hairs in the mid/side frame.
    let aspect = g.resolution.x / g.resolution.y;
    let p = (in.uv - 0.5) * vec2<f32>(aspect, 1.0);
    let lw = 1.0 / g.resolution.y;
    let circle = 1.0 - smoothstep(0.0, lw * 1.5, abs(length(p) - 0.40));
    let axes = (1.0 - smoothstep(0.0, lw, abs(p.x))) + (1.0 - smoothstep(0.0, lw, abs(p.y)));
    let grid = (circle + 0.6 * axes * (1.0 - smoothstep(0.40, 0.44, length(p))));
    c += vec3<f32>(0.010, 0.016, 0.014) * grid;

    // Vignette, like the curved edge of a CRT.
    c *= 1.0 - 0.5 * smoothstep(0.3, 0.9, length(p));
    return vec4<f32>(c, 1.0);
}
