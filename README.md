# editor

A small, native code editor for macOS, built in Rust as both a personal tool and a learning project.

The project is developed one reviewable milestone at a time. See [the build walkthrough](WALKTHROUGH.md) for the agreed scope, architecture, and acceptance checks.

## Current state

Milestone 4 is implemented: the scratch buffer supports click placement and bidirectional, multiline mouse selection in addition to keyboard editing, generated line numbers, and automatic cursor-following scroll. A visible cursor arrives in the next milestone.

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
