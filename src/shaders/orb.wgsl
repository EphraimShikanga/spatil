struct OrbUniform {
    rect: vec4<f32>,
    screen: vec4<f32>,
    body_color: vec4<f32>,
    accent_color: vec4<f32>,
    // x: radius in orb units, y/z: first/last dial angle, w: reveal.
    orbit: vec4<f32>,
    seconds_of_day: f32,
    open: f32,
    hover: f32,
    ambient: f32,
    hand: f32,
    dial: f32,
    light: vec2<f32>,
};

@group(0) @binding(0)
var<uniform> orb: OrbUniform;

const PI: f32 = 3.14159265359;
const TAU: f32 = 6.28318530718;
const REST_PAD: f32 = 1.35;
const SECOND_HAND: f32 = 0.86;
const BEZEL: f32 = 0.88;

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

// Signed distance to a hand lying along `dir`, from `tail` (negative, behind the
// hub) to `tip`, whose half-width tapers from `root` at the hub to `point` at the tip.
fn hand_sd(p: vec2<f32>, dir: vec2<f32>, tail: f32, tip: f32, root: f32, point: f32) -> f32 {
    let along = dot(p, dir);
    let across = abs(dot(p, vec2<f32>(dir.y, -dir.x)));
    let h = clamp(along, tail, tip);
    let t = clamp(h / tip, 0.0, 1.0);
    let half_width = mix(root, point, t);
    return length(vec2<f32>(along - h, across)) - half_width;
}

fn indices_sd(p: vec2<f32>) -> f32 {
    let step = TAU / 12.0;
    let a = atan2(p.x, p.y);
    let k = round(a / step);
    let ak = k * step;
    let dir = dial(ak);
    let along = dot(p, dir);
    let across = abs(dot(p, vec2<f32>(dir.y, -dir.x)));

    let quarter = (i32(k) % 3) == 0;
    let inner = select(0.70, 0.62, quarter);
    let half_width = select(0.022, 0.04, quarter);
    let mid = (inner + 0.80) * 0.5;
    let half_len = (0.80 - inner) * 0.5;
    let q = vec2<f32>(across - half_width, abs(along - mid) - half_len);
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0);
}

