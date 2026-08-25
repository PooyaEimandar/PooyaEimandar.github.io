// KTX portrait sampled through animated binary Matrix glyphs. No head mesh.
struct PortraitUniforms {
  timing: vec4<f32>,    // time, reveal, opacity, motion scale
  viewport: vec4<f32>,  // width, height, viewport aspect, texture aspect
  placement: vec4<f32>, // center-x, center-y, width, height in screen UV
  eyes: vec4<f32>,      // left-x, left-y, right-x, right-y in portrait UV
};

@group(0) @binding(0)
var<uniform> uniforms: PortraitUniforms;

@group(0) @binding(1)
var portrait_texture: texture_2d<f32>;

@group(0) @binding(2)
var portrait_sampler: sampler;

struct VertexOutput {
  @builtin(position) position: vec4<f32>,
  @location(0) uv: vec2<f32>,
};

const BINARY_GLYPH_COUNT: u32 = 2u;
// A dense grid keeps the portrait detailed while leaving each 5x7 digit
// readable on both desktop and narrow mobile canvases.
const BINARY_GRID_SIZE: vec2<f32> = vec2<f32>(96.0, 132.0);

fn hash11(value: f32) -> f32 {
  return fract(sin(value * 127.1 + 23.47) * 43758.5453123);
}

fn binary_row_bits(glyph: u32, row: u32) -> u32 {
  if glyph == 0u {
    switch row {
      case 0u: { return 14u; }
      case 1u: { return 17u; }
      case 2u: { return 19u; }
      case 3u: { return 21u; }
      case 4u: { return 25u; }
      case 5u: { return 17u; }
      default: { return 14u; }
    }
  }
  switch row {
    case 0u: { return 4u; }
    case 1u: { return 12u; }
    case 2u: { return 4u; }
    case 3u: { return 4u; }
    case 4u: { return 4u; }
    case 5u: { return 4u; }
    default: { return 14u; }
  }
}

fn binary_bitmap_mask(cell: vec2<f32>, glyph: u32) -> f32 {
  if glyph >= BINARY_GLYPH_COUNT {
    return 0.0;
  }
  let scaled = clamp(cell, vec2<f32>(0.0), vec2<f32>(0.9999)) * vec2<f32>(5.0, 7.0);
  let column = u32(floor(scaled.x));
  let row = u32(floor(scaled.y));
  let row_bits = binary_row_bits(glyph, row);
  let bit = (row_bits >> (4u - column)) & 1u;
  let pixel = abs(fract(scaled) - vec2<f32>(0.5));
  let pixel_shape = 1.0 - smoothstep(0.34, 0.49, max(pixel.x, pixel.y));
  return f32(bit) * pixel_shape;
}

fn ellipse_mask(uv: vec2<f32>, center: vec2<f32>, radius: vec2<f32>) -> f32 {
  let distance = length((uv - center) / radius);
  return 1.0 - smoothstep(0.72, 1.0, distance);
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
  output.position = vec4<f32>(position, 0.70, 1.0);
  output.uv = position * 0.5 + vec2<f32>(0.5);
  return output;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
  // Match the converter's explicit top-down KTX row convention.
  let screen_uv = vec2<f32>(input.uv.x, 1.0 - input.uv.y);
  let portrait_min = uniforms.placement.xy - uniforms.placement.zw * 0.5;
  let portrait_uv = (screen_uv - portrait_min) / uniforms.placement.zw;
  if any(portrait_uv < vec2<f32>(0.0)) {
    discard;
  }
  if any(portrait_uv > vec2<f32>(1.0)) {
    discard;
  }

  let photo = textureSampleLevel(portrait_texture, portrait_sampler, portrait_uv, 0.0);
  let lower_fade = 1.0 - smoothstep(0.94, 1.0, portrait_uv.y);
  let subject_alpha = photo.a * lower_fade;
  if subject_alpha < 0.004 {
    discard;
  }

  let time = uniforms.timing.x;
  let reveal = uniforms.timing.y;
  let opacity = uniforms.timing.z;
  let motion = uniforms.timing.w;
  let grid_size = BINARY_GRID_SIZE;
  let grid = portrait_uv * grid_size;
  let cell_id = floor(grid);
  let cell_uv = fract(grid);
  let column_seed = hash11(cell_id.x + 17.0);
  let speed = mix(0.055, 0.145, column_seed) * motion;
  let stream_head = fract(time * speed + hash11(cell_id.x + 73.0));
  let wrapped_distance = fract(portrait_uv.y - stream_head + 1.0);
  let trail_length = mix(0.14, 0.48, hash11(cell_id.x + 31.0));
  let trail = pow(clamp(1.0 - wrapped_distance / trail_length, 0.0, 1.0), 1.45);
  let head = exp(-wrapped_distance * 88.0);
  let glyph_epoch = floor(time * mix(4.0, 11.0, column_seed));
  // Each cell deterministically alternates between 0 and 1 as its epoch changes.
  let binary_sample = hash11(cell_id.x * 31.0 + cell_id.y * 17.0 + glyph_epoch);
  let glyph_id = select(0u, 1u, binary_sample >= 0.5);
  let glyph = binary_bitmap_mask(cell_uv, glyph_id);

  let cell_center = (cell_id + vec2<f32>(0.5)) / grid_size;
  let cell_photo = textureSampleLevel(
    portrait_texture,
    portrait_sampler,
    clamp(cell_center, vec2<f32>(0.0), vec2<f32>(1.0)),
    0.0,
  );
  let luminance = dot(photo.rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
  let cell_luminance = dot(cell_photo.rgb, vec3<f32>(0.2126, 0.7152, 0.0722));

  // Reveal the photograph from top to bottom through irregular glyph cells.
  let reveal_edge = mix(-0.10, 1.10, reveal);
  let reveal_noise = (hash11(cell_id.x * 13.0 + cell_id.y * 47.0) - 0.5) * 0.12;
  let reveal_mask = 1.0 - smoothstep(
    reveal_edge - 0.055,
    reveal_edge + 0.055,
    portrait_uv.y + reveal_noise,
  );

  let look = vec2<f32>(sin(time * 0.61) * 0.006, sin(time * 0.39) * 0.002) * motion;
  let left_iris = ellipse_mask(
    portrait_uv,
    uniforms.eyes.xy + look,
    vec2<f32>(0.014, 0.011),
  );
  let right_iris = ellipse_mask(
    portrait_uv,
    uniforms.eyes.zw + look,
    vec2<f32>(0.014, 0.011),
  );
  let iris = clamp(left_iris + right_iris, 0.0, 1.0);

  let portrait_green = vec3<f32>(0.010, 0.18, 0.050)
    + vec3<f32>(0.018, 0.52, 0.135) * luminance * 0.74;
  let glyph_energy = glyph
    * (0.34 + cell_luminance * 0.86)
    * (0.30 + trail * 0.72 + head * 1.18);
  let scan = 0.88 + 0.12 * sin(portrait_uv.y * uniforms.viewport.y * 0.38);
  var color = portrait_green * (0.40 + glyph * 0.24) * scan;
  color += vec3<f32>(0.055, 0.90, 0.245) * glyph_energy;
  color += vec3<f32>(0.22, 1.0, 0.47) * iris * 0.24;

  let alpha = subject_alpha
    * reveal_mask
    * opacity
    * clamp(0.34 + glyph * 0.58 + trail * 0.12, 0.0, 1.0);
  if alpha < 0.008 {
    discard;
  }
  return vec4<f32>(color, alpha);
}
