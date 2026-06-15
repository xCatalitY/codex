# Claude Code Workflows Source Map

Source workspace: `/home/hannah/Projects/claude-code-patcher`

Primary source files reviewed:

- `@/home/hannah/Projects/claude-code-patcher/cli-runnable.js`
- `@/home/hannah/Projects/claude-code-patcher/modules/3606_s24.js`
- `@/home/hannah/Projects/claude-code-patcher/modules/3617_tt6.js`
- `@/home/hannah/Projects/claude-code-patcher/modules/3600_p24.js`
- `@/home/hannah/Projects/claude-code-patcher/modules/3604_r24.js`
- `@/home/hannah/Projects/claude-code-patcher/modules/3605_a24.js`
- `@/home/hannah/Projects/claude-code-patcher/modules/3580_fW4.js`
- `@/home/hannah/Projects/claude-code-patcher/modules/3575_fZH.js`
- `@/home/hannah/Projects/claude-code-patcher/modules/4518_PM9.js`
- `@/home/hannah/Projects/claude-code-patcher/modules/4520_TM9.js`
- `@/home/hannah/Projects/claude-code-patcher/modules/1445_PD6.js`
- `@/home/hannah/Projects/claude-code-patcher/modules/1446_oc.js`
- `@/home/hannah/Projects/claude-code-patcher/modules/3889_QB4.js`
- `@/home/hannah/Projects/claude-code-patcher/modules/3890_EL.js`
- `@/home/hannah/Projects/claude-code-patcher/modules/3909_Hq.js`

This map separates direct observations from porting inferences. Line references point at the de-bundled module files where possible, with `cli-runnable.js` anchors included for bundle-level correlation.

## Bundle Anchors

The bundle is generated and contains very long lines, so the module files are the better source of truth for detailed behavior. The following `cli-runnable.js` anchors are useful for locating the same implementation in the runnable bundle:

- `@/home/hannah/Projects/claude-code-patcher/cli-runnable.js:3359` contains Workflow tool schema, result schema, validation, permission, and local workflow launch logic.
- `@/home/hannah/Projects/claude-code-patcher/cli-runnable.js:3602` contains the nested `workflow()` helper implementation.
- `@/home/hannah/Projects/claude-code-patcher/cli-runnable.js:3653` contains workflow output fields including `workflowName`, `runId`, `scriptPath`, `transcriptDir`, and `sessionUrl`.
- `@/home/hannah/Projects/claude-code-patcher/cli-runnable.js:3682` begins the Workflow tool prompt: "Execute a workflow script".
- `@/home/hannah/Projects/claude-code-patcher/cli-runnable.js:3687` contains the `ultracode` trigger guidance in the tool prompt.
- `@/home/hannah/Projects/claude-code-patcher/cli-runnable.js:4050` contains workflow progress aggregation and `workflow_agent` rendering helpers.
- `@/home/hannah/Projects/claude-code-patcher/cli-runnable.js:4052` contains `/workflows`, `local_workflow`, and dynamic workflow UI code.
- `@/home/hannah/Projects/claude-code-patcher/cli-runnable.js:6915` maps `workflow_keyword_request` attachments into system reminder text.
- `@/home/hannah/Projects/claude-code-patcher/cli-runnable.js:7480` contains config/help/slash-command wiring that includes dynamic workflow UI settings.
- `@/home/hannah/Projects/claude-code-patcher/cli-runnable.js:9666` contains workflow history/snapshot/runId/scriptPath references.
- `@/home/hannah/Projects/claude-code-patcher/cli-runnable.js:30718` initializes runtime state including `ultracode` and headless options.

## Source Map

