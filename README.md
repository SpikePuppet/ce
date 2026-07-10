# editor

A small, native code editor for macOS, built in Rust as both a personal tool and a learning project.

The project is developed one reviewable milestone at a time. See [the build walkthrough](WALKTHROUGH.md) for the agreed scope, architecture, and acceptance checks.

## Current state

Milestone 2 is implemented: the application opens a native macOS window, renders its gutter through a small Metal-backed rectangle pipeline, and draws a temporary Menlo code sample with `glyphon`. Interactive editing begins in the next milestone.

```bash
cargo run
```

## Development checks

```bash
cargo fmt --check
cargo check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```
