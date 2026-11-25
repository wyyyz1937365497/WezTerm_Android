struct ViewUniform {
    resolution: vec2<f32>,
    cell_size: vec2<f32>,
    cursor_cell: vec2<f32>,
    padding: vec2<f32>,
    terminal_size: vec2<u32>,
    // 0: background layer pass, 1: text layer pass. Image quads render in
    // their own draw calls: the deep z<=i32::MIN/2 layer before the
    // background pass, the remaining z<0 layer between it and the text pass,
    // and z>=0 after the text pass.
    mode: u32,
    _pad: u32,
    // Android's default cell background is black. Deep negative-z imagery
    // shows through cells with that background, but stays behind colored cells.
    default_background: vec3<f32>,
    skip_default_background: u32,
};

struct CellRenderData {
    atlas_min: vec2<f32>,
    atlas_max: vec2<f32>,
    glyph_origin: vec2<f32>,
    glyph_size: vec2<f32>,
    foreground: vec4<f32>,
    background: vec4<f32>,
    // x: glyph present, y: underline, z: strikethrough, w: intensity.
    flags: vec4<u32>,
};


struct Cells {
    values: array<CellRenderData>,
};

@group(0) @binding(0)
var<uniform> view: ViewUniform;

@group(0) @binding(1)
var<storage, read> cells: Cells;

@group(0) @binding(2)
var glyph_atlas: texture_2d<f32>;

@group(0) @binding(3)
var glyph_sampler: sampler;

@group(0) @binding(4)
var image_atlas: texture_2d<f32>;

@group(0) @binding(5)
var image_sampler: sampler;

struct FullscreenOutput {
    @builtin(position) position: vec4<f32>,
};

struct ImageOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

fn edge_fade(position: vec2<f32>) -> f32 {
    return smoothstep(0.0, 14.0, position.x) *
        smoothstep(0.0, 14.0, position.y) *
        smoothstep(0.0, 14.0, view.resolution.x - position.x) *
        smoothstep(0.0, 14.0, view.resolution.y - position.y);
}

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> FullscreenOutput {
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0),
    );

    var output: FullscreenOutput;
    output.position = vec4<f32>(positions[vertex_index], 0.0, 1.0);
    return output;
}

@fragment
fn fs_main(input: FullscreenOutput) -> @location(0) vec4<f32> {
    let pixel = input.position.xy - view.padding;
    let cell_float = floor(pixel / view.cell_size);
    let within_cell = fract(pixel / view.cell_size);
    let within_cell_pixels = within_cell * view.cell_size;
    let cell = vec2<u32>(max(cell_float, vec2<f32>(0.0)));
    let inside_terminal = pixel.x >= 0.0 && pixel.y >= 0.0 &&
        cell.x < view.terminal_size.x && cell.y < view.terminal_size.y;

    let fade = 0.82 + 0.18 * edge_fade(input.position.xy);

    if view.mode == 0u {
        // Background layer: while deep negative-z imagery is on screen,
        // default-background cells are left transparent to show that image.
        var background = vec4<f32>(0.0, 0.0, 0.0, 1.0);
        if inside_terminal {
            let cell_index = cell.y * view.terminal_size.x + cell.x;
            background = cells.values[cell_index].background;
        }
        let is_default = view.skip_default_background != 0u && all(
            abs(background.rgb - view.default_background) < vec3<f32>(1.0 / 255.0)
        );
        let alpha = select(1.0, 0.0, is_default);
        return vec4<f32>(background.rgb * fade * alpha, alpha);
    }

    // Text layer. The background (and any z<0 imagery) is already in the
    // framebuffer, so glyph coverage, rules, and the cursor composite with
    // alpha blending. The sequential mixes of the previous single-pass
    // shader collapse into one blended layer:
    // a = 1 - (1-glyph)(1-rule)(1-cursor), rgb = weighted source average.
    var glyph = 0.0;
    var rule = 0.0;
    var foreground = vec3<f32>(0.84, 0.88, 0.94);
    if inside_terminal {
        let cell_index = cell.y * view.terminal_size.x + cell.x;
        let render_cell = cells.values[cell_index];
        foreground = render_cell.foreground.rgb;

        let glyph_pixel = within_cell_pixels - render_cell.glyph_origin;
        let inside_glyph = render_cell.flags.x != 0u &&
            all(glyph_pixel >= vec2<f32>(0.0)) &&
            all(glyph_pixel < render_cell.glyph_size);
        if inside_glyph {
            let glyph_fraction = glyph_pixel / render_cell.glyph_size;
            let atlas_uv = mix(render_cell.atlas_min, render_cell.atlas_max, glyph_fraction);
            let coverage = textureSample(glyph_atlas, glyph_sampler, atlas_uv).r;
            glyph = coverage * render_cell.foreground.a;
        }

        rule = max(
            select(0.0, 1.0, render_cell.flags.y != 0u && within_cell.y > 0.82 && within_cell.y < 0.86),
            select(0.0, 1.0, render_cell.flags.z != 0u && within_cell.y > 0.48 && within_cell.y < 0.52),
        );
    }

    let is_cursor_cell = inside_terminal && all(vec2<f32>(cell) == view.cursor_cell);
    let cursor = select(
        0.0,
        1.0,
        is_cursor_cell && within_cell.x > 0.08 && within_cell.x < 0.17 &&
            within_cell.y > 0.10 && within_cell.y < 0.90,
    );
    let cursor_color = vec3<f32>(0.98, 0.56, 0.18);

    let glyph_weight = glyph * (1.0 - rule) * (1.0 - cursor);
    let rule_weight = rule * (1.0 - cursor);
    let alpha = min(glyph_weight + rule_weight + cursor, 1.0);
    var color = foreground * (glyph_weight + rule_weight) + cursor_color * cursor;
    if alpha > 0.0 {
        color = color / alpha;
    }
    return vec4<f32>(color * fade, alpha);
}

@vertex
fn vs_image_main(
    @builtin(vertex_index) vertex_index: u32,
    @location(0) rect: vec4<f32>,
    @location(1) uv: vec4<f32>,
) -> ImageOutput {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 1.0),
    );
    let corner = corners[vertex_index];
    let pixel = mix(rect.xy, rect.zw, corner);

    var output: ImageOutput;
    output.position = vec4<f32>(
        pixel.x / view.resolution.x * 2.0 - 1.0,
        1.0 - pixel.y / view.resolution.y * 2.0,
        0.0,
        1.0,
    );
    output.uv = mix(uv.xy, uv.zw, corner);
    return output;
}

@fragment
fn fs_image_main(input: ImageOutput) -> @location(0) vec4<f32> {
    // Atlas pixels are stored premultiplied; hand them straight to the
    // premultiplied blend state so linear filtering between opaque and
    // transparent texels cannot produce dark edge fringes.
    let texel = textureSample(image_atlas, image_sampler, input.uv);
    let fade = 0.82 + 0.18 * edge_fade(input.position.xy);
    return vec4<f32>(texel.rgb * fade, texel.a);
}