| Area | Evidence | Notes |
| --- | --- | --- |
| Workflow tool prompt and contract | `@/home/hannah/Projects/claude-code-patcher/modules/3606_s24.js:8`, `@/home/hannah/Projects/claude-code-patcher/modules/3606_s24.js:12`, `@/home/hannah/Projects/claude-code-patcher/modules/3606_s24.js:21`, `@/home/hannah/Projects/claude-code-patcher/modules/3606_s24.js:34`, `@/home/hannah/Projects/claude-code-patcher/modules/3606_s24.js:50`, `@/home/hannah/Projects/claude-code-patcher/modules/3606_s24.js:58`, `@/home/hannah/Projects/claude-code-patcher/modules/3606_s24.js:66`, `@/home/hannah/Projects/claude-code-patcher/modules/3606_s24.js:86`, `@/home/hannah/Projects/claude-code-patcher/modules/3606_s24.js:166` | Defines the model-facing API and policy for when to invoke workflows. |
| Workflow tool implementation | `@/home/hannah/Projects/claude-code-patcher/modules/3617_tt6.js:1`, `@/home/hannah/Projects/claude-code-patcher/modules/3617_tt6.js:70`, `@/home/hannah/Projects/claude-code-patcher/modules/3617_tt6.js:103`, `@/home/hannah/Projects/claude-code-patcher/modules/3617_tt6.js:175`, `@/home/hannah/Projects/claude-code-patcher/modules/3617_tt6.js:184`, `@/home/hannah/Projects/claude-code-patcher/modules/3617_tt6.js:239`, `@/home/hannah/Projects/claude-code-patcher/modules/3617_tt6.js:288`, `@/home/hannah/Projects/claude-code-patcher/modules/3617_tt6.js:317`, `@/home/hannah/Projects/claude-code-patcher/modules/3617_tt6.js:336`, `@/home/hannah/Projects/claude-code-patcher/modules/3617_tt6.js:366`, `@/home/hannah/Projects/claude-code-patcher/modules/3617_tt6.js:409`, `@/home/hannah/Projects/claude-code-patcher/modules/3617_tt6.js:430`, `@/home/hannah/Projects/claude-code-patcher/modules/3617_tt6.js:515`, `@/home/hannah/Projects/claude-code-patcher/modules/3617_tt6.js:583` | Resolves script/name/path, validates, requests permission, registers a background local workflow task, runs the VM asynchronously, and returns monitor/resume instructions. |
| Local workflow task registry | `@/home/hannah/Projects/claude-code-patcher/modules/3575_fZH.js:13`, `@/home/hannah/Projects/claude-code-patcher/modules/3575_fZH.js:52`, `@/home/hannah/Projects/claude-code-patcher/modules/3575_fZH.js:105`, `@/home/hannah/Projects/claude-code-patcher/modules/3575_fZH.js:122`, `@/home/hannah/Projects/claude-code-patcher/modules/3575_fZH.js:143`, `@/home/hannah/Projects/claude-code-patcher/modules/3575_fZH.js:147`, `@/home/hannah/Projects/claude-code-patcher/modules/3575_fZH.js:155`, `@/home/hannah/Projects/claude-code-patcher/modules/3575_fZH.js:177`, `@/home/hannah/Projects/claude-code-patcher/modules/3575_fZH.js:281` | Owns `local_workflow` task objects, progress, terminal transitions, pause/resume prompt generation, kill/skip/retry, notification, and output file persistence. |
| Workflow VM wrapper | `@/home/hannah/Projects/claude-code-patcher/modules/3605_a24.js:1`, `@/home/hannah/Projects/claude-code-patcher/modules/3605_a24.js:20`, `@/home/hannah/Projects/claude-code-patcher/modules/3605_a24.js:42`, `@/home/hannah/Projects/claude-code-patcher/modules/3605_a24.js:49` | Runs workflow code in a VM, caps recorded logs, races aborts, checks JSON-serializable result, and returns run metrics. |
| VM context and exposed globals | `@/home/hannah/Projects/claude-code-patcher/modules/3604_r24.js:27`, `@/home/hannah/Projects/claude-code-patcher/modules/3604_r24.js:43`, `@/home/hannah/Projects/claude-code-patcher/modules/3604_r24.js:54`, `@/home/hannah/Projects/claude-code-patcher/modules/3604_r24.js:64`, `@/home/hannah/Projects/claude-code-patcher/modules/3604_r24.js:75` | Provides `agent`, `parallel`, `pipeline`, `phase`, `workflow`, `budget`, timers, `args`, `log`, and `console` inside the VM with code generation disabled. |
| Agent orchestration | `@/home/hannah/Projects/claude-code-patcher/modules/3600_p24.js:1`, `@/home/hannah/Projects/claude-code-patcher/modules/3600_p24.js:51`, `@/home/hannah/Projects/claude-code-patcher/modules/3600_p24.js:91`, `@/home/hannah/Projects/claude-code-patcher/modules/3600_p24.js:127`, `@/home/hannah/Projects/claude-code-patcher/modules/3600_p24.js:238`, `@/home/hannah/Projects/claude-code-patcher/modules/3600_p24.js:291`, `@/home/hannah/Projects/claude-code-patcher/modules/3600_p24.js:384`, `@/home/hannah/Projects/claude-code-patcher/modules/3600_p24.js:588`, `@/home/hannah/Projects/claude-code-patcher/modules/3600_p24.js:871`, `@/home/hannah/Projects/claude-code-patcher/modules/3600_p24.js:899`, `@/home/hannah/Projects/claude-code-patcher/modules/3600_p24.js:1026`, `@/home/hannah/Projects/claude-code-patcher/modules/3600_p24.js:1044` | Implements `agent()`, phase grouping, concurrency, token budget, journal cache, local subagent execution, retries, `parallel()`, `pipeline()`, and the built-in workflow subagent definition. |
| Child workflow helper | `@/home/hannah/Projects/claude-code-patcher/modules/3580_fW4.js:22`, `@/home/hannah/Projects/claude-code-patcher/modules/3580_fW4.js:58`, `@/home/hannah/Projects/claude-code-patcher/modules/3580_fW4.js:75` | Lets a workflow call another named/path workflow with `workflow(nameOrRef, args)`, grouped progress, log prefixing, and a one-level nesting limit. |
| Saved/named workflow discovery | `@/home/hannah/Projects/claude-code-patcher/modules/3603_n24.js:1`, `@/home/hannah/Projects/claude-code-patcher/modules/3603_n24.js:16`, `@/home/hannah/Projects/claude-code-patcher/modules/3603_n24.js:28`, `@/home/hannah/Projects/claude-code-patcher/modules/3603_n24.js:42`, `@/home/hannah/Projects/claude-code-patcher/modules/3601_ut6.js:1`, `@/home/hannah/Projects/claude-code-patcher/modules/3601_ut6.js:31`, `@/home/hannah/Projects/claude-code-patcher/modules/3604_r24.js:1` | Loads user, project, plugin, and built-in named workflows. Local workflows shadow plugin and built-in workflows by name. |
| Save dynamic workflow UI/action | `@/home/hannah/Projects/claude-code-patcher/modules/4291_k49.js:1`, `@/home/hannah/Projects/claude-code-patcher/modules/4291_k49.js:20`, `@/home/hannah/Projects/claude-code-patcher/modules/4292_eAq.js:1`, `@/home/hannah/Projects/claude-code-patcher/modules/4292_eAq.js:42`, `@/home/hannah/Projects/claude-code-patcher/modules/4292_eAq.js:84` | Saves dynamic scripts as explicit named workflow files under `.claude/workflows` or `~/.claude/workflows`. |
| `/workflows` command | `@/home/hannah/Projects/claude-code-patcher/modules/4520_TM9.js:1`, `@/home/hannah/Projects/claude-code-patcher/modules/4518_PM9.js:33`, `@/home/hannah/Projects/claude-code-patcher/modules/4518_PM9.js:121`, `@/home/hannah/Projects/claude-code-patcher/modules/4518_PM9.js:150`, `@/home/hannah/Projects/claude-code-patcher/modules/4518_PM9.js:235`, `@/home/hannah/Projects/claude-code-patcher/modules/4518_PM9.js:350` | Defines the `/workflows` slash command and the running/completed workflow browser. |
| Workflow detail/status UI | `@/home/hannah/Projects/claude-code-patcher/modules/4293_cm8.js:1245`, `@/home/hannah/Projects/claude-code-patcher/modules/4293_cm8.js:1320`, `@/home/hannah/Projects/claude-code-patcher/modules/4293_cm8.js:1369`, `@/home/hannah/Projects/claude-code-patcher/modules/4293_cm8.js:1385`, `@/home/hannah/Projects/claude-code-patcher/modules/3616_kZ4.js:1`, `@/home/hannah/Projects/claude-code-patcher/modules/3616_kZ4.js:39`, `@/home/hannah/Projects/claude-code-patcher/modules/3616_kZ4.js:116`, `@/home/hannah/Projects/claude-code-patcher/modules/3616_kZ4.js:170`, `@/home/hannah/Projects/claude-code-patcher/modules/3615_Sy8.js:1`, `@/home/hannah/Projects/claude-code-patcher/modules/3615_Sy8.js:58`, `@/home/hannah/Projects/claude-code-patcher/modules/3615_Sy8.js:91` | Renders tool-use messages, progress, terminal summaries, detail dialog, phase aggregation, selected agent transcript, pause/resume, stop, save, retry, and skip. |
| Background task UI | `@/home/hannah/Projects/claude-code-patcher/modules/4294__zq.js:28`, `@/home/hannah/Projects/claude-code-patcher/modules/4294__zq.js:118`, `@/home/hannah/Projects/claude-code-patcher/modules/4294__zq.js:229`, `@/home/hannah/Projects/claude-code-patcher/modules/4294__zq.js:536`, `@/home/hannah/Projects/claude-code-patcher/modules/4294__zq.js:660`, `@/home/hannah/Projects/claude-code-patcher/modules/4699_R09.js:1`, `@/home/hannah/Projects/claude-code-patcher/modules/3960_AmH.js:62`, `@/home/hannah/Projects/claude-code-patcher/modules/3709_qh8.js:39` | Integrates `local_workflow` into background task lists, footer/status summaries, stop/dismiss behavior, and task kind labels. |
| Footer workflow selection | `@/home/hannah/Projects/claude-code-patcher/modules/4739_fv9.js:237`, `@/home/hannah/Projects/claude-code-patcher/modules/4739_fv9.js:337`, `@/home/hannah/Projects/claude-code-patcher/modules/4739_fv9.js:517`, `@/home/hannah/Projects/claude-code-patcher/modules/4739_fv9.js:1365`, `@/home/hannah/Projects/claude-code-patcher/modules/4739_fv9.js:1489`, `@/home/hannah/Projects/claude-code-patcher/modules/4739_fv9.js:1779` | Adds footer state, keyword notification, `alt+w` suppression toggle, workflow progress preview, and workflow detail overlay selection. |
| Feature gates/settings | `@/home/hannah/Projects/claude-code-patcher/modules/1445_PD6.js:1`, `@/home/hannah/Projects/claude-code-patcher/modules/1446_oc.js:1`, `@/home/hannah/Projects/claude-code-patcher/modules/1446_oc.js:11`, `@/home/hannah/Projects/claude-code-patcher/modules/0584_yu.js:473`, `@/home/hannah/Projects/claude-code-patcher/modules/3993_Zl4.js:305`, `@/home/hannah/Projects/claude-code-patcher/modules/4458_AOq.js:90`, `@/home/hannah/Projects/claude-code-patcher/modules/1447_yA.js:79` | Gates dynamic workflows by env/settings/feature flags/model support, exposes `/config` toggles, and connects `/effort ultracode` with dynamic workflow behavior. |
| Keyword trigger and prompt attachments | `@/home/hannah/Projects/claude-code-patcher/modules/3889_QB4.js:1`, `@/home/hannah/Projects/claude-code-patcher/modules/3889_QB4.js:57`, `@/home/hannah/Projects/claude-code-patcher/modules/3890_EL.js:110`, `@/home/hannah/Projects/claude-code-patcher/modules/3890_EL.js:365`, `@/home/hannah/Projects/claude-code-patcher/modules/3890_EL.js:375`, `@/home/hannah/Projects/claude-code-patcher/modules/3909_Hq.js:3617`, `@/home/hannah/Projects/claude-code-patcher/modules/3909_Hq.js:3625` | Detects `ultracode`, injects workflow keyword request reminders, and injects ultra-effort enter/exit reminders. |
| Task started hooks/transcript events | `@/home/hannah/Projects/claude-code-patcher/modules/3796_Xe.js:1171`, `@/home/hannah/Projects/claude-code-patcher/modules/2965_NF6.js:493`, `@/home/hannah/Projects/claude-code-patcher/modules/2965_NF6.js:2343` | Emits `task_started` system events and exposes task hook/schema metadata including `workflow_name` for local workflows. |
| Workflow snapshots and journal | `@/home/hannah/Projects/claude-code-patcher/modules/3598_It6.js:1`, `@/home/hannah/Projects/claude-code-patcher/modules/3598_It6.js:18`, `@/home/hannah/Projects/claude-code-patcher/modules/3598_It6.js:125`, `@/home/hannah/Projects/claude-code-patcher/modules/3598_It6.js:136` | Stores run snapshots, discovers history for `/workflows`, and stores reusable agent results in `journal.jsonl`. |

