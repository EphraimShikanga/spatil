struct OrbUniform {
    rect: vec4<f32>,
    screen: vec4<f32>,
    body_color: vec4<f32>,
    accent_color: vec4<f32>,
    seconds_of_day: f32,
    open: f32,
    hover: f32,
    ambient: f32,
};

@group(0) @binding(0)
var<uniform> orb: OrbUniform;

const PAD: f32 = 2.0;
const TAU: f32 = 6.28318530718;
const DOT_ORBIT: f32 = 1.22;
const DOT_RADIUS: f32 = 0.105;
const OPEN_RING: f32 = 1.62;

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) p: vec2<f32>,
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

fn seg_dist(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>) -> f32 {
    let pa = p - a;
    let ba = b - a;
    let h = clamp(dot(pa, ba) / max(dot(ba, ba), 1e-6), 0.0, 1.0);
    return length(pa - ba * h);
}

fn dial(angle: f32) -> vec2<f32> {
    return vec2<f32>(sin(angle), cos(angle));
}

fn ring_mask(d: f32, radius: f32, thickness: f32, aa: f32) -> f32 {
    let x = abs(d - radius) - thickness * 0.5;
    return 1.0 - smoothstep(-aa, aa, x);
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
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    let centre = vec2<f32>(orb.rect.x + orb.rect.z * 0.5, orb.rect.y + orb.rect.w * 0.5);
    let half = max(orb.rect.z, 1.0) * 0.5;
    let local = (quad_corner(vertex_index) * 2.0 - 1.0) * (half * PAD);
    let pixel = centre + local;

    var out: VertexOutput;
    out.clip_position = vec4<f32>(
        pixel.x / max(orb.screen.x, 1.0) * 2.0 - 1.0,
        1.0 - pixel.y / max(orb.screen.y, 1.0) * 2.0,
        0.0,
        1.0,
    );
    out.p = vec2<f32>(local.x, -local.y) / half;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let p = in.p;
    let d = length(p);
    let aa = max(fwidth(d), 0.0005) * 1.1;

    let body_r = 1.0 + orb.hover * 0.06 + orb.open * 0.04;
    let accent = orb.accent_color.rgb;
    let hands_rgb = mix(accent, vec3<f32>(1.0, 1.0, 1.0), 0.55);

    var color = vec4<f32>(0.0, 0.0, 0.0, 0.0);

    let body = 1.0 - smoothstep(body_r - aa, body_r + aa, d);
    let body_a = orb.body_color.a * mix(0.94, 0.58, orb.ambient);
    color = over(color, vec4<f32>(orb.body_color.rgb, body * body_a));

    let rim = ring_mask(d, body_r, 0.035 + orb.hover * 0.02, aa);
    let rim_a = mix(0.30, 0.85, orb.hover) * mix(1.0, 0.6, orb.ambient);
    color = over(color, vec4<f32>(accent, rim * rim_a));

    let secs = orb.seconds_of_day;
    let minute = dial(TAU * fract(secs / 3600.0));
    let hour = dial(TAU * fract(secs / 43200.0));
    let hand_a = mix(0.95, 0.22, orb.ambient);

    let hour_d = seg_dist(p, vec2<f32>(0.0, 0.0), hour * 0.42);
    let hour_m = 1.0 - smoothstep(0.038 - aa, 0.038 + aa, hour_d);
    color = over(color, vec4<f32>(hands_rgb, hour_m * hand_a));

    let min_d = seg_dist(p, vec2<f32>(0.0, 0.0), minute * 0.64);
    let min_m = 1.0 - smoothstep(0.027 - aa, 0.027 + aa, min_d);
    color = over(color, vec4<f32>(hands_rgb, min_m * hand_a));

    let hub = 1.0 - smoothstep(0.06 - aa, 0.06 + aa, d);
    color = over(color, vec4<f32>(hands_rgb, hub * hand_a));

    let track = ring_mask(d, DOT_ORBIT, 0.012, aa);
    color = over(color, vec4<f32>(accent, track * mix(0.10, 0.34, orb.ambient)));

    let dot_pos = dial(TAU * fract(secs / 60.0)) * DOT_ORBIT;
    let dot_d = length(p - dot_pos);
    let dot_a = mix(0.5, 1.0, orb.ambient);
    let glow = (1.0 - smoothstep(DOT_RADIUS, DOT_RADIUS * 3.2, dot_d)) * 0.35 * dot_a;
    color = over(color, vec4<f32>(accent, glow));
    let dot_m = 1.0 - smoothstep(DOT_RADIUS - aa, DOT_RADIUS + aa, dot_d);
    color = over(color, vec4<f32>(mix(accent, vec3<f32>(1.0), 0.35), dot_m * dot_a));

    if (orb.open > 0.001) {
        let r = mix(body_r, OPEN_RING, orb.open);
        let open_m = ring_mask(d, r, 0.03, aa);
        color = over(color, vec4<f32>(accent, open_m * orb.open * 0.8));
    }

    if (color.a < 0.002) {
        discard;
    }
    return color;
}
