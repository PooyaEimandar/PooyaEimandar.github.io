mod portrait;
mod timeline;

use std::{
    cell::RefCell,
    rc::Rc,
    sync::atomic::{AtomicBool, Ordering},
};

use bytemuck::{Pod, Zeroable};
use portrait::RgbaKtxImage;
use sib::render::{
    Example, ExampleSettings, FrameStats, RenderContext, RenderError, RenderResult, bind_group,
    buffer, glam, render_pass, shader, text, text_mesh, texture, wgpu, winit,
};
use timeline::TimelineSlide;

const FONT_BYTES: &[u8] = include_bytes!("../assets/fonts/FiraMono-Regular.ttf");
const MATRIX_GLYPH_ATLAS_BYTES: &[u8] = include_bytes!("../assets/textures/vazirmatn-persian.ktx");
const MATRIX_GLYPH_ATLAS_WIDTH: u32 = 256;
const MATRIX_GLYPH_ATLAS_HEIGHT: u32 = 128;
const MATRIX_INTRO_END: f32 = 1.65;
const FACE_REVEAL_END: f32 = 3.25;
const TIMELINE_START: f32 = 3.55;
const MATRIX_SIGNAL_MIN_INTERVAL: f32 = 30.0;
const MATRIX_SIGNAL_MAX_INTERVAL: f32 = 60.0;
const MATRIX_SIGNAL_DURATION: f32 = 8.0;
const SLIDE_DURATION: f32 = 0.82;
const TERMINAL_TEXT_SCALE: f32 = 1.5;
const TERMINAL_BASE_FONT_SIZE: f32 = 0.142;
const TERMINAL_BASE_LINE_HEIGHT: f32 = 0.198;
const TERMINAL_FONT_SIZE: f32 = TERMINAL_BASE_FONT_SIZE * TERMINAL_TEXT_SCALE;
const TERMINAL_LINE_HEIGHT: f32 = TERMINAL_BASE_LINE_HEIGHT * TERMINAL_TEXT_SCALE;
const TERMINAL_DEPTH: f32 = 0.012 * TERMINAL_TEXT_SCALE;
const MOBILE_TERMINAL_SIZE_MULTIPLIER: f32 = 1.5;
const MOBILE_TERMINAL_BASE_SCALE: f32 = 0.68 * MOBILE_TERMINAL_SIZE_MULTIPLIER;
const MOBILE_TERMINAL_BOOSTED_SCALE: f32 = 0.72 * MOBILE_TERMINAL_SIZE_MULTIPLIER;
const MOBILE_TERMINAL_BOOST_START_ASPECT: f32 = 0.48;
const MOBILE_TERMINAL_BOOST_END_ASPECT: f32 = 0.58;
const MOBILE_TIMELINE_MAX_ASPECT: f32 = 0.8;
const MOBILE_TERMINAL_VERTICAL_OFFSET: f32 = -0.43;
// The atlas supplies the solid face of each glyph. Keep the extruded contour
// narrow so it adds depth without visually turning the regular font bold.
const TERMINAL_STROKE_WIDTH: f32 = TERMINAL_FONT_SIZE * 0.045;
const _: () = assert!(MATRIX_INTRO_END < FACE_REVEAL_END);
const _: () = assert!(FACE_REVEAL_END < TIMELINE_START);
const _: () = assert!(MATRIX_SIGNAL_MIN_INTERVAL < MATRIX_SIGNAL_MAX_INTERVAL);
const _: () = assert!(MATRIX_SIGNAL_DURATION < MATRIX_SIGNAL_MIN_INTERVAL);
const _: () = assert!(TERMINAL_TEXT_SCALE == 1.5);
const _: () = assert!(MOBILE_TERMINAL_SIZE_MULTIPLIER == 1.5);
const _: () = assert!(TERMINAL_STROKE_WIDTH <= TERMINAL_FONT_SIZE * 0.05);
const _: () = assert!(MOBILE_TERMINAL_BASE_SCALE < MOBILE_TERMINAL_BOOSTED_SCALE);
const _: () = assert!(MOBILE_TERMINAL_BOOST_START_ASPECT < MOBILE_TERMINAL_BOOST_END_ASPECT);
const _: () = assert!(MOBILE_TERMINAL_BOOST_END_ASPECT < MOBILE_TIMELINE_MAX_ASPECT);

static REDUCED_MOTION: AtomicBool = AtomicBool::new(false);
static PRIMARY_ACTION_REQUEST: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TerminalPrimaryAction {
    Reveal,
    Advance,
}

fn terminal_primary_action(
    typed_characters: usize,
    total_characters: usize,
) -> TerminalPrimaryAction {
    if typed_characters < total_characters {
        TerminalPrimaryAction::Reveal
    } else {
        TerminalPrimaryAction::Advance
    }
}

#[derive(Clone, Copy, Debug)]
struct MatrixSignalSchedule {
    rng_state: u32,
    next_start: f32,
    active_start: f32,
    column: f32,
}

impl MatrixSignalSchedule {
    fn with_seed(seed: u32) -> Self {
        let mut schedule = Self {
            rng_state: seed.max(1),
            next_start: 0.0,
            active_start: -MATRIX_SIGNAL_DURATION,
            column: 0.5,
        };
        schedule.next_start = schedule.random_interval();
        schedule
    }

    fn next_random(&mut self) -> f32 {
        let mut value = self.rng_state;
        value ^= value << 13;
        value ^= value >> 17;
        value ^= value << 5;
        self.rng_state = value;
        value as f32 / u32::MAX as f32
    }

    fn random_interval(&mut self) -> f32 {
        MATRIX_SIGNAL_MIN_INTERVAL
            + self.next_random() * (MATRIX_SIGNAL_MAX_INTERVAL - MATRIX_SIGNAL_MIN_INTERVAL)
    }

    fn update(&mut self, elapsed: f32) {
        if elapsed >= self.next_start {
            self.active_start = elapsed;
            // Keep the complete atlas glyph cell away from a clipped edge column.
            self.column = 0.06 + self.next_random() * 0.88;
            self.next_start = elapsed + self.random_interval();
        }
    }

    fn uniforms(self, elapsed: f32, motion_enabled: bool) -> [f32; 4] {
        let age = elapsed - self.active_start;
        let active = motion_enabled && (0.0..MATRIX_SIGNAL_DURATION).contains(&age);
        [
            self.active_start,
            MATRIX_SIGNAL_DURATION,
            self.column,
            if active { 1.0 } else { 0.0 },
        ]
    }
}

#[cfg(target_arch = "wasm32")]
fn matrix_signal_seed() -> u32 {
    let timestamp_bits = js_sys::Date::now().to_bits();
    ((timestamp_bits as u32) ^ ((timestamp_bits >> 32) as u32) ^ 0x4a53_4848).max(1)
}

#[cfg(not(target_arch = "wasm32"))]
fn matrix_signal_seed() -> u32 {
    0x4a53_4848
}

#[cfg(target_arch = "wasm32")]
fn current_year() -> i32 {
    js_sys::Date::new_0().get_full_year() as i32
}

#[cfg(not(target_arch = "wasm32"))]
fn current_year() -> i32 {
    let unix_days = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        / 86_400;
    gregorian_year_from_unix_days(unix_days as i64)
}

#[cfg(not(target_arch = "wasm32"))]
fn gregorian_year_from_unix_days(unix_days: i64) -> i32 {
    let shifted_days = unix_days + 719_468;
    let era = if shifted_days >= 0 {
        shifted_days
    } else {
        shifted_days - 146_096
    } / 146_097;
    let day_of_era = shifted_days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    (year + i64::from(month <= 2)) as i32
}

fn copyright_notice() -> String {
    format!("© {} Pooya Eimandar. All rights reserved.", current_year())
}

#[cfg(target_arch = "wasm32")]
thread_local! {
    static LINK_PICK_SNAPSHOT: RefCell<Option<LinkPickSnapshot>> = const { RefCell::new(None) };
}

type PendingPortrait = Rc<RefCell<Option<Result<RgbaKtxImage, String>>>>;

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct MatrixUniforms {
    timing: [f32; 4],
    viewport: [f32; 4],
    signal: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct PortraitUniforms {
    timing: [f32; 4],
    viewport: [f32; 4],
    placement: [f32; 4],
    eyes: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct TextUniforms {
    view_projection: [[f32; 4]; 4],
    model: [[f32; 4]; 4],
    params: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct TerminalVertex {
    position: [f32; 3],
    color: [f32; 4],
    /// Character reveal order and vertex kind (0 = glyph, 1 = cursor).
    meta: [f32; 2],
}

impl TerminalVertex {
    const ATTRIBUTES: [wgpu::VertexAttribute; 3] =
        wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x4, 2 => Float32x2];

    fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRIBUTES,
        }
    }
}

struct Pipelines {
    matrix: wgpu::RenderPipeline,
    portrait: wgpu::RenderPipeline,
    text: wgpu::RenderPipeline,
}

struct GpuMatrixGlyphAtlas {
    _texture: texture::Texture,
    bind_group: wgpu::BindGroup,
}

impl GpuMatrixGlyphAtlas {
    fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layout: &wgpu::BindGroupLayout,
        uniform_buffer: &wgpu::Buffer,
    ) -> RenderResult<Self> {
        let image =
            portrait::parse_ktx1_rgba8_asset(MATRIX_GLYPH_ATLAS_BYTES, "vazirmatn-persian.ktx")?;
        if image.width != MATRIX_GLYPH_ATLAS_WIDTH || image.height != MATRIX_GLYPH_ATLAS_HEIGHT {
            return Err(RenderError::message(format!(
                "vazirmatn-persian.ktx is {}x{}; expected {}x{}",
                image.width, image.height, MATRIX_GLYPH_ATLAS_WIDTH, MATRIX_GLYPH_ATLAS_HEIGHT,
            )));
        }
        let rgba = texture::ImageRgba8::new(image.width, image.height, image.rgba)?;
        let texture = texture::Texture::from_rgba8_2d(
            device,
            queue,
            Some("Vazirmatn Persian glyph atlas"),
            &rgba,
        )?;
        let bind_group = bind_group::uniform_texture_sampler_bind_group(
            device,
            Some("matrix glyph atlas bind group"),
            layout,
            uniform_buffer,
            &texture,
        );
        Ok(Self {
            _texture: texture,
            bind_group,
        })
    }
}

struct GpuPortrait {
    _texture: texture::Texture,
    bind_group: wgpu::BindGroup,
    aspect_ratio: f32,
}

impl GpuPortrait {
    fn from_image(
        context: &RenderContext,
        layout: &wgpu::BindGroupLayout,
        uniform_buffer: &wgpu::Buffer,
        image: RgbaKtxImage,
    ) -> RenderResult<Self> {
        let aspect_ratio = image.aspect_ratio();
        let rgba = texture::ImageRgba8::new(image.width, image.height, image.rgba)?;
        let texture = texture::Texture::from_rgba8_2d(
            &context.device,
            &context.queue,
            Some("Pooya KTX portrait"),
            &rgba,
        )?;
        let bind_group = bind_group::uniform_texture_sampler_bind_group(
            &context.device,
            Some("Pooya portrait bind group"),
            layout,
            uniform_buffer,
            &texture,
        );
        Ok(Self {
            _texture: texture,
            bind_group,
            aspect_ratio,
        })
    }

    fn transparent_placeholder(
        context: &RenderContext,
        layout: &wgpu::BindGroupLayout,
        uniform_buffer: &wgpu::Buffer,
    ) -> RenderResult<Self> {
        let mut portrait = Self::from_image(
            context,
            layout,
            uniform_buffer,
            RgbaKtxImage {
                width: 1,
                height: 1,
                rgba: vec![0, 0, 0, 0],
            },
        )?;
        // The production KTX is a square alpha canvas. Retain the same layout
        // while the asynchronous browser fetch is pending.
        portrait.aspect_ratio = 1.0;
        Ok(portrait)
    }
}

struct GpuTextMesh {
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    index_count: u32,
    cell_advance: f32,
    links: Rc<Vec<TerminalLinkHitbox>>,
}

impl GpuTextMesh {
    fn from_mesh(device: &wgpu::Device, mesh: &TerminalMesh) -> Self {
        Self {
            vertex_buffer: buffer::vertex_buffer(
                device,
                Some("timeline text vertices"),
                &mesh.vertices,
            ),
            index_buffer: buffer::index_buffer(
                device,
                Some("timeline text indices"),
                &mesh.indices,
            ),
            index_count: mesh.indices.len() as u32,
            cell_advance: mesh.cell_advance,
            links: Rc::new(mesh.links.clone()),
        }
    }
}

