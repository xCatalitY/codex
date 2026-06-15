# Codex Target Architecture Map: Ultracode And Workflows

This map covers the Rust crates and TUI surfaces most relevant to porting Claude-like
ultracode and workflow behavior into the target Codex repo. The key conclusion is that
Codex already has strong primitives for model/effort selection, skills, prompt/context
fragments, and subagent spawning, but it does not currently have a first-class workflow
registry or workflow execution state machine.

The best porting shape is a small workflow layer that composes existing mechanisms:

- TUI slash command entry points for activation and discovery.
- Config schema fields for workflow roots/profiles and optional ultracode defaults.
- Prompt/context injection through the extension/context-fragment path.
- Existing model/reasoning events for mode selection.
- Existing multi-agent tools and roles for delegated work, initially prompt-driven.

Avoid treating "ultracode" as only a model alias or only a reasoning-effort string.
The existing reasoning schema can carry custom values, but a Claude-like mode also
needs instructions, model/effort preferences, workflow state, and TUI affordances.

## Read First Constraint

The repo-level `AGENTS.md` says new model-visible context must be incremental, bounded,
and implemented as structured context fragments when injected into user-visible context.
It also says broad new concepts should avoid `codex-core` unless they genuinely need to
live there. That shapes the recommended MVP: put workflow parsing/catalog state outside
core where possible, and touch core only at config/session/context integration points.

## Current Codex Capabilities

### Slash Command Parsing And Dispatch

Current:

- Slash commands are a static Rust enum. `SlashCommand` is the root built-in command
  set, and its declaration order controls popup presentation order:
  `@/home/hannah/Projects/temp/codex/codex-rs/tui/src/slash_command.rs:7` and
  `@/home/hannah/Projects/temp/codex/codex-rs/tui/src/slash_command.rs:13`.
- Existing commands already include `/model`, `/skills`, `/plan`, `/goal`,
  `/agent`/`/subagents`, `/side`/`/btw`, `/apps`, and `/plugins`:
  `@/home/hannah/Projects/temp/codex/codex-rs/tui/src/slash_command.rs:82`.
- Only a whitelist supports inline arguments. `/plan`, `/goal`, `/side`, `/btw`,
  `/review`, and several utility commands are allowed; most built-ins are popup-only:
  `@/home/hannah/Projects/temp/codex/codex-rs/tui/src/slash_command.rs:156`.
- Commands have task-running availability rules:
  `@/home/hannah/Projects/temp/codex/codex-rs/tui/src/slash_command.rs:189`.
- The popup layer currently represents slash options as either a built-in command or
  a service-tier command, not arbitrary workflow entries:
  `@/home/hannah/Projects/temp/codex/codex-rs/tui/src/bottom_pane/slash_commands.rs:21`.
- Feature flags and side-conversation state filter visible commands:
  `@/home/hannah/Projects/temp/codex/codex-rs/tui/src/bottom_pane/slash_commands.rs:56`
  and
  `@/home/hannah/Projects/temp/codex/codex-rs/tui/src/bottom_pane/slash_commands.rs:70`.
- Service tiers are the one dynamic-ish slash item class and are inserted after
  `/model`, which is a useful precedent for adding workflow-derived items:
  `@/home/hannah/Projects/temp/codex/codex-rs/tui/src/bottom_pane/slash_commands.rs:86`.
- Chat composer input classifies queued text as slash, shell, or plain user input:
  `@/home/hannah/Projects/temp/codex/codex-rs/tui/src/bottom_pane/chat_composer/slash_input.rs:200`.
- Popup selection returns either `InputResult::Command` or service-tier command;
  tab completion has special behavior only for `/skills`:
  `@/home/hannah/Projects/temp/codex/codex-rs/tui/src/bottom_pane/chat_composer/slash_input.rs:211`
  and
  `@/home/hannah/Projects/temp/codex/codex-rs/tui/src/bottom_pane/chat_composer/slash_input.rs:489`.
- Slash dispatch is centralized in `chatwidget/slash_dispatch.rs`; it gates
  side-conversation/task-running behavior, then matches each command:
  `@/home/hannah/Projects/temp/codex/codex-rs/tui/src/chatwidget/slash_dispatch.rs:132`
  and
  `@/home/hannah/Projects/temp/codex/codex-rs/tui/src/chatwidget/slash_dispatch.rs:150`.
