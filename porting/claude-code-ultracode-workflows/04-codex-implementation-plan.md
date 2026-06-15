# Codex Implementation Plan: Claude-like Ultracode and Workflows

## Scope

This plan ports the Claude Code ultracode/workflow behavior into Codex as native Rust features, not as a bundle patch. The intended end state is:

- `/effort ultracode` enables a session-scoped mode with `reasoning.effort = "xhigh"` plus standing dynamic-workflow orchestration.
- A prompt keyword can request ultracode for a single turn without permanently changing the session.
- Stage-1 workflow orchestration can use Codex's existing collaboration-mode and multi-agent-v2 surfaces.
- Full parity adds a Codex-native `Workflow` tool/runtime, local workflow run state, and a `/workflows` browser/status surface comparable to Claude Code's implementation.

Out of scope for this plan: permission bypasses, red-team identity, custom updater, generic model alias work, memory/dream workflows, and resume-search improvements. They are source-adjacent in `claude-code-patcher`, but they are not required to make ultracode and dynamic workflows work in Codex.

## Evidence Anchors

Source behavior from `/home/hannah/Projects/claude-code-patcher`:

- Patch 047 defines the Claude patcher behavior as "max effort plus dynamic workflows", exposes `/effort ultracode`, and explicitly keeps it session-scoped: `patches/047_ultracode_max_effort.py:1-10`, `patches/047_ultracode_max_effort.py:17-25`, `patches/047_ultracode_max_effort.py:64-105`.
- Patch 047 gates Claude ultracode on workflows instead of model xhigh support, maps settings/control paths from xhigh to max, maps `/effort ultracode` to `max`, and makes keyword turns submit `effort:"max"` when a workflow keyword attachment is present: `patches/047_ultracode_max_effort.py:30-62`, `patches/047_ultracode_max_effort.py:109-145`, `patches/047_ultracode_max_effort.py:148-181`.
- The Codex port intentionally diverges from that Claude patcher detail: Codex's highest supported effort is `xhigh`, so ultracode maps to `xhigh` plus dynamic workflow orchestration and `/effort max` is rejected.
- Dynamic workflows are implemented as a model-visible `Workflow` tool with JavaScript-like scripts, local run/task state, permission checks, resume support, and a `/workflows` monitor/browser. Source maps are in `02-claude-workflows-source-map.md`.

Target surfaces in `/home/hannah/Projects/temp/codex`:

- Codex exposes `ReasoningEffort::XHigh` as the highest built-in effort and serializes it as `"xhigh"`: `codex-rs/protocol/src/openai_models.rs:37-64`, `codex-rs/protocol/src/openai_models.rs:96-129`.
- The TUI slash command enum has `/model` and `/memories` but no `/effort`, `/workflow`, or `/workflows`. Inline args are explicitly opted in per command: `codex-rs/tui/src/slash_command.rs:7-80`, `codex-rs/tui/src/slash_command.rs:156-174`, `codex-rs/tui/src/slash_command.rs:260-265`.
- Slash dispatch already splits no-arg command handling, inline-arg handling, task-running checks, and prepared inline command submission: `codex-rs/tui/src/chatwidget/slash_dispatch.rs:132-150`, `codex-rs/tui/src/chatwidget/slash_dispatch.rs:541-595`, `codex-rs/tui/src/chatwidget/slash_dispatch.rs:635-714`.
- User turns and turn-context overrides already carry model, effort, and collaboration mode: `codex-rs/tui/src/app_command.rs:42-69`, `codex-rs/tui/src/app_command.rs:158-217`; message submission uses the effective collaboration mode to populate `model`, `effort`, and `collaboration_mode`: `codex-rs/tui/src/chatwidget/input_submission.rs:313-352`.
- Collaboration mode settings currently contain model, reasoning effort, and developer instructions; masks can partially update those fields: `codex-rs/protocol/src/config_types.rs:619-708`.
- TUI state already has non-Plan reasoning setters, effective reasoning resolution, and collaboration-mask updates that emit settings updates: `codex-rs/tui/src/chatwidget/settings.rs:174-193`, `codex-rs/tui/src/chatwidget/settings.rs:463-475`, `codex-rs/tui/src/chatwidget/settings.rs:694-740`.
- The model popup already displays custom reasoning efforts and can apply model+effort without persisting: `codex-rs/tui/src/chatwidget/model_popups.rs:500-532`.
- Config already has root model/reasoning config, nested `[agents]`, nested `[memories]`, and centralized `[features]`: `codex-rs/config/src/config_toml.rs:136-220`, `codex-rs/config/src/config_toml.rs:417-441`, `codex-rs/config/src/config_toml.rs:660-703`.
- Multi-agent-v2 already has default usage hints, tool names, concurrency defaults, and a validation rule that conflicts `agents.max_threads` with `features.multi_agent_v2`: `codex-rs/core/src/config/mod.rs:186-246`, `codex-rs/core/src/config/mod.rs:1303-1339`, `codex-rs/core/src/config/mod.rs:3072-3150`.
- `spawn_agent` already supports role/model/reasoning overrides and hands off to `AgentControl::spawn_agent_with_metadata`: `codex-rs/core/src/tools/handlers/multi_agents_v2/spawn.rs:39-140`; agent control already enforces execution capacity and max thread limits: `codex-rs/core/src/agent/control/spawn.rs:193-235`.
- Repo rules require schema regeneration after config changes, bounded context fragments, integration tests for agent-logic changes, staged changes under roughly 500-800 LoC, and TUI snapshots for UI text/rendering changes: `AGENTS.md:34-67`, `AGENTS.md:86-123`, `AGENTS.md:172-190`.

## Product Semantics

`ultracode` is not just another effort label. It is a compound mode:

1. Reasoning effort is set to `ReasoningEffort::XHigh`.
2. Workflow orchestration instructions are injected while the mode is active.
3. In the MVP, if multi-agent-v2 is enabled, those instructions point at existing `spawn_agent`, `followup_task`, and `send_message` tools. If multi-agent-v2 is disabled, the mode falls back to a single-agent workflow discipline.
4. In the full implementation, Codex exposes a `Workflow` tool that accepts declarative workflow scripts, tracks local workflow tasks, and presents `/workflows` run state. This is the closest match to Claude Code dynamic workflows.
5. Interactive `/effort ultracode` toggles are session-scoped by default and do not rewrite user config.
6. The prompt keyword applies only to the submitted turn.

The important compatibility distinction: `model_reasoning_effort = "xhigh"` remains a plain high-effort setting. Ultracode requires both xhigh effort and `workflow_mode = "ultracode"`.

## Data Structures

### Protocol

Target: `codex-rs/protocol/src/config_types.rs`.

Add an explicit workflow mode to collaboration settings instead of encoding ultracode in `developer_instructions` strings:

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowMode {
    Disabled,
    Dynamic,
    Ultracode,
}

pub struct Settings {
    pub model: String,
    pub reasoning_effort: Option<ReasoningEffort>,
    pub developer_instructions: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_mode: Option<WorkflowMode>,
}

