struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_pos: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) in_vertex_index: u32) -> VertexOutput {
    // A single full-screen triangle
    let uv = vec2<f32>(vec2<u32>(
        (in_vertex_index << 1u) & 2u,
        in_vertex_index & 2u
    ));
    var out: VertexOutput;
    out.clip_position = vec4<f32>(uv * 2.0 - 1.0, 1.0, 1.0); // Z = 1.0 (far clipping plane)
    
    // Scale the uv coordinates to give a nice coordinate space for the dots
    let aspect_ratio = 16.0 / 9.0; // Approximation for shader coordinates
    out.world_pos = out.clip_position.xy * vec2<f32>(aspect_ratio * 40.0, 40.0); 
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let grid_size = 2.0;
    let dot_radius = 0.15;
    let local_pos = fract(in.world_pos / grid_size) * grid_size;
    let dist = distance(local_pos, vec2<f32>(grid_size / 2.0));
    
    // Vignette / depth fade
    let center_dist = length(in.clip_position.xy);
    let fade = smoothstep(1.5, 0.0, center_dist);
    
    if dist < dot_radius {
        let color = mix(vec3<f32>(0.1, 0.1, 0.1), vec3<f32>(0.4, 0.4, 0.4), fade);
        return vec4<f32>(color, 1.0); // Fading dots
    } else {
        let bg_color = mix(vec3<f32>(0.0, 0.0, 0.0), vec3<f32>(0.05, 0.05, 0.05), fade);
        return vec4<f32>(bg_color, 1.0); // Fading background
    }
}
