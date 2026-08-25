struct MatrixUniforms {
  timing: vec4<f32>,   // time, intensity, motion scale, seed
  viewport: vec4<f32>, // width, height, aspect, target column count
};

@group(0) @binding(0)
var<uniform> uniforms: MatrixUniforms;

struct VertexOutput {
  @builtin(position) position: vec4<f32>,
  @location(0) uv: vec2<f32>,
};

const PERSIAN_LETTER_COUNT: u32 = 32u;

// A compact GPU-native 5x7 atlas containing the complete Persian alphabet:
// ا ب پ ت ث ج چ ح خ د ذ ر ز ژ س ش ص ض ط ظ ع غ ف ق ک گ ل م ن و ه ی
// Each row is a five-bit bitmap. Keeping it in WGSL makes every falling letter
// GPU-rendered and textureless, with no font or image download at runtime.
const PERSIAN_GLYPH_ROWS: array<array<u32, 7>, 32> = array<array<u32, 7>, 32>(
  // ا (alef)
  array<u32, 7>(4u, 4u, 4u, 4u, 4u, 4u, 6u),
  // ب (be)
  array<u32, 7>(0u, 0u, 17u, 17u, 31u, 0u, 4u),
  // پ (pe)
  array<u32, 7>(0u, 0u, 17u, 17u, 31u, 0u, 21u),
  // ت (te)
  array<u32, 7>(10u, 0u, 17u, 17u, 31u, 0u, 0u),
  // ث (se)
  array<u32, 7>(21u, 0u, 17u, 17u, 31u, 0u, 0u),
  // ج (jim)
  array<u32, 7>(0u, 14u, 16u, 14u, 1u, 14u, 4u),
  // چ (che)
  array<u32, 7>(0u, 14u, 16u, 14u, 1u, 14u, 21u),
  // ح (he jimi)
  array<u32, 7>(0u, 14u, 16u, 14u, 1u, 14u, 0u),
  // خ (khe)
  array<u32, 7>(4u, 14u, 16u, 14u, 1u, 14u, 0u),
  // د (dal)
  array<u32, 7>(0u, 6u, 2u, 2u, 30u, 0u, 0u),
  // ذ (zal)
  array<u32, 7>(4u, 6u, 2u, 2u, 30u, 0u, 0u),
  // ر (re)
  array<u32, 7>(0u, 0u, 6u, 2u, 30u, 0u, 0u),
  // ز (ze)
  array<u32, 7>(4u, 0u, 6u, 2u, 30u, 0u, 0u),
  // ژ (zhe)
  array<u32, 7>(21u, 0u, 6u, 2u, 30u, 0u, 0u),
  // س (sin)
  array<u32, 7>(0u, 0u, 21u, 21u, 31u, 0u, 0u),
  // ش (shin)
  array<u32, 7>(21u, 0u, 21u, 21u, 31u, 0u, 0u),
  // ص (sad)
  array<u32, 7>(0u, 0u, 29u, 21u, 31u, 0u, 0u),
  // ض (zad)
  array<u32, 7>(4u, 0u, 29u, 21u, 31u, 0u, 0u),
  // ط (ta)
  array<u32, 7>(4u, 4u, 31u, 21u, 31u, 0u, 0u),
  // ظ (za)
  array<u32, 7>(1u, 4u, 31u, 21u, 31u, 0u, 0u),
  // ع (ain)
  array<u32, 7>(0u, 14u, 8u, 14u, 1u, 14u, 0u),
  // غ (ghain)
  array<u32, 7>(4u, 14u, 8u, 14u, 1u, 14u, 0u),
  // ف (fe)
  array<u32, 7>(4u, 14u, 17u, 15u, 1u, 31u, 0u),
  // ق (qaf)
  array<u32, 7>(10u, 14u, 17u, 15u, 1u, 31u, 0u),
  // ک (kaf)
  array<u32, 7>(16u, 17u, 10u, 4u, 31u, 0u, 0u),
  // گ (gaf)
  array<u32, 7>(10u, 16u, 17u, 10u, 31u, 0u, 0u),
  // ل (lam)
  array<u32, 7>(4u, 4u, 4u, 4u, 4u, 28u, 0u),
  // م (mim)
  array<u32, 7>(0u, 14u, 17u, 15u, 1u, 1u, 0u),
  // ن (nun)
  array<u32, 7>(4u, 0u, 17u, 17u, 31u, 0u, 0u),
  // و (vav)
  array<u32, 7>(6u, 9u, 7u, 1u, 2u, 4u, 8u),
  // ه (he)
  array<u32, 7>(14u, 17u, 14u, 4u, 14u, 0u, 0u),
  // ی (ye)
  array<u32, 7>(0u, 0u, 1u, 1u, 31u, 0u, 10u),
);

fn hash11(value: f32) -> f32 {
  return fract(sin(value * 127.1 + uniforms.timing.w) * 43758.5453123);
}

fn persian_bitmap_mask(cell: vec2<f32>, glyph: u32) -> f32 {
  if glyph >= PERSIAN_LETTER_COUNT {
    return 0.0;
  }
  let scaled = clamp(cell, vec2<f32>(0.0), vec2<f32>(0.9999)) * vec2<f32>(5.0, 7.0);
  let column = u32(floor(scaled.x));
  let row = u32(floor(scaled.y));
  let row_bits = PERSIAN_GLYPH_ROWS[glyph][row];
  let bit = (row_bits >> (4u - column)) & 1u;
  let pixel = abs(fract(scaled) - vec2<f32>(0.5));
  let pixel_shape = 1.0 - smoothstep(0.36, 0.50, max(pixel.x, pixel.y));
  return f32(bit) * pixel_shape;
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
  let cell_size = clamp(resolution.x / max(uniforms.viewport.w, 24.0), 10.0, 19.0);
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
  let glyph_id = u32(floor(hash11(cell_id.x * 31.0 + cell_id.y * 17.0 + glyph_epoch)
    * f32(PERSIAN_LETTER_COUNT)));
  let glyph = persian_bitmap_mask(cell_uv, glyph_id);

  let random_sparse = step(0.14, hash11(cell_id.x * 7.0 + cell_id.y * 41.0));
  let scan = 0.84 + 0.16 * sin(pixel.y * 0.34);
  let vignette_uv = input.uv * 2.0 - vec2<f32>(1.0);
  let vignette = clamp(1.12 - dot(vignette_uv, vignette_uv) * 0.34, 0.30, 1.0);
  let energy = glyph * random_sparse * (trail * 0.78 + head * 1.75) * scan * vignette * uniforms.timing.y;
  let green = vec3<f32>(0.012, 0.42, 0.115) * energy;
  let white_hot = vec3<f32>(0.56, 1.0, 0.73) * glyph * head * 0.72 * uniforms.timing.y;
  let haze = vec3<f32>(0.0, 0.018, 0.008) * (0.5 + trail * 0.5);
  return vec4<f32>(haze + green + white_hot, 1.0);
}
