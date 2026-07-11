use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::sync::Arc;

use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::error::{EventLoopError, OsError};
use winit::event::{Ime, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

use crate::gpu::{GpuError, GpuState, RenderOutcome};
use crate::input::{EditorInput, InputState};
use crate::theme;

#[derive(Debug)]
pub enum AppError {
    EventLoop(EventLoopError),
    Window(OsError),
    Gpu(GpuError),
}

impl Display for AppError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::EventLoop(error) => write!(formatter, "event loop error: {error}"),
            Self::Window(error) => write!(formatter, "window creation error: {error}"),
            Self::Gpu(error) => write!(formatter, "GPU error: {error}"),
        }
    }
}

impl Error for AppError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::EventLoop(error) => Some(error),
            Self::Window(error) => Some(error),
            Self::Gpu(error) => Some(error),
        }
    }
}

impl From<EventLoopError> for AppError {
    fn from(error: EventLoopError) -> Self {
        Self::EventLoop(error)
    }
}

impl From<OsError> for AppError {
    fn from(error: OsError) -> Self {
        Self::Window(error)
    }
}

impl From<GpuError> for AppError {
    fn from(error: GpuError) -> Self {
        Self::Gpu(error)
    }
}

pub fn run() -> Result<(), AppError> {
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Wait);

    let mut application = Application::default();
    event_loop.run_app(&mut application)?;

    application.failure.map_or(Ok(()), Err)
}

#[derive(Default)]
struct Application {
    gpu: Option<GpuState>,
    input: InputState,
    failure: Option<AppError>,
}

impl Application {
    fn initialize(&mut self, event_loop: &ActiveEventLoop) -> Result<(), AppError> {
        let window_attributes = Window::default_attributes()
            .with_title(theme::WINDOW_TITLE)
            .with_inner_size(LogicalSize::new(
                theme::INITIAL_WINDOW_WIDTH,
                theme::INITIAL_WINDOW_HEIGHT,
            ))
            .with_visible(false);
        let window = Arc::new(event_loop.create_window(window_attributes)?);
        let gpu = pollster::block_on(GpuState::new(window, event_loop))?;

        gpu.window().set_ime_allowed(true);
        gpu.window().set_visible(true);
        gpu.window().request_redraw();
        self.gpu = Some(gpu);
        Ok(())
    }

    fn fail(&mut self, event_loop: &ActiveEventLoop, error: AppError) {
        if self.failure.is_none() {
            self.failure = Some(error);
        }
        event_loop.exit();
    }

    fn render_frame(&mut self, event_loop: &ActiveEventLoop) {
        let Some(gpu) = self.gpu.as_mut() else {
            return;
        };

        match gpu.render() {
            Ok(RenderOutcome::Retry) => gpu.window().request_redraw(),
            Ok(RenderOutcome::Presented | RenderOutcome::Skipped) => {}
            Err(error) => self.fail(event_loop, error.into()),
        }
    }

    fn apply_input(&mut self, input: EditorInput) {
        let Some(gpu) = self.gpu.as_mut() else {
            return;
        };

        gpu.apply_input(input);
        gpu.window().request_redraw();
    }
}

impl ApplicationHandler for Application {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(gpu) = &self.gpu {
            gpu.window().request_redraw();
            return;
        }

        if let Err(error) = self.initialize(event_loop) {
            self.fail(event_loop, error);
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        let Some(gpu) = &self.gpu else {
            return;
        };
        if window_id != gpu.window().id() {
            return;
        }

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                let gpu = self.gpu.as_mut().expect("GPU state was checked above");
                gpu.resize(size);
                self.render_frame(event_loop);
            }
            WindowEvent::ScaleFactorChanged { .. } => {
                let gpu = self.gpu.as_mut().expect("GPU state was checked above");
                gpu.resize(gpu.window().inner_size());
                self.render_frame(event_loop);
            }
            WindowEvent::Occluded(false) => {
                gpu.window().request_redraw();
            }
            WindowEvent::ModifiersChanged(modifiers) => {
                self.input.update_modifiers(modifiers);
            }
            WindowEvent::CursorMoved { position, .. } => {
                let scale_factor = self
                    .gpu
                    .as_ref()
                    .expect("GPU state was checked above")
                    .window()
                    .scale_factor();
                if let Some(input) = self.input.handle_cursor_moved(position, scale_factor) {
                    self.apply_input(input);
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if let Some(input) = self.input.handle_mouse_input(state, button) {
                    self.apply_input(input);
                }
            }
            WindowEvent::Focused(false) => self.input.cancel_pointer_drag(),
            WindowEvent::KeyboardInput { event, .. } => {
                if let Some(input) = self.input.handle_key_event(&event) {
                    self.apply_input(input);
                }
            }
            WindowEvent::Ime(Ime::Commit(text)) if !text.is_empty() => {
                self.apply_input(EditorInput::InsertText(text));
            }
            WindowEvent::RedrawRequested => self.render_frame(event_loop),
            _ => {}
        }
    }
}
