# editor

A small, native code editor for macOS, built in Rust as both a personal tool and a learning project.

The project is developed one reviewable milestone at a time. See [the build walkthrough](WALKTHROUGH.md) for the agreed scope, architecture, and acceptance checks.

## MVP controls

- Type normally to insert text, including Unicode and committed macOS IME text.
- Use Cmd+O to open a file, Cmd+S to save, and Cmd+Shift+S to Save As.
- Use the arrow keys to move the insertion cursor.
- Use Backspace to delete and Return to create a line.
- Use Tab to insert four spaces.
- Click to place the cursor; click and drag to select text.
- Type or press Backspace to replace or delete a selection.

The editor keeps its block cursor visible while the scratch buffer scrolls and suspends rendering when its window has no drawable size.

## Run

```bash
cargo run
```

Debug builds print the selected GPU adapter and first presented frame to the terminal once. Normal release builds do not print these diagnostics.

## Current limitations

The editor currently exposes one active document at a time. It does not yet provide:

- Undo, redo, copy, or paste
- Syntax highlighting or language tooling
- Keyboard-extended selection
- IME pre-edit text or candidate-window positioning
- Tabs, split panes, visible scrollbars, or settings

Dirty documents prompt before they are replaced or closed.

## Development checks

```bash
cargo fmt --check
cargo check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
cargo build --release
```