- Dispatch examples: `/model` opens the model popup, `/plan` applies Plan mode,
  `/goal` opens goal controls, `/agent` opens agent controls, and `/skills` opens
  the skills menu:
  `@/home/hannah/Projects/temp/codex/codex-rs/tui/src/chatwidget/slash_dispatch.rs:262`,
  `@/home/hannah/Projects/temp/codex/codex-rs/tui/src/chatwidget/slash_dispatch.rs:284`,
  `@/home/hannah/Projects/temp/codex/codex-rs/tui/src/chatwidget/slash_dispatch.rs:287`,
  `@/home/hannah/Projects/temp/codex/codex-rs/tui/src/chatwidget/slash_dispatch.rs:305`,
  and
  `@/home/hannah/Projects/temp/codex/codex-rs/tui/src/chatwidget/slash_dispatch.rs:425`.
- Inline commands have a separate parser and handler. `/plan <args>` applies Plan
  mode and submits the argument as the user message; `/goal <args>` either sets or
  controls durable goal state:
  `@/home/hannah/Projects/temp/codex/codex-rs/tui/src/chatwidget/slash_dispatch.rs:541`,
  `@/home/hannah/Projects/temp/codex/codex-rs/tui/src/chatwidget/slash_dispatch.rs:635`,
  `@/home/hannah/Projects/temp/codex/codex-rs/tui/src/chatwidget/slash_dispatch.rs:694`,
  and
  `@/home/hannah/Projects/temp/codex/codex-rs/tui/src/chatwidget/slash_dispatch.rs:715`.

Missing:

- There is no user-defined slash command registry.
- There is no workflow slash item type beside built-ins and service tiers.
- Adding a new command today means touching the enum, popup filtering, dispatch, and
  tests.
- Inline argument behavior is opt-in per command, so workflow commands need explicit
  parser and completion design.

Best integration point:

- Add a small `/workflow` command plus optional `/ultracode` alias first. If workflow
  names should appear directly in the slash popup, extend `SlashCommandItem` with a
  workflow variant using the service-tier insertion pattern as the precedent.

### Model, Reasoning, And Ultracode Controls

Current:

- TUI events already separate updating the active model, reasoning effort, and
  persisted model selection:
  `@/home/hannah/Projects/temp/codex/codex-rs/tui/src/app_event.rs:650`.
- Popup events exist for reasoning selection, Plan-mode reasoning scope, and all-model
  selection:
  `@/home/hannah/Projects/temp/codex/codex-rs/tui/src/app_event.rs:703`.
- Plan-mode reasoning has its own update/persist events:
  `@/home/hannah/Projects/temp/codex/codex-rs/tui/src/app_event.rs:830`.
- `/model` loads presets from the TUI model catalog and is disabled until the session
  is configured:
  `@/home/hannah/Projects/temp/codex/codex-rs/tui/src/chatwidget/model_popups.rs:8`.
- The full picker selects both model and effort:
  `@/home/hannah/Projects/temp/codex/codex-rs/tui/src/chatwidget/model_popups.rs:170`.
- Selection actions can update model, reasoning effort, persist settings, or prompt for
  Plan-mode scope:
  `@/home/hannah/Projects/temp/codex/codex-rs/tui/src/chatwidget/model_popups.rs:216`
  and
  `@/home/hannah/Projects/temp/codex/codex-rs/tui/src/chatwidget/model_popups.rs:239`.
- The reasoning popup supports advertised model efforts, high-effort warnings, and
  persistence:
  `@/home/hannah/Projects/temp/codex/codex-rs/tui/src/chatwidget/model_popups.rs:350`.
- Reasoning labels already include `Minimal`, `Low`, `Medium`, `High`, `XHigh`, and
  `Custom`:
  `@/home/hannah/Projects/temp/codex/codex-rs/tui/src/chatwidget/model_popups.rs:500`.
- Keyboard shortcuts adjust the active model's supported effort list, with separate
  behavior for Plan mode:
  `@/home/hannah/Projects/temp/codex/codex-rs/tui/src/chatwidget/reasoning_shortcuts.rs:41`.
- Collaboration mode changes send an `override_turn_context` command with the current
  effective mode:
  `@/home/hannah/Projects/temp/codex/codex-rs/tui/src/chatwidget/settings.rs:720`.
- Protocol reasoning effort is an open enum with `Custom(String)` and tests covering
  custom values:
  `@/home/hannah/Projects/temp/codex/codex-rs/protocol/src/openai_models.rs:37`,
  `@/home/hannah/Projects/temp/codex/codex-rs/protocol/src/openai_models.rs:115`, and
  `@/home/hannah/Projects/temp/codex/codex-rs/protocol/src/openai_models.rs:698`.
