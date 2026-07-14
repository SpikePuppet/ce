use std::mem;

use bytemuck::{Pod, Zeroable};
use glyphon::cosmic_text::{Align, Attrs, Buffer as TextBuffer, Family, Metrics, Shaping, Wrap};
use glyphon::{
    Cache, FontSystem, Resolution, SwashCache, TextArea, TextAtlas, TextBounds, TextRenderer,
    Viewport,
};
use wgpu::util::DeviceExt;
use wgpu::{
    BindGroup, BindGroupDescriptor, BindGroupEntry, BindGroupLayoutDescriptor,
    BindGroupLayoutEntry, BindingType, BlendState, Buffer, BufferBindingType, BufferDescriptor,
    BufferUsages, ColorTargetState, ColorWrites, Device, FragmentState, MultisampleState,
    PipelineCompilationOptions, PipelineLayoutDescriptor, PrimitiveState, PrimitiveTopology, Queue,
    RenderPass, RenderPipeline, RenderPipelineDescriptor, ShaderModuleDescriptor, ShaderSource,
    ShaderStages, TextureFormat, VertexBufferLayout, VertexState, VertexStepMode,
};
use winit::dpi::PhysicalSize;

use crate::agent::{AgentPanelLayout, AgentPanelMode, AgentPanelView};
use crate::clipboard::ClipboardProvider;
use crate::document::{DocumentError, DocumentInfo, Documents};
use crate::editor::{
    CursorRectangle, DiagnosticRectangle, EditorLayout, OverlayGeometry, ScrollbarRectangle,
    SelectionRectangle,
};
use crate::input::{ClipboardCommand, EditorCommand, EditorInput, HistoryCommand};
use crate::lsp::{CompletionItem, DiagnosticUpdate, LspDocument, Position};
use crate::modal::{ModalGeometry, ModalLayout, ModalRowTone, ModalView};
use crate::theme;

const INITIAL_RECTANGLE_CAPACITY: usize = 16;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TabAction {
    Reveal,
    Close,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(dead_code)]
pub enum SplashAction {
    OpenFile,
    OpenDirectory,
}

