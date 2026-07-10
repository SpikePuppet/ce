pub const WINDOW_TITLE: &str = "Editor";
pub const INITIAL_WINDOW_WIDTH: f64 = 960.0;
pub const INITIAL_WINDOW_HEIGHT: f64 = 640.0;

pub const EDITOR_BACKGROUND: wgpu::Color = wgpu::Color {
    // #1e1e1e converted from sRGB to linear for an sRGB swapchain.
    r: 0.012_983_032,
    g: 0.012_983_032,
    b: 0.012_983_032,
    a: 1.0,
};

pub const GUTTER_BACKGROUND: [f32; 4] = [0.009_134_059, 0.009_134_059, 0.009_134_059, 1.0];
pub const GUTTER_DIVIDER: [f32; 4] = [0.024_157_632, 0.024_157_632, 0.024_157_632, 1.0];
pub const EDITOR_TEXT: glyphon::Color = glyphon::Color::rgb(212, 212, 212);
pub const LINE_NUMBER_TEXT: glyphon::Color = glyphon::Color::rgb(133, 133, 133);

pub const FONT_FAMILY: &str = "Menlo";
pub const FONT_SIZE: f32 = 15.0;
pub const LINE_HEIGHT: f32 = 24.0;
pub const GUTTER_WIDTH: f32 = 64.0;
pub const GUTTER_TEXT_RIGHT: f32 = 48.0;
pub const EDITOR_TEXT_LEFT: f32 = 80.0;
pub const CONTENT_TOP: f32 = 16.0;
pub const CONTENT_RIGHT_PADDING: f32 = 16.0;
pub const CONTENT_BOTTOM_PADDING: f32 = 16.0;
