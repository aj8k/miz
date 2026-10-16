// Shared declarations, prepended to every shader by src/gpu/shaders.rs
// (WGSL has no #include). Keep this file small: everything here is
// compiled into every pipeline.

// Must match `Globals` in src/render/params.rs field for field.
struct Globals {
    resolution: vec2<f32>,  // size of the current render target, pixels
    time: f32,              // wall-clock seconds since start
    energy_time: f32,       // seconds that run faster when the music is louder
    dt: f32,                // frame delta, seconds
    bass: f32,              // aggregate band energies, 0..1 (auto-gained)
    low_mid: f32,
    high_mid: f32,
    treble: f32,
    loudness: f32,
    width: f32,             // stereo width: 0 mono .. 1 anti-phase
    beat: f32,              // beat pulse: jumps on onsets, decays smoothly
    idle: f32,              // 0 = music playing .. 1 = silence for a while
    calm: f32,              // 1 = calm mode, 0 = energetic
    band_count: u32,
    beat_count: u32,
    frame: u32,
    gain: f32,              // auto-gain reference mapped to 0..1 (HUD only)
    _pad0: f32,
    _pad1: f32,
};

@group(0) @binding(0) var<uniform> g: Globals;
// Smoothed band levels, low to high, `g.band_count` entries.
@group(0) @binding(1) var<storage, read> bands: array<f32>;
// The last 1024 stereo frames (oldest first), auto-gained to about ±0.8.
@group(0) @binding(2) var<storage, read> wave: array<vec2<f32>>;

const PI: f32 = 3.14159265;
const TAU: f32 = 6.28318531;

// Band level at x in 0..1 (low .. high frequency), linearly interpolated
// between neighbouring bands so curves don't look stepped.
fn band_at(x: f32) -> f32 {
    let last = max(g.band_count, 1u) - 1u;
    let p = clamp(x, 0.0, 1.0) * f32(last);
    let i = u32(floor(p));
    let j = min(i + 1u, last);
    return mix(bands[i], bands[j], fract(p));
}

// Inigo Quilez's cosine palette: smooth, loopable colour ramps from four
// vec3 parameters. https://iquilezles.org/articles/palettes/
fn palette(t: f32, a: vec3<f32>, b: vec3<f32>, c: vec3<f32>, d: vec3<f32>) -> vec3<f32> {
    return a + b * cos(TAU * (c * t + d));
}

// Cheap hash -> 0..1, for dithering and grain.
fn hash12(p: vec2<f32>) -> f32 {
    var p3 = fract(vec3<f32>(p.x, p.y, p.x) * 0.1031);
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    // 0..1 across the target, (0,0) = top-left.
    @location(0) uv: vec2<f32>,
};

// A single triangle that covers the whole viewport (vertices at
// (-1,-1), (3,-1), (-1,3) in clip space). Cheaper than a quad: no diagonal
// seam, no vertex buffer — draw(0..3).
@vertex
fn vs_fullscreen(@builtin(vertex_index) vi: u32) -> VsOut {
    let xy = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    var o: VsOut;
    o.pos = vec4<f32>(xy * 2.0 - 1.0, 0.0, 1.0);
    o.uv = vec2<f32>(xy.x, 1.0 - xy.y);
    return o;
}