#[derive(Clone, Copy, Debug, PartialEq)]
#[allow(dead_code)]
pub struct SplashGeometry {
    pub open_file: Rectangle,
    pub open_directory: Rectangle,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rectangle {
    pub origin: [f32; 2],
    pub size: [f32; 2],
    pub color: [f32; 4],
}

impl Rectangle {
    pub const fn new(origin: [f32; 2], size: [f32; 2], color: [f32; 4]) -> Self {
        Self {
            origin,
            size,
            color,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct RectangleInstance {
    origin: [f32; 2],
    size: [f32; 2],
    color: [f32; 4],
}

impl RectangleInstance {
    fn from_logical(rectangle: Rectangle, scale_factor: f32) -> Self {
        Self {
            origin: rectangle.origin.map(|value| value * scale_factor),
            size: rectangle.size.map(|value| value * scale_factor),
            color: rectangle.color,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct ViewportUniform {
    size: [f32; 2],
    _padding: [f32; 2],
}

const SPLASH_MARK: &str = "       ####  ######\n      ##     ##\n      ##     #####\n      ##     ##\n       ####  ######";
const SPLASH_DIVIDER_WIDTH: f32 = 160.0;
const SPLASH_DIVIDER_HEIGHT: f32 = 1.0;

struct RectangleRenderer {
    pipeline: RenderPipeline,
    viewport_buffer: Buffer,
    viewport_bind_group: BindGroup,
    instance_buffer: Buffer,
    instance_capacity: usize,
    instance_count: u32,
    instances: Vec<RectangleInstance>,
}

impl RectangleRenderer {
    fn new(device: &Device, surface_format: TextureFormat) -> Self {
        let viewport_uniform = ViewportUniform {
            size: [1.0, 1.0],
            _padding: [0.0; 2],
        };
        let viewport_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("rectangle viewport uniform"),
            contents: bytemuck::bytes_of(&viewport_uniform),
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
        });
        let viewport_bind_group_layout =
            device.create_bind_group_layout(&BindGroupLayoutDescriptor {
                label: Some("rectangle viewport bind group layout"),
                entries: &[BindGroupLayoutEntry {
                    binding: 0,
                    visibility: ShaderStages::VERTEX,
                    ty: BindingType::Buffer {
                        ty: BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            });
        let viewport_bind_group = device.create_bind_group(&BindGroupDescriptor {
            label: Some("rectangle viewport bind group"),
            layout: &viewport_bind_group_layout,
            entries: &[BindGroupEntry {
                binding: 0,
                resource: viewport_buffer.as_entire_binding(),
            }],
        });

        let shader = device.create_shader_module(ShaderModuleDescriptor {
            label: Some("rectangle shader"),
            source: ShaderSource::Wgsl(include_str!("../shaders/rectangles.wgsl").into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some("rectangle pipeline layout"),
            bind_group_layouts: &[Some(&viewport_bind_group_layout)],
            immediate_size: 0,
        });
        let attributes = wgpu::vertex_attr_array![
            0 => Float32x2,
            1 => Float32x2,
            2 => Float32x4
        ];
        let pipeline = device.create_render_pipeline(&RenderPipelineDescriptor {
            label: Some("rectangle pipeline"),
            layout: Some(&pipeline_layout),
            vertex: VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[Some(VertexBufferLayout {
                    array_stride: mem::size_of::<RectangleInstance>() as wgpu::BufferAddress,
                    step_mode: VertexStepMode::Instance,
                    attributes: &attributes,
                })],
                compilation_options: PipelineCompilationOptions::default(),
            },
            fragment: Some(FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(ColorTargetState {
                    format: surface_format,
                    blend: Some(BlendState::ALPHA_BLENDING),
                    write_mask: ColorWrites::ALL,
                })],
                compilation_options: PipelineCompilationOptions::default(),
            }),
            primitive: PrimitiveState {
                topology: PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: MultisampleState::default(),
            cache: None,
            multiview_mask: None,
        });
        let instance_buffer = create_instance_buffer(device, INITIAL_RECTANGLE_CAPACITY);

        Self {
            pipeline,
            viewport_buffer,
            viewport_bind_group,
            instance_buffer,
            instance_capacity: INITIAL_RECTANGLE_CAPACITY,
            instance_count: 0,
            instances: Vec::with_capacity(INITIAL_RECTANGLE_CAPACITY),
        }
    }

    fn prepare(
        &mut self,
        device: &Device,
        queue: &Queue,
        physical_size: PhysicalSize<u32>,
        scale_factor: f32,
        rectangles: &[Rectangle],
    ) {
        let viewport = ViewportUniform {
            size: [physical_size.width as f32, physical_size.height as f32],
            _padding: [0.0; 2],
        };
        queue.write_buffer(&self.viewport_buffer, 0, bytemuck::bytes_of(&viewport));

        self.instances.clear();
        self.instances.extend(
            rectangles
                .iter()
                .copied()
                .map(|rectangle| RectangleInstance::from_logical(rectangle, scale_factor)),
        );

        if self.instances.len() > self.instance_capacity {
            self.instance_capacity = self.instances.len().next_power_of_two();
            self.instance_buffer = create_instance_buffer(device, self.instance_capacity);
        }

        if !self.instances.is_empty() {
            queue.write_buffer(
                &self.instance_buffer,
                0,
                bytemuck::cast_slice(&self.instances),
            );
        }
        self.instance_count = self.instances.len() as u32;
    }

    fn render<'pass>(&'pass self, render_pass: &mut RenderPass<'pass>) {
        if self.instance_count == 0 {
            return;
        }

        render_pass.set_pipeline(&self.pipeline);
        render_pass.set_bind_group(0, &self.viewport_bind_group, &[]);
        render_pass.set_vertex_buffer(0, self.instance_buffer.slice(..));
        render_pass.draw(0..6, 0..self.instance_count);
    }
}

struct SplashLayout {
    content_origin: [f32; 2],
    content_width: f32,
    mark_origin: [f32; 2],
    mark_size: [f32; 2],
    divider: Rectangle,
    tagline_origin: [f32; 2],
    tagline_size: [f32; 2],
    actions: SplashGeometry,
}

struct SplashTextState {
    font_system: FontSystem,
    swash_cache: SwashCache,
    mark_buffer: TextBuffer,
    tagline_buffer: TextBuffer,
    open_file_label: TextBuffer,
    open_file_shortcut: TextBuffer,
    open_directory_label: TextBuffer,
    open_directory_shortcut: TextBuffer,
    cached_content_width: Option<f32>,
    scene_rectangles: Vec<Rectangle>,
    hovered_action: Option<SplashAction>,
    prepared: bool,
}

impl SplashTextState {
    fn new() -> Self {
        let mut font_system = FontSystem::new();
        let swash_cache = SwashCache::new();
        let mark_metrics =
            Metrics::new(theme::SPLASH_MARK_FONT_SIZE, theme::SPLASH_MARK_LINE_HEIGHT);
        let body_metrics = Metrics::new(theme::FONT_SIZE, theme::LINE_HEIGHT);

        let mut mark_buffer = TextBuffer::new(&mut font_system, mark_metrics);
        mark_buffer.set_wrap(Wrap::None);
        let mut tagline_buffer = TextBuffer::new(&mut font_system, body_metrics);
        tagline_buffer.set_wrap(Wrap::None);
        let mut open_file_label = TextBuffer::new(&mut font_system, body_metrics);
        open_file_label.set_wrap(Wrap::None);
        let mut open_file_shortcut = TextBuffer::new(&mut font_system, body_metrics);
        open_file_shortcut.set_wrap(Wrap::None);
        let mut open_directory_label = TextBuffer::new(&mut font_system, body_metrics);
        open_directory_label.set_wrap(Wrap::None);
        let mut open_directory_shortcut = TextBuffer::new(&mut font_system, body_metrics);
        open_directory_shortcut.set_wrap(Wrap::None);

        mark_buffer.set_text(
            SPLASH_MARK,
            &splash_text_attributes(),
            Shaping::Advanced,
            None,
        );
        tagline_buffer.set_text(
            "It's just a code editor",
            &splash_text_attributes(),
            Shaping::Advanced,
            None,
        );
        open_file_label.set_text(
            "Open File",
            &splash_text_attributes(),
            Shaping::Advanced,
            None,
        );
        open_file_shortcut.set_text(
            "Cmd+O",
            &splash_text_attributes(),
            Shaping::Advanced,
            Some(Align::Right),
        );
        open_directory_label.set_text(
            "Open Directory",
            &splash_text_attributes(),
            Shaping::Advanced,
            None,
        );
        open_directory_shortcut.set_text(
            "Cmd+Shift+O",
            &splash_text_attributes(),
            Shaping::Advanced,
            Some(Align::Right),
        );

        Self {
            font_system,
            swash_cache,
            mark_buffer,
            tagline_buffer,
            open_file_label,
            open_file_shortcut,
            open_directory_label,
            open_directory_shortcut,
            cached_content_width: None,
            scene_rectangles: Vec::with_capacity(12),
            hovered_action: None,
            prepared: false,
        }
    }

    fn prepare(&mut self, logical_viewport: [f32; 2], hovered_action: Option<SplashAction>) {
        let layout = splash_layout(logical_viewport);
        if self.cached_content_width != Some(layout.content_width) {
            self.mark_buffer
                .set_size(Some(layout.content_width), Some(layout.mark_size[1]));
            self.tagline_buffer
                .set_size(Some(layout.content_width), Some(layout.tagline_size[1]));
            let action_text_width =
                (layout.content_width - 2.0 * theme::SPLASH_ACTION_HORIZONTAL_PADDING).max(1.0);
            let action_text_height = theme::SPLASH_ACTION_HEIGHT;
            for buffer in [
                &mut self.open_file_label,
                &mut self.open_file_shortcut,
                &mut self.open_directory_label,
                &mut self.open_directory_shortcut,
            ] {
                buffer.set_size(Some(action_text_width), Some(action_text_height));
            }
            self.cached_content_width = Some(layout.content_width);
        }

        for buffer in [
            &mut self.mark_buffer,
            &mut self.tagline_buffer,
            &mut self.open_file_label,
            &mut self.open_file_shortcut,
            &mut self.open_directory_label,
            &mut self.open_directory_shortcut,
        ] {
            buffer.shape_until_scroll(&mut self.font_system, false);
        }

        self.hovered_action = hovered_action;
        self.scene_rectangles = splash_rectangles(layout.actions, layout.divider, hovered_action);
        self.prepared = true;
    }

    fn hide(&mut self) {
        self.hovered_action = None;
        self.prepared = false;
        self.scene_rectangles.clear();
    }

    #[cfg(test)]
    fn has_shaped_glyphs(&self) -> bool {
        [
            &self.mark_buffer,
            &self.tagline_buffer,
            &self.open_file_label,
            &self.open_file_shortcut,
            &self.open_directory_label,
            &self.open_directory_shortcut,
        ]
        .into_iter()
        .all(buffer_has_shaped_glyphs)
    }
}

struct SplashRenderState {
    rectangles: RectangleRenderer,
    text_renderer: TextRenderer,
    text: SplashTextState,
    hovered_action: Option<SplashAction>,
    visible: bool,
}

struct SplashPrepareContext<'a> {
    device: &'a Device,
    queue: &'a Queue,
    physical_size: PhysicalSize<u32>,
    scale_factor: f32,
    viewport: &'a Viewport,
    atlas: &'a mut TextAtlas,
    logical_viewport: [f32; 2],
}

impl SplashRenderState {
    fn new(device: &Device, surface_format: TextureFormat, atlas: &mut TextAtlas) -> Self {
        let rectangles = RectangleRenderer::new(device, surface_format);
        let text_renderer = TextRenderer::new(atlas, device, MultisampleState::default(), None);

        Self {
            rectangles,
            text_renderer,
            text: SplashTextState::new(),
            hovered_action: None,
            visible: false,
        }
    }

    fn set_visible(&mut self, visible: bool) {
        if self.visible == visible {
            return;
        }
        self.visible = visible;
        self.hovered_action = None;
        self.text.hide();
    }

    fn action_at_position(
        &self,
        position: [f32; 2],
        logical_viewport: [f32; 2],
    ) -> Option<SplashAction> {
        self.visible
            .then(|| splash_action_at_position(position, splash_layout(logical_viewport).actions))
            .flatten()
    }

    fn update_hover(&mut self, position: [f32; 2], logical_viewport: [f32; 2]) -> bool {
        if !self.visible {
            return update_splash_hover_state(
                &mut self.hovered_action,
                [-1.0, -1.0],
                logical_viewport,
            );
        }
        update_splash_hover_state(&mut self.hovered_action, position, logical_viewport)
    }

    fn prepare(&mut self, context: SplashPrepareContext<'_>) -> Result<(), glyphon::PrepareError> {
        if !self.visible {
            self.text.hide();
            self.rectangles.prepare(
                context.device,
                context.queue,
                context.physical_size,
                context.scale_factor,
                &[],
            );
            return Ok(());
        }

        self.text
            .prepare(context.logical_viewport, self.hovered_action);
        self.rectangles.prepare(
            context.device,
            context.queue,
            context.physical_size,
            context.scale_factor,
            &self.text.scene_rectangles,
        );

        let layout = splash_layout(context.logical_viewport);
        let action_text_offset = [
            theme::SPLASH_ACTION_HORIZONTAL_PADDING,
            (theme::SPLASH_ACTION_HEIGHT - theme::LINE_HEIGHT) * 0.5,
        ];
        let action_text_size = [
            (layout.content_width - 2.0 * theme::SPLASH_ACTION_HORIZONTAL_PADDING).max(1.0),
            theme::SPLASH_ACTION_HEIGHT,
        ];
        let open_file_text = Rectangle::new(
            [
                layout.actions.open_file.origin[0] + action_text_offset[0],
                layout.actions.open_file.origin[1] + action_text_offset[1],
            ],
            action_text_size,
            [0.0; 4],
        );
        let open_directory_text = Rectangle::new(
            [
                layout.actions.open_directory.origin[0] + action_text_offset[0],
                layout.actions.open_directory.origin[1] + action_text_offset[1],
            ],
            action_text_size,
            [0.0; 4],
        );
        let mut areas = Vec::with_capacity(7);
        areas.push(TextArea {
            buffer: &self.text.mark_buffer,
            left: layout.mark_origin[0] * context.scale_factor,
            top: layout.mark_origin[1] * context.scale_factor,
            scale: context.scale_factor,
            bounds: physical_bounds(
                Rectangle::new(layout.content_origin, layout.mark_size, [0.0; 4]),
                context.scale_factor,
            ),
            default_color: theme::SPLASH_MARK_TEXT,
            custom_glyphs: &[],
        });
        areas.push(TextArea {
            buffer: &self.text.tagline_buffer,
            left: layout.tagline_origin[0] * context.scale_factor,
            top: layout.tagline_origin[1] * context.scale_factor,
            scale: context.scale_factor,
            bounds: physical_bounds(
                Rectangle::new(layout.tagline_origin, layout.tagline_size, [0.0; 4]),
                context.scale_factor,
            ),
            default_color: theme::EDITOR_TEXT,
            custom_glyphs: &[],
        });
        for (buffer, color, rectangle) in [
            (
                &self.text.open_file_label,
                theme::EDITOR_TEXT,
                open_file_text,
            ),
            (
                &self.text.open_file_shortcut,
                theme::SPLASH_SHORTCUT_TEXT,
                open_file_text,
            ),
            (
                &self.text.open_directory_label,
                theme::EDITOR_TEXT,
                open_directory_text,
            ),
            (
                &self.text.open_directory_shortcut,
                theme::SPLASH_SHORTCUT_TEXT,
                open_directory_text,
            ),
        ] {
            areas.push(TextArea {
                buffer,
                left: rectangle.origin[0] * context.scale_factor,
                top: rectangle.origin[1] * context.scale_factor,
                scale: context.scale_factor,
                bounds: physical_bounds(rectangle, context.scale_factor),
                default_color: color,
                custom_glyphs: &[],
            });
        }

        self.text_renderer.prepare(
            context.device,
            context.queue,
            &mut self.text.font_system,
            context.atlas,
            context.viewport,
            areas,
            &mut self.text.swash_cache,
        )?;
        Ok(())
    }

    fn render<'pass>(
        &'pass self,
        render_pass: &mut RenderPass<'pass>,
        atlas: &'pass TextAtlas,
        viewport: &'pass Viewport,
    ) -> Result<(), glyphon::RenderError> {
        if !self.visible || !self.text.prepared {
            return Ok(());
        }
        self.rectangles.render(render_pass);
        self.text_renderer.render(atlas, viewport, render_pass)
    }
}

struct ModalTextState {
    font_system: FontSystem,
    swash_cache: SwashCache,
    title_buffer: TextBuffer,
    subtitle_buffer: TextBuffer,
    header_action_buffer: TextBuffer,
    body_buffer: TextBuffer,
    status_buffer: TextBuffer,
    composer_label_buffer: TextBuffer,
    composer_field_buffer: TextBuffer,
    composer_button_buffer: TextBuffer,
    close_buffer: TextBuffer,
    cached_title_text: String,
    cached_body_text: String,
    cached_status_text: String,
    cached_body_size: Option<[f32; 2]>,
    scene_rectangles: Vec<Rectangle>,
    prepared: bool,
}

impl ModalTextState {
    fn new() -> Self {
        let mut font_system = FontSystem::new();
        let swash_cache = SwashCache::new();
        let metrics = Metrics::new(theme::FONT_SIZE, theme::LINE_HEIGHT);

        let title_buffer = modal_buffer(&mut font_system, metrics);
        let subtitle_buffer = modal_buffer(&mut font_system, metrics);
        let header_action_buffer = modal_buffer(&mut font_system, metrics);
        let body_buffer = modal_buffer(&mut font_system, metrics);
        let status_buffer = modal_buffer(&mut font_system, metrics);
        let composer_label_buffer = modal_buffer(&mut font_system, metrics);
        let composer_field_buffer = modal_buffer(&mut font_system, metrics);
        let composer_button_buffer = modal_buffer(&mut font_system, metrics);
        let close_buffer = modal_buffer(&mut font_system, metrics);

        Self {
            font_system,
            swash_cache,
            title_buffer,
            subtitle_buffer,
            header_action_buffer,
            body_buffer,
            status_buffer,
            composer_label_buffer,
            composer_field_buffer,
            composer_button_buffer,
            close_buffer,
            cached_title_text: String::new(),
            cached_body_text: String::new(),
            cached_status_text: String::new(),
            cached_body_size: None,
            scene_rectangles: Vec::with_capacity(16),
            prepared: false,
        }
    }

    fn prepare(&mut self, view: &ModalView, logical_viewport: [f32; 2]) -> ModalGeometry {
        let geometry = ModalGeometry::for_viewport_with_layout(
            logical_viewport,
            ModalLayout {
                composer: view.composer.is_some(),
                content_rows: view
                    .composer
                    .as_ref()
                    .map(|_| view.total_rows.clamp(7, 14)),
            },
        );
        let body_size = geometry.content_size;
        self.cached_title_text.clone_from(&view.title);
        self.cached_body_text = modal_rows_text(view);
        self.cached_status_text.clone_from(&view.status);
        self.cached_body_size = Some(body_size);

        prepare_modal_buffer(
            &mut self.font_system,
            &mut self.title_buffer,
            &view.title,
            [geometry.size[0], theme::LINE_HEIGHT],
        );
        prepare_modal_buffer(
            &mut self.font_system,
            &mut self.subtitle_buffer,
            &view.subtitle,
            [geometry.size[0], theme::LINE_HEIGHT],
        );
        prepare_modal_buffer(
            &mut self.font_system,
            &mut self.header_action_buffer,
            view.header_action
                .as_ref()
                .map_or("", |action| action.label.as_str()),
            geometry.header_action_size,
        );
        prepare_modal_buffer(
            &mut self.font_system,
            &mut self.body_buffer,
            &self.cached_body_text,
            body_size,
        );
        prepare_modal_buffer(
            &mut self.font_system,
            &mut self.status_buffer,
            &view.status,
            geometry.status_size,
        );

        let (composer_label, composer_field, composer_button) = view.composer.as_ref().map_or(
            ("", String::new(), ""),
            |composer| {
                let field_text = if composer.value.is_empty() {
                    if composer.focused {
                        format!("|  {}", composer.placeholder)
                    } else {
                        composer.placeholder.clone()
                    }
                } else {
                    let mut value = composer.value.clone();
                    if composer.focused {
                        value.insert(composer.cursor.min(value.len()), '|');
                    }
                    value
                };
                (
                    composer.label.as_str(),
                    field_text,
                    composer.button_label.as_str(),
                )
            },
        );
        prepare_modal_buffer(
            &mut self.font_system,
            &mut self.composer_label_buffer,
            composer_label,
            geometry.composer_label_size,
        );
        prepare_modal_buffer(
            &mut self.font_system,
            &mut self.composer_field_buffer,
            &composer_field,
            geometry.composer_field_size,
        );
        prepare_modal_buffer(
            &mut self.font_system,
            &mut self.composer_button_buffer,
            composer_button,
            geometry.composer_button_size,
        );
        prepare_modal_buffer(
            &mut self.font_system,
            &mut self.close_buffer,
            "×",
            geometry.close_size,
        );

        self.scene_rectangles =
            modal_rectangles(view, geometry, logical_viewport[0], logical_viewport[1]);
        self.prepared = true;
        geometry
    }

    fn hide(&mut self) {
        self.prepared = false;
        self.scene_rectangles.clear();
    }

    #[cfg(test)]
    fn has_shaped_body_glyphs(&self) -> bool {
        buffer_has_shaped_glyphs(&self.body_buffer)
    }

    #[cfg(test)]
    fn has_shaped_close_glyphs(&self) -> bool {
        buffer_has_shaped_glyphs(&self.close_buffer)
    }
}

fn modal_buffer(font_system: &mut FontSystem, metrics: Metrics) -> TextBuffer {
    let mut buffer = TextBuffer::new(font_system, metrics);
    buffer.set_wrap(Wrap::None);
    buffer
}

fn prepare_modal_buffer(
    font_system: &mut FontSystem,
    buffer: &mut TextBuffer,
    text: &str,
    size: [f32; 2],
) {
    buffer.set_size(Some(size[0].max(1.0)), Some(size[1].max(1.0)));
    buffer.set_text(text, &modal_text_attributes(), Shaping::Advanced, None);
    buffer.shape_until_scroll(font_system, false);
}

struct ModalRenderState {
    rectangles: RectangleRenderer,
    text_renderer: TextRenderer,
    text: ModalTextState,
}

struct ModalPrepareContext<'a> {
    device: &'a Device,
    queue: &'a Queue,
    physical_size: PhysicalSize<u32>,
    scale_factor: f32,
    viewport: &'a Viewport,
    atlas: &'a mut TextAtlas,
    logical_viewport: [f32; 2],
}

impl ModalRenderState {
    fn new(device: &Device, surface_format: TextureFormat, atlas: &mut TextAtlas) -> Self {
        let rectangles = RectangleRenderer::new(device, surface_format);
        let text_renderer = TextRenderer::new(atlas, device, MultisampleState::default(), None);

        Self {
            rectangles,
            text_renderer,
            text: ModalTextState::new(),
        }
    }

    fn prepare(
        &mut self,
        context: ModalPrepareContext<'_>,
        view: Option<&ModalView>,
    ) -> Result<(), glyphon::PrepareError> {
        let Some(view) = view else {
            self.text.hide();
            self.rectangles.prepare(
                context.device,
                context.queue,
                context.physical_size,
                context.scale_factor,
                &[],
            );
            return Ok(());
        };

        let geometry = self.text.prepare(view, context.logical_viewport);
        self.rectangles.prepare(
            context.device,
            context.queue,
            context.physical_size,
            context.scale_factor,
            &self.text.scene_rectangles,
        );

        let modal_bounds = physical_bounds(
            Rectangle::new(geometry.origin, geometry.size, [0.0; 4]),
            context.scale_factor,
        );
        let close_bounds = physical_bounds(
            Rectangle::new(geometry.close_origin, geometry.close_size, [0.0; 4]),
            context.scale_factor,
        );
        let body_area = TextArea {
            buffer: &self.text.body_buffer,
            left: geometry.content_origin[0] * context.scale_factor,
            top: (geometry.origin[1] + 8.0) * context.scale_factor,
            scale: context.scale_factor,
            bounds: modal_bounds,
            default_color: theme::MODAL_TEXT,
            custom_glyphs: &[],
        };
        let close_area = TextArea {
            buffer: &self.text.close_buffer,
            left: (geometry.close_origin[0] + 8.0) * context.scale_factor,
            top: (geometry.close_origin[1] + 2.0) * context.scale_factor,
            scale: context.scale_factor,
            bounds: close_bounds,
            default_color: theme::MODAL_TEXT,
            custom_glyphs: &[],
        };
        self.text_renderer.prepare(
            context.device,
            context.queue,
            &mut self.text.font_system,
            context.atlas,
            context.viewport,
            [body_area, close_area],
            &mut self.text.swash_cache,
        )?;
        Ok(())
    }

    fn render<'pass>(
        &'pass self,
        render_pass: &mut RenderPass<'pass>,
        atlas: &'pass TextAtlas,
        viewport: &'pass Viewport,
    ) -> Result<(), glyphon::RenderError> {
        if !self.text.prepared {
            return Ok(());
        }
        self.rectangles.render(render_pass);
        self.text_renderer.render(atlas, viewport, render_pass)
    }
}

struct AgentPanelTextState {
    font_system: FontSystem,
    swash_cache: SwashCache,
    buffer: TextBuffer,
    cached_text: String,
    cached_size: Option<[f32; 2]>,
    scene_rectangles: Vec<Rectangle>,
    prepared: bool,
}

impl AgentPanelTextState {
    fn new() -> Self {
        let mut font_system = FontSystem::new();
        let swash_cache = SwashCache::new();
        let metrics = Metrics::new(theme::FONT_SIZE, theme::LINE_HEIGHT);
        let mut buffer = TextBuffer::new(&mut font_system, metrics);
        buffer.set_wrap(Wrap::Word);

        Self {
            font_system,
            swash_cache,
            buffer,
            cached_text: String::new(),
            cached_size: None,
            scene_rectangles: Vec::with_capacity(32),
            prepared: false,
        }
    }

