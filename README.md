# editor

A small, native code editor for macOS, built in Rust as both a personal tool and a learning project.

The project is developed one reviewable milestone at a time. See [the build walkthrough](WALKTHROUGH.md) for the agreed scope, architecture, and acceptance checks.

## Current state

Milestone 3 is implemented: the application contains an editable in-memory scratch buffer with multiline typing, four-space Tab, Backspace, arrow-key movement, generated line numbers, and automatic cursor-following scroll. Mouse selection and a visible cursor arrive in later milestones.

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
