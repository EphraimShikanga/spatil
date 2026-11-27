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
    // x: card corner radius px, y: smooth-union blend px, z: dial angle facing the screen, w: focus.
    shape: vec4<f32>,
    // fold, park, voice, tint
    agent_a: vec4<f32>,
    // dim, step alpha, step angle, ring hub
    agent_b: vec4<f32>,
    // solid hub, arc alpha, arc fraction, privacy
    agent_c: vec4<f32>,
    marks: array<vec4<f32>, 3>,
    ink: vec4<f32>,
    hand_ink: vec4<f32>,
    dark: vec4<f32>,
    amber: vec4<f32>,
    quiet: vec4<f32>,
    deep: vec4<f32>,
    // The now-playing crown and its strip, in pixels.
    crown: vec4<f32>,
    crown_art: vec4<f32>,
    // alpha, art present, crown radius, art radius
    crown_style: vec4<f32>,
    strip: vec4<f32>,
    // alpha, raised (shadow and top highlight), playing, radius
    strip_style: vec4<f32>,
    strip_color: vec4<f32>,
    strip_art: vec4<f32>,
    // pause/play centre, next centre
    glyphs: vec4<f32>,
    // x, y, width, played fraction
    line: vec4<f32>,
};

@group(0) @binding(0)
var<uniform> orb: OrbUniform;
@group(0) @binding(1)
var art_texture: texture_2d<f32>;
@group(0) @binding(2)
var art_sampler: sampler;

const TAU: f32 = 6.28318530718;
const SECOND_HAND: f32 = 0.86;
const SHADOW: f32 = 28.0;
// Design units are px at a 56px orb; the shader's face has radius 1.
const UNIT: f32 = 1.0 / 28.0;
const MARK_OUTER: f32 = 23.8 * UNIT;

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

