struct TextUniforms {
  view_projection: mat4x4<f32>,
  model: mat4x4<f32>,
  params: vec4<f32>, // time, opacity, typed characters, motion
};

@group(0) @binding(0)
var<uniform> uniforms: TextUniforms;

struct VertexInput {
  @location(0) position: vec3<f32>,
  @location(1) color: vec4<f32>,
  @location(2) terminal_tag: vec2<f32>, // character order, 0 glyph / 1 cursor
};

struct VertexOutput {
  @builtin(position) position: vec4<f32>,
  @location(0) alpha: f32,
  @location(1) @interpolate(flat) character_order: f32,
  @location(2) @interpolate(flat) kind: f32,
  @location(3) color: vec3<f32>,
};

@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
  let world = uniforms.model * vec4<f32>(input.position, 1.0);
  var output: VertexOutput;
  output.position = uniforms.view_projection * world;
  output.alpha = input.color.a;
  output.character_order = input.terminal_tag.x;
  output.kind = input.terminal_tag.y;
  output.color = input.color.rgb;
  return output;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
  let time = uniforms.params.x;
  let opacity = uniforms.params.y;
  let typed_characters = uniforms.params.z;
  let motion = uniforms.params.w;

  if input.kind > 0.5 {
    if abs(input.character_order - floor(typed_characters + 0.001)) > 0.25 {
      discard;
    }
    let blink = mix(1.0, step(0.42, fract(time * 1.45)), motion);
    if blink < 0.5 {
      discard;
    }
    return vec4<f32>(vec3<f32>(1.0), opacity);
  }

  if input.character_order >= typed_characters {
    discard;
  }

  let alpha = clamp(input.alpha * opacity, 0.0, 1.0);
  if alpha < 0.02 {
    discard;
  }

  // CPU tags supply only flat white for terminal copy and exact cyan for
  // hyperlinks. There is no gradient, scanline, tint, rim, or glow modulation.
  return vec4<f32>(input.color, alpha);
}
