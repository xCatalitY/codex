use super::*;
use codex_config::types::WorkflowApproval;
use color_eyre::eyre::WrapErr;
use pretty_assertions::assert_eq;
use std::path::Path;

#[test]
fn app_scoped_key_path_quotes_dotted_app_ids() {
    assert_eq!(
        app_scoped_key_path("plugin.linear", "enabled"),
        "apps.\"plugin.linear\".enabled"
    );
}

#[test]
fn trusted_project_edit_targets_project_trust_level() {
    assert_eq!(
        trusted_project_edit(Path::new("/workspace/team.project")),
        ConfigEdit {
            key_path: "projects.\"/workspace/team.project\".trust_level".to_string(),
            value: serde_json::json!("trusted"),
            merge_strategy: MergeStrategy::Replace,
        }
    );
}

#[test]
fn format_config_error_preserves_server_validation_message() {
    let err = Err::<(), _>(color_eyre::eyre::eyre!(
        "config/batchWrite failed: Invalid configuration: features.fast_mode=true violates \
         managed requirements; allowed set [fast_mode=false]"
    ))
    .wrap_err("config/batchWrite failed in TUI")
    .unwrap_err();

    assert_eq!(
        format_config_error(&err),
        "config/batchWrite failed in TUI: config/batchWrite failed: Invalid configuration: \
         features.fast_mode=true violates managed requirements; allowed set [fast_mode=false]"
    );
}

#[test]
fn workflow_settings_edits_target_workflows_table() {
    assert_eq!(
        build_workflow_settings_edits(
            /*enabled*/ true,
            WorkflowMode::Ultracode,
            WorkflowApproval::Ask,
            /*keyword_trigger_enabled*/ false
        ),
        vec![
            ConfigEdit {
                key_path: "workflows.enabled".to_string(),
                value: serde_json::json!(true),
                merge_strategy: MergeStrategy::Replace,
            },
            ConfigEdit {
                key_path: "workflows.mode".to_string(),
                value: serde_json::json!("ultracode"),
                merge_strategy: MergeStrategy::Replace,
            },
            ConfigEdit {
                key_path: "workflows.approval".to_string(),
                value: serde_json::json!("ask"),
                merge_strategy: MergeStrategy::Replace,
            },
            ConfigEdit {
                key_path: "workflows.keyword_trigger_enabled".to_string(),
                value: serde_json::json!(false),
                merge_strategy: MergeStrategy::Replace,
            },
        ]
    );
}

#[test]
fn named_workflow_approval_edit_quotes_workflow_names() {
    assert_eq!(
        build_named_workflow_approval_edit("sample:release", Some(WorkflowApproval::Allow)),
        ConfigEdit {
            key_path: "workflows.named.\"sample:release\".approval".to_string(),
            value: serde_json::json!("allow"),
            merge_strategy: MergeStrategy::Replace,
        }
    );
    assert_eq!(
        build_named_workflow_approval_edit("team.release", None),
        ConfigEdit {
            key_path: "workflows.named.\"team.release\".approval".to_string(),
            value: serde_json::Value::Null,
            merge_strategy: MergeStrategy::Replace,
        }
    );
}

#[test]
fn named_workflow_enabled_edit_quotes_workflow_names() {
    assert_eq!(
        build_named_workflow_enabled_edit("sample:release", Some(false)),
        ConfigEdit {
            key_path: "workflows.named.\"sample:release\".enabled".to_string(),
            value: serde_json::json!(false),
            merge_strategy: MergeStrategy::Replace,
        }
    );
    assert_eq!(
        build_named_workflow_enabled_edit("team.release", None),
        ConfigEdit {
            key_path: "workflows.named.\"team.release\".enabled".to_string(),
            value: serde_json::Value::Null,
            merge_strategy: MergeStrategy::Replace,
        }
    );
}