fn to_srgb(c: vec3<f32>) -> vec3<f32> {
    let hi = 1.055 * pow(max(c, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(hi, c * 12.92, c <= vec3<f32>(0.0031308));
}

fn to_linear(c: vec3<f32>) -> vec3<f32> {
    let hi = pow((max(c, vec3<f32>(0.0)) + 0.055) / 1.055, vec3<f32>(2.4));
    return select(hi, c / 12.92, c <= vec3<f32>(0.04045));
}

// The design's translucent strokes composite in sRGB, as a browser does; in linear light the
// same alpha reads several times brighter.
fn veil(base: vec3<f32>, toward: vec3<f32>, t: f32) -> vec3<f32> {
    return to_linear(mix(to_srgb(base), to_srgb(toward), clamp(t, 0.0, 1.0)));
}

// A round-capped stroke along `dir` from radius `inner` to `outer`.
fn radial_sd(p: vec2<f32>, dir: vec2<f32>, inner: f32, outer: f32, half_width: f32) -> f32 {
    let along = clamp(dot(p, dir), inner, outer);
    return length(p - dir * along) - half_width;
}

// Distance to a round-capped arc of `radius` running clockwise from `start` for `span`.
fn arc_sd(p: vec2<f32>, radius: f32, start: f32, span: f32) -> f32 {
    let a = fract((atan2(p.x, p.y) - start) / TAU) * TAU;
    if (a <= span) {
        return abs(length(p) - radius);
    }
    let ends = min(length(p - dial(start) * radius), length(p - dial(start + span) * radius));
    return ends;
}

fn box_sd(pixel: vec2<f32>, rect: vec4<f32>, radius: f32) -> f32 {
    let half = rect.zw * 0.5;
    return rounded_box(pixel - (rect.xy + half), half, radius);
}

fn tri_sd(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>, c: vec2<f32>) -> f32 {
    let e0 = b - a;
    let e1 = c - b;
    let e2 = a - c;
    let v0 = p - a;
    let v1 = p - b;
    let v2 = p - c;
    let q0 = v0 - e0 * clamp(dot(v0, e0) / dot(e0, e0), 0.0, 1.0);
    let q1 = v1 - e1 * clamp(dot(v1, e1) / dot(e1, e1), 0.0, 1.0);
    let q2 = v2 - e2 * clamp(dot(v2, e2) / dot(e2, e2), 0.0, 1.0);
    let s = sign(e0.x * e2.y - e0.y * e2.x);
    let d = min(
        min(vec2<f32>(dot(q0, q0), s * (v0.x * e0.y - v0.y * e0.x)), vec2<f32>(dot(q1, q1), s * (v1.x * e1.y - v1.y * e1.x))),
        vec2<f32>(dot(q2, q2), s * (v2.x * e2.y - v2.y * e2.x)),
    );
    return -sqrt(d.x) * sign(d.y);
}

// Crown.dc.html's 14px glyphs, drawn in their own 24-unit viewBox; returns pixels.
fn glyph_sd(pixel: vec2<f32>, centre: vec2<f32>, which: i32) -> f32 {
    let scale = 14.0 / 24.0;
    let q = (pixel - centre) / scale + vec2<f32>(12.0);
    var d = 1e4;
    if (which == 0) {
        d = min(rounded_box(q - vec2<f32>(8.0, 12.0), vec2<f32>(2.0, 7.0), 1.0),
                rounded_box(q - vec2<f32>(16.0, 12.0), vec2<f32>(2.0, 7.0), 1.0));
    } else if (which == 1) {
        d = tri_sd(q, vec2<f32>(8.0, 5.0), vec2<f32>(8.0, 19.0), vec2<f32>(19.0, 12.0));
    } else {
        d = min(tri_sd(q, vec2<f32>(6.0, 5.0), vec2<f32>(15.0, 12.0), vec2<f32>(6.0, 19.0)),
                rounded_box(q - vec2<f32>(17.25, 12.0), vec2<f32>(1.25, 7.0), 1.0));
    }
    return d * scale;
}

// Album art filling `rect` like `object-fit: cover`, or the empty slot colour without art.
fn art_at(pixel: vec2<f32>, rect: vec4<f32>) -> vec3<f32> {
    if (orb.crown_style.y < 0.5) {
        return orb.deep.rgb;
    }
    var uv = (pixel - rect.xy) / max(rect.zw, vec2<f32>(1.0));
    let aspect = rect.z / max(rect.w, 1.0);
    if (aspect < 1.0) {
        uv.x = 0.5 + (uv.x - 0.5) * aspect;
    } else {
        uv.y = 0.5 + (uv.y - 0.5) / aspect;
    }
    return textureSampleLevel(art_texture, art_sampler, uv, 0.0).rgb;
}

fn mark_level(k: i32) -> f32 {
    let i = ((k % 12) + 12) % 12;
    return orb.marks[i / 4][i % 4];
}

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    let orb_lo = orb.rect.xy - vec2<f32>(SHADOW);
    let orb_hi = orb.rect.xy + orb.rect.zw + vec2<f32>(SHADOW);
    let card_lo = orb.card.xy - vec2<f32>(SHADOW);
    let card_hi = orb.card.xy + orb.card.zw + vec2<f32>(SHADOW);
    var lo = min(orb_lo, card_lo);
    var hi = max(orb_hi, card_hi);
    if (orb.strip_style.x > 0.001 && orb.strip.z > 0.5) {
        lo = min(lo, orb.strip.xy - vec2<f32>(48.0));
        hi = max(hi, orb.strip.xy + orb.strip.zw + vec2<f32>(48.0));
    }
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
    let radius_px = half * (1.0 + orb.hover * 0.04 * (1.0 - orb.open));

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

    if (orb.strip_style.x > 0.001 && orb.strip.z > 0.5) {
        let k = orb.strip_style.x;
        let radius = orb.strip_style.w;
        let shape = box_sd(in.pixel, orb.strip, radius);
        if (orb.strip_style.y > 0.5) {
            let drop = box_sd(in.pixel - vec2<f32>(0.0, 14.0), orb.strip, radius);
            let s = 1.0 - smoothstep(-34.0 * 0.4, 34.0, drop);
            color = over(color, vec4<f32>(vec3<f32>(0.0), s * s * 0.55 * k));
        }
        let cover = fill(shape, aa_px);
        var c = orb.strip_color.rgb;
        if (orb.strip_style.y > 0.5) {
            let below = fill(box_sd(in.pixel - vec2<f32>(0.0, 1.0), orb.strip, radius), aa_px);
            c = mix(c, veil(c, vec3<f32>(1.0), 0.06), cover * (1.0 - below));
        }

        let art_shape = box_sd(in.pixel, orb.strip_art, 8.0);
        var art = art_at(in.pixel, orb.strip_art);
        art = mix(art, veil(art, vec3<f32>(1.0), 0.08), fill(abs(art_shape + 0.5) - 0.5, aa_px));
        c = mix(c, art, fill(art_shape, aa_px));

        let play_glyph = select(1, 0, orb.strip_style.z > 0.5);
        let glyphs = min(glyph_sd(in.pixel, orb.glyphs.xy, play_glyph), glyph_sd(in.pixel, orb.glyphs.zw, 2));
        c = mix(c, orb.ink.rgb, fill(glyphs, aa_px));

        let track = vec4<f32>(orb.line.x, orb.line.y, orb.line.z, 2.0);
        let played = vec4<f32>(orb.line.x, orb.line.y, orb.line.z * orb.line.w, 2.0);
        c = mix(c, veil(c, orb.ink.rgb, 0.14), fill(box_sd(in.pixel, track, 0.0), aa_px));
        c = mix(c, orb.ink.rgb, fill(box_sd(in.pixel, played, 0.0), aa_px));

        color = over(color, vec4<f32>(c, cover * k));
    }

    if (orb.crown_style.x > 0.001) {
        let k = orb.crown_style.x;
        let shape = box_sd(in.pixel, orb.crown, orb.crown_style.z);
        var c = mix(body, veil(body, vec3<f32>(1.0), 0.12), fill(abs(shape + 0.5) - 0.5, aa_px));
        let art_shape = box_sd(in.pixel, orb.crown_art, orb.crown_style.w);
        c = mix(c, art_at(in.pixel, orb.crown_art), fill(art_shape, aa_px));
        color = over(color, vec4<f32>(c, fill(shape, aa_px) * k * orb.body_color.a));
    }

    let fold = orb.agent_a.x;
    let park = orb.agent_a.y;
    let voice = orb.agent_a.z;
    let tint = orb.agent_a.w;
    let dim = orb.agent_b.x;
    let ring_hub = orb.agent_b.w;
    let solid_hub = orb.agent_c.x;
    let dark = orb.dark.rgb;

    let face = fill(d - 1.0, aa);
    if (face > 0.0) {
        // Multiplicative: blending is linear, so an additive lift reads far brighter than it looks.
        let lift = 1.0 + orb.hover * 0.35 * (1.0 - ambient) * (1.0 - orb.open);
        let shade = mix(1.1, 0.88, smoothstep(0.0, 1.0, d));
        var f = vec4<f32>(body * shade * lift * mix(1.0, 0.5, ambient), face * orb.body_color.a);

        let ring = fill(abs(d - 27.0 * UNIT) - 0.4 * UNIT, aa);
        f = vec4<f32>(mix(f.rgb, veil(f.rgb, vec3<f32>(1.0), mix(0.10, 0.07, ambient)), ring), f.a);

        // The twelve marks: hour indices at rest, voice bars from the rim while it listens or speaks.
        let step = TAU / 12.0;
        let k = i32(round(atan2(p.x, p.y) / step));
        let dir = dial(f32(k) * step);
        let quarter = (k % 3) == 0;
        let rest_inner = select(20.7, 18.5, quarter) * UNIT;
        let rest_half = select(0.55, 0.9, quarter) * UNIT;
        let voice_inner = MARK_OUTER - (2.5 + 12.3 * mark_level(k)) * UNIT;
        let mark = radial_sd(
            p,
            dir,
            mix(rest_inner, voice_inner, voice),
            MARK_OUTER,
            mix(rest_half, 0.95 * UNIT, voice),
        );
        let rest_toward = mix(vec3<f32>(1.0), lume, ambient);
        let rest_alpha = select(0.34, 0.62, quarter) * mix(1.0, 0.7, ambient) * mix(1.0, 0.28, dim);
        let rest_rgb = veil(f.rgb, rest_toward, rest_alpha);
        let mark_rgb = mix(rest_rgb, mix(orb.ink.rgb, accent, tint), voice);
        f = vec4<f32>(mix(f.rgb, mark_rgb, fill(mark, aa)), f.a);
        let tick_glow = exp(-max(mark, 0.0) * 90.0) * ambient * 0.25 * (1.0 - voice);
        f = over(f, vec4<f32>(rest_toward, tick_glow));

        if (orb.warn > 0.001) {
            let band = arc_sd(p, 24.5 * UNIT, TAU * 5.0 / 12.0, TAU / 6.0) - 1.3 * UNIT;
            f = over(f, vec4<f32>(orb.amber.rgb, fill(band, aa) * orb.warn));
        }

        if (orb.agent_c.y > 0.001) {
            let band = arc_sd(p, 26.4 * UNIT, 0.0, TAU * orb.agent_c.z) - 1.1 * UNIT;
            f = over(f, vec4<f32>(accent, fill(band, aa) * orb.agent_c.y));
        }

        if (orb.agent_b.y > 0.001) {
            let bright = radial_sd(p, dial(orb.agent_b.z), 16.5 * UNIT, MARK_OUTER, 1.1 * UNIT);
            f = over(f, vec4<f32>(orb.ink.rgb, fill(bright, aa) * orb.agent_b.y));
        }

        let secs = orb.seconds_of_day;
        // Parking scales the angle, so leaving twelve is always a clockwise sweep to the time.
        let minute = dial(TAU * fract(secs / 3600.0) * (1.0 - park));
        let hour = dial(TAU * fract(secs / 43200.0) * (1.0 - park));
        let ink = mix(orb.hand_ink.rgb, lume, ambient);
        let reach = 1.0 - fold;

        if (orb.dial > 0.001 && reach > 0.001) {
            let hour_tip = 0.5 * reach;
            let minute_tip = 0.78 * reach;
            let drop = vec2<f32>(0.0, 0.035);
            let shadow_h = hand_sd(p + drop, hour, 0.0, hour_tip, 0.05, 0.034);
            let shadow_m = hand_sd(p + drop, minute, -0.1 * reach, minute_tip, 0.036, 0.022);
            let hand_shadow = fill(min(shadow_h, shadow_m) - 0.015, 0.06) * (1.0 - ambient);
            f = vec4<f32>(f.rgb * (1.0 - hand_shadow * 0.5), f.a);

            let hands = min(
                hand_sd(p, hour, 0.0, hour_tip, 0.05, 0.034),
                hand_sd(p, minute, -0.1 * reach, minute_tip, 0.036, 0.022),
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

        // One hub morphing between three looks: solid with a dark pin, a ring, all accent.
        let hub_outer = (2.1 + ring_hub * (3.8 - 2.1) + solid_hub * (2.6 - 2.1)) * UNIT;
        let hub_inner = (0.75 + ring_hub * (2.4 - 0.75) - solid_hub * 0.75) * UNIT;
        f = over(f, vec4<f32>(accent, fill(d - hub_outer, aa)));
        f = over(f, vec4<f32>(dark, fill(d - hub_inner, aa) * smoothstep(0.0, 0.2 * UNIT, hub_inner)));

        color = over(color, f);
    }

    if (orb.alert > 0.001) {
        let focus = orb.shape.w;
        let pip_d = length(p - dial(orb.shape.z) * (24.04 * UNIT));
        color = over(color, vec4<f32>(dark, fill(pip_d - 6.0 * UNIT, aa) * orb.alert));
        color = over(color, vec4<f32>(accent, fill(pip_d - 4.2 * UNIT, aa) * orb.alert * (1.0 - focus)));
        let hollow = abs(pip_d - 3.4 * UNIT) - 0.7 * UNIT;
        color = over(color, vec4<f32>(orb.quiet.rgb, fill(hollow, aa) * orb.alert * focus));
    }

    if (orb.agent_c.w > 0.001) {
        let light_d = length(p - dial(0.0) * (21.5 * UNIT));
        color = over(color, vec4<f32>(dark, fill(light_d - 5.4 * UNIT, aa) * orb.agent_c.w));
        color = over(color, vec4<f32>(orb.amber.rgb, fill(light_d - 3.6 * UNIT, aa) * orb.agent_c.w));
    }

    if (color.a < 0.002) {
        discard;
    }
    return color;
}
