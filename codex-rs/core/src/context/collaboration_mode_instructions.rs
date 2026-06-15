use super::ContextualUserFragment;
use codex_protocol::config_types::CollaborationMode;
use codex_protocol::config_types::WorkflowMode;
use codex_protocol::protocol::COLLABORATION_MODE_CLOSE_TAG;
use codex_protocol::protocol::COLLABORATION_MODE_OPEN_TAG;

const DYNAMIC_WORKFLOW_INSTRUCTIONS: &str = r#"Dynamic workflows are enabled for this turn.

When the `workflow` tool is available, use it for tasks that benefit from explicit orchestration, reusable phases, parallel branches, or subagents. Prefer direct execution for small linear tasks. Every workflow script must begin with `export const meta = { name, description, ... }` using static literal strings for at least `name` and `description`, then plain JavaScript body code. Optional `meta.inputSchema` is validated against supplied `args` before body code runs for common JSON Schema fields. Inline and file-based workflows can require review before execution depending on `[workflows].approval`; if approval is denied, choose a smaller direct approach or ask the user how to proceed. If you use workflows, keep each phase bounded, use `phase`/`log` to report progress, and synthesize the final result yourself. Workflow scripts may call normal tools, `agent(...)`, `parallel(...)`, `pipeline(...)`, and `workflow(...)` only when those helpers are available. Prefer `pipeline(items, stage1, stage2, ...)` for multi-stage per-item work; use `parallel(items)` only when a barrier is actually needed. `budget.total` is null and `budget.remaining()` is Infinity when no Codex workflow token target exists, so loops using budget must also have a hard iteration cap. `agent(...)` accepts `label`, `phase`, `agentType`/`agent_type`, `model`, `reasoningEffort`/`reasoning_effort`, `serviceTier`/`service_tier`, and `forkTurns`/`fork_turns` when the corresponding Codex spawn-agent field is supported. Use `workflow("name", args)`, `workflow("plugin:name", args)`, or `workflow({scriptPath: "./path.js"}, args)` to run a configured child workflow; child workflow nesting is limited to one level. Use top-level `await`; if you wrap work in `workflow(async () => { ... })`, write `await workflow(...)` or `return workflow(...)` so nested tools complete before the workflow returns."#;

const ULTRACODE_WORKFLOW_INSTRUCTIONS: &str = r#"Ultracode is active for this turn: use xhigh reasoning plus dynamic workflow orchestration.

Break complex work into explicit phases, spawn subagents only for independent bounded subtasks, and keep responsibility for final integration in the root turn. Do not claim extra permissions or tool access; use the workflow/runtime tools exactly as exposed."#;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CollaborationModeInstructions {
    instructions: String,
}

impl CollaborationModeInstructions {
    pub(crate) fn from_collaboration_mode(collaboration_mode: &CollaborationMode) -> Option<Self> {
        let mut sections = Vec::new();
        if let Some(instructions) = collaboration_mode
            .settings
            .developer_instructions
            .as_ref()
            .filter(|instructions| !instructions.is_empty())
        {
            sections.push(instructions.clone());
        }

        match collaboration_mode.workflow_mode() {
            WorkflowMode::Disabled => {}
            WorkflowMode::Dynamic => {
                sections.push(DYNAMIC_WORKFLOW_INSTRUCTIONS.to_string());
            }
            WorkflowMode::Ultracode => {
                sections.push(ULTRACODE_WORKFLOW_INSTRUCTIONS.to_string());
                sections.push(DYNAMIC_WORKFLOW_INSTRUCTIONS.to_string());
            }
        }

        (!sections.is_empty()).then(|| Self {
            instructions: sections.join("\n\n"),
        })
    }
}

impl ContextualUserFragment for CollaborationModeInstructions {
    fn role(&self) -> &'static str {
        "developer"
    }

    fn markers(&self) -> (&'static str, &'static str) {
        Self::type_markers()
    }

    fn type_markers() -> (&'static str, &'static str) {
        (COLLABORATION_MODE_OPEN_TAG, COLLABORATION_MODE_CLOSE_TAG)
    }

    fn body(&self) -> String {
        self.instructions.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_protocol::config_types::ModeKind;
    use codex_protocol::config_types::Settings;
    use codex_protocol::openai_models::ReasoningEffort;

    fn mode(workflow_mode: Option<WorkflowMode>) -> CollaborationMode {
        CollaborationMode {
            mode: ModeKind::Default,
            settings: Settings {
                model: "gpt-5.5-codex".to_string(),
                reasoning_effort: Some(ReasoningEffort::XHigh),
                developer_instructions: None,
                workflow_mode,
            },
        }
    }

    #[test]
    fn disabled_workflow_mode_without_developer_text_has_no_fragment() {
        assert!(CollaborationModeInstructions::from_collaboration_mode(&mode(None)).is_none());
    }

    #[test]
    fn dynamic_workflow_mode_adds_workflow_guidance() {
        let fragment = CollaborationModeInstructions::from_collaboration_mode(&mode(Some(
            WorkflowMode::Dynamic,
        )))
        .expect("dynamic workflow instructions");

        let rendered = fragment.render();
        assert!(rendered.contains("Dynamic workflows are enabled"));
        assert!(rendered.contains("workflow"));
        assert!(!rendered.contains("Ultracode is active"));
    }

    #[test]
    fn ultracode_workflow_mode_adds_xhigh_and_workflow_guidance() {
        let fragment = CollaborationModeInstructions::from_collaboration_mode(&mode(Some(
            WorkflowMode::Ultracode,
        )))
        .expect("ultracode workflow instructions");

        let rendered = fragment.render();
        assert!(rendered.contains("Ultracode is active"));
        assert!(rendered.contains("xhigh reasoning"));
        assert!(rendered.contains("Dynamic workflows are enabled"));
    }
}
