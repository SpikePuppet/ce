# Agent System Implementation Plan

## Status

This document is the implementation handoff for adding an agent system to the editor. It records the product decisions already made and turns them into reviewable engineering milestones.

The executor should treat the decisions marked **Required** as settled. Items marked **Suggested** may change if the codebase or an upstream API makes a better implementation apparent, but the user-facing behavior and safety properties must remain intact.

This is a plan only. Do not interpret the presence of this file as permission to implement every milestone in one pass.

## Executor operating rules

1. Preserve all pre-existing and uncommitted user work. In particular, the editor already has modal and project/file-tree work in progress. Inspect the worktree before each milestone and do not replace or revert unrelated changes.
2. Implement one milestone at a time. Keep the editor compiling and usable at the end of every milestone, run the milestone checks, summarize the result, and stop for review before moving on.
3. Do not commit, stage, create a branch, install OpenCode, or download external binaries unless the user asks.
4. Verify the current Rust ACP SDK and OpenCode APIs against their official documentation at implementation time. Do not invent protocol fields or rely on examples in this document as exact upstream APIs.
5. Keep all process, protocol, filesystem, and persistence work off the winit/AppKit UI thread.
6. Build the UI against a deterministic fake backend before coupling it to OpenCode. The editor must still launch and work normally when OpenCode is absent or misconfigured.
7. Treat security and auditability as product behavior, not deferred hardening. No write may bypass approval, and no collapsed UI may discard the underlying record.

## Objective

Add a first-class agent experience to the Rust editor, using the Agent Client Protocol (ACP) to connect to OpenCode. The initial release should let a user:

- open a focused agent drawer;
- chat with an OpenCode-backed agent;
- watch every step of a running turn as it happens;
- inspect the exact chronological activity behind any collapsed summary;
- approve or reject proposed file changes;
- review changes inline, in a full editor diff, or in a hover preview;
- keep multiple agent threads without turning the editor into a permanently cluttered chat client;
- configure supported providers using files under the user's home configuration directory.

The editor is an ACP client. OpenCode is the first and preferred agent harness. The editor does not implement its own LLM loop in this project phase.

## Settled product decisions

### Harness and protocol

- **Required:** Use ACP as the editor-to-agent boundary.
- **Required:** OpenCode is the first supported harness, launched through `opencode acp`.
- **Required:** The editor's domain model and UI must not depend directly on OpenCode-specific JSON. Keep room for another ACP harness later.
- **Required:** Negotiate ACP capabilities at runtime. Do not assume optional session, tool, terminal, or filesystem capabilities exist.
- **Required:** The first release permits read/search behavior and approval-gated proposed edits.
- **Required:** Autonomous terminal execution is outside the first release.

### Panel layout

- **Required:** The agent lives in a focused drawer on the right side of the editor.
- **Required:** Its default width is two-fifths of the available editor width (`0.4`).
- **Required:** A draggable splitter resizes it. The width ratio is clamped to a useful range, suggested as `0.30..=0.55`, while also preserving minimum editor and drawer widths.
- **Required:** The chosen width is remembered across launches.
- **Required:** When the window is too narrow to preserve both minimum widths, the focused drawer takes over the editor content area.
- **Required:** Narrow mode is determined from layout constraints, not a single hard-coded device-pixel breakpoint.
- **Required:** The drawer shows one active thread. There is no permanent agent-thread sidebar and threads do not become editor tabs.

### Thread navigation

- **Required:** Clicking the thread title opens a compact, searchable switcher.
- **Required:** The switcher prioritizes the current thread, pinned threads, running or waiting threads, and roughly three to five recent threads.
- **Required:** Older threads remain available through history/search.
- **Required:** Background threads may show `running`, `waiting`, `failed`, or `complete` attention states but never steal focus.

### Turn stack

- **Required:** A user request and the complete agent run it initiates form one turn.
- **Required:** The active turn stays expanded. Completed turns normally auto-collapse.
- **Required:** A collapsed turn shows at least its request/title, final state, action count, changed-file count, and duration.
- **Required:** Permission requests, failures, and interrupted work do not auto-collapse.
- **Required:** The active plan is pinned within the running turn. When complete, it folds into the turn's inspectable history.
- **Required:** Tool calls may be visually grouped, but grouping is presentation only.

### Exact audit trail

- **Required:** Summaries are derived indexes and never replace source events.
- **Required:** Expanding a turn reveals the exact chronological record retained by the editor.
- **Required:** Individual steps can reveal inputs, outputs, state transitions, permission decisions, timestamps, commands and exit status when available, file locations, and before/after diffs.
- **Required:** Every detail is labelled with its fidelity:
  - `Exact`: directly executed, observed, or supplied as raw ACP data.
  - `Agent-reported`: described by the harness without corresponding raw data.
  - `Truncated`: intentionally limited, with the limit made visible.
  - `Redacted`: sensitive material was removed.
- **Required:** ACP fields such as raw input or output are optional. The UI must distinguish omitted data from empty data.
- **Required:** OpenCode model-context compaction must not compact or replace the editor's local audit history.

### Live execution

