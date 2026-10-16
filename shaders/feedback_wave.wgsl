// The fresh waveform, drawn as a closed ring on top of the warped previous
// frame (additive blending). Each frame's ring is then carried outwards by
// the warp, which is what builds the tunnel.
//
// Thick lines: GPUs only rasterize 1-pixel lines, so every segment of the
// ring becomes a quad (two triangles, 6 vertices) extruded along the
// segment's normal. The vertex shader computes positions straight from the
// waveform buffer — there is no vertex buffer at all.

const SEGMENTS: u32 = 512u;

struct WaveOut {
    @builtin(position) pos: vec4<f32>,
    // -1 .. 1 across the line's width, for a soft (anti-aliased) edge.
    @location(0) across: f32,
    // Position around the ring, 0..1.
    @location(1) u: f32,
};

// Mono sample, low-passed with a triangular window over ±24 samples
// (≈ 1 kHz cutoff): raw 48 kHz samples look like jagged noise; the
// smoothed shape keeps the bass and mids and is much calmer.
fn smooth_mono(i: i32) -> f32 {
    var s = 0.0;
    var wsum = 0.0;
    for (var k = -6; k <= 6; k++) {
        let w = 7.0 - abs(f32(k));
        let v = wave[clamp(i + k * 4, 0, 1023)];
        s += (v.x + v.y) * w;
        wsum += 2.0 * w;
    }
    return s / wsum;
}

// Point on the ring at parameter u in 0..1, in aspect-corrected space
// (height = 1). The ring is mirrored — samples run 0..1023 up one side and
// back down the other — so it closes without a seam.
fn ring_point(u: f32) -> vec2<f32> {
    let m = 1.0 - abs(u * 2.0 - 1.0);
    let x = m * 1023.0;
    let i = i32(floor(x));
    let s = mix(smooth_mono(i), smooth_mono(min(i + 1, 1023)), fract(x));

    // In silence the line would be a perfect static circle; let a slow,
    // soft shape move through it instead.
    let ghost = 0.35 * sin(u * TAU * 3.0 + g.time * 0.3) * sin(g.time * 0.21);
    let shape = mix(s, ghost, g.idle);

    let base = 0.16 + 0.05 * g.bass;
    let r = base + 0.06 * shape;
    let a = u * TAU + g.energy_time * 0.08;
    return vec2<f32>(cos(a), sin(a)) * r;
}

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> WaveOut {
    let seg = vi / 6u;
    let corner = vi % 6u;
    let a = ring_point(f32(seg) / f32(SEGMENTS));
    let b = ring_point(f32(seg + 1u) / f32(SEGMENTS));
    let dir = normalize(b - a + vec2<f32>(1e-6, 0.0));
    let normal = vec2<f32>(-dir.y, dir.x);

    // Two triangles: (a-, a+, b-) and (b-, a+, b+).
    let at_b = corner == 2u || corner == 3u || corner == 5u;
    let side = select(-1.0, 1.0, corner == 1u || corner == 4u || corner == 5u);
    // Half-width: ~1.6 px at the target's height, whatever the resolution.
    let half_width = 1.6 / g.resolution.y;
    let p = select(a, b, at_b) + normal * side * half_width;

    let aspect = g.resolution.x / g.resolution.y;
    var o: WaveOut;
    o.pos = vec4<f32>(p.x * 2.0 / aspect, p.y * 2.0, 0.0, 1.0);
    o.across = side;
    o.u = f32(seg) / f32(SEGMENTS);
    return o;
}

@fragment
fn fs_main(in: WaveOut) -> @location(0) vec4<f32> {
    let edge = 1.0 - smoothstep(0.3, 1.0, abs(in.across));
    // Hue: slowly cycling over time, nudged by the bass, and varying a
    // little around the ring.
    let hue = palette(g.energy_time * 0.03 + 0.15 * g.bass + 0.1 * sin(in.u * TAU),
                      vec3<f32>(0.55), vec3<f32>(0.45),
                      vec3<f32>(1.0), vec3<f32>(0.0, 0.33, 0.67));
    let level = mix(0.10 + 0.18 * g.loudness, 0.12, g.idle) + 0.04 * g.beat;
    let intensity = level * mix(1.7, 0.8, g.calm);
    return vec4<f32>(hue * intensity * edge, 1.0);
}
