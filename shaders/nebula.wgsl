// Nebula: domain-warped fractal noise ("fbm of fbm"), after Inigo Quilez,
// https://iquilezles.org/articles/warp/
//
// fbm(p) is smooth random terrain: several octaves of value noise, each
// finer and fainter. Domain warping feeds noise into the *coordinates* of
// more noise: f(p + fbm(p + fbm(p))). That folds the terrain into swirls
// and filaments that look like gas clouds — and animating the inner
// offsets makes them flow.
//
// Audio: bass drives brightness and zoom, treble the finest detail
// octaves, beats send a soft ripple through the domain.

fn hash21(p: vec2<f32>) -> f32 {
    var p3 = fract(vec3<f32>(p.x, p.y, p.x) * 0.1031);
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}

// Value noise: random values on a grid, smoothly interpolated.
fn noise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f); // smoothstep: no visible grid creases
    let a = hash21(i);
    let b = hash21(i + vec2<f32>(1.0, 0.0));
    let c = hash21(i + vec2<f32>(0.0, 1.0));
    let d = hash21(i + vec2<f32>(1.0, 1.0));
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

// Fractional Brownian motion: 5 octaves; each one is rotated and scaled
// (the matrix) so octave grids don't line up. `fine` weights the two
// finest octaves (driven by treble).
fn fbm(p_in: vec2<f32>, fine: f32) -> f32 {
    let m = mat2x2<f32>(1.6, 1.2, -1.2, 1.6);
    var p = p_in;
    var v = 0.0;
    var amp = 0.5;
    for (var i = 0; i < 5; i++) {
        let w = select(1.0, fine, i >= 3);
        v += amp * w * noise(p);
        p = m * p;
        amp *= 0.5;
    }
    return v;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let aspect = g.resolution.x / g.resolution.y;
    var p = (in.uv - 0.5) * vec2<f32>(aspect, 1.0);

    let bass = mix(g.bass, 0.3 + 0.1 * sin(g.time * 0.3), g.idle);
    let t = g.energy_time * 0.06;
    let fine = 0.35 + 0.8 * mix(g.treble, 0.2, g.idle);

    // Beats: a soft circular ripple pushing the domain outwards.
    let r = length(p);
    p += (p / max(r, 1e-4)) * g.beat * 0.025 * sin(r * 10.0 - g.time * 3.0);

    // Bass zooms in a little (bigger, closer clouds).
    p *= 1.9 - 0.35 * bass;

    let q = vec2<f32>(fbm(p + vec2<f32>(0.0, 0.0) + vec2<f32>(0.0, t), fine),
                      fbm(p + vec2<f32>(5.2, 1.3) - vec2<f32>(t, 0.0), fine));
    let w = vec2<f32>(fbm(p + 3.5 * q + vec2<f32>(1.7, 9.2) + 0.6 * t, fine),
                      fbm(p + 3.5 * q + vec2<f32>(8.3, 2.8) - 0.4 * t, fine));
    let f = fbm(p + 3.5 * w, fine);

    // Colouring: dark space, gas where the density is high, hot
    // highlights where the warp is strongest, dark lanes between folds.
    let drift = g.energy_time * 0.01;
    // Two neighbouring hues from a saturated rainbow palette (channels
    // 120° out of phase, so it never passes through grey); the dark
    // background is a tinted version of the gas colour.
    let rainbow_a = vec3<f32>(0.5);
    let rainbow_b = vec3<f32>(0.5);
    let rainbow_d = vec3<f32>(0.0, 0.33, 0.67);
    let gas = palette(drift, rainbow_a, rainbow_b, vec3<f32>(1.0), rainbow_d);
    let hot = palette(drift + 0.17, rainbow_a, rainbow_b, vec3<f32>(1.0), rainbow_d);
    let deep = gas * 0.12;
    let dens = smoothstep(0.36, 0.95, f);
    var c = mix(deep, gas, dens);
    c = mix(c, hot, clamp(dot(q, q) * 0.9 - 0.35, 0.0, 1.0) * dens);
    c = mix(c, c * 0.35, clamp(abs(w.x - 0.5) * 2.0, 0.0, 1.0) * 0.5);

    // Density -> brightness (squared for contrast); bass lifts the cloud.
    c *= dens * dens * (0.6 + 0.5 * bass + 0.08 * g.beat) * 1.6;
    // Darken towards the edges so it feels like looking into depth.
    c *= 1.0 - 0.4 * smoothstep(0.3, 0.9, r);
    return vec4<f32>(c, 1.0);
}
