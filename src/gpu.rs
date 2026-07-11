use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::sync::Arc;

use wgpu::{
    AdapterInfo, Backend, Backends, CommandEncoderDescriptor, CurrentSurfaceTexture,
    DeviceDescriptor, Instance, InstanceDescriptor, LoadOp, Operations, PowerPreference,
    RenderPassColorAttachment, RenderPassDescriptor, RequestAdapterOptions, SurfaceConfiguration,
    TextureViewDescriptor,
};
use winit::dpi::PhysicalSize;
use winit::event_loop::ActiveEventLoop;
use winit::window::Window;

use crate::clipboard::ClipboardProvider;
use crate::document::{DocumentError, DocumentInfo};
use crate::input::{ClipboardCommand, EditorCommand, EditorInput, HistoryCommand};
use crate::render::Renderer;
use crate::theme;

#[derive(Debug)]
pub enum GpuError {
    CreateSurface(wgpu::CreateSurfaceError),
    RequestAdapter(wgpu::RequestAdapterError),
    RequestDevice(wgpu::RequestDeviceError),
    SurfaceUnsupported,
    UnexpectedBackend(Backend),
    SurfaceValidation,
    PrepareText(glyphon::PrepareError),
    RenderText(glyphon::RenderError),
}

impl Display for GpuError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::CreateSurface(error) => {
                write!(formatter, "could not create the GPU surface: {error}")
            }
            Self::RequestAdapter(error) => {
                write!(formatter, "could not select a GPU adapter: {error}")
            }
            Self::RequestDevice(error) => {
                write!(formatter, "could not create a GPU device: {error}")
            }
            Self::SurfaceUnsupported => {
                formatter.write_str("the selected GPU cannot present to this window")
            }
            Self::UnexpectedBackend(backend) => {
                write!(
                    formatter,
                    "expected the Metal backend, received {backend:?}"
                )
            }
            Self::SurfaceValidation => {
                formatter.write_str("wgpu reported a surface validation error")
            }
            Self::PrepareText(error) => write!(formatter, "could not prepare text: {error}"),
            Self::RenderText(error) => write!(formatter, "could not render text: {error}"),
        }
    }
}

impl Error for GpuError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::CreateSurface(error) => Some(error),
            Self::RequestAdapter(error) => Some(error),
            Self::RequestDevice(error) => Some(error),
            Self::PrepareText(error) => Some(error),
            Self::RenderText(error) => Some(error),
            Self::SurfaceUnsupported | Self::UnexpectedBackend(_) | Self::SurfaceValidation => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RenderOutcome {
    Presented,
    Retry,
    Skipped,
}

pub struct GpuState {
    instance: Instance,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface_config: SurfaceConfiguration,
    surface_is_configured: bool,
    renderer: Renderer,
    #[cfg(debug_assertions)]
    first_frame_reported: bool,

    // Keep the window last so it is dropped after the surface that references it.
    window: Arc<Window>,
}

impl GpuState {
    pub async fn new(window: Arc<Window>, event_loop: &ActiveEventLoop) -> Result<Self, GpuError> {
        let mut instance_descriptor = InstanceDescriptor::new_with_display_handle(Box::new(
            event_loop.owned_display_handle(),
        ));
        instance_descriptor.backends = Backends::METAL;

        let instance = Instance::new(instance_descriptor);
        let surface = instance
            .create_surface(window.clone())
            .map_err(GpuError::CreateSurface)?;
        let adapter = instance
            .request_adapter(&RequestAdapterOptions {
                power_preference: PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                ..Default::default()
            })
            .await
            .map_err(GpuError::RequestAdapter)?;

        let adapter_info = adapter.get_info();
        if adapter_info.backend != Backend::Metal {
            return Err(GpuError::UnexpectedBackend(adapter_info.backend));
        }
        report_adapter(&adapter_info);

        let (device, queue) = adapter
            .request_device(&DeviceDescriptor {
                label: Some("editor device"),
                ..Default::default()
            })
            .await
            .map_err(GpuError::RequestDevice)?;

        let initial_size = window.inner_size();
        let (width, height) = drawable_extent(initial_size).unwrap_or((1, 1));
        let mut surface_config = surface
            .get_default_config(&adapter, width, height)
            .ok_or(GpuError::SurfaceUnsupported)?;

        let capabilities = surface.get_capabilities(&adapter);
        if let Some(srgb_format) = capabilities
            .formats
            .iter()
            .copied()
            .find(wgpu::TextureFormat::is_srgb)
        {
            surface_config.format = srgb_format;
        }

        let surface_is_configured = drawable_extent(initial_size).is_some();
        if surface_is_configured {
            surface.configure(&device, &surface_config);
        }
        let renderer = Renderer::new(&device, &queue, surface_config.format);

        Ok(Self {
            instance,
            surface,
            device,
            queue,
            surface_config,
            surface_is_configured,
            renderer,
            #[cfg(debug_assertions)]
            first_frame_reported: false,
            window,
        })
    }

    pub fn window(&self) -> &Window {
        &self.window
    }

    pub fn resize(&mut self, size: PhysicalSize<u32>) {
        let Some((width, height)) = drawable_extent(size) else {
            self.surface_is_configured = false;
            return;
        };

        self.surface_config.width = width;
        self.surface_config.height = height;
        self.configure_surface();
    }

