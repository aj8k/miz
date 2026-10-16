// Scene transition: blends the outgoing scene (group 1) into the incoming
// one (group 2). g.transition is already eased (smoothstep) on the CPU.

@group(1) @binding(0) var from_tex: texture_2d<f32>;
@group(1) @binding(1) var from_smp: sampler;
@group(2) @binding(0) var to_tex: texture_2d<f32>;
@group(2) @binding(1) var to_smp: sampler;

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let a = textureSample(from_tex, from_smp, in.uv).rgb;
    let b = textureSample(to_tex, to_smp, in.uv).rgb;
    return vec4<f32>(mix(a, b, g.transition), 1.0);
}