- Model metadata carries default/supported reasoning, base instructions, tool mode, and
  multi-agent version:
  `@/home/hannah/Projects/temp/codex/codex-rs/protocol/src/openai_models.rs:194`
  and
  `@/home/hannah/Projects/temp/codex/codex-rs/protocol/src/openai_models.rs:346`.
- Backend model metadata can become picker-ready presets:
  `@/home/hannah/Projects/temp/codex/codex-rs/protocol/src/openai_models.rs:553`.
- The model manager returns picker models and resolves default model candidates:
  `@/home/hannah/Projects/temp/codex/codex-rs/models-manager/src/manager.rs:78`
  and
  `@/home/hannah/Projects/temp/codex/codex-rs/models-manager/src/manager.rs:108`.
- Per-model config overrides can alter reasoning summary support, context window, tool
  output limits, base instructions, and personality metadata:
  `@/home/hannah/Projects/temp/codex/codex-rs/models-manager/src/model_info.rs:23`.

Missing:

- There is no semantic "ultracode" mode that bundles model, effort, prompts, context
  policy, and workflow behavior.
- There is no workflow-specific model/effort profile.
- There is no first-class UI affordance for "temporary mode for this turn/thread" beyond
  existing model/effort/Plan controls.

Best integration point:

- For MVP, model ultracode as a workflow profile that may request an existing model and
  reasoning effort through `AppEvent::UpdateModel` and `AppEvent::UpdateReasoningEffort`.
  Only add a new model-catalog entry if the UX truly needs it to appear in `/model`.

### Config Schema And Runtime Config

Current:

- User-defined model providers already live in config TOML:
  `@/home/hannah/Projects/temp/codex/codex-rs/config/src/config_toml.rs:270`.
- Reasoning and model verbosity fields already exist in `ConfigToml`:
  `@/home/hannah/Projects/temp/codex/codex-rs/config/src/config_toml.rs:328`.
- `agents`, `memories`, and `skills` are already top-level config concepts:
  `@/home/hannah/Projects/temp/codex/codex-rs/config/src/config_toml.rs:417`.
- Agent config supports max thread/depth/runtime caps and inline/declared roles:
  `@/home/hannah/Projects/temp/codex/codex-rs/config/src/config_toml.rs:659`.
- Runtime `Config` carries model/service tier, model provider, instruction flags,
  agent limits/roles, and reasoning settings:
  `@/home/hannah/Projects/temp/codex/codex-rs/core/src/config/mod.rs:587`,
  `@/home/hannah/Projects/temp/codex/codex-rs/core/src/config/mod.rs:597`,
  `@/home/hannah/Projects/temp/codex/codex-rs/core/src/config/mod.rs:655`,
  `@/home/hannah/Projects/temp/codex/codex-rs/core/src/config/mod.rs:667`,
  `@/home/hannah/Projects/temp/codex/codex-rs/core/src/config/mod.rs:829`, and
  `@/home/hannah/Projects/temp/codex/codex-rs/core/src/config/mod.rs:908`.
- Config validation already enforces agent limits and runtime bounds:
  `@/home/hannah/Projects/temp/codex/codex-rs/core/src/config/mod.rs:3109`.
- Config construction resolves model/service tier, instruction inclusion flags, skills,
  apps, collaboration mode, and environment context:
  `@/home/hannah/Projects/temp/codex/codex-rs/core/src/config/mod.rs:3198`,
  `@/home/hannah/Projects/temp/codex/codex-rs/core/src/config/mod.rs:3224`, and
  `@/home/hannah/Projects/temp/codex/codex-rs/core/src/config/mod.rs:3415`.
- JSON schema generation is driven by `ConfigToml`:
  `@/home/hannah/Projects/temp/codex/codex-rs/config/src/schema.rs:118` and
  `@/home/hannah/Projects/temp/codex/codex-rs/core/src/bin/config_schema.rs:5`.

Missing:

- No workflow config table or manifest root exists.
- No config-backed slash alias mechanism exists.
- No ultracode profile exists that can bundle model, reasoning effort, injected
  instructions, and enabled workflow features.

Best integration point:

- Add a minimal `[workflows]` config section only after deciding the runtime shape.
  The first useful fields are roots/enabled, default workflow, and an `ultracode`
  profile with optional model and reasoning effort. Any schema change must update the
  generated schema with `just write-config-schema`.