- **Required:** Every ACP update appears in the active turn as it arrives: streaming text, plan changes, tool state, permission waits, file activity, diffs, errors, stop, and cancellation.
- **Required:** Tool details are expandable while the tool is still running.
- **Required:** Auto-scroll occurs only while the user is already pinned to the bottom.
- **Required:** If the user scrolls upward, preserve their position and show a `N new events` affordance.
- **Required:** A permission request remains pinned and visible until answered.
- **Required:** Cancellation visibly transitions unfinished work through cancellation and records the final states received from the harness.

### Change review

- **Required:** The drawer contains a compact change summary grouped by file.
- **Required:** Clicking a changed file opens its full diff in the editor.
- **Required:** `Review in editor` opens a full multi-file review workspace or review tab.
- **Required:** Hovering a changed file shows a noninteractive diff preview after a short delay, suggested as 200 ms.
- **Required:** Keyboard focus on a changed file provides the same preview behavior; hover cannot be the only access path.
- **Required:** In narrow/full-screen drawer mode, the diff preview overlays the conversation.
- **Required:** Applying a change requires explicit approval and conflict detection against the current buffer or disk version.

### Configuration and credentials

- **Required:** Do not use macOS Keychain.
- **Required:** Store editor agent configuration beneath `~/.config/editor/`.
- **Required:** Prefer one secret per file rather than one large secrets document.
- **Required:** Support configuration for OpenAI, Anthropic, Google, Baseten, AWS Bedrock, and generic OpenAI-compatible endpoints.
- **Required:** Bedrock configuration models the AWS credential chain: profile or access key/secret/session token plus region. Do not present it as a single API-key provider.
- **Required:** Generate OpenCode configuration that refers to secret files rather than copying secret values into generated JSON or environment variables.
- **Required:** Project configuration may provide instructions such as `AGENTS.md`, but project plugins and project-provided executable integrations are not automatically trusted in the first release.

## Initial-release non-goals

- A native Rust LLM or agent loop.
- Rig or another native model abstraction as a production backend.
- Multiple harness installation or a harness marketplace.
- Automatic download or installation of OpenCode.
- Autonomous terminal access.
- Enabling arbitrary project plugins or MCP servers by default.
- Remote/cloud ACP transports; use the local stdio process first.
- Vector search, embeddings, RAG, or a semantic code index.
- Inline edit prediction or autocomplete driven by the agent.
- Subagents, worktrees, or multi-agent orchestration.
- Cross-device sync or cloud thread history.
- Automatic history deletion or retention policies.
- Expanding platform support beyond the editor's current platform scope.
- Keychain integration.

## Proposed architecture

### Boundary diagram

```text
┌──────────────────────────────── Rust editor ────────────────────────────────┐
│                                                                            │
│  winit/AppKit UI thread                                                    │
│  ┌──────────────┐    commands     ┌─────────────────────────────────────┐  │
│  │ Agent drawer │ ───────────────▶ │ AgentRuntime background owner       │  │
│  │ + review UI  │                  │ - ACP client                         │  │
│  └──────┬───────┘ ◀─────────────── │ - OpenCode subprocess               │  │
│         │       AppEvent::Agent    │ - ordered persistence writer        │  │
│         │                          │ - filesystem/context bridge          │  │
│  ┌──────▼───────┐                  └──────────────────┬──────────────────┘  │
│  │ reducer +    │                                     │ stdio ACP            │
│  │ view models  │                                     │                      │
│  └──────────────┘                                     │                      │
└───────────────────────────────────────────────────────┼──────────────────────┘
                                                        ▼
                                             ┌────────────────────┐
                                             │ `opencode acp`     │
                                             │ provider adapters  │
                                             └────────────────────┘
```

The UI thread owns view state and rendering. `AgentRuntime` owns protocol and process state. The two communicate through typed channels plus the existing winit user-event mechanism. No ACP read, write, subprocess wait, configuration write, or audit-log fsync may block event handling or drawing.

### Event-loop integration

The application currently uses a winit user event specialized to language-server events. Generalize it to an application-wide event without changing language behavior:

```rust
enum AppEvent {
    Language(LspEvent),
    Agent(AgentEvent),
    // Add Project(ProjectEvent) only if project scanning actually benefits from it.
}
```

Suggested event path:

```text
ACP update
  → normalize and redact
  → append ordered source event / blob reference
  → publish AppEvent::Agent through EventLoopProxy
  → reduce domain state
  → derive the smallest affected view model
  → request redraw
```

`persist before publish` prevents the UI from showing an event that cannot be recovered after a crash. Persistence must still occur on a background thread. If persistence fails, publish a durable-error event if possible, mark the thread volatile or failed, surface the problem prominently, and refuse further mutating actions until the user understands that the audit guarantee is degraded.

Streaming text chunks may be coalesced for rendering at most once per event-loop iteration, but the stored record must retain either the raw chunks or an exact reconstructed final stream plus its event ordering. Never discard tool or permission transitions as a redraw optimization.

### Suggested source organization

Adapt this layout to existing project conventions rather than creating empty abstraction files:

