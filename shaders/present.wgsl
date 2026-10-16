// Final pass: scales the (possibly lower-resolution) HDR scene to the
// window, compresses highlights, encodes to sRGB and dithers.
//
// We render into a *non*-sRGB swapchain and do the sRGB encoding here, so
// we can add dither noise after encoding — at exactly one 8-bit step. That
// hides banding in the slow, dark gradients these scenes are full of.

@group(1) @binding(0) var scene_tex: texture_2d<f32>;
@group(1) @binding(1) var scene_smp: sampler;

// Soft shoulder: identity below `knee`, then a tanh roll-off towards 1.0,
// so bright spots saturate gracefully instead of clipping hard.
fn tonemap(c: vec3<f32>) -> vec3<f32> {
    let knee = 0.6;
    let over = max(c - knee, vec3<f32>(0.0));
    return min(c, vec3<f32>(knee)) + (1.0 - knee) * tanh(over / (1.0 - knee));
}

fn linear_to_srgb(c: vec3<f32>) -> vec3<f32> {
    let lo = c * 12.92;
    let hi = 1.055 * pow(c, vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(hi, lo, c <= vec3<f32>(0.0031308));
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let hdr = max(textureSample(scene_tex, scene_smp, in.uv).rgb, vec3<f32>(0.0));
    var c = linear_to_srgb(clamp(tonemap(hdr), vec3<f32>(0.0), vec3<f32>(1.0)));
    // Triangular-PDF dither (difference of two uniform noises), ±1 LSB.
    let n = hash12(in.pos.xy + f32(g.frame % 64u) * 17.0)
          - hash12(in.pos.yx + f32(g.frame % 64u) * 31.0);
    c += n / 255.0;
    return vec4<f32>(c, 1.0);
}