    fn prepare(&mut self, view: &AgentPanelView, layout: AgentPanelLayout) {
        let Some(drawer) = layout.drawer else {
            self.hide();
            return;
        };
        let size = [drawer.size[0].max(1.0), drawer.size[1].max(1.0)];
        if self.cached_size != Some(size) {
            self.buffer
                .set_size(Some((size[0] - 24.0).max(1.0)), Some(size[1]));
            self.cached_size = Some(size);
        }
        let text = agent_panel_text(view);
        if self.cached_text != text {
            self.buffer
                .set_text(&text, &modal_text_attributes(), Shaping::Advanced, None);
            self.cached_text = text;
        }
        self.buffer.shape_until_scroll(&mut self.font_system, false);
        self.scene_rectangles = agent_panel_rectangles(layout, view.focused);
        self.prepared = true;
    }

    fn hide(&mut self) {
        self.prepared = false;
        self.scene_rectangles.clear();
    }
}

struct AgentPanelRenderState {
    rectangles: RectangleRenderer,
    text_renderer: TextRenderer,
    text: AgentPanelTextState,
}

impl AgentPanelRenderState {
    fn new(device: &Device, surface_format: TextureFormat, atlas: &mut TextAtlas) -> Self {
        Self {
            rectangles: RectangleRenderer::new(device, surface_format),
            text_renderer: TextRenderer::new(atlas, device, MultisampleState::default(), None),
            text: AgentPanelTextState::new(),
        }
    }

    fn prepare(
        &mut self,
        context: ModalPrepareContext<'_>,
        view: Option<&AgentPanelView>,
    ) -> Result<(), glyphon::PrepareError> {
        let layout = view.map_or_else(
            || AgentPanelLayout::calculate(context.logical_viewport, false, 0.4),
            |view| {
                AgentPanelLayout::calculate(
                    context.logical_viewport,
                    view.visible,
                    view.width_ratio,
                )
            },
        );
        let Some(view) = view.filter(|view| view.visible) else {
            self.text.hide();
            self.rectangles.prepare(
                context.device,
                context.queue,
                context.physical_size,
                context.scale_factor,
                &[],
            );
            return Ok(());
        };

        self.text.prepare(view, layout);
        self.rectangles.prepare(
            context.device,
            context.queue,
            context.physical_size,
            context.scale_factor,
            &self.text.scene_rectangles,
        );

        let drawer = layout
            .drawer
            .expect("visible agent panel layout includes drawer");
        let text_area = TextArea {
            buffer: &self.text.buffer,
            left: (drawer.origin[0] + 12.0) * context.scale_factor,
            top: 12.0 * context.scale_factor,
            scale: context.scale_factor,
            bounds: physical_bounds(drawer, context.scale_factor),
            default_color: theme::MODAL_TEXT,
            custom_glyphs: &[],
        };
        self.text_renderer.prepare(
            context.device,
            context.queue,
            &mut self.text.font_system,
            context.atlas,
            context.viewport,
            [text_area],
            &mut self.text.swash_cache,
        )?;
        Ok(())
    }

