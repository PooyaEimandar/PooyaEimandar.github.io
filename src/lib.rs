mod portrait;
mod timeline;

use std::{
    cell::RefCell,
    rc::Rc,
    sync::atomic::{AtomicBool, Ordering},
};

use bytemuck::{Pod, Zeroable};
use portrait::PortraitImage;
use sib::render::{
    Example, ExampleSettings, FrameStats, RenderContext, RenderError, RenderResult, bind_group,
    buffer, glam, render_pass, shader, text, text_mesh, texture, wgpu, winit,
};
use timeline::TimelineSlide;

const FONT_BYTES: &[u8] = include_bytes!("../assets/fonts/FiraMono-Regular.ttf");
const MATRIX_INTRO_END: f32 = 1.65;
const FACE_REVEAL_END: f32 = 3.25;
const TIMELINE_START: f32 = 3.55;
const SLIDE_DURATION: f32 = 0.82;
const TERMINAL_TEXT_SCALE: f32 = 1.5;
const TERMINAL_BASE_FONT_SIZE: f32 = 0.142;
const TERMINAL_BASE_LINE_HEIGHT: f32 = 0.198;
const TERMINAL_FONT_SIZE: f32 = TERMINAL_BASE_FONT_SIZE * TERMINAL_TEXT_SCALE;
const TERMINAL_LINE_HEIGHT: f32 = TERMINAL_BASE_LINE_HEIGHT * TERMINAL_TEXT_SCALE;
const TERMINAL_DEPTH: f32 = 0.012 * TERMINAL_TEXT_SCALE;
// The atlas supplies the solid face of each glyph. Keep the extruded contour
// narrow so it adds depth without visually turning the regular font bold.
const TERMINAL_STROKE_WIDTH: f32 = TERMINAL_FONT_SIZE * 0.045;
const _: () = assert!(MATRIX_INTRO_END < FACE_REVEAL_END);
const _: () = assert!(FACE_REVEAL_END < TIMELINE_START);
const _: () = assert!(TERMINAL_TEXT_SCALE == 1.5);
const _: () = assert!(TERMINAL_STROKE_WIDTH <= TERMINAL_FONT_SIZE * 0.05);

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

#[cfg(target_arch = "wasm32")]
thread_local! {
    static LINK_PICK_SNAPSHOT: RefCell<Option<LinkPickSnapshot>> = const { RefCell::new(None) };
}