#[derive(Default)]
struct TerminalMesh {
    vertices: Vec<TerminalVertex>,
    indices: Vec<u32>,
    cell_advance: f32,
    links: Vec<TerminalLinkHitbox>,
}

#[derive(Clone, Debug)]
struct TerminalLinkHitbox {
    min: glam::Vec2,
    max: glam::Vec2,
    end_character: usize,
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    url: String,
}

#[cfg(target_arch = "wasm32")]
#[derive(Clone)]
struct LinkPickSnapshot {
    viewport: glam::Vec2,
    view_projection: glam::Mat4,
    model: glam::Mat4,
    typed_characters: usize,
    opacity: f32,
    links: Rc<Vec<TerminalLinkHitbox>>,
}

impl TerminalLinkHitbox {
    fn contains(&self, point: glam::Vec2) -> bool {
        point.x >= self.min.x
            && point.x <= self.max.x
            && point.y >= self.min.y
            && point.y <= self.max.y
    }
}

#[derive(Clone, Copy)]
struct TouchGesture {
    id: u64,
    start: glam::Vec2,
}

struct Portfolio {
    pending_portrait: PendingPortrait,
    pipelines: Option<Pipelines>,
    matrix_uniform_buffer: Option<wgpu::Buffer>,
    matrix_glyph_atlas: Option<GpuMatrixGlyphAtlas>,
    portrait_bind_group_layout: Option<wgpu::BindGroupLayout>,
    portrait_uniform_buffer: Option<wgpu::Buffer>,
    text_uniform_buffer: Option<wgpu::Buffer>,
    text_bind_group: Option<wgpu::BindGroup>,
    portrait: Option<GpuPortrait>,
    timeline_slides: Vec<TimelineSlide>,
    timeline_meshes: Vec<Option<GpuTextMesh>>,
    mobile_timeline_line_limit: Option<usize>,
    text_overlay: Option<text::TextOverlay>,
    copyright_notice: String,
    depth: Option<texture::Texture>,
    frame_stats: FrameStats,
    elapsed: f32,
    matrix_signal: MatrixSignalSchedule,
    section_age: f32,
    slide_progress: f32,
    current_slide: usize,
    wheel_accumulator: f32,
    cursor_position: Option<glam::Vec2>,
    touch_gesture: Option<TouchGesture>,
    hovered_link: Option<usize>,
    portrait_load_consumed: bool,
    has_encoded_frame: bool,
    renderer_ready_dispatched: bool,
}

impl Portfolio {
    fn new(pending_portrait: PendingPortrait) -> Self {
        Self {
            pending_portrait,
            pipelines: None,
            matrix_uniform_buffer: None,
            matrix_glyph_atlas: None,
            portrait_bind_group_layout: None,
            portrait_uniform_buffer: None,
            text_uniform_buffer: None,
            text_bind_group: None,
            portrait: None,
            timeline_slides: Vec::new(),
            timeline_meshes: Vec::new(),
            mobile_timeline_line_limit: None,
            text_overlay: None,
            copyright_notice: copyright_notice(),
            depth: None,
            frame_stats: FrameStats::new(),
            elapsed: 0.0,
            matrix_signal: MatrixSignalSchedule::with_seed(matrix_signal_seed()),
            section_age: 0.0,
            slide_progress: 0.0,
            current_slide: 0,
            wheel_accumulator: 0.0,
            cursor_position: None,
            touch_gesture: None,
            hovered_link: None,
            portrait_load_consumed: false,
            has_encoded_frame: false,
            renderer_ready_dispatched: false,
        }
    }

    fn navigate(&mut self, delta: i32) {
        if self.timeline_meshes.is_empty() || delta == 0 {
            return;
        }
        self.elapsed = self.elapsed.max(TIMELINE_START);
        let count = self.timeline_meshes.len() as i32;
        self.current_slide = (self.current_slide as i32 + delta).rem_euclid(count) as usize;
        self.section_age = 0.0;
        self.slide_progress = if REDUCED_MOTION.load(Ordering::Relaxed) {
            1.0
        } else {
            0.0
        };
        self.hovered_link = None;
        self.touch_gesture = None;
        self.dispatch_timeline_change();
    }

    fn perform_primary_action(&mut self) {
        let Some((total_characters, typing_duration)) = self
            .timeline_slides
            .get(self.current_slide)
            .map(|slide| (slide.character_count(), slide.typing_duration()))
        else {
            return;
        };
        match terminal_primary_action(self.visible_typed_characters(), total_characters) {
            TerminalPrimaryAction::Reveal => {
                // A primary action during typing completes the current page. It
                // also completes its entrance opacity/translation so the whole
                // terminal becomes readable in the same rendered frame.
                self.elapsed = self.elapsed.max(TIMELINE_START + 0.42);
                self.slide_progress = 1.0;
                self.section_age = typing_duration + f32::EPSILON;
                self.hovered_link = None;
            }
            TerminalPrimaryAction::Advance => self.navigate(1),
        }
    }

    fn jump_to(&mut self, index: usize) {
        if self.timeline_meshes.is_empty() {
            return;
        }
        self.elapsed = self.elapsed.max(TIMELINE_START);
        self.current_slide = index.min(self.timeline_meshes.len() - 1);
        self.section_age = 0.0;
        self.slide_progress = if REDUCED_MOTION.load(Ordering::Relaxed) {
            1.0
        } else {
            0.0
        };
        self.hovered_link = None;
        self.touch_gesture = None;
        self.dispatch_timeline_change();
    }

    fn consume_portrait_load(&mut self, context: &RenderContext) {
        if self.portrait_load_consumed {
            return;
        }
        let result = self.pending_portrait.borrow_mut().take();
        let Some(result) = result else {
            return;
        };
        self.portrait_load_consumed = true;
        match result {
            Ok(image) => {
                let dimensions = (image.width, image.height);
                let Some(layout) = self.portrait_bind_group_layout.as_ref() else {
                    log_message("Portrait layout is unavailable; keeping transparent fallback");
                    return;
                };
                let Some(uniform_buffer) = self.portrait_uniform_buffer.as_ref() else {
                    log_message("Portrait uniforms are unavailable; keeping transparent fallback");
                    return;
                };
                match GpuPortrait::from_image(context, layout, uniform_buffer, image) {
                    Ok(portrait) => {
                        log_message(&format!(
                            "Loaded Matrix portrait from {} ({}x{})",
                            portrait::PORTRAIT_KTX_URL,
                            dimensions.0,
                            dimensions.1,
                        ));
                        self.portrait = Some(portrait);
                    }
                    Err(error) => log_message(&format!(
                        "Portrait upload failed ({error}); keeping transparent fallback",
                    )),
                }
            }
            Err(error) => {
                log_message(&format!(
                    "Portrait asset unavailable ({error}); keeping transparent fallback",
                ));
            }
        }
    }

    fn update_uniforms(&self, context: &RenderContext) {
        let reduced = REDUCED_MOTION.load(Ordering::Relaxed);
        let motion = if reduced { 0.0 } else { 1.0 };
        let aspect = context.aspect_ratio().max(0.01);
        let (view_projection, camera_distance) = responsive_camera(aspect);
        let matrix = MatrixUniforms {
            timing: [
                self.elapsed,
                smoothstep(0.0, 0.55, self.elapsed),
                motion,
                19.86,
            ],
            viewport: [
                context.surface_config.width as f32,
                context.surface_config.height as f32,
                aspect,
                (context.surface_config.width as f32 / 15.0).clamp(34.0, 112.0),
            ],
            // start time, travel duration, normalized column, active flag
            signal: self.matrix_signal.uniforms(self.elapsed, !reduced),
        };

        let reveal = smoothstep(MATRIX_INTRO_END, FACE_REVEAL_END, self.elapsed);
        let texture_aspect = self
            .portrait
            .as_ref()
            .map(|portrait| portrait.aspect_ratio)
            .unwrap_or(1.0);
        let portrait = PortraitUniforms {
            timing: [self.elapsed, reveal, reveal, motion],
            viewport: [
                context.surface_config.width as f32,
                context.surface_config.height as f32,
                aspect,
                texture_aspect,
            ],
            placement: portrait_placement(aspect, texture_aspect),
            // The local converter aspect-fits the original portrait into a
            // square canvas. These source-calibrated positions are used only
            // for subtle shader iris accents; the photograph remains
            // the source of facial identity.
            eyes: [0.402, 0.435, 0.619, 0.433],
        };

        let terminal_lines = self
            .timeline_slides
            .get(self.current_slide)
            .map(TimelineSlide::line_count)
            .unwrap_or(1);
        let text_model =
            terminal_model(aspect, camera_distance, self.slide_progress, terminal_lines);
        let text_opacity = smoothstep(TIMELINE_START, TIMELINE_START + 0.42, self.elapsed)
            * smoothstep(0.0, 0.38, self.slide_progress);
        let typed_characters = self
            .timeline_slides
            .get(self.current_slide)
            .map(|terminal| {
                let total = terminal.character_count();
                (if reduced {
                    total
                } else {
                    terminal.typed_characters_at(self.section_age)
                }) as f32
            })
            .unwrap_or_default();
        let text = TextUniforms {
            view_projection: view_projection.to_cols_array_2d(),
            model: text_model.to_cols_array_2d(),
            params: [self.elapsed, text_opacity, typed_characters, motion],
        };

        if let Some(buffer) = &self.matrix_uniform_buffer {
            context
                .queue
                .write_buffer(buffer, 0, bytemuck::bytes_of(&matrix));
        }
        if let Some(buffer) = &self.portrait_uniform_buffer {
            context
                .queue
                .write_buffer(buffer, 0, bytemuck::bytes_of(&portrait));
        }
        if let Some(buffer) = &self.text_uniform_buffer {
            context
                .queue
                .write_buffer(buffer, 0, bytemuck::bytes_of(&text));
        }
        update_browser_link_snapshot(
            context,
            view_projection,
            text_model,
            typed_characters as usize,
            text_opacity,
            self.timeline_meshes
                .get(self.current_slide)
                .and_then(Option::as_ref)
                .map(|mesh| &mesh.links),
        );
    }

    fn prepare_text_overlay(&mut self, context: &RenderContext) -> RenderResult<()> {
        let Some(slide) = self.timeline_slides.get(self.current_slide) else {
            return Ok(());
        };
        let Some(text_mesh) = self
            .timeline_meshes
            .get(self.current_slide)
            .and_then(Option::as_ref)
        else {
            return Ok(());
        };
        let reduced = REDUCED_MOTION.load(Ordering::Relaxed);
        let aspect = context.aspect_ratio().max(0.01);
        let (view_projection, camera_distance) = responsive_camera(aspect);
        let model = terminal_model(
            aspect,
            camera_distance,
            self.slide_progress,
            slide.line_count(),
        );
        let viewport = glam::Vec2::new(
            context.surface_config.width as f32,
            context.surface_config.height as f32,
        );
        let max_columns = slide
            .terminal
            .lines()
            .map(|line| line.chars().count())
            .max()
            .unwrap_or(1);
        let opacity = smoothstep(TIMELINE_START, TIMELINE_START + 0.42, self.elapsed)
            * smoothstep(0.0, 0.38, self.slide_progress);
        let Some(layout) = terminal_overlay_layout(
            viewport,
            view_projection,
            model,
            slide.line_count(),
            max_columns,
            text_mesh.cell_advance,
        ) else {
            return Ok(());
        };

        let typed_characters = if reduced {
            slide.character_count()
        } else {
            slide.typed_characters_at(self.section_age)
        };
        let visible = slide
            .terminal
            .chars()
            .take(typed_characters)
            .collect::<String>();
        let mut white_layer = visible.chars().collect::<Vec<_>>();
        for link in &slide.links {
            let visible_end = link.end_character.min(typed_characters);
            for character in white_layer
                .iter_mut()
                .take(visible_end)
                .skip(link.start_character)
            {
                *character = ' ';
            }
        }
        let cursor_visible = reduced || (self.elapsed * 1.45).fract() >= 0.42;
        if cursor_visible {
            white_layer.push('█');
        }

        let copyright_layout =
            copyright_overlay_layout(viewport, context.window.scale_factor() as f32);
        let copyright_notice = self.copyright_notice.as_str();

        let overlay = self
            .text_overlay
            .as_mut()
            .ok_or_else(|| RenderError::message("terminal text overlay is not initialized"))?;
        overlay.clear();
        overlay.add_text(
            &white_layer.into_iter().collect::<String>(),
            terminal_overlay_style(layout.font_size, layout.line_height, opacity, false),
            layout.placement,
        );
        for link in &slide.links {
            let visible_characters = typed_characters
                .saturating_sub(link.start_character)
                .min(link.end_character - link.start_character);
            if visible_characters == 0 {
                continue;
            }
            let link_text = slide
                .terminal
                .chars()
                .skip(link.start_character)
                .take(visible_characters)
                .collect::<String>();
            let mut placement = layout.placement;
            placement.left += link.start_column as f32 * layout.cell_advance;
            placement.top += link.line as f32 * layout.line_height;
            placement.width =
                (link.end_column - link.start_column + 1) as f32 * layout.cell_advance;
            placement.height = layout.line_height * 1.25;
            overlay.add_text(
                &link_text,
                terminal_overlay_style(layout.font_size, layout.line_height, opacity, true),
                placement,
            );
        }
        overlay.add_text(
            copyright_notice,
            copyright_overlay_style(copyright_layout.font_size, copyright_layout.line_height),
            copyright_layout.placement,
        );
        overlay.prepare(context)
    }