### Prompt And Context Injection

Current:

- Model-visible user-context fragments use the `ContextualUserFragment` trait:
  `@/home/hannah/Projects/temp/codex/codex-rs/context-fragments/src/fragment.rs:37`.
- Fragments own role/body/markers and render into response input items:
  `@/home/hannah/Projects/temp/codex/codex-rs/context-fragments/src/fragment.rs:65`
  and
  `@/home/hannah/Projects/temp/codex/codex-rs/context-fragments/src/fragment.rs:100`.
- Core context exports the existing fragment types, including skills, permissions,
  collaboration mode, environment context, and user instructions:
  `@/home/hannah/Projects/temp/codex/codex-rs/core/src/context/mod.rs:1` and
  `@/home/hannah/Projects/temp/codex/codex-rs/core/src/context/mod.rs:31`.
- Registered contextual user fragments are enumerated centrally:
  `@/home/hannah/Projects/temp/codex/codex-rs/core/src/context/contextual_user_message.rs:46`.
- The session picks base instructions from config, conversation history, or current
  model instructions, then derives collaboration mode from model and reasoning effort:
  `@/home/hannah/Projects/temp/codex/codex-rs/core/src/session/mod.rs:571` and
  `@/home/hannah/Projects/temp/codex/codex-rs/core/src/session/mod.rs:595`.
- `SessionConfiguration` carries provider, collaboration mode, reasoning summary,
  developer instructions, and base instructions:
  `@/home/hannah/Projects/temp/codex/codex-rs/core/src/session/mod.rs:610`.
- `build_initial_context` is the primary aggregation point for developer and contextual
  user instructions:
  `@/home/hannah/Projects/temp/codex/codex-rs/core/src/session/mod.rs:2816`.
- That function already inserts permissions, developer instructions, collaboration
  mode, personality, apps, skills, plugins, extension prompt fragments, user
  instructions, token budget, environment context, multi-agent usage hints, and
  guardian prompts:
  `@/home/hannah/Projects/temp/codex/codex-rs/core/src/session/mod.rs:2849`,
  `@/home/hannah/Projects/temp/codex/codex-rs/core/src/session/mod.rs:2868`,
  `@/home/hannah/Projects/temp/codex/codex-rs/core/src/session/mod.rs:2878`,
  `@/home/hannah/Projects/temp/codex/codex-rs/core/src/session/mod.rs:2892`,
  `@/home/hannah/Projects/temp/codex/codex-rs/core/src/session/mod.rs:2909`,
  `@/home/hannah/Projects/temp/codex/codex-rs/core/src/session/mod.rs:2923`,
  `@/home/hannah/Projects/temp/codex/codex-rs/core/src/session/mod.rs:2946`,
  `@/home/hannah/Projects/temp/codex/codex-rs/core/src/session/mod.rs:2956`,
  `@/home/hannah/Projects/temp/codex/codex-rs/core/src/session/mod.rs:2978`,
  `@/home/hannah/Projects/temp/codex/codex-rs/core/src/session/mod.rs:2988`,
  `@/home/hannah/Projects/temp/codex/codex-rs/core/src/session/mod.rs:3000`,
  `@/home/hannah/Projects/temp/codex/codex-rs/core/src/session/mod.rs:3014`, and
  `@/home/hannah/Projects/temp/codex/codex-rs/core/src/session/mod.rs:3043`.
- Context-window renewal reinjects initial context and persists compacted replacement:
  `@/home/hannah/Projects/temp/codex/codex-rs/core/src/session/mod.rs:3088`.
- Context update recording avoids reinjecting full context when a baseline exists:
  `@/home/hannah/Projects/temp/codex/codex-rs/core/src/session/mod.rs:3126`.
- Turn execution builds skills/plugins and records context updates before sampling:
  `@/home/hannah/Projects/temp/codex/codex-rs/core/src/session/turn.rs:135`.
- Sampling sends cloned history as prompt input:
  `@/home/hannah/Projects/temp/codex/codex-rs/core/src/session/turn.rs:216`.
- Responses requests separate base instructions, formatted input, tools, reasoning,
  service tier, and parallel-tool settings:
  `@/home/hannah/Projects/temp/codex/codex-rs/core/src/client.rs:720` and
  `@/home/hannah/Projects/temp/codex/codex-rs/core/src/client.rs:737`.

Missing:

