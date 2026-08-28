struct GaugeUniform {
    screen: vec4<f32>,
    track: vec4<f32>,
    fill: vec4<f32>,
    urgent: vec4<f32>,
};

@group(0) @binding(0)
var<uniform> gauge: GaugeUniform;

const TAU: f32 = 6.28318530718;
const RADIUS: f32 = 0.82;
const THICKNESS: f32 = 0.11;

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) p: vec2<f32>,
    @location(1) level: f32,
    @location(2) alpha: f32,
    @location(3) urgent: f32,
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
        pixel.x / max(gauge.screen.x, 1.0) * 2.0 - 1.0,
        1.0 - pixel.y / max(gauge.screen.y, 1.0) * 2.0,
        0.0,
        1.0,
    );
    let local = corner * 2.0 - 1.0;
    out.p = vec2<f32>(local.x, -local.y);
    out.level = params.x;
    out.alpha = params.y;
    out.urgent = params.z;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let d = length(in.p);
    let aa = max(fwidth(d), 0.002) * 1.2;

    let band = 1.0 - smoothstep(THICKNESS * 0.5 - aa, THICKNESS * 0.5 + aa, abs(d - RADIUS));
    if (band < 0.004) {
        discard;
    }

    let angle = atan2(in.p.x, in.p.y);
    let turn = select(angle, angle + TAU, angle < 0.0) / TAU;

    let filled = step(turn, clamp(in.level, 0.0, 1.0));
    let lit = mix(gauge.fill, gauge.urgent, in.urgent);
    let color = mix(gauge.track, lit, filled);
    return vec4<f32>(color.rgb, color.a * band * in.alpha);
}
