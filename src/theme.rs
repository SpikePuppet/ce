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
pub const TAB_ACTIVE_BACKGROUND: [f32; 4] = [0.018_500_22, 0.018_500_22, 0.019_382_361, 1.0];
pub const TAB_INACTIVE_BACKGROUND: [f32; 4] = [0.012_983_032, 0.012_983_032, 0.012_983_032, 1.0];
pub const TAB_DIVIDER: [f32; 4] = [0.024_157_632, 0.024_157_632, 0.024_157_632, 1.0];
pub const SELECTION_BACKGROUND: [f32; 4] = [0.019_382_361, 0.078_187_42, 0.187_820_78, 1.0];
pub const CURSOR_BACKGROUND: [f32; 4] = [0.093_058_96, 0.332_451_55, 0.672_443_15, 1.0];
pub const EDITOR_TEXT: glyphon::Color = glyphon::Color::rgb(212, 212, 212);
pub const LINE_NUMBER_TEXT: glyphon::Color = glyphon::Color::rgb(133, 133, 133);
pub const CURSOR_TEXT: glyphon::Color = glyphon::Color::rgb(30, 30, 30);
pub const TAB_ACTIVE_TEXT: glyphon::Color = glyphon::Color::rgb(225, 225, 225);
pub const TAB_INACTIVE_TEXT: glyphon::Color = glyphon::Color::rgb(150, 150, 150);

pub const FONT_FAMILY: &str = "Menlo";
pub const FONT_SIZE: f32 = 15.0;
pub const LINE_HEIGHT: f32 = 24.0;
pub const TAB_WIDTH: u16 = 4;
pub const MINIMUM_GUTTER_WIDTH: f32 = 64.0;
pub const GUTTER_LEFT_PADDING: f32 = 16.0;
pub const GUTTER_TEXT_RIGHT_PADDING: f32 = 16.0;
pub const EDITOR_TEXT_LEFT_PADDING: f32 = 16.0;
pub const APPROXIMATE_CELL_WIDTH: f32 = 9.0;
pub const TAB_BAR_HEIGHT: f32 = 36.0;
pub const MAXIMUM_TAB_WIDTH: f32 = 200.0;
pub const TAB_TEXT_HORIZONTAL_PADDING: f32 = 12.0;
pub const CONTENT_TOP: f32 = TAB_BAR_HEIGHT + 16.0;
pub const CONTENT_RIGHT_PADDING: f32 = 16.0;
pub const CONTENT_BOTTOM_PADDING: f32 = 16.0;
