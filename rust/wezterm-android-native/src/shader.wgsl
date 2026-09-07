struct ViewUniform {
    resolution: vec2<f32>,
    cell_size: vec2<f32>,
    cursor_cell: vec2<f32>,
    padding: vec2<f32>,
    terminal_size: vec2<u32>,
    reserved: vec2<u32>,
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

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0),
    );

    var output: VertexOutput;
    output.position = vec4<f32>(positions[vertex_index], 0.0, 1.0);
    return output;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let pixel = input.position.xy - view.padding;
    let cell_float = floor(pixel / view.cell_size);
    let within_cell = fract(pixel / view.cell_size);
    let within_cell_pixels = within_cell * view.cell_size;
    let cell = vec2<u32>(max(cell_float, vec2<f32>(0.0)));
    let inside_terminal = pixel.x >= 0.0 && pixel.y >= 0.0 &&
        cell.x < view.terminal_size.x && cell.y < view.terminal_size.y;

    let background = vec3<f32>(0.0, 0.0, 0.0);
    var color = background;

    if inside_terminal {
        let cell_index = cell.y * view.terminal_size.x + cell.x;
        let render_cell = cells.values[cell_index];
        color = render_cell.background.rgb;

        let glyph_pixel = within_cell_pixels - render_cell.glyph_origin;
        let inside_glyph = render_cell.flags.x != 0u &&
            all(glyph_pixel >= vec2<f32>(0.0)) &&
            all(glyph_pixel < render_cell.glyph_size);
        if inside_glyph {
            let glyph_fraction = glyph_pixel / render_cell.glyph_size;
            let atlas_uv = mix(render_cell.atlas_min, render_cell.atlas_max, glyph_fraction);
            let coverage = textureSample(glyph_atlas, glyph_sampler, atlas_uv).r;
            color = mix(color, render_cell.foreground.rgb, coverage * render_cell.foreground.a);
        }

        let underline = select(
            0.0,
            1.0,
            render_cell.flags.y != 0u && within_cell.y > 0.82 && within_cell.y < 0.86,
        );
        let strikethrough = select(
            0.0,
            1.0,
            render_cell.flags.z != 0u && within_cell.y > 0.48 && within_cell.y < 0.52,
        );
        color = mix(color, render_cell.foreground.rgb, max(underline, strikethrough));
    }

    let is_cursor_cell = inside_terminal && all(vec2<f32>(cell) == view.cursor_cell);
    let cursor_bar = select(
        0.0,
        1.0,
        is_cursor_cell && within_cell.x > 0.08 && within_cell.x < 0.17 &&
            within_cell.y > 0.10 && within_cell.y < 0.90,
    );
    color = mix(color, vec3<f32>(0.98, 0.56, 0.18), cursor_bar);

    let edge_fade = smoothstep(0.0, 14.0, input.position.x) *
        smoothstep(0.0, 14.0, input.position.y) *
        smoothstep(0.0, 14.0, view.resolution.x - input.position.x) *
        smoothstep(0.0, 14.0, view.resolution.y - input.position.y);
    return vec4<f32>(color * (0.82 + 0.18 * edge_fade), 1.0);
}