```text
src/
  app_event.rs                 application-wide winit user event
  agent/
    mod.rs                     public agent subsystem surface
    model.rs                   backend-neutral thread/turn/event domain types
    reducer.rs                 deterministic state transitions
    runtime.rs                 background owner and command channel
    acp.rs                     ACP client adapter and capability mapping
    opencode.rs                command/config/process specifics
    config.rs                  config schema, paths, permissions, generation
    persistence.rs             JSONL, blobs, crash recovery, migrations
    context.rs                 open-buffer and workspace filesystem bridge
    changes.rs                 proposed changes, conflicts, approval/apply
    panel.rs                   panel interaction state and view-model building
    review.rs                  full diff/review model
```

It is acceptable to start with fewer files and split them as responsibilities solidify. Do not put ACP transport code, credential parsing, or filesystem mutations into `app.rs` or rendering code.

### Domain model

Use stable identifiers and an event-derived model. Exact names may vary, but the responsibilities should remain:

```rust
struct ThreadId(/* stable UUID-like value */);
struct TurnId(/* stable value */);
struct EventId(/* thread-local monotonically ordered sequence */);

struct AgentThread {
    id: ThreadId,
    metadata: ThreadMetadata,
    turns: Vec<Turn>,
    status: ThreadStatus,
}

struct Turn {
    id: TurnId,
    request: UserRequest,
    state: TurnState,
    events: Vec<EventId>,
    summary: TurnSummary,
}

enum TurnEventKind {
    UserMessage,
    AssistantTextDelta,
    AssistantTextCompleted,
    PlanUpdated,
    ToolCallStarted,
    ToolCallUpdated,
    ToolCallCompleted,
    PermissionRequested,
    PermissionResolved,
    FileRead,
    ChangeProposed,
    ChangeDecision,
    ChangeApplied,
    Error,
    CancellationRequested,
    TurnCompleted,
}

enum RecordFidelity {
    Exact,
    AgentReported,
    Truncated { limit: usize },
    Redacted,
}
```

Also model:

- `AgentCommand`: start thread, submit prompt, cancel turn, answer permission, apply/reject change, load history, shutdown.
- `AgentEvent`: normalized, UI-safe notifications from the runtime.
- `CapabilitySet`: negotiated ACP features, kept with the connection/session.
- `ToolCallRecord`: stable tool id, title, kind, state, locations, raw-data availability, timestamps, and blob references.
- `PermissionRequest`: options, scope, originating tool, state, and exact decision.
- `ChangeSet`: per-file before/after content or patches plus expected base identities.
- `AgentPanelState`: visibility, focus, scroll pin, new-event count, hover target, active thread, and switcher state.

Do not deserialize protocol objects directly into UI state. The ACP adapter should map protocol messages into backend-neutral events with explicit fidelity and omission information.

### Reducer rules

Keep state transitions deterministic and independently testable:

- Only the reducer changes thread/turn/tool domain state.
- Event sequence is monotonically increasing per thread.
- Define behavior for duplicate, late, or unknown-tool updates rather than panicking.
- Terminal states cannot silently return to running.
- A turn is not complete while it has an unanswered blocking permission.
- A cancellation request does not pretend cancellation has finished.
- Derived action/file counts come from retained events and change records.
- UI expansion/collapse and scroll state are local presentation state and are not the audit source of truth.
- Replaying the same persisted events after restart produces the same domain state.

### Rendering and input boundaries

Follow the existing modal architecture's separation between state/view and GPU rendering:

- Build immutable, renderer-oriented view models such as `AgentPanelView`, `TurnView`, `ToolView`, and `ChangedFileView`.
- Rendering consumes view models and geometry; it does not query ACP state or the active editor document.
- The agent composer owns an independent text buffer, selection, IME state, cursor, and scroll state. It is not an editor `Document`.
- Input is routed by explicit priority. Suggested order: permission/modal surface, thread switcher or hover surface, focused agent composer/panel, then editor.
- The drawer captures keyboard/text input only when the relevant agent control is focused.
- Resizing, hovering, focus changes, and scrolling must use the same geometry used for painting.
- The hover diff is noninteractive. Pointer movement into it must not create an accidental focus trap.
- Respect Unicode, IME composition, clipboard, cursor blinking, and high-DPI scaling from the first composer implementation.

### Layout calculation

Use one layout function as the source of truth for paint and hit testing:

```text
available_width = window width minus persistent chrome
desired_drawer   = available_width × stored_width_ratio
drawer_width     = clamp(desired_drawer, min_drawer, max_by_editor_min)

if available_width < min_drawer + splitter + min_editor:
    narrow takeover mode
else:
    split editor + drawer mode
```

Suggested configuration:

```toml
[ui.agent_panel]
width_ratio = 0.4
```

Write the new ratio at the end of a resize gesture rather than on every pointer move. Maintain the live value in memory during the drag. Use an atomic configuration update and preserve unrelated settings and comments where practical.

## ACP and OpenCode integration

### Runtime lifecycle

The first production adapter should:

