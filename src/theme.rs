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
pub const TAB_ACTION_HOVER_BACKGROUND: [f32; 4] = [0.10, 0.10, 0.115, 1.0];
pub const TAB_ACTIVE_INDICATOR: [f32; 4] = [0.18, 0.55, 0.90, 1.0];
pub const TAB_DIRTY_OUTLINE: [f32; 4] = [0.85, 0.65, 0.13, 1.0];
pub const SELECTION_BACKGROUND: [f32; 4] = [0.019_382_361, 0.078_187_42, 0.187_820_78, 1.0];
pub const CURSOR_BACKGROUND: [f32; 4] = [0.093_058_96, 0.332_451_55, 0.672_443_15, 1.0];
pub const DIAGNOSTIC_ERROR: [f32; 4] = [0.88, 0.18, 0.20, 1.0];
pub const DIAGNOSTIC_WARNING: [f32; 4] = [0.85, 0.65, 0.13, 1.0];
pub const DIAGNOSTIC_INFORMATION: [f32; 4] = [0.18, 0.55, 0.90, 1.0];
pub const DIAGNOSTIC_HINT: [f32; 4] = [0.45, 0.55, 0.60, 1.0];
pub const OVERLAY_BACKGROUND: [f32; 4] = [0.025, 0.025, 0.027, 1.0];
pub const OVERLAY_BORDER: [f32; 4] = [0.12, 0.12, 0.13, 1.0];
pub const OVERLAY_SELECTION: [f32; 4] = [0.04, 0.12, 0.25, 1.0];
pub const MODAL_SCRIM: [f32; 4] = [0.0, 0.0, 0.0, 0.62];
pub const MODAL_BACKGROUND: [f32; 4] = [0.019, 0.019, 0.022, 1.0];
pub const MODAL_BORDER: [f32; 4] = [0.16, 0.16, 0.18, 1.0];
pub const MODAL_ROW_HOVER: [f32; 4] = [0.035, 0.05, 0.075, 1.0];
pub const MODAL_ROW_SELECTION: [f32; 4] = [0.035, 0.10, 0.21, 1.0];
pub const MODAL_BADGE_BACKGROUND: [f32; 4] = [0.09, 0.09, 0.11, 1.0];
pub const SCROLLBAR_THUMB: [f32; 4] = [0.32, 0.32, 0.34, 0.75];
pub const OVERLAY_TEXT: glyphon::Color = glyphon::Color::rgb(220, 220, 220);
pub const MODAL_TEXT: glyphon::Color = glyphon::Color::rgb(224, 224, 228);
pub const EDITOR_TEXT: glyphon::Color = glyphon::Color::rgb(212, 212, 212);
pub const LINE_NUMBER_TEXT: glyphon::Color = glyphon::Color::rgb(133, 133, 133);
pub const CURSOR_TEXT: glyphon::Color = glyphon::Color::rgb(30, 30, 30);
pub const TAB_ACTIVE_TEXT: glyphon::Color = glyphon::Color::rgb(225, 225, 225);
pub const TAB_INACTIVE_TEXT: glyphon::Color = glyphon::Color::rgb(150, 150, 150);
pub const SYNTAX_ATTRIBUTE: glyphon::Color = glyphon::Color::rgb(156, 220, 254);
pub const SYNTAX_BUILTIN: glyphon::Color = glyphon::Color::rgb(78, 201, 176);
pub const SYNTAX_COMMENT: glyphon::Color = glyphon::Color::rgb(106, 153, 85);
pub const SYNTAX_CONSTANT: glyphon::Color = glyphon::Color::rgb(79, 193, 255);
pub const SYNTAX_FUNCTION: glyphon::Color = glyphon::Color::rgb(220, 220, 170);
pub const SYNTAX_KEYWORD: glyphon::Color = glyphon::Color::rgb(197, 134, 192);
pub const SYNTAX_NUMBER: glyphon::Color = glyphon::Color::rgb(181, 206, 168);
pub const SYNTAX_OPERATOR: glyphon::Color = glyphon::Color::rgb(212, 212, 212);
pub const SYNTAX_STRING: glyphon::Color = glyphon::Color::rgb(206, 145, 120);
pub const SYNTAX_TYPE: glyphon::Color = glyphon::Color::rgb(78, 201, 176);

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
pub const MAXIMUM_TAB_WIDTH: f32 = 240.0;
pub const TAB_TEXT_HORIZONTAL_PADDING: f32 = 12.0;
pub const TAB_ACTION_BUTTON_WIDTH: f32 = 20.0;
pub const TAB_ACTION_GAP: f32 = 2.0;
pub const TAB_ACTION_RIGHT_PADDING: f32 = 4.0;
pub const TAB_LABEL_ACTION_GAP: f32 = 6.0;
pub const CONTENT_TOP: f32 = TAB_BAR_HEIGHT + 16.0;
pub const CONTENT_RIGHT_PADDING: f32 = 16.0;
pub const CONTENT_BOTTOM_PADDING: f32 = 16.0;
pub const COMPLETION_WIDTH: f32 = 360.0;
pub const COMPLETION_DOCUMENTATION_WIDTH: f32 = 360.0;
pub const HOVER_MINIMUM_WIDTH: f32 = 220.0;
pub const HOVER_MAXIMUM_WIDTH: f32 = 560.0;
pub const OVERLAY_PADDING: f32 = 8.0;
pub const SCROLLBAR_THICKNESS: f32 = 5.0;
pub const SCROLLBAR_MARGIN: f32 = 3.0;
pub const SCROLLBAR_MINIMUM_LENGTH: f32 = 24.0;
pub const MODAL_WINDOW_MARGIN: f32 = 24.0;
pub const MODAL_MAXIMUM_WIDTH: f32 = 720.0;
pub const MODAL_MAXIMUM_HEIGHT: f32 = 560.0;
pub const MODAL_PADDING: f32 = 16.0;
pub const MODAL_HEADER_HEIGHT: f32 = 80.0;
pub const MODAL_FOOTER_HEIGHT: f32 = 32.0;
pub const MODAL_CLOSE_SIZE: f32 = 28.0;
pub const MODAL_BADGE_HEIGHT: f32 = 18.0;
pub const MODAL_BADGE_HORIZONTAL_PADDING: f32 = 6.0;
