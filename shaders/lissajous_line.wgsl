// Stereo oscilloscope ("Lissajous" / goniometer): each stereo sample is a
// point, left vs right; consecutive points are joined like a CRT beam.
//
// Axes are rotated 45° into mid/side: up = mid (L+R), right = side (R-L).
// So mono shows as a vertical line, wide stereo as a fat cloud, and
// out-of-phase content lies flat — exactly like a studio goniometer.

const SEGMENTS: u32 = 1023u;

struct LineOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) across: f32,
    @location(1) intensity: f32,
};

// Stereo sample, gently low-passed (triangular window over ±6 samples,
// ≈ 4 kHz cutoff): keeps the shape of the music, drops the hiss that
// turns the trace into scribbles.
fn smooth_lr(i: u32) -> vec2<f32> {
    var s = vec2<f32>(0.0);
    var wsum = 0.0;
    for (var k = -3; k <= 3; k++) {
        let w = 4.0 - abs(f32(k));
        s += wave[u32(clamp(i32(i) + k * 2, 0, 1023))] * w;
        wsum += w;
    }
    return s / wsum;
}

fn scope_point(i: u32) -> vec2<f32> {
    let s = smooth_lr(i);
    let mid = (s.x + s.y) * 0.70710678;
    let side = (s.y - s.x) * 0.70710678;
    let live = vec2<f32>(side, mid) * 0.40;

    // Silence: a slowly morphing 3:2 Lissajous figure instead of a dot.
    let th = f32(i) / f32(SEGMENTS) * TAU;
    let idle = 0.22 * vec2<f32>(sin(3.0 * th + g.time * 0.15), sin(2.0 * th));
    return mix(live, idle, g.idle);
}

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> LineOut {
    let seg = vi / 6u;
    let corner = vi % 6u;
    let a = scope_point(seg);
    let b = scope_point(min(seg + 1u, SEGMENTS));
    let d = b - a;
    let len = length(d);
    let dir = select(vec2<f32>(1.0, 0.0), d / len, len > 1e-6);
    let normal = vec2<f32>(-dir.y, dir.x);

    let at_b = corner == 2u || corner == 3u || corner == 5u;
    let side = select(-1.0, 1.0, corner == 1u || corner == 4u || corner == 5u);
    let half_width = 1.3 / g.resolution.y;
    // Extend each segment by half a width at both ends so joints overlap.
    let p = select(a - dir * half_width, b + dir * half_width, at_b)
          + normal * side * half_width;

    let aspect = g.resolution.x / g.resolution.y;
    var o: LineOut;
    o.pos = vec4<f32>(p.x * 2.0 / aspect, p.y * 2.0, 0.0, 1.0);
    o.across = side;
    // A real beam deposits less energy per spot when it moves fast: long
    // segments are dimmer. That's what gives a scope its depth. The cap
    // stops the many tiny segments near silence from piling up into a
    // blinding spot in the centre.
    o.intensity = clamp(0.0025 / (len + 0.0015), 0.10, 0.35);
    return o;
}

@fragment
fn fs_main(in: LineOut) -> @location(0) vec4<f32> {
    let edge = 1.0 - smoothstep(0.2, 1.0, abs(in.across));
    // Phosphor colour drifting slowly between green, teal and amber.
    let hue = palette(g.energy_time * 0.015 + 0.35,
                      vec3<f32>(0.5, 0.6, 0.5), vec3<f32>(0.35, 0.3, 0.3),
                      vec3<f32>(1.0), vec3<f32>(0.0, 0.25, 0.5));
    let level = mix(0.10 + 0.10 * g.loudness, 0.10, g.idle);
    return vec4<f32>(hue * level * in.intensity * edge, 1.0);
}