    fn render<'pass>(
        &'pass self,
        render_pass: &mut RenderPass<'pass>,
        atlas: &'pass TextAtlas,
        viewport: &'pass Viewport,
    ) -> Result<(), glyphon::RenderError> {
        if !self.text.prepared {
            return Ok(());
        }
        self.rectangles.render(render_pass);
        self.text_renderer.render(atlas, viewport, render_pass)
    }
}

fn create_instance_buffer(device: &Device, capacity: usize) -> Buffer {
    device.create_buffer(&BufferDescriptor {
        label: Some("rectangle instance buffer"),
        size: (capacity * mem::size_of::<RectangleInstance>()) as wgpu::BufferAddress,
        usage: BufferUsages::VERTEX | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

pub struct Renderer {
    rectangles: RectangleRenderer,
    overlay_rectangles: RectangleRenderer,
    splash: SplashRenderState,
    modal: ModalRenderState,
    agent_panel: AgentPanelRenderState,
    text_viewport: Viewport,
    text_atlas: TextAtlas,
    text_renderer: TextRenderer,
    overlay_text_renderer: TextRenderer,
    documents: Documents,
    scene_rectangles: Vec<Rectangle>,
    overlay_scene_rectangles: Vec<Rectangle>,
    modal_view: Option<ModalView>,
    agent_panel_view: Option<AgentPanelView>,
    cursor_visible: bool,
    hovered_tab_action: Option<(usize, TabAction)>,
}

impl Renderer {
    pub fn new(device: &Device, queue: &Queue, surface_format: TextureFormat) -> Self {
        let rectangles = RectangleRenderer::new(device, surface_format);
        let overlay_rectangles = RectangleRenderer::new(device, surface_format);
        let cache = Cache::new(device);
        let text_viewport = Viewport::new(device, &cache);
        let mut text_atlas = TextAtlas::new(device, queue, &cache, surface_format);
        let text_renderer =
            TextRenderer::new(&mut text_atlas, device, MultisampleState::default(), None);
        let overlay_text_renderer =
            TextRenderer::new(&mut text_atlas, device, MultisampleState::default(), None);
        let splash = SplashRenderState::new(device, surface_format, &mut text_atlas);
        let modal = ModalRenderState::new(device, surface_format, &mut text_atlas);
        let agent_panel = AgentPanelRenderState::new(device, surface_format, &mut text_atlas);

        Self {
            rectangles,
            overlay_rectangles,
            splash,
            modal,
            agent_panel,
            text_viewport,
            text_atlas,
            text_renderer,
            overlay_text_renderer,
            documents: Documents::new(),
            scene_rectangles: Vec::with_capacity(INITIAL_RECTANGLE_CAPACITY),
            overlay_scene_rectangles: Vec::with_capacity(7),
            modal_view: None,
            agent_panel_view: None,
            cursor_visible: false,
            hovered_tab_action: None,
        }
    }

    pub fn apply_input(&mut self, input: EditorInput) -> bool {
        self.documents.apply_input(input)
    }

    pub fn apply_command(&mut self, command: EditorCommand) {
        self.documents.apply_command(command);
    }

    pub fn apply_history_command(&mut self, command: HistoryCommand) -> bool {
        self.documents.apply_history_command(command)
    }

    pub fn break_history_group(&mut self) {
        self.documents.break_history_group();
    }

    pub fn toggle_markdown_presentation(&mut self) -> bool {
        self.documents.toggle_active_presentation()
    }

    pub fn apply_clipboard_command<C: ClipboardProvider>(
        &mut self,
        command: ClipboardCommand,
        clipboard: &mut C,
    ) -> Result<bool, C::Error> {
        self.documents.apply_clipboard_command(command, clipboard)
    }

    pub fn document_info(&self) -> DocumentInfo {
        self.documents.active_info()
    }

    pub fn document_info_at(&self, index: usize) -> Option<DocumentInfo> {
        self.documents.info_at(index)
    }

    pub fn document_count(&self) -> usize {
        self.documents.len()
    }

    pub fn active_document_index(&self) -> usize {
        self.documents.active_index()
    }

    pub fn switch_document(&mut self, index: usize) -> bool {
        self.documents.switch_to(index)
    }

    pub fn close_active_document(&mut self) {
        self.documents.close_active();
    }

    pub fn tab_at_position(&self, position: [f32; 2], viewport_width: f32) -> Option<usize> {
        tab_at_position(
            position,
            self.editor_hit_width(viewport_width),
            self.documents.len(),
        )
    }

    pub fn tab_action_at_position(
        &self,
        position: [f32; 2],
        viewport_width: f32,
    ) -> Option<(usize, TabAction)> {
        let editor_width = self.editor_hit_width(viewport_width);
        tab_action_at_position(position, editor_width, self.documents.len()).and_then(
            |(index, action)| {
                (action != TabAction::Reveal || self.documents.info_at(index)?.path.is_some())
                    .then_some((index, action))
            },
        )
    }

    pub fn open_document(&mut self, path: std::path::PathBuf) -> Result<(), DocumentError> {
        self.documents.open_path(path)
    }

    pub fn save_document(&mut self, path: std::path::PathBuf) -> Result<(), DocumentError> {
        self.documents.save_active_as(path)
    }

    pub fn lsp_documents(&self) -> Vec<LspDocument> {
        self.documents.lsp_documents()
    }

    pub fn apply_diagnostics(&mut self, update: &DiagnosticUpdate) -> bool {
        self.documents.apply_diagnostics(update)
    }

    pub fn clear_diagnostics(&mut self) {
        self.documents.clear_diagnostics();
    }

    pub fn active_lsp_position(&self) -> Option<(std::path::PathBuf, Position)> {
        self.documents.active_lsp_position()
    }

    pub fn apply_completion(&mut self, item: &CompletionItem) -> bool {
        self.documents.apply_completion(item)
    }

    pub fn active_path_is(&self, path: &std::path::Path) -> bool {
        self.documents.active_path_is(path)
    }

    pub fn go_to_position(&mut self, position: Position) {
        self.documents.go_to_position(position);
    }

    pub fn show_completion(
        &mut self,
        rows: &[String],
        selected: usize,
        documentation: Option<&str>,
    ) {
        self.documents
            .show_completion(rows, selected, documentation);
    }

    pub fn show_hover(&mut self, contents: &str) {
        self.documents.show_hover(contents);
    }

    pub fn dismiss_overlay(&mut self) -> bool {
        self.documents.dismiss_overlay()
    }

    pub fn scroll_document(&mut self, delta: [f32; 2]) -> bool {
        self.documents.scroll_active(delta)
    }

    pub fn completion_item_at_position(&self, position: [f32; 2]) -> Option<usize> {
        self.documents.completion_item_at_position(position)
    }

    pub fn overlay_contains_position(&self, position: [f32; 2]) -> bool {
        self.documents.overlay_contains_position(position)
    }

    pub fn update_diagnostic_hover(&mut self, position: [f32; 2]) -> bool {
        self.documents.update_diagnostic_hover(position)
    }

    pub fn update_tab_path_hover(&mut self, position: [f32; 2], viewport_width: f32) -> bool {
        let editor_width = self.editor_hit_width(viewport_width);
        let hovered_action = self.tab_action_at_position(position, viewport_width);
        let action_changed = self.hovered_tab_action != hovered_action;
        self.hovered_tab_action = hovered_action;
        let path_changed = if hovered_action.is_some() {
            self.documents
                .update_tab_path_hover([-1.0, -1.0], editor_width)
        } else {
            self.documents.update_tab_path_hover(position, editor_width)
        };
        action_changed || path_changed
    }

    pub fn set_cursor_visible(&mut self, visible: bool) {
        self.cursor_visible = visible;
    }

    pub fn set_modal_view(&mut self, view: Option<ModalView>) {
        self.modal_view = view;
    }

    pub fn set_agent_panel_view(&mut self, view: Option<AgentPanelView>) {
        self.agent_panel_view = view;
    }

    pub fn set_splash_visible(&mut self, visible: bool) {
        self.splash.set_visible(visible);
    }

    pub fn splash_action_at_position(
        &self,
        position: [f32; 2],
        logical_viewport: [f32; 2],
    ) -> Option<SplashAction> {
        self.splash.action_at_position(position, logical_viewport)
    }

    pub fn update_splash_hover(&mut self, position: [f32; 2], logical_viewport: [f32; 2]) -> bool {
        self.splash.update_hover(position, logical_viewport)
    }

    pub fn prepare(
        &mut self,
        device: &Device,
        queue: &Queue,
        physical_size: PhysicalSize<u32>,
        scale_factor: f32,
    ) -> Result<(), glyphon::PrepareError> {
        let (logical_width, logical_height) = logical_extent(physical_size, scale_factor);
        let modal_view = self.modal_view.clone();
        let agent_panel_view = self.agent_panel_view.clone();
        let agent_panel_layout = agent_panel_view.as_ref().map_or_else(
            || AgentPanelLayout::calculate([logical_width, logical_height], false, 0.4),
            |view| {
                AgentPanelLayout::calculate(
                    [logical_width, logical_height],
                    view.visible,
                    view.width_ratio,
                )
            },
        );
        let editor_view_width = if agent_panel_layout.mode == AgentPanelMode::Narrow {
            0.0
        } else {
            agent_panel_layout.editor_width
        };

        self.text_viewport.update(
            queue,
            Resolution {
                width: physical_size.width,
                height: physical_size.height,
            },
        );

        if self.splash.visible {
            self.scene_rectangles.clear();
            self.overlay_scene_rectangles.clear();
            self.splash.prepare(SplashPrepareContext {
                device,
                queue,
                physical_size,
                scale_factor,
                viewport: &self.text_viewport,
                atlas: &mut self.text_atlas,
                logical_viewport: [logical_width, logical_height],
            })?;
            return self.modal.prepare(
                ModalPrepareContext {
                    device,
                    queue,
                    physical_size,
                    scale_factor,
                    viewport: &self.text_viewport,
                    atlas: &mut self.text_atlas,
                    logical_viewport: [logical_width, logical_height],
                },
                modal_view.as_ref(),
            );
        }

        self.splash.prepare(SplashPrepareContext {
            device,
            queue,
            physical_size,
            scale_factor,
            viewport: &self.text_viewport,
            atlas: &mut self.text_atlas,
            logical_viewport: [logical_width, logical_height],
        })?;

        let tab_count = self.documents.len();
        let active_tab = self.documents.active_index();
        let tab_width = tab_width(editor_view_width, tab_count);
        let hovered_tab_action = self.hovered_tab_action;
        let dirty_tabs = (0..tab_count)
            .map(|index| self.documents.info_at(index).is_some_and(|info| info.dirty))
            .collect::<Vec<_>>();
        let editor = self.documents.active_editor_mut();
        editor.resize(editor_view_width, logical_height);
        let layout = editor.layout();

        self.scene_rectangles.clear();
        for index in 0..tab_count {
            self.scene_rectangles.push(Rectangle::new(
                [index as f32 * tab_width, 0.0],
                [tab_width, theme::TAB_BAR_HEIGHT],
                if index == active_tab {
                    theme::TAB_ACTIVE_BACKGROUND
                } else {
                    theme::TAB_INACTIVE_BACKGROUND
                },
            ));
        }
        if let Some((index, action)) = hovered_tab_action {
            let (origin, size) = tab_action_geometry(index, action, tab_width);
            self.scene_rectangles.push(Rectangle::new(
                origin,
                size,
                theme::TAB_ACTION_HOVER_BACKGROUND,
            ));
        }
        self.scene_rectangles.push(Rectangle::new(
            [0.0, theme::TAB_BAR_HEIGHT - 1.0],
            [editor_view_width, 1.0],
            theme::TAB_DIVIDER,
        ));
        for (index, dirty) in dirty_tabs.into_iter().enumerate() {
            if dirty {
                push_tab_outline(
                    &mut self.scene_rectangles,
                    [index as f32 * tab_width, 0.0],
                    [tab_width, theme::TAB_BAR_HEIGHT],
                    1.0,
                    theme::TAB_DIRTY_OUTLINE,
                );
            }
            if index == active_tab {
                self.scene_rectangles.push(Rectangle::new(
                    [index as f32 * tab_width + 1.0, theme::TAB_BAR_HEIGHT - 3.0],
                    [(tab_width - 2.0).max(0.0), 3.0],
                    theme::TAB_ACTIVE_INDICATOR,
                ));
            }
        }
        if layout.gutter_width > 0.0 {
            self.scene_rectangles.push(Rectangle::new(
                [0.0, theme::TAB_BAR_HEIGHT],
                [
                    layout.gutter_width,
                    (logical_height - theme::TAB_BAR_HEIGHT).max(0.0),
                ],
                theme::GUTTER_BACKGROUND,
            ));
            self.scene_rectangles.push(Rectangle::new(
                [layout.gutter_width - 1.0, theme::TAB_BAR_HEIGHT],
                [1.0, (logical_height - theme::TAB_BAR_HEIGHT).max(0.0)],
                theme::GUTTER_DIVIDER,
            ));
        }
        self.scene_rectangles
            .extend(
                editor
                    .selection_rectangles()
                    .iter()
                    .filter_map(|rectangle| {
                        translate_selection_rectangle(
                            *rectangle,
                            layout,
                            editor_view_width,
                            logical_height,
                        )
                    }),
            );
        self.scene_rectangles
            .extend(
                editor
                    .diagnostic_rectangles()
                    .iter()
                    .filter_map(|rectangle| {
                        translate_diagnostic_rectangle(
                            *rectangle,
                            layout,
                            editor_view_width,
                            logical_height,
                        )
                    }),
            );
        let scrollbars = editor.scrollbars();
        let overlay_geometry = editor.overlay_geometry();
        self.overlay_scene_rectangles.clear();
        self.overlay_scene_rectangles.extend(
            [scrollbars.vertical, scrollbars.horizontal]
                .into_iter()
                .flatten()
                .filter_map(|scrollbar| {
                    translate_scrollbar_rectangle(
                        scrollbar,
                        layout,
                        editor_view_width,
                        logical_height,
                    )
                }),
        );
        if let Some(overlay) = overlay_geometry {
            self.overlay_scene_rectangles.extend(overlay_rectangles(
                overlay,
                layout,
                editor_view_width,
                logical_height,
            ));
        }
        let cursor_rectangle = self
            .cursor_visible
            .then(|| editor.cursor_rectangle())
            .flatten()
            .and_then(|rectangle| {
                translate_cursor_rectangle(rectangle, layout, editor_view_width, logical_height)
            });
        if let Some(rectangle) = cursor_rectangle {
            self.scene_rectangles.push(rectangle);
        }
        self.rectangles.prepare(
            device,
            queue,
            physical_size,
            scale_factor,
            &self.scene_rectangles,
        );
        self.overlay_rectangles.prepare(
            device,
            queue,
            physical_size,
            scale_factor,
            &self.overlay_scene_rectangles,
        );

        let physical_editor_width = (editor_view_width * scale_factor).round() as i32;
        let physical_height = physical_size.height.min(i32::MAX as u32) as i32;
        let gutter_right = (layout.gutter_width * scale_factor).round() as i32;
        let content_top = theme::CONTENT_TOP * scale_factor;
        let editor_left = layout.code_left * scale_factor;
        let (
            font_system,
            swash_cache,
            tab_labels,
            tab_reveal_actions,
            tab_close_actions,
            line_numbers,
            code,
            overlay_buffer,
            overlay_documentation_buffer,
        ) = editor.render_parts();
        let line_number_area = TextArea {
            buffer: line_numbers,
            left: 0.0,
            top: content_top,
            scale: scale_factor,
            bounds: TextBounds {
                left: 0,
                top: 0,
                right: gutter_right,
                bottom: physical_height,
            },
            default_color: theme::LINE_NUMBER_TEXT,
            custom_glyphs: &[],
        };
        let code_area = TextArea {
            buffer: code,
            left: editor_left,
            top: content_top,
            scale: scale_factor,
            bounds: TextBounds {
                left: gutter_right,
                top: 0,
                right: physical_editor_width,
                bottom: physical_height,
            },
            default_color: theme::EDITOR_TEXT,
            custom_glyphs: &[],
        };
        let cursor_text_area = cursor_rectangle.map(|rectangle| TextArea {
            buffer: code,
            left: editor_left,
            top: content_top,
            scale: scale_factor,
            bounds: physical_bounds(rectangle, scale_factor),
            default_color: theme::CURSOR_TEXT,
            custom_glyphs: &[],
        });
        let overlay_area = overlay_buffer
            .zip(overlay_geometry)
            .and_then(|(buffer, geometry)| {
                let rectangle = translate_overlay_rectangle(
                    geometry.origin,
                    geometry.size,
                    geometry.window_coordinates,
                    layout,
                    editor_view_width,
                    logical_height,
                    theme::OVERLAY_BACKGROUND,
                )?;
                Some(TextArea {
                    buffer,
                    left: (rectangle.origin[0] + theme::OVERLAY_PADDING) * scale_factor,
                    top: (rectangle.origin[1] + theme::OVERLAY_PADDING) * scale_factor,
                    scale: scale_factor,
                    bounds: physical_bounds(rectangle, scale_factor),
                    default_color: theme::OVERLAY_TEXT,
                    custom_glyphs: &[],
                })
            });
        let overlay_documentation_area = overlay_documentation_buffer
            .zip(overlay_geometry)
            .and_then(|(buffer, geometry)| {
                let rectangle = translate_overlay_rectangle(
                    geometry.origin,
                    geometry.size,
                    geometry.window_coordinates,
                    layout,
                    editor_view_width,
                    logical_height,
                    theme::OVERLAY_BACKGROUND,
                )?;
                Some(TextArea {
                    buffer,
                    left: (rectangle.origin[0] + theme::COMPLETION_WIDTH + theme::OVERLAY_PADDING)
                        * scale_factor,
                    top: (rectangle.origin[1] + theme::OVERLAY_PADDING) * scale_factor,
                    scale: scale_factor,
                    bounds: physical_bounds(rectangle, scale_factor),
                    default_color: theme::OVERLAY_TEXT,
                    custom_glyphs: &[],
                })
            });
        let tab_text_top = (theme::TAB_BAR_HEIGHT - theme::LINE_HEIGHT) * 0.5;
        let tab_areas = (0..tab_count).map(|index| {
            let tab_left = index as f32 * tab_width;
            TextArea {
                buffer: tab_labels,
                left: (tab_left + theme::TAB_TEXT_HORIZONTAL_PADDING) * scale_factor,
                top: (tab_text_top - index as f32 * theme::LINE_HEIGHT) * scale_factor,
                scale: scale_factor,
                bounds: TextBounds {
                    left: (tab_left * scale_factor).round() as i32,
                    top: (tab_text_top * scale_factor).ceil() as i32,
                    right: ((tab_action_geometry(index, TabAction::Reveal, tab_width).0[0]
                        - theme::TAB_LABEL_ACTION_GAP)
                        .max(tab_left)
                        * scale_factor)
                        .round()
                        .min(physical_editor_width as f32) as i32,
                    bottom: ((tab_text_top + theme::LINE_HEIGHT) * scale_factor).floor() as i32,
                },
                default_color: if index == active_tab {
                    theme::TAB_ACTIVE_TEXT
                } else {
                    theme::TAB_INACTIVE_TEXT
                },
                custom_glyphs: &[],
            }
        });
        let tab_action_areas = (0..tab_count).flat_map(|index| {
            [
                (tab_reveal_actions, TabAction::Reveal),
                (tab_close_actions, TabAction::Close),
            ]
            .map(move |(buffer, action)| {
                let (origin, size) = tab_action_geometry(index, action, tab_width);
                TextArea {
                    buffer,
                    left: (origin[0]
                        + (theme::TAB_ACTION_BUTTON_WIDTH - theme::APPROXIMATE_CELL_WIDTH) * 0.5)
                        * scale_factor,
                    top: (tab_text_top - index as f32 * theme::LINE_HEIGHT) * scale_factor,
                    scale: scale_factor,
                    bounds: TextBounds {
                        left: (origin[0] * scale_factor).round() as i32,
                        top: (tab_text_top * scale_factor).ceil() as i32,
                        right: ((origin[0] + size[0]) * scale_factor).round() as i32,
                        bottom: ((tab_text_top + theme::LINE_HEIGHT) * scale_factor).floor() as i32,
                    },
                    default_color: if index == active_tab
                        || hovered_tab_action == Some((index, action))
                    {
                        theme::TAB_ACTIVE_TEXT
                    } else {
                        theme::TAB_INACTIVE_TEXT
                    },
                    custom_glyphs: &[],
                }
            })
        });
        let editor_areas = [Some(line_number_area), Some(code_area), cursor_text_area]
            .into_iter()
            .flatten();
        let text_areas = tab_areas.chain(tab_action_areas).chain(editor_areas);

        self.text_renderer.prepare(
            device,
            queue,
            font_system,
            &mut self.text_atlas,
            &self.text_viewport,
            text_areas,
            swash_cache,
        )?;
        self.overlay_text_renderer.prepare(
            device,
            queue,
            font_system,
            &mut self.text_atlas,
            &self.text_viewport,
            overlay_area.into_iter().chain(overlay_documentation_area),
            swash_cache,
        )?;
        self.agent_panel.prepare(
            ModalPrepareContext {
                device,
                queue,
                physical_size,
                scale_factor,
                viewport: &self.text_viewport,
                atlas: &mut self.text_atlas,
                logical_viewport: [logical_width, logical_height],
            },
            agent_panel_view.as_ref(),
        )?;
        self.modal.prepare(
            ModalPrepareContext {
                device,
                queue,
                physical_size,
                scale_factor,
                viewport: &self.text_viewport,
                atlas: &mut self.text_atlas,
                logical_viewport: [logical_width, logical_height],
            },
            modal_view.as_ref(),
        )
    }

    pub fn render<'pass>(
        &'pass self,
        render_pass: &mut RenderPass<'pass>,
    ) -> Result<(), glyphon::RenderError> {
        if self.splash.visible {
            self.splash
                .render(render_pass, &self.text_atlas, &self.text_viewport)?;
        } else {
            self.rectangles.render(render_pass);
            self.text_renderer
                .render(&self.text_atlas, &self.text_viewport, render_pass)?;
            self.overlay_rectangles.render(render_pass);
            self.overlay_text_renderer.render(
                &self.text_atlas,
                &self.text_viewport,
                render_pass,
            )?;
            self.agent_panel
                .render(render_pass, &self.text_atlas, &self.text_viewport)?;
        }
        self.modal
            .render(render_pass, &self.text_atlas, &self.text_viewport)
    }

    pub fn finish_frame(&mut self) {
        self.text_atlas.trim();
    }

    fn editor_hit_width(&self, viewport_width: f32) -> f32 {
        self.agent_panel_view
            .as_ref()
            .map_or(viewport_width, |view| {
                let layout = AgentPanelLayout::calculate(
                    [viewport_width, theme::INITIAL_WINDOW_HEIGHT as f32],
                    view.visible,
                    view.width_ratio,
                );
                if layout.mode == AgentPanelMode::Narrow {
                    0.0
                } else {
                    layout.editor_width
                }
            })
    }
}

fn tab_width(viewport_width: f32, tab_count: usize) -> f32 {
    if tab_count == 0 {
        return 0.0;
    }
    (viewport_width / tab_count as f32).min(theme::MAXIMUM_TAB_WIDTH)
}

fn tab_at_position(position: [f32; 2], viewport_width: f32, tab_count: usize) -> Option<usize> {
    if position[0] < 0.0
        || position[1] < 0.0
        || position[1] >= theme::TAB_BAR_HEIGHT
        || tab_count == 0
    {
        return None;
    }
    let width = tab_width(viewport_width, tab_count);
    let index = (position[0] / width).floor() as usize;
    (index < tab_count).then_some(index)
}

fn tab_action_at_position(
    position: [f32; 2],
    viewport_width: f32,
    tab_count: usize,
) -> Option<(usize, TabAction)> {
    let index = tab_at_position(position, viewport_width, tab_count)?;
    [TabAction::Reveal, TabAction::Close]
        .into_iter()
        .find(|action| {
            let (origin, size) =
                tab_action_geometry(index, *action, tab_width(viewport_width, tab_count));
            position[0] >= origin[0]
                && position[0] < origin[0] + size[0]
                && position[1] >= origin[1]
                && position[1] < origin[1] + size[1]
        })
        .map(|action| (index, action))
}

fn tab_action_geometry(index: usize, action: TabAction, tab_width: f32) -> ([f32; 2], [f32; 2]) {
    let right = (index + 1) as f32 * tab_width - theme::TAB_ACTION_RIGHT_PADDING;
    let left = match action {
        TabAction::Close => right - theme::TAB_ACTION_BUTTON_WIDTH,
        TabAction::Reveal => right - 2.0 * theme::TAB_ACTION_BUTTON_WIDTH - theme::TAB_ACTION_GAP,
    };
    (
        [
            left,
            (theme::TAB_BAR_HEIGHT - theme::TAB_ACTION_BUTTON_WIDTH) * 0.5,
        ],
        [
            theme::TAB_ACTION_BUTTON_WIDTH,
            theme::TAB_ACTION_BUTTON_WIDTH,
        ],
    )
}

fn splash_layout(logical_viewport: [f32; 2]) -> SplashLayout {
    let safe_width = (logical_viewport[0] - 2.0 * theme::SPLASH_MINIMUM_PADDING).max(1.0);
    let content_width = theme::SPLASH_CONTENT_WIDTH.min(safe_width);
    let mark_line_count = SPLASH_MARK.lines().count() as f32;
    let mark_height = mark_line_count * theme::SPLASH_MARK_LINE_HEIGHT;
    let tagline_height = theme::LINE_HEIGHT;
    let actions_height = 2.0 * theme::SPLASH_ACTION_HEIGHT + theme::SPLASH_ACTION_GAP;
    let total_height = mark_height
        + theme::SPLASH_TAGLINE_TOP_GAP
        + tagline_height
        + theme::SPLASH_ACTION_TOP_GAP
        + actions_height;
    let content_left = ((logical_viewport[0] - content_width) * 0.5)
        .max(theme::SPLASH_MINIMUM_PADDING)
        .min((logical_viewport[0] - content_width).max(theme::SPLASH_MINIMUM_PADDING));
    let available_top = (logical_viewport[1] - total_height) * 0.48;
    let content_top = available_top.max(theme::SPLASH_MINIMUM_PADDING).min(
        (logical_viewport[1] - total_height - theme::SPLASH_MINIMUM_PADDING)
            .max(theme::SPLASH_MINIMUM_PADDING),
    );
    let mark_origin = [content_left, content_top];
    let mark_size = [content_width, mark_height];
    let divider_width = SPLASH_DIVIDER_WIDTH.min(content_width);
    let divider_origin = [
        content_left + (content_width - divider_width) * 0.5,
        content_top + mark_height + 7.0,
    ];
    let tagline_origin = [
        content_left,
        content_top + mark_height + theme::SPLASH_TAGLINE_TOP_GAP,
    ];
    let first_action_top = tagline_origin[1] + tagline_height + theme::SPLASH_ACTION_TOP_GAP;
    let open_file = Rectangle::new(
        [content_left, first_action_top],
        [content_width, theme::SPLASH_ACTION_HEIGHT],
        [0.0; 4],
    );
    let open_directory = Rectangle::new(
        [
            content_left,
            first_action_top + theme::SPLASH_ACTION_HEIGHT + theme::SPLASH_ACTION_GAP,
        ],
        [content_width, theme::SPLASH_ACTION_HEIGHT],
        [0.0; 4],
    );

    SplashLayout {
        content_origin: [content_left, content_top],
        content_width,
        mark_origin,
        mark_size,
        divider: Rectangle::new(
            divider_origin,
            [divider_width, SPLASH_DIVIDER_HEIGHT],
            color_from_glyph(theme::SPLASH_MUTED_TEXT),
        ),
        tagline_origin,
        tagline_size: [content_width, tagline_height],
        actions: SplashGeometry {
            open_file,
            open_directory,
        },
    }
}

fn splash_rectangles(
    geometry: SplashGeometry,
    divider: Rectangle,
    hovered_action: Option<SplashAction>,
) -> Vec<Rectangle> {
    let mut rectangles = Vec::with_capacity(11);
    rectangles.push(divider);
    for (action, rectangle) in [
        (SplashAction::OpenFile, geometry.open_file),
        (SplashAction::OpenDirectory, geometry.open_directory),
    ] {
        rectangles.push(Rectangle::new(
            rectangle.origin,
            rectangle.size,
            if hovered_action == Some(action) {
                theme::SPLASH_ACTION_HOVER_BACKGROUND
            } else {
                theme::SPLASH_ACTION_BACKGROUND
            },
        ));
        push_tab_outline(
            &mut rectangles,
            rectangle.origin,
            rectangle.size,
            1.0,
            if hovered_action == Some(action) {
                theme::SPLASH_ACTION_HOVER_BORDER
            } else {
                theme::SPLASH_ACTION_BORDER
            },
        );
    }
    rectangles
}

fn splash_action_at_position(position: [f32; 2], geometry: SplashGeometry) -> Option<SplashAction> {
    if contains_rectangle(position, geometry.open_file) {
        Some(SplashAction::OpenFile)
    } else if contains_rectangle(position, geometry.open_directory) {
        Some(SplashAction::OpenDirectory)
    } else {
        None
    }
}

fn update_splash_hover_state(
    hovered_action: &mut Option<SplashAction>,
    position: [f32; 2],
    logical_viewport: [f32; 2],
) -> bool {
    let next_hovered_action =
        splash_action_at_position(position, splash_layout(logical_viewport).actions);
    let changed = *hovered_action != next_hovered_action;
    *hovered_action = next_hovered_action;
    changed
}

fn contains_rectangle(position: [f32; 2], rectangle: Rectangle) -> bool {
    position[0] >= rectangle.origin[0]
        && position[1] >= rectangle.origin[1]
        && position[0] < rectangle.origin[0] + rectangle.size[0]
        && position[1] < rectangle.origin[1] + rectangle.size[1]
}

fn color_from_glyph(color: glyphon::Color) -> [f32; 4] {
    let [r, g, b, a] = color.as_rgba();
    [
        srgb_u8_to_linear(r),
        srgb_u8_to_linear(g),
        srgb_u8_to_linear(b),
        a as f32 / 255.0,
    ]
}

fn srgb_u8_to_linear(value: u8) -> f32 {
    let value = value as f32 / 255.0;
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

fn modal_text_attributes() -> Attrs<'static> {
    Attrs::new().family(Family::Name(theme::FONT_FAMILY))
}

fn splash_text_attributes() -> Attrs<'static> {
    Attrs::new().family(Family::Name(theme::FONT_FAMILY))
}

fn modal_text(view: &ModalView, visible_rows: usize) -> String {
    let mut text = String::new();
    if let Some(action) = &view.header_action {
        text.push_str(&action.label);
        text.push_str("  ");
    }
    text.push_str(&view.title);
    text.push('\n');
    text.push_str(&view.subtitle);
    text.push_str("\n\n");
    for row in &view.rows {
        if let Some(control) = &row.control {
            text.push_str(&format!("{:<7}", control.label));
        }
        text.push_str(&"  ".repeat(row.depth));
        text.push_str(if row.expandable {
            if row.expanded { "▾ " } else { "▸ " }
        } else {
            "  "
        });
        text.push_str(&row.label);
        for badge in &row.badges {
            text.push_str("  ");
            text.push_str(&badge.label);
        }
        text.push('\n');
    }
    for _ in view.rows.len()..visible_rows {
        text.push('\n');
    }
    text.push('\n');
    text.push_str(&view.status);
    if let Some(composer) = &view.composer {
        text.push('\n');
        let mut value = if composer.value.is_empty() {
            composer.placeholder.clone()
        } else {
            composer.value.clone()
        };
        if composer.focused {
            let cursor = composer.cursor.min(value.len());
            value.insert(cursor, '|');
        }
        text.push_str(&value);
        text.push_str("    ");
        text.push_str(&composer.button_label);
    }
    text
}

fn modal_rectangles(
    view: &ModalView,
    geometry: ModalGeometry,
    logical_width: f32,
    logical_height: f32,
) -> Vec<Rectangle> {
    let mut rectangles = Vec::with_capacity(12 + view.rows.len() * 3);
    rectangles.push(Rectangle::new(
        [0.0, 0.0],
        [logical_width, logical_height],
        theme::MODAL_SCRIM,
    ));
    rectangles.push(Rectangle::new(
        geometry.origin,
        geometry.size,
        theme::MODAL_BACKGROUND,
    ));
    push_tab_outline(
        &mut rectangles,
        geometry.origin,
        geometry.size,
        1.0,
        theme::MODAL_BORDER,
    );
    rectangles.push(Rectangle::new(
        [
            geometry.origin[0],
            geometry.origin[1] + theme::MODAL_HEADER_HEIGHT - 1.0,
        ],
        [geometry.size[0], 1.0],
        theme::MODAL_BORDER,
    ));
    rectangles.push(Rectangle::new(
        [geometry.origin[0], geometry.footer_origin[1]],
        [geometry.size[0], 1.0],
        theme::MODAL_BORDER,
    ));
    rectangles.push(Rectangle::new(
        geometry.close_origin,
        geometry.close_size,
        theme::MODAL_BADGE_BACKGROUND,
    ));
    if let Some(action) = &view.header_action {
        rectangles.push(Rectangle::new(
            geometry.header_action_origin,
            geometry.header_action_size,
            if action.enabled {
                theme::MODAL_CONTROL_BACKGROUND
            } else {
                theme::MODAL_CONTROL_DISABLED
            },
        ));
    }

    for (row_index, row) in view.rows.iter().enumerate() {
        let row_origin = [
            geometry.content_origin[0],
            geometry.content_origin[1] + row_index as f32 * theme::LINE_HEIGHT,
        ];
        if let Some(color) = match row.tone {
            ModalRowTone::Default => None,
            ModalRowTone::Muted => Some(theme::MODAL_CONTROL_DISABLED),
            ModalRowTone::Section => Some(theme::MODAL_ROW_SECTION),
            ModalRowTone::Addition => Some(theme::MODAL_ROW_ADDITION),
            ModalRowTone::Removal => Some(theme::MODAL_ROW_REMOVAL),
            ModalRowTone::Hunk => Some(theme::MODAL_ROW_HUNK),
        } {
            rectangles.push(Rectangle::new(
                row_origin,
                [geometry.content_size[0], theme::LINE_HEIGHT],
                color,
            ));
        }
        if row.selected || row.hovered {
            rectangles.push(Rectangle::new(
                row_origin,
                [geometry.content_size[0], theme::LINE_HEIGHT],
                if row.selected {
                    theme::MODAL_ROW_SELECTION
                } else {
                    theme::MODAL_ROW_HOVER
                },
            ));
        }
        if let Some(control) = &row.control {
            rectangles.push(Rectangle::new(
                [
                    row_origin[0],
                    row_origin[1] + (theme::LINE_HEIGHT - theme::MODAL_BADGE_HEIGHT) * 0.5,
                ],
                [
                    theme::MODAL_ROW_CONTROL_WIDTH - theme::MODAL_BADGE_HORIZONTAL_PADDING,
                    theme::MODAL_BADGE_HEIGHT,
                ],
                if control.enabled {
                    theme::MODAL_CONTROL_BACKGROUND
                } else {
                    theme::MODAL_CONTROL_DISABLED
                },
            ));
        }
        let control_columns = row.control.as_ref().map_or(0, |_| 7);
        let label_columns = control_columns + row.depth * 2 + 2 + row.label.chars().count();
        let mut badge_left = geometry.content_origin[0]
            + (label_columns as f32 + 2.0) * theme::APPROXIMATE_CELL_WIDTH
            - theme::MODAL_BADGE_HORIZONTAL_PADDING;
        for badge in &row.badges {
            let width = badge.label.chars().count() as f32 * theme::APPROXIMATE_CELL_WIDTH
                + 2.0 * theme::MODAL_BADGE_HORIZONTAL_PADDING;
            rectangles.push(Rectangle::new(
                [
                    badge_left,
                    row_origin[1] + (theme::LINE_HEIGHT - theme::MODAL_BADGE_HEIGHT) * 0.5,
                ],
                [width, theme::MODAL_BADGE_HEIGHT],
                theme::MODAL_BADGE_BACKGROUND,
            ));
            badge_left += width + 2.0 * theme::APPROXIMATE_CELL_WIDTH
                - 2.0 * theme::MODAL_BADGE_HORIZONTAL_PADDING;
        }
    }

    if let Some(composer) = &view.composer {
        rectangles.push(Rectangle::new(
            geometry.composer_field_origin,
            geometry.composer_field_size,
            theme::MODAL_FIELD_BACKGROUND,
        ));
        push_tab_outline(
            &mut rectangles,
            geometry.composer_field_origin,
            geometry.composer_field_size,
            1.0,
            if composer.focused {
                theme::TAB_ACTIVE_INDICATOR
            } else {
                theme::MODAL_BORDER
            },
        );
        rectangles.push(Rectangle::new(
            geometry.composer_button_origin,
            geometry.composer_button_size,
            if composer.button_enabled {
                theme::MODAL_PRIMARY_BUTTON
            } else {
                theme::MODAL_CONTROL_DISABLED
            },
        ));
    }

    if view.horizontal_content_width > 0 {
        let visible_columns =
            (geometry.content_size[0] / theme::APPROXIMATE_CELL_WIDTH).floor() as usize;
        if view.horizontal_content_width > visible_columns {
            let track_width = geometry.content_size[0];
            let thumb_width = (visible_columns as f32 / view.horizontal_content_width as f32
                * track_width)
                .max(theme::SCROLLBAR_MINIMUM_LENGTH)
                .min(track_width);
            let maximum_offset = view
                .horizontal_content_width
                .saturating_sub(visible_columns);
            let progress = if maximum_offset == 0 {
                0.0
            } else {
                view.horizontal_offset.min(maximum_offset) as f32 / maximum_offset as f32
            };
            rectangles.push(Rectangle::new(
                [
                    geometry.content_origin[0] + progress * (track_width - thumb_width),
                    geometry.content_origin[1] + geometry.content_size[1]
                        - theme::SCROLLBAR_THICKNESS,
                ],
                [thumb_width, theme::SCROLLBAR_THICKNESS],
                theme::SCROLLBAR_THUMB,
            ));
        }
    }

    if view.total_rows > geometry.visible_rows && geometry.visible_rows > 0 {
        let track_height = geometry.content_size[1];
        let thumb_height = (track_height * geometry.visible_rows as f32 / view.total_rows as f32)
            .max(theme::SCROLLBAR_MINIMUM_LENGTH)
            .min(track_height);
        let maximum_first = view.total_rows.saturating_sub(geometry.visible_rows);
        let progress = if maximum_first == 0 {
            0.0
        } else {
            view.first_row.min(maximum_first) as f32 / maximum_first as f32
        };
        rectangles.push(Rectangle::new(
            [
                geometry.origin[0] + geometry.size[0] - theme::SCROLLBAR_THICKNESS - 4.0,
                geometry.content_origin[1] + progress * (track_height - thumb_height),
            ],
            [theme::SCROLLBAR_THICKNESS, thumb_height],
            theme::SCROLLBAR_THUMB,
        ));
    }
    rectangles
}

fn agent_panel_text(view: &AgentPanelView) -> String {
    let mut text = String::new();
    text.push_str(&view.title);
    text.push('\n');
    text.push_str(&view.status);
    if view.new_event_count > 0 {
        text.push_str(" - ");
        text.push_str(&view.new_event_count.to_string());
        text.push_str(" new");
    }
    text.push_str("\n\n");
    for row in &view.rows {
        text.push_str(row);
        text.push('\n');
    }
    text.push('\n');
    text.push_str("> ");
    let mut composer = if view.composer.is_empty() {
        view.placeholder.clone()
    } else {
        view.composer.clone()
    };
    if view.focused {
        let cursor = view.composer_cursor.min(composer.len());
        composer.insert(cursor, '|');
    }
    text.push_str(&composer);
    text
}

fn agent_panel_rectangles(layout: AgentPanelLayout, focused: bool) -> Vec<Rectangle> {
    let Some(drawer) = layout.drawer else {
        return Vec::new();
    };
    let mut rectangles = Vec::with_capacity(12);
    if let Some(splitter) = layout.splitter {
        rectangles.push(Rectangle::new(
            splitter.origin,
            splitter.size,
            theme::MODAL_BORDER,
        ));
    }
    rectangles.push(Rectangle::new(
        drawer.origin,
        drawer.size,
        theme::MODAL_BACKGROUND,
    ));
    push_tab_outline(
        &mut rectangles,
        drawer.origin,
        drawer.size,
        1.0,
        theme::MODAL_BORDER,
    );
    if let Some(header) = layout.header {
        rectangles.push(Rectangle::new(
            header.origin,
            header.size,
            theme::TAB_ACTIVE_BACKGROUND,
        ));
        rectangles.push(Rectangle::new(
            [header.origin[0], header.origin[1] + header.size[1] - 1.0],
            [header.size[0], 1.0],
            theme::MODAL_BORDER,
        ));
    }
    if let Some(composer) = layout.composer {
        rectangles.push(Rectangle::new(
            composer.origin,
            composer.size,
            theme::MODAL_FIELD_BACKGROUND,
        ));
        push_tab_outline(
            &mut rectangles,
            composer.origin,
            composer.size,
            1.0,
            if focused {
                theme::TAB_ACTIVE_INDICATOR
            } else {
                theme::MODAL_BORDER
            },
        );
    }
    rectangles
}

fn push_tab_outline(
    rectangles: &mut Vec<Rectangle>,
    origin: [f32; 2],
    size: [f32; 2],
    thickness: f32,
    color: [f32; 4],
) {
    rectangles.extend([
        Rectangle::new(origin, [size[0], thickness], color),
        Rectangle::new(
            [origin[0], origin[1] + size[1] - thickness],
            [size[0], thickness],
            color,
        ),
        Rectangle::new(origin, [thickness, size[1]], color),
        Rectangle::new(
            [origin[0] + size[0] - thickness, origin[1]],
            [thickness, size[1]],
            color,
        ),
    ]);
}

fn logical_extent(physical_size: PhysicalSize<u32>, scale_factor: f32) -> (f32, f32) {
    (
        physical_size.width as f32 / scale_factor,
        physical_size.height as f32 / scale_factor,
    )
}

fn translate_selection_rectangle(
    selection: SelectionRectangle,
    layout: EditorLayout,
    viewport_width: f32,
    viewport_height: f32,
) -> Option<Rectangle> {
    translate_editor_rectangle(
        selection.origin,
        selection.size,
        layout,
        viewport_width,
        viewport_height,
        theme::SELECTION_BACKGROUND,
    )
}

fn translate_cursor_rectangle(
    cursor: CursorRectangle,
    layout: EditorLayout,
    viewport_width: f32,
    viewport_height: f32,
) -> Option<Rectangle> {
    translate_editor_rectangle(
        cursor.origin,
        cursor.size,
        layout,
        viewport_width,
        viewport_height,
        theme::CURSOR_BACKGROUND,
    )
}

fn translate_diagnostic_rectangle(
    diagnostic: DiagnosticRectangle,
    layout: EditorLayout,
    viewport_width: f32,
    viewport_height: f32,
) -> Option<Rectangle> {
    translate_editor_rectangle(
        diagnostic.origin,
        diagnostic.size,
        layout,
        viewport_width,
        viewport_height,
        diagnostic.color,
    )
}

fn translate_scrollbar_rectangle(
    scrollbar: ScrollbarRectangle,
    layout: EditorLayout,
    viewport_width: f32,
    viewport_height: f32,
) -> Option<Rectangle> {
    translate_editor_rectangle(
        scrollbar.origin,
        scrollbar.size,
        layout,
        viewport_width,
        viewport_height,
        theme::SCROLLBAR_THUMB,
    )
}

fn overlay_rectangles(
    overlay: OverlayGeometry,
    layout: EditorLayout,
    viewport_width: f32,
    viewport_height: f32,
) -> Vec<Rectangle> {
    let mut rectangles = Vec::with_capacity(5);
    if let Some(border) = translate_overlay_rectangle(
        overlay.origin,
        overlay.size,
        overlay.window_coordinates,
        layout,
        viewport_width,
        viewport_height,
        theme::OVERLAY_BORDER,
    ) {
        rectangles.push(border);
    }
    let inner_origin = [overlay.origin[0] + 1.0, overlay.origin[1] + 1.0];
    let inner_size = [
        (overlay.size[0] - 2.0).max(0.0),
        (overlay.size[1] - 2.0).max(0.0),
    ];
    if let Some(background) = translate_overlay_rectangle(
        inner_origin,
        inner_size,
        overlay.window_coordinates,
        layout,
        viewport_width,
        viewport_height,
        theme::OVERLAY_BACKGROUND,
    ) {
        rectangles.push(background);
    }
    if let Some(row) = overlay.selected_row {
        let origin = [
            overlay.origin[0] + 1.0,
            overlay.origin[1] + theme::OVERLAY_PADDING + row as f32 * theme::LINE_HEIGHT,
        ];
        let size = [(overlay.selection_width - 2.0).max(0.0), theme::LINE_HEIGHT];
        if let Some(selection) = translate_overlay_rectangle(
            origin,
            size,
            overlay.window_coordinates,
            layout,
            viewport_width,
            viewport_height,
            theme::OVERLAY_SELECTION,
        ) {
            rectangles.push(selection);
        }
    }
    if overlay.has_documentation_pane {
        let divider_origin = [
            overlay.origin[0] + overlay.selection_width,
            overlay.origin[1] + 1.0,
        ];
        if let Some(divider) = translate_overlay_rectangle(
            divider_origin,
            [1.0, (overlay.size[1] - 2.0).max(0.0)],
            overlay.window_coordinates,
            layout,
            viewport_width,
            viewport_height,
            theme::OVERLAY_BORDER,
        ) {
            rectangles.push(divider);
        }
    }
    if let Some(scroll) = overlay.completion_scroll
        && scroll.item_count > scroll.visible_items
        && scroll.visible_items > 0
    {
        let track_height = scroll.visible_items as f32 * theme::LINE_HEIGHT;
        let thumb_height = (track_height * scroll.visible_items as f32 / scroll.item_count as f32)
            .clamp(
                theme::SCROLLBAR_MINIMUM_LENGTH.min(track_height),
                track_height,
            );
        let maximum_first = scroll.item_count - scroll.visible_items;
        let offset =
            scroll.first_item as f32 / maximum_first as f32 * (track_height - thumb_height);
        let thumb = ScrollbarRectangle {
            origin: [
                overlay.origin[0] + overlay.selection_width
                    - theme::SCROLLBAR_MARGIN
                    - theme::SCROLLBAR_THICKNESS,
                overlay.origin[1] + theme::OVERLAY_PADDING + offset,
            ],
            size: [theme::SCROLLBAR_THICKNESS, thumb_height],
        };
        if let Some(rectangle) =
            translate_scrollbar_rectangle(thumb, layout, viewport_width, viewport_height)
        {
            rectangles.push(rectangle);
        }
    }
    rectangles
}

fn translate_overlay_rectangle(
    origin: [f32; 2],
    size: [f32; 2],
    window_coordinates: bool,
    layout: EditorLayout,
    viewport_width: f32,
    viewport_height: f32,
    color: [f32; 4],
) -> Option<Rectangle> {
    if !window_coordinates {
        return translate_editor_rectangle(
            origin,
            size,
            layout,
            viewport_width,
            viewport_height,
            color,
        );
    }
    let left = origin[0].max(0.0);
    let top = origin[1].max(0.0);
    let right = (origin[0] + size[0]).min(viewport_width);
    let bottom = (origin[1] + size[1]).min(viewport_height);
    (right > left && bottom > top).then_some(Rectangle::new(
        [left, top],
        [right - left, bottom - top],
        color,
    ))
}

fn translate_editor_rectangle(
    origin: [f32; 2],
    size: [f32; 2],
    layout: EditorLayout,
    viewport_width: f32,
    viewport_height: f32,
    color: [f32; 4],
) -> Option<Rectangle> {
    let left = (layout.code_left + origin[0]).max(layout.gutter_width);
    let top = (theme::CONTENT_TOP + origin[1]).max(0.0);
    let right = (layout.code_left + origin[0] + size[0]).min(viewport_width);
    let bottom = (theme::CONTENT_TOP + origin[1] + size[1]).min(viewport_height);

    (right > left && bottom > top).then_some(Rectangle::new(
        [left, top],
        [right - left, bottom - top],
        color,
    ))
}

fn physical_bounds(rectangle: Rectangle, scale_factor: f32) -> TextBounds {
    TextBounds {
        left: (rectangle.origin[0] * scale_factor).floor() as i32,
        top: (rectangle.origin[1] * scale_factor).floor() as i32,
        right: ((rectangle.origin[0] + rectangle.size[0]) * scale_factor).ceil() as i32,
        bottom: ((rectangle.origin[1] + rectangle.size[1]) * scale_factor).ceil() as i32,
    }
}

#[cfg(test)]
fn buffer_has_shaped_glyphs(buffer: &TextBuffer) -> bool {
    buffer.layout_runs().any(|run| !run.glyphs.is_empty())
}

#[cfg(test)]
mod tests {
    use super::{
        ModalTextState, Rectangle, RectangleInstance, SplashAction, SplashTextState, TabAction,
        logical_extent, splash_action_at_position, splash_layout, tab_action_at_position,
        tab_at_position, tab_width, translate_selection_rectangle, update_splash_hover_state,
    };
    use crate::editor::{EditorLayout, SelectionRectangle};
    use crate::modal::{ModalBadge, ModalRow, ModalView};
    use winit::dpi::PhysicalSize;

    #[test]
    fn logical_rectangle_is_scaled_once_for_retina() {
        let rectangle = Rectangle::new([4.0, 8.0], [64.0, 24.0], [1.0, 0.5, 0.25, 1.0]);
        let instance = RectangleInstance::from_logical(rectangle, 2.0);

        assert_eq!(instance.origin, [8.0, 16.0]);
        assert_eq!(instance.size, [128.0, 48.0]);
        assert_eq!(instance.color, rectangle.color);
    }

    #[test]
    fn physical_retina_viewport_converts_back_to_logical_points() {
        assert_eq!(
            logical_extent(PhysicalSize::new(1920, 1280), 2.0),
            (960.0, 640.0)
        );
    }

    #[test]
    fn horizontally_scrolled_selection_is_clipped_before_the_gutter() {
        let layout = EditorLayout {
            gutter_width: 64.0,
            gutter_text_width: 48.0,
            code_left: 80.0,
        };
        let selection = SelectionRectangle {
            origin: [-40.0, 0.0],
            size: [60.0, 24.0],
        };

        let rectangle = translate_selection_rectangle(selection, layout, 960.0, 640.0)
            .expect("selection remains partially visible");
        assert_eq!(rectangle.origin[0], 64.0);
        assert_eq!(rectangle.size[0], 36.0);
    }

    #[test]
    fn tabs_share_available_width_and_hit_test_only_the_strip() {
        assert_eq!(tab_width(960.0, 2), crate::theme::MAXIMUM_TAB_WIDTH);
        assert_eq!(tab_width(300.0, 3), 100.0);
        assert_eq!(tab_at_position([50.0, 12.0], 300.0, 3), Some(0));
        assert_eq!(tab_at_position([250.0, 12.0], 300.0, 3), Some(2));
        assert_eq!(tab_at_position([50.0, 50.0], 300.0, 3), None);
        assert_eq!(tab_at_position([500.0, 12.0], 960.0, 2), None);
    }

    #[test]
    fn tab_actions_have_separate_reveal_and_close_hit_targets() {
        assert_eq!(
            tab_action_at_position([155.0, 12.0], 400.0, 2),
            Some((0, TabAction::Reveal))
        );
        assert_eq!(
            tab_action_at_position([180.0, 12.0], 400.0, 2),
            Some((0, TabAction::Close))
        );
        assert_eq!(tab_action_at_position([100.0, 12.0], 400.0, 2), None);
        assert_eq!(tab_action_at_position([180.0, 50.0], 400.0, 2), None);
    }

    #[test]
    fn splash_layout_centers_default_viewport() {
        let layout = splash_layout([960.0, 640.0]);

        assert_eq!(layout.content_width, crate::theme::SPLASH_CONTENT_WIDTH);
        assert!((layout.content_origin[0] - 300.0).abs() < 0.01);
        let center_y = layout.content_origin[1]
            + (layout.actions.open_directory.origin[1] + layout.actions.open_directory.size[1]
                - layout.content_origin[1])
                * 0.5;
        assert!((center_y - 320.0).abs() < 10.0);
    }

    #[test]
    fn splash_layout_clamps_narrow_viewports_without_overlapping_actions() {
        let layout = splash_layout([300.0, 360.0]);

        assert_eq!(
            layout.content_origin[0],
            crate::theme::SPLASH_MINIMUM_PADDING
        );
        assert_eq!(layout.content_width, 252.0);
        assert!(
            layout.actions.open_directory.origin[1]
                >= layout.actions.open_file.origin[1]
                    + layout.actions.open_file.size[1]
                    + crate::theme::SPLASH_ACTION_GAP
        );
        assert!(
            layout.actions.open_directory.origin[1] + layout.actions.open_directory.size[1]
                <= 360.0
        );
    }

    #[test]
    fn splash_actions_hit_test_edges_and_outside_points() {
        let geometry = splash_layout([960.0, 640.0]).actions;
        let open_file = geometry.open_file;
        let open_directory = geometry.open_directory;

        assert_eq!(
            splash_action_at_position(open_file.origin, geometry),
            Some(SplashAction::OpenFile)
        );
        assert_eq!(
            splash_action_at_position(
                [
                    open_directory.origin[0] + open_directory.size[0] - 0.1,
                    open_directory.origin[1] + open_directory.size[1] - 0.1,
                ],
                geometry
            ),
            Some(SplashAction::OpenDirectory)
        );
        assert_eq!(
            splash_action_at_position(
                [open_file.origin[0] + open_file.size[0], open_file.origin[1]],
                geometry
            ),
            None
        );
        assert_eq!(
            splash_action_at_position([open_file.origin[0] - 1.0, open_file.origin[1]], geometry),
            None
        );
    }

    #[test]
    fn splash_hover_changes_only_when_the_action_changes() {
        let viewport = [960.0, 640.0];
        let geometry = splash_layout(viewport).actions;
        let mut hover = None;

        assert!(update_splash_hover_state(
            &mut hover,
            geometry.open_file.origin,
            viewport
        ));
        assert_eq!(hover, Some(SplashAction::OpenFile));
        assert!(!update_splash_hover_state(
            &mut hover,
            [
                geometry.open_file.origin[0] + 2.0,
                geometry.open_file.origin[1] + 2.0
            ],
            viewport
        ));
        assert!(update_splash_hover_state(
            &mut hover,
            geometry.open_directory.origin,
            viewport
        ));
        assert_eq!(hover, Some(SplashAction::OpenDirectory));
        assert!(update_splash_hover_state(
            &mut hover,
            [-1.0, -1.0],
            viewport
        ));
        assert_eq!(hover, None);
        assert!(!update_splash_hover_state(
            &mut hover,
            [-1.0, -1.0],
            viewport
        ));
    }

    #[test]
    fn splash_text_shapes_all_buffers_and_hides_stale_state() {
        let mut text = SplashTextState::new();

        text.prepare([960.0, 640.0], Some(SplashAction::OpenFile));

        assert!(text.has_shaped_glyphs());
        assert!(text.prepared);
        assert_eq!(text.hovered_action, Some(SplashAction::OpenFile));
        assert!(!text.scene_rectangles.is_empty());

        text.hide();

        assert!(!text.prepared);
        assert_eq!(text.hovered_action, None);
        assert!(text.scene_rectangles.is_empty());
    }

    #[test]
    fn loading_modal_shapes_title_subtitle_status_and_close_glyphs() {
        let mut text = ModalTextState::new();

        text.prepare(&loading_view(), [960.0, 640.0]);

        assert!(text.has_shaped_body_glyphs());
        assert!(text.has_shaped_close_glyphs());
        assert!(text.cached_body_text.contains("Project files"));
        assert!(text.cached_body_text.contains("/project"));
        assert!(text.cached_body_text.contains("Scanning project..."));
    }

    #[test]
    fn populated_modal_shapes_rows_and_badges() {
        let mut text = ModalTextState::new();

        text.prepare(&populated_view(), [960.0, 640.0]);

        assert!(text.has_shaped_body_glyphs());
        assert!(text.cached_body_text.contains("src"));
        assert!(text.cached_body_text.contains("main.rs"));
        assert!(text.cached_body_text.contains("dotfile"));
        assert!(text.cached_body_text.contains("ignored"));
        assert!(text.scene_rectangles.len() > 8);
    }

    #[test]
    fn replacing_modal_content_reshapes_body_buffer() {
        let mut text = ModalTextState::new();
        text.prepare(&loading_view(), [960.0, 640.0]);
        let loading_text = text.cached_body_text.clone();
        let loading_glyphs = body_glyph_count(&text);

        text.prepare(&populated_view(), [960.0, 640.0]);

        assert_ne!(text.cached_body_text, loading_text);
        assert_ne!(body_glyph_count(&text), loading_glyphs);
        assert!(text.cached_body_text.contains("main.rs"));
    }

    #[test]
    fn changing_modal_geometry_invalidates_layout_size_and_keeps_glyphs() {
        let mut text = ModalTextState::new();
        let view = populated_view();
        text.prepare(&view, [960.0, 640.0]);
        let initial_size = text.cached_body_size;

        text.prepare(&view, [480.0, 360.0]);

        assert_ne!(text.cached_body_size, initial_size);
        assert!(text.has_shaped_body_glyphs());
        assert!(text.has_shaped_close_glyphs());
    }

    #[test]
    fn repreparing_unchanged_modal_content_is_stable() {
        let mut text = ModalTextState::new();
        let view = populated_view();
        text.prepare(&view, [960.0, 640.0]);
        let first_text = text.cached_body_text.clone();
        let first_size = text.cached_body_size;
        let first_glyphs = body_glyph_count(&text);

        text.prepare(&view, [960.0, 640.0]);

        assert_eq!(text.cached_body_text, first_text);
        assert_eq!(text.cached_body_size, first_size);
        assert_eq!(body_glyph_count(&text), first_glyphs);
        assert!(text.has_shaped_body_glyphs());
    }

    #[test]
    fn hiding_modal_prevents_stale_state_from_being_submitted() {
        let mut text = ModalTextState::new();
        text.prepare(&populated_view(), [960.0, 640.0]);
        assert!(text.prepared);
        assert!(!text.scene_rectangles.is_empty());

        text.hide();

        assert!(!text.prepared);
        assert!(text.scene_rectangles.is_empty());
    }

    #[test]
    fn modal_text_state_owns_font_resources() {
        let mut text = ModalTextState::new();

        text.prepare(&populated_view(), [960.0, 640.0]);

        assert!(text.has_shaped_body_glyphs());
        assert!(text.has_shaped_close_glyphs());
    }

    fn loading_view() -> ModalView {
        ModalView {
            title: "Project files".to_string(),
            subtitle: "/project".to_string(),
            header_action: None,
            rows: Vec::new(),
            first_row: 0,
            total_rows: 0,
            status: "Scanning project...".to_string(),
            composer: None,
            horizontal_offset: 0,
            horizontal_content_width: 0,
        }
    }

    fn populated_view() -> ModalView {
        ModalView {
            title: "Project files".to_string(),
            subtitle: "/project".to_string(),
            header_action: None,
            rows: vec![
                ModalRow {
                    id: "src".to_string(),
                    depth: 0,
                    label: "src".to_string(),
                    badges: Vec::new(),
                    control: None,
                    tone: crate::modal::ModalRowTone::Default,
                    expandable: true,
                    expanded: true,
                    selected: false,
                    hovered: false,
                },
                ModalRow {
                    id: "src/main.rs".to_string(),
                    depth: 1,
                    label: "main.rs".to_string(),
                    badges: vec![
                        ModalBadge {
                            label: "dotfile".to_string(),
                        },
                        ModalBadge {
                            label: "ignored".to_string(),
                        },
                    ],
                    control: None,
                    tone: crate::modal::ModalRowTone::Default,
                    expandable: false,
                    expanded: false,
                    selected: true,
                    hovered: false,
                },
            ],
            first_row: 0,
            total_rows: 2,
            status: "2 entries".to_string(),
            composer: None,
            horizontal_offset: 0,
            horizontal_content_width: 0,
        }
    }

    fn body_glyph_count(text: &ModalTextState) -> usize {
        text.body_buffer
            .layout_runs()
            .map(|run| run.glyphs.len())
            .sum()
    }
}