1. Resolve an explicitly configured OpenCode binary or a clearly documented `PATH` lookup.
2. Launch `opencode acp` with piped stdin/stdout/stderr and the intended working directory.
3. Perform ACP initialization and store negotiated capabilities.
4. Create, load, or resume a session only when supported.
5. Map session updates into normalized events.
6. Route client callbacks for filesystem access and permission requests to the editor.
7. Support user cancellation and graceful shutdown.
8. Bound shutdown time, then terminate an unresponsive child without freezing the editor.
9. Surface command path, harness version when available, connection state, and capability limitations in diagnostics.

One ACP connection may support multiple sessions, but do not design UI state around that assumption. Keep connection identity and thread/session identity separate. If OpenCode or a negotiated capability cannot resume old sessions, retain the local history and clearly label a continued thread as a new remote session.

### Async runtime choice

The repository currently has no general async runtime. Milestone 0 must prove the smallest workable option with the current Rust toolchain and winit architecture. The likely design is a dedicated background thread that owns an async executor and the ACP connection. Requirements matter more than the executor choice:

- it must not take over the main thread;
- shutdown must be explicit and testable;
- command/event ordering must be defined;
- dependencies should remain proportionate to the feature;
- no nested runtime calls from UI callbacks;
- panics or child-process exits must become visible agent events.

### Capability handling

Map negotiated capabilities into user-visible behavior:

- Hide or disable unsupported actions with a short reason.
- Never infer write or terminal authority from a tool title.
- Treat session list/load/resume as optional.
- Preserve unknown ACP updates in the raw audit record where safe, even if the current UI cannot render them semantically.
- Record the protocol and harness version/capability snapshot at session start for later diagnosis.

### Fake ACP fixture

Before live integration, create a deterministic in-process or child-process fixture that can emit:

- streaming assistant text;
- a changing plan;
- overlapping tools with pending/running/completed states;
- a permission request and answer;
- file reads and a proposed multi-file edit;
- a large/truncated output;
- an error;
- cancellation during a tool;
- malformed, duplicate, late, and unknown updates;
- clean and abrupt disconnects.

Use this fixture for reducer, runtime, persistence, and UI tests. Live OpenCode tests are supplemental and must not be required for the standard offline test suite.

## Configuration design

### Directory layout

Use an injectable path provider so tests never touch the real home directory. The production default is:

```text
~/.config/editor/
  config.toml
  secrets/
    openai
    anthropic
    google
    baseten
    aws_access_key_id          # only for static-credential mode
    aws_secret_access_key
    aws_session_token         # optional
    <custom-provider-name>
  generated/
    opencode.json
  state/
    threads/
      <thread-id>/
        metadata.json
        events.jsonl
        blobs/
          <content-hash>
```

Permissions on Unix-like systems:

- configuration root and all child directories: `0700`;
- secret files: `0600`;
- generated OpenCode configuration: `0600`;
- thread metadata, logs, and blobs: `0600` because prompts and source content may be sensitive.

Set permissions on creation, verify them on load, and show a clear warning if group/world access is detected. Do not print secret content in that warning.

### Config schema

The exact TOML schema may evolve, but it must separate secret references from nonsecret provider metadata. For example:

```toml
[agent]
harness = "opencode"
binary = "opencode"

[ui.agent_panel]
width_ratio = 0.4

[providers.openai]
kind = "openai"
secret_file = "secrets/openai"
default_model = "<user-selected-model>"

[providers.local]
kind = "openai-compatible"
base_url = "http://localhost:8000/v1"
secret_file = "secrets/local"
default_model = "<user-selected-model>"

[providers.bedrock]
kind = "aws-bedrock"
region = "us-west-2"
profile = "default"
```

Do not put real model names in compiled defaults unless they are confirmed at implementation time. Provider models change; make model identifiers user-configurable and validate only the syntax the editor truly understands.

### Provider requirements

- **OpenAI:** API key file and optional base URL/organization fields supported by the chosen OpenCode provider.
- **Anthropic:** API key file and nonsecret model selection.
- **Google:** API key file and nonsecret model selection.
- **Baseten:** support its OpenAI-compatible endpoint shape unless OpenCode provides a better native adapter at implementation time.
- **Generic OpenAI-compatible:** arbitrary base URL, provider id, model id, optional headers that refer to secret files, and an adapter mode selected from the OpenCode-supported packages. Chat Completions and Responses compatibility must not be conflated.
- **AWS Bedrock:** region plus profile-based credentials by default; optionally static access key, secret, and session token files. Respect the AWS provider/credential resolution behavior actually supported by OpenCode.

### Generated OpenCode configuration

Generate `generated/opencode.json` deterministically and atomically. Prefer OpenCode's file substitution form, such as a `{file:...}` reference supported by the installed OpenCode version, so the JSON never contains the credential itself.

Set OpenCode's supported configuration override to point at this generated file. Avoid placing secret values in:

- process arguments;
- environment variables where file references are supported;
- event logs;
- debug output;
- UI error messages;
- generated snapshots or tests.

OpenCode may merge global and project configuration. The editor must make active configuration sources visible. In the first release, do not automatically enable project plugins or other project-provided executable extensions. If preventing their load requires an OpenCode setting or isolated config directory, verify and use the supported mechanism; otherwise surface the limitation before launching.

### Configuration writes

