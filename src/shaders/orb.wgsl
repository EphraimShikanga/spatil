struct OrbUniform {
    rect: vec4<f32>,
    screen: vec4<f32>,
    body_color: vec4<f32>,
    accent_color: vec4<f32>,
    // The control-centre card in pixels; equal to `rect` while closed.
    card: vec4<f32>,
    seconds_of_day: f32,
    open: f32,
    hover: f32,
    ambient: f32,
    hand: f32,
    dial: f32,
    alert: f32,
    warn: f32,
    // x: card corner radius px, y: smooth-union blend px, z: dial angle facing the screen.
    shape: vec4<f32>,
};

@group(0) @binding(0)
var<uniform> orb: OrbUniform;

const TAU: f32 = 6.28318530718;
const SECOND_HAND: f32 = 0.86;
const SHADOW: f32 = 28.0;
const AMBER: vec3<f32> = vec3<f32>(0.98, 0.66, 0.22);
const INK: vec3<f32> = vec3<f32>(0.955, 0.965, 0.975);

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) pixel: vec2<f32>,
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

fn dial(angle: f32) -> vec2<f32> {
    return vec2<f32>(sin(angle), cos(angle));
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

fn rounded_box(p: vec2<f32>, half_size: vec2<f32>, radius: f32) -> f32 {
    let r = min(radius, min(half_size.x, half_size.y));
    let q = abs(p) - half_size + vec2<f32>(r, r);
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - r;
}

fn smooth_min(a: f32, b: f32, k: f32) -> f32 {
    if (k <= 0.0) {
        return min(a, b);
    }
    let h = clamp(0.5 + 0.5 * (b - a) / k, 0.0, 1.0);
    return mix(b, a, h) - k * h * (1.0 - h);
}

// A hand along `dir` from `tail` (behind the hub) to `tip`, narrowing from `root` to `point`.
fn hand_sd(p: vec2<f32>, dir: vec2<f32>, tail: f32, tip: f32, root: f32, point: f32) -> f32 {
    let along = dot(p, dir);
    let across = abs(dot(p, vec2<f32>(dir.y, -dir.x)));
    let h = clamp(along, tail, tip);
    let half_width = mix(root, point, clamp(h / tip, 0.0, 1.0));
    return length(vec2<f32>(along - h, across)) - half_width;
}

fn indices_sd(p: vec2<f32>) -> vec2<f32> {
    let step = TAU / 12.0;
    let k = round(atan2(p.x, p.y) / step);
    let dir = dial(k * step);
    let along = dot(p, dir);
    let across = abs(dot(p, vec2<f32>(dir.y, -dir.x)));
    let quarter = (i32(k) % 3) == 0;
    let inner = select(0.74, 0.66, quarter);
    let half_width = select(0.02, 0.032, quarter);
    let outer = 0.85;
    let q = vec2<f32>(across - half_width, abs(along - (inner + outer) * 0.5) - (outer - inner) * 0.5);
    let sd = length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0);
    return vec2<f32>(sd, select(0.0, 1.0, quarter));
}

