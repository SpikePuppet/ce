# Agent System Architecture Decisions

Milestone 0 records the upstream checks and implementation decisions needed before adding the editor-side agent UI or runtime code. This file intentionally contains no prompts, credentials, provider responses, generated session IDs, or user-specific home paths.

## Verification Snapshot

- Date verified: 2026-07-13.
- Editor toolchain: Rust 1.92.0, Cargo 1.92.0, package edition 2024.
- Official ACP Rust SDK checked: `agent-client-protocol` 1.2.0.
- Local OpenCode checked: 1.14.46.
- Existing repository checks are not clean before agent work. `cargo fmt --check`, `cargo check`, and `cargo test` currently fail in uncommitted modal, file-tree, and git-panel work already present in the worktree; see "Current check blockers".

## Upstream Facts

- ACP v1 is JSON-RPC. The stdio transport uses UTF-8 JSON-RPC messages delimited by newlines, with no embedded newlines in a message. The client launches the agent subprocess, writes to stdin, reads valid ACP messages from stdout, and may capture stderr logs.
- ACP initialization negotiates `protocolVersion`, client capabilities, agent capabilities, authentication methods, and implementation information.
- Omitted capabilities are unsupported. The editor must not infer filesystem write, terminal, session-list, load, resume, close, or MCP transport support without the initialize response advertising it.
- `session/new` requires an absolute `cwd` and an explicit `mcpServers` list. Session load/resume/list/close/delete behavior is capability-gated.
- OpenCode documents `opencode acp` as the editor command for ACP; it communicates over JSON-RPC via stdio.
- OpenCode config supports JSON/JSONC, `OPENCODE_CONFIG`, per-project config, and `{file:...}` substitution for file-backed secret values.
- OpenCode project config has higher precedence than global and custom config. `OPENCODE_CONFIG` alone is not an isolation boundary against project config.
- OpenCode plugins can be loaded from `.opencode/plugins/`, global plugin directories, or the `plugin` option. The installed `opencode acp --help` also exposes `--pure`, described by OpenCode as running without external plugins.
- OpenCode custom OpenAI-compatible providers use provider entries with an `npm` adapter, display name, `options.baseURL`, models, and optional `apiKey` or headers. Its docs distinguish `@ai-sdk/openai-compatible` for `/v1/chat/completions` from `@ai-sdk/openai` for `/v1/responses`.
- OpenCode Bedrock auth is not a single API key. It supports AWS environment/profile behavior and provider options such as `region`, `profile`, and endpoint.

## Local Probe Results

The throwaway SDK spike lived under `/private/tmp/acp-sdk-check` and was not added to this repository.

- `cargo add agent-client-protocol@1.2.0` resolved successfully with the repository's Rust version.
- `cargo check` succeeded for a minimal binary importing `agent_client_protocol::{Client, Stdio}` and `agent_client_protocol::schema::{ProtocolVersion, v1::InitializeRequest}`.
- `opencode acp --pure --cwd <workspace>` started successfully outside the filesystem sandbox.
- A no-prompt `initialize` request advertising `fs.readTextFile=false`, `fs.writeTextFile=false`, and `terminal=false` returned:
  - `protocolVersion: 1`
  - agent name/version: `OpenCode` / `1.14.46`
  - `loadSession: true`
  - `mcpCapabilities.http: true`
  - `mcpCapabilities.sse: true`
  - `promptCapabilities.embeddedContext: true`
  - `promptCapabilities.image: true`
  - `sessionCapabilities.close`, `fork`, `list`, and `resume`
  - one auth method instructing terminal login through OpenCode
- A no-prompt `session/new` request returned a session id plus initial `configOptions`, `modes`, and `_meta`. The `configOptions` payload can be very large and must be treated as dynamic data, not a fixture or compiled model list.
- Running OpenCode inside the repository sandbox failed before initialize with a SQLite readonly-database error while checkpointing OpenCode state. Production code must expect OpenCode to write its own state during startup/session setup and must surface startup failures distinctly from protocol failures.

## Decisions

### Runtime Boundary

Use a dedicated background owner for agent work. The UI thread sends typed commands to the owner and receives typed `AppEvent::Agent` notifications through winit. The owner is responsible for subprocess lifecycle, ACP I/O, persistence ordering, stderr capture/redaction, and shutdown.