- No `WorkflowInstructions` or `UltracodeInstructions` fragment exists.
- No lifecycle exists for workflow activation, workflow update, workflow completion,
  or workflow cancellation.
- There is no current thread/turn workflow state that can be reflected into context
  incrementally.

Best integration point:

- Prefer an extension-style prompt contributor for MVP, because `build_initial_context`
  already accepts extension prompt fragments at
  `@/home/hannah/Projects/temp/codex/codex-rs/core/src/session/mod.rs:2956`.
  Add a core contextual user fragment only if the workflow state must be represented
  as a registered contextual user message or diffed across context windows.

### Skills And Workflow-Like Prompt Injection

Current:

- Legacy core skills can install/uninstall bundled system skills, cache skills by
  effective config, load roots, and filter disabled skills:
  `@/home/hannah/Projects/temp/codex/codex-rs/core-skills/src/manager.rs:51`,
  `@/home/hannah/Projects/temp/codex/codex-rs/core-skills/src/manager.rs:97`,
  `@/home/hannah/Projects/temp/codex/codex-rs/core-skills/src/manager.rs:124`, and
  `@/home/hannah/Projects/temp/codex/codex-rs/core-skills/src/manager.rs:180`.
- Skill injection finds explicit skill mentions and injects selected `SKILL.md`
  prompts:
  `@/home/hannah/Projects/temp/codex/codex-rs/core-skills/src/injection.rs:58` and
  `@/home/hannah/Projects/temp/codex/codex-rs/core-skills/src/injection.rs:146`.
- Skill instructions are model-visible user fragments with `<skill>` markers:
  `@/home/hannah/Projects/temp/codex/codex-rs/core-skills/src/skill_instructions.rs:22`.
- Session turn setup resolves mentioned skills/plugins and injects them:
  `@/home/hannah/Projects/temp/codex/codex-rs/core/src/session/turn.rs:520`.
- Available skills are a developer fragment:
  `@/home/hannah/Projects/temp/codex/codex-rs/core/src/context/available_skills_instructions.rs:34`.
- The newer skills extension initializes thread state from config and selected roots:
  `@/home/hannah/Projects/temp/codex/codex-rs/ext/skills/src/extension.rs:52`.
- It contributes available-skill context, exposes tools when providers exist, builds a
  catalog with host/bundled/orchestrator skills, and injects selected skill prompts:
  `@/home/hannah/Projects/temp/codex/codex-rs/ext/skills/src/extension.rs:91`,
  `@/home/hannah/Projects/temp/codex/codex-rs/ext/skills/src/extension.rs:133`,
  `@/home/hannah/Projects/temp/codex/codex-rs/ext/skills/src/extension.rs:153`,
  `@/home/hannah/Projects/temp/codex/codex-rs/ext/skills/src/extension.rs:185`, and
  `@/home/hannah/Projects/temp/codex/codex-rs/ext/skills/src/extension.rs:198`.
- Orchestrator-owned skills are discovered/read via MCP resources with explicit
  byte/time/page caps:
  `@/home/hannah/Projects/temp/codex/codex-rs/ext/skills/src/provider/orchestrator.rs:24`,
  `@/home/hannah/Projects/temp/codex/codex-rs/ext/skills/src/provider/orchestrator.rs:49`, and
  `@/home/hannah/Projects/temp/codex/codex-rs/ext/skills/src/provider/orchestrator.rs:152`.
- The skills extension has tests proving host-loaded skill catalog injection and
  selected entrypoint injection:
  `@/home/hannah/Projects/temp/codex/codex-rs/ext/skills/tests/skills_extension.rs:54`
  and
  `@/home/hannah/Projects/temp/codex/codex-rs/ext/skills/tests/skills_extension.rs:140`.

Missing:

- Skills are prompt packages, not workflow runners.
- There is no step/checkpoint/state model.
- There is no workflow auto-trigger or user-confirmed activation path.
- There is no mechanism for a workflow to declare required subagent roles or model
  profiles beyond what the prompt asks the model to do.

Best integration point:

- Reuse skills for reusable knowledge and tactics, but do not overload `SKILL.md` as the
  workflow runtime. A workflow layer can reference skills, inject its own bounded
  workflow instructions, and use existing skills injection for supporting material.

### Subagent And Task Orchestration

Current:

- Default multi-agent v2 guidance already exists in config:
  `@/home/hannah/Projects/temp/codex/codex-rs/core/src/config/mod.rs:195`.
- Agent caps and roles are config-backed:
  `@/home/hannah/Projects/temp/codex/codex-rs/config/src/config_toml.rs:659`.
