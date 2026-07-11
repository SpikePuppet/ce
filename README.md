# editor

A small, native code editor for macOS, built in Rust as both a personal tool and a learning project.

The project is developed one reviewable milestone at a time. See [the build walkthrough](WALKTHROUGH.md) for the agreed scope, architecture, and acceptance checks.

## Current state

Milestone 5 is implemented: the scratch buffer now has a focused, blinking block insertion cursor in addition to keyboard editing, click placement, bidirectional multiline selection, generated line numbers, and automatic cursor-following scroll.

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