pub struct CollaborationModeMask {
    pub name: String,
    pub mode: Option<ModeKind>,
    pub model: Option<String>,
    pub reasoning_effort: Option<Option<ReasoningEffort>>,
    pub developer_instructions: Option<Option<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_mode: Option<Option<WorkflowMode>>,
}
```

Update `CollaborationMode::with_updates` and `apply_mask` to carry `workflow_mode`. Keep `None` backward-compatible and interpret it as disabled unless a config preset says otherwise. `Dynamic` means workflow instructions/tooling are active without changing effort. `Ultracode` means workflow mode plus xhigh effort. This avoids adding a new `AppCommand::UserTurn` field because `AppCommand` already carries `CollaborationMode` for user turns and overrides.

Do not add `ModeKind::Ultracode` in the first implementation. `ModeKind` is currently the visible Default/Plan axis (`codex-rs/protocol/src/config_types.rs:571-617`); ultracode is better modeled as a workflow overlay on Default/Plan so it can combine with future modes without exploding the enum.

### Core Config

Targets:

- `codex-rs/config/src/config_toml.rs`
- `codex-rs/config/src/types.rs`
- `codex-rs/core/src/config/mod.rs`
- `codex-rs/core/config.schema.json` generated by `just write-config-schema`

Add these config structs:

```rust
pub struct UltracodeToml {
    pub enabled: Option<bool>,
    pub effort: Option<ReasoningEffort>,
    pub keyword_triggers: Option<Vec<String>>,
    pub session_toggle_persists: Option<bool>,
    pub require_workflows: Option<bool>,
}