- Agent roles load from config layers and `.codex/agents`, with inline or config-file
  declarations:
  `@/home/hannah/Projects/temp/codex/codex-rs/core/src/config/agent_roles.rs:19`
  and
  `@/home/hannah/Projects/temp/codex/codex-rs/core/src/config/agent_roles.rs:146`.
- Role application is separate from orchestration decisions; the multi-agent tool
  handler owns when a spawn happens:
  `@/home/hannah/Projects/temp/codex/codex-rs/core/src/agent/role.rs:1`.
- Role configs can preserve current provider/tier unless the role overrides them:
  `@/home/hannah/Projects/temp/codex/codex-rs/core/src/agent/role.rs:56`.
- Spawn-agent tool descriptions include available roles and annotate locked
  model/reasoning/service-tier settings:
  `@/home/hannah/Projects/temp/codex/codex-rs/core/src/agent/role.rs:217`
  and
  `@/home/hannah/Projects/temp/codex/codex-rs/core/src/agent/role.rs:250`.
- Agent control has a spawn thread API and internal capacity/depth/residency handling:
  `@/home/hannah/Projects/temp/codex/codex-rs/core/src/agent/control/spawn.rs:82`
  and
  `@/home/hannah/Projects/temp/codex/codex-rs/core/src/agent/control/spawn.rs:193`.
- Multi-agent v1 and v2 tool specs exist; v2 exposes direct `spawn_agent` with
  `task_name` and `message`:
  `@/home/hannah/Projects/temp/codex/codex-rs/core/src/tools/handlers/multi_agents_spec.rs:11`,
  `@/home/hannah/Projects/temp/codex/codex-rs/core/src/tools/handlers/multi_agents_spec.rs:48`, and
  `@/home/hannah/Projects/temp/codex/codex-rs/core/src/tools/handlers/multi_agents_spec.rs:80`.
- The v1 guidance says to use subagents only when the user explicitly asks for
  sub-agents, delegation, or parallel work:
  `@/home/hannah/Projects/temp/codex/codex-rs/core/src/tools/handlers/multi_agents_spec.rs:660`.
- The v2 description frames subagents as concrete bounded subtasks with canonical task
  paths and the same tools as the root agent:
  `@/home/hannah/Projects/temp/codex/codex-rs/core/src/tools/handlers/multi_agents_spec.rs:718`.
- The v2 handler parses role/model/reasoning overrides, applies role config, creates
  task paths, and emits subagent-started events:
  `@/home/hannah/Projects/temp/codex/codex-rs/core/src/tools/handlers/multi_agents_v2/spawn.rs:39`,
  `@/home/hannah/Projects/temp/codex/codex-rs/core/src/tools/handlers/multi_agents_v2/spawn.rs:64`,
  `@/home/hannah/Projects/temp/codex/codex-rs/core/src/tools/handlers/multi_agents_v2/spawn.rs:95`, and
  `@/home/hannah/Projects/temp/codex/codex-rs/core/src/tools/handlers/multi_agents_v2/spawn.rs:147`.
- There is also a CSV job tool that spawns one worker subagent per row, with worker
  reporting:
  `@/home/hannah/Projects/temp/codex/codex-rs/core/src/tools/handlers/agent_jobs_spec.rs:6`,
  `@/home/hannah/Projects/temp/codex/codex-rs/core/src/tools/handlers/agent_jobs_spec.rs:74`,
  `@/home/hannah/Projects/temp/codex/codex-rs/core/src/tools/handlers/agent_jobs/spawn_agents_on_csv.rs:64`, and
  `@/home/hannah/Projects/temp/codex/codex-rs/core/src/tools/handlers/agent_jobs/spawn_agents_on_csv.rs:153`.
- Agent max-thread behavior has tests:
  `@/home/hannah/Projects/temp/codex/codex-rs/core/src/agent/control_tests.rs:1575`.

Missing:

- Workflows cannot declaratively spawn or require subagents.
- Workflows cannot define agent-role bundles, task templates, or dependency graphs.
- Host-level orchestration is not exposed as a workflow runtime; current behavior is
  model/tool driven.

Best integration point:

- For MVP, keep workflow orchestration prompt-driven: the workflow instructions can tell
  the root model when to use existing `spawn_agent` tools, while agent roles remain
  configured through existing config. Defer host-driven subagent spawning until the
  workflow state model and user-consent semantics are clear.

