mod agent;
mod app;
mod app_event;
mod clipboard;
mod cursor;
mod document;
mod editor;
mod git;
mod git_screen;
mod gpu;
mod input;
mod lsp;
mod markdown;
mod modal;
mod project;
mod render;
mod syntax;
mod theme;

fn main() {
    if let Err(error) = app::run() {
        eprintln!("editor failed: {error}");
        std::process::exit(1);
    }
}
