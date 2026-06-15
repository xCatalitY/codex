# Claude Code Ultracode Source Map

## Scope

This source map covers Claude Code upstream `2.1.170` and the local patcher implementation that rewires Ultracode behavior through patch 047.

- Upstream package version is `2.1.170`: @/home/hannah/Projects/claude-code-patcher/claude-code-package/package.json:3
- Upstream bundled CLI identifies as `2.1.170`: @/home/hannah/Projects/claude-code-patcher/claude-code-package/cli.js:2
- Local patched runnable bundle is based on `2.1.170`: @/home/hannah/Projects/claude-code-patcher/cli-runnable.js:2
- Local patched bundle carries patched version metadata `2.1.170-patched.48`: @/home/hannah/Projects/claude-code-patcher/cli-runnable.js:4
- Patch 047 is explicitly targeted at `2.1.170`: @/home/hannah/Projects/claude-code-patcher/patches/047_ultracode_max_effort.py:15
- Patch 047 depends on patch 002: @/home/hannah/Projects/claude-code-patcher/patches/047_ultracode_max_effort.py:25
- Patch 002 is the persistence/schema patch for `max` effort: @/home/hannah/Projects/claude-code-patcher/patches/002_permanent_max_effort.py:1

## Executive Map

Claude Code has two Ultracode entry points:

1. Literal keyword `ultracode` in a user prompt.
2. Session mode via `/effort ultracode` or `settings.ultracode`.

In upstream `2.1.170`, both are workflow-coupled, but only session mode is effort-coupled:

- The literal keyword is detected only when workflows and the keyword trigger are enabled, then converted into a `workflow_keyword_request` attachment: @/home/hannah/Projects/claude-code-patcher/modules/3890_EL.js:116
- The upstream prompt construction sends that attachment through, but does not add a per-turn effort override for the keyword path: @/home/hannah/Projects/claude-code-patcher/modules/4785_KXq.js:303
- Upstream `/effort ultracode` maps to `xhigh`: @/home/hannah/Projects/claude-code-patcher/modules/4458_AOq.js:32
- Upstream Ultracode availability is gated by workflow availability and `xhigh` model support: @/home/hannah/Projects/claude-code-patcher/modules/1447_yA.js:76

Patch 047 changes the local patched bundle so Ultracode maps to `max`:

- Patch 047 rewrites Ultracode availability from `NP() && xhigh-capable model` to workflow-only `NP()`: @/home/hannah/Projects/claude-code-patcher/patches/047_ultracode_max_effort.py:32
- Patch 047 rewrites active Ultracode state from resolved `xhigh` to raw/session `max` with compatible environment override handling: @/home/hannah/Projects/claude-code-patcher/patches/047_ultracode_max_effort.py:37
- Patch 047 rewrites `/effort ultracode` from `xhigh` to `max`: @/home/hannah/Projects/claude-code-patcher/patches/047_ultracode_max_effort.py:110
- Patch 047 adds a literal-keyword per-turn `effort:"max"` override when the prompt has a `workflow_keyword_request` attachment: @/home/hannah/Projects/claude-code-patcher/patches/047_ultracode_max_effort.py:139
- The patched runnable contains that literal-keyword per-turn override: @/home/hannah/Projects/claude-code-patcher/cli-runnable.js:11331

## Literal Keyword `ultracode`

### 1. UI highlighting is workflow-gated and keyword-trigger-gated

The prompt input highlighter only marks `ultracode` when workflows are enabled and the keyword trigger is enabled:

- The highlighter asks `NP() && f78()` before applying keyword highlights: @/home/hannah/Projects/claude-code-patcher/modules/4739_fv9.js:332
- `NP()` is the effective workflow-enabled check: @/home/hannah/Projects/claude-code-patcher/modules/1446_oc.js:1
- `f78()` reads `settings.workflowKeywordTriggerEnabled`, defaulting to true: @/home/hannah/Projects/claude-code-patcher/modules/1446_oc.js:16
- The keyword-trigger setting description explicitly names `ultracode`: @/home/hannah/Projects/claude-code-patcher/modules/0584_yu.js:483

