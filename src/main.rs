mod app;
mod clipboard;
mod cursor;
mod document;
mod editor;
mod gpu;
mod input;
mod lsp;
mod render;
mod syntax;
mod theme;

fn main() {
    if let Err(error) = app::run() {
        eprintln!("editor failed: {error}");
        std::process::exit(1);
    }
}
