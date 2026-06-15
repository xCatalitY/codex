# Ultracode and Dynamic Workflows Porting Packet

This directory is an internal implementation packet for porting Claude Code's ultracode and dynamic workflow behavior into Codex. It intentionally lives under `porting/` instead of product docs.

## Artifacts

- `01-claude-ultracode-source-map.md`: Claude Code ultracode source map. Covers the upstream `xhigh` behavior, patch 047's Claude-specific `max` rewrite, the keyword path, and the `/effort ultracode` path.
- `02-claude-workflows-source-map.md`: Claude Code dynamic workflow source map. Covers the `Workflow` tool, JavaScript-like workflow contract, local workflow task registry, runtime helpers, permissions, and `/workflows` browser.
- `03-codex-target-architecture-map.md`: Codex target map. Covers slash commands, reasoning effort handling, collaboration mode settings, prompt/context injection, multi-agent-v2, and the missing runtime/UI pieces.
- `04-codex-implementation-plan.md`: staged implementation plan for adding `/effort ultracode`, workflow mode, keyword-triggered one-turn ultracode, prompt-level workflow guidance, the full `Workflow` tool/runtime, and `/workflows`.
- `05-current-status.md`: current parity checklist and verification log. Start here before continuing implementation work.

## Main Conclusions

- Patched Claude Code treats ultracode as `max` effort plus workflow orchestration, but Codex does not have a valid `max` effort level. The Codex port maps ultracode to `xhigh` plus workflow orchestration.
- Plain `xhigh` effort remains distinct from ultracode. Ultracode needs both `ReasoningEffort::XHigh` and an explicit workflow mode.
- Claude Code has two ultracode entry paths: the persistent session command `/effort ultracode` and a one-turn prompt keyword attachment path.
- Claude Code dynamic workflows are not just instructions. Full parity requires a model-visible `Workflow` tool, script/runtime validation, local run state, cancellation/resume, and a `/workflows` monitor/browser.
- The scanned Claude source exposes `/workflows` as the browser/monitor. A singular `/workflow` command is best used in Codex as a compact mode/status/toggle command, while `/workflows` should own run browsing.
- Codex already has the main foundation for the first slice: custom reasoning efforts, collaboration mode settings/masks, bounded context fragments, slash-command plumbing, and multi-agent-v2.

## Recommended Port Order

1. Add `/effort` with `xhigh` plus `/effort ultracode` session state.
2. Add `WorkflowMode::{Disabled, Dynamic, Ultracode}` to collaboration settings and config.
3. Add bounded workflow instructions and keyword-triggered one-turn ultracode overrides.
4. Reuse multi-agent-v2 for the MVP orchestration path.
5. Add the full `Workflow` tool/runtime behind a feature/config gate.
6. Add `/workflows` run browser/status UI.
7. Harden protocol compatibility, config schema, focused tests, and TUI snapshots.

## Reference Caveats

- `cli-runnable.js` line references are coarse because the runnable is bundled. Prefer the split `modules/*.js` references for source behavior and `patches/047_ultracode_max_effort.py` for patched delta evidence.
- The implementation plan is scoped to ultracode and dynamic workflows only. Memory/dream, resume search, updater, aliases, and release-channel features are intentionally out of scope for this packet.