### 2. Keyword parsing is literal-word matching with exclusions

The keyword scanner is a word-boundary parser, not a substring search:

- `tKq` builds a word-boundary regex for the requested token: @/home/hannah/Projects/claude-code-patcher/modules/3889_QB4.js:1
- It skips slash-start input, so `/effort ultracode` is not handled by this keyword scanner: @/home/hannah/Projects/claude-code-patcher/modules/3889_QB4.js:3
- It ignores quoted text, code spans, HTML-ish spans, path-like suffixes, hyphenated/question/dotted words, and embedded identifier contexts: @/home/hannah/Projects/claude-code-patcher/modules/3889_QB4.js:4
- `eKq` binds the scanner to the literal `ultracode`: @/home/hannah/Projects/claude-code-patcher/modules/3889_QB4.js:51
- `uB4` exposes the boolean "does this prompt contain ultracode" result: @/home/hannah/Projects/claude-code-patcher/modules/3889_QB4.js:57

### 3. Prompt attachments convert the keyword into workflow opt-in

The attachment builder only checks keyword workflow requests on regular user prompts, only on the main thread, and only when workflows are enabled:

- Attachment generation is guarded by `NP()` before workflow keyword and ultra-effort attachments are considered: @/home/hannah/Projects/claude-code-patcher/modules/3890_EL.js:112
- `workflow_keyword_request` is added only for regular user prompts, when keyword suppression is absent and `f78()` allows the trigger: @/home/hannah/Projects/claude-code-patcher/modules/3890_EL.js:116
- The keyword scan uses `preExpansionInput` when available, otherwise the current input text: @/home/hannah/Projects/claude-code-patcher/modules/3890_EL.js:119
- `ULf` emits telemetry `tengu_workflow_keyword` and returns `{type:"workflow_keyword_request"}` only when `uB4` finds `ultracode`: @/home/hannah/Projects/claude-code-patcher/modules/3890_EL.js:364
- `workflow_keyword_request` is an allowed attachment type: @/home/hannah/Projects/claude-code-patcher/modules/4213_cfq.js:24

### 4. The attachment tells the model to use the Workflow tool

The prompt-materialization layer turns `workflow_keyword_request` into a system reminder:

- `workflow_keyword_request` maps to text saying the user included keyword `ultracode` and wants the assistant to use the Workflow tool: @/home/hannah/Projects/claude-code-patcher/modules/3909_Hq.js:3617
- The Workflow tool prompt defines keyword `ultracode` as explicit opt-in to workflow use: @/home/hannah/Projects/claude-code-patcher/modules/3606_s24.js:8
- The Workflow tool prompt separately defines "Ultracode" standing behavior: workflow for substantive tasks, multi-phase workflows, adversarial verification, and solo handling for trivial/conversational tasks: @/home/hannah/Projects/claude-code-patcher/modules/3606_s24.js:32
- The Workflow tool itself is enabled only when `NP()` is true: @/home/hannah/Projects/claude-code-patcher/modules/3617_tt6.js:170
- Workflow tool validation returns managed-disabled and unavailable errors when workflow gates fail: @/home/hannah/Projects/claude-code-patcher/modules/3617_tt6.js:191

### 5. Upstream keyword path does not force effort

In upstream `2.1.170`, the user prompt construction passes through the attachment set and passes a normal resolved effort value to `Dk9`; it does not special-case `workflow_keyword_request` into `max` or `xhigh`:

- `Dk9` constructs the user prompt message and records effort telemetry only when an effort argument is passed: @/home/hannah/Projects/claude-code-patcher/modules/4782_Jk9.js:1
- Upstream prompt construction calls `Dk9` with `_T(mainLoopModel, MO(K))`, then wraps with `Ng8`: @/home/hannah/Projects/claude-code-patcher/modules/4785_KXq.js:303
- Upstream bundled equivalent has no `workflow_keyword_request` effort override at the prompt construction site: @/home/hannah/Projects/claude-code-patcher/claude-code-package/cli.js:11329

Upstream result: literal `ultracode` is a workflow opt-in attachment. It does not itself activate session Ultracode and does not force per-turn `xhigh` or `max`.

