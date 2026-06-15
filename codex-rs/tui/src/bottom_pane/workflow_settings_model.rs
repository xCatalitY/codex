use codex_config::types::WorkflowApproval;
use codex_protocol::config_types::WorkflowMode;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WorkflowNamedPolicyItem {
    pub(crate) name: String,
    pub(crate) enabled: Option<bool>,
    pub(crate) approval: Option<WorkflowApproval>,
}

pub(super) fn workflow_mode_label(mode: WorkflowMode) -> &'static str {
    match mode {
        WorkflowMode::Disabled => "off",
        WorkflowMode::Dynamic => "dynamic",
        WorkflowMode::Ultracode => "ultracode",
    }
}

pub(super) fn next_workflow_mode(mode: WorkflowMode) -> WorkflowMode {
    match mode {
        WorkflowMode::Disabled => WorkflowMode::Dynamic,
        WorkflowMode::Dynamic => WorkflowMode::Ultracode,
        WorkflowMode::Ultracode => WorkflowMode::Disabled,
    }
}

pub(super) fn workflow_approval_label(approval: WorkflowApproval) -> &'static str {
    match approval {
        WorkflowApproval::Auto => "auto",
        WorkflowApproval::Ask => "ask",
        WorkflowApproval::Allow => "allow",
        WorkflowApproval::Deny => "deny",
    }
}

pub(super) fn next_workflow_approval(approval: WorkflowApproval) -> WorkflowApproval {
    match approval {
        WorkflowApproval::Auto => WorkflowApproval::Ask,
        WorkflowApproval::Ask => WorkflowApproval::Allow,
        WorkflowApproval::Allow => WorkflowApproval::Deny,
        WorkflowApproval::Deny => WorkflowApproval::Auto,
    }
}

pub(super) fn workflow_enabled_override_label(enabled: Option<bool>) -> &'static str {
    match enabled {
        Some(true) => "enabled",
        Some(false) => "disabled",
        None => "inherited",
    }
}

pub(super) fn next_workflow_enabled_override(enabled: Option<bool>) -> Option<bool> {
    match enabled {
        None => Some(true),
        Some(true) => Some(false),
        Some(false) => None,
    }
}

pub(super) fn workflow_optional_approval_label(approval: Option<WorkflowApproval>) -> &'static str {
    match approval {
        Some(approval) => workflow_approval_label(approval),
        None => "inherited",
    }
}

pub(super) fn next_optional_workflow_approval(
    approval: Option<WorkflowApproval>,
) -> Option<WorkflowApproval> {
    match approval {
        None => Some(WorkflowApproval::Auto),
        Some(WorkflowApproval::Auto) => Some(WorkflowApproval::Ask),
        Some(WorkflowApproval::Ask) => Some(WorkflowApproval::Allow),
        Some(WorkflowApproval::Allow) => Some(WorkflowApproval::Deny),
        Some(WorkflowApproval::Deny) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workflow_mode_cycles_through_supported_codex_modes() {
        assert_eq!(
            next_workflow_mode(WorkflowMode::Disabled),
            WorkflowMode::Dynamic
        );
        assert_eq!(
            next_workflow_mode(WorkflowMode::Dynamic),
            WorkflowMode::Ultracode
        );
        assert_eq!(
            next_workflow_mode(WorkflowMode::Ultracode),
            WorkflowMode::Disabled
        );
        assert_eq!(workflow_mode_label(WorkflowMode::Ultracode), "ultracode");
    }

    #[test]
    fn workflow_approval_cycles_without_max_or_hidden_states() {
        assert_eq!(
            next_workflow_approval(WorkflowApproval::Auto),
            WorkflowApproval::Ask
        );
        assert_eq!(
            next_workflow_approval(WorkflowApproval::Ask),
            WorkflowApproval::Allow
        );
        assert_eq!(
            next_workflow_approval(WorkflowApproval::Allow),
            WorkflowApproval::Deny
        );
        assert_eq!(
            next_workflow_approval(WorkflowApproval::Deny),
            WorkflowApproval::Auto
        );
        assert_eq!(workflow_approval_label(WorkflowApproval::Auto), "auto");
    }

    #[test]
    fn named_workflow_policy_cycles_include_inherited_state() {
        assert_eq!(next_workflow_enabled_override(None), Some(true));
        assert_eq!(next_workflow_enabled_override(Some(true)), Some(false));
        assert_eq!(next_workflow_enabled_override(Some(false)), None);
        assert_eq!(workflow_enabled_override_label(None), "inherited");

        assert_eq!(
            next_optional_workflow_approval(None),
            Some(WorkflowApproval::Auto)
        );
        assert_eq!(
            next_optional_workflow_approval(Some(WorkflowApproval::Deny)),
            None
        );
        assert_eq!(workflow_optional_approval_label(None), "inherited");
    }
}
