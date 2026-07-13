struct GridUniform {
    inv_view_proj: mat4x4<f32>,
    light_view_proj: mat4x4<f32>,
    camera_eye: vec4<f32>,
    grid_color: vec4<f32>,
    void_color: vec4<f32>,
    axis_color: vec4<f32>,
    grid_half_width: f32,
    grid_horizon_dist: f32,
    grid_spacing: f32,
    enable_shadows: f32,
};

@group(0) @binding(0)
var<uniform> grid: GridUniform;

@group(1) @binding(0)
var shadow_map: texture_depth_2d;
@group(1) @binding(1)
var shadow_sampler: sampler_comparison;

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) ndc: vec2<f32>,
};

// Full-screen triangle at Z=1.0 (far plane); LessEqual depth lets windows win.
@vertex
fn vs_main(@builtin(vertex_index) in_vertex_index: u32) -> VertexOutput {
    let uv = vec2<f32>(
        f32((in_vertex_index << 1u) & 2u),
        f32(in_vertex_index & 2u),
    );
    var out: VertexOutput;
    let ndc = uv * 2.0 - 1.0;
    out.clip_position = vec4<f32>(ndc, 1.0, 1.0);
    out.ndc = ndc;
    return out;
}

// Light reaching a floor point: 0 = shadowed, 1 = lit. Outside the frustum = lit.
fn floor_shadow(world_pos: vec3<f32>) -> f32 {
    if grid.enable_shadows < 0.5 {
        return 1.0;
    }
    let light_clip = grid.light_view_proj * vec4<f32>(world_pos, 1.0);
    let proj = light_clip.xyz / light_clip.w;
    let uv = vec2<f32>(proj.x * 0.5 + 0.5, proj.y * -0.5 + 0.5);
    if uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0 || proj.z > 1.0 {
        return 1.0;
    }
    // 0.002 bias against self-shadow acne.
    return textureSampleCompare(shadow_map, shadow_sampler, uv, proj.z - 0.002);
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // World-space camera ray via the inverse view-projection (near z=0, far z=1).
    let near_h = grid.inv_view_proj * vec4<f32>(in.ndc, 0.0, 1.0);
    let far_h = grid.inv_view_proj * vec4<f32>(in.ndc, 1.0, 1.0);
    let near = near_h.xyz / near_h.w;
    let far = far_h.xyz / far_h.w;
    let origin = grid.camera_eye.xyz;
    let dir = normalize(far - near);

    if abs(dir.y) < 1e-6 || (-origin.y / dir.y) <= 0.0 {
        return grid.void_color;
    }
    let t = -origin.y / dir.y;
    let hit = origin + t * dir;

    if abs(hit.x) > grid.grid_half_width {
        return grid.void_color;
    }

    // fwidth keeps lines ~1px wide at any depth. The center axes (coord == 0)
    // are drawn thicker and in axis_color, on top of the regular grid.
    let coord = hit.xz / grid.grid_spacing;
    let deriv = fwidth(coord);
    let line_dist = abs(fract(coord - 0.5) - 0.5) / deriv;
    let grid_i = 1.0 - min(min(line_dist.x, line_dist.y), 1.0);
    let axis_dist = abs(coord) / deriv;
    let axis_i = 1.0 - smoothstep(0.0, 2.0, min(axis_dist.x, axis_dist.y));

    let cam_dist = length(hit.xz - grid.camera_eye.xz);
    let horizon = 1.0 - smoothstep(grid.grid_horizon_dist * 0.5, grid.grid_horizon_dist, cam_dist);

    var color = mix(grid.void_color.rgb, grid.grid_color.rgb, grid_i * horizon);
    color = mix(color, grid.axis_color.rgb, axis_i * horizon);
    color *= mix(0.4, 1.0, floor_shadow(hit));
    return vec4<f32>(color, 1.0);
}