### 6. Patch 047 makes the keyword force `max` for that turn

Patch 047 adds a prompt-construction special case:

- Patch 047 replaces the `Dk9(...)` call with an object spread that adds `{effort:"max"}` when attachments include `workflow_keyword_request`: @/home/hannah/Projects/claude-code-patcher/patches/047_ultracode_max_effort.py:139
- The patched runnable contains `c.some((n)=>n.attachment.type==="workflow_keyword_request")&&{effort:"max"}` at the prompt construction site: @/home/hannah/Projects/claude-code-patcher/cli-runnable.js:11331

Patched result: literal `ultracode` is both a workflow opt-in and a per-turn `max` effort request. It is still not the same thing as setting session `ultracode:true`.

## `/effort ultracode`

### 1. Upstream command availability and parser

The `/effort` command advertises `ultracode` only when `cu(w7())` says the current model can use upstream Ultracode:

- Help text appends `|ultracode` only when `cu(w7())` is true: @/home/hannah/Projects/claude-code-patcher/modules/4458_AOq.js:13
- Command argument hints append `ultracode` only when `cu(w7())` is true: @/home/hannah/Projects/claude-code-patcher/modules/4460_LY9.js:7
- The parser maps argument `ultracode` to `{value:"xhigh"}` when `cu(model)` is true: @/home/hannah/Projects/claude-code-patcher/modules/4458_AOq.js:32

### 2. Upstream command execution gates

The upstream `bdf` handler for `/effort ultracode` has two explicit gates:

- If `cu()` is false, it reports workflows must be enabled: @/home/hannah/Projects/claude-code-patcher/modules/4458_AOq.js:118
- If `cu(w7())` is false, it tells the user to switch to an `xhigh`-capable model: @/home/hannah/Projects/claude-code-patcher/modules/4458_AOq.js:119

Then upstream applies `xhigh`:

- Remote-control apply uses `KOq("xhigh", true)`: @/home/hannah/Projects/claude-code-patcher/modules/4458_AOq.js:122
- The command warns if `CLAUDE_CODE_EFFORT_LEVEL` overrides with a value other than `xhigh`: @/home/hannah/Projects/claude-code-patcher/modules/4458_AOq.js:124
- The app-state update is `{type:"effortUpdate", value:"xhigh", ultracode:true}`: @/home/hannah/Projects/claude-code-patcher/modules/4458_AOq.js:135
- The app-state reducer writes `effortValue` and `ultracode` from the update: @/home/hannah/Projects/claude-code-patcher/modules/4458_AOq.js:167
- Noninteractive command execution applies the same `effortUpdate` state change: @/home/hannah/Projects/claude-code-patcher/modules/4459_JY9.js:17

### 3. Patch 047 command changes

Patch 047 changes `/effort ultracode` from `xhigh` to `max`:

- Parser replacement maps `ultracode` to `{value:"max"}`: @/home/hannah/Projects/claude-code-patcher/patches/047_ultracode_max_effort.py:110
- The explicit `xhigh` model gate is removed by replacing it with an empty string: @/home/hannah/Projects/claude-code-patcher/patches/047_ultracode_max_effort.py:116
- Remote-control apply is changed from `KOq("xhigh", true)` to `KOq("max", true)`: @/home/hannah/Projects/claude-code-patcher/patches/047_ultracode_max_effort.py:123
- The environment-override check changes from `q!=="xhigh"` to `q!=="max"`: @/home/hannah/Projects/claude-code-patcher/patches/047_ultracode_max_effort.py:128
- The command's state update changes to `{value:"max", ultracode:true}`: @/home/hannah/Projects/claude-code-patcher/patches/047_ultracode_max_effort.py:134
- The patched runnable contains the command parser, remote apply, environment check, status text, and state update changes on the minified `/effort` command line: @/home/hannah/Projects/claude-code-patcher/cli-runnable.js:9141

Patched result: `/effort ultracode` requires workflows but no longer requires `xhigh` model support. It requests `max`.

## Settings and Config Interactions

### Workflow settings

Workflow availability is centralized in `NP()`:

