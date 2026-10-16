// "Bass pulse": a soft orb that breathes with the bass, a thin ring that
// wobbles with the mids, and faint ripples drifting outwards. The simplest
// possible audio-reactive scene — a sanity check for the whole pipeline.

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let aspect = g.resolution.x / g.resolution.y;
    // Centered coordinates, aspect-corrected, y up: circles stay round.
    let p = (in.uv - 0.5) * vec2<f32>(aspect, -1.0);
    let r = length(p);
    let a = atan2(p.y, p.x);
    let t = g.energy_time;

    // In silence, fall back to a slow "breathing" instead of freezing.
    let breath = 0.5 + 0.5 * sin(g.time * 0.4);
    let bass = mix(g.bass, 0.3 * breath, g.idle);
    let mids = mix(g.low_mid, 0.2, g.idle);
    let loud = mix(g.loudness, 0.25 + 0.15 * breath, g.idle);

    let radius = 0.10 + 0.16 * bass;
    let orb = exp(-pow(r / radius, 2.0) * 1.6);

    let wobble = 0.012 * sin(a * 5.0 + t * 0.8) * (0.3 + mids)
               + 0.006 * sin(a * 9.0 - t * 1.3) * g.high_mid;
    let ring_r = radius * 1.45 + wobble;
    let ring = exp(-pow((r - ring_r) / 0.01, 2.0)) * (0.25 + 0.75 * loud);

    // Ripples move outwards at a speed tied to energy time.
    let ripples = 0.5 + 0.5 * sin(r * 28.0 - t * 1.6);
    let halo = ripples * exp(-r * 3.5) * 0.07;

    let hue = palette(t * 0.02 + r * 0.5,
                      vec3<f32>(0.5), vec3<f32>(0.4),
                      vec3<f32>(1.0), vec3<f32>(0.0, 0.33, 0.67));
    var col = hue * (orb * 0.75 + ring * 0.6 + halo);
    // Beat: a *small* brightness lift (the pulse itself is already smoothed).
    col *= 1.0 + 0.12 * g.beat;
    // Deep blue vignette so the background isn't pure black.
    col += vec3<f32>(0.008, 0.010, 0.022) * (1.0 - smoothstep(0.0, 0.9, r));
    return vec4<f32>(col, 1.0);
}
