// Phosphor decay for the oscilloscope. The shader outputs nothing useful:
// the work is done by the blend state (src * 0 + dst * constant), so the
// persistence buffer is multiplied by exp(-dt/tau) *in place* — no second
// buffer needed, unlike the feedback warp, which has to *move* pixels.

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return vec4<f32>(0.0);
}