    pub fn apply_input(&mut self, input: EditorInput) -> bool {
        self.renderer.apply_input(input)
    }

    pub fn apply_command(&mut self, command: EditorCommand) {
        self.renderer.apply_command(command);
    }

    pub fn apply_history_command(&mut self, command: HistoryCommand) -> bool {
        self.renderer.apply_history_command(command)
    }

    pub fn break_history_group(&mut self) {
        self.renderer.break_history_group();
    }

    pub fn apply_clipboard_command<C: ClipboardProvider>(
        &mut self,
        command: ClipboardCommand,
        clipboard: &mut C,
    ) -> Result<bool, C::Error> {
        self.renderer.apply_clipboard_command(command, clipboard)
    }

    pub fn document_info(&self) -> DocumentInfo {
        self.renderer.document_info()
    }

    pub fn open_document(&mut self, path: std::path::PathBuf) -> Result<(), DocumentError> {
        self.renderer.open_document(path)
    }

    pub fn save_document(&mut self, path: std::path::PathBuf) -> Result<(), DocumentError> {
        self.renderer.save_document(path)
    }

    pub fn set_cursor_visible(&mut self, visible: bool) {
        self.renderer.set_cursor_visible(visible);
    }

    pub fn render(&mut self) -> Result<RenderOutcome, GpuError> {
        if !self.surface_is_configured {
            return Ok(RenderOutcome::Skipped);
        }

        let (frame, reconfigure_after_present) = match self.surface.get_current_texture() {
            CurrentSurfaceTexture::Success(frame) => (frame, false),
            CurrentSurfaceTexture::Suboptimal(frame) => (frame, true),
            CurrentSurfaceTexture::Timeout => return Ok(RenderOutcome::Retry),
            CurrentSurfaceTexture::Occluded => return Ok(RenderOutcome::Skipped),
            CurrentSurfaceTexture::Outdated => {
                self.configure_surface();
                return Ok(RenderOutcome::Retry);
            }
            CurrentSurfaceTexture::Lost => {
                self.recreate_surface()?;
                return Ok(RenderOutcome::Retry);
            }
            CurrentSurfaceTexture::Validation => return Err(GpuError::SurfaceValidation),
        };

        self.renderer
            .prepare(
                &self.device,
                &self.queue,
                PhysicalSize::new(self.surface_config.width, self.surface_config.height),
                self.window.scale_factor() as f32,
            )
            .map_err(GpuError::PrepareText)?;

        let view = frame.texture.create_view(&TextureViewDescriptor {
            label: Some("editor frame view"),
            ..Default::default()
        });
        let mut encoder = self
            .device
            .create_command_encoder(&CommandEncoderDescriptor {
                label: Some("editor frame encoder"),
            });

        {
            let mut render_pass = encoder.begin_render_pass(&RenderPassDescriptor {
                label: Some("editor clear pass"),
                color_attachments: &[Some(RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: Operations {
                        load: LoadOp::Clear(theme::EDITOR_BACKGROUND),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            self.renderer
                .render(&mut render_pass)
                .map_err(GpuError::RenderText)?;
        }

        self.queue.submit(Some(encoder.finish()));
        self.queue.present(frame);
        self.renderer.finish_frame();
        self.report_first_frame();

        if reconfigure_after_present {
            self.configure_surface();
        }

        Ok(RenderOutcome::Presented)
    }

    fn configure_surface(&mut self) {
        self.surface.configure(&self.device, &self.surface_config);
        self.surface_is_configured = true;
    }

    fn recreate_surface(&mut self) -> Result<(), GpuError> {
        self.surface = self
            .instance
            .create_surface(self.window.clone())
            .map_err(GpuError::CreateSurface)?;
        self.configure_surface();
        Ok(())
    }

    #[cfg(debug_assertions)]
    fn report_first_frame(&mut self) {
        if self.first_frame_reported {
            return;
        }

        eprintln!(
            "First frame: {}x{} physical pixels at {:.2}x scale",
            self.surface_config.width,
            self.surface_config.height,
            self.window.scale_factor()
        );
        self.first_frame_reported = true;
    }

    #[cfg(not(debug_assertions))]
    fn report_first_frame(&mut self) {}
}

fn drawable_extent(size: PhysicalSize<u32>) -> Option<(u32, u32)> {
    (size.width > 0 && size.height > 0).then_some((size.width, size.height))
}

fn report_adapter(adapter: &AdapterInfo) {
    #[cfg(debug_assertions)]
    eprintln!(
        "GPU adapter: {} ({:?}) via {:?}",
        adapter.name, adapter.device_type, adapter.backend
    );

    #[cfg(not(debug_assertions))]
    let _ = adapter;
}

#[cfg(test)]
mod tests {
    use super::drawable_extent;
    use winit::dpi::PhysicalSize;

    #[test]
    fn drawable_extent_accepts_non_zero_dimensions() {
        assert_eq!(
            drawable_extent(PhysicalSize::new(960, 640)),
            Some((960, 640))
        );
    }

    #[test]
    fn drawable_extent_rejects_zero_width() {
        assert_eq!(drawable_extent(PhysicalSize::new(0, 640)), None);
    }

    #[test]
    fn drawable_extent_rejects_zero_height() {
        assert_eq!(drawable_extent(PhysicalSize::new(960, 0)), None);
    }
}
