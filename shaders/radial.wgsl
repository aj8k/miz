// Radial spectrum: the bands wrapped around a circle, mirrored left/right,
// so the spectrum becomes a slowly turning, breathing flower.
//
// Everything is computed per pixel in polar coordinates: for each pixel
// we know its angle (-> which band) and radius (-> inside/outside the
// petal), so there is no geometry at all.

// Spectrum level at x (0 = lowest band, 1 = highest), smoothed across
// neighbouring bands so petals have round tips instead of corners.
fn level(x: f32) -> f32 {
    let d = 0.012;
    let v = 0.25 * band_at(x - d) + 0.5 * band_at(x) + 0.25 * band_at(x + d);
    // In silence, a slow ghost spectrum keeps the flower breathing.
    let ghost = 0.25 + 0.15 * sin(x * 9.0 + g.time * 0.35) * sin(g.time * 0.23);
    return mix(v, ghost, g.idle);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let aspect = g.resolution.x / g.resolution.y;
    let p = (in.uv - 0.5) * vec2<f32>(aspect, -1.0);
    let r = length(p);

    // Angle measured from the bottom, slowly rotating; mirrored so both
    // halves show the same spectrum: x = 0 at the bottom (bass), 1 at the top.
    let rot = g.energy_time * 0.04;
    let a = atan2(p.x, -p.y) + rot;
    let wrapped = a - TAU * floor((a + PI) / TAU); // -PI..PI
    let x = abs(wrapped) / PI;

    let v = pow(level(x), 1.25);
    let bass = mix(g.bass, 0.3 + 0.1 * sin(g.time * 0.4), g.idle);
    let r0 = 0.13 + 0.05 * bass;           // breathing inner radius
    let len = 0.34 * v;
    let r1 = r0 + len;                     // petal edge at this angle

    let aa = 1.5 / g.resolution.y;         // anti-aliasing width
    let d_out = r - r1;                    // > 0 outside the petal
    let inside = smoothstep(aa, -aa, d_out) * smoothstep(r0 - aa, r0 + aa, r);

    // Petal body: a gradient that brightens towards its tip.
    let along = clamp((r - r0) / max(len, 1e-3), 0.0, 1.0);
    let body = inside * (0.04 + 0.22 * along * along);
    // Bright rim along the petal edge, and a soft glow outside it.
    let rim = exp(-abs(d_out) / (2.5 * aa)) * (0.25 + 0.35 * v);
    let outside_disk = smoothstep(r0 - aa, r0 + aa, r);
    let glow = exp(-max(d_out, 0.0) / 0.035) * 0.10 * v * outside_disk;
    // Thin inner circle.
    let inner = exp(-abs(r - r0) / (1.5 * aa)) * 0.18;

    // Colour runs along the spectrum (bass warm -> treble cool) and
    // drifts slowly over time.
    let hue = palette(x * 0.55 + g.energy_time * 0.02,
                      vec3<f32>(0.55), vec3<f32>(0.45),
                      vec3<f32>(1.0), vec3<f32>(0.05, 0.30, 0.60));
    var c = hue * (body + rim + glow) + vec3<f32>(0.6, 0.65, 0.8) * inner;
    c *= 1.0 + 0.10 * g.beat;
    c += vec3<f32>(0.006, 0.007, 0.014) * (1.0 - smoothstep(0.0, 0.9, r));
    return vec4<f32>(c, 1.0);
}
