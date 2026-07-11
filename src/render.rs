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
use crate::editor::{CursorRectangle, EditorLayout, SelectionRectangle};
use crate::input::{ClipboardCommand, EditorCommand, EditorInput};
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
    text_viewport: Viewport,
    text_atlas: TextAtlas,
    text_renderer: TextRenderer,
    documents: Documents,
    scene_rectangles: Vec<Rectangle>,
    cursor_visible: bool,
}

impl Renderer {
    pub fn new(device: &Device, queue: &Queue, surface_format: TextureFormat) -> Self {
        let rectangles = RectangleRenderer::new(device, surface_format);
        let cache = Cache::new(device);
        let text_viewport = Viewport::new(device, &cache);
        let mut text_atlas = TextAtlas::new(device, queue, &cache, surface_format);
        let text_renderer =
            TextRenderer::new(&mut text_atlas, device, MultisampleState::default(), None);

        Self {
            rectangles,
            text_viewport,
            text_atlas,
            text_renderer,
            documents: Documents::new(),
            scene_rectangles: Vec::with_capacity(INITIAL_RECTANGLE_CAPACITY),
            cursor_visible: false,
        }
    }

    pub fn apply_input(&mut self, input: EditorInput) -> bool {
        self.documents.apply_input(input)
    }

    pub fn apply_command(&mut self, command: EditorCommand) {
        self.documents.apply_command(command);
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

    pub fn open_document(&mut self, path: std::path::PathBuf) -> Result<(), DocumentError> {
        self.documents.replace_active_from_path(path)
    }

    pub fn save_document(&mut self, path: std::path::PathBuf) -> Result<(), DocumentError> {
        self.documents.save_active_as(path)
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
        self.scene_rectangles.push(Rectangle::new(
            [0.0, 0.0],
            [layout.gutter_width, logical_height],
            theme::GUTTER_BACKGROUND,
        ));
        self.scene_rectangles.push(Rectangle::new(
            [layout.gutter_width - 1.0, 0.0],
            [1.0, logical_height],
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

        let physical_width = physical_size.width.min(i32::MAX as u32) as i32;
        let physical_height = physical_size.height.min(i32::MAX as u32) as i32;
        let gutter_right = (layout.gutter_width * scale_factor).round() as i32;
        let content_top = theme::CONTENT_TOP * scale_factor;
        let editor_left = layout.code_left * scale_factor;
        let (font_system, swash_cache, line_numbers, code) = editor.render_parts();
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
        let text_areas = [Some(line_number_area), Some(code_area), cursor_text_area]
            .into_iter()
            .flatten();

        self.text_renderer.prepare(
            device,
            queue,
            font_system,
            &mut self.text_atlas,
            &self.text_viewport,
            text_areas,
            swash_cache,
        )
    }

    pub fn render<'pass>(
        &'pass self,
        render_pass: &mut RenderPass<'pass>,
    ) -> Result<(), glyphon::RenderError> {
        self.rectangles.render(render_pass);
        self.text_renderer
            .render(&self.text_atlas, &self.text_viewport, render_pass)
    }

    pub fn finish_frame(&mut self) {
        self.text_atlas.trim();
    }
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
    use super::{Rectangle, RectangleInstance, logical_extent, translate_selection_rectangle};
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
}
