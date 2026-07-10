mod app;
mod editor;
mod gpu;
mod render;
mod theme;

fn main() {
    if let Err(error) = app::run() {
        eprintln!("editor failed: {error}");
        std::process::exit(1);
    }
}