- `NP()` returns false if workflows are disabled by env/managed setting, policy, or feature availability: @/home/hannah/Projects/claude-code-patcher/modules/1446_oc.js:1
- `NP()` otherwise returns the effective `enableWorkflows` setting or default-on state: @/home/hannah/Projects/claude-code-patcher/modules/1446_oc.js:6
- `MD_` implements `CLAUDE_CODE_WORKFLOWS=true/false`, the `tengu_workflows_enabled` feature flag, Pro gating, and default availability: @/home/hannah/Projects/claude-code-patcher/modules/1446_oc.js:29
- `disableWorkflows` and `enableWorkflows` are declared settings: @/home/hannah/Projects/claude-code-patcher/modules/0584_yu.js:473
- The config UI toggles `enableWorkflows` and `disableWorkflows`: @/home/hannah/Projects/claude-code-patcher/modules/3993_Zl4.js:315

### Keyword trigger setting

The keyword trigger is independently configurable:

- `workflowKeywordTriggerEnabled` is a setting whose description names the `ultracode` keyword trigger: @/home/hannah/Projects/claude-code-patcher/modules/0584_yu.js:483
- `f78()` defaults the trigger to true when the setting is absent: @/home/hannah/Projects/claude-code-patcher/modules/1446_oc.js:16
- The config UI toggles `workflowKeywordTriggerEnabled`, representing enabled as unset and disabled as `false`: @/home/hannah/Projects/claude-code-patcher/modules/3993_Zl4.js:324
- The config serialization/status path includes `workflowKeywordTriggerEnabled`: @/home/hannah/Projects/claude-code-patcher/modules/3993_Zl4.js:1159

### `settings.ultracode`

Upstream exposes a boolean `ultracode` setting:

- Upstream settings schema describes `ultracode` as session-scoped, enabled by `--settings` or remote `apply_flag_settings`, and implemented as `xhigh effort plus standing dynamic-workflow orchestration`: @/home/hannah/Projects/claude-code-patcher/modules/0584_yu.js:705
- Upstream SDK/settings schema also describes `ultracode` as `xhigh effort plus dynamic workflow orchestration`: @/home/hannah/Projects/claude-code-patcher/modules/2966_Fi7.js:740
- `GD6` initializes app-state `ultracode` from `settings.ultracode===true` and calls `BC()`: @/home/hannah/Projects/claude-code-patcher/modules/1447_yA.js:115
- Interactive and headless startup initialize app state with `effortValue: TD6(A.effort)` and `ultracode: GD6(A.effort)`: @/home/hannah/Projects/claude-code-patcher/modules/5070_Wc8.js:1878
- The second startup path uses the same initialization pattern: @/home/hannah/Projects/claude-code-patcher/modules/5070_Wc8.js:2166

Patch 047 changes the setting's effort effect and descriptions:

- Patch 047 changes `settings.ultracode` from returning `xhigh` to returning `max`: @/home/hannah/Projects/claude-code-patcher/patches/047_ultracode_max_effort.py:45
- Patch 047 changes settings prose from `xhigh effort` to `max effort`: @/home/hannah/Projects/claude-code-patcher/patches/047_ultracode_max_effort.py:55
- Patch 047 removes the upstream "xhigh-capable model" requirement from the settings description: @/home/hannah/Projects/claude-code-patcher/patches/047_ultracode_max_effort.py:60
- The patched runnable returns `max` for `settings.ultracode===true`: @/home/hannah/Projects/claude-code-patcher/cli-runnable.js:258
- The patched runnable settings description says `max effort plus standing dynamic-workflow orchestration`: @/home/hannah/Projects/claude-code-patcher/cli-runnable.js:178

### Remote control and applied settings

The `/effort` remote-control helper sends both effort and ultracode intent:

