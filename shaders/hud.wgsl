// Debug HUD, drawn procedurally over the final image (alpha blended).
// Bottom panel: band levels on the left; on the right one meter per row:
// bass, low-mid, high-mid, treble, loudness, width, gain, and the safety
// limiter's activity (empty = idle; it grows while the limiter is holding
// the brightness back) — plus a beat dot.
// Colours here are display (sRGB) values: the target is a non-sRGB swapchain.

@group(1) @binding(0) var<storage, read> lim: Limiter;

fn in_rect(p: vec2<f32>, lo: vec2<f32>, hi: vec2<f32>) -> bool {
    return all(p >= lo) && all(p < hi);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let res = g.resolution;
    let px = in.uv * res; // pixels, origin top-left
    let s = max(res.y / 1080.0, 0.5); // HiDPI-ish scale factor
    let margin = 16.0 * s;
    let pad = 10.0 * s;
    let h = clamp(res.y * 0.22, 100.0 * s, 260.0 * s);
    let lo = vec2<f32>(margin, res.y - margin - h);
    let hi = vec2<f32>(res.x - margin, res.y - margin);
    if !in_rect(px, lo, hi) {
        return vec4<f32>(0.0);
    }

    var col = vec4<f32>(0.02, 0.02, 0.03, 0.62);
    let ilo = lo + pad;
    let ihi = hi - pad;
    let size = ihi - ilo;
    let split = ilo.x + size.x * 0.72;

    if px.x < split - pad {
        // Band bars.
        let n = f32(max(g.band_count, 1u));
        let x = (px.x - ilo.x) / (split - pad - ilo.x);
        let idx = min(u32(x * n), max(g.band_count, 1u) - 1u);
        let y = (ihi.y - px.y) / size.y; // 0 at the bottom
        let level = bands[idx];
        if fract(x * n) < 0.78 && y >= 0.0 && y < level {
            let c = mix(vec3<f32>(0.25, 0.75, 0.85), vec3<f32>(0.98, 0.72, 0.35), y);
            col = vec4<f32>(c, 0.92);
        }
    } else {
        // Meters, one per row.
        let rows = 8.0;
        let fy = (px.y - ilo.y) / size.y * rows;
        if fy < 0.0 || fy >= rows {
            return col; // the panel's padding
        }
        let row = u32(floor(fy));
        let in_row = fract(fy);
        var v = 0.0;
        var c = vec3<f32>(0.8);
        switch row {
            case 0u: { v = g.bass;     c = vec3<f32>(0.95, 0.45, 0.35); }
            case 1u: { v = g.low_mid;  c = vec3<f32>(0.95, 0.75, 0.35); }
            case 2u: { v = g.high_mid; c = vec3<f32>(0.55, 0.85, 0.45); }
            case 3u: { v = g.treble;   c = vec3<f32>(0.40, 0.75, 0.95); }
            case 4u: { v = g.loudness; c = vec3<f32>(0.85, 0.85, 0.85); }
            case 5u: { v = g.width;    c = vec3<f32>(0.75, 0.55, 0.95); }
            case 6u: { v = g.gain;     c = vec3<f32>(0.55, 0.55, 0.60); }
            default: { v = clamp(abs(log2(lim.exposure)), 0.0, 1.0); c = vec3<f32>(1.0, 0.35, 0.3); }
        }
        let beat_d = 0.9 * size.y / rows;
        let x0 = split;
        let x1 = ihi.x - beat_d - pad;
        let fx = (px.x - x0) / (x1 - x0);
        if in_row > 0.2 && in_row < 0.8 && fx >= 0.0 && fx < 1.0 {
            col = select(vec4<f32>(c * 0.25, 0.8), vec4<f32>(c, 0.95), fx < v);
        }
        // Beat dot, top right.
        let centre = vec2<f32>(ihi.x - beat_d * 0.5, ilo.y + beat_d * 0.5);
        let d = length(px - centre) / (beat_d * 0.45);
        if d < 1.0 {
            let b = clamp(g.beat, 0.0, 1.0);
            col = vec4<f32>(mix(vec3<f32>(0.15), vec3<f32>(1.0, 0.85, 0.5), b), 0.95);
        }
    }
    return col;
}