- Use temp-file-plus-rename atomic writes.
- Do not truncate a working config if serialization fails.
- Keep a typed schema version and a migration path from the first persisted release.
- Preserve unknown configuration fields where practical, or refuse a lossy rewrite with a clear message.
- Secret updates replace only the selected secret file.
- Never place sample or fake keys resembling real credentials in repository fixtures.

## Audit persistence

### Source-of-truth format

Each thread owns an append-only event stream:

```text
state/threads/<thread-id>/metadata.json
state/threads/<thread-id>/events.jsonl
state/threads/<thread-id>/blobs/<content-hash>
```

`metadata.json` contains thread identity, display title, pin/archive state, creation/update times, provider/harness references, and a schema version. It is written atomically.

`events.jsonl` contains ordered envelopes. A conceptual envelope is:

```json
{
  "schema_version": 1,
  "sequence": 42,
  "event_id": "...",
  "turn_id": "...",
  "recorded_at": "...",
  "source": "acp|editor|user",
  "fidelity": "exact|agent_reported|truncated|redacted",
  "kind": "tool_call_updated",
  "payload": {}
}
```

Large outputs, full buffer snapshots, and diffs move to content-addressed blobs. Events store hashes, byte sizes, media/type hints, truncation metadata, and whether the blob is available. Hash the exact stored bytes and verify on read.

### Recovery and migrations

- Replay events to rebuild turn summaries and domain state.
- Tolerate and ignore one partially written final JSONL record after a crash; do not silently skip corruption in the middle of the file.
- Validate monotonic sequence values and blob hashes.
- Treat indexes and summaries as disposable caches.
- Preserve unknown event kinds for forward compatibility.
- Never run a destructive migration without making failure recoverable.
- No automatic retention deletion in the first release. Provide explicit thread deletion later, with a confirmation step and blob cleanup scoped to that thread.

### Redaction

Redact before persistence and before UI publication. At minimum:

- exact configured secret values;
- common authorization header forms;
- provider tokens from environment/config error output;
- generated configuration fields that unexpectedly contain inline credentials.

Redaction must be represented in fidelity metadata. Do not promise arbitrary-secret detection; document that source files and prompts themselves are part of local history. Tests should use synthetic sentinel secrets and assert they do not appear anywhere under the test state directory.

## Editor context and filesystem bridge

### Context sent with a turn

The composer should make included context visible and removable. Suggested initial context:

- workspace root;
- active file path and language;
- selection or cursor location when explicitly included;
- open-buffer content for referenced files, including unsaved edits;
- current diagnostics relevant to included files;
- user-added files or ranges.

Avoid silently attaching the entire workspace. Display what will be sent before submission and keep context attachment separate from the text prompt.

### Filesystem reads

When ACP requests a file the editor owns:

- return the current unsaved buffer contents rather than stale disk content;
- preserve exact Unicode text;
- resolve paths against a canonical workspace root;
- reject parent traversal, absolute escape, and symlink escape;
- record the requested path, resolved path, source (`buffer` or `disk`), content identity, and fidelity;
- enforce reasonable output limits and label truncation.

### Proposed writes and conflict detection

The agent does not write directly to disk in the initial release. A proposal records:

- target path;
- before content identity (buffer revision and/or content hash);
- proposed after content or exact patch;
- originating tool and turn;
- any ACP-provided raw diff fields;
- generated editor diff and fidelity.

Before applying:

1. Re-resolve and revalidate the workspace path.
2. Compare the expected base identity with the current buffer or disk identity.
3. If unchanged, allow explicit approval to apply.
4. If changed, mark a conflict and open review; never overwrite silently.
5. Apply through the editor's document model so undo/dirty state and rendering remain correct.
6. Persist the decision and exact resulting content identity.

Multi-file changes should support approve all, reject all, and per-file decisions, but only after the review model can accurately represent partial approval. Avoid a misleading `Approve all` that fails halfway without reporting which files changed.

## UI behavior specification

### Drawer chrome

The focused drawer contains, from top to bottom:

1. thread title/status and switcher affordance;
2. scrollable turn stack;
3. pinned permission or blocking-error surface when present;
4. composer with visible context attachments and send/cancel action.

The drawer should feel like part of the editor rather than a floating modal. Reuse theme tokens, typography, focus rings, and existing GPU primitives where possible.

### Turn header and collapse

A completed turn's collapsed row should communicate useful state without becoming a second transcript:

```text
✓ Explain and update parser handling       7 actions · 2 files · 18s
```

Expansion restores the complete chronological timeline. Preserve manual expansion state during the session. A new active turn may collapse previously successful turns, but never force-collapse a permission, failure, or interrupted turn.

### Running timeline

Represent tool state explicitly rather than through prose alone:

```text
● Reading src/editor.rs                    running
✓ Searched for selection handling          34 ms
◐ Preparing changes                        waiting for approval
```

Each row has a stable identity so streaming updates modify the correct item rather than duplicating it. Text streaming and tool activity may interleave. Preserve true chronological order in the expanded audit, even if the compact active view groups adjacent low-value events.

### Permissions

A permission card must show:

- what action is requested;
- which file(s) or scope it affects;
- the originating tool/turn;
- available decisions and their scope;
- whether a decision is one-time or persisted;
- any missing detail that prevents informed approval.

