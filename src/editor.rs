use glyphon::cosmic_text::Align;
use glyphon::{Attrs, Buffer, Family, FontSystem, Metrics, Shaping, SwashCache, Wrap};

use crate::theme;

const SAMPLE_CODE: &str = "fn main() {\n    println!(\"hello, editor\");\n}";
const SAMPLE_LINE_NUMBERS: &str = "1\n2\n3";

pub struct EditorPreview {
    font_system: FontSystem,
    swash_cache: SwashCache,
    line_numbers: Buffer,
    code: Buffer,
    logical_size: Option<(f32, f32)>,
}

impl EditorPreview {
    pub fn new() -> Self {
        let mut font_system = FontSystem::new();
        let swash_cache = SwashCache::new();
        let metrics = Metrics::new(theme::FONT_SIZE, theme::LINE_HEIGHT);
        let attrs = Attrs::new().family(Family::Name(theme::FONT_FAMILY));

        let mut line_numbers = Buffer::new(&mut font_system, metrics);
        line_numbers.set_wrap(Wrap::None);
        line_numbers.set_text(
            SAMPLE_LINE_NUMBERS,
            &attrs,
            Shaping::Advanced,
            Some(Align::Right),
        );

        let mut code = Buffer::new(&mut font_system, metrics);
        code.set_wrap(Wrap::None);
        code.set_text(SAMPLE_CODE, &attrs, Shaping::Advanced, None);

        Self {
            font_system,
            swash_cache,
            line_numbers,
            code,
            logical_size: None,
        }
    }

    pub fn resize(&mut self, width: f32, height: f32) {
        let size = (width, height);
        if self.logical_size == Some(size) {
            return;
        }
        self.logical_size = Some(size);

        let content_height = (height - theme::CONTENT_TOP - theme::CONTENT_BOTTOM_PADDING).max(1.0);
        let code_width = (width - theme::EDITOR_TEXT_LEFT - theme::CONTENT_RIGHT_PADDING).max(1.0);

        self.line_numbers
            .set_size(Some(theme::GUTTER_TEXT_RIGHT), Some(content_height));
        self.code.set_size(Some(code_width), Some(content_height));
        self.line_numbers
            .shape_until_scroll(&mut self.font_system, false);
        self.code.shape_until_scroll(&mut self.font_system, false);
    }

    pub fn render_parts(&mut self) -> (&mut FontSystem, &mut SwashCache, &Buffer, &Buffer) {
        (
            &mut self.font_system,
            &mut self.swash_cache,
            &self.line_numbers,
            &self.code,
        )
    }
}