- `KOq` sends subtype `apply_flag_settings` with `settings:{effortLevel:H??null, ultracode:$}`: @/home/hannah/Projects/claude-code-patcher/modules/4458_AOq.js:39
- The remote-control receiver updates `effortLevel` and notifies settings metadata: @/home/hannah/Projects/claude-code-patcher/modules/5060_Vd9.js:2987
- Upstream remote-control Ultracode update uses `xhigh` when `ultracode` is true: @/home/hannah/Projects/claude-code-patcher/modules/5060_Vd9.js:2999
- Remote `get_settings` reports applied `model`, applied `effort`, and applied `ultracode` via `Na(...)`: @/home/hannah/Projects/claude-code-patcher/modules/5060_Vd9.js:3019
- Patch 047 changes the unchanged check and remote-control effort update from `xhigh` to `max`: @/home/hannah/Projects/claude-code-patcher/patches/047_ultracode_max_effort.py:50
- The patched runnable remote-control receiver uses `max` for Ultracode: @/home/hannah/Projects/claude-code-patcher/cli-runnable.js:30505

## Model Gates and Effort Values

### Effort capability gates

The upstream model gate functions distinguish general effort, `max`, and `xhigh`:

- `kP(model)` checks whether the model supports effort at all, including model-family exclusions, explicit capability overrides, environment override, known supported models, and provider mapping fallback: @/home/hannah/Projects/claude-code-patcher/modules/1447_yA.js:1
- `QkH(model)` checks `max_effort` support and excludes Claude 3, older Opus 4.x, Sonnet 4.0/4.5, and Haiku 4.5: @/home/hannah/Projects/claude-code-patcher/modules/1447_yA.js:26
- `JDH(model)` checks `xhigh_effort` support and excludes more models than `QkH`, including Opus 4.6 and Sonnet 4.6: @/home/hannah/Projects/claude-code-patcher/modules/1447_yA.js:51
- `supportedEffortLevels` exposes `max` only when `QkH(model)` is true and exposes `xhigh` only when `JDH(model)` is true: @/home/hannah/Projects/claude-code-patcher/modules/5060_Vd9.js:704

### Upstream Ultracode gate is `xhigh`

Upstream Ultracode availability is `NP()` plus optional `JDH(model)`:

- `cu(model)` returns `NP() && (model is absent || JDH(model))`: @/home/hannah/Projects/claude-code-patcher/modules/1447_yA.js:76
- `Na(model, effort, ultracode)` returns active Ultracode only when `ultracode===true`, workflows are enabled, and resolved effort is `xhigh`: @/home/hannah/Projects/claude-code-patcher/modules/1447_yA.js:79
- The upstream bundled equivalent uses the same `cu` and `Na` logic: @/home/hannah/Projects/claude-code-patcher/claude-code-package/cli.js:259

### Patch 047 Ultracode gate is workflow-only with `max` active state

Patch 047 changes availability and active-state checks:

- `cu(model)` is replaced with `NP()`, removing the `JDH(model)` check: @/home/hannah/Projects/claude-code-patcher/patches/047_ultracode_max_effort.py:32
- `Na` is replaced so active Ultracode requires `ultracode===true`, workflows enabled, raw/session effort equal to `max`, and no incompatible `CLAUDE_CODE_EFFORT_LEVEL` override: @/home/hannah/Projects/claude-code-patcher/patches/047_ultracode_max_effort.py:37
- The patched runnable contains `function cu(H){return NP()}` and the patched `Na` body on the model-effort helper line: @/home/hannah/Projects/claude-code-patcher/cli-runnable.js:260

### `xhigh` versus `max`

The relevant upstream effort values are:

- Persistable upstream settings whitelist is only `low`, `medium`, `high`, and `xhigh`: @/home/hannah/Projects/claude-code-patcher/modules/1447_yA.js:111
- Runtime effort enum already includes `max`: @/home/hannah/Projects/claude-code-patcher/modules/1447_yA.js:261
- `xhigh` UI label says "Fable 5, Opus 4.8/4.7 only": @/home/hannah/Projects/claude-code-patcher/modules/1447_yA.js:243
- `max` UI label says "Fable 5, Opus 4.6+, Sonnet 4.6": @/home/hannah/Projects/claude-code-patcher/modules/1447_yA.js:244
- Patch 002 adds `max` to the settings persistence whitelist: @/home/hannah/Projects/claude-code-patcher/patches/002_permanent_max_effort.py:32
- Patch 002 adds `max` to the settings schema enum: @/home/hannah/Projects/claude-code-patcher/patches/002_permanent_max_effort.py:50

