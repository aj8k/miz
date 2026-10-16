// Feedback warp — the MilkDrop core.
//
// Every frame we draw the *previous* frame back onto the screen, slightly
// transformed: zoomed in a touch (so everything drifts outwards), rotated a
// little, displaced by a gentle wave field, blurred a hair, darkened and
// hue-shifted. Then the fresh waveform is drawn on top (feedback_wave.wgsl).
// Repeated 72 times a second, these tiny steps turn a single line into
// tunnels, spirals and smoke trails.
//
// All amounts are *per second* and scaled by g.dt, so the look doesn't
// depend on the frame rate.

@group(1) @binding(0) var prev_tex: texture_2d<f32>;
@group(1) @binding(1) var prev_smp: sampler;

fn rot2(a: f32) -> mat2x2<f32> {
    let c = cos(a);
    let s = sin(a);
    return mat2x2<f32>(c, s, -s, c);
}

// Rotates a colour's hue by angle `a`: a rotation of RGB space around the
// grey axis (1,1,1)/√3 (Rodrigues' formula). Brightness stays the same.
fn hue_rotate(c: vec3<f32>, a: f32) -> vec3<f32> {
    let k = vec3<f32>(0.57735027);
    let cs = cos(a);
    return c * cs + cross(k, c) * sin(a) + k * dot(k, c) * (1.0 - cs);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let aspect = g.resolution.x / g.resolution.y;
    let dt = g.dt;
    let t = g.energy_time;
    let calm = g.calm;

    // Aspect-corrected coordinates around the centre: a circle stays round.
    var p = (in.uv - 0.5) * vec2<f32>(aspect, 1.0);
    let r = length(p);

    // Zoom: sampling slightly *closer* to the centre makes the image flow
    // outwards. Bass pushes it; a beat gives a small extra shove.
    let zoom_speed = 0.16 + 0.55 * g.bass + mix(0.35, 0.15, calm) * g.beat;
    p *= exp(-dt * zoom_speed);

    // Rotation driven by the mids. Its direction reverses every few
    // minutes (slow sine of energy time), so spirals don't wind forever.
    let spin = (0.05 + 0.22 * g.low_mid) * sin(t * 0.04 + 1.3);
    // Plus a radius-dependent swirl: rings at different distances turn by
    // different amounts, which bends straight trails into soft spirals.
    let swirl = 0.10 * sin(r * 7.0 - t * 0.45) * (0.3 + g.high_mid);
    p = rot2(dt * (spin + swirl)) * p;

    // A slow wave field: liquid, "breathing" distortion. Treble adds a
    // little more ripple.
    let wobble = (0.025 + 0.06 * g.high_mid + 0.04 * g.treble) * mix(1.4, 1.0, calm);
    p += dt * wobble * vec2<f32>(sin(p.y * 6.0 + t * 0.7), sin(p.x * 6.0 - t * 0.6));

    let uv = p / vec2<f32>(aspect, 1.0) + 0.5;

    // Sample with a mild 5-tap blur: trails soften into smoke as they age.
    // The blur strength is per second, like everything else.
    let px = 1.0 / g.resolution;
    let centre = textureSample(prev_tex, prev_smp, uv).rgb;
    let around = 0.25 * (
        textureSample(prev_tex, prev_smp, uv + vec2<f32>( px.x,  px.y)).rgb +
        textureSample(prev_tex, prev_smp, uv + vec2<f32>(-px.x,  px.y)).rgb +
        textureSample(prev_tex, prev_smp, uv + vec2<f32>( px.x, -px.y)).rgb +
        textureSample(prev_tex, prev_smp, uv + vec2<f32>(-px.x, -px.y)).rgb);
    var c = mix(centre, around, clamp(dt * 5.0, 0.0, 0.5));

    // Anything that came from outside the previous frame (rotated corners)
    // fades to black instead of smearing the clamped edge pixels.
    let edge = min(min(uv.x, 1.0 - uv.x), min(uv.y, 1.0 - uv.y));
    c *= smoothstep(-0.01, 0.0, edge);

    // Decay: exponential fade with a time constant in seconds. Calm mode
    // keeps trails longer. Rgba16Float makes this smooth — in 8-bit, slow
    // fades get stuck on the last few steps and leave "burnt-in" pixels.
    let decay_tau = mix(0.45, 0.7, calm);
    c *= exp(-dt / decay_tau);

    // Colour drift: hues rotate slowly as trails age, so a tunnel shows a
    // gentle rainbow from the centre outwards.
    c = hue_rotate(c, dt * mix(0.8, 0.5, calm));

    return vec4<f32>(c, 1.0);
}
