struct QuadUniform {
    mvp: mat4x4<f32>,
    uv_scale: vec2<f32>,
    _padding: vec2<f32>,
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
    var color = textureSample(t_diffuse, s_diffuse, in.uv);
    // Force opaque: Xrgb8888's alpha byte is undefined, and with alpha blending
    // a src alpha of 0 would make the quad vanish into the background. Writing
    // a=1.0 also keeps Spatil's own framebuffer opaque to the host compositor.
    color.a = 1.0;
    return color;
}