    fn dispatch_timeline_change(&self) {
        let Some(slide) = self.timeline_slides.get(self.current_slide) else {
            return;
        };
        dispatch_timeline_event(
            self.current_slide,
            self.timeline_slides.len(),
            &slide.eyebrow,
            &slide.heading,
            &slide.summary,
        );
    }

    fn visible_typed_characters(&self) -> usize {
        let Some(slide) = self.timeline_slides.get(self.current_slide) else {
            return 0;
        };
        if REDUCED_MOTION.load(Ordering::Relaxed) {
            slide.character_count()
        } else {
            slide.typed_characters_at(self.section_age)
        }
    }

    fn hit_test_link(&self, context: &RenderContext, screen: glam::Vec2) -> Option<usize> {
        if self.elapsed < TIMELINE_START || self.slide_progress < 0.12 {
            return None;
        }
        let text_mesh = self
            .timeline_meshes
            .get(self.current_slide)
            .and_then(Option::as_ref)?;
        let aspect = context.aspect_ratio().max(0.01);
        let (view_projection, camera_distance) = responsive_camera(aspect);
        let terminal_lines = self
            .timeline_slides
            .get(self.current_slide)
            .map(TimelineSlide::line_count)
            .unwrap_or(1);
        let model = terminal_model(aspect, camera_distance, self.slide_progress, terminal_lines);
        let viewport = glam::Vec2::new(
            context.surface_config.width as f32,
            context.surface_config.height as f32,
        );
        let typed = self.visible_typed_characters();
        let opacity = smoothstep(TIMELINE_START, TIMELINE_START + 0.42, self.elapsed)
            * smoothstep(0.0, 0.38, self.slide_progress);
        pick_terminal_link(
            screen,
            viewport,
            view_projection,
            model,
            typed,
            opacity,
            text_mesh.links.as_slice(),
        )
    }

    fn update_hovered_link(&mut self, context: &RenderContext, screen: glam::Vec2) {
        let hovered = self.hit_test_link(context, screen);
        if hovered == self.hovered_link {
            return;
        }
        self.hovered_link = hovered;
        context.window.set_cursor(if hovered.is_some() {
            winit::window::CursorIcon::Pointer
        } else {
            winit::window::CursorIcon::Default
        });
    }

    fn rebuild_timeline_geometry(
        &mut self,
        context: &RenderContext,
        mobile_line_limit: Option<usize>,
    ) -> RenderResult<()> {
        let previous_position = self.timeline_slides.get(self.current_slide).map(|slide| {
            (
                slide.primary_entry_id().to_owned(),
                slide.source_line_start(),
                (self.section_age / slide.typing_duration().max(f32::EPSILON)).clamp(0.0, 1.0),
                self.slide_progress,
            )
        });
        let slides = if let Some(max_lines) = mobile_line_limit {
            timeline::load_mobile_slides(max_lines)
        } else {
            timeline::load_slides()
        }
        .map_err(RenderError::message)?;
        let current_slide = previous_position
            .as_ref()
            .and_then(|(entry_id, source_line_start, _, _)| {
                slides.iter().position(|slide| {
                    slide.contains_entry(entry_id)
                        && slide.source_line_start() == *source_line_start
                })
            })
            .or_else(|| {
                previous_position
                    .as_ref()
                    .and_then(|(entry_id, source_line_start, _, _)| {
                        slides
                            .iter()
                            .enumerate()
                            .filter(|(_, slide)| slide.contains_entry(entry_id))
                            .min_by_key(|(_, slide)| {
                                slide.source_line_start().abs_diff(*source_line_start)
                            })
                            .map(|(index, _)| index)
                    })
            })
            .unwrap_or(0);
        let meshes = if mobile_line_limit.is_some() {
            let current_mesh = slides
                .get(current_slide)
                .map(build_timeline_mesh)
                .transpose()?
                .map(|mesh| GpuTextMesh::from_mesh(&context.device, &mesh));
            let mut meshes = (0..slides.len()).map(|_| None).collect::<Vec<_>>();
            if let Some(current_mesh) = current_mesh {
                meshes[current_slide] = Some(current_mesh);
            }
            meshes
        } else {
            slides
                .iter()
                .map(build_timeline_mesh)
                .collect::<RenderResult<Vec<_>>>()?
                .iter()
                .map(|mesh| Some(GpuTextMesh::from_mesh(&context.device, mesh)))
                .collect()
        };

        self.timeline_slides = slides;
        self.timeline_meshes = meshes;
        self.mobile_timeline_line_limit = mobile_line_limit;
        self.current_slide = current_slide;
        self.section_age = if let Some((_, _, typing_progress, _)) = previous_position.as_ref() {
            self.timeline_slides[current_slide].typing_duration() * typing_progress
        } else {
            0.0
        };
        self.slide_progress = previous_position
            .map(|(_, _, _, slide_progress)| slide_progress)
            .unwrap_or_else(|| {
                if REDUCED_MOTION.load(Ordering::Relaxed) {
                    1.0
                } else {
                    0.0
                }
            });
        self.hovered_link = None;
        self.touch_gesture = None;
        Ok(())
    }

    fn ensure_timeline_mesh(
        &mut self,
        context: &RenderContext,
        slide_index: usize,
    ) -> RenderResult<()> {
        let Some(slot) = self.timeline_meshes.get(slide_index) else {
            return Ok(());
        };
        if slot.is_some() {
            return Ok(());
        }
        let Some(slide) = self.timeline_slides.get(slide_index) else {
            return Ok(());
        };
        let mesh = build_timeline_mesh(slide)?;
        self.timeline_meshes[slide_index] = Some(GpuTextMesh::from_mesh(&context.device, &mesh));
        Ok(())
    }
}

impl Example for Portfolio {
    fn settings(&self) -> ExampleSettings {
        ExampleSettings {
            title: "Pooya Eimandar · Rust/WebGPU Timeline".to_owned(),
            initial_size: winit::dpi::PhysicalSize::new(1440, 900),
            ..Default::default()
        }
    }

    fn init(&mut self, context: &mut RenderContext) -> RenderResult<()> {
        #[cfg(target_arch = "wasm32")]
        let validation_scope = context
            .device
            .push_error_scope(wgpu::ErrorFilter::Validation);
        mount_browser_canvas(context);
        dispatch_scene_progress(
            "matrix",
            28,
            "WebGPU device acquired. Preparing the Persian glyph atlas.",
        );

        let matrix_layout = bind_group::uniform_texture_sampler_layout(
            &context.device,
            Some("matrix uniform texture layout"),
            wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
            wgpu::ShaderStages::FRAGMENT,
            wgpu::TextureViewDimension::D2,
        );
        let portrait_layout = bind_group::uniform_texture_sampler_layout(
            &context.device,
            Some("portrait uniform texture layout"),
            wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
            wgpu::ShaderStages::FRAGMENT,
            wgpu::TextureViewDimension::D2,
        );
        let text_layout = bind_group::uniform_layout(
            &context.device,
            Some("timeline text uniform layout"),
            wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
        );

        let matrix_shader = shader::wgsl_module(
            &context.device,
            Some("procedural matrix rain shader"),
            include_str!("../assets/shaders/matrix.wgsl"),
        );
        let portrait_shader = shader::wgsl_module(
            &context.device,
            Some("KTX Persian Matrix portrait shader"),
            include_str!("../assets/shaders/portrait.wgsl"),
        );
        let text_shader = shader::wgsl_module(
            &context.device,
            Some("timeline 3d text shader"),
            include_str!("../assets/shaders/timeline_text.wgsl"),
        );
        dispatch_scene_progress("face", 44, "Shaders loaded. Uploading Matrix textures.");

        let initial_matrix = MatrixUniforms::zeroed();
        let initial_portrait = PortraitUniforms::zeroed();
        let initial_text = TextUniforms::zeroed();
        let matrix_uniform_buffer =
            buffer::uniform_buffer(&context.device, Some("matrix uniforms"), &initial_matrix);
        let portrait_uniform_buffer = buffer::uniform_buffer(
            &context.device,
            Some("portrait uniforms"),
            &initial_portrait,
        );
        let text_uniform_buffer = buffer::uniform_buffer(
            &context.device,
            Some("timeline text uniforms"),
            &initial_text,
        );
        let matrix_glyph_atlas = GpuMatrixGlyphAtlas::new(
            &context.device,
            &context.queue,
            &matrix_layout,
            &matrix_uniform_buffer,
        )
        .map_err(report_renderer_init_error)?;
        let text_bind_group = bind_group::uniform_bind_group(
            &context.device,
            Some("timeline text uniform bind group"),
            &text_layout,
            &text_uniform_buffer,
        );

        let portrait = GpuPortrait::transparent_placeholder(
            context,
            &portrait_layout,
            &portrait_uniform_buffer,
        )
        .map_err(report_renderer_init_error)?;
        dispatch_scene_progress(
            "face",
            60,
            "Textures ready. Building the timeline geometry.",
        );
        self.pipelines = Some(Pipelines {
            matrix: create_matrix_pipeline(context, &matrix_layout, &matrix_shader),
            portrait: create_portrait_pipeline(context, &portrait_layout, &portrait_shader),
            text: create_text_pipeline(context, &text_layout, &text_shader),
        });
        self.matrix_uniform_buffer = Some(matrix_uniform_buffer);
        self.matrix_glyph_atlas = Some(matrix_glyph_atlas);
        self.portrait_bind_group_layout = Some(portrait_layout);
        self.portrait_uniform_buffer = Some(portrait_uniform_buffer);
        self.portrait = Some(portrait);
        self.text_uniform_buffer = Some(text_uniform_buffer);
        self.text_bind_group = Some(text_bind_group);

        self.rebuild_timeline_geometry(
            context,
            responsive_mobile_line_limit(
                glam::Vec2::new(
                    context.surface_config.width as f32,
                    context.surface_config.height as f32,
                ),
                context.window.scale_factor() as f32,
            ),
        )
        .map_err(report_renderer_init_error)?;
        dispatch_scene_progress(
            "timeline",
            76,
            "Timeline geometry built. Preparing text shaping.",
        );
        self.text_overlay = Some(
            text::TextOverlay::with_font_data(context, [FONT_BYTES.to_vec()])
                .map_err(report_renderer_init_error)?,
        );
        self.depth = Some(texture::Texture::depth(
            &context.device,
            &context.surface_config,
        ));
        self.update_uniforms(context);
        dispatch_scene_progress(
            "timeline",
            90,
            "Renderer initialized. Waiting for the first frame.",
        );
        #[cfg(target_arch = "wasm32")]
        watch_webgpu_validation_scope(validation_scope);

        Ok(())
    }

    fn resize(&mut self, context: &mut RenderContext, _size: winit::dpi::PhysicalSize<u32>) {
        self.depth = Some(texture::Texture::depth(
            &context.device,
            &context.surface_config,
        ));
        let mobile_line_limit = responsive_mobile_line_limit(
            glam::Vec2::new(
                context.surface_config.width as f32,
                context.surface_config.height as f32,
            ),
            context.window.scale_factor() as f32,
        );
        if mobile_line_limit != self.mobile_timeline_line_limit {
            match self.rebuild_timeline_geometry(context, mobile_line_limit) {
                Ok(()) if self.renderer_ready_dispatched => self.dispatch_timeline_change(),
                Ok(()) => {}
                Err(error) => log_message(&format!(
                    "Could not rebuild the responsive timeline geometry: {error}"
                )),
            }
        }
        self.update_uniforms(context);
    }

