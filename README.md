# editor

A small, native code editor for macOS, built in Rust as both a personal tool and a learning project.

The project is developed one reviewable milestone at a time. See [the build walkthrough](WALKTHROUGH.md) for the agreed scope, architecture, and acceptance checks.

## MVP controls

- Type normally to insert text, including Unicode and committed macOS IME text.
- Use Cmd+O to open a file, Cmd+S to save, and Cmd+Shift+S to Save As.
- Use Cmd+A to select all and Cmd+C/X/V to copy, cut, and paste.
- Use Cmd+Z to undo and Cmd+Shift+Z to redo.
- Click a tab to switch documents; Control+Tab and Control+Shift+Tab cycle between them.
- Use Cmd+Shift+[ or Cmd+Shift+] as an alternate tab-switching shortcut, and Cmd+W to close the active tab.
- Use the arrow keys to move the insertion cursor.
- Hold Shift while moving to select; Option moves by word or paragraph, and Cmd moves to line or document boundaries.
- Use Backspace to delete and Return to create a line.
- Use Tab to insert four spaces.
- Click to place the cursor; click and drag to select text.
- Type or press Backspace to replace or delete a selection.
- Python `.py` and `.pyi` files receive incremental syntax highlighting; other files remain plain text.
- Python diagnostics are supplied by `pyright-langserver` when that executable is available on `PATH`.
- Use Ctrl+Space for Python completion, Cmd+I for hover information, and F12 to go to a definition.
- In completion menus, use Up/Down to navigate, Enter or Tab to accept, and Escape to dismiss.

The editor keeps its block cursor visible while the scratch buffer scrolls and suspends rendering when its window has no drawable size.

## Run

Install Pyright and ensure `pyright-langserver` is on `PATH` to enable diagnostics. The editor still runs normally without it.

```bash
cargo run
```

Debug builds print the selected GPU adapter and first presented frame to the terminal once. Normal release builds do not print these diagnostics.

## Current limitations

The editor supports multiple open documents with one active tab. It does not yet provide:

- Automatic completion triggers, signature help, symbol search, or refactoring commands
- IME pre-edit text or candidate-window positioning
- Split panes, visible scrollbars, or settings

Dirty documents prompt before their tab or the window is closed.

## Development checks

```bash
cargo fmt --check
cargo check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
cargo build --release
```
