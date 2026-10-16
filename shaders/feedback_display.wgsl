// Shows the feedback buffer. Kept separate from the buffer itself so
// display-only touches (vignette, a soft centre glow) don't feed back into
// the next frame and accumulate.

@group(1) @binding(0) var fb_tex: texture_2d<f32>;
@group(1) @binding(1) var fb_smp: sampler;

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    var c = textureSample(fb_tex, fb_smp, in.uv).rgb;
    let aspect = g.resolution.x / g.resolution.y;
    let p = (in.uv - 0.5) * vec2<f32>(aspect, 1.0);
    let r = length(p);
    // Soft vignette: pulls attention to the centre, hides the edges where
    // the warp has the least history.
    c *= 1.0 - 0.45 * smoothstep(0.35, 0.95, r);
    // A faint glow in the middle, breathing with the bass, in the same hue
    // as the fresh waveform.
    let glow = exp(-r * r * 60.0) * (0.012 + 0.03 * mix(g.bass, 0.3, g.idle));
    c += glow * palette(g.energy_time * 0.03,
                        vec3<f32>(0.55), vec3<f32>(0.45),
                        vec3<f32>(1.0), vec3<f32>(0.0, 0.33, 0.67));
    return vec4<f32>(c, 1.0);
}
