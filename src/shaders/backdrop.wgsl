struct BackdropUniform {
    // rgb = dim colour (linear), a = how far the world is darkened.
    color: vec4<f32>,
};
@group(0) @binding(0)
var<uniform> backdrop: BackdropUniform;

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> @builtin(position) vec4<f32> {
    // Full-screen triangle, no vertex buffer.
    let x = f32(i32(vertex_index) / 2) * 4.0 - 1.0;
    let y = f32(i32(vertex_index) & 1) * 4.0 - 1.0;
    return vec4<f32>(x, y, 0.0, 1.0);
}

@fragment
fn fs_main() -> @location(0) vec4<f32> {
    return backdrop.color;
}
