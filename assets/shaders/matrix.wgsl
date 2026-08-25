struct MatrixUniforms {
  timing: vec4<f32>,   // time, intensity, motion scale, seed
  viewport: vec4<f32>, // width, height, aspect, target column count
  signal: vec4<f32>,   // start time, travel duration, normalized column, active flag
};

@group(0) @binding(0)
var<uniform> uniforms: MatrixUniforms;

@group(0) @binding(1)
var persian_glyph_atlas: texture_2d<f32>;

@group(0) @binding(2)
var persian_glyph_sampler: sampler;

struct VertexOutput {
  @builtin(position) position: vec4<f32>,
  @location(0) uv: vec2<f32>,
};

const PERSIAN_LETTER_COUNT: u32 = 32u;
const PERSIAN_ATLAS_COLUMNS: u32 = 8u;
const PERSIAN_ATLAS_ROWS: u32 = 4u;
const PERSIAN_ATLAS_TEXELS: vec2<f32> = vec2<f32>(256.0, 128.0);
const PERSIAN_GLYPH_RENDER_SCALE: f32 = 2.0;
const MATRIX_SIGNAL_GLYPH_COUNT: u32 = 8u;

fn hash11(value: f32) -> f32 {
  return fract(sin(value * 127.1 + uniforms.timing.w) * 43758.5453123);
}

fn persian_atlas_mask(cell: vec2<f32>, glyph: u32) -> f32 {
  if glyph >= PERSIAN_LETTER_COUNT {
    return 0.0;
  }
  let atlas_grid = vec2<f32>(f32(PERSIAN_ATLAS_COLUMNS), f32(PERSIAN_ATLAS_ROWS));
  let atlas_cell = vec2<f32>(
    f32(glyph % PERSIAN_ATLAS_COLUMNS),
    f32(glyph / PERSIAN_ATLAS_COLUMNS),
  );
  let texel = vec2<f32>(1.0) / PERSIAN_ATLAS_TEXELS;
  let cell_min = atlas_cell / atlas_grid + texel;
  let cell_max = (atlas_cell + vec2<f32>(1.0)) / atlas_grid - texel;
  let atlas_uv = mix(cell_min, cell_max, clamp(cell, vec2<f32>(0.0), vec2<f32>(1.0)));
  let coverage = textureSampleLevel(
    persian_glyph_atlas,
    persian_glyph_sampler,
    atlas_uv,
    0.0,
  ).a;
  return smoothstep(0.08, 0.72, coverage);
}

fn matrix_signal_glyph(index: u32) -> u32 {
  switch index {
    case 0u: { return 5u; }
    case 1u: { return 0u; }
    case 2u: { return 29u; }
    case 3u: { return 31u; }
    case 4u: { return 9u; }
    case 5u: { return 15u; }
    case 6u: { return 0u; }
    default: { return 30u; }
  }
}

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
  var positions = array<vec2<f32>, 3>(
    vec2<f32>(-1.0, -1.0),
    vec2<f32>(3.0, -1.0),
    vec2<f32>(-1.0, 3.0),
  );
  let position = positions[vertex_index];
  var output: VertexOutput;
  output.position = vec4<f32>(position, 0.999, 1.0);
  output.uv = position * 0.5 + vec2<f32>(0.5);
  return output;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
  let resolution = max(uniforms.viewport.xy, vec2<f32>(1.0));
  let base_cell_size = clamp(resolution.x / max(uniforms.viewport.w, 24.0), 10.0, 19.0);
  let cell_size = base_cell_size * PERSIAN_GLYPH_RENDER_SCALE;
  let pixel = vec2<f32>(input.uv.x, 1.0 - input.uv.y) * resolution;
  let grid = pixel / cell_size;
  let cell_id = floor(grid);
  let cell_uv = fract(grid);

  let column_seed = hash11(cell_id.x + 11.0);
  let speed = mix(0.055, 0.17, column_seed) * uniforms.timing.z;
  let stream_head = fract(uniforms.timing.x * speed + hash11(cell_id.x + 71.0));
  let cell_v = pixel.y / resolution.y;
  let wrapped_distance = fract(cell_v - stream_head + 1.0);
  let trail_length = mix(0.10, 0.46, hash11(cell_id.x + 29.0));
  let trail = pow(clamp(1.0 - wrapped_distance / trail_length, 0.0, 1.0), 1.55);
  let head = exp(-wrapped_distance * 92.0);

  let glyph_epoch = floor(uniforms.timing.x * mix(5.0, 13.0, column_seed));
  var glyph_id = u32(floor(hash11(cell_id.x * 31.0 + cell_id.y * 17.0 + glyph_epoch)
    * f32(PERSIAN_LETTER_COUNT)));

  let column_count = max(ceil(resolution.x / cell_size), 1.0);
  let row_count = max(ceil(resolution.y / cell_size), 1.0);
  let signal_column = floor(clamp(uniforms.signal.z, 0.0, 0.9999) * column_count);
  let signal_progress = clamp(
    (uniforms.timing.x - uniforms.signal.x) / max(uniforms.signal.y, 0.001),
    0.0,
    1.0,
  );
  let signal_last_top = max(row_count - f32(MATRIX_SIGNAL_GLYPH_COUNT), 0.0);
  let signal_top = floor(mix(0.0, signal_last_top, signal_progress));
  let signal_offset = i32(cell_id.y - signal_top);
  let signal_revealed = min(
    u32(floor(clamp(signal_progress * 4.0, 0.0, 1.0) * f32(MATRIX_SIGNAL_GLYPH_COUNT))) + 1u,
    MATRIX_SIGNAL_GLYPH_COUNT,
  );
  var signal_cell = 0.0;
  if uniforms.signal.w > 0.5 {
    if abs(cell_id.x - signal_column) < 0.5 {
      if signal_offset >= 0 {
        if signal_offset < i32(signal_revealed) {
          glyph_id = matrix_signal_glyph(u32(signal_offset));
          signal_cell = 1.0;
        }
      }
    }
  }

  // Keep implicit texture derivatives in uniform control flow for broad WebGPU compatibility.
  let glyph = persian_atlas_mask(cell_uv, glyph_id);

  let random_sparse = step(0.14, hash11(cell_id.x * 7.0 + cell_id.y * 41.0));
  let scan = 0.84 + 0.16 * sin(pixel.y * 0.34);
  let vignette_uv = input.uv * 2.0 - vec2<f32>(1.0);
  let vignette = clamp(1.12 - dot(vignette_uv, vignette_uv) * 0.34, 0.30, 1.0);
  let rain_strength = max(random_sparse * (trail * 0.78 + head * 1.75), signal_cell * 1.18);
  let energy = glyph * rain_strength * scan * vignette * uniforms.timing.y;
  let hot_strength = max(head * 0.72, signal_cell * 0.38);
  let green = vec3<f32>(0.012, 0.42, 0.115) * energy;
  let white_hot = vec3<f32>(0.56, 1.0, 0.73) * glyph * hot_strength * uniforms.timing.y;
  let haze = vec3<f32>(0.0, 0.018, 0.008) * (0.5 + trail * 0.5);
  return vec4<f32>(haze + green + white_hot, 1.0);
}