    fn input(&mut self, context: &mut RenderContext, event: &winit::event::WindowEvent) -> bool {
        use winit::event::{ElementState, MouseButton, MouseScrollDelta, TouchPhase, WindowEvent};
        use winit::keyboard::{Key, NamedKey};

        match event {
            WindowEvent::KeyboardInput { event, .. }
                if event.state == ElementState::Pressed && !event.repeat =>
            {
                match &event.logical_key {
                    Key::Named(NamedKey::Enter) | Key::Named(NamedKey::Space) => {
                        PRIMARY_ACTION_REQUEST.store(true, Ordering::Relaxed)
                    }
                    Key::Named(NamedKey::ArrowRight)
                    | Key::Named(NamedKey::ArrowDown)
                    | Key::Named(NamedKey::PageDown) => self.navigate(1),
                    Key::Named(NamedKey::ArrowLeft)
                    | Key::Named(NamedKey::ArrowUp)
                    | Key::Named(NamedKey::PageUp) => self.navigate(-1),
                    Key::Named(NamedKey::Home) => self.jump_to(0),
                    Key::Named(NamedKey::End) => {
                        self.jump_to(self.timeline_meshes.len().saturating_sub(1))
                    }
                    _ => return false,
                }
                true
            }
            WindowEvent::MouseInput {
                state: ElementState::Released,
                button: MouseButton::Left,
                ..
            } => {
                let is_visible_link = self
                    .cursor_position
                    .and_then(|screen| self.hit_test_link(context, screen))
                    .is_some();
                if !is_visible_link {
                    PRIMARY_ACTION_REQUEST.store(true, Ordering::Relaxed);
                }
                true
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let amount = match delta {
                    MouseScrollDelta::LineDelta(_, y) => *y,
                    MouseScrollDelta::PixelDelta(position) => position.y as f32 / 52.0,
                };
                self.wheel_accumulator += amount;
                if self.wheel_accumulator.abs() >= 0.72 {
                    self.navigate(if self.wheel_accumulator < 0.0 { 1 } else { -1 });
                    self.wheel_accumulator = 0.0;
                }
                true
            }
            WindowEvent::CursorMoved { position, .. } => {
                let screen = glam::Vec2::new(position.x as f32, position.y as f32);
                self.cursor_position = Some(screen);
                self.update_hovered_link(context, screen);
                true
            }
            WindowEvent::CursorLeft { .. } => {
                self.cursor_position = None;
                self.hovered_link = None;
                context
                    .window
                    .set_cursor(winit::window::CursorIcon::Default);
                true
            }
            WindowEvent::Touch(touch) => {
                let screen = glam::Vec2::new(touch.location.x as f32, touch.location.y as f32);
                match touch.phase {
                    TouchPhase::Started => {
                        if self.touch_gesture.is_none() {
                            self.touch_gesture = Some(TouchGesture {
                                id: touch.id,
                                start: screen,
                            });
                        }
                        self.update_hovered_link(context, screen);
                    }
                    TouchPhase::Moved => {
                        if self
                            .touch_gesture
                            .is_some_and(|gesture| gesture.id == touch.id)
                        {
                            self.update_hovered_link(context, screen);
                        }
                    }
                    TouchPhase::Ended => {
                        if self
                            .touch_gesture
                            .is_some_and(|gesture| gesture.id == touch.id)
                        {
                            let gesture = self.touch_gesture.take().expect("gesture was present");
                            let travel = screen - gesture.start;
                            let swipe_threshold = 34.0 * context.window.scale_factor() as f32;
                            if travel.y.abs() >= swipe_threshold && travel.y.abs() > travel.x.abs()
                            {
                                self.navigate(if travel.y < 0.0 { 1 } else { -1 });
                            } else if self.hit_test_link(context, screen).is_none() {
                                PRIMARY_ACTION_REQUEST.store(true, Ordering::Relaxed);
                            }
                        }
                        self.hovered_link = None;
                    }
                    TouchPhase::Cancelled => {
                        if self
                            .touch_gesture
                            .is_some_and(|gesture| gesture.id == touch.id)
                        {
                            self.touch_gesture = None;
                        }
                        self.hovered_link = None;
                    }
                }
                true
            }
            WindowEvent::Focused(false) => {
                self.wheel_accumulator = 0.0;
                self.touch_gesture = None;
                self.hovered_link = None;
                context
                    .window
                    .set_cursor(winit::window::CursorIcon::Default);
                false
            }
            _ => false,
        }
    }

    fn update(&mut self, context: &mut RenderContext) {
        if self.has_encoded_frame && !self.renderer_ready_dispatched {
            self.renderer_ready_dispatched = true;
            dispatch_renderer_ready(self.timeline_slides.len());
            self.dispatch_timeline_change();
        }

        self.frame_stats.tick();
        let dt = self.frame_stats.delta_seconds().clamp(0.0, 1.0 / 15.0);
        let reduced = REDUCED_MOTION.load(Ordering::Relaxed);
        let primary_requested = PRIMARY_ACTION_REQUEST.swap(false, Ordering::Relaxed);
        if primary_requested {
            self.perform_primary_action();
        }

        if let Err(error) = self.ensure_timeline_mesh(context, self.current_slide) {
            log_message(&format!(
                "Could not build timeline page {}: {error}",
                self.current_slide + 1
            ));
        }
        let current_page_complete = self
            .timeline_slides
            .get(self.current_slide)
            .is_some_and(|slide| self.visible_typed_characters() >= slide.character_count());
        if current_page_complete && !self.timeline_meshes.is_empty() {
            let next_slide = (self.current_slide + 1) % self.timeline_meshes.len();
            if let Err(error) = self.ensure_timeline_mesh(context, next_slide) {
                log_message(&format!(
                    "Could not prepare timeline page {}: {error}",
                    next_slide + 1
                ));
            }
        }

        if reduced {
            self.elapsed = self.elapsed.max(TIMELINE_START + 1.0);
            self.slide_progress = 1.0;
        } else {
            self.elapsed += dt;
            if self.elapsed >= TIMELINE_START {
                self.slide_progress = (self.slide_progress + dt / SLIDE_DURATION).min(1.0);
                self.section_age += dt;
            }
        }
        self.matrix_signal.update(self.elapsed);

        self.consume_portrait_load(context);
        if let Some(screen) = self.cursor_position {
            self.update_hovered_link(context, screen);
        }
        self.update_uniforms(context);
    }

    fn render(
        &mut self,
        context: &mut RenderContext,
        view: &wgpu::TextureView,
        encoder: &mut wgpu::CommandEncoder,
    ) -> RenderResult<()> {
        self.prepare_text_overlay(context)?;
        let pipelines = self
            .pipelines
            .as_ref()
            .ok_or_else(|| RenderError::message("portfolio pipelines are not initialized"))?;
        let matrix_glyph_atlas = self
            .matrix_glyph_atlas
            .as_ref()
            .ok_or_else(|| RenderError::message("matrix glyph atlas is not initialized"))?;
        let text_bind_group = self
            .text_bind_group
            .as_ref()
            .ok_or_else(|| RenderError::message("text bind group is not initialized"))?;
        let portrait = self
            .portrait
            .as_ref()
            .ok_or_else(|| RenderError::message("portrait texture is not initialized"))?;
        let depth = self
            .depth
            .as_ref()
            .ok_or_else(|| RenderError::message("depth texture is not initialized"))?;

        let mut pass = render_pass::begin_color_depth(
            encoder,
            Some("portfolio matrix scene"),
            view,
            Some(&depth.view),
            wgpu::Color::BLACK,
            1.0,
        );
        pass.set_pipeline(&pipelines.matrix);
        pass.set_bind_group(0, &matrix_glyph_atlas.bind_group, &[]);
        pass.draw(0..3, 0..1);

        if self.elapsed >= MATRIX_INTRO_END - 0.2 {
            pass.set_pipeline(&pipelines.portrait);
            pass.set_bind_group(0, &portrait.bind_group, &[]);
            pass.draw(0..3, 0..1);
        }

        if self.elapsed >= TIMELINE_START - 0.1
            && let Some(text_mesh) = self
                .timeline_meshes
                .get(self.current_slide)
                .and_then(Option::as_ref)
        {
            pass.set_pipeline(&pipelines.text);
            pass.set_bind_group(0, text_bind_group, &[]);
            pass.set_vertex_buffer(0, text_mesh.vertex_buffer.slice(..));
            pass.set_index_buffer(text_mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..text_mesh.index_count, 0, 0..1);
        }

        drop(pass);
        if let Some(text_overlay) = self.text_overlay.as_ref() {
            let mut text_pass =
                render_pass::begin_color_load(encoder, Some("WebGPU text overlay"), view);
            text_overlay.render(&mut text_pass)?;
        }
        self.has_encoded_frame = true;
        Ok(())
    }
}

fn build_timeline_mesh(slide: &TimelineSlide) -> RenderResult<TerminalMesh> {
    let options = text_mesh::TextMeshOptions {
        font_size: TERMINAL_FONT_SIZE,
        line_height: TERMINAL_LINE_HEIGHT,
        depth: TERMINAL_DEPTH,
        // Sib's 3D text mesh extrudes font contours. A proportional stroke
        // closes the gap between the inner and outer contour of each stem,
        // while the filled glyph-atlas pass below supplies the complete front
        // face (including antialiased joins and curves).
        stroke_width: TERMINAL_STROKE_WIDTH,
        curve_steps: 3,
        family: text_mesh::TextMeshFamily::Name("Fira Mono"),
        center: false,
        ..Default::default()
    };
    let pipe = text_mesh::TextMesh::from_font_bytes(FONT_BYTES, "|", [1.0; 4], options)?;
    let double_pipe = text_mesh::TextMesh::from_font_bytes(FONT_BYTES, "||", [1.0; 4], options)?;
    let cell_advance = double_pipe.bounds.width() - pipe.bounds.width();
    if !cell_advance.is_finite() || cell_advance <= f32::EPSILON {
        return Err(RenderError::message(
            "Fira Mono did not produce a valid terminal cell advance",
        ));
    }
    let pipe_center = glam::Vec2::new(pipe.bounds.center()[0], pipe.bounds.center()[1]);
    let first_cell_left = pipe_center.x - cell_advance * 0.5;
    let lines = slide.terminal.lines().collect::<Vec<_>>();
    let vertical_center =
        pipe_center.y - (lines.len().saturating_sub(1) as f32 * TERMINAL_LINE_HEIGHT * 0.5);
    let mut mesh = TerminalMesh {
        cell_advance,
        ..Default::default()
    };
    let mut character_offset = 0_usize;

    for (line_index, line) in lines.iter().enumerate() {
        let line_color = [1.0; 4];
        let line_mesh =
            text_mesh::TextMesh::from_font_bytes(FONT_BYTES, line, line_color, options)?;
        let base = u32::try_from(mesh.vertices.len())
            .map_err(|_| RenderError::message("terminal vertex count exceeds u32"))?;
        let line_character_count = line.chars().count().max(1);

        mesh.vertices
            .extend(line_mesh.vertices.iter().map(|vertex| {
                let column = ((vertex.position[0] - pipe_center.x) / cell_advance)
                    .round()
                    .clamp(0.0, line_character_count.saturating_sub(1) as f32)
                    as usize;
                let character_order = character_offset + column;
                TerminalVertex {
                    position: [
                        vertex.position[0] - first_cell_left,
                        vertex.position[1]
                            - line_index as f32 * TERMINAL_LINE_HEIGHT
                            - vertical_center,
                        vertex.position[2],
                    ],
                    color: terminal_character_color(slide, character_order),
                    meta: [character_order as f32, 0.0],
                }
            }));
        mesh.indices
            .extend(line_mesh.indices.iter().map(|index| base + index));
        character_offset += line.chars().count();
        if line_index + 1 < lines.len() {
            character_offset += 1;
        }
    }

    let mut row = 0_usize;
    let mut column = 0_usize;
    append_terminal_cursor(
        &mut mesh,
        0,
        row,
        column,
        cell_advance,
        pipe_center.y - vertical_center,
    )?;
    for (order, character) in slide.terminal.chars().enumerate() {
        if character == '\n' {
            row += 1;
            column = 0;
        } else {
            column += 1;
        }
        append_terminal_cursor(
            &mut mesh,
            order + 1,
            row,
            column,
            cell_advance,
            pipe_center.y - vertical_center,
        )?;
    }

    mesh.links = slide
        .links
        .iter()
        .map(|link| {
            let center_y =
                pipe_center.y - link.line as f32 * TERMINAL_LINE_HEIGHT - vertical_center;
            TerminalLinkHitbox {
                min: glam::Vec2::new(
                    link.start_column as f32 * cell_advance,
                    center_y - TERMINAL_LINE_HEIGHT * 0.48,
                ),
                max: glam::Vec2::new(
                    link.end_column as f32 * cell_advance,
                    center_y + TERMINAL_LINE_HEIGHT * 0.48,
                ),
                end_character: link.end_character,
                url: link.url.clone(),
            }
        })
        .collect();
    Ok(mesh)
}