The effort resolver still performs capability fallback:

- `va(model, effort)` returns `undefined` when the model does not support effort at all: @/home/hannah/Projects/claude-code-patcher/modules/1447_yA.js:158
- If the requested/resolved effort is `max` but `QkH(model)` is false, it returns `high`: @/home/hannah/Projects/claude-code-patcher/modules/1447_yA.js:165
- If the requested/resolved effort is `xhigh` but `JDH(model)` is false, it returns `high`: @/home/hannah/Projects/claude-code-patcher/modules/1447_yA.js:166
- The patch 047 docstring calls this the safe fallback for models without `max` effort: @/home/hannah/Projects/claude-code-patcher/patches/047_ultracode_max_effort.py:8

### Environment override

`CLAUDE_CODE_EFFORT_LEVEL` participates in both effort resolution and Ultracode active-state messaging:

- `dkH()` reads `CLAUDE_CODE_EFFORT_LEVEL`, treats absent or `auto` as null, and parses other values through `du`: @/home/hannah/Projects/claude-code-patcher/modules/1447_yA.js:123
- The upstream `/effort ultracode` message warns when the environment override exists and is not `xhigh`: @/home/hannah/Projects/claude-code-patcher/modules/4458_AOq.js:124
- Patch 047 changes that check to warn when the environment override exists and is not `max`: @/home/hannah/Projects/claude-code-patcher/patches/047_ultracode_max_effort.py:128
- Patch 047 changes active-state `Na` to require environment override absent, null, or `max`: @/home/hannah/Projects/claude-code-patcher/patches/047_ultracode_max_effort.py:37

## Dynamic Workflow Coupling

Ultracode is coupled to workflows through three independent surfaces:

1. Keyword opt-in attachment.
2. Session Ultracode reminders.
3. Workflow tool enablement and validation.

### Keyword opt-in attachment

- The literal keyword path creates `workflow_keyword_request`: @/home/hannah/Projects/claude-code-patcher/modules/3890_EL.js:116
- The materialized attachment says the user included `ultracode` and wants Workflow tool usage: @/home/hannah/Projects/claude-code-patcher/modules/3909_Hq.js:3617
- The Workflow tool prompt treats keyword `ultracode` as explicit opt-in: @/home/hannah/Projects/claude-code-patcher/modules/3606_s24.js:8

### Session Ultracode reminders

- `FLf` emits `ultra_effort_enter` when `Na(mainLoopModel, MO(app), Iy8(app))` says session Ultracode is active: @/home/hannah/Projects/claude-code-patcher/modules/3890_EL.js:370
- `FLf` emits `ultra_effort_exit` when the previous app state had `ultracode` true but current active state is false: @/home/hannah/Projects/claude-code-patcher/modules/3890_EL.js:399
- `ultra_effort_enter` is an allowed attachment type: @/home/hannah/Projects/claude-code-patcher/modules/4213_cfq.js:24
- The materialized `ultra_effort_enter` text says Ultracode is active and asks for Workflow tool usage for every substantive task: @/home/hannah/Projects/claude-code-patcher/modules/3909_Hq.js:3625
- The statusline computes active Ultracode with `Na` and feeds it to the status label helper: @/home/hannah/Projects/claude-code-patcher/modules/4739_fv9.js:1558

### Workflow tool behavior

- The Workflow tool prompt says Workflow calls are allowed after explicit opt-in, including keyword `ultracode` or active Ultracode session: @/home/hannah/Projects/claude-code-patcher/modules/3606_s24.js:8
- The Workflow tool prompt says active Ultracode implies standing opt-in for substantive tasks: @/home/hannah/Projects/claude-code-patcher/modules/3606_s24.js:32
- The Workflow usage warning is bypassed when `Na(...)` reports active Ultracode: @/home/hannah/Projects/claude-code-patcher/modules/3619_qe6.js:6

## UI, Status, Slider, and Model Picker

### Status label

