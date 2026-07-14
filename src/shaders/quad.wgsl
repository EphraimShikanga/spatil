struct QuadUniform {
    mvp: mat4x4<f32>,
    model: mat4x4<f32>,
    uv_scale: vec2<f32>,
    alpha: f32,
    _pad: f32,
    tint: vec4<f32>,
};
@group(0) @binding(0)
var<uniform> quad: QuadUniform;

@group(0) @binding(1)
var t_diffuse: texture_2d<f32>;
@group(0) @binding(2)
var s_diffuse: sampler;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) uv: vec2<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(model: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.uv = model.uv * quad.uv_scale;
    out.clip_position = quad.mvp * vec4<f32>(model.position, 1.0);
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let surface = textureSample(t_diffuse, s_diffuse, in.uv);
    let rgb = mix(surface.rgb, quad.tint.rgb, quad.tint.a);
    // Opacity comes from the uniform, never the sampled alpha: Xrgb8888's alpha
    // byte is undefined. Strata passes alpha=1 (opaque, as before); Aether passes
    // glass_alpha for translucent panes.
    return vec4<f32>(rgb, quad.alpha);
}