fn terminal_character_color(slide: &TimelineSlide, character_order: usize) -> [f32; 4] {
    if slide
        .links
        .iter()
        .any(|link| character_order >= link.start_character && character_order < link.end_character)
    {
        [0.0, 1.0, 1.0, 1.0]
    } else {
        [1.0; 4]
    }
}

fn append_terminal_cursor(
    mesh: &mut TerminalMesh,
    order: usize,
    row: usize,
    column: usize,
    cell_advance: f32,
    first_line_center_y: f32,
) -> RenderResult<()> {
    let base = u32::try_from(mesh.vertices.len())
        .map_err(|_| RenderError::message("terminal cursor vertex count exceeds u32"))?;
    let center = glam::Vec2::new(
        (column as f32 + 0.5) * cell_advance,
        first_line_center_y - row as f32 * TERMINAL_LINE_HEIGHT,
    );
    let half_size = glam::Vec2::new(cell_advance * 0.36, TERMINAL_FONT_SIZE * 0.48);
    let z = TERMINAL_DEPTH + 0.008;
    let color = [1.0; 4];
    for position in [
        center + glam::Vec2::new(-half_size.x, -half_size.y),
        center + glam::Vec2::new(half_size.x, -half_size.y),
        center + glam::Vec2::new(half_size.x, half_size.y),
        center + glam::Vec2::new(-half_size.x, half_size.y),
    ] {
        mesh.vertices.push(TerminalVertex {
            position: [position.x, position.y, z],
            color,
            meta: [order as f32, 1.0],
        });
    }
    mesh.indices
        .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    Ok(())
}

fn create_matrix_pipeline(
    context: &RenderContext,
    layout: &wgpu::BindGroupLayout,
    shader: &wgpu::ShaderModule,
) -> wgpu::RenderPipeline {
    create_matrix_pipeline_for_format(
        &context.device,
        context.surface_config.format,
        layout,
        shader,
    )
}

fn create_matrix_pipeline_for_format(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    layout: &wgpu::BindGroupLayout,
    shader: &wgpu::ShaderModule,
) -> wgpu::RenderPipeline {
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("matrix pipeline layout"),
        bind_group_layouts: &[Some(layout)],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("procedural matrix rain pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(format.into())],
        }),
        primitive: wgpu::PrimitiveState {
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: texture::DEPTH_FORMAT,
            depth_write_enabled: Some(false),
            depth_compare: Some(wgpu::CompareFunction::Always),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    })
}

fn create_portrait_pipeline(
    context: &RenderContext,
    layout: &wgpu::BindGroupLayout,
    shader: &wgpu::ShaderModule,
) -> wgpu::RenderPipeline {
    let pipeline_layout = context
        .device
        .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("KTX Matrix portrait pipeline layout"),
            bind_group_layouts: &[Some(layout)],
            immediate_size: 0,
        });
    context
        .device
        .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("KTX Persian Matrix portrait pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: context.surface_config.format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: texture::DEPTH_FORMAT,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Always),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        })
}

fn create_text_pipeline(
    context: &RenderContext,
    layout: &wgpu::BindGroupLayout,
    shader: &wgpu::ShaderModule,
) -> wgpu::RenderPipeline {
    create_geometry_pipeline(
        context,
        "timeline 3d text pipeline",
        layout,
        shader,
        &[TerminalVertex::layout()],
    )
}

fn create_geometry_pipeline(
    context: &RenderContext,
    label: &'static str,
    layout: &wgpu::BindGroupLayout,
    shader: &wgpu::ShaderModule,
    vertex_buffers: &[wgpu::VertexBufferLayout<'static>],
) -> wgpu::RenderPipeline {
    let pipeline_layout = context
        .device
        .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some(label),
            bind_group_layouts: &[Some(layout)],
            immediate_size: 0,
        });
    context
        .device
        .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(label),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: vertex_buffers,
            },
            fragment: Some(wgpu::FragmentState {
                module: shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: context.surface_config.format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: texture::DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        })
}

fn responsive_camera(aspect: f32) -> (glam::Mat4, f32) {
    let distance = if aspect < 0.72 {
        11.6
    } else if aspect < 1.15 {
        10.0
    } else {
        8.4
    };
    let eye = glam::Vec3::new(0.0, 0.0, distance);
    let view = glam::camera::rh::view::look_at_mat4(eye, glam::Vec3::ZERO, glam::Vec3::Y);
    let projection = glam::camera::rh::proj::directx::perspective(
        44.0_f32.to_radians(),
        aspect.max(0.01),
        0.1,
        64.0,
    );
    (projection * view, distance)
}

/// Returns top-left normalized screen placement as center-x, center-y, width,
/// and height. The KTX uses a transparent square canvas, so pixel aspect is
/// preserved explicitly instead of stretching the portrait on mobile.
fn portrait_placement(viewport_aspect: f32, texture_aspect: f32) -> [f32; 4] {
    let viewport_aspect = viewport_aspect.max(0.01);
    let texture_aspect = texture_aspect.max(0.01);
    if viewport_aspect >= 1.15 {
        let height = 1.04;
        let width = height * texture_aspect / viewport_aspect;
        [0.73, 0.51, width, height]
    } else if viewport_aspect >= 0.80 {
        let height = 0.98;
        let width = height * texture_aspect / viewport_aspect;
        [0.62, 0.50, width, height]
    } else {
        let mut width = 1.55;
        let mut height = width * viewport_aspect / texture_aspect;
        if height > 1.05 {
            height = 1.05;
            width = height * texture_aspect / viewport_aspect;
        }
        [0.55, 0.50, width, height]
    }
}

#[derive(Clone, Copy, Debug)]
struct TerminalOverlayLayout {
    placement: text::TextPlacement,
    font_size: f32,
    line_height: f32,
    cell_advance: f32,
}

#[derive(Clone, Copy, Debug)]
struct CopyrightOverlayLayout {
    placement: text::TextPlacement,
    font_size: f32,
    line_height: f32,
}

fn copyright_overlay_layout(viewport: glam::Vec2, scale_factor: f32) -> CopyrightOverlayLayout {
    let scale_factor = scale_factor.max(1.0);
    let logical_viewport = viewport / scale_factor;
    let horizontal_padding = (logical_viewport.x * 0.04).clamp(16.0, 48.0) * scale_factor;
    let font_size = if logical_viewport.x < 480.0 {
        11.0
    } else {
        12.0
    } * scale_factor;
    let line_height = font_size * 1.45;
    let bottom_margin = (logical_viewport.y * 0.028).clamp(18.0, 32.0) * scale_factor;
    CopyrightOverlayLayout {
        placement: text::TextPlacement {
            left: horizontal_padding,
            top: (viewport.y - bottom_margin - line_height).max(0.0),
            width: (viewport.x - horizontal_padding * 2.0).max(1.0),
            height: line_height * 1.25,
            scale: 1.0,
        },
        font_size,
        line_height,
    }
}

fn copyright_overlay_style(font_size: f32, line_height: f32) -> text::TextStyle {
    text::TextStyle {
        font_size,
        line_height,
        color: [255, 255, 255, 210],
        family: text::TextFamily::Name("Fira Mono"),
        shaping: text::Shaping::Advanced,
        align: Some(text::Align::Center),
    }
}

fn terminal_overlay_style(
    font_size: f32,
    line_height: f32,
    opacity: f32,
    is_link: bool,
) -> text::TextStyle {
    let alpha = (opacity.clamp(0.0, 1.0) * 255.0).round() as u8;
    text::TextStyle {
        font_size,
        line_height,
        color: if is_link {
            [0, 255, 255, alpha]
        } else {
            [255, 255, 255, alpha]
        },
        family: text::TextFamily::Name("Fira Mono"),
        shaping: text::Shaping::Advanced,
        align: None,
    }
}

fn project_terminal_point(
    local: glam::Vec2,
    viewport: glam::Vec2,
    view_projection: glam::Mat4,
    model: glam::Mat4,
) -> Option<glam::Vec2> {
    let clip = view_projection * model * glam::Vec4::new(local.x, local.y, 0.0, 1.0);
    if !clip.is_finite() || clip.w.abs() <= f32::EPSILON {
        return None;
    }
    let ndc = clip.truncate() / clip.w;
    ndc.is_finite().then_some(glam::Vec2::new(
        (ndc.x + 1.0) * viewport.x * 0.5,
        (1.0 - ndc.y) * viewport.y * 0.5,
    ))
}

fn terminal_overlay_layout(
    viewport: glam::Vec2,
    view_projection: glam::Mat4,
    model: glam::Mat4,
    line_count: usize,
    max_columns: usize,
    cell_advance: f32,
) -> Option<TerminalOverlayLayout> {
    if viewport.x <= 0.0
        || viewport.y <= 0.0
        || !viewport.is_finite()
        || cell_advance <= f32::EPSILON
    {
        return None;
    }
    let local_top = line_count.max(1) as f32 * TERMINAL_LINE_HEIGHT * 0.5;
    let top_left = project_terminal_point(
        glam::Vec2::new(0.0, local_top),
        viewport,
        view_projection,
        model,
    )?;
    let unit_right = project_terminal_point(
        glam::Vec2::new(1.0, local_top),
        viewport,
        view_projection,
        model,
    )?;
    let unit_down = project_terminal_point(
        glam::Vec2::new(0.0, local_top - 1.0),
        viewport,
        view_projection,
        model,
    )?;
    let pixels_per_local =
        ((unit_right.x - top_left.x).abs() + (unit_down.y - top_left.y).abs()) * 0.5;
    if !pixels_per_local.is_finite() || pixels_per_local <= f32::EPSILON {
        return None;
    }

    let font_size = TERMINAL_FONT_SIZE * pixels_per_local;
    let line_height = TERMINAL_LINE_HEIGHT * pixels_per_local;
    let cell_advance = cell_advance * pixels_per_local;
    Some(TerminalOverlayLayout {
        placement: text::TextPlacement {
            left: top_left.x,
            top: top_left.y,
            width: (max_columns + 2) as f32 * cell_advance,
            height: (line_count.max(1) as f32 + 1.0) * line_height,
            scale: 1.0,
        },
        font_size,
        line_height,
        cell_advance,
    })
}

fn responsive_mobile_line_limit(viewport: glam::Vec2, scale_factor: f32) -> Option<usize> {
    if viewport.x <= 0.0 || viewport.y <= 0.0 || !viewport.is_finite() {
        return None;
    }
    let scale_factor = scale_factor.max(1.0);
    let logical_viewport = viewport / scale_factor;
    let aspect = viewport.x / viewport.y;
    if !uses_mobile_timeline(aspect) {
        return None;
    }

    let header_height = if logical_viewport.x <= 620.0 {
        100.0
    } else if logical_viewport.x <= 980.0 {
        104.0
    } else {
        88.0
    } * scale_factor;
    let safety_margin = 4.0 * scale_factor;
    let copyright_top = copyright_overlay_layout(viewport, scale_factor)
        .placement
        .top
        - safety_margin;
    let (view_projection, camera_distance) = responsive_camera(aspect);

    for line_count in
        (timeline::MOBILE_MIN_TERMINAL_LINES..=timeline::MOBILE_MAX_TERMINAL_LINES).rev()
    {
        let model = terminal_model(aspect, camera_distance, 1.0, line_count);
        let Some(layout) =
            terminal_overlay_layout(viewport, view_projection, model, line_count, 27, 1.0)
        else {
            continue;
        };
        let bottom = layout.placement.top + layout.placement.height;
        if layout.placement.top >= header_height + safety_margin && bottom <= copyright_top {
            return Some(line_count);
        }
    }

    Some(timeline::MOBILE_MIN_TERMINAL_LINES)
}