- Upstream Ultracode status text says `xhigh effort + dynamic workflows`: @/home/hannah/Projects/claude-code-patcher/modules/3980_B9q.js:1
- Patch 047 changes that text to `max effort + dynamic workflows`: @/home/hannah/Projects/claude-code-patcher/patches/047_ultracode_max_effort.py:66

### Slider and command UI

- Upstream `/effort` slider includes an Ultracode entry only when `cu(model)` is true: @/home/hannah/Projects/claude-code-patcher/modules/4458_AOq.js:257
- Upstream slider sublabel is `xhigh + workflows`: @/home/hannah/Projects/claude-code-patcher/modules/4458_AOq.js:276
- Upstream confirmation maps selected `ultracode` to effort `xhigh`: @/home/hannah/Projects/claude-code-patcher/modules/4458_AOq.js:508
- Patch 047 changes slider sublabel to `max + workflows`: @/home/hannah/Projects/claude-code-patcher/patches/047_ultracode_max_effort.py:145
- Patch 047 changes slider and confirmation effort mapping from `xhigh` to `max`: @/home/hannah/Projects/claude-code-patcher/patches/047_ultracode_max_effort.py:151
- The patched runnable carries these slider and command UI changes on the minified `/effort` command line: @/home/hannah/Projects/claude-code-patcher/cli-runnable.js:9141

### Model picker

- Upstream model picker display treats selected `ultracode` as `xhigh` when compatible, with fallback handling when incompatible: @/home/hannah/Projects/claude-code-patcher/modules/3981_Tb8.js:130
- Upstream model picker persistence maps `ultracode` to `xhigh`: @/home/hannah/Projects/claude-code-patcher/modules/3981_Tb8.js:218
- Upstream model picker session-state helper sets `{effortValue:"xhigh", ultracode:true}`: @/home/hannah/Projects/claude-code-patcher/modules/3981_Tb8.js:500
- Patch 047 changes model picker display, persistence, and session-state mappings from `xhigh` to `max`: @/home/hannah/Projects/claude-code-patcher/patches/047_ultracode_max_effort.py:163
- The patched runnable maps model picker Ultracode to `max`: @/home/hannah/Projects/claude-code-patcher/cli-runnable.js:7469

## Patch and Test Anchors

Patch 047's own stated purpose:

- The docstring says upstream Ultracode used `xhigh` plus workflows and was hidden on models without `xhigh`: @/home/hannah/Projects/claude-code-patcher/patches/047_ultracode_max_effort.py:4
- The docstring says patch 047 makes Ultracode use `max` plus dynamic workflows: @/home/hannah/Projects/claude-code-patcher/patches/047_ultracode_max_effort.py:7
- The docstring says users can type the `ultracode` keyword or run `/effort ultracode`: @/home/hannah/Projects/claude-code-patcher/patches/047_ultracode_max_effort.py:9

The test suite checks the expected patched anchors:

- Tests expect patched `cu`, patched active `Na`, `settings.ultracode` returning `max`, and `max effort + dynamic workflows`: @/home/hannah/Projects/claude-code-patcher/test_patches.py:312
- Tests expect `/effort ultracode` parser return `{value:"max"}` and `KOq("max"`: @/home/hannah/Projects/claude-code-patcher/test_patches.py:316
- Tests expect `effortUpdate` value `max`: @/home/hannah/Projects/claude-code-patcher/test_patches.py:318
- Tests expect literal keyword prompt construction to add `{effort:"max"}` when `workflow_keyword_request` is present: @/home/hannah/Projects/claude-code-patcher/test_patches.py:319
- Tests expect the slider sublabel and model picker session-state mapping to use `max`: @/home/hannah/Projects/claude-code-patcher/test_patches.py:320

Docs summarize patch 047:

- `FEATURES.md` says patch 047 makes Ultracode keyword and `/effort ultracode` use `max` effort with dynamic workflows: @/home/hannah/Projects/claude-code-patcher/FEATURES.md:264
- `README.md` lists patch 047 as "Ultracode Max Effort": @/home/hannah/Projects/claude-code-patcher/README.md:70

## Source Delta Table