fn arc_mask(p: vec2<f32>, centre_angle: f32, half_span: f32) -> f32 {
    let a = atan2(p.x, p.y) - centre_angle;
    let wrapped = abs(a - TAU * round(a / TAU));
    return 1.0 - smoothstep(half_span - 0.06, half_span, wrapped);
}

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    let orb_lo = orb.rect.xy - vec2<f32>(SHADOW);
    let orb_hi = orb.rect.xy + orb.rect.zw + vec2<f32>(SHADOW);
    let card_lo = orb.card.xy - vec2<f32>(SHADOW);
    let card_hi = orb.card.xy + orb.card.zw + vec2<f32>(SHADOW);
    let lo = min(orb_lo, card_lo);
    let hi = max(orb_hi, card_hi);
    let pixel = mix(lo, hi, quad_corner(vertex_index));

    var out: VertexOutput;
    out.clip_position = vec4<f32>(
        pixel.x / max(orb.screen.x, 1.0) * 2.0 - 1.0,
        1.0 - pixel.y / max(orb.screen.y, 1.0) * 2.0,
        0.0,
        1.0,
    );
    out.pixel = pixel;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let half = max(orb.rect.z, 1.0) * 0.5;
    let centre = orb.rect.xy + vec2<f32>(half);
    let radius_px = half * (1.0 + orb.hover * 0.04);

    let card_half = orb.card.zw * 0.5;
    let card_sd = rounded_box(in.pixel - (orb.card.xy + card_half), card_half, orb.shape.x);
    let orb_sd = length(in.pixel - centre) - radius_px;
    let surface_sd = smooth_min(orb_sd, card_sd, orb.shape.y);

    // Screen-space normal of the surface edge, for the top-lit rim.
    let grad = vec2<f32>(dpdx(surface_sd), dpdy(surface_sd));
    let edge_up = smoothstep(0.2, -0.9, grad.y / max(length(grad), 1e-4));
    let aa_px = max(length(grad), 0.5) * 0.75;

    // Orb-local space: body edge at 1, +Y up, angles clockwise from twelve.
    let p = vec2<f32>(in.pixel.x - centre.x, centre.y - in.pixel.y) / radius_px;
    let d = length(p);
    let aa = aa_px / radius_px;

    let ambient = orb.ambient;
    let accent = orb.accent_color.rgb;
    let lume = mix(accent, vec3<f32>(0.9, 1.0, 0.95), 0.6);
    let body = orb.body_color.rgb;

    var color = vec4<f32>(0.0);

    let blur = mix(10.0, SHADOW - 4.0, orb.open);
    let shadow_sd = smooth_min(
        length(in.pixel - centre - vec2<f32>(0.0, 3.0)) - radius_px,
        rounded_box(in.pixel - (orb.card.xy + card_half) - vec2<f32>(0.0, 6.0), card_half, orb.shape.x),
        orb.shape.y,
    );
    let s = 1.0 - smoothstep(-blur * 0.4, blur, shadow_sd);
    color = vec4<f32>(0.0, 0.0, 0.0, s * s * mix(0.35, 0.5, orb.open) * orb.body_color.a);

    let surface = fill(surface_sd, aa_px);
    if (surface > 0.0) {
        let card_rgb = body * 1.3;
        color = over(color, vec4<f32>(card_rgb, surface * orb.body_color.a));
        let rim = surface * smoothstep(-2.0 * aa_px, 0.0, surface_sd);
        color = over(color, vec4<f32>(mix(vec3<f32>(0.0), vec3<f32>(1.0), edge_up), rim * mix(0.35, 0.12, edge_up)));
    }

    let face = fill(d - 1.0, aa);
    if (face > 0.0) {
        // Multiplicative: blending is linear, so an additive lift reads far brighter than it looks.
        let lift = 1.0 + orb.hover * 0.35 * (1.0 - ambient);
        let shade = mix(1.1, 0.88, smoothstep(0.0, 1.0, d));
        var f = vec4<f32>(body * shade * lift * mix(1.0, 0.5, ambient), face * orb.body_color.a);

        let ring = fill(abs(d - 0.965) - 0.012, aa);
        f = over(f, vec4<f32>(vec3<f32>(1.0), ring * 0.09 * (1.0 - ambient * 0.5)));

        let idx = indices_sd(p);
        let tick_rgb = mix(vec3<f32>(1.0), lume, ambient);
        let tick_a = mix(0.34, 0.62, idx.y) * mix(1.0, 0.85, ambient);
        let tick_glow = exp(-max(idx.x, 0.0) * 90.0) * ambient * 0.25;
        f = over(f, vec4<f32>(tick_rgb, max(fill(idx.x, aa) * tick_a, tick_glow)));

        if (orb.warn > 0.001) {
            let band = fill(abs(d - 0.93) - 0.035, aa) * arc_mask(p, TAU * 0.5, 0.55);
            f = over(f, vec4<f32>(AMBER, band * orb.warn));
        }

        let secs = orb.seconds_of_day;
        let minute = dial(TAU * fract(secs / 3600.0));
        let hour = dial(TAU * fract(secs / 43200.0));
        let ink = mix(INK, lume, ambient);

        if (orb.dial > 0.001) {
            let drop = vec2<f32>(0.0, 0.035);
            let shadow_h = hand_sd(p + drop, hour, 0.0, 0.5, 0.05, 0.034);
            let shadow_m = hand_sd(p + drop, minute, -0.1, 0.78, 0.036, 0.022);
            let hand_shadow = fill(min(shadow_h, shadow_m) - 0.015, 0.06) * (1.0 - ambient);
            f = vec4<f32>(f.rgb * (1.0 - hand_shadow * 0.5), f.a);

            let hands = min(
                hand_sd(p, hour, 0.0, 0.5, 0.05, 0.034),
                hand_sd(p, minute, -0.1, 0.78, 0.036, 0.022),
            );
            let glow = exp(-max(hands, 0.0) * 70.0) * ambient * 0.3;
            f = over(f, vec4<f32>(ink, max(fill(hands, aa), glow) * orb.dial));
        }

        if (orb.hand > 0.001) {
            let second = dial(TAU * fract(secs / 60.0));
            let reach = SECOND_HAND * orb.hand;
            let needle = hand_sd(p, second, -0.22 * orb.hand, reach, 0.016, 0.012);
            let weight = length(p + second * 0.15 * orb.hand) - 0.05 * orb.hand;
            f = over(f, vec4<f32>(accent, fill(min(needle, weight), aa) * orb.hand));
        }

        f = over(f, vec4<f32>(accent, fill(d - 0.075, aa)));
        f = over(f, vec4<f32>(body * 0.6, fill(d - 0.026, aa)));

        color = over(color, f);
    }

    if (orb.alert > 0.001) {
        let pip_c = dial(orb.shape.z) * 0.86;
        let pip_d = length(p - pip_c);
        color = over(color, vec4<f32>(body * 0.5, fill(pip_d - 0.2, aa) * orb.alert));
        color = over(color, vec4<f32>(accent, fill(pip_d - 0.14, aa) * orb.alert));
    }

    if (color.a < 0.002) {
        discard;
    }
    return color;
}
