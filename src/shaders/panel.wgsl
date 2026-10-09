struct PanelUniform {
    screen: vec4<f32>,
    fill: vec4<f32>,
    border: vec4<f32>,
    accent: vec4<f32>,
};

@group(0) @binding(0)
var<uniform> panel: PanelUniform;

const BORDER: f32 = 1.0;
const ALPHA_CUTOFF: f32 = 0.004;

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) local: vec2<f32>,
    @location(1) half_size: vec2<f32>,
    @location(2) radius: f32,
    @location(3) alpha: f32,
    @location(4) highlight: f32,
};

fn quad_corner(vertex_index: u32) -> vec2<f32> {
    let tri = vertex_index / 3u;
    let k = vertex_index % 3u;
    var c = vec2<f32>(0.0, 0.0);
    if (tri == 0u) {
        if (k == 1u) { c = vec2<f32>(1.0, 0.0); }
        else if (k == 2u) { c = vec2<f32>(0.0, 1.0); }
    } else {
        if (k == 0u) { c = vec2<f32>(1.0, 0.0); }
        else if (k == 1u) { c = vec2<f32>(1.0, 1.0); }
        else { c = vec2<f32>(0.0, 1.0); }
    }
    return c;
}

@vertex
fn vs_main(
    @builtin(vertex_index) vertex_index: u32,
    @location(0) rect: vec4<f32>,
    @location(1) params: vec4<f32>,
) -> VertexOutput {
    let corner = quad_corner(vertex_index);
    let pixel = vec2<f32>(rect.x, rect.y) + corner * vec2<f32>(rect.z, rect.w);

    var out: VertexOutput;
    out.clip_position = vec4<f32>(
        pixel.x / max(panel.screen.x, 1.0) * 2.0 - 1.0,
        1.0 - pixel.y / max(panel.screen.y, 1.0) * 2.0,
        0.0,
        1.0,
    );
    out.half_size = vec2<f32>(rect.z, rect.w) * 0.5;
    out.local = (corner - 0.5) * vec2<f32>(rect.z, rect.w);
    out.radius = params.x;
    out.alpha = params.y;
    out.highlight = params.z;
    return out;
}

// Signed distance to a rounded box, in pixels: negative inside, zero on the edge.
fn rounded_box(p: vec2<f32>, half_size: vec2<f32>, radius: f32) -> f32 {
    let r = min(radius, min(half_size.x, half_size.y));
    let q = abs(p) - half_size + vec2<f32>(r, r);
    return length(max(q, vec2<f32>(0.0, 0.0))) + min(max(q.x, q.y), 0.0) - r;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let d = rounded_box(in.local, in.half_size, in.radius);
    let aa = max(fwidth(d), 0.5);

    let inside = 1.0 - smoothstep(-aa, aa, d);
    if (inside < ALPHA_CUTOFF) {
        discard;
    }
    let edge = 1.0 - smoothstep(-aa, aa, d + BORDER);
    let body = mix(panel.fill, panel.accent, in.highlight);
    let color = mix(panel.border, body, edge);
    return vec4<f32>(color.rgb, color.a * inside * in.alpha);
}
