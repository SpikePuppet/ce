use std::mem;

use bytemuck::{Pod, Zeroable};
use glyphon::{Cache, Resolution, TextArea, TextAtlas, TextBounds, TextRenderer, Viewport};
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

use crate::clipboard::ClipboardProvider;
use crate::document::{DocumentError, DocumentInfo, Documents};
use crate::editor::{
    CursorRectangle, DiagnosticRectangle, EditorLayout, OverlayGeometry, ScrollbarRectangle,
    SelectionRectangle,
};
use crate::input::{ClipboardCommand, EditorCommand, EditorInput, HistoryCommand};
use crate::lsp::{CompletionItem, DiagnosticUpdate, LspDocument, Position};
use crate::theme;

const INITIAL_RECTANGLE_CAPACITY: usize = 16;

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
    text_viewport: Viewport,
    text_atlas: TextAtlas,
    text_renderer: TextRenderer,
    overlay_text_renderer: TextRenderer,
    documents: Documents,
    scene_rectangles: Vec<Rectangle>,
    overlay_scene_rectangles: Vec<Rectangle>,
    cursor_visible: bool,
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

        Self {
            rectangles,
            overlay_rectangles,
            text_viewport,
            text_atlas,
            text_renderer,
            overlay_text_renderer,
            documents: Documents::new(),
            scene_rectangles: Vec::with_capacity(INITIAL_RECTANGLE_CAPACITY),
            overlay_scene_rectangles: Vec::with_capacity(7),
            cursor_visible: false,
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
        tab_at_position(position, viewport_width, self.documents.len())
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

    pub fn set_cursor_visible(&mut self, visible: bool) {
        self.cursor_visible = visible;
    }

    pub fn prepare(
        &mut self,
        device: &Device,
        queue: &Queue,
        physical_size: PhysicalSize<u32>,
        scale_factor: f32,
    ) -> Result<(), glyphon::PrepareError> {
        let (logical_width, logical_height) = logical_extent(physical_size, scale_factor);
        let tab_count = self.documents.len();
        let active_tab = self.documents.active_index();
        let tab_width = tab_width(logical_width, tab_count);
        let editor = self.documents.active_editor_mut();
        editor.resize(logical_width, logical_height);
        let layout = editor.layout();

        self.text_viewport.update(
            queue,
            Resolution {
                width: physical_size.width,
                height: physical_size.height,
            },
        );

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
        self.scene_rectangles.push(Rectangle::new(
            [0.0, theme::TAB_BAR_HEIGHT - 1.0],
            [logical_width, 1.0],
            theme::TAB_DIVIDER,
        ));
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
        self.scene_rectangles
            .extend(
                editor
                    .selection_rectangles()
                    .iter()
                    .filter_map(|rectangle| {
                        translate_selection_rectangle(
                            *rectangle,
                            layout,
                            logical_width,
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
                            logical_width,
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
                    translate_scrollbar_rectangle(scrollbar, layout, logical_width, logical_height)
                }),
        );
        if let Some(overlay) = overlay_geometry {
            self.overlay_scene_rectangles.extend(overlay_rectangles(
                overlay,
                layout,
                logical_width,
                logical_height,
            ));
        }
        let cursor_rectangle = self
            .cursor_visible
            .then(|| editor.cursor_rectangle())
            .flatten()
            .and_then(|rectangle| {
                translate_cursor_rectangle(rectangle, layout, logical_width, logical_height)
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

        let physical_width = physical_size.width.min(i32::MAX as u32) as i32;
        let physical_height = physical_size.height.min(i32::MAX as u32) as i32;
        let gutter_right = (layout.gutter_width * scale_factor).round() as i32;
        let content_top = theme::CONTENT_TOP * scale_factor;
        let editor_left = layout.code_left * scale_factor;
        let (
            font_system,
            swash_cache,
            tab_labels,
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
                right: physical_width,
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
                let rectangle = translate_editor_rectangle(
                    geometry.origin,
                    geometry.size,
                    layout,
                    logical_width,
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
                let rectangle = translate_editor_rectangle(
                    geometry.origin,
                    geometry.size,
                    layout,
                    logical_width,
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
                    top: 0,
                    right: ((tab_left + tab_width) * scale_factor)
                        .round()
                        .min(physical_width as f32) as i32,
                    bottom: (theme::TAB_BAR_HEIGHT * scale_factor).round() as i32,
                },
                default_color: if index == active_tab {
                    theme::TAB_ACTIVE_TEXT
                } else {
                    theme::TAB_INACTIVE_TEXT
                },
                custom_glyphs: &[],
            }
        });
        let editor_areas = [Some(line_number_area), Some(code_area), cursor_text_area]
            .into_iter()
            .flatten();
        let text_areas = tab_areas.chain(editor_areas);

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
        )
    }

    pub fn render<'pass>(
        &'pass self,
        render_pass: &mut RenderPass<'pass>,
    ) -> Result<(), glyphon::RenderError> {
        self.rectangles.render(render_pass);
        self.text_renderer
            .render(&self.text_atlas, &self.text_viewport, render_pass)?;
        self.overlay_rectangles.render(render_pass);
        self.overlay_text_renderer
            .render(&self.text_atlas, &self.text_viewport, render_pass)
    }

    pub fn finish_frame(&mut self) {
        self.text_atlas.trim();
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
    if let Some(border) = translate_editor_rectangle(
        overlay.origin,
        overlay.size,
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
    if let Some(background) = translate_editor_rectangle(
        inner_origin,
        inner_size,
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
        if let Some(selection) = translate_editor_rectangle(
            origin,
            size,
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
        if let Some(divider) = translate_editor_rectangle(
            divider_origin,
            [1.0, (overlay.size[1] - 2.0).max(0.0)],
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
mod tests {
    use super::{
        Rectangle, RectangleInstance, logical_extent, tab_at_position, tab_width,
        translate_selection_rectangle,
    };
    use crate::editor::{EditorLayout, SelectionRectangle};
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
        assert_eq!(tab_at_position([450.0, 12.0], 960.0, 2), None);
    }
}