## Dynamic vs Explicit Workflows

### Observed: Dynamic Inline Workflows

Dynamic workflows are ad hoc scripts passed to the Workflow tool with the `script` field. The model-facing prompt explicitly frames the Workflow tool as executing background scripts and tells the user to use `/workflows` to monitor them: `@/home/hannah/Projects/claude-code-patcher/modules/3606_s24.js:8`.

The tool schema accepts inline `script`, optional `args`, optional `scriptPath`, and optional `resumeFromRunId`: `@/home/hannah/Projects/claude-code-patcher/modules/3617_tt6.js:70`. Resolution precedence is `scriptPath`, then `name`, then inline `script`: `@/home/hannah/Projects/claude-code-patcher/modules/3617_tt6.js:1`.

Inline scripts must declare `meta` as a pure object literal and then use plain JavaScript with workflow APIs. The prompt documents required `meta`, `agent()` signatures, `args`, `budget`, child `workflow()`, deterministic restrictions, concurrency cap, agent cap, and resume behavior: `@/home/hannah/Projects/claude-code-patcher/modules/3606_s24.js:50`, `@/home/hannah/Projects/claude-code-patcher/modules/3606_s24.js:58`, `@/home/hannah/Projects/claude-code-patcher/modules/3606_s24.js:66`, `@/home/hannah/Projects/claude-code-patcher/modules/3606_s24.js:86`, `@/home/hannah/Projects/claude-code-patcher/modules/3606_s24.js:166`.

