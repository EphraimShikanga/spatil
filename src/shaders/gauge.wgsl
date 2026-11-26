struct GaugeUniform {
    // xy: screen size, zw: cursor position in pixels (the light).
    screen: vec4<f32>,
    body: vec4<f32>,
    fill: vec4<f32>,
    urgent: vec4<f32>,
};

@group(0) @binding(0)
var<uniform> gauge: GaugeUniform;

const TAU: f32 = 6.28318530718;
// Seat radius as a fraction of the quad; the rest is room for the shadow. 1 / GAUGE_PAD.
const SEAT: f32 = 0.7142857;
const BAND: f32 = 0.8;

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) p: vec2<f32>,
    @location(1) params: vec4<f32>,
    @location(2) light: vec2<f32>,
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

fn fill(sd: f32, aa: f32) -> f32 {
    return 1.0 - smoothstep(-aa, aa, sd);
}

fn over(dst: vec4<f32>, src: vec4<f32>) -> vec4<f32> {
    let a = src.a + dst.a * (1.0 - src.a);
    if (a <= 0.0001) {
        return vec4<f32>(0.0, 0.0, 0.0, 0.0);
    }
    let rgb = (src.rgb * src.a + dst.rgb * dst.a * (1.0 - src.a)) / a;
    return vec4<f32>(rgb, a);
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
    out.p = vec2<f32>(local.x, -local.y) / SEAT;
    out.params = params;

    // Same squash as the orb's `dial_light`, measured from this seat.
    let centre = vec2<f32>(rect.x, rect.y) + vec2<f32>(rect.z, rect.w) * 0.5;
    let seat_px = max(rect.z * 0.5 * SEAT, 1.0);
    let v = vec2<f32>(gauge.screen.z - centre.x, centre.y - gauge.screen.w) / seat_px;
    out.light = v / sqrt(1.0 + dot(v, v));
    return out;
}

// params: x level (0..1), y alpha, z urgent, w 1 when the module has a level.
@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let p = in.p;
    let d = length(p);
    let px = max(fwidth(d), 0.001);
    let aa = px * 0.75;
    let alpha = in.params.y;
    let ldir = normalize(vec3<f32>(in.light * 1.4 + vec2<f32>(-0.15, 0.25), 0.9));

    var color = vec4<f32>(0.0);

    let shadow_c = p + ldir.xy * 0.1 + vec2<f32>(0.0, 0.08);
    let shadow = 1.0 - smoothstep(0.85, 1.3, length(shadow_c));
    color = over(color, vec4<f32>(0.0, 0.0, 0.0, shadow * 0.32));

    let disc = fill(d - 1.0, aa);
    if (disc > 0.0) {
        let normal = normalize(vec3<f32>(p * 0.55, 1.0));
        let diffuse = max(dot(normal, ldir), 0.0);
        let falloff = mix(1.0, 0.65, smoothstep(0.3, 1.0, d));
        var seat = vec4<f32>(gauge.body.rgb * (0.82 + 0.38 * diffuse) * falloff, disc * gauge.body.a);

        let band = disc * (1.0 - fill(d - BAND, aa));
        let facing = 0.5 + 0.5 * dot(normalize(p + vec2<f32>(1e-5)), normalize(ldir.xy + vec2<f32>(1e-5)));
        let metal = (gauge.body.rgb * 1.6 + 0.04) * mix(0.5, 1.2, facing);

        let urgent = in.params.z;
        let turn = fract(atan2(p.x, p.y) / TAU + 1.0);
        let edge = px / max(TAU * d, 0.001);
        let level = clamp(in.params.x, 0.0, 1.0);
        let filled = (1.0 - smoothstep(level - edge, level + edge, turn)) * in.params.w;
        let lit = mix(gauge.fill.rgb, gauge.urgent.rgb, urgent);
        var band_rgb = mix(metal, lit * mix(0.8, 1.1, facing), filled);
        // Attention without a level lights the whole rim.
        band_rgb = mix(band_rgb, gauge.urgent.rgb, urgent * (1.0 - in.params.w));
        seat = over(seat, vec4<f32>(band_rgb, band));

        let groove = fill(abs(d - BAND) - px * 0.5, aa);
        seat = over(seat, vec4<f32>(vec3<f32>(0.0), groove * 0.5));

        let glint_c = in.light * 0.55 + vec2<f32>(-0.12, 0.2) * (1.0 - length(in.light));
        let glint_d = length((p - glint_c) * vec2<f32>(1.0, 1.35));
        let glint = exp(-glint_d * glint_d * 9.0) * 0.12 * (1.0 - smoothstep(0.75, 1.0, d));
        seat = vec4<f32>(seat.rgb + vec3<f32>(glint), seat.a);

        color = over(color, seat);
    }

    if (color.a * alpha < 0.004) {
        discard;
    }
    return vec4<f32>(color.rgb, color.a * alpha);
}
