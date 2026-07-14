struct QuadUniform {
    mvp: mat4x4<f32>,
    model: mat4x4<f32>,
    uv_scale: vec2<f32>,
    alpha: f32,
    _pad: f32,
    tint: vec4<f32>,
};

struct Light {
    view_proj: mat4x4<f32>,
};

@group(0) @binding(0)
var<uniform> light: Light;

@group(1) @binding(0)
var<uniform> quad: QuadUniform;

// Depth-only (no fragment stage): quad re-projected from the light.
@vertex
fn vs_main(@location(0) position: vec3<f32>, @location(1) uv: vec2<f32>) -> @builtin(position) vec4<f32> {
    return light.view_proj * quad.model * vec4<f32>(position, 1.0);
}
