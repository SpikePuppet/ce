# Modal architecture

The editor's centered modal is a shared application surface. The project file tree is its first
screen, not a special-case overlay. Git, search, diagnostics, project commands, and other future
systems should extend this surface through the contracts in `src/modal.rs` instead of adding new
window-level rendering or input branches.

## Responsibilities

The modal is split into four layers:

1. `ModalHost<S>` owns visibility, pointer and keyboard translation, scroll accumulation, outside
   click and close-button behavior, and the viewport-dependent page size. It is generic over a
   `ModalScreen` and contains no file-tree or Git logic.
2. `ModalScreen` is the content contract. A screen returns a `ModalView` and handles semantic
   `ModalAction` values such as move, expand, collapse, activate, hover, click, and scroll. It
   returns a `ModalOutcome` for application-level effects.
3. `ModalView`, `ModalRow`, and `ModalBadge` are renderer-neutral presentation data. Rows support
   hierarchy, expansion, selection, hover, and any number of badges. The renderer never inspects
   a filesystem path or domain object.
4. `Renderer` consumes only `ModalView`. Its modal render state owns the modal rectangle renderer,
   text renderer, font system, glyph cache, and text buffers. It draws the scrim, centered card,
   header, clipped visible rows, selection and hover states, badges, close button, status footer,
   and proportional scrollbar as the final window layer.

The current concrete screen is `FileTreeScreen` in `src/project.rs`. Its project traversal and Git
ignore detection run on a worker thread. The application polls the completed snapshot when the
worker requests a redraw, then updates the screen and regenerates its view.

## Data and event flow

```text
winit event
    -> Application checks global modal shortcuts
    -> visible ModalHost captures the event
    -> ModalAction
    -> ModalScreen mutates domain state
    -> ModalOutcome (optional application effect)
    -> ModalView
    -> GpuState
    -> Renderer final overlay pass
```

The application owns effects that cross subsystem boundaries. For example, `FileTreeScreen`
returns `ModalOutcome::OpenFile`; `Application` opens or activates the document through the normal
tab path, synchronizes window and LSP state, and then republishes the still-visible modal view.
Screens must not reach into `GpuState`, `Documents`, or the native event loop directly.

## Input invariants

- Global shortcuts such as Cmd+T are evaluated first so they can close or replace the active
  modal screen.
- While a modal is visible, all remaining pointer, wheel, and keyboard events are consumed before
  completion, tab, or editor handling. There is no click-through or text editing behind a modal.
- Geometry and hit testing both use `ModalGeometry`, keeping Retina conversion and responsive
  sizing out of screen implementations.
- `ModalHost` turns raw input into semantic actions. Screens decide what those actions mean.
- Opening a document is an outcome, not a close action. The modal stays visible unless the screen
  explicitly returns `ModalOutcome::Close`.

## Rendering invariants

- Modal rendering happens after editor rectangles, editor text, scrollbars, completion, hover, and
  diagnostic overlays. The modal is the final window layer.
- Modal preparation is independent from document and tab preparation. Modal text must use the
  modal-owned font system, glyph cache, buffers, and text renderer rather than the active document's
  resources.
- Cosmic Text buffers must be shaped after direct text updates. Any modal `set_text` call must be
  followed by `shape_until_scroll` before layout runs are submitted to Glyphon.
- Screens provide only the currently visible page of rows. The complete domain model can be large,
  but text shaping and GPU row geometry stay proportional to the modal viewport.
- Screen-specific colors, geometry, or text buffers should not be introduced. Add generic row or
  badge presentation data when a future screen needs a genuinely reusable visual affordance.
- Window-space geometry remains in logical points until the existing renderer boundary converts it
  to physical pixels.

## Adding another screen

1. Define domain state such as `GitScreen` and implement `ModalScreen`.
2. Map the domain model to generic `ModalRow` and `ModalBadge` values. For example, Git can attach
   `modified`, `staged`, `untracked`, or `conflict` badges without changing the renderer.
3. Translate `ModalAction` values into domain navigation and return a `ModalOutcome` for effects the
   application must perform.
4. Add the screen to an application-level screen router. An enum that delegates `ModalScreen` is
   preferred when one Cmd+T surface can switch among multiple systems.
5. Keep expensive discovery or commands off the window event thread. Workers should publish owned
   results and request a redraw; only the application mutates live screen state.
6. Test the screen independently with semantic actions and `ModalView` assertions. Re-test the host
   only when changing shared input, geometry, or dismissal behavior.

## File-tree policies

The file tree deliberately inventories every readable directory entry. It does not apply hidden,
ignore, `.git`, or build-output filters. Names beginning with `.` receive a `dotfile` badge and Git
ignored paths receive an `ignored` badge; an entry can display both. Git ignore classification is
performed in one batched `git check-ignore` operation after traversal.

Directory symlinks are followed only while their canonical target remains inside the project root
and has not already been visited. The symlink entry is always retained, while root confinement and
canonical tracking prevent escapes, cycles, or repeated traversal of the same directory. Unreadable
directories are counted in the status footer because their contents cannot be enumerated by the
operating system.