fn wrap_angle(a: f32) -> f32 {
    return a - TAU * floor((a + PI) / TAU);
}

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    let centre = vec2<f32>(orb.rect.x + orb.rect.z * 0.5, orb.rect.y + orb.rect.w * 0.5);
    let half = max(orb.rect.z, 1.0) * 0.5;
    var pad = REST_PAD;
    if (orb.orbit.w > 0.001) {
        pad = max(pad, orb.orbit.x + 0.2);
    }
    let local = (quad_corner(vertex_index) * 2.0 - 1.0) * (half * pad);
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
    let px = max(fwidth(d), 0.0005);
    let aa = px * 0.75;

    let radius = 1.0 + orb.hover * 0.04;
    let accent = orb.accent_color.rgb;
    let ambient = orb.ambient;
    let lit = 1.0 - ambient;

    // The light is the cursor: `light` is where it sits relative to the dial, already
    // softly clamped. When idle the cursor has become the second hand, so the light
    // fades and the lume takes over.
    let light = orb.light;
    let ldir = normalize(vec3<f32>(light * 1.4 + vec2<f32>(-0.15, 0.25), 0.9));

    var color = vec4<f32>(0.0);

    // Contact shadow on whatever is behind the orb, cast away from the light.
    let shadow_c = p + ldir.xy * 0.09 + vec2<f32>(0.0, 0.07);
    let shadow = 1.0 - smoothstep(radius * 0.9, radius * 1.28, length(shadow_c));
    color = over(color, vec4<f32>(0.0, 0.0, 0.0, shadow * 0.38 * orb.body_color.a));

    let body = fill(d - radius, aa);
    if (body > 0.0) {
        let r = d / radius;
        let normal = normalize(vec3<f32>(p / radius * 0.55, 1.0));
        let diffuse = max(dot(normal, ldir), 0.0);
        let falloff = mix(1.0, 0.62, smoothstep(0.35, 1.0, r));
        let enamel = orb.body_color.rgb
            * (0.82 + 0.38 * diffuse * lit)
            * falloff
            * mix(1.0, 0.6, ambient);
        var face = vec4<f32>(enamel, body * orb.body_color.a);

        let hands_rgb = mix(accent, vec3<f32>(1.0), 0.78);
        let lume_rgb = mix(accent, vec3<f32>(0.92, 1.0, 0.96), 0.55);
        let ink = mix(hands_rgb, lume_rgb, ambient);
        let ink_a = mix(0.92, 1.0, ambient);

        let secs = orb.seconds_of_day;
        let minute = dial(TAU * fract(secs / 3600.0));
        let hour = dial(TAU * fract(secs / 43200.0));

        // Hands float above the enamel; their shadow falls away from the light.
        let lift = -ldir.xy * 0.05 * lit;
        let shadow_hour = hand_sd(p + lift, hour, -0.1, 0.46, 0.062, 0.022);
        let shadow_min = hand_sd(p + lift, minute, -0.16, 0.7, 0.046, 0.014);
        let hand_shadow = fill(min(shadow_hour, shadow_min) - 0.02, 0.05) * orb.dial * lit;
        face = vec4<f32>(face.rgb * (1.0 - hand_shadow * 0.45), face.a);

        let idx = indices_sd(p);
        let idx_m = fill(idx, aa);
        let idx_glow = exp(-max(idx, 0.0) * 28.0) * ambient * 0.35;
        face = over(face, vec4<f32>(ink, max(idx_m * mix(0.55, 0.85, orb.hover + ambient), idx_glow)));

        // Bezel: a polished band lit on the side facing the cursor.
        let bezel = fill(d - radius, aa) * (1.0 - fill(d - radius * BEZEL, aa));
        let facing = 0.5 + 0.5 * dot(normalize(p + vec2<f32>(1e-5)), normalize(ldir.xy + vec2<f32>(1e-5)));
        let metal = mix(orb.body_color.rgb * 1.6 + 0.04, mix(accent, vec3<f32>(1.0), 0.35), 0.35 + 0.4 * orb.hover);
        let bezel_rgb = metal * mix(0.45, 1.25, facing * lit) * mix(1.0, 0.55, ambient);
        face = over(face, vec4<f32>(bezel_rgb, bezel));

        let groove = fill(abs(d - radius * BEZEL) - px * 0.5, aa);
        face = over(face, vec4<f32>(vec3<f32>(0.0), groove * 0.55));

        if (orb.dial > 0.001) {
            let hour_sd = hand_sd(p, hour, -0.1, 0.46, 0.062, 0.022);
            let min_sd = hand_sd(p, minute, -0.16, 0.7, 0.046, 0.014);
            let hands_sd = min(hour_sd, min_sd);
            let glow = exp(-max(hands_sd, 0.0) * 22.0) * ambient * 0.45;
            let hands_m = fill(hands_sd, aa);
            face = over(face, vec4<f32>(ink, max(hands_m * ink_a, glow) * orb.dial));
        }

        if (orb.hand > 0.001) {
            let second = dial(TAU * fract(secs / 60.0));
            let reach = SECOND_HAND * orb.hand;
            let needle = hand_sd(p, second, -0.2 * orb.hand, reach, 0.016, 0.01);
            let weight = length(p + second * 0.13 * orb.hand) - 0.045 * orb.hand;
            let tip = length(p - second * reach) - 0.045;
            let sec_sd = min(min(needle, weight), tip);
            face = over(face, vec4<f32>(accent, fill(sec_sd, aa) * orb.hand));
        }

        let hub = fill(d - 0.085, aa);
        face = over(face, vec4<f32>(ink, hub));
        let jewel = fill(d - 0.04, aa);
        face = over(face, vec4<f32>(accent * mix(1.0, 1.3, ambient), jewel));

        // The crystal sits above everything: a soft glint where the light catches it.
        let glint_c = light * 0.55 + vec2<f32>(-0.12, 0.2) * (1.0 - length(light));
        let glint_d = length((p - glint_c) * vec2<f32>(1.0, 1.35));
        let glint = exp(-glint_d * glint_d * 9.0) * 0.16 * lit * (1.0 - smoothstep(0.8, 1.0, r));
        face = vec4<f32>(face.rgb + vec3<f32>(glint), face.a);

        let fresnel = pow(smoothstep(0.7, 1.0, r), 3.0) * body;
        face = over(face, vec4<f32>(accent, fresnel * (0.18 + 0.3 * orb.hover + 0.25 * orb.open)));

        color = over(color, face);
    }

    // The orbit the modules ride on: a hairline at their exact radius and span.
    if (orb.orbit.w > 0.001 && d > radius) {
        let mid = (orb.orbit.y + orb.orbit.z) * 0.5;
        let half_span = (orb.orbit.z - orb.orbit.y) * 0.5 + 0.22;
        let da = abs(wrap_angle(atan2(p.x, p.y) - mid));
        let reach = half_span * orb.orbit.w;
        let along = 1.0 - smoothstep(reach - 0.25, reach, da);
        let line = fill(abs(d - orb.orbit.x) - px * 0.5, aa);
        color = over(color, vec4<f32>(accent, line * along * 0.45 * orb.orbit.w));
    }

    if (color.a < 0.002) {
        discard;
    }
    return color;
}