| Surface | Upstream `2.1.170` | Patch 047 / local patched bundle |
|---|---|---|
| Availability | `cu(model)=NP() && (model absent || JDH(model))`: @/home/hannah/Projects/claude-code-patcher/modules/1447_yA.js:76 | `cu(model)=NP()`: @/home/hannah/Projects/claude-code-patcher/patches/047_ultracode_max_effort.py:32 |
| Active state | `ultracode===true && NP() && va(model, effort)==="xhigh"`: @/home/hannah/Projects/claude-code-patcher/modules/1447_yA.js:79 | `ultracode===true && NP() && effort==="max"` with env compatibility: @/home/hannah/Projects/claude-code-patcher/patches/047_ultracode_max_effort.py:37 |
| `settings.ultracode` | returns `xhigh`: @/home/hannah/Projects/claude-code-patcher/modules/1443_XD6.js:1 | returns `max`: @/home/hannah/Projects/claude-code-patcher/patches/047_ultracode_max_effort.py:45 |
| `/effort ultracode` parse | returns `{value:"xhigh"}`: @/home/hannah/Projects/claude-code-patcher/modules/4458_AOq.js:32 | returns `{value:"max"}`: @/home/hannah/Projects/claude-code-patcher/patches/047_ultracode_max_effort.py:110 |
| `/effort ultracode` model gate | rejects current models that fail `cu(w7())`: @/home/hannah/Projects/claude-code-patcher/modules/4458_AOq.js:119 | removes the explicit `xhigh` model gate: @/home/hannah/Projects/claude-code-patcher/patches/047_ultracode_max_effort.py:116 |
| Remote apply | `KOq("xhigh", true)`: @/home/hannah/Projects/claude-code-patcher/modules/4458_AOq.js:122 | `KOq("max", true)`: @/home/hannah/Projects/claude-code-patcher/patches/047_ultracode_max_effort.py:123 |
| Literal keyword effort | no keyword-specific effort override: @/home/hannah/Projects/claude-code-patcher/modules/4785_KXq.js:303 | keyword attachment adds `{effort:"max"}`: @/home/hannah/Projects/claude-code-patcher/cli-runnable.js:11331 |
| Slider label | `xhigh + workflows`: @/home/hannah/Projects/claude-code-patcher/modules/4458_AOq.js:276 | `max + workflows`: @/home/hannah/Projects/claude-code-patcher/patches/047_ultracode_max_effort.py:145 |
| Model picker state | `{effortValue:"xhigh", ultracode:true}`: @/home/hannah/Projects/claude-code-patcher/modules/3981_Tb8.js:500 | `{effortValue:"max", ultracode:true}`: @/home/hannah/Projects/claude-code-patcher/patches/047_ultracode_max_effort.py:173 |

## Codex Implications

1. Treat Ultracode as two related but distinct behaviors: keyword-triggered workflow opt-in and session Ultracode mode. Upstream keyword handling does not persist session Ultracode; patch 047 adds only a per-turn `max` effort override for the keyword path.

2. If porting the patched semantics, gate Ultracode availability on workflow availability, not `xhigh` model support. Patch 047 explicitly removes the upstream `JDH(model)` gate while preserving the existing effort resolver's safe fallback from unsupported `max` to `high`.

3. Model `max` and `xhigh` separately. Upstream `max` exists as a runtime effort value and has a broader capability gate than `xhigh`; patch 002 is needed because upstream persistence originally allowed `xhigh` but not `max`.

4. Preserve environment override behavior. Patched active Ultracode is only active when the requested/session effort is `max` and `CLAUDE_CODE_EFFORT_LEVEL` is absent, null/auto, or `max`.

5. Preserve workflow coupling as explicit prompt/tool state. The keyword and session reminders do not directly execute a workflow; they supply explicit opt-in and instructions for the Workflow tool, which is still guarded by workflow availability and validation.

6. Keep remote/settings parity. A complete port needs equivalents for `settings.ultracode`, `/effort ultracode`, keyword-trigger config, applied settings reporting, and remote `apply_flag_settings` so local UI, headless, and remote control surfaces agree on `max` Ultracode semantics.