## Missing Pieces For Claude-Like Ultracode And Workflows

1. Workflow registry:
   A catalog of workflow definitions, roots, aliases, descriptions, and activation
   metadata. Current slash commands and skills do not provide this.

2. Workflow state:
   Thread/turn state for active workflow, current phase, selected profile, and completion
   or cancellation. Existing `/goal` has durable goal semantics, but not generic workflow
   phases.

3. Ultracode semantic profile:
   A named profile that can bundle model preference, reasoning effort, prompt additions,
   tool/subagent guidance, and maybe display state. Existing `ReasoningEffort::Custom`
   can represent custom effort values but not the mode as a whole.

4. Workflow prompt fragments:
   Bounded model-visible instructions for active workflows, ideally through extension
   prompt fragments first. A core `WorkflowInstructions` fragment is only needed if
   workflow context must participate in the contextual user message registry.

5. Slash UX:
   `/workflow` and `/ultracode` activation, workflow discovery/completion, task-running
   availability, inline args, and queued slash behavior.

6. Tests and snapshots:
   New tests must cover parser behavior, config schema, context injection, model/effort
   side effects, and TUI snapshots for popup/disabled states.

## Minimal Viable Port Surface

The minimal viable port should add a workflow activation path without inventing a
host-level workflow engine on day one.

### MVP Files To Touch

Slash command and TUI:

- `@/home/hannah/Projects/temp/codex/codex-rs/tui/src/slash_command.rs:7`
  Add `/workflow` and, if desired, `/ultracode` as an alias or separate command. Update
  descriptions, inline-arg support, and task-running availability.
- `@/home/hannah/Projects/temp/codex/codex-rs/tui/src/bottom_pane/slash_commands.rs:21`
  If workflow names should show in the popup, add a workflow item variant beside built-in
  and service-tier items.
- `@/home/hannah/Projects/temp/codex/codex-rs/tui/src/bottom_pane/chat_composer/slash_input.rs:211`
  Add completion/argument behavior for `/workflow <name>` and `/ultracode`.
- `@/home/hannah/Projects/temp/codex/codex-rs/tui/src/chatwidget/slash_dispatch.rs:150`
  Dispatch workflow activation, cancellation, and status.
- `@/home/hannah/Projects/temp/codex/codex-rs/tui/src/app_event.rs:650`
  Reuse existing model/effort events when an ultracode profile requests them.

Config:

- `@/home/hannah/Projects/temp/codex/codex-rs/config/src/config_toml.rs:417`
  Add a `[workflows]` table near other user capability concepts.
- `@/home/hannah/Projects/temp/codex/codex-rs/core/src/config/mod.rs:587`
  Resolve workflow config into runtime config only as far as TUI/session code needs.
- `@/home/hannah/Projects/temp/codex/codex-rs/config/src/schema.rs:118`
  Regenerate schema after config changes with `just write-config-schema`.

Workflow implementation:

- Prefer a new crate or extension module such as `codex-rs/ext/workflows` for catalog
  loading, activation state, and prompt contribution. This matches the repo instruction
  to avoid stuffing broad new concepts into `codex-core`.
- Use extension prompt fragments through
  `@/home/hannah/Projects/temp/codex/codex-rs/core/src/session/mod.rs:2956`
  for the first port.
- If core fragments become necessary, implement a bounded fragment using
  `@/home/hannah/Projects/temp/codex/codex-rs/context-fragments/src/fragment.rs:37`
  and register it near
  `@/home/hannah/Projects/temp/codex/codex-rs/core/src/context/contextual_user_message.rs:46`.

Model/reasoning:

- Reuse model popup/state paths instead of introducing a new model mechanism:
  `@/home/hannah/Projects/temp/codex/codex-rs/tui/src/chatwidget/model_popups.rs:216`.
- Use existing `ReasoningEffort` values or `Custom(String)` where supported:
  `@/home/hannah/Projects/temp/codex/codex-rs/protocol/src/openai_models.rs:37`.
- Do not add an "ultracode" model preset unless it is an actual backend/catalog model or
  the UX requires it as a visible profile.

Subagents:

- Do not directly spawn subagents from the workflow runtime in the first port. Instead,
  inject workflow guidance that can use existing `spawn_agent` tools:
  `@/home/hannah/Projects/temp/codex/codex-rs/core/src/tools/handlers/multi_agents_spec.rs:718`.
- Use existing agent role config for workflow-recommended roles:
  `@/home/hannah/Projects/temp/codex/codex-rs/core/src/config/agent_roles.rs:19`.