Dynamic workflow scripts are persisted under the session directory after invocation. The prompt says every invocation persists under the session directory and should be resumed with `scriptPath`: `@/home/hannah/Projects/claude-code-patcher/modules/3606_s24.js:34`. The implementation obtains a transcript directory and persisted script path before launch: `@/home/hannah/Projects/claude-code-patcher/modules/3617_tt6.js:336`.

### Observed: Explicit Named Workflows

Explicit workflows are `.js` workflow files discoverable from user, project, plugin, or built-in locations. User workflows come from `~/.claude/workflows`; project workflows come from `.claude/workflows`; both are constrained to `.js`, size-capped, and required to parse valid `meta`: `@/home/hannah/Projects/claude-code-patcher/modules/3603_n24.js:1`, `@/home/hannah/Projects/claude-code-patcher/modules/3603_n24.js:16`, `@/home/hannah/Projects/claude-code-patcher/modules/3603_n24.js:28`, `@/home/hannah/Projects/claude-code-patcher/modules/3603_n24.js:42`.

Plugin workflows are loaded from plugin workflow paths and namespaced as `pluginName:workflowName`: `@/home/hannah/Projects/claude-code-patcher/modules/3601_ut6.js:1`, `@/home/hannah/Projects/claude-code-patcher/modules/3601_ut6.js:31`. The merged registry returns built-ins, plugin workflows, and local workflows, with local names suppressing plugin and built-in names: `@/home/hannah/Projects/claude-code-patcher/modules/3604_r24.js:1`.

Named workflows can be invoked by the Workflow tool with `name`, and permission rules can allow or deny by workflow name. The permission path distinguishes named workflows from dynamic scripts and can suggest a local settings allow rule for a named workflow: `@/home/hannah/Projects/claude-code-patcher/modules/3617_tt6.js:239`.

Dynamic workflows can be converted into explicit workflows through the save UI. The save implementation writes under project `.claude/workflows/<slug>.js` or user `~/.claude/workflows/<slug>.js`, uses exclusive write unless overwrite is requested, clears caches, and emits telemetry: `@/home/hannah/Projects/claude-code-patcher/modules/4291_k49.js:1`, `@/home/hannah/Projects/claude-code-patcher/modules/4291_k49.js:20`. The save dialog success text says the saved workflow can be invoked as `/<name>` or `Workflow({name: "<name>"})`: `@/home/hannah/Projects/claude-code-patcher/modules/4292_eAq.js:42`.

### Observed: `/workflows` vs `/workflow`

There is an explicit `/workflows` slash command. Its command definition is `name: "workflows"`, type `local-jsx`, immediate, with description "Browse running and completed workflows": `@/home/hannah/Projects/claude-code-patcher/modules/4520_TM9.js:1`.

