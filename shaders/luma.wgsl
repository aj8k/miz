// Photosensitivity safety: limit how fast the picture's *average*
// brightness may change, whatever the scenes do.
//
// Pass 1 (`reduce`): each 16x16 workgroup sums the luminance of a sparse
// grid of pixels (every 4th in x and y) in fast workgroup memory, then adds
// its partial sum to a global atomic counter. Atomics only exist for
// integers, so the sum is stored in fixed point.
//
// Pass 2 (`update`, one thread): turns the sum into a mean and moves the
// "allowed" mean towards it by at most `safety_rate * dt`. The ratio
// allowed / real becomes the exposure the present pass applies — in the
// same frame, so a sudden flash is already damped on its first frame.
//
// Rate 0.25/s means: a brightness swing of 10% of full white takes at
// least 0.4 s each way, so at most ~1.25 such flashes per second — well
// under the 3/s guideline (WCAG 2.3.1). Calm mode uses a lower rate.

const STRIDE: u32 = 4u;
const FIXED: f32 = 256.0;     // fixed-point scale of the atomic sum
const MAX_LUMA: f32 = 4.0;    // clamp HDR outliers so the sum can't overflow

@group(1) @binding(0) var scene: texture_2d<f32>;
@group(1) @binding(1) var<storage, read_write> sum: atomic<u32>;
@group(1) @binding(2) var<storage, read_write> lim: Limiter;

var<workgroup> partial: array<f32, 256>;

@compute @workgroup_size(16, 16)
fn reduce(@builtin(global_invocation_id) gid: vec3<u32>,
          @builtin(local_invocation_index) li: u32) {
    let dims = textureDimensions(scene);
    let p = gid.xy * STRIDE;
    var l = 0.0;
    if p.x < dims.x && p.y < dims.y {
        let c = textureLoad(scene, p, 0).rgb;
        l = min(dot(c, vec3<f32>(0.2126, 0.7152, 0.0722)), MAX_LUMA);
    }
    partial[li] = l;
    workgroupBarrier();
    // Tree reduction: 256 -> 128 -> ... -> 1, halving the active threads.
    for (var step = 128u; step > 0u; step = step >> 1u) {
        if li < step {
            partial[li] += partial[li + step];
        }
        workgroupBarrier();
    }
    if li == 0u {
        atomicAdd(&sum, u32(partial[0] * FIXED));
    }
}

@compute @workgroup_size(1)
fn update() {
    let dims = textureDimensions(scene);
    let samples = f32(((dims.x + STRIDE - 1u) / STRIDE) * ((dims.y + STRIDE - 1u) / STRIDE));
    let mean = f32(atomicLoad(&sum)) / FIXED / samples;
    atomicStore(&sum, 0u); // ready for the next frame

    if lim.initialized < 0.5 {
        lim.adapted = mean;
        lim.initialized = 1.0;
    }
    let step = g.safety_rate * g.dt;
    lim.adapted = clamp(mean, lim.adapted - step, lim.adapted + step);
    // Bounded both ways: down to 1/50 (enough to swallow even a full-screen
    // black/white strobe) and up to 2x (slowing a sudden blackout).
    lim.exposure = clamp(lim.adapted / max(mean, 1e-5), 0.02, 2.0);
    lim.mean = mean;
}
