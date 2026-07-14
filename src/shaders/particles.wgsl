struct ParticleUniform {
    view_proj: mat4x4<f32>,
    camera_right: vec4<f32>,
    camera_up: vec4<f32>,
    camera_eye: vec4<f32>,
    bg_color: vec4<f32>,
    color: vec4<f32>,
    size: f32,
    fog_density: f32,
    _pad: vec2<f32>,
};

@group(0) @binding(0)
var<uniform> u: ParticleUniform;

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) offset: vec2<f32>,
    @location(1) dist: f32,
};

// Each particle is one camera-facing quad; the corner is picked from the vertex
// index and offset in world space by the camera basis, so a fixed world size
// gives perspective size-attenuation (near = bigger) for free.
@vertex
fn vs_main(@builtin(vertex_index) vi: u32, @location(0) center: vec3<f32>) -> VertexOutput {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, 1.0),
    );
    let c = corners[vi];
    let world = center + (u.camera_right.xyz * c.x + u.camera_up.xyz * c.y) * u.size;
    var out: VertexOutput;
    out.clip_position = u.view_proj * vec4<f32>(world, 1.0);
    out.offset = c;
    out.dist = length(center - u.camera_eye.xyz);
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let d = length(in.offset);
    let alpha = (1.0 - smoothstep(0.7, 1.0, d)) * u.color.a;
    // Exponential fog fades distant particles into the void for depth.
    let fog = exp(-u.fog_density * in.dist);
    let rgb = mix(u.bg_color.rgb, u.color.rgb, fog);
    return vec4<f32>(rgb, alpha * fog);
}