I did not observe a built-in singular `/workflow` slash command in the searched workflow source. The singular execution surface is the `Workflow` tool, while saved named workflows are invoked as `/<name>` according to the save dialog text: `@/home/hannah/Projects/claude-code-patcher/modules/4292_eAq.js:42`.

## Trigger Detection and Prompt Injection

### Observed: Explicit Opt-In Rules

The Workflow tool prompt tells the model not to invoke workflows unless the user explicitly opts in. Allowed triggers include the exact keyword `ultracode`, the session being in ultracode mode, direct user request for workflow/multi-agent orchestration, slash command or skill instructions, and named/saved workflow instructions: `@/home/hannah/Projects/claude-code-patcher/modules/3606_s24.js:12`.

The same prompt allows a hybrid approach where the assistant may first do a quick local scout and then launch a workflow if appropriate: `@/home/hannah/Projects/claude-code-patcher/modules/3606_s24.js:21`.

### Observed: Keyword Detection

The keyword detector looks for `ultracode` as a case-insensitive word, rejects prompts that start with `/`, ignores bracketed and quoted regions, and avoids some path or hyphen contexts: `@/home/hannah/Projects/claude-code-patcher/modules/3889_QB4.js:1`. The exported keyword predicate returns true for `ultracode`: `@/home/hannah/Projects/claude-code-patcher/modules/3889_QB4.js:57`.

Prompt context builders add a `workflow_keyword_request` attachment only when workflows are enabled, the prompt is a regular user prompt, suppression is not active, and the keyword trigger setting is enabled: `@/home/hannah/Projects/claude-code-patcher/modules/3890_EL.js:110`, `@/home/hannah/Projects/claude-code-patcher/modules/3890_EL.js:365`.

The UI shows a per-turn notification that a dynamic workflow was requested and exposes `alt+w` to ignore or undo the keyword trigger for the turn: `@/home/hannah/Projects/claude-code-patcher/modules/4739_fv9.js:517`, `@/home/hannah/Projects/claude-code-patcher/modules/4739_fv9.js:1365`.

### Observed: Context Injection

`workflow_keyword_request` becomes a system reminder telling the model that the user included "ultracode", has opted into multi-agent orchestration, and that the model should use the Workflow tool: `@/home/hannah/Projects/claude-code-patcher/modules/3909_Hq.js:3617`.

`ultra_effort_enter` and `ultra_effort_exit` become reminders that either push substantive tasks toward Workflow usage or revert to standard opt-in behavior: `@/home/hannah/Projects/claude-code-patcher/modules/3909_Hq.js:3625`. The attachment generator chooses full, sparse, or exit reminders based on current ultracode session state and prior attachments: `@/home/hannah/Projects/claude-code-patcher/modules/3890_EL.js:375`.

## Workflow Request Lifecycle

### Observed Lifecycle

1. Resolve the requested script. `scriptPath` wins, then `name`, then inline `script`; if none is supplied, validation returns an error: `@/home/hannah/Projects/claude-code-patcher/modules/3617_tt6.js:1`.
2. Validate feature availability. The tool is enabled only if `NP()` is true: `@/home/hannah/Projects/claude-code-patcher/modules/3617_tt6.js:175`.
3. Validate safety and syntax. Validation rejects abort fallback calls, managed/workflow-disabled states, syntax/meta failures, deterministic-regex bans for inline scripts, and attempts to resume a run that is already running: `@/home/hannah/Projects/claude-code-patcher/modules/3617_tt6.js:184`.
4. Request permission. Named workflows check allow/deny/ask rules by workflow name. Dynamic scripts default to asking for review before run: `@/home/hannah/Projects/claude-code-patcher/modules/3617_tt6.js:239`.
5. Allocate run state. The call path parses `meta`, generates or reuses a `wf_...` run id, and allocates a local task id: `@/home/hannah/Projects/claude-code-patcher/modules/3617_tt6.js:288`.
6. Compile before launching. Compile errors are returned as an `async_launched` response with an error payload, but without running the workflow: `@/home/hannah/Projects/claude-code-patcher/modules/3617_tt6.js:317`.
7. Persist script/transcript locations. The call computes transcript/script paths and marks source mode before async execution: `@/home/hannah/Projects/claude-code-patcher/modules/3617_tt6.js:336`.
8. Register a background task. The implementation registers a `local_workflow` task with script, path, args, prompt, summary, workflow name, phases, model, run id, progress counters, abort controller, and agent controllers: `@/home/hannah/Projects/claude-code-patcher/modules/3617_tt6.js:366`, `@/home/hannah/Projects/claude-code-patcher/modules/3575_fZH.js:13`.
9. Run asynchronously. The async closure batches progress every 16 ms and then calls the VM wrapper with run id, progress callback, agent controller callback, args, phase titles, token budget, and journal: `@/home/hannah/Projects/claude-code-patcher/modules/3617_tt6.js:382`, `@/home/hannah/Projects/claude-code-patcher/modules/3617_tt6.js:409`.
10. Snapshot and transition. Completion telemetry is recorded, snapshots are written, and the task is transitioned to killed, failed, or completed: `@/home/hannah/Projects/claude-code-patcher/modules/3617_tt6.js:430`, `@/home/hannah/Projects/claude-code-patcher/modules/3617_tt6.js:515`.
11. Notify. Terminal completion/failure paths enqueue a notification summarizing results, failures, output truncation, usage, and recovery instructions: `@/home/hannah/Projects/claude-code-patcher/modules/3575_fZH.js:177`.
12. Return monitor instructions. The immediate tool result gives the background task id, transcript directory, script path, run id, resume instructions, and `/workflows` monitoring hint: `@/home/hannah/Projects/claude-code-patcher/modules/3617_tt6.js:583`.

