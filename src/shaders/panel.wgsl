struct PanelUniform {
    screen: vec4<f32>,
    fill: vec4<f32>,
    notch: vec4<f32>,
    tint: vec4<f32>,
};

@group(0) @binding(0)
var<uniform> panel: PanelUniform;

const ALPHA_CUTOFF: f32 = 0.004;
const SHADOW: f32 = 18.0;
const SHADOW_DROP: f32 = 5.0;

const SURFACE: u32 = 0u;
const HIGHLIGHT: u32 = 1u;

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) local: vec2<f32>,
    @location(1) half_size: vec2<f32>,
    @location(2) radius: f32,
    @location(3) alpha: f32,
    @location(4) @interpolate(flat) kind: u32,
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
    let kind = u32(params.z + 0.5);
    // Surfaces grow their quad so the shadow has somewhere to fall.
    let pad = select(0.0, SHADOW, kind == SURFACE);
    let size = vec2<f32>(rect.z, rect.w);
    let corner = quad_corner(vertex_index);
    let pixel = vec2<f32>(rect.x, rect.y) - pad + corner * (size + 2.0 * pad);

    var out: VertexOutput;
    out.clip_position = vec4<f32>(
        pixel.x / max(panel.screen.x, 1.0) * 2.0 - 1.0,
        1.0 - pixel.y / max(panel.screen.y, 1.0) * 2.0,
        0.0,
        1.0,
    );
    out.half_size = size * 0.5;
    out.local = pixel - (vec2<f32>(rect.x, rect.y) + out.half_size);
    out.radius = params.x;
    out.alpha = params.y;
    out.kind = kind;
    return out;
}

// Signed distance to a rounded box, in pixels: negative inside, zero on the edge.
fn rounded_box(p: vec2<f32>, half_size: vec2<f32>, radius: f32) -> f32 {
    let r = min(radius, min(half_size.x, half_size.y));
    let q = abs(p) - half_size + vec2<f32>(r, r);
    return length(max(q, vec2<f32>(0.0, 0.0))) + min(max(q.x, q.y), 0.0) - r;
}

fn over(dst: vec4<f32>, src: vec4<f32>) -> vec4<f32> {
    let a = src.a + dst.a * (1.0 - src.a);
    if (a <= 0.0001) {
        return vec4<f32>(0.0, 0.0, 0.0, 0.0);
    }
    let rgb = (src.rgb * src.a + dst.rgb * dst.a * (1.0 - src.a)) / a;
    return vec4<f32>(rgb, a);
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let d = rounded_box(in.local, in.half_size, in.radius);
    let aa = max(fwidth(d), 0.5);
    let inside = 1.0 - smoothstep(-aa, aa, d);

    var color = vec4<f32>(0.0);

    if (in.kind == SURFACE) {
        let shadow_d = rounded_box(in.local - vec2<f32>(0.0, SHADOW_DROP), in.half_size, in.radius);
        let s = 1.0 - smoothstep(-6.0, SHADOW, shadow_d);
        color = vec4<f32>(0.0, 0.0, 0.0, s * s * 0.4);

        // Same enamel as the orb: lit from above, a little darker toward the bottom.
        let v = clamp(in.local.y / max(in.half_size.y, 1.0), -1.0, 1.0);
        let body = panel.fill.rgb * mix(1.18, 0.9, v * 0.5 + 0.5);
        color = over(color, vec4<f32>(body, panel.fill.a * inside));

        let rim = inside * smoothstep(-2.0 * aa, 0.0, d);
        let top = smoothstep(0.1, -0.9, v);
        let rim_rgb = mix(vec3<f32>(0.0), vec3<f32>(1.0), top);
        color = over(color, vec4<f32>(rim_rgb, rim * mix(0.4, 0.16, top)));
    } else if (in.kind == HIGHLIGHT) {
        color = vec4<f32>(panel.tint.rgb, panel.tint.a * inside);
        let notch_c = in.local - vec2<f32>(-in.half_size.x + 5.0, 0.0);
        let notch_d = rounded_box(notch_c, vec2<f32>(1.5, in.half_size.y * 0.42), 1.5);
        let notch = 1.0 - smoothstep(-aa, aa, notch_d);
        color = over(color, vec4<f32>(panel.notch.rgb, panel.notch.a * notch));
    } else {
        color = vec4<f32>(1.0, 1.0, 1.0, 0.09 * inside);
    }

    let a = color.a * in.alpha;
    if (a < ALPHA_CUTOFF) {
        discard;
    }
    return vec4<f32>(color.rgb, a);
}