pub struct WorkflowsToml {
    pub enabled: Option<bool>,
    pub max_instruction_bytes: Option<usize>,
    pub fallback_to_single_agent: Option<bool>,
    pub enable_runtime_tool: Option<bool>,
    pub max_script_bytes: Option<usize>,
    pub max_concurrent_runs: Option<usize>,
    pub workflow_dirs: Option<Vec<PathBuf>>,
}
```

Effective defaults:

- `ultracode.enabled = false`
- `ultracode.effort = "xhigh"`
- `ultracode.keyword_triggers = ["ultracode"]`
- `ultracode.session_toggle_persists = false`
- `ultracode.require_workflows = true`
- `workflows.enabled = true`
- `workflows.max_instruction_bytes = 6000`
- `workflows.fallback_to_single_agent = true`
- `workflows.enable_runtime_tool = false` until the full runtime lands
- `workflows.max_script_bytes = 65536`
- `workflows.max_concurrent_runs = 4`
- `workflows.workflow_dirs = [".codex/workflows"]` plus a global config-home workflows directory if that pattern is already accepted elsewhere in Codex

Keep multi-agent concurrency under existing `[features.multi_agent_v2]` config. Do not introduce another concurrency key because target config already validates multi-agent-v2 separately and rejects `agents.max_threads` when v2 is enabled (`codex-rs/core/src/config/mod.rs:1314-1339`).

## Slash Commands

Targets:

- `codex-rs/tui/src/slash_command.rs`
- `codex-rs/tui/src/chatwidget/slash_dispatch.rs`
- New helper module, for example `codex-rs/tui/src/chatwidget/effort_command.rs`
- New helper module, for example `codex-rs/tui/src/chatwidget/workflow_command.rs`
- Full runtime helper modules, for example `codex-rs/tui/src/chatwidget/workflows_browser.rs` and `codex-rs/core/src/tools/handlers/workflow/*`

Add commands:

| Command | Inline args | During task | Behavior |
|---|---:|---:|---|
| `/effort` | yes | no | No args shows current effort and valid values. With args updates session effort. |
| `/effort ultracode` | yes | no | Sets `reasoning_effort = XHigh`, `workflow_mode = Ultracode`, status label `ultracode - xhigh + workflows`. |
| `/effort max` | yes | no | Rejected with guidance to use `/effort xhigh` or `/effort ultracode`; Codex has no `max` effort. |
| `/effort auto` | yes | no | Clears explicit reasoning effort and disables ultracode workflow mode unless an explicit workflow command keeps it on. |
| `/workflow` | yes | no | Shows workflow mode/status. |
| `/workflow on` | yes | no | Enables `WorkflowMode::Dynamic` without changing effort. |
| `/workflow off` | yes | no | Disables workflow mode without changing effort. |
| `/workflow ultracode` | yes | no | Alias for `/effort ultracode`. |
| `/workflows` | optional | yes | Opens the workflow run browser/status surface once the runtime exists. |

Implementation details:

- Add `SlashCommand::Effort`, `SlashCommand::Workflow`, and `SlashCommand::Workflows`.
- Add `Effort`, `Workflow`, and `Workflows` to `supports_inline_args` as needed.
- Keep `/effort` unavailable during a running task, matching the current `/model` behavior, so an in-flight request cannot be partially retargeted.
- Use `apply_model_and_effort_without_persist` for effort-only changes where possible (`codex-rs/tui/src/chatwidget/model_popups.rs:519-532`), but route ultracode through collaboration-mask updates so workflow mode is included.
- Keep command output short and stateful: "Effort: xhigh", "Ultracode enabled for this session", "Workflow mode disabled".

## Runtime Behavior

### Startup

1. Load `[ultracode]` and `[workflows]` into `Config`.
2. If `ultracode.enabled = true`, initialize the active non-Plan collaboration mode with:
   - `reasoning_effort = Some(XHigh)` or the configured `ultracode.effort`.
   - `workflow_mode = Some(WorkflowMode::Ultracode)`.
3. If `workflows.enabled = false`, ignore configured ultracode workflow mode unless `ultracode.require_workflows = false`; still allow plain xhigh effort.
4. Emit one TUI notice only when an interactive session starts with ultracode active.

### `/effort ultracode`

1. Parse `ultracode` in the new effort command helper.
2. Build a `CollaborationModeMask` over the current non-Plan mode:
   - `reasoning_effort = Some(Some(ReasoningEffort::XHigh))`
   - `workflow_mode = Some(Some(WorkflowMode::Ultracode))`
   - `developer_instructions = None` unless the user already has active mode instructions.
3. Call the existing collaboration update path, so the setting is included in subsequent `UserTurn` commands (`codex-rs/tui/src/chatwidget/settings.rs:694-740`).
4. Do not call `PersistModelSelection` unless a future explicit config option asks to persist interactive effort toggles. Source ultracode text says interactive toggles never persist (`patches/047_ultracode_max_effort.py:71-78`).

### Keyword Turn

1. In `input_submission`, before `AppCommand::user_turn`, inspect normalized visible user text.
2. Trigger only on an exact configured prefix token, for example `ultracode`, `ultracode:`, or `/ultracode` if added later. Avoid substring matches inside prose.
3. For that turn only, clone the effective collaboration mode and apply xhigh effort plus `WorkflowMode::Ultracode`.
4. Do not mutate the user-visible prompt text. The source uses a workflow keyword request attachment and overlays `effort:"max"` in Claude Code (`patches/047_ultracode_max_effort.py:141-145`); Codex models this as a per-turn collaboration override with `xhigh`.

### Workflow Instructions

Targets:

- `codex-rs/core/src/context/workflow_instructions.rs` (new)
- `codex-rs/core/src/context/mod.rs`
- `codex-rs/core/src/session/turn.rs`
- `codex-rs/core/src/session/mod.rs` if a settings snapshot needs plumbing

Add a bounded `WorkflowInstructions` context fragment implementing `ContextualUserFragment`. This follows the repo rule that model-visible injected fragments must be bounded structs in core context (`AGENTS.md:86-93`).

Instruction content:

- State that ultracode means high-thoroughness execution with explicit task decomposition.
- Prefer doing the work directly unless independent subtasks can materially shorten the work.
- When multi-agent-v2 tools are available, use `spawn_agent`, `followup_task`, and `send_message` for bounded subtasks only; this should extend the existing usage hints rather than override them (`codex-rs/core/src/config/mod.rs:195-238`).
- Require final synthesis by the root agent.
- Do not claim extra permissions, sandbox bypasses, or tool availability.

Fragment rules:

- Hard cap with `workflows.max_instruction_bytes`; default 6000 bytes.
- Inject while `workflow_mode == Dynamic` or `workflow_mode == Ultracode`.
- Do not rewrite historical messages.
- Include a stable fragment id so repeated turns do not produce avoidable context cache churn.

### Multi-agent Runtime

Do not add a new automatic task splitter in stage 1. Let the model use existing multi-agent-v2 tools:

- `spawn_agent` already accepts model/reasoning overrides and role names (`codex-rs/core/src/tools/handlers/multi_agents_v2/spawn.rs:39-93`).
- `AgentControl` already enforces execution capacity and max thread limits (`codex-rs/core/src/agent/control/spawn.rs:193-235`).
- V2 descendants already resume separately from legacy v1 descendant recovery (`codex-rs/core/src/agent/control/spawn.rs:522-598`).

If multi-agent-v2 is disabled:

- Keep xhigh effort active.
- Inject single-agent workflow instructions only if `workflows.fallback_to_single_agent = true`.
- Show status as `ultracode - xhigh` or `ultracode - xhigh, workflows unavailable` instead of promising agent orchestration.

### Full Workflow Runtime

The Claude source is more than prompt text. Full parity needs a Codex-native tool/runtime with these responsibilities:

- Expose a model-visible `Workflow` tool when `workflows.enable_runtime_tool = true`.
- Accept inline scripts and named script paths from trusted workflow directories.
- Parse workflow metadata and reject dynamic code-loading/import patterns.
- Provide deterministic helpers for `agent`, `parallel`, `pipeline`, `workflow`, `phase`, `log`, `args`, and `budget`, matching the source contract summarized in `02-claude-workflows-source-map.md`.
- Represent each workflow run as a local task with id, title, phase, progress log, state, cancel/resume affordances, and final result.
- Reuse existing Codex multi-agent-v2 spawn/follow-up/send paths from inside the runtime, so agent limits and permissions remain centralized.
- Ask/review permissions before executing unknown inline workflows; allow explicit config/rules for trusted named workflows.
- Add `/workflows` as the monitor/browser for running, waiting, failed, completed, and resumable workflow runs.

### Request Layer

The request builder already uses the configured effort when building the Responses reasoning block: `codex-rs/core/src/client.rs:713-752`. Add a targeted integration test that a turn in ultracode mode sends `"xhigh"` as the effort value.

Add provider-failure handling only if real tests show the API rejects `"xhigh"` for some model/provider. If implemented, retry once with the previous effective effort or model default and show a non-persistent TUI warning. Do not silently downgrade ultracode in normal code paths.

## File and Module Targets

### Protocol and Config

- `codex-rs/protocol/src/config_types.rs`: add `WorkflowMode`, add `Settings.workflow_mode`, add `CollaborationModeMask.workflow_mode`, update `with_updates` and `apply_mask`, update TS/schema tests.
- `codex-rs/protocol/src/openai_models.rs`: no enum change needed for `xhigh`; it is already a built-in effort.
- `codex-rs/config/src/config_toml.rs`: add `ultracode` and `workflows` root fields near existing user-facing config groups.
- `codex-rs/config/src/types.rs`: add effective config types for ultracode/workflows.
- `codex-rs/core/src/config/mod.rs`: load defaults, validate byte caps/concurrency caps, resolve workflow directories, and expose helpers such as `config.ultracode_effort()` and `config.workflow_mode_enabled()`.
- `codex-rs/core/config.schema.json`: regenerate with `just write-config-schema`.

### TUI

- `codex-rs/tui/src/slash_command.rs`: add command variants and inline-arg support.
- `codex-rs/tui/src/chatwidget/slash_dispatch.rs`: route new commands to helper modules; keep central file changes small.
- `codex-rs/tui/src/chatwidget/effort_command.rs` (new): parse and apply effort values, including `ultracode`.
- `codex-rs/tui/src/chatwidget/workflow_command.rs` (new): parse workflow mode commands and status text.
- `codex-rs/tui/src/chatwidget/input_submission.rs`: add keyword-trigger per-turn override.
- `codex-rs/tui/src/chatwidget/settings.rs`: add `set_workflow_mode`, update effective collaboration mode, and preserve Plan-specific reasoning behavior.
- `codex-rs/tui/src/chatwidget/model_popups.rs`: keep `xhigh` and `ultracode` labels distinct.
- `codex-rs/tui/src/chatwidget/workflows_browser.rs` (new, full runtime): show workflow runs, progress, resume/cancel, and completed output.
- Status/footer modules: show `ultracode - xhigh + workflows` only when workflow mode is active.

### Core Runtime

- `codex-rs/core/src/context/workflow_instructions.rs` (new): bounded fragment.
- `codex-rs/core/src/session/turn.rs`: include workflow fragment when the effective turn collaboration mode asks for it.
- `codex-rs/core/src/session/handlers.rs`: ensure settings-update events include workflow mode in the applied snapshot.
- `codex-rs/core/src/codex_thread.rs`: include workflow mode in settings overrides and snapshots if needed.
- `codex-rs/core/src/tools/handlers/multi_agents_v2/*`: no required stage-1 logic changes; add tests proving ultracode instructions do not bypass existing spawn limits.

### Full Workflow Runtime

- `codex-rs/core/src/tools/handlers/workflow/mod.rs` (new): tool registration, schema, permission decision, call entry point.
- `codex-rs/core/src/tools/handlers/workflow/script.rs` (new): parse/validate workflow scripts or a constrained workflow DSL.
- `codex-rs/core/src/tools/handlers/workflow/runtime.rs` (new): execute phases, helpers, budget, cancellation, and nested workflow guardrails.
- `codex-rs/core/src/tools/handlers/workflow/store.rs` (new): local workflow run state and resumable journal.
- `codex-rs/core/src/tools/handlers/workflow/agent_bridge.rs` (new): call existing multi-agent-v2 control paths, not a second spawner.
- `codex-rs/core/src/tools/mod.rs`: register the `Workflow` tool when enabled.
- `codex-rs/protocol/src/protocol.rs`: add events or state payloads for workflow run progress if existing task events are insufficient.
- `codex-rs/tui/src/chatwidget/workflows_browser.rs`: render the run list/status comparable to Claude `/workflows`.

## Staged Implementation

### Stage 0: Guardrails and Feature Shape

Deliverables:

- Confirm source maps from other agents identify the exact Claude Code workflow prompt text and any non-obvious keyword semantics.
- Add a short design note in the PR body, not in the codebase, that locks the distinction between plain `xhigh` and `ultracode`.
- Add empty feature/config structs only if the first code PR needs them; otherwise avoid placeholder config.

Exit criteria:

- No runtime behavior change.
- Agreement that ultracode is a workflow overlay, not a new `ModeKind`.

### Stage 1: XHigh Effort and `/effort`

Deliverables:

- Add `SlashCommand::Effort`.
- Implement `/effort low|medium|high|xhigh|auto|off`.
- Implement `/effort ultracode` as xhigh effort plus a temporary internal ultracode flag if protocol `WorkflowMode` has not landed yet.
- Ensure `/effort max` is rejected with clear guidance.
- Add TUI status messages and snapshots.

Target files:

- `codex-rs/tui/src/slash_command.rs`
- `codex-rs/tui/src/chatwidget/slash_dispatch.rs`
- `codex-rs/tui/src/chatwidget/effort_command.rs`
- `codex-rs/tui/src/chatwidget/settings.rs`
- `codex-rs/tui/src/chatwidget/model_popups.rs`

Tests:

- Parse known effort values and reject unknown/empty args.
- Snapshot command completions and command output.
- Verify `/effort ultracode` does not persist model selection.

### Stage 2: Workflow Mode State

Deliverables:

- Add `WorkflowMode` to protocol collaboration settings and masks.
- Wire config defaults through `Config`.
- Update settings updates, snapshots, replay, and app-server serialization as needed.
- Add `/workflow on|off|status|ultracode`.
- Move `/effort ultracode` to the durable `WorkflowMode::Ultracode` representation.

Target files:

- `codex-rs/protocol/src/config_types.rs`
- `codex-rs/config/src/config_toml.rs`
- `codex-rs/config/src/types.rs`
- `codex-rs/core/src/config/mod.rs`
- `codex-rs/tui/src/chatwidget/workflow_command.rs`
- `codex-rs/core/config.schema.json`

Tests:

- Round-trip `WorkflowMode` through serde and TypeScript bindings.
- Load config TOML for `[ultracode]` and `[workflows]`.
- Resume old rollouts with no workflow field.
- Snapshot status text for plain xhigh versus ultracode.

### Stage 3: Workflow Instructions and Multi-agent Integration

Deliverables:

- Add `WorkflowInstructions` context fragment.
- Inject it only when workflow mode is active.
- Include multi-agent-v2 guidance only when tools are actually enabled.
- Add single-agent fallback wording.
- Ensure spawned agents inherit appropriate workflow mode only when the root explicitly requests full workflow inheritance. Default child agents should receive task-specific instructions plus existing multi-agent hints.

Target files:

- `codex-rs/core/src/context/workflow_instructions.rs`
- `codex-rs/core/src/context/mod.rs`
- `codex-rs/core/src/session/turn.rs`
- `codex-rs/core/tests/suite/*`

Tests:

- Integration test: ultracode turn includes xhigh effort and workflow fragment.
- Integration test: non-ultracode xhigh effort does not include workflow fragment.
- Integration test: multi-agent-v2 disabled yields fallback text and no spawn-tool claims.
- Context-size test: fragment respects configured cap.

### Stage 4: Prompt Keyword

Deliverables:

- Add keyword trigger parser using `ultracode.keyword_triggers`.
- Apply per-turn collaboration override without changing session state.
- Show a transient TUI note if useful, but avoid adding noisy history entries.

Target files:

- `codex-rs/tui/src/chatwidget/input_submission.rs`
- `codex-rs/tui/src/chatwidget/effort_command.rs`
- Core integration tests for the resulting turn context

Tests:

- `ultracode refactor this` sends xhigh + workflow for one turn.
- Next turn returns to previous effort/workflow state.
- Prose containing `ultracode` mid-sentence does not trigger.

### Stage 5: Full Workflow Tool Runtime

Deliverables:

- Add the model-visible `Workflow` tool behind `workflows.enable_runtime_tool`.
- Add script parsing/validation, deterministic helper surface, local run IDs, and task state.
- Bridge `agent`, `parallel`, and `pipeline` to existing multi-agent-v2 control APIs.
- Add cancel/resume plumbing and a run-state store.
- Keep the MVP prompt-only workflow mode working when the runtime tool is disabled.

Target files:

- `codex-rs/core/src/tools/handlers/workflow/*`
- `codex-rs/core/src/tools/mod.rs`
- `codex-rs/protocol/src/protocol.rs`
- `codex-rs/core/tests/suite/*`

Tests:

- Tool schema accepts script, args, script path, and resume ID according to the selected contract.
- Parser rejects imports, dynamic code loading, non-deterministic APIs, and invalid metadata.
- Runtime enforces max script bytes, max concurrent runs, and nested workflow limits.
- Workflow agent helpers cannot exceed existing multi-agent-v2 capacity/permission limits.
- Cancellation/resume tests cover running, waiting, failed, and completed states.

### Stage 6: `/workflows` Browser

Deliverables:

- Add `/workflows` list/status UI.
- Show active, waiting, failed, completed, and resumable workflow runs.
- Support opening a workflow detail view, resuming where allowed, and cancelling live runs.
- Keep `/workflow` as the compact toggle/status command and `/workflows` as the browser.

Target files:

- `codex-rs/tui/src/slash_command.rs`
- `codex-rs/tui/src/chatwidget/slash_dispatch.rs`
- `codex-rs/tui/src/chatwidget/workflows_browser.rs`
- workflow protocol/event files from Stage 5

Tests:

- TUI snapshots for empty, running, waiting, failed, completed, and detail states.
- Browser can open while a turn is in progress when the data path permits it.
- Resume/cancel actions dispatch the correct workflow runtime event.

### Stage 7: Hardening and Release Gate

Deliverables:

- Update config schema.
- Add any user-facing docs needed for new config keys and commands.
- Audit telemetry/logging for effort and workflow mode without logging prompt contents.
- Verify old sessions, config lock replay, and app-server clients tolerate absent workflow fields.

Exit criteria:

- All focused tests pass.
- TUI snapshots accepted for visible command/status changes.
- Full `just test` run is requested only after focused package tests pass, per repo guidance.

## Testing Plan

Run from `codex-rs` unless noted:

1. Formatting and schema:
   - `just fmt`
   - `just write-config-schema` after config type changes
2. Protocol/config:
   - `just test -p codex-protocol`
   - `just test -p codex-config`
3. Core workflow behavior:
   - `just test -p codex-core`
   - Add integration tests under `core/tests/suite`, because agent-logic changes require integration coverage (`AGENTS.md:104-112`).
4. TUI commands/status:
   - `just test -p codex-tui`
   - Inspect pending `insta` snapshots for `/effort`, `/workflow`, `/workflows`, and status/footer text.
5. Workflow runtime:
   - Add and run focused workflow runtime tests in the crate that owns `codex-rs/core/src/tools/handlers/workflow/*`.
6. Lints:
   - `just fix -p codex-tui`, `just fix -p codex-core`, or the scoped crates touched, after implementation is otherwise stable.

Do not run raw `cargo test`; repo instructions require `just test` (`AGENTS.md:61-67`).

## Risks and Mitigations

| Risk | Mitigation |
|---|---|
| `xhigh` is rejected by a provider/model. | Add request-level regression tests and only add one-shot fallback after a real provider rejection is observed. Surface any downgrade clearly. |
| Ultracode context causes cache churn or context bloat. | Inject a stable, bounded `WorkflowInstructions` fragment only while workflow mode is active; cap by `workflows.max_instruction_bytes`; follow context rules in `AGENTS.md:86-93`. |
| Workflow mode becomes confused with Plan mode. | Model ultracode as `WorkflowMode`, not `ModeKind`; preserve Plan-specific effort handling in `settings.rs`. |
| Multi-agent-v2 limits are bypassed. | Reuse existing `spawn_agent` and `AgentControl` capacity paths; do not add a separate spawner. |
| Interactive toggles unexpectedly persist. | Default `ultracode.session_toggle_persists = false`; `/effort ultracode` uses session collaboration-mask updates, not config writes. |
| Workflow runtime creates a second agent scheduler. | Route workflow agent helpers through existing multi-agent-v2 control APIs and tests, and keep limits/permissions centralized. |
| Workflow scripts become an unrestricted code-execution surface. | Use a constrained runtime/parser, deny imports/dynamic code loading, cap script size, and ask/review unknown inline workflows. |
| `/workflow` and `/workflows` semantics blur. | Keep `/workflow` as mode/status/toggle and `/workflows` as the browser for actual runs. |
| Protocol/app-server clients break on new fields. | Use optional serde-default fields, add backward compatibility tests, and verify old rollout replay. |
| The change grows too large for review. | Land stages independently and keep central TUI/core files thin by adding helper modules, following `AGENTS.md:45-58` and `AGENTS.md:117-123`. |

## Definition of Done

- `/effort ultracode` reliably sets xhigh effort plus workflow mode for the current session.
- `/effort xhigh` remains plain xhigh effort without workflow instructions.
- The `ultracode` keyword applies xhigh effort plus workflow mode to one turn only.
- Workflow instructions are bounded, visible in tests, and do not claim unavailable tools.
- Multi-agent-v2 orchestration works through existing tools and limits.
- The `Workflow` tool can run, cancel, resume, and report local workflow runs when enabled.
- `/workflow` controls workflow mode and `/workflows` exposes run state.
- Config schema, focused tests, and TUI snapshots are updated for every user-facing surface.