### Observed Persistence

Workflow snapshots live under the session workflow directory as `<runId>.json`, and agent transcripts live under `<session>/subagents/workflows/<runId>`: `@/home/hannah/Projects/claude-code-patcher/modules/3598_It6.js:1`. `/workflows` history loads snapshots, normalizes them, and sorts by start time descending: `@/home/hannah/Projects/claude-code-patcher/modules/3598_It6.js:18`.

Agent journal entries are stored as JSON lines in `journal.jsonl` and are keyed by prior key, prompt, and normalized options including schema/model/isolation/agentType: `@/home/hannah/Projects/claude-code-patcher/modules/3598_It6.js:125`, `@/home/hannah/Projects/claude-code-patcher/modules/3598_It6.js:136`.

## Agent Orchestration

### Observed: VM Runtime

The VM context exposes `log`, `phase`, `budget`, `console`, timers, `agent`, `parallel`, `pipeline`, `workflow`, and JSON-cloned `args`: `@/home/hannah/Projects/claude-code-patcher/modules/3604_r24.js:27`. It disables string and wasm code generation: `@/home/hannah/Projects/claude-code-patcher/modules/3604_r24.js:54`.

The top-level VM wrapper records up to 1000 logs, races workflow execution against abort, validates JSON-serializable return values, and reports result, agent count, logs, failures, duration, and errors: `@/home/hannah/Projects/claude-code-patcher/modules/3605_a24.js:1`, `@/home/hannah/Projects/claude-code-patcher/modules/3605_a24.js:20`, `@/home/hannah/Projects/claude-code-patcher/modules/3605_a24.js:42`, `@/home/hannah/Projects/claude-code-patcher/modules/3605_a24.js:49`.

### Observed: `agent()`

The workflow runtime computes a concurrency cap as `min(16, max(2, cpuCount - 2))`: `@/home/hannah/Projects/claude-code-patcher/modules/3600_p24.js:1`. It enforces a hard lifetime cap of 1000 agents and a token budget ceiling: `@/home/hannah/Projects/claude-code-patcher/modules/3600_p24.js:51`, `@/home/hannah/Projects/claude-code-patcher/modules/3600_p24.js:1026`.

`agent()` normalizes schema and options, checks caps and budget, assigns phase/label state, checks the journal cache, emits queued/start/done/error progress events, and routes into local subagent execution: `@/home/hannah/Projects/claude-code-patcher/modules/3600_p24.js:127`, `@/home/hannah/Projects/claude-code-patcher/modules/3600_p24.js:384`.

Custom `agentType` is resolved against active agents and permission rules, then augmented with structured output instructions when a schema is requested: `@/home/hannah/Projects/claude-code-patcher/modules/3600_p24.js:238`. Worktree isolation can create a worktree and append a notice to the subagent prompt: `@/home/hannah/Projects/claude-code-patcher/modules/3600_p24.js:291`.

The subagent loop calls the regular agent runner with `transcriptSubdir: workflows/<runId>`, `spawnedByWorkflowRunId`, a workflow-specific override agent id, optional model, optional worktree path, and constructed prompt messages: `@/home/hannah/Projects/claude-code-patcher/modules/3600_p24.js:384`.

The runtime handles stalled agents, user retry, user skip, API errors, structured output failures, and retry backoff; skipped/API-error agents return null and record failure metadata: `@/home/hannah/Projects/claude-code-patcher/modules/3600_p24.js:588`. Worktrees are cleaned or preserved based on post-run changes: `@/home/hannah/Projects/claude-code-patcher/modules/3600_p24.js:682`.

The default workflow subagent is named `workflow-subagent`, uses all tools, and carries system prompt guidance that final text is the return value and structured output must call the provided structured output tool: `@/home/hannah/Projects/claude-code-patcher/modules/3600_p24.js:1044`.

### Observed: `parallel()`, `pipeline()`, and `workflow()`

`parallel()` runs function thunks concurrently, waits for all settlements, logs failures, and converts budget-exceeded slots to null: `@/home/hannah/Projects/claude-code-patcher/modules/3600_p24.js:871`.

`pipeline()` runs each item independently through sequential stages, waits for all settlements, and logs failures: `@/home/hannah/Projects/claude-code-patcher/modules/3600_p24.js:899`.

Child `workflow(nameOrRef, args)` resolves a named or path workflow, parses/validates its script body, creates a grouped phase named with `> name`, forces child agent calls into that phase, prefixes logs, runs in a child VM context, and rejects deeper nesting: `@/home/hannah/Projects/claude-code-patcher/modules/3580_fW4.js:22`, `@/home/hannah/Projects/claude-code-patcher/modules/3580_fW4.js:58`, `@/home/hannah/Projects/claude-code-patcher/modules/3580_fW4.js:75`.

## UI, Menu, and Status Behavior

### Observed: `/workflows` Browser

