struct OrbUniform {
    rect: vec4<f32>,
    screen: vec4<f32>,
    body_color: vec4<f32>,
    accent_color: vec4<f32>,
    seconds_of_day: f32,
    open: f32,
    hover: f32,
    ambient: f32,
    hand: f32,
};

@group(0) @binding(0)
var<uniform> orb: OrbUniform;

const PAD: f32 = 2.0;
const TAU: f32 = 6.28318530718;
const SECOND_HAND: f32 = 0.86;
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
    let body_a = orb.body_color.a * mix(0.94, 0.80, orb.ambient);
    color = over(color, vec4<f32>(orb.body_color.rgb, body * body_a));

    let rim = ring_mask(d, body_r, 0.035 + orb.hover * 0.02, aa);
    let rim_a = mix(0.30, 0.85, orb.hover) * mix(1.0, 0.7, orb.ambient);
    color = over(color, vec4<f32>(accent, rim * rim_a));

    let secs = orb.seconds_of_day;
    let minute = dial(TAU * fract(secs / 3600.0));
    let hour = dial(TAU * fract(secs / 43200.0));

    let hour_d = seg_dist(p, vec2<f32>(0.0, 0.0), hour * 0.42);
    let hour_m = 1.0 - smoothstep(0.038 - aa, 0.038 + aa, hour_d);
    color = over(color, vec4<f32>(hands_rgb, hour_m * 0.95));

    let min_d = seg_dist(p, vec2<f32>(0.0, 0.0), minute * 0.64);
    let min_m = 1.0 - smoothstep(0.027 - aa, 0.027 + aa, min_d);
    color = over(color, vec4<f32>(hands_rgb, min_m * 0.95));

    if (orb.hand > 0.001) {
        let second = dial(TAU * fract(secs / 60.0));
        let grown = SECOND_HAND * orb.hand;
        let sec_d = seg_dist(p, second * -0.14 * orb.hand, second * grown);
        let sec_m = 1.0 - smoothstep(0.018 - aa, 0.018 + aa, sec_d);
        color = over(color, vec4<f32>(accent, sec_m * orb.hand));

        let tip_d = length(p - second * grown);
        let tip_m = 1.0 - smoothstep(0.05 - aa, 0.05 + aa, tip_d);
        color = over(color, vec4<f32>(mix(accent, vec3<f32>(1.0), 0.35), tip_m * orb.hand));
    }

    let hub = 1.0 - smoothstep(0.06 - aa, 0.06 + aa, d);
    color = over(color, vec4<f32>(hands_rgb, hub * 0.95));

    if (orb.open > 0.001) {
        let r = mix(body_r, OPEN_RING, orb.open);
        let open_m = ring_mask(d, r, 0.11, aa);
        color = over(color, vec4<f32>(accent, open_m * orb.open * 0.9));

        let halo = (1.0 - smoothstep(body_r, r, d)) * step(body_r, d);
        color = over(color, vec4<f32>(orb.body_color.rgb, halo * orb.open * 0.5));
    }

    if (color.a < 0.002) {
        discard;
    }
    return color;
}
