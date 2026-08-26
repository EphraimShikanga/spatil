struct CursorUniform {
    rect: vec4<f32>,
    screen: vec4<f32>,
    fill: vec4<f32>,
    outline: vec4<f32>,
    alpha: f32,
    angle: f32,
    shrink: f32,
    _pad: f32,
};

@group(0) @binding(0)
var<uniform> cur: CursorUniform;

const OUTLINE: f32 = 0.075;

var<private> ARROW: array<vec2<f32>, 7> = array<vec2<f32>, 7>(
    vec2<f32>(0.00, 0.00),
    vec2<f32>(0.00, 0.86),
    vec2<f32>(0.24, 0.66),
    vec2<f32>(0.39, 1.05),
    vec2<f32>(0.57, 0.97),
    vec2<f32>(0.42, 0.60),
    vec2<f32>(0.69, 0.57),
);

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) q: vec2<f32>,
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

fn arrow_sdf(p: vec2<f32>) -> f32 {
    var d = dot(p - ARROW[0], p - ARROW[0]);
    var s = 1.0;
    for (var i = 0; i < 7; i = i + 1) {
        let j = (i + 6) % 7;
        let e = ARROW[j] - ARROW[i];
        let w = p - ARROW[i];
        let b = w - e * clamp(dot(w, e) / max(dot(e, e), 1e-6), 0.0, 1.0);
        d = min(d, dot(b, b));
        let c = vec3<bool>((p.y >= ARROW[i].y), (p.y < ARROW[j].y), (e.x * w.y > e.y * w.x));
        if (all(c) || all(!c)) {
            s = -s;
        }
    }
    return s * sqrt(d);
}

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    let corner = quad_corner(vertex_index);
    let pixel = vec2<f32>(cur.rect.x, cur.rect.y) + corner * vec2<f32>(cur.rect.z, cur.rect.w);

    var out: VertexOutput;
    out.clip_position = vec4<f32>(
        pixel.x / max(cur.screen.x, 1.0) * 2.0 - 1.0,
        1.0 - pixel.y / max(cur.screen.y, 1.0) * 2.0,
        0.0,
        1.0,
    );
    out.q = corner * 2.0 - 1.0;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    if (cur.alpha < 0.002) {
        discard;
    }

    let s = sin(cur.angle);
    let c = cos(cur.angle);
    let rotated = vec2<f32>(in.q.x * c - in.q.y * s, in.q.x * s + in.q.y * c);
    let p = rotated / max(cur.shrink, 0.05);

    let d = arrow_sdf(p);
    let aa = max(fwidth(d), 0.001) * 1.2;

    let body = 1.0 - smoothstep(-aa, aa, d);
    let edge = 1.0 - smoothstep(OUTLINE - aa, OUTLINE + aa, d);
    if (edge < 0.002) {
        discard;
    }

    let rgb = mix(cur.outline.rgb, cur.fill.rgb, body);
    return vec4<f32>(rgb, edge * cur.alpha);
}