`/workflows` loads saved snapshots and live tasks, dedupes them by workflow run id, and sorts newest first: `@/home/hannah/Projects/claude-code-patcher/modules/4518_PM9.js:33`. List mode supports navigation, enter to detail, `x` to stop a running workflow, `s` to save a dynamic workflow when the script is available, and escape/space to close: `@/home/hannah/Projects/claude-code-patcher/modules/4518_PM9.js:121`, `@/home/hannah/Projects/claude-code-patcher/modules/4518_PM9.js:350`.

Detail mode wires workflow controls into the shared workflow detail component: kill workflow, pause workflow, resume from generated prompt, skip agent, retry agent, and save script: `@/home/hannah/Projects/claude-code-patcher/modules/4518_PM9.js:150`. Save mode opens the save dynamic workflow UI: `@/home/hannah/Projects/claude-code-patcher/modules/4518_PM9.js:235`.

### Observed: Tool Message and Detail Views

The Workflow tool-use message displays named workflow labels as "dynamic workflow: name"; otherwise it uses the meta description or first line of the script: `@/home/hannah/Projects/claude-code-patcher/modules/3616_kZ4.js:1`. Running-state UI points users to `/workflows` to monitor/save dynamic workflow runs: `@/home/hannah/Projects/claude-code-patcher/modules/3616_kZ4.js:170`.

Terminal summaries show completed/failed/stopped state, duration, agent count, and token usage: `@/home/hannah/Projects/claude-code-patcher/modules/3616_kZ4.js:116`. Phase aggregation maps workflow progress into phase summaries and falls back to an "Agents" phase: `@/home/hannah/Projects/claude-code-patcher/modules/3615_Sy8.js:58`.

The workflow detail dialog computes phases, displays selected agent transcripts, and supports retry, stop selected agent/workflow, pause/resume, save, and navigation keys: `@/home/hannah/Projects/claude-code-patcher/modules/4293_cm8.js:1245`, `@/home/hannah/Projects/claude-code-patcher/modules/4293_cm8.js:1320`, `@/home/hannah/Projects/claude-code-patcher/modules/4293_cm8.js:1369`, `@/home/hannah/Projects/claude-code-patcher/modules/4293_cm8.js:1385`.

### Observed: Background and Footer Integration

The background task dialog partitions local workflow tasks into a dynamic workflows category and routes them to workflow detail rendering: `@/home/hannah/Projects/claude-code-patcher/modules/4294__zq.js:28`, `@/home/hannah/Projects/claude-code-patcher/modules/4294__zq.js:229`, `@/home/hannah/Projects/claude-code-patcher/modules/4294__zq.js:536`.

Dismiss/kill behavior for `local_workflow` is special-cased so running workflows are killed instead of just dismissed: `@/home/hannah/Projects/claude-code-patcher/modules/4699_R09.js:1`. The task kind label maps local workflows to "workflow": `@/home/hannah/Projects/claude-code-patcher/modules/3960_AmH.js:62`.

Footer state tracks workflow selection and a `workflowFooterIndex`: `@/home/hannah/Projects/claude-code-patcher/modules/4739_fv9.js:237`. The footer can show workflow progress previews and open the workflow detail overlay: `@/home/hannah/Projects/claude-code-patcher/modules/4739_fv9.js:1489`, `@/home/hannah/Projects/claude-code-patcher/modules/4739_fv9.js:1779`.

## Config Flags and Settings

### Observed: Hard Disable and Feature Gate

`K78()` disables workflows when `CLAUDE_CODE_DISABLE_WORKFLOWS` is set or managed/settings include `disableWorkflows: true`: `@/home/hannah/Projects/claude-code-patcher/modules/1445_PD6.js:1`.

`NP()` is the main workflow availability gate. It returns false if workflows are disabled, if the `allow_workflows` feature is unavailable, if environment/backend gates fail, or if workflow settings resolve false: `@/home/hannah/Projects/claude-code-patcher/modules/1446_oc.js:1`.

The same module exposes `workflowKeywordTriggerEnabled`, environment/experiment inputs including `CLAUDE_CODE_WORKFLOWS`, `tengu_workflows_enabled`, and a default-on condition that excludes pro mode: `@/home/hannah/Projects/claude-code-patcher/modules/1446_oc.js:11`.

The settings schema includes `disableWorkflows`, `enableWorkflows`, and `workflowKeywordTriggerEnabled`: `@/home/hannah/Projects/claude-code-patcher/modules/0584_yu.js:473`.

### Observed: User Configuration UI

The `/config` UI includes toggles for "Dynamic workflows" and "Ultracode keyword trigger", writing `enableWorkflows` and `workflowKeywordTriggerEnabled` into user settings: `@/home/hannah/Projects/claude-code-patcher/modules/3993_Zl4.js:305`.

`/effort ultracode` checks dynamic workflow availability and xhigh-capable model support, then applies flag settings with `{ effortLevel: "xhigh", ultracode: true }` and updates runtime state: `@/home/hannah/Projects/claude-code-patcher/modules/4458_AOq.js:90`, `@/home/hannah/Projects/claude-code-patcher/modules/4458_AOq.js:162`.

The ultracode helper requires workflows enabled and xhigh-capable model support; active ultracode mode also requires resolved effort to be xhigh: `@/home/hannah/Projects/claude-code-patcher/modules/1447_yA.js:79`.

## TaskCreate and Hook Semantics

### Observed