The first release should prefer one-time approvals. Do not add broad permanent workspace write grants until their semantics and persistence are separately designed.

### Hover diff preview

- Start the timer only after the changed-file row is stable under the pointer or keyboard focus.
- Cancel when the target changes, the panel scrolls, a modal opens, or the row disappears.
- Render a bounded preview with file name, change counts, and the relevant diff hunk(s).
- Show an explicit truncation indicator for large diffs.
- Keep it noninteractive and outside the accessibility tab order.
- Clicking or activating the underlying row opens the full editor diff.
- Reuse modal overlay/rendering infrastructure only where it fits; the persistent drawer itself is not a modal.

### Full review workspace

`Review in editor` should open a review surface that can:

- navigate changed files;
- show before/after or unified diff using editor-quality text rendering;
- preserve unsaved buffer bases;
- show approval and conflict state;
- jump from a turn/tool to the relevant hunk;
- approve/reject at the supported granularity;
- return focus to the originating thread.

Do not overload ordinary source tabs with hidden review state. Choose either a clearly identified review tab/workspace or a dedicated review mode and make its lifecycle explicit.

### Accessibility and keyboard behavior

- Every hover action has a keyboard route.
- Focus order is deterministic and visible.
- Escape closes the topmost transient surface before hiding the drawer.
- Provide commands/shortcuts for toggling the drawer, focusing the composer, switching threads, cancelling a run, and opening review. Choose exact bindings after checking existing conflicts.
- Status is communicated by text/icon shape as well as color.
- Screen-reader semantics should be added where the current native/windowing stack allows; document platform limitations rather than silently omitting them.

## Milestone sequence

### Milestone 0 — Integration spike and recorded decisions

**Goal:** Prove the upstream pieces and choose the runtime boundary without changing editor behavior.

Tasks:

1. Verify the official `agent-client-protocol` crate builds with the repository's Rust version and dependency graph.
2. Verify an installed OpenCode can launch in ACP mode and complete initialize/session setup with a minimal Rust client or fixture.
3. Record actual capability negotiation and representative update shapes without storing real prompts or credentials in the repository.
4. Decide the async executor/background-thread design and document shutdown/error behavior.
5. Confirm the supported OpenCode overrides for isolated/generated config and file-referenced credentials.
6. Confirm whether project plugins/config can be disabled or isolated. Record any unavoidable trust limitation.
7. Decide how ACP raw messages are captured safely without coupling persistence to SDK-private representations.

Deliverable:

- A short `docs/AGENT_SYSTEM_ARCHITECTURE.md` decision record or a focused addition to this plan with verified versions and APIs.
- Optional throwaway test/spike code must be removed unless it becomes a maintained fixture.

Exit checks:

- No editor behavior regression.
- No real credentials, home paths, or provider responses committed.
- Unknown upstream behavior is converted into an explicit implementation decision.

### Milestone 1 — Application event and agent domain foundation

**Goal:** Establish typed, backend-neutral state and nonblocking event flow.

Tasks:

1. Generalize the winit user event from language-only to `AppEvent` while preserving all LSP behavior.
2. Add stable agent domain types, command/event types, and a pure reducer.
3. Add a background runtime shell with clean startup, command receipt, event publication, and shutdown.
4. Add a deterministic fake backend capable of a simple streaming turn.
5. Add reducer tests for valid and invalid state transitions.

Exit checks:

- Existing editor, file tree, modal, LSP, completion, hover, and navigation behavior still works.
- The fake backend can deliver events without blocking the UI thread.
- Duplicate, late, cancellation, and error events do not panic.
- No OpenCode dependency is needed to run tests or the editor.

### Milestone 2 — Focused drawer shell

**Goal:** Ship the layout and input container without production agent connectivity.

Tasks:

1. Add the right drawer, splitter, default `0.4` ratio, constraints, and narrow takeover mode.
2. Persist the ratio atomically beneath the injected configuration root.
3. Add independent composer text input with focus, selection, IME, clipboard, cursor, and scrolling.
4. Add explicit input routing between modal surfaces, agent panel, and editor.
5. Add wide/narrow/splitter geometry and hit-testing tests.
6. Add panel toggle/focus commands after resolving keybinding conflicts.

Exit checks:

- Drawer resizing is smooth and remembered after restart.
- Editor content is not painted or hit-tested beneath the drawer in split mode.
- Narrow takeover activates from constraints and returns cleanly to split mode.
- Opening/closing/focusing the drawer does not corrupt editor input or modal input.
- The panel works with the fake backend or a static placeholder only.

### Milestone 3 — Live turn stack and audit store

**Goal:** Make the complete agreed interaction testable with deterministic data.

Tasks:

1. Implement active/complete turn layout, collapse rules, timeline rows, tool expansion, plan pinning, and status summaries.
2. Implement scroll pinning and `N new events` behavior.
3. Implement ordered JSONL persistence, metadata, content-addressed blobs, redaction, and replay.
4. Enforce persist-before-publish semantics in the fake runtime path.
5. Show fidelity labels and distinguish unavailable raw fields from empty values.
6. Add permission, error, cancellation, truncation, and abrupt-disconnect scenarios to the fixture.
7. Add crash-tail recovery and replay tests.