fn mobile_terminal_scale(aspect: f32) -> f32 {
    mix(
        MOBILE_TERMINAL_BASE_SCALE,
        MOBILE_TERMINAL_BOOSTED_SCALE,
        smoothstep(
            MOBILE_TERMINAL_BOOST_START_ASPECT,
            MOBILE_TERMINAL_BOOST_END_ASPECT,
            aspect,
        ),
    )
}

fn uses_mobile_timeline(aspect: f32) -> bool {
    aspect < MOBILE_TIMELINE_MAX_ASPECT
}

fn terminal_model(
    aspect: f32,
    camera_distance: f32,
    slide_progress: f32,
    line_count: usize,
) -> glam::Mat4 {
    let mobile_scale = mobile_terminal_scale(aspect);
    let base_scale: f32 = if aspect >= 1.15 {
        0.72
    } else if aspect >= MOBILE_TIMELINE_MAX_ASPECT {
        0.68
    } else {
        // Browser chrome shortens the visible mobile viewport and produces a
        // wider aspect than a full-height device frame. Boost that common
        // layout while the mobile-specific 27-column transcript keeps the
        // enlarged glyphs inside exceptionally tall, narrow screens.
        mobile_scale
    };
    let plane_distance = (camera_distance - 1.05).max(0.1);
    let visible_height = 2.0 * plane_distance * (22.0_f32.to_radians()).tan();
    let content_height = line_count.max(1) as f32 * TERMINAL_LINE_HEIGHT;
    let text_scale = if aspect < 0.8 {
        base_scale
    } else {
        base_scale.min(visible_height * 0.86 / content_height)
    };
    let target_x = if aspect >= 1.15 {
        -4.62
    } else if aspect >= MOBILE_TIMELINE_MAX_ASPECT {
        -2.70
    } else {
        -1.85 * (mobile_scale / MOBILE_TERMINAL_BASE_SCALE)
    };
    let target_y = if uses_mobile_timeline(aspect) {
        MOBILE_TERMINAL_VERTICAL_OFFSET
    } else {
        0.0
    };
    let slide_offset = mix(-camera_distance * 1.45, 0.0, ease_out_cubic(slide_progress));
    glam::Mat4::from_translation(glam::Vec3::new(target_x + slide_offset, target_y, 1.05))
        * glam::Mat4::from_scale(glam::Vec3::splat(text_scale))
}

fn raycast_terminal_plane(
    screen: glam::Vec2,
    viewport: glam::Vec2,
    view_projection: glam::Mat4,
    model: glam::Mat4,
) -> Option<glam::Vec2> {
    if viewport.x <= 0.0 || viewport.y <= 0.0 || !screen.is_finite() || !viewport.is_finite() {
        return None;
    }
    let ndc = glam::Vec2::new(
        screen.x / viewport.x * 2.0 - 1.0,
        1.0 - screen.y / viewport.y * 2.0,
    );
    let inverse = (view_projection * model).try_inverse()?;
    let near = inverse.project_point3(glam::Vec3::new(ndc.x, ndc.y, 0.0));
    let far = inverse.project_point3(glam::Vec3::new(ndc.x, ndc.y, 1.0));
    let direction = far - near;
    if !near.is_finite() || !direction.is_finite() || direction.z.abs() < 1.0e-5 {
        return None;
    }
    let distance = -near.z / direction.z;
    if !distance.is_finite() || distance < 0.0 {
        return None;
    }
    let local = near + direction * distance;
    local.is_finite().then_some(local.truncate())
}

fn pick_terminal_link(
    screen: glam::Vec2,
    viewport: glam::Vec2,
    view_projection: glam::Mat4,
    model: glam::Mat4,
    typed_characters: usize,
    opacity: f32,
    links: &[TerminalLinkHitbox],
) -> Option<usize> {
    if opacity < 0.08 {
        return None;
    }
    let local = raycast_terminal_plane(screen, viewport, view_projection, model)?;
    links
        .iter()
        .position(|link| typed_characters >= link.end_character && link.contains(local))
}

#[cfg(target_arch = "wasm32")]
fn update_browser_link_snapshot(
    context: &RenderContext,
    view_projection: glam::Mat4,
    model: glam::Mat4,
    typed_characters: usize,
    opacity: f32,
    links: Option<&Rc<Vec<TerminalLinkHitbox>>>,
) {
    LINK_PICK_SNAPSHOT.with(|snapshot| {
        *snapshot.borrow_mut() = Some(LinkPickSnapshot {
            viewport: glam::Vec2::new(
                context.surface_config.width as f32,
                context.surface_config.height as f32,
            ),
            view_projection,
            model,
            typed_characters,
            opacity,
            links: links.cloned().unwrap_or_default(),
        });
    });
}

#[cfg(not(target_arch = "wasm32"))]
fn update_browser_link_snapshot(
    _context: &RenderContext,
    _view_projection: glam::Mat4,
    _model: glam::Mat4,
    _typed_characters: usize,
    _opacity: f32,
    _links: Option<&Rc<Vec<TerminalLinkHitbox>>>,
) {
}

