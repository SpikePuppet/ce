mod app;
mod gpu;
mod theme;

fn main() {
    if let Err(error) = app::run() {
        eprintln!("editor failed: {error}");
        std::process::exit(1);
    }
}
