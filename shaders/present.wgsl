// Final pass: scales the (possibly lower-resolution) HDR scene to the
// window, compresses highlights, encodes to sRGB and dithers.
//
// We render into a *non*-sRGB swapchain and do the sRGB encoding here, so
// we can add dither noise after encoding — at exactly one 8-bit step. That
// hides banding in the slow, dark gradients these scenes are full of.

@group(1) @binding(0) var scene_tex: texture_2d<f32>;
@group(1) @binding(1) var scene_smp: sampler;
@group(2) @binding(0) var<storage, read> lim: Limiter;

const LUMA: vec3<f32> = vec3<f32>(0.2126, 0.7152, 0.0722);

// Soft shoulder: identity below `knee`, then a tanh roll-off towards
// `ceiling`, so bright spots saturate gracefully instead of clipping hard.
fn tonemap(c: vec3<f32>, knee: f32, ceiling: f32) -> vec3<f32> {
    let over = max(c - knee, vec3<f32>(0.0));
    let room = ceiling - knee;
    return min(c, vec3<f32>(knee)) + room * tanh(over / room);
}

fn linear_to_srgb(c: vec3<f32>) -> vec3<f32> {
    let lo = c * 12.92;
    let hi = 1.055 * pow(c, vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(hi, lo, c <= vec3<f32>(0.0031308));
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    var hdr = max(textureSample(scene_tex, scene_smp, in.uv).rgb, vec3<f32>(0.0));
    // Safety limiter (luma.wgsl): keeps the average brightness from
    // changing faster than the allowed rate.
    hdr *= lim.exposure;
    // Mode grading. Energetic: a bit more saturated and brighter than the
    // scenes' own colours. Calm: muted, dimmer, with a lower ceiling.
    let calm = g.calm;
    hdr = mix(vec3<f32>(dot(hdr, LUMA)), hdr, mix(1.2, 0.75, calm));
    hdr = max(hdr, vec3<f32>(0.0)); // oversaturation can push channels below 0
    hdr *= mix(1.1, 0.8, calm);
    let mapped = tonemap(hdr, mix(0.6, 0.45, calm), mix(1.0, 0.8, calm));
    var c = linear_to_srgb(clamp(mapped, vec3<f32>(0.0), vec3<f32>(1.0)));
    // Triangular-PDF dither (difference of two uniform noises), ±1 LSB.
    let n = hash12(in.pos.xy + f32(g.frame % 64u) * 17.0)
          - hash12(in.pos.yx + f32(g.frame % 64u) * 31.0);
    c += n / 255.0;
    return vec4<f32>(c, 1.0);
}
