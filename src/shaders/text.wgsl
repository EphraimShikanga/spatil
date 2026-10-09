struct TextUniform {
    screen: vec4<f32>,
};

@group(0) @binding(0)
var<uniform> text: TextUniform;
@group(0) @binding(1)
var t_atlas: texture_2d<f32>;
@group(0) @binding(2)
var s_atlas: sampler;

const ALPHA_CUTOFF: f32 = 0.004;

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
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
    @location(1) uv: vec4<f32>,
    @location(2) color: vec4<f32>,
) -> VertexOutput {
    let corner = quad_corner(vertex_index);
    let pixel = vec2<f32>(rect.x, rect.y) + corner * vec2<f32>(rect.z, rect.w);

    var out: VertexOutput;
    out.clip_position = vec4<f32>(
        pixel.x / max(text.screen.x, 1.0) * 2.0 - 1.0,
        1.0 - pixel.y / max(text.screen.y, 1.0) * 2.0,
        0.0,
        1.0,
    );
    out.uv = vec2<f32>(
        mix(uv.x, uv.z, corner.x),
        mix(uv.y, uv.w, corner.y),
    );
    out.color = color;
    return out;
}

// Mask glyphs are stored as (1,1,1,coverage) and colour glyphs as straight RGBA,
// so a single multiply serves both: a white instance colour leaves an emoji
// untouched, while a mask takes the instance colour and keeps only its coverage.
@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let texel = textureSample(t_atlas, s_atlas, in.uv);
    let alpha = in.color.a * texel.a;
    if (alpha < ALPHA_CUTOFF) {
        discard;
    }
    return vec4<f32>(in.color.rgb * texel.rgb, alpha);
}