fn smoothstep(edge0: f32, edge1: f32, value: f32) -> f32 {
    if (edge1 - edge0).abs() <= f32::EPSILON {
        return (value >= edge1) as u8 as f32;
    }
    let t = ((value - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn ease_out_cubic(value: f32) -> f32 {
    1.0 - (1.0 - value.clamp(0.0, 1.0)).powi(3)
}

fn mix(from: f32, to: f32, amount: f32) -> f32 {
    from + (to - from) * amount.clamp(0.0, 1.0)
}

#[cfg(target_arch = "wasm32")]
fn mount_browser_canvas(context: &RenderContext) {
    use winit::platform::web::WindowExtWebSys;

    let Some(canvas) = context.window.canvas() else {
        return;
    };
    canvas.set_id("webgpu-canvas");
    let _ = canvas.set_attribute("tabindex", "0");
    let _ = canvas.set_attribute("aria-label", "Pooya Eimandar interactive WebGPU timeline");
    if let Some(document) = web_sys::window().and_then(|window| window.document())
        && let Some(mount) = document.get_element_by_id("scene-mount")
    {
        let _ = mount.append_child(&canvas);
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn mount_browser_canvas(_context: &RenderContext) {}

#[cfg(target_arch = "wasm32")]
fn watch_webgpu_validation_scope(scope: wgpu::ErrorScopeGuard) {
    let result = scope.pop();
    wasm_bindgen_futures::spawn_local(async move {
        if let Some(error) = result.await {
            dispatch_renderer_error(&error.to_string());
        }
    });
}

fn report_renderer_init_error(error: RenderError) -> RenderError {
    #[cfg(target_arch = "wasm32")]
    dispatch_renderer_error(&error.to_string());
    error
}

#[cfg(target_arch = "wasm32")]
fn dispatch_renderer_error(message: &str) {
    let detail = js_sys::Object::new();
    let _ = js_sys::Reflect::set(
        &detail,
        &wasm_bindgen::JsValue::from_str("message"),
        &wasm_bindgen::JsValue::from_str(message),
    );
    dispatch_custom_event("pooya:renderer-error", &detail.into());
}

#[cfg(target_arch = "wasm32")]
fn dispatch_scene_progress(stage: &str, progress: u8, message: &str) {
    let detail = js_sys::Object::new();
    for (key, value) in [
        ("stage", wasm_bindgen::JsValue::from_str(stage)),
        (
            "progress",
            wasm_bindgen::JsValue::from_f64(f64::from(progress)),
        ),
        ("message", wasm_bindgen::JsValue::from_str(message)),
    ] {
        let _ = js_sys::Reflect::set(&detail, &wasm_bindgen::JsValue::from_str(key), &value);
    }
    dispatch_custom_event("pooya:scene-progress", &detail.into());
}

#[cfg(not(target_arch = "wasm32"))]
fn dispatch_scene_progress(_stage: &str, _progress: u8, _message: &str) {}

#[cfg(target_arch = "wasm32")]
fn dispatch_renderer_ready(slide_count: usize) {
    let detail = js_sys::Object::new();
    let _ = js_sys::Reflect::set(
        &detail,
        &wasm_bindgen::JsValue::from_str("slideCount"),
        &wasm_bindgen::JsValue::from_f64(slide_count as f64),
    );
    dispatch_custom_event("pooya:renderer-ready", &detail.into());
}

#[cfg(not(target_arch = "wasm32"))]
fn dispatch_renderer_ready(_slide_count: usize) {}

#[cfg(target_arch = "wasm32")]
fn dispatch_timeline_event(
    index: usize,
    count: usize,
    eyebrow: &str,
    heading: &str,
    summary: &str,
) {
    let detail = js_sys::Object::new();
    for (key, value) in [
        ("index", wasm_bindgen::JsValue::from_f64(index as f64)),
        ("count", wasm_bindgen::JsValue::from_f64(count as f64)),
        ("eyebrow", wasm_bindgen::JsValue::from_str(eyebrow)),
        ("heading", wasm_bindgen::JsValue::from_str(heading)),
        ("description", wasm_bindgen::JsValue::from_str(summary)),
    ] {
        let _ = js_sys::Reflect::set(&detail, &wasm_bindgen::JsValue::from_str(key), &value);
    }
    dispatch_custom_event("pooya:timeline-change", &detail.into());
}

#[cfg(not(target_arch = "wasm32"))]
fn dispatch_timeline_event(
    _index: usize,
    _count: usize,
    _eyebrow: &str,
    _heading: &str,
    _summary: &str,
) {
}

#[cfg(target_arch = "wasm32")]
fn dispatch_custom_event(name: &str, detail: &wasm_bindgen::JsValue) {
    let init = web_sys::CustomEventInit::new();
    init.set_detail(detail);
    if let Ok(event) = web_sys::CustomEvent::new_with_event_init_dict(name, &init)
        && let Some(window) = web_sys::window()
    {
        let _ = window.dispatch_event(&event);
    }
}

#[cfg(target_arch = "wasm32")]
fn log_message(message: &str) {
    web_sys::console::info_1(&wasm_bindgen::JsValue::from_str(message));
}

#[cfg(not(target_arch = "wasm32"))]
fn log_message(message: &str) {
    eprintln!("{message}");
}

/// Starts the native renderer. The browser entry point is `wasm_start` below.
#[cfg(not(target_arch = "wasm32"))]
pub fn run_portfolio() -> RenderResult<()> {
    let pending = Rc::new(RefCell::new(Some(
        portrait::load_default_portrait().map_err(|error| error.to_string()),
    )));
    sib::render::run(Portfolio::new(pending))
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn set_reduced_motion(reduced: bool) {
    REDUCED_MOTION.store(reduced, Ordering::Relaxed);
}

/// Queues the primary terminal action for the next animation frame.
///
/// During typing this reveals the complete current session. Once the session
/// is complete, the same action advances exactly one session. Pointer bridges
/// can call this after `activate_timeline_link` returns `false`; duplicate DOM
/// and winit requests in the same event turn intentionally collapse to one.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn reveal_or_advance_timeline() -> bool {
    PRIMARY_ACTION_REQUEST.store(true, Ordering::Relaxed);
    true
}

/// Activates a visible terminal link at canvas backing-store coordinates.
///
/// The browser bridge must call this synchronously from its `pointerup`
/// handler so `window.open` retains browser user activation.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn activate_timeline_link(x_physical: f32, y_physical: f32) -> bool {
    let url = LINK_PICK_SNAPSHOT.with(|snapshot| {
        let snapshot = snapshot.borrow();
        let snapshot = snapshot.as_ref()?;
        let index = pick_terminal_link(
            glam::Vec2::new(x_physical, y_physical),
            snapshot.viewport,
            snapshot.view_projection,
            snapshot.model,
            snapshot.typed_characters,
            snapshot.opacity,
            snapshot.links.as_slice(),
        )?;
        Some(snapshot.links.get(index)?.url.clone())
    });
    let Some(url) = url.filter(|url| url.starts_with("https://")) else {
        return false;
    };
    if let Some(window) = web_sys::window() {
        let _ = window.open_with_url_and_target_and_features(&url, "_blank", "noopener,noreferrer");
    }
    true
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen(start)]
pub fn wasm_start() -> Result<(), wasm_bindgen::JsValue> {
    let pending: PendingPortrait = Rc::new(RefCell::new(None));
    let loader_slot = pending.clone();
    wasm_bindgen_futures::spawn_local(async move {
        let result = portrait::load_default_portrait()
            .await
            .map_err(|error| error.to_string());
        *loader_slot.borrow_mut() = Some(result);
    });
    sib::render::run(Portfolio::new(pending))
        .map_err(|error| wasm_bindgen::JsValue::from_str(&error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reduced_motion_api_state_is_atomic() {
        REDUCED_MOTION.store(true, Ordering::Relaxed);
        assert!(REDUCED_MOTION.load(Ordering::Relaxed));
        REDUCED_MOTION.store(false, Ordering::Relaxed);
    }

    #[test]
    fn primary_terminal_action_reveals_before_it_advances() {
        assert_eq!(
            terminal_primary_action(0, 840),
            TerminalPrimaryAction::Reveal
        );
        assert_eq!(
            terminal_primary_action(839, 840),
            TerminalPrimaryAction::Reveal
        );
        assert_eq!(
            terminal_primary_action(840, 840),
            TerminalPrimaryAction::Advance
        );
    }

    #[test]
    fn matrix_signal_uses_random_30_to_60_second_intervals() {
        let mut schedule = MatrixSignalSchedule::with_seed(0x1234_5678);
        let mut previous_start = 0.0;

        for _ in 0..32 {
            let scheduled_start = schedule.next_start;
            let interval = scheduled_start - previous_start;
            assert!(interval >= MATRIX_SIGNAL_MIN_INTERVAL - 0.001);
            assert!(interval <= MATRIX_SIGNAL_MAX_INTERVAL + 0.001);

            schedule.update(scheduled_start);
            let active = schedule.uniforms(scheduled_start, true);
            assert_eq!(active[0], scheduled_start);
            assert!((0.06..=0.94).contains(&active[2]));
            assert_eq!(active[3], 1.0);
            assert_eq!(schedule.uniforms(scheduled_start, false)[3], 0.0);
            assert_eq!(
                schedule.uniforms(scheduled_start + MATRIX_SIGNAL_DURATION + 0.01, true)[3],
                0.0
            );
            previous_start = scheduled_start;
        }
    }

    #[test]
    fn copyright_notice_uses_the_current_year_and_stays_bottom_centered() {
        let notice = copyright_notice();
        assert_eq!(
            notice,
            format!("© {} Pooya Eimandar. All rights reserved.", current_year())
        );

        for (viewport, scale_factor) in [
            (glam::Vec2::new(1440.0, 900.0), 1.0),
            (glam::Vec2::new(780.0, 1688.0), 2.0),
        ] {
            let layout = copyright_overlay_layout(viewport, scale_factor);
            let center = layout.placement.left + layout.placement.width * 0.5;
            assert!((center - viewport.x * 0.5).abs() < 0.01);
            assert!(layout.placement.top > viewport.y * 0.8);
            assert!(layout.placement.top + layout.placement.height <= viewport.y);
            let style = copyright_overlay_style(layout.font_size, layout.line_height);
            assert_eq!(style.color, [255, 255, 255, 210]);
            assert!(matches!(style.align, Some(text::Align::Center)));
        }

        assert!(include_str!("../index.html").contains("Pooya Eimandar. All rights reserved."));
    }

    #[test]
    fn regular_filled_overlay_keeps_the_exact_requested_scale_and_flat_palette() {
        assert_eq!(TERMINAL_FONT_SIZE / TERMINAL_BASE_FONT_SIZE, 1.5);
        assert_eq!(TERMINAL_LINE_HEIGHT / TERMINAL_BASE_LINE_HEIGHT, 1.5);

        let white = terminal_overlay_style(24.0, 33.0, 1.0, false);
        let cyan = terminal_overlay_style(24.0, 33.0, 1.0, true);
        assert_eq!(white.color, [255, 255, 255, 255]);
        assert_eq!(cyan.color, [0, 255, 255, 255]);

        let viewport = glam::Vec2::new(1440.0, 900.0);
        let aspect = viewport.x / viewport.y;
        let (view_projection, camera_distance) = responsive_camera(aspect);
        let model = terminal_model(aspect, camera_distance, 1.0, 20);
        let layout = terminal_overlay_layout(
            viewport,
            view_projection,
            model,
            20,
            42,
            TERMINAL_FONT_SIZE * 0.6,
        )
        .unwrap();
        let base_projected_size = layout.font_size / TERMINAL_TEXT_SCALE;
        assert!((layout.font_size / base_projected_size - 1.5).abs() <= f32::EPSILON);
        assert!(layout.placement.width > 0.0 && layout.placement.height > 0.0);
    }

    #[test]
    fn terminal_shader_uses_flat_palette_and_matrix_passes_use_their_requested_glyph_sets() {
        let terminal = include_str!("../assets/shaders/timeline_text.wgsl");
        assert!(terminal.contains("return vec4<f32>(vec3<f32>(1.0), opacity)"));
        assert!(terminal.contains("return vec4<f32>(input.color, alpha)"));
        assert!(!terminal.contains("input.normal"));

        let matrix = include_str!("../assets/shaders/matrix.wgsl");
        let portrait = include_str!("../assets/shaders/portrait.wgsl");

        assert!(matrix.contains("PERSIAN_LETTER_COUNT: u32 = 32u"));
        assert!(matrix.contains("PERSIAN_ATLAS_COLUMNS: u32 = 8u"));
        assert!(matrix.contains("PERSIAN_ATLAS_ROWS: u32 = 4u"));
        assert!(matrix.contains("PERSIAN_GLYPH_RENDER_SCALE: f32 = 2.0"));
        assert!(matrix.contains("persian_atlas_mask"));
        assert!(matrix.contains("textureSampleLevel("));
        assert!(matrix.contains("matrix_signal_glyph"));
        for mapping in [
            "case 0u: { return 5u; }",
            "case 1u: { return 0u; }",
            "case 2u: { return 29u; }",
            "case 3u: { return 31u; }",
            "case 4u: { return 9u; }",
            "case 5u: { return 15u; }",
            "case 6u: { return 0u; }",
            "default: { return 30u; }",
        ] {
            assert!(matrix.contains(mapping));
        }
        assert!(!matrix.contains("MATRIX_SIGNAL_COLOR"));
        assert!(!matrix.contains("vec3<f32>(1.0, 0.78, 0.06)"));
        assert!(!matrix.contains("PERSIAN_GLYPH_ROWS"));
        assert!(!matrix.contains("persian_bitmap_mask"));
        assert!(!matrix.contains("&&"));
        assert!(!matrix.contains("||"));
        assert!(!matrix.contains("PERSIAN_DIGIT_COUNT"));
        assert!(!matrix.contains("persian_word_glyph"));

        assert!(portrait.contains("BINARY_GLYPH_COUNT: u32 = 2u"));
        assert!(portrait.contains("BINARY_GRID_SIZE: vec2<f32> = vec2<f32>(96.0, 132.0)"));
        assert!(portrait.contains("binary_row_bits"));
        assert!(portrait.contains("binary_bitmap_mask"));
        assert!(portrait.contains("select(0u, 1u, binary_sample >= 0.5)"));
        assert!(!portrait.contains("BINARY_GLYPH_ROWS"));
        assert!(!portrait.contains("blink_phase"));
        assert!(!portrait.contains("eyelids"));
        assert!(!portrait.contains("&&"));
        assert!(!portrait.contains("||"));
        assert!(!portrait.contains("PERSIAN_LETTER_COUNT"));
        assert!(!portrait.contains("PERSIAN_GLYPH_ROWS"));
        assert!(!portrait.contains("persian_bitmap_mask"));
        assert!(!portrait.contains("ا ب پ ت ث ج چ ح خ د ذ ر ز ژ"));
    }

    #[test]
    fn vazirmatn_matrix_atlas_has_32_populated_glyph_cells() {
        let font = include_bytes!("../assets/fonts/Vazirmatn-Regular.ttf");
        assert!(font.len() > 100_000);
        assert_eq!(&font[..4], &[0, 1, 0, 0]);

        let image =
            portrait::parse_ktx1_rgba8_asset(MATRIX_GLYPH_ATLAS_BYTES, "vazirmatn-persian.ktx")
                .unwrap();
        assert_eq!(
            (image.width, image.height),
            (MATRIX_GLYPH_ATLAS_WIDTH, MATRIX_GLYPH_ATLAS_HEIGHT)
        );

        const CELL_SIZE: usize = 32;
        let atlas_width = image.width as usize;
        let atlas_rgba = image.rgba.as_slice();
        for glyph in 0..32_usize {
            let cell_x = (glyph % 8) * CELL_SIZE;
            let cell_y = (glyph / 8) * CELL_SIZE;
            let covered = (cell_y..cell_y + CELL_SIZE)
                .flat_map(|y| {
                    (cell_x..cell_x + CELL_SIZE)
                        .map(move |x| atlas_rgba[(y * atlas_width + x) * 4 + 3])
                })
                .filter(|alpha| *alpha >= 64)
                .count();
            assert!(
                covered >= 18,
                "glyph atlas cell {glyph} is unexpectedly empty"
            );
        }
    }

    #[test]
    fn wgsl_modules_parse_and_validate() {
        for (name, source) in [
            ("matrix", include_str!("../assets/shaders/matrix.wgsl")),
            ("portrait", include_str!("../assets/shaders/portrait.wgsl")),
            (
                "timeline_text",
                include_str!("../assets/shaders/timeline_text.wgsl"),
            ),
        ] {
            let module = naga::front::wgsl::parse_str(source)
                .unwrap_or_else(|error| panic!("{name}.wgsl failed to parse: {error}"));
            naga::valid::Validator::new(
                naga::valid::ValidationFlags::all(),
                naga::valid::Capabilities::all(),
            )
            .validate(&module)
            .unwrap_or_else(|error| panic!("{name}.wgsl failed validation: {error}"));
        }
    }

    #[test]
    fn terminal_meshes_tag_every_cursor_and_link_range() {
        let slides = timeline::load_slides().unwrap();
        for slide in slides {
            let mesh = build_timeline_mesh(&slide).unwrap();
            assert!(!mesh.vertices.is_empty());
            assert_eq!(mesh.indices.len() % 3, 0);
            assert_eq!(mesh.links.len(), slide.links.len());
            assert!(mesh.vertices.iter().all(|vertex| {
                vertex.color == [1.0; 4] || vertex.color == [0.0, 1.0, 1.0, 1.0]
            }));
            assert!(mesh.vertices.iter().all(|vertex| {
                let character_order = vertex.meta[0] as usize;
                let is_link = vertex.meta[1] < 0.5
                    && slide.links.iter().any(|link| {
                        character_order >= link.start_character
                            && character_order < link.end_character
                    });
                vertex.color
                    == if is_link {
                        [0.0, 1.0, 1.0, 1.0]
                    } else {
                        [1.0; 4]
                    }
            }));
            assert_eq!(
                mesh.vertices
                    .iter()
                    .filter(|vertex| vertex.meta[1] > 0.5)
                    .count(),
                (slide.character_count() + 1) * 4
            );
            assert!(mesh.vertices.iter().all(|vertex| {
                vertex.meta[1] > 0.5
                    || vertex.meta[0] >= 0.0 && vertex.meta[0] < slide.character_count() as f32
            }));
            for link in mesh.links {
                assert!(link.min.x < link.max.x && link.min.y < link.max.y);
                assert!(link.contains((link.min + link.max) * 0.5));
            }
        }
    }

    #[test]
    fn terminal_raycast_and_link_reveal_gate_match_projected_plane() {
        let aspect = 16.0 / 9.0;
        let viewport = glam::Vec2::new(1280.0, 720.0);
        let (view_projection, camera_distance) = responsive_camera(aspect);
        let model = terminal_model(aspect, camera_distance, 1.0, 24);
        let expected_local = glam::Vec2::new(0.35, -0.20);
        let clip =
            view_projection * model * glam::Vec4::new(expected_local.x, expected_local.y, 0.0, 1.0);
        let ndc = clip.truncate() / clip.w;
        let screen = glam::Vec2::new(
            (ndc.x + 1.0) * 0.5 * viewport.x,
            (1.0 - ndc.y) * 0.5 * viewport.y,
        );
        let actual_local =
            raycast_terminal_plane(screen, viewport, view_projection, model).unwrap();
        assert!((actual_local - expected_local).length() < 1.0e-3);

        let links = [TerminalLinkHitbox {
            min: expected_local - glam::Vec2::splat(0.12),
            max: expected_local + glam::Vec2::splat(0.12),
            end_character: 9,
            url: "https://example.com".to_owned(),
        }];
        assert_eq!(
            pick_terminal_link(screen, viewport, view_projection, model, 9, 1.0, &links,),
            Some(0)
        );
        assert_eq!(
            pick_terminal_link(screen, viewport, view_projection, model, 8, 1.0, &links,),
            None
        );
        assert_eq!(
            pick_terminal_link(screen, viewport, view_projection, model, 9, 0.0, &links,),
            None
        );
    }

    #[test]
    fn responsive_terminal_models_keep_every_wrapped_line_in_view() {
        let full_height_mobile_scale = mobile_terminal_scale(390.0 / 844.0);
        let browser_view_mobile_scale = mobile_terminal_scale(390.0 / 700.0);
        assert!((full_height_mobile_scale - MOBILE_TERMINAL_BASE_SCALE).abs() < f32::EPSILON);
        assert!(browser_view_mobile_scale >= full_height_mobile_scale * 1.04);
        assert!(browser_view_mobile_scale <= MOBILE_TERMINAL_BOOSTED_SCALE);
        assert_eq!(
            responsive_mobile_line_limit(glam::Vec2::new(390.0, 844.0), 1.0),
            Some(timeline::MOBILE_MAX_TERMINAL_LINES)
        );
        assert_eq!(
            responsive_mobile_line_limit(glam::Vec2::new(780.0, 1688.0), 2.0),
            responsive_mobile_line_limit(glam::Vec2::new(390.0, 844.0), 1.0)
        );
        assert!(
            responsive_mobile_line_limit(glam::Vec2::new(320.0, 493.0), 1.0)
                .is_some_and(|lines| lines < timeline::MOBILE_MAX_TERMINAL_LINES)
        );
        assert!(responsive_mobile_line_limit(glam::Vec2::new(768.0, 1024.0), 1.0).is_some());
        assert_eq!(
            responsive_mobile_line_limit(glam::Vec2::new(801.0, 1000.0), 1.0),
            None
        );

        let options = text_mesh::TextMeshOptions {
            font_size: TERMINAL_FONT_SIZE,
            line_height: TERMINAL_LINE_HEIGHT,
            depth: TERMINAL_DEPTH,
            stroke_width: TERMINAL_STROKE_WIDTH,
            curve_steps: 3,
            family: text_mesh::TextMeshFamily::Name("Fira Mono"),
            center: false,
            ..Default::default()
        };
        let pipe =
            text_mesh::TextMesh::from_font_bytes(FONT_BYTES, "|", [1.0; 4], options).unwrap();
        let double_pipe =
            text_mesh::TextMesh::from_font_bytes(FONT_BYTES, "||", [1.0; 4], options).unwrap();
        let cell_advance = double_pipe.bounds.width() - pipe.bounds.width();

        for viewport in [
            glam::Vec2::new(320.0, 493.0),
            glam::Vec2::new(360.0, 800.0),
            glam::Vec2::new(390.0, 844.0),
            glam::Vec2::new(390.0, 700.0),
            glam::Vec2::new(768.0, 1024.0),
            glam::Vec2::new(1440.0, 900.0),
        ] {
            let aspect = viewport.x / viewport.y;
            let mobile_line_limit = responsive_mobile_line_limit(viewport, 1.0);
            let slides = if let Some(max_lines) = mobile_line_limit {
                timeline::load_mobile_slides(max_lines).unwrap()
            } else {
                timeline::load_slides().unwrap()
            };
            for slide in slides {
                if let Some(max_lines) = mobile_line_limit {
                    assert!(slide.line_count() <= max_lines);
                }
                let (view_projection, camera_distance) = responsive_camera(aspect);
                let model = terminal_model(aspect, camera_distance, 1.0, slide.line_count());
                let max_columns = slide
                    .terminal
                    .lines()
                    .map(|line| line.chars().count())
                    .max()
                    .unwrap_or(1) as f32;
                let min_local = glam::Vec2::new(
                    -cell_advance * 0.1,
                    -(slide.line_count() as f32) * TERMINAL_LINE_HEIGHT * 0.5 - TERMINAL_FONT_SIZE,
                );
                let max_local = glam::Vec2::new(
                    (max_columns + 1.0) * cell_advance,
                    slide.line_count() as f32 * TERMINAL_LINE_HEIGHT * 0.5 + TERMINAL_FONT_SIZE,
                );
                let mut max_abs_ndc = glam::Vec2::ZERO;
                let mut min_ndc = glam::Vec2::splat(f32::INFINITY);
                let mut max_ndc = glam::Vec2::splat(f32::NEG_INFINITY);
                for local in [
                    glam::Vec2::new(min_local.x, min_local.y),
                    glam::Vec2::new(min_local.x, max_local.y),
                    glam::Vec2::new(max_local.x, min_local.y),
                    glam::Vec2::new(max_local.x, max_local.y),
                ] {
                    let clip =
                        view_projection * model * glam::Vec4::new(local.x, local.y, 0.0, 1.0);
                    let ndc = (clip.truncate() / clip.w).truncate();
                    min_ndc = min_ndc.min(ndc);
                    max_ndc = max_ndc.max(ndc);
                    max_abs_ndc = max_abs_ndc.max(ndc.abs());
                }
                assert!(
                    max_abs_ndc.x <= 0.99 && max_abs_ndc.y <= 0.90,
                    "{} lines exceed the {viewport:?} viewport: NDC {min_ndc:?}..{max_ndc:?}",
                    slide.line_count()
                );

                if mobile_line_limit.is_some() {
                    let legacy_scale = mix(
                        0.68,
                        0.72,
                        smoothstep(
                            MOBILE_TERMINAL_BOOST_START_ASPECT,
                            MOBILE_TERMINAL_BOOST_END_ASPECT,
                            aspect,
                        ),
                    );
                    let rendered_scale = model.x_axis.truncate().length();
                    assert!(
                        (rendered_scale / legacy_scale - MOBILE_TERMINAL_SIZE_MULTIPLIER).abs()
                            <= 0.001,
                        "mobile terminal scale is not exactly 1.5x at {viewport:?}"
                    );

                    let terminal_top = (1.0 - max_ndc.y) * viewport.y * 0.5;
                    let terminal_bottom = (1.0 - min_ndc.y) * viewport.y * 0.5;
                    let copyright_top = copyright_overlay_layout(viewport, 1.0).placement.top;
                    assert!(
                        terminal_top >= 100.0,
                        "mobile terminal begins behind the fixed header at {viewport:?}: {terminal_top:.1}px"
                    );
                    assert!(
                        terminal_bottom <= copyright_top,
                        "mobile terminal overlaps copyright at {viewport:?}: {terminal_bottom:.1}px > {copyright_top:.1}px"
                    );
                }
            }
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn matrix_shader_matches_scene_depth_and_renders_green_pixels_offscreen() {
        use std::{sync::mpsc, time::Duration};

        const WIDTH: u32 = 128;
        const HEIGHT: u32 = 96;
        const BYTES_PER_PIXEL: u32 = 4;
        const BYTES_PER_ROW: u32 = WIDTH * BYTES_PER_PIXEL;

        let instance = wgpu::Instance::default();
        let adapter =
            match pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                compatible_surface: None,
                force_fallback_adapter: false,
            })) {
                Ok(adapter) => adapter,
                Err(error) => {
                    eprintln!("matrix GPU smoke skipped: no native adapter ({error})");
                    return;
                }
            };
        let adapter_info = adapter.get_info();
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("matrix GPU smoke device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::downlevel_defaults(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            memory_hints: wgpu::MemoryHints::MemoryUsage,
            trace: wgpu::Trace::Off,
        }))
        .expect("native adapter should create a baseline WebGPU device");

        let format = wgpu::TextureFormat::Rgba8Unorm;
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("matrix GPU smoke target"),
            size: wgpu::Extent3d {
                width: WIDTH,
                height: HEIGHT,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let target_view = target.create_view(&wgpu::TextureViewDescriptor::default());
        let depth_target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("matrix GPU smoke depth target"),
            size: wgpu::Extent3d {
                width: WIDTH,
                height: HEIGHT,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: texture::DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let depth_view = depth_target.create_view(&wgpu::TextureViewDescriptor::default());
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("matrix GPU smoke readback"),
            size: u64::from(BYTES_PER_ROW) * u64::from(HEIGHT),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let uniforms = MatrixUniforms {
            timing: [2.45, 1.0, 1.0, 19.86],
            viewport: [
                WIDTH as f32,
                HEIGHT as f32,
                WIDTH as f32 / HEIGHT as f32,
                34.0,
            ],
            signal: [0.0, MATRIX_SIGNAL_DURATION, 0.5, 0.0],
        };
        let uniform_layout = bind_group::uniform_texture_sampler_layout(
            &device,
            Some("matrix GPU smoke uniform texture layout"),
            wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
            wgpu::ShaderStages::FRAGMENT,
            wgpu::TextureViewDimension::D2,
        );
        let uniform_buffer =
            buffer::uniform_buffer(&device, Some("matrix GPU smoke uniforms"), &uniforms);
        let glyph_atlas =
            GpuMatrixGlyphAtlas::new(&device, &queue, &uniform_layout, &uniform_buffer).unwrap();
        let module = shader::wgsl_module(
            &device,
            Some("matrix GPU smoke shader"),
            include_str!("../assets/shaders/matrix.wgsl"),
        );
        let pipeline = create_matrix_pipeline_for_format(&device, format, &uniform_layout, &module);

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("matrix GPU smoke encoder"),
        });
        {
            let mut pass = render_pass::begin_color_depth(
                &mut encoder,
                Some("matrix GPU smoke pass"),
                &target_view,
                Some(&depth_view),
                wgpu::Color::BLACK,
                1.0,
            );
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &glyph_atlas.bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &target,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(BYTES_PER_ROW),
                    rows_per_image: Some(HEIGHT),
                },
            },
            wgpu::Extent3d {
                width: WIDTH,
                height: HEIGHT,
                depth_or_array_layers: 1,
            },
        );

        let submission = queue.submit([encoder.finish()]);
        let slice = readback.slice(..);
        let (sender, receiver) = mpsc::sync_channel(1);
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
        device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(submission),
                timeout: Some(Duration::from_secs(5)),
            })
            .expect("matrix GPU smoke submission should complete within five seconds");
        receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("matrix GPU smoke map callback should run")
            .expect("matrix GPU smoke readback should map");

        let bytes = slice.get_mapped_range();
        let mut non_black = 0_usize;
        let mut green_dominant = 0_usize;
        let mut bright_green = 0_usize;
        let mut max_green = 0_u8;
        let mut luminance_sum = 0.0_f64;
        for pixel in bytes.chunks_exact(BYTES_PER_PIXEL as usize) {
            let [red, green, blue, _alpha] = [pixel[0], pixel[1], pixel[2], pixel[3]];
            non_black += usize::from(red > 2 || green > 2 || blue > 2);
            green_dominant += usize::from(green > red && green > blue);
            bright_green += usize::from(green >= 16 && green > red && green > blue);
            max_green = max_green.max(green);
            luminance_sum +=
                0.2126 * f64::from(red) + 0.7152 * f64::from(green) + 0.0722 * f64::from(blue);
        }
        let pixel_count = usize::try_from(WIDTH * HEIGHT).unwrap();
        let mean_luminance = luminance_sum / pixel_count as f64;
        println!(
            "matrix GPU smoke [{} / {:?}]: pixels={pixel_count}, nonblack={non_black}, green={green_dominant}, bright_green={bright_green}, mean_luma={mean_luminance:.3}, max_green={max_green}",
            adapter_info.name, adapter_info.backend
        );
        assert!(
            non_black > pixel_count / 20,
            "matrix output is effectively black"
        );
        assert!(
            green_dominant > pixel_count / 10,
            "matrix output is not meaningfully green"
        );
        assert!(
            bright_green > pixel_count / 200,
            "matrix output contains too few lit glyph pixels"
        );
        assert!(mean_luminance > 1.0, "matrix output luminance is too low");
        assert!(max_green > 48, "matrix output has no bright stream pixels");
        drop(bytes);
        readback.unmap();
    }
}