Exit checks:

- A long fake run is observable step by step.
- Expanding a completed turn shows all retained events in order.
- Restart reconstructs equivalent thread/turn state.
- A partial final JSONL line does not destroy earlier history.
- Synthetic secrets are absent from state and rendered diagnostics.
- Scrolling upward never jumps when new events arrive.

### Milestone 4 — OpenCode ACP read-only integration

**Goal:** Replace the fake stream with a real, safe OpenCode session for chat and read-only work.

Tasks:

1. Add the official Rust ACP client dependency verified in Milestone 0.
2. Implement OpenCode subprocess discovery, launch, initialize, capability storage, stderr capture/redaction, shutdown, and crash reporting.
3. Map ACP updates into the existing normalized event model.
4. Implement session creation and cancellation; use list/load/resume only when negotiated.
5. Render unsupported capability states clearly.
6. Keep filesystem writes and terminal execution disabled or rejected.
7. Add a setup/diagnostic surface when OpenCode is missing.

Exit checks:

- The editor remains fully usable with no OpenCode installed.
- A live prompt streams into the same turn UI used by the fake backend.
- Tool details and permissions show only data actually provided.
- Cancellation and harness exit never freeze the editor.
- No write or terminal action can occur through an accidental capability path.

### Milestone 5 — Provider and secret configuration

**Goal:** Configure OpenCode providers without Keychain or inline credential leakage.

Tasks:

1. Implement typed config paths/schema, atomic writes, permissions, warnings, and schema versioning.
2. Implement secret-file create/update flow with masked input and no value echo.
3. Generate deterministic OpenCode JSON containing file references rather than secret values.
4. Add generic OpenAI-compatible configuration first, then OpenAI, Anthropic, Google, Baseten, and Bedrock forms.
5. Add model/provider selection to the agent setup or composer chrome.
6. Show active config sources and project-trust limitations.
7. Test all generation and redaction with an injected temporary root.

Exit checks:

- Required directories/files have the specified permissions.
- Generated OpenCode JSON contains no secret value.
- Generic base URL and model ids round-trip unchanged.
- Bedrock supports profile/region and optional static/session credentials without labelling them one API key.
- Malformed config produces a recoverable UI error and does not overwrite a good file.

### Milestone 6 — Editor context and safe filesystem reads

**Goal:** Let the agent reason about the actual editor state, including unsaved work.

Tasks:

1. Add visible context attachments to the composer.
2. Provide active file, selection, diagnostics, and explicitly included files/ranges.
3. Implement ACP client filesystem reads from open buffers first and disk second.
4. Enforce canonical workspace boundaries and symlink-escape protection.
5. Record exact source, revision/hash, and truncation metadata for reads.
6. Add Unicode, unsaved-buffer, traversal, symlink, and large-file tests.

Exit checks:

- The agent receives unsaved buffer content when reading an open file.
- The user can see and remove context before sending.
- Reads outside the workspace are rejected and audited.
- Context collection and large reads never block the UI thread.

### Milestone 7 — Proposed edits and review experience

**Goal:** Add explicit, conflict-safe file changes and all three review views.

Tasks:

1. Normalize ACP edit proposals into `ChangeSet` records with base identities.
2. Add permission cards and one-time approve/reject decisions.
3. Add conflict detection against current buffer/disk content.
4. Apply approved edits through the editor document model and preserve undo/dirty semantics.
5. Add drawer change summary and per-file activation.
6. Add delayed hover/keyboard diff preview.
7. Add full multi-file `Review in editor` surface.
8. Persist exact decisions, applied result identities, and partial failures.

Exit checks:

- No proposed edit changes a file before approval.
- Changing the buffer after proposal creates a conflict instead of an overwrite.
- Drawer summary, hover preview, and full review agree on the files and hunks.
- Hover preview is cancelled correctly and never traps input.
- A multi-file partial failure reports exact results and leaves an auditable state.

### Milestone 8 — Multiple threads, recovery, and background status

**Goal:** Add useful concurrency/history without permanent clutter.

Tasks:

1. Implement compact searchable thread switcher and pinned/recent groupings.
2. Restore thread metadata and event-derived state at startup.
3. Support background live sessions only within negotiated connection limits.
4. Add waiting/running/failed/complete attention badges without focus stealing.
5. Implement local continuation when remote resume is unavailable, with a clear session-boundary marker.
6. Add explicit thread deletion only if its confirmation and data cleanup are complete; otherwise defer it.

Exit checks:

- Older threads are findable without a permanent sidebar.
- Switching threads preserves scroll, expansion, composer draft, and active-run state.
- Background activity never changes the active editor or drawer thread automatically.
- Restart does not lose completed local audit history.

### Milestone 9 — Hardening and release readiness

**Goal:** Validate the complete initial release and document its limits.

Tasks:

1. Test slow streams, large output, many turns, many changed files, process crashes, disk-full/write failures, and config corruption.
2. Add bounded queues/backpressure where a harness can outpace UI or disk.
3. Measure frame time and memory during streaming and large diff review.
4. Audit all logs and error paths for credential leakage.
5. Document setup, controls, provider configuration, storage paths, trust model, limitations, and recovery.
6. Update `README.md` and `WALKTHROUGH.md` only to describe behavior that actually exists.
7. Decide whether terminal support deserves a separate future plan. Do not slip it into this milestone.

Exit checks:

- Standard offline checks pass.
- Manual OpenCode smoke test passes with at least one native provider and one OpenAI-compatible endpoint when credentials are available.
- Missing/invalid credentials, missing harness, denied permission, harness crash, and persistence failure have actionable UI states.
- No known path bypasses workspace boundaries or approval-gated writes.

## Test strategy

### Unit tests

- Reducer table tests for every turn, tool, permission, cancellation, and terminal transition.
- Duplicate/out-of-order/unknown event handling.
- Summary/action/file counts derived from events.
- Layout constraints, ratio clamping, narrow takeover, splitter hit testing.
- Scroll pin/unread-event logic.
- Hover timer cancellation and focus equivalence.
- Config parsing, migrations, deterministic generation, and redaction.
- Path canonicalization and base-identity conflict detection.
- Blob hashing and event envelope serialization.

### Integration tests

- Fake ACP stdio or channel fixture with fragmented frames and deterministic timing control.
- Runtime command/event ordering and graceful/forced shutdown.
- Persistence replay, partial-tail recovery, corrupt-middle detection, and permission modes.
- Unsaved buffer filesystem responses.
- Proposal → permission → apply/reject/conflict lifecycle.
- Thread switch and restart recovery.

### Manual tests

- Window resize across wide/narrow threshold on standard and Retina displays.
- IME composition and Unicode prompt editing.
- Long-running live OpenCode stream while editing another file.
- Scroll upward during rapid events and return to latest.
- Hover diff near each window edge and in narrow mode.
- OpenCode missing, outdated, misconfigured, killed, or returning unsupported capabilities.
- Provider setup for OpenAI-compatible and each supported native provider form.
- Project containing OpenCode config/plugins to validate the trust warning or isolation behavior.

### Standard repository checks

Run the checks supported by the repository after each milestone, normally:

```text
cargo fmt --check
cargo check
cargo test
cargo clippy --all-targets --all-features -- -D warnings
cargo build --release
```

If an existing unrelated warning prevents strict Clippy, report it with evidence; do not weaken new-agent-code lint quality or silently edit unrelated code.

## Release acceptance checklist

The initial release is complete only when all of these are true:

- [ ] OpenCode connects over ACP without blocking the editor.
- [ ] The panel defaults to 40%, resizes, persists, and takes over on narrow windows.
- [ ] Live turns show streaming text, plan and tool transitions, permissions, errors, and cancellation.
- [ ] Completed summaries can always be expanded into the retained chronological record.
- [ ] Exact, agent-reported, truncated, redacted, and unavailable data are distinguishable.
- [ ] Thread history survives restart, including after a partially written final event.
- [ ] Multiple threads are accessible through a compact switcher without a permanent sidebar.
- [ ] Background threads signal attention without stealing focus.
- [ ] Provider credentials live beneath `~/.config/editor/`, not Keychain.
- [ ] Secrets do not appear in generated OpenCode JSON, audit logs, diagnostics, or test snapshots.
- [ ] OpenAI, Anthropic, Google, Baseten, Bedrock, and generic OpenAI-compatible configuration forms exist.
- [ ] The agent can read unsaved editor buffers and cannot escape the workspace root.
- [ ] File modifications are proposed, reviewable, conflict-checked, and explicitly approved.
- [ ] Changed files have drawer summary, hover/keyboard preview, and full editor review.
- [ ] Missing OpenCode or credentials does not prevent ordinary editor use.
- [ ] Documentation accurately states project configuration trust and first-release limitations.

## Deferred decisions

These do not block the first milestones and should be decided only with implementation evidence:

1. Exact async executor crate and feature set.
2. Exact full-review surface: special tab versus dedicated editor mode.
3. Exact keyboard shortcuts after checking current bindings.
4. Exact ACP raw-frame retention representation supported by the chosen SDK.
5. Whether one OpenCode process can reliably serve all background threads or a bounded process pool is safer.
6. Whether config editing can preserve TOML comments with the selected serialization library.
7. Thread deletion and future retention UX.
8. Terminal support, its permission model, and terminal-output fidelity.

## Upstream references

Use official sources and re-check them when implementing because protocol and harness behavior may evolve:

- ACP protocol overview: <https://agentclientprotocol.com/protocol/v1/overview>
- ACP Rust SDK: <https://docs.rs/agent-client-protocol/latest/agent_client_protocol/>
- OpenCode ACP support: <https://opencode.ai/docs/acp/>
- OpenCode configuration: <https://opencode.ai/docs/config/>
- OpenCode providers: <https://opencode.ai/docs/providers/>

## Recommended first executor prompt

Give the executor this file and ask it to implement **Milestone 0 only**. It should inspect the current worktree first, verify upstream APIs, record concrete decisions, run existing checks, and stop for review. Do not ask it to begin the UI until the runtime boundary, configuration isolation, and capability behavior are proven.