I did not observe a literal `TaskCreate` implementation name in the workflow modules. The workflow lifecycle appears to use the local task registry plus transcript/hook events rather than a separate `TaskCreate` API.

The task registry creates a `local_workflow` task via `ms6`: `@/home/hannah/Projects/claude-code-patcher/modules/3575_fZH.js:13`. The transcript layer emits a `task_started` system event with `task_type`, `workflow_name`, and prompt: `@/home/hannah/Projects/claude-code-patcher/modules/3796_Xe.js:1171`.

Hook/schema metadata includes `TaskCreated` and `TaskCompleted` event names and a `task_started` event schema where `workflow_name` is present only for `task_type: "local_workflow"`: `@/home/hannah/Projects/claude-code-patcher/modules/2965_NF6.js:493`, `@/home/hannah/Projects/claude-code-patcher/modules/2965_NF6.js:2343`.

Terminal notifications are emitted through the task notification path with mode `task-notification`: `@/home/hannah/Projects/claude-code-patcher/modules/3575_fZH.js:177`.

## Observed Evidence

- Workflow execution is exposed to the model as a `Workflow` tool, not as a literal `/workflow` slash command: `@/home/hannah/Projects/claude-code-patcher/modules/3617_tt6.js:70`, `@/home/hannah/Projects/claude-code-patcher/modules/4520_TM9.js:1`.
- `/workflows` is a browser for running/completed workflows and is also mentioned in model-facing tool output and UI status copy: `@/home/hannah/Projects/claude-code-patcher/modules/4518_PM9.js:350`, `@/home/hannah/Projects/claude-code-patcher/modules/3617_tt6.js:583`, `@/home/hannah/Projects/claude-code-patcher/modules/3616_kZ4.js:170`.
- Dynamic inline workflows are review-gated, persisted, and run as background `local_workflow` tasks: `@/home/hannah/Projects/claude-code-patcher/modules/3617_tt6.js:239`, `@/home/hannah/Projects/claude-code-patcher/modules/3617_tt6.js:336`, `@/home/hannah/Projects/claude-code-patcher/modules/3575_fZH.js:13`.
- Explicit named workflows are `.js` files with `meta`, loaded from user/project/plugin/built-in sources, and can be invoked by `Workflow({ name })`: `@/home/hannah/Projects/claude-code-patcher/modules/3603_n24.js:1`, `@/home/hannah/Projects/claude-code-patcher/modules/3601_ut6.js:1`, `@/home/hannah/Projects/claude-code-patcher/modules/4292_eAq.js:42`.
- Keyword-based dynamic workflow triggering is implemented through prompt attachments and system reminders rather than directly launching a workflow: `@/home/hannah/Projects/claude-code-patcher/modules/3890_EL.js:365`, `@/home/hannah/Projects/claude-code-patcher/modules/3909_Hq.js:3617`.
- Subagent orchestration is implemented inside the workflow VM through `agent()`, not through the normal user-visible Agent tool surface: `@/home/hannah/Projects/claude-code-patcher/modules/3600_p24.js:127`, `@/home/hannah/Projects/claude-code-patcher/modules/3600_p24.js:384`.
- Workflow state is durable enough for history and resume because scripts, snapshots, transcripts, and journal entries are persisted under the session tree: `@/home/hannah/Projects/claude-code-patcher/modules/3598_It6.js:1`, `@/home/hannah/Projects/claude-code-patcher/modules/3598_It6.js:136`.

## Inferred Behavior and Porting Implications

- Porting needs four coordinated pieces: model-facing Workflow tool prompt/schema, workflow VM/runtime, background `local_workflow` task registry, and `/workflows` UI/history. The source does not treat these as separable.
- `ultracode` should be ported as a prompt/context opt-in mechanism, not as an automatic dispatcher. The observed implementation injects reminders that tell the model to use Workflow; the model still has to call the tool.
- Explicit workflows require a registry and permission layer, because named workflow allow/deny/ask behavior differs from dynamic script review behavior.
- Resume requires preserving `runId`, `scriptPath`, transcript dir, and journal files. A port that only relaunches script text would lose the observed resume/cache behavior.
- The workflow runtime is intentionally constrained: deterministic-script checks, disabled VM code generation, JSON-serializable results, token budget caps, lifetime agent cap, and output item limits all need equivalents in a faithful port.
- The UI model assumes workflows are background tasks with progressive control, not synchronous tool calls. Stop/pause/resume/skip/retry/save controls appear in `/workflows`, the background task dialog, and footer detail overlays.
- The source contains fields and status variants for remote workflows, but the agent orchestration path observed here throws for remote isolation as unavailable in this build. A port should not infer remote workflow support from schema fields alone without verifying the target build path.

## Open Questions / Watchpoints

- `scriptPath` plus `script` is accepted in resolution and returns the given script with the resolved path. Porting should preserve this if resume/edit flows depend on it: `@/home/hannah/Projects/claude-code-patcher/modules/3617_tt6.js:1`.
- Some disallowed tool constants in the built-in `workflow-subagent` definition are minified symbol imports. The surrounding source clearly includes Agent and Workflow among disallowed tools, but symbol-to-display-name mapping should be verified before hard-coding names in a port: `@/home/hannah/Projects/claude-code-patcher/modules/3600_p24.js:1044`.
- The generated bundle line anchors are coarse because many module bodies sit on long bundle lines. Prefer module line references for implementation work and use `cli-runnable.js` anchors only for bundle correlation.