type PendingPortrait = Rc<RefCell<Option<Result<PortraitImage, String>>>>;

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct MatrixUniforms {
    timing: [f32; 4],
    viewport: [f32; 4],
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
        image: PortraitImage,
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
            PortraitImage {
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
    matrix_bind_group: Option<wgpu::BindGroup>,
    portrait_bind_group_layout: Option<wgpu::BindGroupLayout>,
    portrait_uniform_buffer: Option<wgpu::Buffer>,
    text_uniform_buffer: Option<wgpu::Buffer>,
    text_bind_group: Option<wgpu::BindGroup>,
    portrait: Option<GpuPortrait>,
    timeline_slides: Vec<TimelineSlide>,
    timeline_meshes: Vec<GpuTextMesh>,
    text_overlay: Option<text::TextOverlay>,
    depth: Option<texture::Texture>,
    frame_stats: FrameStats,
    elapsed: f32,
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
            matrix_bind_group: None,
            portrait_bind_group_layout: None,
            portrait_uniform_buffer: None,
            text_uniform_buffer: None,
            text_bind_group: None,
            portrait: None,
            timeline_slides: Vec::new(),
            timeline_meshes: Vec::new(),
            text_overlay: None,
            depth: None,
            frame_stats: FrameStats::new(),
            elapsed: 0.0,
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
            // for subtle shader blink/iris accents; the photograph remains
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
                .map(|mesh| &mesh.links),
        );
    }

    fn prepare_text_overlay(&mut self, context: &RenderContext) -> RenderResult<()> {
        let Some(slide) = self.timeline_slides.get(self.current_slide) else {
            return Ok(());
        };
        let Some(text_mesh) = self.timeline_meshes.get(self.current_slide) else {
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
        let text_mesh = self.timeline_meshes.get(self.current_slide)?;
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
        mount_browser_canvas(context);

        let matrix_layout = bind_group::uniform_layout(
            &context.device,
            Some("matrix uniform layout"),
            wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
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
        let matrix_bind_group = bind_group::uniform_bind_group(
            &context.device,
            Some("matrix uniform bind group"),
            &matrix_layout,
            &matrix_uniform_buffer,
        );
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
        )?;
        self.pipelines = Some(Pipelines {
            matrix: create_matrix_pipeline(context, &matrix_layout, &matrix_shader),
            portrait: create_portrait_pipeline(context, &portrait_layout, &portrait_shader),
            text: create_text_pipeline(context, &text_layout, &text_shader),
        });
        self.matrix_uniform_buffer = Some(matrix_uniform_buffer);
        self.matrix_bind_group = Some(matrix_bind_group);
        self.portrait_bind_group_layout = Some(portrait_layout);
        self.portrait_uniform_buffer = Some(portrait_uniform_buffer);
        self.portrait = Some(portrait);
        self.text_uniform_buffer = Some(text_uniform_buffer);
        self.text_bind_group = Some(text_bind_group);

        self.timeline_slides = timeline::load_slides().map_err(RenderError::message)?;
        self.timeline_meshes = self
            .timeline_slides
            .iter()
            .map(build_timeline_mesh)
            .collect::<RenderResult<Vec<_>>>()?
            .iter()
            .map(|mesh| GpuTextMesh::from_mesh(&context.device, mesh))
            .collect();
        self.text_overlay = Some(text::TextOverlay::with_font_data(
            context,
            [FONT_BYTES.to_vec()],
        )?);
        self.depth = Some(texture::Texture::depth(
            &context.device,
            &context.surface_config,
        ));
        self.update_uniforms(context);

        Ok(())
    }

    fn resize(&mut self, context: &mut RenderContext, _size: winit::dpi::PhysicalSize<u32>) {
        self.depth = Some(texture::Texture::depth(
            &context.device,
            &context.surface_config,
        ));
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
        let matrix_bind_group = self
            .matrix_bind_group
            .as_ref()
            .ok_or_else(|| RenderError::message("matrix bind group is not initialized"))?;
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
        pass.set_bind_group(0, matrix_bind_group, &[]);
        pass.draw(0..3, 0..1);

        if self.elapsed >= MATRIX_INTRO_END - 0.2 {
            pass.set_pipeline(&pipelines.portrait);
            pass.set_bind_group(0, &portrait.bind_group, &[]);
            pass.draw(0..3, 0..1);
        }

        if self.elapsed >= TIMELINE_START - 0.1
            && let Some(text_mesh) = self.timeline_meshes.get(self.current_slide)
        {
            pass.set_pipeline(&pipelines.text);
            pass.set_bind_group(0, text_bind_group, &[]);
            pass.set_vertex_buffer(0, text_mesh.vertex_buffer.slice(..));
            pass.set_index_buffer(text_mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..text_mesh.index_count, 0, 0..1);
        }

        drop(pass);
        if self.elapsed >= TIMELINE_START - 0.1
            && let Some(text_overlay) = self.text_overlay.as_ref()
        {
            let mut text_pass =
                render_pass::begin_color_load(encoder, Some("filled timeline glyph atlas"), view);
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

fn terminal_model(
    aspect: f32,
    camera_distance: f32,
    slide_progress: f32,
    line_count: usize,
) -> glam::Mat4 {
    let base_scale: f32 = if aspect >= 1.15 {
        0.72
    } else if aspect >= 0.8 {
        0.68
    } else {
        // Fira Mono Regular has a slightly wider advance than Medium. Preserve
        // the 1.5x glyph size contract while keeping the longest mobile line
        // inside the viewport.
        0.68
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
    } else if aspect >= 0.8 {
        -2.70
    } else {
        -1.85
    };
    let slide_offset = mix(-camera_distance * 1.45, 0.0, ease_out_cubic(slide_progress));
    glam::Mat4::from_translation(glam::Vec3::new(target_x + slide_offset, 0.0, 1.05))
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
        assert!(matrix.contains("array<array<u32, 7>, 32>"));
        assert!(matrix.contains("persian_bitmap_mask"));
        assert!(!matrix.contains("PERSIAN_DIGIT_COUNT"));
        assert!(!matrix.contains("persian_word_glyph"));
        for letter in
            "ا ب پ ت ث ج چ ح خ د ذ ر ز ژ س ش ص ض ط ظ ع غ ف ق ک گ ل م ن و ه ی".split_whitespace()
        {
            assert!(
                matrix.contains(letter),
                "rain atlas is missing Persian letter {letter}"
            );
        }

        assert!(portrait.contains("BINARY_GLYPH_COUNT: u32 = 2u"));
        assert!(portrait.contains("BINARY_GRID_SIZE: vec2<f32> = vec2<f32>(96.0, 132.0)"));
        assert!(portrait.contains("array<array<u32, 7>, 2>"));
        assert!(portrait.contains("binary_bitmap_mask"));
        assert!(portrait.contains("select(0u, 1u, binary_sample >= 0.5)"));
        assert!(!portrait.contains("PERSIAN_LETTER_COUNT"));
        assert!(!portrait.contains("PERSIAN_GLYPH_ROWS"));
        assert!(!portrait.contains("persian_bitmap_mask"));
        assert!(!portrait.contains("ا ب پ ت ث ج چ ح خ د ذ ر ز ژ"));
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

        for slide in timeline::load_slides().unwrap() {
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
            for viewport in [
                glam::Vec2::new(1440.0, 900.0),
                glam::Vec2::new(768.0, 1024.0),
                glam::Vec2::new(390.0, 844.0),
            ] {
                let aspect = viewport.x / viewport.y;
                let (view_projection, camera_distance) = responsive_camera(aspect);
                let model = terminal_model(aspect, camera_distance, 1.0, slide.line_count());
                let mut max_abs_ndc = glam::Vec2::ZERO;
                for local in [
                    glam::Vec2::new(min_local.x, min_local.y),
                    glam::Vec2::new(min_local.x, max_local.y),
                    glam::Vec2::new(max_local.x, min_local.y),
                    glam::Vec2::new(max_local.x, max_local.y),
                ] {
                    let clip =
                        view_projection * model * glam::Vec4::new(local.x, local.y, 0.0, 1.0);
                    let ndc = (clip.truncate() / clip.w).truncate().abs();
                    max_abs_ndc = max_abs_ndc.max(ndc);
                }
                assert!(
                    max_abs_ndc.x <= 0.99 && max_abs_ndc.y <= 0.90,
                    "{} lines exceed the {viewport:?} viewport: max NDC {max_abs_ndc:?}",
                    slide.line_count()
                );

                if viewport.x == 390.0 {
                    let mut projected_y = [0.0; 2];
                    for (index, local_y) in [-TERMINAL_FONT_SIZE * 0.5, TERMINAL_FONT_SIZE * 0.5]
                        .into_iter()
                        .enumerate()
                    {
                        let clip =
                            view_projection * model * glam::Vec4::new(0.0, local_y, 0.0, 1.0);
                        projected_y[index] = clip.y / clip.w;
                    }
                    let em_pixels = (projected_y[1] - projected_y[0]).abs() * viewport.y * 0.5;
                    assert!(
                        em_pixels >= 9.5,
                        "mobile terminal em is too small: {em_pixels:.2}px"
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
        };
        let uniform_layout = bind_group::uniform_layout(
            &device,
            Some("matrix GPU smoke uniform layout"),
            wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
        );
        let uniform_buffer =
            buffer::uniform_buffer(&device, Some("matrix GPU smoke uniforms"), &uniforms);
        let uniform_bind_group = bind_group::uniform_bind_group(
            &device,
            Some("matrix GPU smoke bind group"),
            &uniform_layout,
            &uniform_buffer,
        );
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
            pass.set_bind_group(0, &uniform_bind_group, &[]);
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
