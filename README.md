# editor

A small, native code editor for macOS, built in Rust as both a personal tool and a learning project.

The project is developed one reviewable milestone at a time. See [the build walkthrough](WALKTHROUGH.md) for the agreed scope, architecture, and acceptance checks.

## Current state

Milestone 1 is implemented: the application opens a native macOS window, initializes `wgpu` against Metal, and presents the editor background color. Text editing begins in a later milestone.

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