Milestone 1 can use standard threads and channels for the fake backend. The OpenCode ACP adapter in Milestone 4 should use the official SDK on a dedicated background thread. If the SDK path requires async execution, create the async executor inside that background thread and never enter it from UI callbacks.

### Shutdown and Failure

Shutdown is explicit:

1. Stop accepting new mutating commands for the connection.
2. Send capability-supported cancellation/close notifications for active sessions when available.
3. Close stdin or request SDK shutdown.
4. Wait with a bounded timeout.
5. Kill the child if it does not exit.
6. Publish a normalized agent event for clean exit, protocol error, stderr startup failure, crash, timeout, or forced kill.

Panics, child exits, persistence failures, and channel disconnects become visible `AgentEvent`s. They must not unwind through the UI event loop.

### OpenCode Launch

Resolve the binary from explicit editor config first, then from `PATH`. Launch with:

```text
opencode acp --pure --cwd <workspace>
```

Use `--pure` for the first release to reduce plugin trust exposure. Treat it as verified from the local CLI, not yet as a complete project-config isolation guarantee. Before enabling production OpenCode integration, re-check whether OpenCode has a documented way to ignore project config entirely; if not, surface that limitation in diagnostics.

### Capabilities

The editor starts from no optional authority and enables behavior only from the negotiated response. For the initial read-only OpenCode milestone, advertise no terminal and no write capability. Filesystem reads should be introduced only when the editor's workspace boundary and unsaved-buffer bridge are implemented.

Persist each session's negotiated capability snapshot and agent implementation info for later diagnosis.

### Raw Message Capture

Capture raw ACP frames at the process or SDK transport boundary before mapping into domain events. Store them as newline-delimited JSON blobs or blob references after redaction. The domain reducer receives backend-neutral event types; UI state never deserializes OpenCode-specific JSON directly.

If the SDK does not expose raw frames cleanly, wrap the byte-stream transport used to construct the SDK connection rather than depending on SDK-private structures.

### Configuration and Secrets

Store editor-owned agent config beneath the editor config root, not in Keychain. Generate OpenCode config deterministically under the editor config root and prefer `{file:...}` secret references in generated JSON.

Do not put secret values in process arguments, environment variables, generated JSON, logs, fixtures, or diagnostics when OpenCode supports file substitution. Because `OPENCODE_CONFIG` has lower precedence than project config, it can select the generated config but cannot by itself guarantee isolation from project config.

### Fake Backend

Build Milestones 1-3 against an in-process fake backend before the SDK/OpenCode adapter. The fake backend must use the same command/event interfaces as the production adapter and should eventually emit streaming text, tool transitions, permission waits, proposed changes, errors, cancellation, duplicate/late updates, and abrupt disconnects.

## Current Check Blockers

These failures predate Milestone 0 and are unrelated to agent code:

- `cargo fmt --check`: formatting diffs are present in uncommitted `src/git.rs`, `src/git_screen.rs`, `src/main.rs`, `src/modal.rs`, and `src/render.rs` changes.
- `src/modal.rs`: `ModalControl` derives `Copy` while carrying `TextInput(String)`.
- `src/app.rs`: modal scroll call passes a scalar where the modal API now expects `[f32; 2]`.
- `src/app.rs`: modal key handling passes `&Key` where the modal API now expects `&KeyEvent`.
- `src/app.rs`: a `ModalOutcome::Effect(_)` match arm is missing.
- `src/app.rs`: `FileCommand::ToggleGitPanel` is not handled in the file-command match.
- `src/app.rs`: `Application` has partially migrated from `file_tree` to a generic modal field; tests still see many stale `file_tree` references and missing type inference around them.
- `src/render.rs`: test or helper `ModalView` initializers are missing newer fields.
- `src/render.rs`: test or helper `ModalRow` initializers are missing newer fields.

Milestone 1 should not begin until these baseline compile failures are fixed or explicitly accepted as the active integration target.

## Sources Checked

- <https://agentclientprotocol.com/protocol/v1/overview>
- <https://agentclientprotocol.com/protocol/v1/initialization>
- <https://agentclientprotocol.com/protocol/v1/session-setup>
- <https://agentclientprotocol.com/protocol/v1/transports>
- <https://docs.rs/agent-client-protocol/latest/agent_client_protocol/>
- <https://opencode.ai/docs/acp/>
- <https://opencode.ai/docs/config/>
- <https://opencode.ai/docs/providers/>