### MVP Behavior

1. `/workflow <name>` activates a workflow for the current thread or next turn.
2. `/ultracode [task]` activates the ultracode workflow/profile and optionally submits
   the task text as the user message, following the existing `/plan <args>` pattern at
   `@/home/hannah/Projects/temp/codex/codex-rs/tui/src/chatwidget/slash_dispatch.rs:694`.
3. Activation stores workflow state in an extension/thread store or small runtime state
   object.
4. The active workflow contributes a bounded developer or user-context fragment at the
   same aggregation layer used by skills/extensions.
5. If the workflow profile has model/effort preferences, TUI dispatch reuses the existing
   model/effort events instead of bypassing settings.
6. Workflows may reference existing skills by name, but selected skill prompt injection
   remains owned by the skills system.
7. Workflows may recommend agent roles, but spawning remains model/tool driven in MVP.

### Explicit Non-MVP Work

- A declarative workflow state machine with host-driven step execution.
- Automatic subagent spawning from workflow manifests.
- Dynamic user-defined slash aliases for every workflow.
- New tool namespaces for workflow execution.
- Deep changes to `codex-core` request construction.
- A new model catalog entry unless the backend/catalog actually has a corresponding
  model or product decision.

## Test Map

Existing test surfaces to extend:

- Slash enum and command availability unit tests:
  `@/home/hannah/Projects/temp/codex/codex-rs/tui/src/slash_command.rs:268`.
- Slash popup filtering, feature flags, service tiers, and side conversation tests:
  `@/home/hannah/Projects/temp/codex/codex-rs/tui/src/bottom_pane/slash_commands.rs:155`.
- Slash completion tests:
  `@/home/hannah/Projects/temp/codex/codex-rs/tui/src/bottom_pane/chat_composer/slash_input.rs:571`.
- Disabled command while task running snapshot:
  `@/home/hannah/Projects/temp/codex/codex-rs/tui/src/chatwidget/tests/exec_flow.rs:1047`.
- Reasoning effort custom-value tests:
  `@/home/hannah/Projects/temp/codex/codex-rs/protocol/src/openai_models.rs:698`.
- Extension prompt-fragment tests:
  `@/home/hannah/Projects/temp/codex/codex-rs/core/src/session/tests.rs:7573`.
- Multi-agent usage-hint tests:
  `@/home/hannah/Projects/temp/codex/codex-rs/core/src/session/tests.rs:7611`.
- Skills extension injection tests:
  `@/home/hannah/Projects/temp/codex/codex-rs/ext/skills/tests/skills_extension.rs:54`.
- Agent thread-limit tests:
  `@/home/hannah/Projects/temp/codex/codex-rs/core/src/agent/control_tests.rs:1575`.

New tests for the port:

- `/workflow` and `/ultracode` enum aliases, descriptions, inline-arg policy, and
  availability during active tasks.
- Popup filtering and completion for workflow names.
- Dispatch behavior for activation, cancellation, status, and optional immediate task
  submission.
- Config TOML deserialization, validation, and generated JSON schema.
- Context injection with no active workflow, active workflow, workflow profile changes,
  and context-window reinjection.
- Model/effort event emission when ultracode profile requests a model or effort.
- Skills interaction: workflow references a skill without duplicating the skills
  injection path.
- Subagent guidance: workflow prompt recommends existing roles without bypassing
  `agents.max_threads` or `agents.max_depth`.

## Recommended First Implementation Slice

1. Add a `codex-rs/ext/workflows` crate or module with:
   - workflow catalog structs,
   - a minimal built-in `ultracode` profile,
   - thread state for active workflow,
   - bounded prompt-fragment contribution.

2. Add `/workflow` and `/ultracode` to the TUI:
   - `/workflow` lists/activates named workflows,
   - `/ultracode [task]` activates the built-in ultracode profile and optionally submits
     a task.

3. Add minimal config:
   - workflow roots,
   - enabled flag,
   - optional default profile,
   - optional ultracode model and reasoning effort.

4. Reuse existing model/effort plumbing:
   - do not fork model picker logic,
   - do not add a new reasoning enum unless backend support requires it.

5. Keep orchestration prompt-driven:
   - workflows can instruct the model to use existing skills and subagents,
   - host-driven workflow steps and automatic agent spawning come later.

This slice creates a real user-visible port surface while keeping the blast radius small:
TUI command activation, config/schema, bounded context injection, and tests.
