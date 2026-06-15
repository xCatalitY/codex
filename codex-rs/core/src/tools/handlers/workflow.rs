use crate::config::edit::ConfigEdit;
use crate::config::edit::ConfigEditsBuilder;
use crate::function_tool::FunctionCallError;
use crate::guardian::GuardianApprovalRequest;
use crate::guardian::guardian_rejection_message;
use crate::guardian::guardian_timeout_message;
use crate::guardian::new_guardian_review_id;
use crate::guardian::review_approval_request;
use crate::guardian::routes_approval_to_guardian;
use crate::hook_runtime::run_permission_request_hooks;
use crate::hook_runtime::run_workflow_task_completed_hooks;
use crate::hook_runtime::run_workflow_task_created_hooks;
use crate::tools::code_mode::ExecContext;
use crate::tools::code_mode::handle_runtime_response;
use crate::tools::context::FunctionToolOutput;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::context::boxed_tool_output;
use crate::tools::hook_names::HookToolName;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::ToolExecutor;
use crate::tools::sandboxing::PermissionRequestPayload;
use codex_config::types::WorkflowApproval;
use codex_config::types::WorkflowDefinitionConfig;
use codex_hooks::PermissionRequestDecision;
use codex_protocol::approvals::NetworkPolicyRuleAction;
use codex_protocol::models::FunctionCallOutputContentItem;
use codex_protocol::protocol::{AskForApproval, ReviewDecision};
use codex_protocol::request_user_input::RequestUserInputArgs;
use codex_protocol::request_user_input::RequestUserInputQuestion;
use codex_protocol::request_user_input::RequestUserInputQuestionOption;
use codex_protocol::request_user_input::RequestUserInputResponse;
use codex_tools::AdditionalProperties;
use codex_tools::JsonSchema;
use codex_tools::JsonSchemaPrimitiveType;
use codex_tools::JsonSchemaType;
use codex_tools::ResponsesApiTool;
use codex_tools::ToolName;
use codex_tools::ToolSpec;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value as JsonValue;
use std::collections::BTreeMap;
use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;
use toml_edit::value;

mod metadata;
mod run_store;

use metadata::ValidatedWorkflowScript;
use metadata::WorkflowMetadata;
use metadata::validate_workflow_script;
use run_store::WorkflowRunArtifacts;
use run_store::WorkflowRunIdentity;
use run_store::WorkflowRunStatus;
use run_store::is_safe_workflow_run_id;
use run_store::normalized_resume_run_id;
use run_store::persist_workflow_artifacts;
use run_store::persist_workflow_run_state;
use run_store::unix_time_millis;
use run_store::workflow_output_preview;
use run_store::workflow_resume_cache_hit;
use run_store::workflow_run_artifacts;
use run_store::workflow_run_snapshot;
use run_store::workflow_run_snapshot_dir;

pub(crate) const WORKFLOW_TOOL_NAME: &str = "workflow";
const WORKFLOW_AGENT_JOURNAL_FILE: &str = "journal.jsonl";
const WORKFLOW_AGENT_JOURNAL_MAX_BYTES: u64 = 1024 * 1024;
const WORKFLOW_AGENT_JOURNAL_MAX_ENTRIES: usize = 1000;
const WORKFLOW_APPROVAL_QUESTION_ID_PREFIX: &str = "workflow_approval";
const WORKFLOW_APPROVAL_ALLOW: &str = "Allow";
const WORKFLOW_APPROVAL_ALLOW_FOR_SESSION: &str = "Allow for this session";
const WORKFLOW_APPROVAL_ALLOW_ALWAYS: &str = "Always allow this workflow";
const WORKFLOW_APPROVAL_CANCEL: &str = "Cancel";
const WORKFLOW_APPROVAL_PREVIEW_MAX_CHARS: usize = 1200;
const WORKFLOW_PROGRESS_NOTIFICATION_TYPE: &str = "codex_workflow_progress";
static WORKFLOW_RUN_COUNTER: AtomicU64 = AtomicU64::new(0);

pub(crate) struct WorkflowHandler {
    nested_tool_specs: Vec<ToolSpec>,
}

impl WorkflowHandler {
    pub(crate) fn new(nested_tool_specs: Vec<ToolSpec>) -> Self {
        Self { nested_tool_specs }
    }
}

impl ToolExecutor<ToolInvocation> for WorkflowHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain(WORKFLOW_TOOL_NAME)
    }

    fn spec(&self) -> ToolSpec {
        create_workflow_tool()
    }

    fn handle(&self, invocation: ToolInvocation) -> codex_tools::ToolExecutorFuture<'_> {
        Box::pin(self.handle_call(invocation))
    }
}

impl WorkflowHandler {
    async fn handle_call(
        &self,
        invocation: ToolInvocation,
    ) -> Result<Box<dyn crate::tools::context::ToolOutput>, FunctionCallError> {
        let ToolInvocation {
            session,
            turn,
            call_id,
            payload,
            ..
        } = invocation;
        let arguments = match payload {
            ToolPayload::Function { arguments } => arguments,
            _ => {
                return Err(FunctionCallError::RespondToModel(
                    "workflow expects JSON function arguments".to_string(),
                ));
            }
        };
        let mut args: WorkflowArgs = serde_json::from_str(&arguments).map_err(|err| {
            FunctionCallError::RespondToModel(format!("failed to parse workflow arguments: {err}"))
        })?;
        inherit_resume_args(turn.as_ref(), &mut args).await;
        let workflow_source = resolve_workflow_source(turn.as_ref(), &args).await?;
        let validated = validate_workflow_script(&workflow_source.code)?;
        let workflow_name = workflow_display_name(&args, &workflow_source, &validated);
        let run_id = new_workflow_run_id();
        let started_unix_ms = unix_time_millis();
        let max_output_tokens = args
            .max_output_tokens
            .or(turn.config.workflows.max_output_tokens);
        let workflow_cwd = workflow_cwd(turn.as_ref()).clone();
        let workflow_git_branch =
            codex_git_utils::current_branch_name(workflow_cwd.as_path()).await;
        let workflow_identity = WorkflowRunIdentity {
            session_id: Some(session.session_id().to_string()),
            thread_id: Some(session.thread_id.to_string()),
            workflow_tool_call_id: Some(call_id.clone()),
            cwd: Some(workflow_cwd.display().to_string()),
            git_branch: workflow_git_branch.clone(),
        };
        if let Err(message) = ensure_workflow_approved(
            &session,
            &turn,
            call_id.as_str(),
            &workflow_name,
            &workflow_source,
            &validated,
            &args,
        )
        .await
        {
            let snapshot = workflow_run_snapshot(
                &run_id,
                &workflow_name,
                &workflow_source,
                &validated,
                &args,
                max_output_tokens,
                Some(&workflow_identity),
                None,
                WorkflowRunStatus::Failed,
                started_unix_ms,
                None,
                None,
                Some(message.as_str()),
            );
            persist_workflow_run_state(turn.as_ref(), &snapshot, None).await;
            return Err(FunctionCallError::RespondToModel(message));
        }
        let artifacts = workflow_run_artifacts(turn.as_ref(), &run_id, &workflow_source);
        persist_workflow_artifacts(&workflow_source, &artifacts).await;
        if let Some(cache_hit) =
            workflow_resume_cache_hit(turn.as_ref(), &args, &workflow_source).await
        {
            let output_text = if cache_hit.output_preview.trim().is_empty() {
                format!(
                    "Workflow resume cache hit from `{}`. The prior completed run had no text output.",
                    cache_hit.run_id
                )
            } else {
                format!(
                    "Workflow resume cache hit from `{}`.\n{}",
                    cache_hit.run_id, cache_hit.output_preview
                )
            };
            let snapshot = workflow_run_snapshot(
                &run_id,
                &workflow_name,
                &workflow_source,
                &validated,
                &args,
                max_output_tokens,
                Some(&workflow_identity),
                Some(&artifacts),
                WorkflowRunStatus::Completed,
                started_unix_ms,
                None,
                Some(output_text.as_str()),
                None,
            );
            persist_workflow_run_state(turn.as_ref(), &snapshot, Some(&artifacts)).await;
            let mut output = FunctionToolOutput::from_text(output_text, Some(true));
            prefix_workflow_status(
                &mut output,
                &workflow_name,
                &run_id,
                Some(&artifacts),
                &args,
            );
            return Ok(boxed_tool_output(output));
        }
        let child_definitions =
            collect_child_workflow_definitions(turn.as_ref(), &workflow_source).await;
        let agent_journal_entries = read_resume_agent_journal(turn.as_ref(), &args).await;
        let concurrency_cap = workflow_concurrency_cap_for_turn(turn.as_ref());
        let source = build_workflow_script(
            &run_id,
            &args,
            &validated,
            &child_definitions,
            &agent_journal_entries,
            max_output_tokens,
            concurrency_cap,
            workflow_cwd.as_path(),
            workflow_git_branch.as_deref(),
            workflow_identity.thread_id.as_deref(),
        )?;
        let enabled_tools =
            codex_tools::collect_code_mode_tool_definitions(&self.nested_tool_specs);
        let exec = ExecContext {
            session: session.clone(),
            turn: turn.clone(),
        };
        let started_at = std::time::Instant::now();
        let started_cell = match session
            .services
            .code_mode_service
            .execute(codex_code_mode::ExecuteRequest {
                tool_call_id: call_id.clone(),
                enabled_tools,
                source: source.clone(),
                yield_time_ms: Some(turn.config.workflows.yield_time_ms),
                max_output_tokens,
            })
            .await
        {
            Ok(started_cell) => started_cell,
            Err(err) => {
                let snapshot = workflow_run_snapshot(
                    &run_id,
                    &workflow_name,
                    &workflow_source,
                    &validated,
                    &args,
                    max_output_tokens,
                    Some(&workflow_identity),
                    Some(&artifacts),
                    WorkflowRunStatus::Failed,
                    started_unix_ms,
                    None,
                    None,
                    Some(err.as_str()),
                );
                persist_workflow_run_state(turn.as_ref(), &snapshot, Some(&artifacts)).await;
                return Err(FunctionCallError::RespondToModel(err));
            }
        };
        let cell_id = started_cell.cell_id.clone();
        let runtime_cell_id = cell_id.to_string();
        let code_cell_trace = session.services.rollout_thread_trace.start_code_cell_trace(
            turn.sub_id.as_str(),
            runtime_cell_id.as_str(),
            call_id.as_str(),
            source.as_str(),
        );
        let initial_running_snapshot = workflow_run_snapshot(
            &run_id,
            &workflow_name,
            &workflow_source,
            &validated,
            &args,
            max_output_tokens,
            Some(&workflow_identity),
            Some(&artifacts),
            WorkflowRunStatus::Running,
            started_unix_ms,
            Some(cell_id.as_str()),
            None,
            None,
        );
        persist_workflow_run_state(turn.as_ref(), &initial_running_snapshot, Some(&artifacts))
            .await;
        run_workflow_task_created_hooks(
            &session,
            &turn,
            &workflow_name,
            &run_id,
            Some(cell_id.as_str()),
            Some(validated.metadata.description.as_str()),
        )
        .await;
        session
            .services
            .code_mode_service
            .mark_cell_ready_for_dispatch(&cell_id);
        let response = match started_cell.initial_response().await {
            Ok(response) => response,
            Err(err) => {
                let snapshot = workflow_run_snapshot(
                    &run_id,
                    &workflow_name,
                    &workflow_source,
                    &validated,
                    &args,
                    max_output_tokens,
                    Some(&workflow_identity),
                    Some(&artifacts),
                    WorkflowRunStatus::Failed,
                    started_unix_ms,
                    Some(cell_id.as_str()),
                    None,
                    Some(err.as_str()),
                );
                persist_workflow_run_state(turn.as_ref(), &snapshot, Some(&artifacts)).await;
                run_workflow_task_completed_hooks(
                    &session,
                    &turn,
                    &workflow_name,
                    &run_id,
                    Some(cell_id.as_str()),
                    workflow_run_status_label(WorkflowRunStatus::Failed),
                    Some(validated.metadata.description.as_str()),
                )
                .await;
                return Err(FunctionCallError::RespondToModel(err));
            }
        };
        code_cell_trace.record_initial_response(&response);
        if !matches!(response, codex_code_mode::RuntimeResponse::Yielded { .. }) {
            code_cell_trace.record_ended(&response);
            session
                .services
                .code_mode_service
                .finish_cell_dispatch(&cell_id);
        }
        let response_status = workflow_run_status_for_runtime_response(&response);
        let mut output =
            match handle_runtime_response(&exec, response, max_output_tokens, started_at).await {
                Ok(output) => output,
                Err(err) => {
                    let snapshot = workflow_run_snapshot(
                        &run_id,
                        &workflow_name,
                        &workflow_source,
                        &validated,
                        &args,
                        max_output_tokens,
                        Some(&workflow_identity),
                        Some(&artifacts),
                        WorkflowRunStatus::Failed,
                        started_unix_ms,
                        Some(cell_id.as_str()),
                        None,
                        Some(err.as_str()),
                    );
                    persist_workflow_run_state(turn.as_ref(), &snapshot, Some(&artifacts)).await;
                    run_workflow_task_completed_hooks(
                        &session,
                        &turn,
                        &workflow_name,
                        &run_id,
                        Some(cell_id.as_str()),
                        workflow_run_status_label(WorkflowRunStatus::Failed),
                        Some(validated.metadata.description.as_str()),
                    )
                    .await;
                    return Err(FunctionCallError::RespondToModel(err));
                }
            };
        let output_preview = workflow_output_preview(&output);
        let status = response_status;
        let error = matches!(
            status,
            WorkflowRunStatus::Failed | WorkflowRunStatus::Terminated
        )
        .then_some(output_preview.as_str());
        let snapshot = workflow_run_snapshot(
            &run_id,
            &workflow_name,
            &workflow_source,
            &validated,
            &args,
            max_output_tokens,
            Some(&workflow_identity),
            Some(&artifacts),
            status,
            started_unix_ms,
            Some(cell_id.as_str()),
            Some(output_preview.as_str()),
            error,
        );
        persist_workflow_run_state(turn.as_ref(), &snapshot, Some(&artifacts)).await;
        if status != WorkflowRunStatus::Running {
            run_workflow_task_completed_hooks(
                &session,
                &turn,
                &workflow_name,
                &run_id,
                Some(cell_id.as_str()),
                workflow_run_status_label(status),
                Some(validated.metadata.description.as_str()),
            )
            .await;
        }
        prefix_workflow_status(
            &mut output,
            &workflow_name,
            &run_id,
            Some(&artifacts),
            &args,
        );
        Ok(boxed_tool_output(output))
    }
}

impl CoreToolRuntime for WorkflowHandler {
    fn matches_kind(&self, payload: &ToolPayload) -> bool {
        matches!(payload, ToolPayload::Function { .. })
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkflowArgs {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    script: Option<String>,
    #[serde(default, rename = "scriptPath", alias = "script_path")]
    script_path: Option<String>,
    #[serde(default)]
    args: Option<JsonValue>,
    #[serde(default, rename = "resumeFromRunId", alias = "resume_from_run_id")]
    resume_from_run_id: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    max_output_tokens: Option<usize>,
}

struct WorkflowSource {
    name: String,
    code: String,
    kind: WorkflowSourceKind,
    path: Option<PathBuf>,
}

#[derive(Debug)]
struct ChildWorkflowDefinition {
    keys: Vec<String>,
    metadata: WorkflowMetadata,
    body: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum WorkflowSourceKind {
    Inline,
    ScriptPath,
    Named,
}

#[derive(Debug, PartialEq, Eq)]
enum WorkflowApprovalResolution {
    Allow,
    Ask,
    Deny(String),
}

#[derive(Debug, PartialEq, Eq)]
enum WorkflowApprovalResponseDecision {
    Allow,
    AllowForSession,
    AllowAlways,
    Cancel,
}

#[derive(Clone, Debug, Serialize)]
struct WorkflowApprovalKey {
    source_kind: WorkflowSourceKind,
    source_name: String,
    metadata_name: String,
    path: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
struct WorkflowAgentJournalEntry {
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    entry_type: Option<String>,
    key: String,
    #[serde(default, rename = "agentId", skip_serializing_if = "Option::is_none")]
    agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    child: Option<String>,
    #[serde(
        default,
        rename = "childRunId",
        skip_serializing_if = "Option::is_none"
    )]
    child_run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    result: Option<JsonValue>,
}

async fn ensure_workflow_approved(
    session: &Arc<crate::session::session::Session>,
    turn: &Arc<crate::session::turn_context::TurnContext>,
    call_id: &str,
    workflow_name: &str,
    source: &WorkflowSource,
    validated: &ValidatedWorkflowScript,
    args: &WorkflowArgs,
) -> Result<(), String> {
    match workflow_approval_resolution(turn.as_ref(), source, validated) {
        WorkflowApprovalResolution::Allow => Ok(()),
        WorkflowApprovalResolution::Deny(message) => Err(message),
        WorkflowApprovalResolution::Ask => {
            let cache_key = workflow_session_approval_key(source, &validated.metadata);
            if let Some(key) = cache_key.as_ref()
                && workflow_approval_is_remembered(session.as_ref(), key).await
            {
                return Ok(());
            }

            match run_permission_request_hooks(
                session,
                turn,
                call_id,
                workflow_permission_request_payload(workflow_name, source, validated, args),
            )
            .await
            {
                Some(PermissionRequestDecision::Allow) => return Ok(()),
                Some(PermissionRequestDecision::Deny { message }) => return Err(message),
                None => {}
            }

            if let Some(reason) =
                workflow_approval_prompt_rejection_reason(turn.approval_policy.value())
            {
                return Err(reason.to_string());
            }

            if routes_approval_to_guardian(turn.as_ref()) {
                return apply_workflow_guardian_decision(
                    session.as_ref(),
                    cache_key.as_ref(),
                    review_workflow_with_guardian(
                        session,
                        turn,
                        call_id,
                        workflow_name,
                        source,
                        validated,
                        args,
                    )
                    .await,
                )
                .await;
            }

            let question_id = format!("{WORKFLOW_APPROVAL_QUESTION_ID_PREFIX}_{call_id}");
            let persistent_name = workflow_persistent_approval_name(source, &validated.metadata);
            let response = request_workflow_approval(
                session,
                turn,
                call_id,
                &question_id,
                workflow_name,
                source,
                validated,
                args,
                cache_key.as_ref(),
                persistent_name.as_deref(),
            )
            .await;
            match parse_workflow_approval_response(response, &question_id) {
                WorkflowApprovalResponseDecision::Allow => Ok(()),
                WorkflowApprovalResponseDecision::AllowForSession => {
                    if let Some(key) = cache_key {
                        remember_workflow_approval(session.as_ref(), key).await;
                    }
                    Ok(())
                }
                WorkflowApprovalResponseDecision::AllowAlways => {
                    match (cache_key, persistent_name) {
                        (Some(key), Some(workflow_name)) => {
                            persist_workflow_approval_allow(
                                session.as_ref(),
                                turn.as_ref(),
                                &workflow_name,
                                key,
                            )
                            .await;
                        }
                        (Some(key), None) => {
                            remember_workflow_approval(session.as_ref(), key).await;
                        }
                        (None, _) => {}
                    }
                    Ok(())
                }
                WorkflowApprovalResponseDecision::Cancel => {
                    Err(format!("workflow `{workflow_name}` was not approved"))
                }
            }
        }
    }
}

fn workflow_approval_resolution(
    turn: &crate::session::turn_context::TurnContext,
    source: &WorkflowSource,
    validated: &ValidatedWorkflowScript,
) -> WorkflowApprovalResolution {
    if let Some((name, rule)) = named_workflow_rule(turn, source, &validated.metadata) {
        if rule.enabled == Some(false) {
            return WorkflowApprovalResolution::Deny(format!(
                "workflow `{name}` is disabled by `[workflows.named.{name}]`"
            ));
        }
        if let Some(approval) = rule.approval {
            return workflow_approval_resolution_for_mode(turn, approval, source);
        }
    }

    workflow_approval_resolution_for_mode(turn, turn.config.workflows.approval, source)
}

fn workflow_approval_resolution_for_mode(
    turn: &crate::session::turn_context::TurnContext,
    approval: WorkflowApproval,
    source: &WorkflowSource,
) -> WorkflowApprovalResolution {
    match approval {
        WorkflowApproval::Allow => WorkflowApprovalResolution::Allow,
        WorkflowApproval::Ask => WorkflowApprovalResolution::Ask,
        WorkflowApproval::Deny => WorkflowApprovalResolution::Deny(format!(
            "workflow `{}` is denied by workflow approval config",
            source.name
        )),
        WorkflowApproval::Auto => match source.kind {
            WorkflowSourceKind::Named if workflow_named_source_is_auto_trusted(turn, source) => {
                WorkflowApprovalResolution::Allow
            }
            WorkflowSourceKind::Named => WorkflowApprovalResolution::Ask,
            WorkflowSourceKind::Inline | WorkflowSourceKind::ScriptPath => {
                WorkflowApprovalResolution::Ask
            }
        },
    }
}

fn workflow_named_source_is_auto_trusted(
    turn: &crate::session::turn_context::TurnContext,
    source: &WorkflowSource,
) -> bool {
    if source.kind != WorkflowSourceKind::Named {
        return false;
    }
    let Some(path) = source.path.as_deref() else {
        return false;
    };
    if turn
        .config
        .workflows
        .plugin_workflow_dirs
        .iter()
        .any(|plugin| path.starts_with(plugin.dir.as_path()))
    {
        return true;
    }
    path.starts_with(turn.config.codex_home.join("workflows/.system").as_path())
}

fn named_workflow_rule<'a>(
    turn: &'a crate::session::turn_context::TurnContext,
    source: &WorkflowSource,
    metadata: &WorkflowMetadata,
) -> Option<(String, &'a WorkflowDefinitionConfig)> {
    if source.kind != WorkflowSourceKind::Named {
        return None;
    }
    if source.name.contains(':') {
        return turn
            .config
            .workflows
            .named
            .get(&source.name)
            .map(|rule| (source.name.clone(), rule));
    }
    for name in [&source.name, &metadata.name] {
        if let Some(rule) = turn.config.workflows.named.get(name) {
            return Some((name.clone(), rule));
        }
    }
    None
}

fn workflow_approval_prompt_rejection_reason(
    approval_policy: AskForApproval,
) -> Option<&'static str> {
    match approval_policy {
        AskForApproval::Never => {
            Some("workflow approval required, but approval_policy is set to never")
        }
        AskForApproval::Granular(config) if !config.allows_skill_approval() => Some(
            "workflow approval required, but granular approval config has skill_approval=false",
        ),
        _ => None,
    }
}

fn workflow_session_approval_key(
    source: &WorkflowSource,
    metadata: &WorkflowMetadata,
) -> Option<WorkflowApprovalKey> {
    if source.kind == WorkflowSourceKind::Inline {
        return None;
    }
    Some(WorkflowApprovalKey {
        source_kind: source.kind,
        source_name: source.name.clone(),
        metadata_name: metadata.name.clone(),
        path: source.path.as_ref().map(|path| path.display().to_string()),
    })
}

fn workflow_persistent_approval_name(
    source: &WorkflowSource,
    _metadata: &WorkflowMetadata,
) -> Option<String> {
    if source.kind != WorkflowSourceKind::Named {
        return None;
    }
    normalize_workflow_name(&source.name)
        .ok()
        .map(|_| source.name.clone())
}

fn workflow_permission_request_payload(
    workflow_name: &str,
    source: &WorkflowSource,
    validated: &ValidatedWorkflowScript,
    args: &WorkflowArgs,
) -> PermissionRequestPayload {
    PermissionRequestPayload {
        tool_name: HookToolName::new(WORKFLOW_TOOL_NAME),
        tool_input: serde_json::json!({
            "workflow": workflow_name,
            "source": {
                "kind": workflow_source_kind_label(source.kind),
                "name": source.name.as_str(),
                "path": source.path.as_ref().map(|path| path.display().to_string()),
            },
            "metadata": {
                "name": validated.metadata.name.as_str(),
                "description": validated.metadata.description.as_str(),
                "when_to_use": validated.metadata.when_to_use.as_deref(),
                "input_schema": validated.metadata.input_schema.as_deref(),
                "phases": &validated.metadata.phases,
            },
            "args": args.args.as_ref(),
        }),
    }
}

fn workflow_guardian_request(
    call_id: &str,
    workflow_name: &str,
    source: &WorkflowSource,
    validated: &ValidatedWorkflowScript,
    args: &WorkflowArgs,
) -> GuardianApprovalRequest {
    GuardianApprovalRequest::Workflow {
        id: call_id.to_string(),
        workflow_name: workflow_name.to_string(),
        source_kind: workflow_source_kind_label(source.kind).to_string(),
        source_name: source.name.clone(),
        source_path: source.path.as_ref().map(|path| path.display().to_string()),
        metadata_name: validated.metadata.name.clone(),
        metadata_description: validated.metadata.description.clone(),
        args: args.args.clone(),
    }
}

async fn review_workflow_with_guardian(
    session: &Arc<crate::session::session::Session>,
    turn: &Arc<crate::session::turn_context::TurnContext>,
    call_id: &str,
    workflow_name: &str,
    source: &WorkflowSource,
    validated: &ValidatedWorkflowScript,
    args: &WorkflowArgs,
) -> (String, ReviewDecision) {
    let review_id = new_guardian_review_id();
    let decision = review_approval_request(
        session,
        turn,
        review_id.clone(),
        workflow_guardian_request(call_id, workflow_name, source, validated, args),
        /*retry_reason*/ None,
    )
    .await;
    (review_id, decision)
}

async fn apply_workflow_guardian_decision(
    session: &crate::session::session::Session,
    cache_key: Option<&WorkflowApprovalKey>,
    (review_id, decision): (String, ReviewDecision),
) -> Result<(), String> {
    match decision {
        ReviewDecision::Approved | ReviewDecision::ApprovedExecpolicyAmendment { .. } => Ok(()),
        ReviewDecision::ApprovedForSession => {
            if let Some(key) = cache_key {
                remember_workflow_approval(session, key.clone()).await;
            }
            Ok(())
        }
        ReviewDecision::NetworkPolicyAmendment {
            network_policy_amendment,
        } if network_policy_amendment.action == NetworkPolicyRuleAction::Allow => Ok(()),
        ReviewDecision::TimedOut => Err(guardian_timeout_message()),
        ReviewDecision::Denied
        | ReviewDecision::Abort
        | ReviewDecision::NetworkPolicyAmendment { .. } => {
            Err(guardian_rejection_message(session, &review_id).await)
        }
    }
}

async fn request_workflow_approval(
    session: &Arc<crate::session::session::Session>,
    turn: &Arc<crate::session::turn_context::TurnContext>,
    call_id: &str,
    question_id: &str,
    workflow_name: &str,
    source: &WorkflowSource,
    validated: &ValidatedWorkflowScript,
    args: &WorkflowArgs,
    cache_key: Option<&WorkflowApprovalKey>,
    persistent_name: Option<&str>,
) -> Option<RequestUserInputResponse> {
    let mut options = vec![RequestUserInputQuestionOption {
        label: WORKFLOW_APPROVAL_ALLOW.to_string(),
        description: "Run this workflow once.".to_string(),
    }];
    if cache_key.is_some() {
        options.push(RequestUserInputQuestionOption {
            label: WORKFLOW_APPROVAL_ALLOW_FOR_SESSION.to_string(),
            description: "Run this workflow and remember the choice for this session.".to_string(),
        });
    }
    if persistent_name.is_some() {
        options.push(RequestUserInputQuestionOption {
            label: WORKFLOW_APPROVAL_ALLOW_ALWAYS.to_string(),
            description: "Run this workflow and save a named workflow approval override."
                .to_string(),
        });
    }
    options.push(RequestUserInputQuestionOption {
        label: WORKFLOW_APPROVAL_CANCEL.to_string(),
        description: "Do not run this workflow.".to_string(),
    });

    let question = RequestUserInputQuestion {
        id: question_id.to_string(),
        header: "Approve workflow?".to_string(),
        question: workflow_approval_prompt_text(workflow_name, source, validated, args),
        is_other: false,
        is_secret: false,
        options: Some(options),
    };
    session
        .request_user_input(
            turn.as_ref(),
            call_id.to_string(),
            RequestUserInputArgs {
                questions: vec![question],
            },
        )
        .await
}

fn workflow_approval_prompt_text(
    workflow_name: &str,
    source: &WorkflowSource,
    validated: &ValidatedWorkflowScript,
    args: &WorkflowArgs,
) -> String {
    let mut lines = vec![
        format!("Allow workflow `{workflow_name}` to run?"),
        format!("Source: {}", workflow_source_kind_label(source.kind)),
    ];
    if let Some(path) = source.path.as_ref() {
        lines.push(format!("Path: `{}`", path.display()));
    }
    lines.push(format!("Metadata: `{}`", validated.metadata.name));
    lines.push(format!("Description: {}", validated.metadata.description));
    if let Some(when_to_use) = validated.metadata.when_to_use.as_deref() {
        lines.push(format!(
            "When to use: {}",
            workflow_approval_preview(when_to_use)
        ));
    }
    if !validated.metadata.phases.is_empty() {
        lines.push(format!(
            "Phases: {}",
            workflow_approval_phases_preview(validated)
        ));
    }
    if let Some(input_schema) = validated.metadata.input_schema.as_deref() {
        lines.push(format!(
            "Input schema:\n{}",
            workflow_approval_preview(input_schema)
        ));
    }
    if let Some(args) = args.args.as_ref().filter(|args| !args.is_null()) {
        let args = serde_json::to_string_pretty(args).unwrap_or_else(|_| args.to_string());
        lines.push(format!(
            "Args:\n{}",
            workflow_approval_preview(args.as_str())
        ));
    }
    if let Some(script_preview) = workflow_non_empty_str(source.code.as_str()) {
        lines.push(format!(
            "Script preview:\n{}",
            workflow_approval_preview(script_preview)
        ));
    }
    lines.join("\n")
}

fn workflow_approval_phases_preview(validated: &ValidatedWorkflowScript) -> String {
    let mut phases = validated
        .metadata
        .phases
        .iter()
        .take(6)
        .map(|phase| {
            if let Some(model) = phase.model.as_deref().and_then(workflow_non_empty_str) {
                format!("{} ({model})", phase.title)
            } else {
                phase.title.clone()
            }
        })
        .collect::<Vec<_>>();
    let remaining = validated.metadata.phases.len().saturating_sub(phases.len());
    if remaining > 0 {
        phases.push(format!("... {remaining} more"));
    }
    phases.join(", ")
}

fn workflow_approval_preview(value: &str) -> String {
    let value = value.trim();
    if value.chars().count() <= WORKFLOW_APPROVAL_PREVIEW_MAX_CHARS {
        return value.to_string();
    }
    let mut truncated = value
        .chars()
        .take(WORKFLOW_APPROVAL_PREVIEW_MAX_CHARS)
        .collect::<String>();
    truncated.push_str("... [truncated]");
    truncated
}

fn workflow_non_empty_str(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()).then_some(value)
}

fn parse_workflow_approval_response(
    response: Option<RequestUserInputResponse>,
    question_id: &str,
) -> WorkflowApprovalResponseDecision {
    let Some(response) = response else {
        return WorkflowApprovalResponseDecision::Cancel;
    };
    let Some(answer) = response.answers.get(question_id) else {
        return WorkflowApprovalResponseDecision::Cancel;
    };
    if answer
        .answers
        .iter()
        .any(|answer| answer == WORKFLOW_APPROVAL_ALLOW_ALWAYS)
    {
        return WorkflowApprovalResponseDecision::AllowAlways;
    }
    if answer
        .answers
        .iter()
        .any(|answer| answer == WORKFLOW_APPROVAL_ALLOW_FOR_SESSION)
    {
        return WorkflowApprovalResponseDecision::AllowForSession;
    }
    if answer
        .answers
        .iter()
        .any(|answer| answer == WORKFLOW_APPROVAL_ALLOW)
    {
        return WorkflowApprovalResponseDecision::Allow;
    }
    WorkflowApprovalResponseDecision::Cancel
}

async fn workflow_approval_is_remembered(
    session: &crate::session::session::Session,
    key: &WorkflowApprovalKey,
) -> bool {
    let store = session.services.tool_approvals.lock().await;
    matches!(
        store.get(key),
        Some(codex_protocol::protocol::ReviewDecision::ApprovedForSession)
    )
}

async fn remember_workflow_approval(
    session: &crate::session::session::Session,
    key: WorkflowApprovalKey,
) {
    let mut store = session.services.tool_approvals.lock().await;
    store.put(
        key,
        codex_protocol::protocol::ReviewDecision::ApprovedForSession,
    );
}

async fn persist_workflow_approval_allow(
    session: &crate::session::session::Session,
    turn: &crate::session::turn_context::TurnContext,
    workflow_name: &str,
    key: WorkflowApprovalKey,
) {
    let persist_result = ConfigEditsBuilder::for_config(turn.config.as_ref())
        .with_edits([ConfigEdit::SetPath {
            segments: vec![
                "workflows".to_string(),
                "named".to_string(),
                workflow_name.to_string(),
                "approval".to_string(),
            ],
            value: value("allow"),
        }])
        .apply()
        .await;

    if let Err(err) = persist_result {
        tracing::error!(
            error = %err,
            workflow_name,
            "failed to persist workflow approval"
        );
    } else {
        session.reload_user_config_layer().await;
    }

    remember_workflow_approval(session, key).await;
}

fn workflow_source_kind_label(kind: WorkflowSourceKind) -> &'static str {
    match kind {
        WorkflowSourceKind::Inline => "inline",
        WorkflowSourceKind::ScriptPath => "script_path",
        WorkflowSourceKind::Named => "named",
    }
}

async fn resolve_workflow_source(
    turn: &crate::session::turn_context::TurnContext,
    args: &WorkflowArgs,
) -> Result<WorkflowSource, FunctionCallError> {
    if let Some(script) = args
        .script
        .as_deref()
        .filter(|script| !script.trim().is_empty())
    {
        return Ok(WorkflowSource {
            name: args
                .title
                .as_deref()
                .or(args.name.as_deref())
                .unwrap_or("inline")
                .to_string(),
            code: script.to_string(),
            kind: WorkflowSourceKind::Inline,
            path: None,
        });
    }

    if let Some(script_path) = args
        .script_path
        .as_deref()
        .filter(|script_path| !script_path.trim().is_empty())
    {
        let path = resolve_script_path(turn, script_path).await?;
        let code = read_workflow_file(&path).await?;
        return Ok(WorkflowSource {
            name: args
                .title
                .as_deref()
                .or(args.name.as_deref())
                .unwrap_or_else(|| {
                    path.file_stem()
                        .and_then(|stem| stem.to_str())
                        .unwrap_or("file")
                })
                .to_string(),
            code,
            kind: WorkflowSourceKind::ScriptPath,
            path: Some(path),
        });
    }

    if let Some(name) = args.name.as_deref().filter(|name| !name.trim().is_empty()) {
        let path = find_named_workflow(turn, name).await?;
        let code = read_workflow_file(&path).await?;
        return Ok(WorkflowSource {
            name: name.to_string(),
            code,
            kind: WorkflowSourceKind::Named,
            path: Some(path),
        });
    }

    if args
        .resume_from_run_id
        .as_deref()
        .is_some_and(|run_id| !run_id.trim().is_empty())
    {
        return resolve_resume_workflow_source(turn, args).await;
    }

    Err(FunctionCallError::RespondToModel(
        "workflow requires one of `script`, `script_path`, `name`, or `resumeFromRunId`"
            .to_string(),
    ))
}

async fn resolve_script_path(
    turn: &crate::session::turn_context::TurnContext,
    script_path: &str,
) -> Result<PathBuf, FunctionCallError> {
    let cwd = workflow_cwd(turn).clone();
    let requested = Path::new(script_path);
    let candidate = if requested.is_absolute() {
        codex_utils_absolute_path::AbsolutePathBuf::from_absolute_path(requested)
            .map_err(|err| FunctionCallError::RespondToModel(err.to_string()))?
    } else {
        cwd.join(requested)
    };

    if let Some(path) = canonical_allowed_workflow_file(turn, candidate.as_path()).await {
        return Ok(path);
    }

    for dir in &turn.config.workflows.workflow_dirs {
        let candidate = dir.join(requested);
        if let Some(path) = canonical_allowed_workflow_file(turn, candidate.as_path()).await {
            return Ok(path);
        }
    }

    Err(FunctionCallError::RespondToModel(format!(
        "workflow file `{script_path}` is outside the project or configured workflow directories"
    )))
}

async fn resolve_resume_workflow_source(
    turn: &crate::session::turn_context::TurnContext,
    args: &WorkflowArgs,
) -> Result<WorkflowSource, FunctionCallError> {
    let (resume_run_id, snapshot) = read_resume_snapshot(turn, args).await?;
    let script_path = snapshot
        .get("script_path")
        .and_then(JsonValue::as_str)
        .map(str::trim)
        .filter(|script_path| !script_path.is_empty())
        .ok_or_else(|| {
            FunctionCallError::RespondToModel(format!(
                "workflow run `{resume_run_id}` cannot be resumed because it has no script_path"
            ))
        })?;
    let path = PathBuf::from(script_path);
    let path = if let Some(path) = canonical_allowed_workflow_file(turn, path.as_path()).await {
        path
    } else if let Some(path) =
        canonical_resume_artifact_script_file(turn, resume_run_id.as_str(), path.as_path()).await
    {
        path
    } else {
        return Err(FunctionCallError::RespondToModel(format!(
            "workflow run `{resume_run_id}` points at script `{script_path}`, which is outside the project, configured workflow directories, and its own run artifact directory"
        )));
    };
    let code = read_workflow_file(&path).await?;
    let source_kind = match snapshot
        .get("source")
        .and_then(|source| source.get("kind"))
        .and_then(JsonValue::as_str)
    {
        Some("inline") => WorkflowSourceKind::Inline,
        Some("named") => WorkflowSourceKind::Named,
        Some("script_path") => WorkflowSourceKind::ScriptPath,
        _ => WorkflowSourceKind::ScriptPath,
    };
    let name = args
        .title
        .as_deref()
        .or(args.name.as_deref())
        .or_else(|| {
            snapshot
                .get("workflow_name")
                .and_then(JsonValue::as_str)
                .and_then(non_empty_json_str)
        })
        .unwrap_or("resumed")
        .to_string();
    Ok(WorkflowSource {
        name,
        code,
        kind: source_kind,
        path: Some(path),
    })
}

async fn read_resume_snapshot(
    turn: &crate::session::turn_context::TurnContext,
    args: &WorkflowArgs,
) -> Result<(String, JsonValue), FunctionCallError> {
    let resume_run_id = validated_resume_run_id(args)?;
    let snapshot_path = workflow_run_snapshot_dir(turn).join(format!("{resume_run_id}.json"));
    let contents = tokio::fs::read_to_string(&snapshot_path)
        .await
        .map_err(|err| {
            FunctionCallError::RespondToModel(format!(
                "failed to read workflow resume snapshot {}: {err}",
                snapshot_path.display()
            ))
        })?;
    let snapshot = serde_json::from_str::<JsonValue>(&contents).map_err(|err| {
        FunctionCallError::RespondToModel(format!(
            "failed to parse workflow resume snapshot {}: {err}",
            snapshot_path.display()
        ))
    })?;
    Ok((resume_run_id, snapshot))
}

async fn inherit_resume_args(
    turn: &crate::session::turn_context::TurnContext,
    args: &mut WorkflowArgs,
) {
    if args.args.is_some()
        || args
            .resume_from_run_id
            .as_deref()
            .is_none_or(|run_id| run_id.trim().is_empty())
    {
        return;
    }
    let Ok((_run_id, snapshot)) = read_resume_snapshot(turn, args).await else {
        return;
    };
    if let Some(prior_args) = snapshot.get("args").filter(|args| !args.is_null()) {
        args.args = Some(prior_args.clone());
    }
}

fn validated_resume_run_id(args: &WorkflowArgs) -> Result<String, FunctionCallError> {
    let run_id = args
        .resume_from_run_id
        .as_deref()
        .map(str::trim)
        .filter(|run_id| !run_id.is_empty())
        .ok_or_else(|| {
            FunctionCallError::RespondToModel("resumeFromRunId cannot be empty".to_string())
        })?;
    if !is_safe_workflow_run_id(run_id) {
        return Err(FunctionCallError::RespondToModel(
            "resumeFromRunId must contain only letters, numbers, `_`, or `-`".to_string(),
        ));
    }
    Ok(run_id.to_string())
}

async fn canonical_resume_artifact_script_file(
    turn: &crate::session::turn_context::TurnContext,
    resume_run_id: &str,
    path: &Path,
) -> Option<PathBuf> {
    if path.file_name().and_then(|name| name.to_str()) != Some("script.js") {
        return None;
    }
    let canonical_path = canonical_existing_file(path).await?;
    let canonical_run_dir =
        tokio::fs::canonicalize(workflow_run_snapshot_dir(turn).join(resume_run_id))
            .await
            .ok()?;
    canonical_path
        .starts_with(canonical_run_dir)
        .then_some(canonical_path)
}

fn non_empty_json_str(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then_some(trimmed)
}

async fn find_named_workflow(
    turn: &crate::session::turn_context::TurnContext,
    name: &str,
) -> Result<PathBuf, FunctionCallError> {
    let (namespace, safe_name) = normalize_workflow_name(name)?;
    let mut candidates = Vec::new();
    if let Some(namespace) = namespace {
        for source in &turn.config.workflows.plugin_workflow_dirs {
            if source.namespace == namespace {
                candidates.push(source.dir.join(format!("{safe_name}.js")).to_path_buf());
                candidates.push(
                    source
                        .dir
                        .join(&safe_name)
                        .join("workflow.js")
                        .to_path_buf(),
                );
            }
        }
    } else {
        for dir in &turn.config.workflows.workflow_dirs {
            candidates.push(dir.join(format!("{safe_name}.js")).to_path_buf());
            candidates.push(dir.join(&safe_name).join("workflow.js").to_path_buf());
        }
    }

    for candidate in candidates {
        if let Some(path) = canonical_allowed_workflow_file(turn, candidate.as_path()).await {
            return Ok(path);
        }
    }

    Err(FunctionCallError::RespondToModel(format!(
        "workflow `{name}` was not found in configured workflow directories"
    )))
}

async fn collect_child_workflow_definitions(
    turn: &crate::session::turn_context::TurnContext,
    root_source: &WorkflowSource,
) -> Vec<ChildWorkflowDefinition> {
    let mut definitions = Vec::new();
    let mut seen_names = HashSet::new();
    let root_path = root_source.path.as_deref();

    for (namespace, dir) in workflow_definition_dirs(turn) {
        let mut candidates = Vec::new();
        let Ok(mut entries) = tokio::fs::read_dir(dir.as_path()).await else {
            continue;
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            let Ok(file_type) = entry.file_type().await else {
                continue;
            };
            if file_type.is_file() && path.extension().is_some_and(|ext| ext == "js") {
                if let Some(name) = path.file_stem().and_then(|stem| stem.to_str()) {
                    candidates.push((name.to_string(), path, false));
                }
            } else if file_type.is_dir() {
                let workflow_path = path.join("workflow.js");
                if canonical_allowed_workflow_file(turn, workflow_path.as_path())
                    .await
                    .is_some()
                    && let Some(name) = path.file_name().and_then(|name| name.to_str())
                {
                    candidates.push((name.to_string(), workflow_path, true));
                }
            }
        }

        candidates.sort_by(|left, right| left.0.cmp(&right.0).then(left.2.cmp(&right.2)));
        for (name, path, folder_style) in candidates {
            if !seen_names.insert(name.clone()) {
                continue;
            }
            if root_path.is_some_and(|root| root == path.as_path()) {
                continue;
            }
            let Ok(code) = read_workflow_file(&path).await else {
                continue;
            };
            let Ok(validated) = validate_workflow_script(&code) else {
                tracing::debug!(
                    path = %path.display(),
                    "skipping invalid child workflow definition"
                );
                continue;
            };
            definitions.push(ChildWorkflowDefinition {
                keys: child_workflow_reference_keys(
                    turn,
                    namespace.as_deref(),
                    dir.as_path(),
                    path.as_path(),
                    &name,
                    folder_style,
                ),
                metadata: validated.metadata,
                body: validated.body,
            });
        }
    }

    definitions
}

fn child_workflow_reference_keys(
    turn: &crate::session::turn_context::TurnContext,
    namespace: Option<&str>,
    workflow_dir: &Path,
    path: &Path,
    name: &str,
    folder_style: bool,
) -> Vec<String> {
    let mut keys = Vec::new();
    if let Some(namespace) = namespace {
        push_child_workflow_key(&mut keys, &format!("{namespace}:{name}"));
    } else {
        push_child_workflow_key(&mut keys, name);
        if folder_style {
            push_child_workflow_key(&mut keys, &format!("{name}/workflow.js"));
            push_child_workflow_key(&mut keys, &format!("./{name}/workflow.js"));
        } else {
            push_child_workflow_key(&mut keys, &format!("{name}.js"));
            push_child_workflow_key(&mut keys, &format!("./{name}.js"));
        }
    }

    push_child_workflow_key(&mut keys, &workflow_ref_string(path));
    for base in [workflow_cwd(turn).as_path(), workflow_dir] {
        if let Ok(relative) = path.strip_prefix(base) {
            let relative = workflow_ref_string(relative);
            push_child_workflow_key(&mut keys, &relative);
            push_child_workflow_key(&mut keys, &format!("./{relative}"));
        }
    }

    keys
}

fn push_child_workflow_key(keys: &mut Vec<String>, key: &str) {
    let normalized = key.trim().replace('\\', "/");
    if !normalized.is_empty() && !keys.iter().any(|existing| existing == &normalized) {
        keys.push(normalized);
    }
}

fn workflow_definition_dirs(
    turn: &crate::session::turn_context::TurnContext,
) -> Vec<(Option<String>, &codex_utils_absolute_path::AbsolutePathBuf)> {
    let mut dirs = Vec::new();
    dirs.extend(
        turn.config
            .workflows
            .workflow_dirs
            .iter()
            .map(|dir| (None, dir)),
    );
    dirs.extend(
        turn.config
            .workflows
            .plugin_workflow_dirs
            .iter()
            .map(|source| (Some(source.namespace.clone()), &source.dir)),
    );
    dirs
}

fn workflow_ref_string(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn normalize_workflow_name(name: &str) -> Result<(Option<String>, String), FunctionCallError> {
    let trimmed = name.trim().trim_end_matches(".js");
    let mut parts = trimmed.split(':');
    let first = parts.next().unwrap_or_default();
    let second = parts.next();
    if parts.next().is_some() {
        return Err(FunctionCallError::RespondToModel(
            "workflow name must be a plain file stem such as `release` or a plugin workflow such as `plugin:release`".to_string(),
        ));
    }
    let (namespace, workflow_name) = match second {
        Some(workflow_name) => (Some(first), workflow_name),
        None => (None, first),
    };
    if !is_valid_workflow_name_segment(workflow_name)
        || namespace.is_some_and(|namespace| !is_valid_workflow_name_segment(namespace))
    {
        return Err(FunctionCallError::RespondToModel(
            "workflow name must be a plain file stem such as `release` or a plugin workflow such as `plugin:release`".to_string(),
        ));
    }
    Ok((
        namespace.map(ToString::to_string),
        workflow_name.to_string(),
    ))
}

fn is_valid_workflow_name_segment(segment: &str) -> bool {
    !segment.is_empty()
        && !segment.starts_with('.')
        && !segment.contains('/')
        && !segment.contains('\\')
}

async fn read_workflow_file(path: &Path) -> Result<String, FunctionCallError> {
    tokio::fs::read_to_string(path).await.map_err(|err| {
        FunctionCallError::RespondToModel(format!(
            "failed to read workflow file {}: {err}",
            path.display()
        ))
    })
}

async fn canonical_allowed_workflow_file(
    turn: &crate::session::turn_context::TurnContext,
    path: &Path,
) -> Option<PathBuf> {
    let canonical_path = canonical_existing_file(path).await?;
    let roots = canonical_allowed_workflow_roots(turn).await;
    roots
        .iter()
        .any(|root| canonical_path.starts_with(root))
        .then_some(canonical_path)
}

async fn canonical_existing_file(path: &Path) -> Option<PathBuf> {
    let metadata = tokio::fs::metadata(path).await.ok()?;
    if !metadata.is_file() {
        return None;
    }
    tokio::fs::canonicalize(path).await.ok()
}

async fn canonical_allowed_workflow_roots(
    turn: &crate::session::turn_context::TurnContext,
) -> Vec<PathBuf> {
    let mut roots = vec![workflow_cwd(turn).to_path_buf()];
    roots.extend(
        turn.config
            .workflows
            .workflow_dirs
            .iter()
            .map(codex_utils_absolute_path::AbsolutePathBuf::to_path_buf),
    );
    roots.extend(
        turn.config
            .workflows
            .plugin_workflow_dirs
            .iter()
            .map(|source| source.dir.to_path_buf()),
    );

    let mut canonical_roots = Vec::new();
    for root in roots {
        if let Ok(canonical) = tokio::fs::canonicalize(root).await
            && !canonical_roots
                .iter()
                .any(|existing| existing == &canonical)
        {
            canonical_roots.push(canonical);
        }
    }
    canonical_roots
}

fn workflow_cwd(
    turn: &crate::session::turn_context::TurnContext,
) -> &codex_utils_absolute_path::AbsolutePathBuf {
    turn.environments.primary().map_or_else(
        || {
            #[allow(deprecated)]
            {
                &turn.cwd
            }
        },
        |environment| &environment.cwd,
    )
}

fn build_workflow_script(
    run_id: &str,
    args: &WorkflowArgs,
    validated: &ValidatedWorkflowScript,
    child_definitions: &[ChildWorkflowDefinition],
    agent_journal_entries: &[WorkflowAgentJournalEntry],
    workflow_output_budget_tokens: Option<usize>,
    concurrency_cap: usize,
    workflow_cwd: &Path,
    workflow_git_branch: Option<&str>,
    workflow_parent_thread_id: Option<&str>,
) -> Result<String, FunctionCallError> {
    let args_json =
        serde_json::to_string(&args.args.clone().unwrap_or(JsonValue::Null)).map_err(|err| {
            FunctionCallError::RespondToModel(format!("failed to serialize workflow args: {err}"))
        })?;
    let workflow_name = args
        .title
        .as_deref()
        .or(args.name.as_deref())
        .unwrap_or(validated.metadata.name.as_str());
    let workflow_description = args
        .description
        .clone()
        .unwrap_or_else(|| validated.metadata.description.clone());
    let name_json = serde_json::to_string(workflow_name).map_err(|err| {
        FunctionCallError::RespondToModel(format!("failed to serialize workflow name: {err}"))
    })?;
    let description_json = serde_json::to_string(&Some(workflow_description)).map_err(|err| {
        FunctionCallError::RespondToModel(format!(
            "failed to serialize workflow description: {err}"
        ))
    })?;
    let progress_type_json =
        serde_json::to_string(WORKFLOW_PROGRESS_NOTIFICATION_TYPE).map_err(|err| {
            FunctionCallError::RespondToModel(format!(
                "failed to serialize workflow progress notification type: {err}"
            ))
        })?;
    let input_schema_literal = validated.metadata.input_schema.as_deref().unwrap_or("null");
    let agent_journal_entries_json =
        serde_json::to_string(agent_journal_entries).map_err(|err| {
            FunctionCallError::RespondToModel(format!(
                "failed to serialize workflow agent journal entries: {err}"
            ))
        })?;
    let workflow_output_budget_json = serde_json::to_string(&workflow_output_budget_tokens)
        .map_err(|err| {
            FunctionCallError::RespondToModel(format!(
                "failed to serialize workflow output budget: {err}"
            ))
        })?;
    let run_id_json = serde_json::to_string(run_id).map_err(|err| {
        FunctionCallError::RespondToModel(format!("failed to serialize workflow run id: {err}"))
    })?;
    let workflow_cwd_json =
        serde_json::to_string(&workflow_cwd.display().to_string()).map_err(|err| {
            FunctionCallError::RespondToModel(format!("failed to serialize workflow cwd: {err}"))
        })?;
    let workflow_git_branch_json = serde_json::to_string(&workflow_git_branch).map_err(|err| {
        FunctionCallError::RespondToModel(format!("failed to serialize workflow git branch: {err}"))
    })?;
    let workflow_parent_thread_id_json = serde_json::to_string(&workflow_parent_thread_id)
        .map_err(|err| {
            FunctionCallError::RespondToModel(format!(
                "failed to serialize workflow parent thread id: {err}"
            ))
        })?;
    let child_definitions_source = render_child_workflow_definitions(child_definitions)?;

    Ok(format!(
        r#"
const workflowName = {name_json};
const workflowRunId = {run_id_json};
const workflowCwd = {workflow_cwd_json};
const workflowGitBranch = {workflow_git_branch_json};
const workflowParentThreadId = {workflow_parent_thread_id_json};
const workflowDescription = {description_json};
const args = {args_json};
const WORKFLOW_PROGRESS_NOTIFICATION_TYPE = {progress_type_json};
const __workflowInputSchema = {input_schema_literal};
const __workflowJournalEntries = {agent_journal_entries_json};
const __workflowOutputBudgetTotal = {workflow_output_budget_json};
const WORKFLOW_AGENT_CALL_CAP = 1000;
const WORKFLOW_AGENT_RETRY_CAP = 5;
const WORKFLOW_AGENT_CONTROL_POLL_MS = 10000;
const WORKFLOW_CONCURRENCY_CAP = {concurrency_cap};
const WORKFLOW_LOG_CAP = 1000;
const WORKFLOW_SEQUENCE_ITEM_CAP = 4096;
const __workflowState = {{ name: workflowName, description: workflowDescription, phases: [], agentCount: 0, childCount: 0, childDepth: 0, logPrefix: [], logCount: 0, logSuppressed: false, outputBudgetSpent: 0 }};
const __workflowDefinitions = new Map();
function __stringifyWorkflowValue(value) {{
  if (typeof value === "string") return value;
  if (value === undefined) return "undefined";
  try {{ return JSON.stringify(value, null, 2); }} catch (_) {{ return String(value); }}
}}
function __assertWorkflowJsonSerializable(value, path = "result", seen = new Set()) {{
  if (value === null) return;
  const type = typeof value;
  if (type === "string" || type === "boolean") return;
  if (type === "number") {{
    if (!Number.isFinite(value)) throw new Error(`workflow ${{path}} must be JSON-serializable; non-finite numbers are not allowed`);
    return;
  }}
  if (type === "undefined" || type === "function" || type === "symbol" || type === "bigint") {{
    throw new Error(`workflow ${{path}} must be JSON-serializable; ${{type}} is not allowed`);
  }}
  if (type !== "object") return;
  if (seen.has(value)) throw new Error(`workflow ${{path}} must be JSON-serializable; circular references are not allowed`);
  seen.add(value);
  if (Array.isArray(value)) {{
    for (let index = 0; index < value.length; index++) {{
      __assertWorkflowJsonSerializable(value[index], `${{path}}[${{index}}]`, seen);
    }}
  }} else {{
    for (const key of Object.keys(value)) {{
      __assertWorkflowJsonSerializable(value[key], `${{path}}.${{key}}`, seen);
    }}
  }}
  seen.delete(value);
}}
function __workflowReturnValue(value) {{
  __assertWorkflowJsonSerializable(value);
  return __stringifyWorkflowValue(value);
}}
function __workflowErrorMessage(error) {{
  if (error && typeof error === "object" && "message" in error) return String(error.message);
  return __stringifyWorkflowValue(error);
}}
function __workflowLogPrefix() {{
  return __workflowState.logPrefix.length > 0 ? `[${{__workflowState.logPrefix.join(" > ")}}] ` : "";
}}
function __workflowCurrentName() {{
  return __workflowState.logPrefix.at(-1) || workflowName;
}}
function __workflowProgress(event, fields = {{}}) {{
  const payload = {{ type: WORKFLOW_PROGRESS_NOTIFICATION_TYPE, event, workflow: __workflowCurrentName(), ...fields }};
  try {{ notify(JSON.stringify(payload)); }} catch (_) {{}}
}}
function __workflowMetrics(extra = {{}}) {{
  return {{
    agentCount: __workflowState.agentCount,
    childCount: __workflowState.childCount,
    logCount: __workflowState.logCount,
    logSuppressed: __workflowState.logSuppressed,
    ...extra,
  }};
}}
function __workflowApproxTokens(value) {{
  const textValue = String(value);
  const byteLength = typeof TextEncoder === "function"
    ? new TextEncoder().encode(textValue).length
    : textValue.length;
  return Math.ceil(byteLength / 4);
}}
function __workflowRecordBudgetOutput(value) {{
  __workflowState.outputBudgetSpent += __workflowApproxTokens(value);
}}
function __workflowOutputBudget() {{
  const total = __workflowOutputBudgetTotal ?? null;
  const spent = __workflowState.outputBudgetSpent;
  return {{
    total,
    spent,
    remaining: total === null ? Infinity : Math.max(0, total - spent),
  }};
}}
function __workflowEmitLog(message) {{
  __workflowRecordBudgetOutput(message);
  if (__workflowState.logCount < WORKFLOW_LOG_CAP - 1) {{
    __workflowState.logCount++;
    text(message);
    return;
  }}
  if (!__workflowState.logSuppressed) {{
    __workflowState.logCount++;
    __workflowState.logSuppressed = true;
    text(`${{__workflowLogPrefix()}}workflow log cap reached (${{WORKFLOW_LOG_CAP}}); further log output suppressed`);
  }}
}}
function __workflowStableStringify(value) {{
  if (value === null || typeof value !== "object") return JSON.stringify(value);
  if (Array.isArray(value)) return `[${{value.map(__workflowStableStringify).join(",")}}]`;
  return `{{${{Object.keys(value).sort().map((key) => `${{JSON.stringify(key)}}:${{__workflowStableStringify(value[key])}}`).join(",")}}}}`;
}}
function log(...parts) {{
  __workflowEmitLog(__workflowLogPrefix() + parts.map(__stringifyWorkflowValue).join(" "));
}}
const console = Object.freeze({{
  __proto__: null,
  log,
  info: log,
  warn: log,
  error: log,
  debug: log,
  dir: log,
}});
function phase(name, detail = undefined) {{
  __workflowState.phases.push({{ workflow: __workflowState.logPrefix.at(-1) || workflowName, name, detail }});
  __workflowProgress("phase", {{
    phase: String(name),
    message: detail === undefined ? undefined : __stringifyWorkflowValue(detail),
  }});
  log(`phase: ${{name}}${{detail === undefined ? "" : " " + __stringifyWorkflowValue(detail)}}`);
}}
function budget() {{
  return {{
    total: budget.total,
    spent: budget.spent(),
    remaining: budget.remaining(),
    output: budget.output,
    workflow: workflowName,
    phases: __workflowState.phases.slice(),
  }};
}}
budget.total = null;
budget.spent = function() {{ return 0; }};
budget.remaining = function() {{ return Infinity; }};
Object.defineProperty(budget, "output", {{
  enumerable: true,
  get: __workflowOutputBudget,
}});
function __workflowSchemaError(message) {{
  throw new Error(`workflow args do not match inputSchema: ${{message}}`);
}}
function __workflowSchemaPath(path, key) {{
  const name = String(key);
  return /^[A-Za-z_$][A-Za-z0-9_$]*$/.test(name) ? `${{path}}.${{name}}` : `${{path}}[${{JSON.stringify(name)}}]`;
}}
function __workflowSchemaTypeName(value) {{
  if (value === null) return "null";
  if (Array.isArray(value)) return "array";
  if (Number.isInteger(value)) return "integer";
  return typeof value;
}}
function __workflowSchemaTypes(schema) {{
  if (!schema || typeof schema !== "object" || !("type" in schema)) return [];
  return Array.isArray(schema.type) ? schema.type.map(String) : [String(schema.type)];
}}
function __workflowSchemaMatchesType(value, type) {{
  switch (type) {{
    case "null": return value === null;
    case "array": return Array.isArray(value);
    case "object": return value !== null && typeof value === "object" && !Array.isArray(value);
    case "integer": return Number.isInteger(value);
    case "number": return typeof value === "number" && Number.isFinite(value);
    case "string": return typeof value === "string";
    case "boolean": return typeof value === "boolean";
    default: return true;
  }}
}}
function __workflowSchemaEqual(left, right) {{
  return JSON.stringify(left) === JSON.stringify(right);
}}
function __workflowValidateSchemaValue(value, schema, path) {{
  if (!schema || typeof schema !== "object" || Array.isArray(schema)) return;
  const types = __workflowSchemaTypes(schema);
  if (types.length > 0 && !types.some((type) => __workflowSchemaMatchesType(value, type))) {{
    __workflowSchemaError(`${{path}} must be ${{types.join(" or ")}}, got ${{__workflowSchemaTypeName(value)}}`);
  }}
  if (Array.isArray(schema.enum) && !schema.enum.some((expected) => __workflowSchemaEqual(value, expected))) {{
    __workflowSchemaError(`${{path}} must be one of ${{schema.enum.map(__stringifyWorkflowValue).join(", ")}}`);
  }}
  if ("const" in schema && !__workflowSchemaEqual(value, schema.const)) {{
    __workflowSchemaError(`${{path}} must equal ${{__stringifyWorkflowValue(schema.const)}}`);
  }}
  const requiresObject = schema.properties && typeof schema.properties === "object" || Array.isArray(schema.required);
  if (requiresObject) {{
    if (value === null || typeof value !== "object" || Array.isArray(value)) {{
      __workflowSchemaError(`${{path}} must be object, got ${{__workflowSchemaTypeName(value)}}`);
    }}
    if (Array.isArray(schema.required)) {{
      for (const key of schema.required.map(String)) {{
        if (!(key in value)) __workflowSchemaError(`${{__workflowSchemaPath(path, key)}} is required`);
      }}
    }}
    if (schema.properties && typeof schema.properties === "object") {{
      for (const [key, childSchema] of Object.entries(schema.properties)) {{
        if (key in value) __workflowValidateSchemaValue(value[key], childSchema, __workflowSchemaPath(path, key));
      }}
    }}
  }}
  if (schema.items !== undefined) {{
    if (!Array.isArray(value)) __workflowSchemaError(`${{path}} must be array, got ${{__workflowSchemaTypeName(value)}}`);
    for (let index = 0; index < value.length; index++) {{
      __workflowValidateSchemaValue(value[index], schema.items, `${{path}}[${{index}}]`);
    }}
  }}
}}
function __workflowValidateInputArgs() {{
  if (__workflowInputSchema !== null && __workflowInputSchema !== undefined) {{
    __workflowValidateSchemaValue(args, __workflowInputSchema, "args");
  }}
}}
function __registerWorkflowDefinition(keys, definition) {{
  for (const key of keys) {{
    const normalized = String(key).trim().replaceAll("\\", "/");
    if (normalized && !__workflowDefinitions.has(normalized)) __workflowDefinitions.set(normalized, definition);
  }}
}}
function __lookupWorkflowDefinition(ref) {{
  const raw = __workflowReferenceString(ref).trim().replaceAll("\\", "/");
  if (!raw) return undefined;
  const candidates = [raw];
  if (raw.startsWith("./")) candidates.push(raw.slice(2));
  if (raw.endsWith(".js")) candidates.push(raw.slice(0, -3));
  for (const candidate of candidates) {{
    const definition = __workflowDefinitions.get(candidate);
    if (definition) return definition;
  }}
  return undefined;
}}
function __workflowReferenceString(ref) {{
  if (typeof ref === "string") return ref;
  if (ref && typeof ref === "object" && typeof ref.name === "string") return ref.name;
  if (ref && typeof ref === "object" && typeof ref.scriptPath === "string") return ref.scriptPath;
  if (ref && typeof ref === "object" && typeof ref.script_path === "string") return ref.script_path;
  return "";
}}
function __workflowTool(name) {{
  if (tools[name]) return tools[name];
  const match = ALL_TOOLS.find((tool) =>
    tool.name === name || tool.name.endsWith("__" + name) || tool.name.endsWith("." + name)
  );
  if (!match || !tools[match.name]) throw new Error(`Workflow tool not available: ${{name}}`);
  return tools[match.name];
}}
function __reserveWorkflowAgentSlot() {{
  __workflowState.agentCount++;
  if (__workflowState.agentCount > WORKFLOW_AGENT_CALL_CAP) {{
    throw new Error(
      `Workflow agent() call cap reached (${{WORKFLOW_AGENT_CALL_CAP}}). This usually means a loop using budget.remaining() never terminates because no token budget was set; add a hard iteration cap.`
    );
  }}
  return __workflowState.agentCount;
}}
function __workflowAgentTaskName(value, fallback) {{
  const raw = value === undefined || value === null ? "" : String(value);
  const normalized = raw
    .toLowerCase()
    .replace(/[^a-z0-9_]+/g, "_")
    .replace(/^_+|_+$/g, "")
    .slice(0, 80);
  if (!normalized || normalized === "root" || normalized === "." || normalized === "..") return fallback;
  return normalized;
}}
function __workflowAgentRetryLimit(options) {{
  const raw = options.retries ?? options.max_retries ?? options.maxRetries ?? options.retry;
  if (raw === undefined || raw === true) return WORKFLOW_AGENT_RETRY_CAP;
  if (raw === false || raw === null) return 0;
  const value = Number(raw);
  if (!Number.isFinite(value) || value < 0) throw new Error("agent({{retries}}) must be a non-negative number");
  return Math.min(Math.floor(value), WORKFLOW_AGENT_RETRY_CAP);
}}
function __workflowAgentControlPollMs(options, waitTimeoutMs) {{
  const raw = options.control_poll_ms ?? options.controlPollMs ?? WORKFLOW_AGENT_CONTROL_POLL_MS;
  if (raw === false || raw === null) return waitTimeoutMs;
  const value = Number(raw);
  if (!Number.isFinite(value) || value <= 0) throw new Error("agent({{controlPollMs}}) must be a positive number");
  return Math.min(Math.max(Math.floor(value), 1000), Math.max(waitTimeoutMs, 1000));
}}
function __workflowAgentProgressStallMs(options, waitTimeoutMs, controlPollMs) {{
  const raw = options.progress_stall_ms ?? options.progressStallMs ?? options.stall_warning_ms ?? options.stallWarningMs;
  if (raw === false || raw === null) return null;
  let value;
  if (raw === undefined) {{
    value = Math.min(60000, Math.max(controlPollMs, Math.floor(waitTimeoutMs / 2)));
  }} else {{
    value = Number(raw);
    if (!Number.isFinite(value) || value <= 0) throw new Error("agent({{progressStallMs}}) must be a positive number, false, or null");
    value = Math.floor(value);
  }}
  if (value >= waitTimeoutMs) return null;
  return Math.max(value, 1000);
}}
function __workflowAgentAttemptTaskName(base, retryIndex) {{
  if (retryIndex === 0) return base;
  const suffix = `_retry_${{retryIndex}}`;
  const stemLength = Math.max(1, 96 - suffix.length);
  return `${{String(base).slice(0, stemLength)}}${{suffix}}`;
}}
const __workflowAgentJournal = new Map();
const __workflowAgentJournalStarted = new Map();
const __workflowChildJournal = new Map();
for (const entry of __workflowJournalEntries) {{
  if (!entry || typeof entry.key !== "string") continue;
  if (entry.type === "child_result" && Object.prototype.hasOwnProperty.call(entry, "result")) {{
    __workflowChildJournal.set(entry.key, entry.result);
  }} else if ((entry.type === undefined || entry.type === "result") && Object.prototype.hasOwnProperty.call(entry, "result")) {{
    __workflowAgentJournal.set(entry.key, entry.result);
  }} else if (entry.type === "started") {{
    __workflowAgentJournalStarted.set(entry.key, (__workflowAgentJournalStarted.get(entry.key) ?? 0) + 1);
  }}
}}
let __workflowAgentJournalPriorKey = "";
let __workflowChildJournalPriorKey = "";
let __workflowAgentJournalCacheOpen = true;
function __workflowAgentJournalEnabled(options, returnMetadata) {{
  return !returnMetadata && options.wait !== false && options.journal !== false && options.cache !== false;
}}
function __workflowHashString(value) {{
  let hash = 0x811c9dc5;
  for (let index = 0; index < value.length; index++) {{
    hash ^= value.charCodeAt(index);
    hash = Math.imul(hash, 0x01000193);
  }}
  return (hash >>> 0).toString(16).padStart(8, "0");
}}
function __workflowAgentJournalOptions(options, request) {{
  const normalized = {{}};
  const agentType = options.agentType ?? options.agent_type ?? request.agent_type;
  if (agentType !== undefined && typeof agentType !== "function") normalized.agentType = agentType;
  if (request.model !== undefined && typeof request.model !== "function") normalized.model = request.model;
  if (request.isolation !== undefined && typeof request.isolation !== "function") normalized.isolation = request.isolation;
  if (request.schema !== undefined && typeof request.schema !== "function") normalized.schema = request.schema;
  return normalized;
}}
function __workflowAgentJournalKey(message, options, request) {{
  const keyRequest = {{ ...request }};
  delete keyRequest.task_name;
  const legacy = __workflowStableStringify(keyRequest);
  const material = __workflowStableStringify({{
    options: __workflowAgentJournalOptions(options, request),
    priorKey: __workflowAgentJournalPriorKey,
    prompt: message,
  }});
  return {{ key: `codex-v2:${{__workflowHashString(material)}}`, legacy }};
}}
function __workflowAgentJournalLookup(journalKey) {{
  if (!journalKey || !__workflowAgentJournalCacheOpen) return undefined;
  if (__workflowAgentJournal.has(journalKey.key)) return __workflowAgentJournal.get(journalKey.key);
  if (journalKey.legacy && __workflowAgentJournal.has(journalKey.legacy)) return __workflowAgentJournal.get(journalKey.legacy);
  __workflowAgentJournalCacheOpen = false;
  return undefined;
}}
function __workflowAgentJournalStartedAttempts(journalKey) {{
  if (!journalKey) return 0;
  return (__workflowAgentJournalStarted.get(journalKey.key) ?? 0)
    + (journalKey.legacy ? (__workflowAgentJournalStarted.get(journalKey.legacy) ?? 0) : 0);
}}
function __workflowRecordAgentJournal(journalKey, agentName, message, result) {{
  if (!journalKey || result === undefined) return;
  __workflowProgress("agent_journal_entry", {{
    agent: agentName,
    message,
    data: {{ key: journalKey.key, result }},
  }});
}}
function __workflowRecordAgentJournalStarted(journalKey, agentName, spawn) {{
  if (!journalKey) return;
  const agentId = spawn && (spawn.task_name ?? spawn.agentId ?? spawn.agent_id ?? spawn.id);
  __workflowProgress("agent_journal_started", {{
    agent: agentName,
    data: {{ key: journalKey.key, agentId: agentId === undefined ? "" : String(agentId) }},
  }});
}}
function __workflowAgentIdFromSpawn(spawn, fallback) {{
  if (!spawn || typeof spawn !== "object") return fallback;
  return spawn.task_name ?? spawn.agentId ?? spawn.agent_id ?? spawn.id ?? fallback;
}}
function __workflowAgentTranscriptMetadata(spawn, request, message) {{
  const agentId = __workflowAgentIdFromSpawn(spawn, request.task_name);
  const metadata = {{
    agentId: agentId === undefined ? "" : String(agentId),
    taskName: request.task_name === undefined ? "" : String(request.task_name),
    name: request.task_name === undefined ? "" : String(request.task_name),
    agentName: request.task_name === undefined ? String(agentId ?? "") : String(request.task_name),
    sessionKind: "workflow_agent",
    runId: workflowRunId,
    cwd: workflowCwd,
  }};
  if (workflowGitBranch !== null && workflowGitBranch !== undefined && workflowGitBranch !== "") metadata.gitBranch = String(workflowGitBranch);
  if (workflowParentThreadId !== null && workflowParentThreadId !== undefined && workflowParentThreadId !== "") metadata.parentThreadId = String(workflowParentThreadId);
  if (request.agent_type !== undefined) metadata.agentType = String(request.agent_type);
  if (request.model !== undefined) metadata.model = String(request.model);
  if (request.reasoning_effort !== undefined) metadata.reasoningEffort = String(request.reasoning_effort);
  if (request.service_tier !== undefined) metadata.serviceTier = String(request.service_tier);
  if (request.isolation !== undefined) metadata.isolation = String(request.isolation);
  if (spawn && typeof spawn === "object" && spawn.nickname !== undefined && spawn.nickname !== null) metadata.nickname = String(spawn.nickname);
  if (spawn && typeof spawn === "object") {{
    const toolUseId = spawn.tool_use_id ?? spawn.toolUseId;
    if (toolUseId !== undefined && toolUseId !== null) metadata.toolUseId = String(toolUseId);
    const worktreePath = spawn.worktree_path ?? spawn.worktreePath;
    if (worktreePath !== undefined && worktreePath !== null) metadata.worktreePath = String(worktreePath);
  }}
  if (message && typeof message === "object") {{
    if (message.author !== undefined) metadata.author = String(message.author);
    if (message.recipient !== undefined) metadata.recipient = String(message.recipient);
  }}
  return metadata;
}}
const __workflowAgentTranscriptStarted = new Set();
function __workflowRecordAgentTranscriptStart(agentName, message, spawn, request) {{
  const agentId = __workflowAgentIdFromSpawn(spawn, agentName);
  const key = agentId === undefined ? "" : String(agentId);
  if (!key || __workflowAgentTranscriptStarted.has(key)) return;
  __workflowAgentTranscriptStarted.add(key);
  const data = {{
    agentId: key,
    prompt: message,
    metadata: __workflowAgentTranscriptMetadata(spawn, request, null),
  }};
  __workflowProgress("agent_transcript_entry", {{
    agent: agentName,
    message,
    data,
  }});
}}
function __workflowRecordAgentTranscript(agentName, message, details) {{
  if (!details) return;
  const spawn = details.spawn ?? {{}};
  const agentId = __workflowAgentIdFromSpawn(spawn, agentName);
  const agentKey = agentId === undefined ? "" : String(agentId);
  const data = {{ agentId: agentKey, prompt: message }};
  if (agentKey && __workflowAgentTranscriptStarted.has(agentKey)) data.promptRecorded = true;
  const liveTranscript = Boolean(spawn && (spawn.workflow_live_transcript || spawn.workflowLiveTranscript));
  if (liveTranscript) data.transcriptRecorded = true;
  if (details.finalMessage !== undefined && details.finalMessage !== null) data.finalText = String(details.finalMessage);
  else if (details.result !== undefined && details.result !== null) data.result = details.result;
  if (details.metadata && typeof details.metadata === "object") data.metadata = details.metadata;
  if (!liveTranscript && Array.isArray(details.transcript) && details.transcript.length > 0) data.transcript = details.transcript;
  if (Array.isArray(details.toolCalls) && details.toolCalls.length > 0) data.toolCalls = details.toolCalls;
  if (Array.isArray(details.reasoning) && details.reasoning.length > 0) data.reasoning = details.reasoning;
  __workflowProgress("agent_transcript_entry", {{
    agent: agentName,
    message,
    data,
  }});
}}
function __workflowChildJournalKey(definition, childArgs, childRef) {{
  const material = __workflowStableStringify({{
    args: childArgs === undefined ? null : childArgs,
    name: definition.name,
    priorKey: __workflowChildJournalPriorKey,
    reference: __workflowReferenceString(childRef),
  }});
  return `codex-child-v1:${{__workflowHashString(material)}}`;
}}
function __workflowRecordChildJournal(journalKey, definition, childRunId, result) {{
  if (!journalKey || result === undefined) return;
  try {{
    __assertWorkflowJsonSerializable(result, `child workflow ${{definition.name}} result`);
  }} catch (_) {{
    return;
  }}
  __workflowProgress("child_journal_entry", {{
    child: definition.name,
    childRunId,
    data: {{ key: journalKey, childRunId, result }},
  }});
}}
async function __workflowInterruptAgent(target, reason) {{
  try {{
    const interrupt = __workflowTool("interrupt_agent");
    await interrupt({{ target, reason }});
  }} catch (error) {{
    log(`agent "${{target}}" ${{reason}}; interrupt failed: ${{__workflowErrorMessage(error)}}`);
  }}
}}
async function __workflowAgentControlState(agentId) {{
  try {{
    const control = __workflowTool("workflow_control");
    const state = await control({{ run_id: workflowRunId, agent_id: String(agentId) }});
    if (
      state
      && typeof state === "object"
      && (state.action === "skip" || state.action === "retry")
    ) {{
      return state;
    }}
  }} catch (error) {{
    log(`workflow control check failed: ${{__workflowErrorMessage(error)}}`);
  }}
  return null;
}}
function __workflowAgentOwnMessage(spawn, waited, request) {{
  const messages = Array.isArray(waited && waited.messages) ? waited.messages : [];
  const taskName = spawn && typeof spawn.task_name === "string" ? spawn.task_name : request.task_name;
  const spawnedAgentId = __workflowAgentIdFromSpawn(spawn, taskName);
  return messages.find((message) => {{
    if (!message || typeof message !== "object") return false;
    return message.author === taskName || message.author === spawnedAgentId || message.author === String(spawnedAgentId);
  }}) ?? null;
}}
function __workflowAgentStatusIsFinal(status) {{
  if (status === "interrupted") return true;
  if (!status || typeof status !== "object") return false;
  return Object.prototype.hasOwnProperty.call(status, "completed")
    || Object.prototype.hasOwnProperty.call(status, "errored");
}}
function __workflowAgentMessageIsFinal(message) {{
  if (!message || typeof message !== "object") return false;
  if (typeof message.final_message === "string") return true;
  return __workflowAgentStatusIsFinal(message.status);
}}
async function __workflowWaitAgentWithControl(wait, spawn, request, waitTimeoutMs, controlPollMs, progressStallMs) {{
  const spawnedAgentId = __workflowAgentIdFromSpawn(spawn, request.task_name);
  let remainingMs = waitTimeoutMs;
  let elapsedMs = 0;
  let progressStallSent = false;
  let waited = {{ timed_out: true, messages: [] }};
  while (remainingMs > 0) {{
    const before = await __workflowAgentControlState(spawnedAgentId);
    if (before) return {{ waited, control: before }};
    const intervalMs = Math.min(controlPollMs, remainingMs);
    waited = await wait({{ timeout_ms: intervalMs, include_messages: true }});
    const after = await __workflowAgentControlState(spawnedAgentId);
    if (after) return {{ waited, control: after }};
    if (__workflowAgentMessageIsFinal(__workflowAgentOwnMessage(spawn, waited, request))) {{
      return {{ waited, control: null }};
    }}
    remainingMs -= intervalMs;
    elapsedMs += intervalMs;
    if (!progressStallSent && progressStallMs !== null && elapsedMs >= progressStallMs && remainingMs > 0) {{
      progressStallSent = true;
      __workflowProgress("agent_waiting", {{
        agent: request.task_name,
        agentId: String(spawnedAgentId),
        message: `no agent update for ${{Math.round(elapsedMs / 1000)}}s`,
        data: {{ elapsedMs, timeoutMs: waitTimeoutMs }},
      }});
    }}
  }}
  return {{ waited: {{ timed_out: true, messages: [] }}, control: null }};
}}
function __workflowAgentFinalMessage(message) {{
  if (!message || typeof message !== "object") return undefined;
  if (typeof message.final_message === "string") return message.final_message;
  const status = message.status;
  if (status && typeof status === "object" && typeof status.completed === "string") return status.completed;
  return undefined;
}}
function __workflowAgentResult(spawn, waited, request, returnMetadata = false) {{
  const taskName = spawn && typeof spawn.task_name === "string" ? spawn.task_name : request.task_name;
  const ownMessage = __workflowAgentOwnMessage(spawn, waited, request);
  const finalMessage = __workflowAgentFinalMessage(ownMessage);
  const toolCalls = Array.isArray(ownMessage && ownMessage.tool_calls) ? ownMessage.tool_calls : [];
  const reasoning = Array.isArray(ownMessage && ownMessage.reasoning) ? ownMessage.reasoning : [];
  const transcript = Array.isArray(ownMessage && ownMessage.transcript) ? ownMessage.transcript : [];
  const metadata = __workflowAgentTranscriptMetadata(spawn, request, ownMessage);
  let result = finalMessage ?? (ownMessage ? ownMessage.content : null);
  if (waited && waited.timed_out) {{
    throw new Error(`agent(${{JSON.stringify(taskName)}}) timed out waiting for completion`);
  }}
  if (request.schema && typeof finalMessage === "string") {{
    try {{
      result = JSON.parse(finalMessage);
    }} catch (error) {{
      throw new Error(`agent({{schema}}) returned non-JSON final output: ${{__workflowErrorMessage(error)}}`);
    }}
  }}
  const details = {{ spawn, wait: waited, message: ownMessage, finalMessage, result, toolCalls, reasoning, transcript, metadata }};
  return returnMetadata ? details : result;
}}
function __assertWorkflowSequenceLimit(items, helperName) {{
  if (!Array.isArray(items)) throw new Error(`${{helperName}} expects an array`);
  if (items.length > WORKFLOW_SEQUENCE_ITEM_CAP) {{
    throw new Error(`${{helperName}} accepts at most ${{WORKFLOW_SEQUENCE_ITEM_CAP}} items`);
  }}
}}
function __assertWorkflowParallelItems(items) {{
  __assertWorkflowSequenceLimit(items, "parallel");
  for (let index = 0; index < items.length; index++) {{
    const item = items[index];
    if (typeof item === "function") continue;
    if (Array.isArray(item)) {{
      __assertWorkflowSequenceLimit(item, "parallel item pipeline");
      for (let stepIndex = 0; stepIndex < item.length; stepIndex++) {{
        if (typeof item[stepIndex] !== "function") {{
          throw new TypeError(`parallel() item ${{index + 1}} pipeline step ${{stepIndex + 1}} must be a function`);
        }}
      }}
      continue;
    }}
    throw new TypeError("parallel() expects an array of functions or step arrays. Wrap each call: () => agent(...)");
  }}
}}
async function __runWorkflowLimited(items, runner) {{
  if (items.length === 0) return [];
  const results = new Array(items.length);
  let nextIndex = 0;
  const workerCount = Math.min(WORKFLOW_CONCURRENCY_CAP, items.length);
  async function worker() {{
    while (true) {{
      const index = nextIndex++;
      if (index >= items.length) return;
      try {{
        results[index] = {{ status: "fulfilled", value: await runner(items[index], index) }};
      }} catch (error) {{
        results[index] = {{ status: "rejected", reason: error }};
      }}
    }}
  }}
  await Promise.all(Array.from({{ length: workerCount }}, () => worker()));
  return results;
}}
async function agent(task, options = {{}}) {{
  const message = typeof task === "string" ? task : __stringifyWorkflowValue(task);
  if (options.phase) phase(String(options.phase));
  const agentIndex = __reserveWorkflowAgentSlot();
  const fallbackTaskName = __workflowAgentTaskName(`workflow_${{workflowName}}_${{agentIndex}}`, `workflow_${{agentIndex}}`);
  const baseTaskName = __workflowAgentTaskName(options.label ?? options.task_name ?? options.name, fallbackTaskName);
  const waitTimeoutMs = options.timeout_ms ?? options.timeoutMs ?? options.stall_ms ?? options.stallMs ?? 180000;
  const returnMetadata = Boolean(options.return_metadata ?? options.returnMetadata ?? false);
  const retryLimit = __workflowAgentRetryLimit(options);
  const baseRequest = {{
    message,
    agent_type: options.agent_type ?? options.agentType,
    model: options.model,
    reasoning_effort: options.reasoning_effort ?? options.reasoningEffort,
    service_tier: options.service_tier ?? options.serviceTier,
    isolation: options.isolation,
    fork_turns: options.fork_turns ?? options.forkTurns,
    schema: options.schema ?? options.output_schema ?? options.outputSchema ?? options.json_schema ?? options.jsonSchema,
  }};
  for (const key of Object.keys(baseRequest)) if (baseRequest[key] === undefined) delete baseRequest[key];
  const journalKey = __workflowAgentJournalEnabled(options, returnMetadata)
    ? __workflowAgentJournalKey(message, options, baseRequest)
    : null;
  if (journalKey) __workflowAgentJournalPriorKey = journalKey.key;
  const journalCacheWasOpen = __workflowAgentJournalCacheOpen;
  const journalValue = __workflowAgentJournalLookup(journalKey);
  if (journalValue !== undefined) {{
    __workflowProgress("agent_journal_hit", {{ agent: baseTaskName, message }});
    return journalValue;
  }}
  const startedAttempts = journalCacheWasOpen ? __workflowAgentJournalStartedAttempts(journalKey) : 0;
  if (startedAttempts > 0) {{
    __workflowProgress("agent_journal_started_hit", {{
      agent: baseTaskName,
      message: `${{startedAttempts}} prior started attempt${{startedAttempts === 1 ? "" : "s"}}; respawning`,
    }});
  }}
  for (let retryIndex = 0; retryIndex <= WORKFLOW_AGENT_RETRY_CAP; retryIndex++) {{
    const task_name = __workflowAgentAttemptTaskName(baseTaskName, retryIndex);
    const request = {{
      ...baseRequest,
      task_name,
    }};
    __workflowProgress("agent_start", {{ agent: task_name, message }});
    let spawn;
    try {{
      spawn = await __workflowTool("spawn_agent")(request);
    }} catch (error) {{
      __workflowProgress("agent_failed", {{ agent: task_name, message: __workflowErrorMessage(error) }});
      throw error;
    }}
    const spawnedAgentId = __workflowAgentIdFromSpawn(spawn, task_name);
    __workflowProgress("agent_start", {{ agent: task_name, agentId: String(spawnedAgentId), message }});
    __workflowRecordAgentJournalStarted(journalKey, task_name, spawn);
    __workflowRecordAgentTranscriptStart(task_name, message, spawn, request);
    if (options.wait === false) {{
      __workflowProgress("agent_detached", {{ agent: task_name, agentId: String(spawnedAgentId), message: "wait:false" }});
      return spawn;
    }}
    const wait = __workflowTool("wait_agent");
    const controlPollMs = __workflowAgentControlPollMs(options, waitTimeoutMs);
    const waitOutcome = await __workflowWaitAgentWithControl(
      wait,
      spawn,
      request,
      waitTimeoutMs,
      controlPollMs,
      __workflowAgentProgressStallMs(options, waitTimeoutMs, controlPollMs)
    );
    const waited = waitOutcome.waited;
    if (waitOutcome.control && waitOutcome.control.action === "skip") {{
      await __workflowInterruptAgent(spawnedAgentId, "user-skip");
      __workflowProgress("agent_skipped", {{ agent: task_name, agentId: String(spawnedAgentId), message: waitOutcome.control.message ?? "skip requested" }});
      log(`[skip] agent "${{task_name}}" skipped by user request`);
      if (returnMetadata) {{
        return {{ spawn, wait: waited, message: null, finalMessage: null, result: null, control: waitOutcome.control, toolCalls: [], reasoning: [] }};
      }}
      return null;
    }}
    if (waitOutcome.control && waitOutcome.control.action === "retry") {{
      await __workflowInterruptAgent(spawnedAgentId, "user-retry");
      if (retryIndex < WORKFLOW_AGENT_RETRY_CAP) {{
        __workflowProgress("agent_retry", {{ agent: task_name, agentId: String(spawnedAgentId), message: waitOutcome.control.message ?? "user requested retry" }});
        log(`[retry] agent "${{task_name}}" retry requested by user`);
        continue;
      }}
      throw new Error(`agent(${{JSON.stringify(task_name)}}) retry requested but retry cap ${{WORKFLOW_AGENT_RETRY_CAP}} is reached`);
    }}
    if (waited && waited.timed_out) {{
      await __workflowInterruptAgent(spawn && spawn.task_name ? spawn.task_name : request.task_name, "stalled");
      __workflowProgress("agent_stalled", {{ agent: task_name, agentId: String(spawnedAgentId), message: `timed out after ${{Math.round(waitTimeoutMs / 1000)}}s` }});
      if (retryIndex < retryLimit) {{
        __workflowProgress("agent_retry", {{ agent: task_name, agentId: String(spawnedAgentId), message: `${{retryIndex + 1}}/${{retryLimit}}` }});
        log(`[stall] agent "${{task_name}}" timed out after ${{Math.round(waitTimeoutMs / 1000)}}s - retrying (${{retryIndex + 1}}/${{retryLimit}})`);
        continue;
      }}
    }}
    try {{
      const details = __workflowAgentResult(spawn, waited, request, true);
      const result = returnMetadata ? details : details.result;
      __workflowRecordAgentTranscript(task_name, message, details);
      __workflowRecordAgentJournal(journalKey, task_name, message, result);
      __workflowProgress("agent_complete", {{ agent: task_name, agentId: String(spawnedAgentId) }});
      return result;
    }} catch (error) {{
      __workflowProgress("agent_failed", {{ agent: task_name, agentId: String(spawnedAgentId), message: __workflowErrorMessage(error) }});
      throw error;
    }}
  }}
  throw new Error("agent retry loop exited unexpectedly");
}}
{child_definitions_source}
async function __runPipelineSteps(steps, initialValue = undefined) {{
  __assertWorkflowSequenceLimit(steps, "pipeline");
  let value = initialValue;
  for (let index = 0; index < steps.length; index++) {{
    try {{
      value = await steps[index](value);
    }} catch (error) {{
      const errorMessage = __workflowErrorMessage(error);
      log(`pipeline step ${{index + 1}} failed: ${{errorMessage}}`);
      __workflowProgress("pipeline_failed", {{
        message: `step ${{index + 1}}: ${{errorMessage}}`,
        data: {{ stepIndex: index + 1, error: errorMessage }},
      }});
      return null;
    }}
  }}
  return value;
}}
async function __runPipelineItemsWithoutStages(items) {{
  __assertWorkflowSequenceLimit(items, "pipeline");
  if (items.every((item) => typeof item === "function")) return await __runPipelineSteps(items);
  const settled = await __runWorkflowLimited(items, async (item) => await item);
  return settled.map((result, index) => {{
    if (result.status === "fulfilled") return result.value;
    const errorMessage = __workflowErrorMessage(result.reason);
    log(`pipeline item ${{index + 1}} failed: ${{errorMessage}}`);
    __workflowProgress("pipeline_failed", {{
      message: `item ${{index + 1}}: ${{errorMessage}}`,
      data: {{ itemIndex: index + 1, error: errorMessage }},
    }});
    return null;
  }});
}}
async function __runParallelItem(item) {{
  if (Array.isArray(item)) return await __runPipelineSteps(item);
  return await (typeof item === "function" ? item() : item);
}}
async function parallel(items) {{
  __assertWorkflowParallelItems(items);
  const settled = await __runWorkflowLimited(items, (item) => __runParallelItem(item));
  return settled.map((result, index) => {{
    if (result.status === "fulfilled") return result.value;
    const errorMessage = __workflowErrorMessage(result.reason);
    log(`parallel item ${{index + 1}} failed: ${{errorMessage}}`);
    __workflowProgress("parallel_failed", {{
      message: `item ${{index + 1}}: ${{errorMessage}}`,
      data: {{ itemIndex: index + 1, error: errorMessage }},
    }});
    return null;
  }});
}}
async function pipeline(itemsOrSteps, ...stages) {{
  __assertWorkflowSequenceLimit(itemsOrSteps, "pipeline");
  if (stages.length === 0) return await __runPipelineItemsWithoutStages(itemsOrSteps);
  if (stages.length === 1 && typeof stages[0] !== "function") {{
    return await __runPipelineSteps(itemsOrSteps, stages[0]);
  }}
  __assertWorkflowSequenceLimit(stages, "pipeline stages");
  const settled = await __runWorkflowLimited(itemsOrSteps, async (originalItem, itemIndex) => {{
    let value = originalItem;
    for (let stageIndex = 0; stageIndex < stages.length; stageIndex++) {{
      try {{
        value = await stages[stageIndex](value, originalItem, itemIndex);
      }} catch (error) {{
        const errorMessage = __workflowErrorMessage(error);
        log(`pipeline item ${{itemIndex + 1}} stage ${{stageIndex + 1}} failed: ${{errorMessage}}`);
        __workflowProgress("pipeline_failed", {{
          message: `item ${{itemIndex + 1}} stage ${{stageIndex + 1}}: ${{errorMessage}}`,
          data: {{ itemIndex: itemIndex + 1, stageIndex: stageIndex + 1, error: errorMessage }},
        }});
        return null;
      }}
    }}
    return value;
  }});
  return settled.map((result, index) => {{
    if (result.status === "fulfilled") return result.value;
    const errorMessage = __workflowErrorMessage(result.reason);
    log(`pipeline item ${{index + 1}} failed: ${{errorMessage}}`);
    __workflowProgress("pipeline_failed", {{
      message: `item ${{index + 1}}: ${{errorMessage}}`,
      data: {{ itemIndex: index + 1, error: errorMessage }},
    }});
    return null;
  }});
}}
const __workflowTasks = [];
__workflowValidateInputArgs();
async function __runChildWorkflow(definition, childArgs, childRef = undefined) {{
  if (__workflowState.childDepth >= 1) {{
    throw new Error("Child workflow nesting is limited to one level");
  }}
  const childIndex = ++__workflowState.childCount;
  const childRunId = `${{definition.name}}#${{childIndex}}`;
  const childReference = __workflowReferenceString(childRef);
  const journalKey = __workflowChildJournalKey(definition, childArgs, childRef);
  __workflowChildJournalPriorKey = journalKey;
  const childData = {{ childIndex, childRunId, key: journalKey }};
  if (childReference) childData.reference = childReference;
  log(`workflow: ${{definition.name}} start`);
  __workflowProgress("child_start", {{
    child: definition.name,
    childIndex,
    childRunId,
    data: childData,
  }});
  if (__workflowChildJournal.has(journalKey)) {{
    const cached = __workflowChildJournal.get(journalKey);
    log(`workflow: ${{definition.name}} cache hit`);
    __workflowProgress("child_journal_hit", {{
      child: definition.name,
      childIndex,
      childRunId,
      data: childData,
    }});
    log(`workflow: ${{definition.name}} complete`);
    __workflowProgress("child_complete", {{
      child: definition.name,
      childIndex,
      childRunId,
      data: {{ ...childData, cached: true }},
    }});
    return cached;
  }}
  __workflowState.childDepth++;
  __workflowState.logPrefix.push(definition.name);
  try {{
    const result = await definition.run(childArgs === undefined ? null : childArgs);
    __workflowState.logPrefix.pop();
    log(`workflow: ${{definition.name}} complete`);
    __workflowRecordChildJournal(journalKey, definition, childRunId, result);
    __workflowProgress("child_complete", {{
      child: definition.name,
      childIndex,
      childRunId,
      data: childData,
    }});
    return result;
  }} catch (error) {{
    const message = __workflowErrorMessage(error);
    __workflowState.logPrefix.pop();
    log(`workflow: ${{definition.name}} failed: ${{message}}`);
    __workflowProgress("child_failed", {{
      child: definition.name,
      childIndex,
      childRunId,
      message,
      error: message,
      data: {{ ...childData, error: message }},
    }});
    throw error;
  }} finally {{
    __workflowState.childDepth--;
    if (__workflowState.logPrefix.at(-1) === definition.name) __workflowState.logPrefix.pop();
  }}
}}
async function workflow(bodyOrRef, childArgs = undefined) {{
  if (
    typeof bodyOrRef === "string" ||
    (bodyOrRef && typeof bodyOrRef === "object" && ("name" in bodyOrRef || "scriptPath" in bodyOrRef || "script_path" in bodyOrRef))
  ) {{
    const definition = __lookupWorkflowDefinition(bodyOrRef);
    if (!definition) throw new Error(`Workflow not found: ${{__workflowReferenceString(bodyOrRef)}}`);
    const task = __runChildWorkflow(definition, childArgs, bodyOrRef);
    __workflowTasks.push(task);
    return await task;
  }}
  const task = (async () => (typeof bodyOrRef === "function" ? await bodyOrRef(args) : bodyOrRef))();
  __workflowTasks.push(task);
  return await task;
}}
__workflowProgress("workflow_start", {{ message: workflowDescription ?? undefined }});
try {{
  const __workflowResult = await (async () => {{
  {code}
  }})();
  if (__workflowResult !== undefined) {{
    const __workflowOutput = __workflowReturnValue(__workflowResult);
    __workflowRecordBudgetOutput(__workflowOutput);
    text(__workflowOutput);
  }} else if (__workflowTasks.length > 0) {{
    const __workflowTaskResults = await Promise.all(__workflowTasks);
    const __workflowVisibleResults = __workflowTaskResults.filter((result) => result !== undefined);
    if (__workflowVisibleResults.length === 1) {{
      const __workflowOutput = __workflowReturnValue(__workflowVisibleResults[0]);
      __workflowRecordBudgetOutput(__workflowOutput);
      text(__workflowOutput);
    }} else if (__workflowVisibleResults.length > 1) {{
      const __workflowOutput = __workflowReturnValue(__workflowVisibleResults);
      __workflowRecordBudgetOutput(__workflowOutput);
      text(__workflowOutput);
    }}
  }}
  __workflowProgress("workflow_complete", {{ data: __workflowMetrics() }});
}} catch (error) {{
  __workflowProgress("workflow_failed", {{ message: __workflowErrorMessage(error), data: __workflowMetrics({{ error: __workflowErrorMessage(error) }}) }});
  throw error;
}}
	"#,
        code = validated.body,
        progress_type_json = progress_type_json,
        input_schema_literal = input_schema_literal,
        agent_journal_entries_json = agent_journal_entries_json,
        workflow_output_budget_json = workflow_output_budget_json
    ))
}

fn workflow_concurrency_cap_for_turn(turn: &crate::session::turn_context::TurnContext) -> usize {
    workflow_concurrency_cap(
        workflow_cpu_concurrency_cap(),
        turn.config
            .effective_agent_max_threads(turn.multi_agent_version),
    )
}

fn workflow_concurrency_cap(cpu_cap: usize, agent_cap: Option<usize>) -> usize {
    match agent_cap {
        Some(0) => 1,
        Some(agent_cap) => cpu_cap.min(agent_cap.max(1)),
        None => cpu_cap,
    }
}

fn workflow_cpu_concurrency_cap() -> usize {
    std::thread::available_parallelism()
        .map(|count| count.get().saturating_sub(2).max(2).min(16))
        .unwrap_or(4)
}

fn render_child_workflow_definitions(
    definitions: &[ChildWorkflowDefinition],
) -> Result<String, FunctionCallError> {
    let mut source = String::new();
    for definition in definitions {
        let keys_json = serde_json::to_string(&definition.keys).map_err(|err| {
            FunctionCallError::RespondToModel(format!(
                "failed to serialize child workflow keys: {err}"
            ))
        })?;
        let name_json = serde_json::to_string(&definition.metadata.name).map_err(|err| {
            FunctionCallError::RespondToModel(format!(
                "failed to serialize child workflow name: {err}"
            ))
        })?;
        let description_json =
            serde_json::to_string(&definition.metadata.description).map_err(|err| {
                FunctionCallError::RespondToModel(format!(
                    "failed to serialize child workflow description: {err}"
                ))
            })?;
        source.push_str(&format!(
            r#"
__registerWorkflowDefinition({keys_json}, {{
  name: {name_json},
  description: {description_json},
  run: async function(args) {{
{body}
  }},
}});
"#,
            body = definition.body
        ));
    }
    Ok(source)
}

fn workflow_display_name(
    args: &WorkflowArgs,
    workflow_source: &WorkflowSource,
    validated: &ValidatedWorkflowScript,
) -> String {
    args.title
        .as_deref()
        .or(args.name.as_deref())
        .unwrap_or_else(|| {
            if workflow_source.name == "inline" || workflow_source.name == "file" {
                validated.metadata.name.as_str()
            } else {
                workflow_source.name.as_str()
            }
        })
        .to_string()
}

fn workflow_run_status_for_runtime_response(
    response: &codex_code_mode::RuntimeResponse,
) -> WorkflowRunStatus {
    match response {
        codex_code_mode::RuntimeResponse::Yielded { .. } => WorkflowRunStatus::Running,
        codex_code_mode::RuntimeResponse::Terminated { .. } => WorkflowRunStatus::Terminated,
        codex_code_mode::RuntimeResponse::Result { error_text, .. } => {
            if error_text.is_some() {
                WorkflowRunStatus::Failed
            } else {
                WorkflowRunStatus::Completed
            }
        }
    }
}

fn workflow_run_status_label(status: WorkflowRunStatus) -> &'static str {
    match status {
        WorkflowRunStatus::Running => "running",
        WorkflowRunStatus::Paused => "paused",
        WorkflowRunStatus::Completed => "completed",
        WorkflowRunStatus::Failed => "failed",
        WorkflowRunStatus::Terminated => "terminated",
    }
}

async fn read_resume_agent_journal(
    turn: &crate::session::turn_context::TurnContext,
    args: &WorkflowArgs,
) -> Vec<WorkflowAgentJournalEntry> {
    let Some(resume_run_id) = normalized_resume_run_id(args) else {
        return Vec::new();
    };
    let journal_path = workflow_run_snapshot_dir(turn)
        .join(resume_run_id)
        .join(WORKFLOW_AGENT_JOURNAL_FILE);
    read_agent_journal_entries(journal_path.as_path()).await
}

async fn read_agent_journal_entries(path: &Path) -> Vec<WorkflowAgentJournalEntry> {
    let Ok(metadata) = tokio::fs::metadata(path).await else {
        return Vec::new();
    };
    if !metadata.is_file() || metadata.len() > WORKFLOW_AGENT_JOURNAL_MAX_BYTES {
        return Vec::new();
    }
    let Ok(contents) = tokio::fs::read_to_string(path).await else {
        return Vec::new();
    };
    contents
        .lines()
        .filter_map(|line| serde_json::from_str::<WorkflowAgentJournalEntry>(line).ok())
        .filter(|entry| !entry.key.trim().is_empty())
        .take(WORKFLOW_AGENT_JOURNAL_MAX_ENTRIES)
        .collect()
}

fn new_workflow_run_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let counter = WORKFLOW_RUN_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("wf_{nanos:x}_{counter:x}")
}

fn prefix_workflow_status(
    output: &mut FunctionToolOutput,
    workflow_name: &str,
    run_id: &str,
    artifacts: Option<&WorkflowRunArtifacts>,
    args: &WorkflowArgs,
) {
    let mut prefix = format!("Workflow `{workflow_name}`\nRun: `{run_id}`\n");
    if let Some(artifacts) = artifacts {
        if let Some(script_path) = artifacts.script_path.as_ref() {
            prefix.push_str(&format!("Script: `{}`\n", script_path.display()));
        }
        prefix.push_str(&format!(
            "Transcripts: `{}`\n",
            artifacts.transcript_dir.display()
        ));
    }
    prefix.push_str("Monitor: `/workflows`\n");
    if args
        .resume_from_run_id
        .as_deref()
        .is_some_and(|id| !id.trim().is_empty())
    {
        prefix.push_str(
            "Resume: `resumeFromRunId` was recorded; Codex replays a completed prior run when script hash and args match, and can reuse matching prior agent journal entries during partial reruns.\n",
        );
    } else if let Some(script_path) = artifacts.and_then(|artifacts| artifacts.script_path.as_ref())
    {
        prefix.push_str(&format!(
            "Resume: call `workflow` with `scriptPath: \"{}\"` and `resumeFromRunId: \"{run_id}\"` to replay this completed run when script hash and args still match.\n",
            script_path.display()
        ));
    }
    if let Some(FunctionCallOutputContentItem::InputText { text }) = output.body.first_mut() {
        text.insert_str(0, &prefix);
    } else {
        output
            .body
            .insert(0, FunctionCallOutputContentItem::InputText { text: prefix });
    }
}

fn create_workflow_tool() -> ToolSpec {
    let mut properties = BTreeMap::new();
    properties.insert(
        "name".to_string(),
        JsonSchema::string(Some(
            "Name of a workflow file in a configured workflow directory, or a namespaced plugin workflow such as `plugin:release`.".to_string(),
        )),
    );
    properties.insert(
        "script".to_string(),
        JsonSchema::string(Some(
                "Inline JavaScript workflow source. Must begin with `export const meta = { name, description, ... }` using pure literal metadata, followed by body code. Inline workflows may require approval before execution depending on `[workflows].approval`. Optional `meta.inputSchema` is validated against `args` before body code runs for common JSON Schema fields: type, required, properties, items, enum, and const. Use top-level await and helpers: workflow, agent, parallel, pipeline, phase, log, console, budget, tools. Workflow return values must be JSON-serializable; circular references, BigInt, functions, symbols, undefined values, and non-finite numbers fail the run. log()/console output is capped so runaway scripts cannot flood the transcript; final workflow return output is still emitted. pipeline(items, stage1, stage2, ...) runs each item through all stages independently; parallel(items) accepts function thunks or step arrays, rejects raw promises/values, waits for all branches, maps branch failures to null, and bounds concurrent branches to the workflow concurrency cap. budget is an object with total, spent(), and remaining(); Codex currently exposes no per-turn token target, so total is null and remaining() is Infinity. agent(...) accepts label, phase, agentType/agent_type, model, reasoningEffort/reasoning_effort, serviceTier/service_tier, forkTurns/fork_turns, stallMs/stall_ms, progressStallMs/progress_stall_ms, retries/maxRetries, isolation, and schema/outputSchema final-output constraints; labels/task names are normalized to Codex task-name segments, long waits emit nonterminal agent_waiting progress before timeout, and timed-out waits are interrupted and retried up to five times by default. isolation: 'worktree' runs in an isolated local git worktree, while unsupported isolation modes fail explicitly. Without schema it returns final text, and with schema it parses the final JSON object. Set returnMetadata only for Codex-specific debugging. If wrapping work in workflow(...), call it as await workflow(async () => { ... }) or return it. Use workflow(\"name\", args), workflow(\"plugin:name\", args), or workflow({scriptPath: \"./path.js\"}, args) to invoke a configured child workflow; child nesting is limited to one level.".to_string(),
        )),
    );
    properties.insert(
        "script_path".to_string(),
        JsonSchema::string(Some(
            "Path to a JavaScript workflow file, relative to the project or configured workflow directories.".to_string(),
        )),
    );
    properties.insert(
        "args".to_string(),
        JsonSchema::object(
            BTreeMap::new(),
            None,
            Some(AdditionalProperties::Boolean(true)),
        ),
    );
    properties.insert(
        "resumeFromRunId".to_string(),
        JsonSchema::string(Some(
            "Prior workflow run id to resume. This may be supplied by itself; Codex loads the prior snapshot's script artifact and args. Codex replays the prior completed output without executing JavaScript when the prior snapshot has the same script hash and args; otherwise, matching prior agent journal entries can be reused during the rerun.".to_string(),
        )),
    );
    properties.insert(
        "title".to_string(),
        JsonSchema::string(Some(
            "Display title for an inline workflow run.".to_string(),
        )),
    );
    properties.insert(
        "description".to_string(),
        JsonSchema::string(Some("Short purpose of the workflow run.".to_string())),
    );
    properties.insert(
        "max_output_tokens".to_string(),
        JsonSchema {
            schema_type: Some(JsonSchemaType::Single(JsonSchemaPrimitiveType::Integer)),
            description: Some("Optional token budget for direct workflow output.".to_string()),
            ..Default::default()
        },
    );

    ToolSpec::Function(ResponsesApiTool {
        name: WORKFLOW_TOOL_NAME.to_string(),
        description: "Run a JavaScript workflow for dynamic workflow and ultracode sessions. Scripts must begin with `export const meta = { name, description, ... }` using pure literal metadata. Optional `meta.inputSchema` is validated against `args` before body code runs for common JSON Schema fields: type, required, properties, items, enum, and const. Workflows can orchestrate normal tools, spawn subagents with `agent(...)`, run item/stage pipelines with `pipeline(items, stage1, stage2, ...)`, run bounded barrier branches with `parallel(...)` function thunks or step arrays, call configured child workflows with `workflow(\"name\", args)`, namespaced plugin workflows with `workflow(\"plugin:name\", args)`, or file workflows with `workflow({scriptPath: \"./path.js\"}, args)`, mark phases, and yield long-running run state through the code-mode runtime. Workflow return values must be JSON-serializable; circular references, BigInt, functions, symbols, undefined values, and non-finite numbers fail the run. log() and console methods output through the same capped workflow log path to prevent runaway transcript growth, while final workflow return output is still emitted. `budget` is an object with Claude-compatible no-target semantics: total is null, spent() is 0, and remaining() is Infinity until Codex has a per-turn workflow token budget source. parallel() and item-stage pipeline() preserve result order while limiting concurrent branches to the workflow concurrency cap. agent(...) accepts Claude-style aliases for label, phase, agentType, reasoningEffort, serviceTier, forkTurns, stallMs, progressStallMs, retries/maxRetries, isolation, and output schemas where Codex has matching spawn-agent fields; labels/task names are normalized to Codex task-name segments, long waits emit nonterminal agent_waiting progress before timeout, and timed-out waits are interrupted and retried up to five times by default. isolation: 'worktree' runs in an isolated local git worktree, while unsupported isolation modes fail explicitly. Without schema it returns final text, and with schema it parses the final JSON object. Set returnMetadata only for Codex-specific debugging. Workflow execution is gated by `[workflows].approval` and `[workflows.named.<name>]`; inline, file-based, and project/user named workflows ask by default, while plugin and bundled-system named workflows are allowed by default unless configured otherwise. Use top-level await. If you call `workflow(async () => { ... })`, await or return it. Child workflow nesting is limited to one level.".to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(properties, None, Some(AdditionalProperties::Boolean(false))),
        output_schema: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct WorkflowAgentRuntimeProbe {
        state: std::sync::Mutex<WorkflowAgentRuntimeProbeState>,
        first_wait_times_out: bool,
    }

    #[derive(Default)]
    struct WorkflowAgentRuntimeProbeState {
        spawns: Vec<String>,
        wait_calls: usize,
        interrupts: Vec<String>,
        interrupt_reasons: Vec<Option<String>>,
        control_polls: Vec<String>,
        notifications: Vec<JsonValue>,
    }

    impl Default for WorkflowAgentRuntimeProbe {
        fn default() -> Self {
            Self {
                state: std::sync::Mutex::new(WorkflowAgentRuntimeProbeState::default()),
                first_wait_times_out: true,
            }
        }
    }

    impl WorkflowAgentRuntimeProbe {
        fn always_complete() -> Self {
            Self {
                state: std::sync::Mutex::new(WorkflowAgentRuntimeProbeState::default()),
                first_wait_times_out: false,
            }
        }

        fn spawns(&self) -> Vec<String> {
            self.state.lock().expect("probe state").spawns.clone()
        }

        fn wait_calls(&self) -> usize {
            self.state.lock().expect("probe state").wait_calls
        }

        fn interrupts(&self) -> Vec<String> {
            self.state.lock().expect("probe state").interrupts.clone()
        }

        fn interrupt_reasons(&self) -> Vec<Option<String>> {
            self.state
                .lock()
                .expect("probe state")
                .interrupt_reasons
                .clone()
        }

        fn control_polls(&self) -> Vec<String> {
            self.state
                .lock()
                .expect("probe state")
                .control_polls
                .clone()
        }

        fn notifications(&self) -> Vec<JsonValue> {
            self.state
                .lock()
                .expect("probe state")
                .notifications
                .clone()
        }
    }

    impl codex_code_mode::CodeModeSessionDelegate for WorkflowAgentRuntimeProbe {
        fn invoke_tool<'a>(
            &'a self,
            invocation: codex_code_mode::CodeModeNestedToolCall,
            cancellation_token: tokio_util::sync::CancellationToken,
        ) -> codex_code_mode::ToolInvocationFuture<'a> {
            Box::pin(async move {
                if cancellation_token.is_cancelled() {
                    return Err("cancelled".to_string());
                }

                let input = invocation.input.unwrap_or(JsonValue::Null);
                let mut state = self.state.lock().expect("probe state");
                match invocation.tool_name.name.as_str() {
                    "spawn_agent" => {
                        let task_name = input
                            .get("task_name")
                            .and_then(JsonValue::as_str)
                            .expect("spawn_agent task_name")
                            .to_string();
                        state.spawns.push(task_name.clone());
                        Ok(serde_json::json!({
                            "task_name": task_name,
                            "agentId": task_name,
                            "workflow_live_transcript": false,
                        }))
                    }
                    "wait_agent" => {
                        state.wait_calls += 1;
                        let wait_call = state.wait_calls;
                        let task_name = state
                            .spawns
                            .last()
                            .cloned()
                            .expect("wait_agent after spawn_agent");
                        if self.first_wait_times_out && wait_call == 1 {
                            Ok(serde_json::json!({
                                "message": "Wait timed out.",
                                "timed_out": true,
                                "messages": [],
                            }))
                        } else {
                            Ok(serde_json::json!({
                                "message": "Wait completed.",
                                "timed_out": false,
                                "messages": [
                                    {
                                        "author": task_name,
                                        "recipient": "root",
                                        "content": "agent-ok",
                                        "status": { "completed": "agent-ok" },
                                        "final_message": "agent-ok",
                                    }
                                ],
                            }))
                        }
                    }
                    "interrupt_agent" => {
                        let target = input
                            .get("target")
                            .and_then(JsonValue::as_str)
                            .expect("interrupt_agent target")
                            .to_string();
                        let reason = input
                            .get("reason")
                            .and_then(JsonValue::as_str)
                            .map(ToString::to_string);
                        state.interrupts.push(target);
                        state.interrupt_reasons.push(reason);
                        Ok(serde_json::json!({ "ok": true }))
                    }
                    "workflow_control" => {
                        let agent_id = input
                            .get("agent_id")
                            .and_then(JsonValue::as_str)
                            .expect("workflow_control agent_id")
                            .to_string();
                        state.control_polls.push(agent_id);
                        Ok(JsonValue::Null)
                    }
                    other => Err(format!("unexpected workflow tool: {other}")),
                }
            })
        }

        fn notify<'a>(
            &'a self,
            _call_id: String,
            _cell_id: codex_code_mode::CellId,
            text: String,
            cancellation_token: tokio_util::sync::CancellationToken,
        ) -> codex_code_mode::NotificationFuture<'a> {
            Box::pin(async move {
                if cancellation_token.is_cancelled() {
                    return Err("cancelled".to_string());
                }
                if let Ok(value) = serde_json::from_str::<JsonValue>(&text) {
                    self.state
                        .lock()
                        .expect("probe state")
                        .notifications
                        .push(value);
                }
                Ok(())
            })
        }

        fn cell_closed(&self, _cell_id: &codex_code_mode::CellId) {}
    }

    fn workflow_agent_test_tool(name: &str) -> codex_code_mode::ToolDefinition {
        codex_code_mode::ToolDefinition {
            name: name.to_string(),
            tool_name: ToolName::plain(name),
            description: format!("test {name}"),
            kind: codex_code_mode::CodeModeToolKind::Function,
            input_schema: None,
            output_schema: None,
        }
    }

    fn workflow_agent_test_tools() -> Vec<codex_code_mode::ToolDefinition> {
        [
            "spawn_agent",
            "wait_agent",
            "interrupt_agent",
            "workflow_control",
        ]
        .into_iter()
        .map(workflow_agent_test_tool)
        .collect()
    }

    fn output_text(response: codex_code_mode::RuntimeResponse) -> String {
        let content_items = match response {
            codex_code_mode::RuntimeResponse::Result { content_items, .. }
            | codex_code_mode::RuntimeResponse::Yielded { content_items, .. }
            | codex_code_mode::RuntimeResponse::Terminated { content_items, .. } => content_items,
        };
        content_items
            .into_iter()
            .map(|item| match item {
                codex_code_mode::FunctionCallOutputContentItem::InputText { text } => text,
                codex_code_mode::FunctionCallOutputContentItem::InputImage { .. } => String::new(),
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    async fn run_inline_workflow_text(script: &str) -> String {
        output_text(run_inline_workflow_response(script, None, &[]).await)
    }

    async fn run_inline_workflow_text_with_output_budget(
        script: &str,
        output_budget_tokens: usize,
    ) -> String {
        output_text(
            run_inline_workflow_response_with_output_budget(
                script,
                None,
                &[],
                Some(output_budget_tokens),
            )
            .await,
        )
    }

    async fn run_inline_workflow_text_with_child_definitions(
        script: &str,
        child_definitions: &[ChildWorkflowDefinition],
    ) -> String {
        output_text(run_inline_workflow_response(script, None, child_definitions).await)
    }

    async fn run_inline_workflow_response(
        script: &str,
        workflow_args: Option<serde_json::Value>,
        child_definitions: &[ChildWorkflowDefinition],
    ) -> codex_code_mode::RuntimeResponse {
        run_inline_workflow_response_with_output_budget(
            script,
            workflow_args,
            child_definitions,
            None,
        )
        .await
    }

    async fn run_inline_workflow_response_with_output_budget(
        script: &str,
        workflow_args: Option<serde_json::Value>,
        child_definitions: &[ChildWorkflowDefinition],
        output_budget_tokens: Option<usize>,
    ) -> codex_code_mode::RuntimeResponse {
        let args = WorkflowArgs {
            name: None,
            script: Some(script.to_string()),
            script_path: None,
            args: workflow_args,
            resume_from_run_id: None,
            title: Some("helper-test".to_string()),
            description: None,
            max_output_tokens: None,
        };
        let source = WorkflowSource {
            name: "helper-test".to_string(),
            code: args.script.clone().expect("script"),
            kind: WorkflowSourceKind::Inline,
            path: None,
        };
        let validated = validate_workflow_script(&source.code).expect("valid workflow script");
        let script = build_test_workflow_script(
            "wf_test",
            &args,
            &validated,
            child_definitions,
            &[],
            output_budget_tokens,
        )
        .expect("workflow script");
        let service = codex_code_mode::CodeModeService::new();
        service
            .execute(codex_code_mode::ExecuteRequest {
                tool_call_id: "call-1".to_string(),
                enabled_tools: Vec::new(),
                source: script,
                yield_time_ms: None,
                max_output_tokens: None,
            })
            .await
            .expect("start workflow script")
            .initial_response()
            .await
            .expect("workflow result")
    }

    fn build_test_workflow_script(
        run_id: &str,
        args: &WorkflowArgs,
        validated: &ValidatedWorkflowScript,
        child_definitions: &[ChildWorkflowDefinition],
        agent_journal_entries: &[WorkflowAgentJournalEntry],
        workflow_output_budget_tokens: Option<usize>,
    ) -> Result<String, FunctionCallError> {
        build_test_workflow_script_with_concurrency_cap(
            run_id,
            args,
            validated,
            child_definitions,
            agent_journal_entries,
            workflow_output_budget_tokens,
            workflow_cpu_concurrency_cap(),
        )
    }

    fn build_test_workflow_script_with_concurrency_cap(
        run_id: &str,
        args: &WorkflowArgs,
        validated: &ValidatedWorkflowScript,
        child_definitions: &[ChildWorkflowDefinition],
        agent_journal_entries: &[WorkflowAgentJournalEntry],
        workflow_output_budget_tokens: Option<usize>,
        concurrency_cap: usize,
    ) -> Result<String, FunctionCallError> {
        build_workflow_script(
            run_id,
            args,
            validated,
            child_definitions,
            agent_journal_entries,
            workflow_output_budget_tokens,
            concurrency_cap,
            Path::new("/tmp/workflow-cwd"),
            Some("workflow-branch"),
            Some("thread-workflow-parent"),
        )
    }

    fn approval_test_script() -> ValidatedWorkflowScript {
        ValidatedWorkflowScript {
            metadata: WorkflowMetadata {
                name: "release".to_string(),
                description: "Release workflow".to_string(),
                when_to_use: None,
                input_schema: None,
                phases: Vec::new(),
            },
            body: "return 'ok';".to_string(),
        }
    }

    fn workflow_source(name: &str, kind: WorkflowSourceKind) -> WorkflowSource {
        WorkflowSource {
            name: name.to_string(),
            code: String::new(),
            kind,
            path: None,
        }
    }

    fn workflow_source_with_path(
        name: &str,
        kind: WorkflowSourceKind,
        path: PathBuf,
    ) -> WorkflowSource {
        WorkflowSource {
            name: name.to_string(),
            code: String::new(),
            kind,
            path: Some(path),
        }
    }

    fn test_stable_json_string(value: &JsonValue) -> String {
        match value {
            JsonValue::Null | JsonValue::Bool(_) | JsonValue::Number(_) | JsonValue::String(_) => {
                serde_json::to_string(value).expect("serialize scalar")
            }
            JsonValue::Array(values) => format!(
                "[{}]",
                values
                    .iter()
                    .map(test_stable_json_string)
                    .collect::<Vec<_>>()
                    .join(",")
            ),
            JsonValue::Object(object) => {
                let mut keys = object.keys().collect::<Vec<_>>();
                keys.sort();
                format!(
                    "{{{}}}",
                    keys.into_iter()
                        .map(|key| {
                            format!(
                                "{}:{}",
                                serde_json::to_string(key).expect("serialize key"),
                                test_stable_json_string(&object[key])
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(",")
                )
            }
        }
    }

    fn test_agent_journal_key(prior_key: &str, prompt: &str, options: JsonValue) -> String {
        let material = test_stable_json_string(&serde_json::json!({
            "options": options,
            "priorKey": prior_key,
            "prompt": prompt,
        }));
        let mut hash = 0x811c9dc5_u32;
        for byte in material.bytes() {
            hash ^= u32::from(byte);
            hash = hash.wrapping_mul(0x01000193);
        }
        format!("codex-v2:{hash:08x}")
    }

    fn test_child_journal_key(
        prior_key: &str,
        name: &str,
        reference: &str,
        args: JsonValue,
    ) -> String {
        let material = test_stable_json_string(&serde_json::json!({
            "args": args,
            "name": name,
            "priorKey": prior_key,
            "reference": reference,
        }));
        let mut hash = 0x811c9dc5_u32;
        for byte in material.bytes() {
            hash ^= u32::from(byte);
            hash = hash.wrapping_mul(0x01000193);
        }
        format!("codex-child-v1:{hash:08x}")
    }

    #[tokio::test]
    async fn workflow_agent_journal_reader_preserves_started_and_result_entries() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let journal_path = temp_dir.path().join(WORKFLOW_AGENT_JOURNAL_FILE);
        tokio::fs::write(
            &journal_path,
            concat!(
                r#"{"type":"started","key":"codex-v2:first","agentId":"agent-one"}"#,
                "\n",
                r#"{"type":"result","key":"codex-v2:first","agentId":"agent-one","result":{"ok":true}}"#,
                "\n",
                r#"{"type":"child_result","key":"codex-child-v1:first","child":"child","childRunId":"child#1","result":"child-ok"}"#,
                "\n",
            ),
        )
        .await
        .expect("write journal");

        let entries = read_agent_journal_entries(journal_path.as_path()).await;

        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].entry_type.as_deref(), Some("started"));
        assert_eq!(entries[0].key, "codex-v2:first");
        assert_eq!(entries[0].agent_id.as_deref(), Some("agent-one"));
        assert_eq!(entries[0].result, None);
        assert_eq!(entries[1].entry_type.as_deref(), Some("result"));
        assert_eq!(entries[1].agent_id.as_deref(), Some("agent-one"));
        assert_eq!(entries[1].result, Some(serde_json::json!({ "ok": true })));
        assert_eq!(entries[2].entry_type.as_deref(), Some("child_result"));
        assert_eq!(entries[2].key, "codex-child-v1:first");
        assert_eq!(entries[2].child.as_deref(), Some("child"));
        assert_eq!(entries[2].child_run_id.as_deref(), Some("child#1"));
        assert_eq!(
            entries[2].result,
            Some(JsonValue::String("child-ok".to_string()))
        );
    }

    #[tokio::test]
    async fn workflow_approval_auto_asks_for_dynamic_and_local_named_sources() {
        let (_session, turn) = crate::session::tests::make_session_and_context().await;
        let validated = approval_test_script();

        assert_eq!(
            WorkflowApprovalResolution::Ask,
            workflow_approval_resolution(
                &turn,
                &workflow_source("inline", WorkflowSourceKind::Inline),
                &validated,
            )
        );
        assert_eq!(
            WorkflowApprovalResolution::Ask,
            workflow_approval_resolution(
                &turn,
                &workflow_source("file", WorkflowSourceKind::ScriptPath),
                &validated,
            )
        );
        assert_eq!(
            WorkflowApprovalResolution::Ask,
            workflow_approval_resolution(
                &turn,
                &workflow_source("release", WorkflowSourceKind::Named),
                &validated,
            )
        );
    }

    #[tokio::test]
    async fn workflow_approval_auto_allows_plugin_and_system_named_sources() {
        use codex_utils_absolute_path::test_support::PathExt as _;

        let plugin_dir = tempfile::tempdir().expect("plugin workflow dir");
        let (_session, mut turn) = crate::session::tests::make_session_and_context().await;
        let plugin_dir = plugin_dir.path().abs();
        let plugin_path = plugin_dir.join("release.js").to_path_buf();
        let system_path = turn
            .config
            .codex_home
            .join("workflows/.system/release.js")
            .to_path_buf();
        std::sync::Arc::make_mut(&mut turn.config)
            .workflows
            .plugin_workflow_dirs = vec![crate::config::WorkflowPluginDirectory {
            namespace: "sample".to_string(),
            plugin_id: "sample@test".to_string(),
            dir: plugin_dir,
        }];
        let validated = approval_test_script();

        assert_eq!(
            WorkflowApprovalResolution::Allow,
            workflow_approval_resolution(
                &turn,
                &workflow_source_with_path(
                    "sample:release",
                    WorkflowSourceKind::Named,
                    plugin_path
                ),
                &validated,
            )
        );
        assert_eq!(
            WorkflowApprovalResolution::Allow,
            workflow_approval_resolution(
                &turn,
                &workflow_source_with_path("release", WorkflowSourceKind::Named, system_path),
                &validated,
            )
        );
    }

    #[tokio::test]
    async fn workflow_approval_named_override_can_deny_or_ask() {
        let (_session, mut turn) = crate::session::tests::make_session_and_context().await;
        let config = std::sync::Arc::make_mut(&mut turn.config);
        config.workflows.named.insert(
            "release".to_string(),
            WorkflowDefinitionConfig {
                enabled: Some(false),
                approval: Some(WorkflowApproval::Allow),
            },
        );
        let validated = approval_test_script();

        assert_eq!(
            WorkflowApprovalResolution::Deny(
                "workflow `release` is disabled by `[workflows.named.release]`".to_string()
            ),
            workflow_approval_resolution(
                &turn,
                &workflow_source("release", WorkflowSourceKind::Named),
                &validated,
            )
        );

        let config = std::sync::Arc::make_mut(&mut turn.config);
        config.workflows.named.insert(
            "release".to_string(),
            WorkflowDefinitionConfig {
                enabled: Some(true),
                approval: Some(WorkflowApproval::Ask),
            },
        );

        assert_eq!(
            WorkflowApprovalResolution::Ask,
            workflow_approval_resolution(
                &turn,
                &workflow_source("release", WorkflowSourceKind::Named),
                &validated,
            )
        );
    }

    #[test]
    fn workflow_approval_prompt_rejection_respects_approval_policy() {
        assert_eq!(
            Some("workflow approval required, but approval_policy is set to never"),
            workflow_approval_prompt_rejection_reason(AskForApproval::Never)
        );
        assert_eq!(
            None,
            workflow_approval_prompt_rejection_reason(AskForApproval::OnRequest)
        );
        assert_eq!(
            Some(
                "workflow approval required, but granular approval config has skill_approval=false"
            ),
            workflow_approval_prompt_rejection_reason(AskForApproval::Granular(
                codex_protocol::protocol::GranularApprovalConfig {
                    sandbox_approval: true,
                    rules: true,
                    skill_approval: false,
                    request_permissions: true,
                    mcp_elicitations: true,
                }
            ))
        );
    }

    #[test]
    fn workflow_approval_prompt_includes_bounded_source_metadata_and_args() {
        let script = r#"
export const meta = {
  name: 'release',
  description: 'Deploy release artifacts',
  whenToUse: 'Use when a release workflow needs approval',
  phases: [
    { title: 'Build' },
    { title: 'Publish', model: 'xhigh' },
  ],
  inputSchema: {
    type: 'object',
    required: ['channel'],
    properties: {
      channel: { type: 'string' },
    },
  },
}
log('ship');
"#;
        let validated = validate_workflow_script(script).expect("valid workflow");
        let source = WorkflowSource {
            name: "release".to_string(),
            code: script.to_string(),
            kind: WorkflowSourceKind::Named,
            path: Some(PathBuf::from("/tmp/workflows/release.js")),
        };
        let args = WorkflowArgs {
            name: Some("release".to_string()),
            script: None,
            script_path: None,
            args: Some(serde_json::json!({
                "channel": "alpha",
                "notes": "x".repeat(WORKFLOW_APPROVAL_PREVIEW_MAX_CHARS + 32),
            })),
            resume_from_run_id: None,
            title: None,
            description: None,
            max_output_tokens: None,
        };

        let prompt = workflow_approval_prompt_text("release", &source, &validated, &args);

        assert!(
            prompt.contains("Allow workflow `release` to run?"),
            "{prompt}"
        );
        assert!(prompt.contains("Source: named"), "{prompt}");
        assert!(
            prompt.contains("Path: `/tmp/workflows/release.js`"),
            "{prompt}"
        );
        assert!(prompt.contains("Metadata: `release`"), "{prompt}");
        assert!(
            prompt.contains("Description: Deploy release artifacts"),
            "{prompt}"
        );
        assert!(
            prompt.contains("When to use: Use when a release workflow needs approval"),
            "{prompt}"
        );
        assert!(
            prompt.contains("Phases: Build, Publish (xhigh)"),
            "{prompt}"
        );
        assert!(prompt.contains("Input schema:"), "{prompt}");
        assert!(prompt.contains("required"), "{prompt}");
        assert!(prompt.contains("Args:"), "{prompt}");
        assert!(prompt.contains("\"channel\": \"alpha\""), "{prompt}");
        assert!(prompt.contains("... [truncated]"), "{prompt}");
        assert!(prompt.contains("Script preview:"), "{prompt}");
        assert!(prompt.contains("export const meta"), "{prompt}");
    }

    #[test]
    fn parse_workflow_approval_response_prefers_session_allow() {
        use codex_protocol::request_user_input::RequestUserInputAnswer;
        use std::collections::HashMap;

        let response = RequestUserInputResponse {
            answers: HashMap::from([(
                "workflow_approval_call".to_string(),
                RequestUserInputAnswer {
                    answers: vec![
                        WORKFLOW_APPROVAL_ALLOW.to_string(),
                        WORKFLOW_APPROVAL_ALLOW_FOR_SESSION.to_string(),
                    ],
                },
            )]),
        };

        assert_eq!(
            WorkflowApprovalResponseDecision::AllowForSession,
            parse_workflow_approval_response(Some(response), "workflow_approval_call")
        );
        assert_eq!(
            WorkflowApprovalResponseDecision::Cancel,
            parse_workflow_approval_response(None, "workflow_approval_call")
        );
    }

    #[test]
    fn parse_workflow_approval_response_supports_persistent_allow() {
        use codex_protocol::request_user_input::RequestUserInputAnswer;
        use std::collections::HashMap;

        let response = RequestUserInputResponse {
            answers: HashMap::from([(
                "workflow_approval_call".to_string(),
                RequestUserInputAnswer {
                    answers: vec![WORKFLOW_APPROVAL_ALLOW_ALWAYS.to_string()],
                },
            )]),
        };

        assert_eq!(
            WorkflowApprovalResponseDecision::AllowAlways,
            parse_workflow_approval_response(Some(response), "workflow_approval_call")
        );
    }

    #[test]
    fn workflow_persistent_approval_name_only_accepts_named_workflows() {
        let validated = approval_test_script();

        assert_eq!(
            Some("release".to_string()),
            workflow_persistent_approval_name(
                &workflow_source("release", WorkflowSourceKind::Named),
                &validated.metadata,
            )
        );
        assert_eq!(
            Some("team:release".to_string()),
            workflow_persistent_approval_name(
                &workflow_source("team:release", WorkflowSourceKind::Named),
                &validated.metadata,
            )
        );
        assert_eq!(
            None,
            workflow_persistent_approval_name(
                &workflow_source("../release", WorkflowSourceKind::Named),
                &validated.metadata,
            )
        );
        assert_eq!(
            None,
            workflow_persistent_approval_name(
                &workflow_source("release", WorkflowSourceKind::ScriptPath),
                &validated.metadata,
            )
        );
    }

    #[tokio::test]
    async fn persistent_workflow_approval_writes_named_allow_and_remembers_session() {
        let (session, turn) = crate::session::tests::make_session_and_context().await;
        let key = WorkflowApprovalKey {
            source_kind: WorkflowSourceKind::Named,
            source_name: "release".to_string(),
            metadata_name: "release".to_string(),
            path: Some("/tmp/release.js".to_string()),
        };

        persist_workflow_approval_allow(&session, &turn, "release", key.clone()).await;

        let contents = std::fs::read_to_string(turn.config.codex_home.join("config.toml"))
            .expect("read persisted config");
        let parsed: codex_config::config_toml::ConfigToml =
            toml::from_str(&contents).expect("parse persisted config");
        let approval = parsed
            .workflows
            .and_then(|workflows| workflows.named)
            .and_then(|named| named.get("release").cloned())
            .and_then(|rule| rule.approval);
        assert_eq!(Some(WorkflowApproval::Allow), approval);
        assert!(workflow_approval_is_remembered(&session, &key).await);
    }

    #[test]
    fn workflow_guardian_request_includes_source_metadata_and_args() {
        let validated = approval_test_script();
        let source = workflow_source_with_path(
            "release",
            WorkflowSourceKind::Named,
            PathBuf::from("/tmp/release.js"),
        );
        let args = WorkflowArgs {
            name: Some("release".to_string()),
            script: None,
            script_path: None,
            args: Some(serde_json::json!({ "target": "alpha" })),
            resume_from_run_id: None,
            title: None,
            description: None,
            max_output_tokens: None,
        };

        let request =
            workflow_guardian_request("call-workflow", "release", &source, &validated, &args);
        let payload = crate::guardian::guardian_approval_request_to_json(&request)
            .expect("serialize guardian workflow request");

        assert_eq!(payload["tool"], "workflow");
        assert_eq!(payload["workflowName"], "release");
        assert_eq!(payload["source"]["kind"], "named");
        assert_eq!(payload["source"]["name"], "release");
        assert_eq!(payload["source"]["path"], "/tmp/release.js");
        assert_eq!(payload["metadata"]["name"], "release");
        assert_eq!(payload["metadata"]["description"], "Release workflow");
        assert_eq!(payload["args"]["target"], "alpha");
    }

    #[tokio::test]
    async fn workflow_guardian_approved_for_session_remembers_approval() {
        let (session, _turn) = crate::session::tests::make_session_and_context().await;
        let key = WorkflowApprovalKey {
            source_kind: WorkflowSourceKind::Named,
            source_name: "release".to_string(),
            metadata_name: "release".to_string(),
            path: Some("/tmp/release.js".to_string()),
        };

        apply_workflow_guardian_decision(
            &session,
            Some(&key),
            (
                "guardian-review".to_string(),
                ReviewDecision::ApprovedForSession,
            ),
        )
        .await
        .expect("guardian approval should allow workflow");

        assert!(workflow_approval_is_remembered(&session, &key).await);
    }

    #[tokio::test]
    async fn denied_workflow_does_not_persist_source_artifacts_before_approval() {
        let (session, mut turn) = crate::session::tests::make_session_and_context().await;
        std::sync::Arc::make_mut(&mut turn.config)
            .workflows
            .approval = WorkflowApproval::Deny;
        let snapshot_dir = workflow_run_snapshot_dir(&turn);
        let session = Arc::new(session);
        let turn = Arc::new(turn);
        let script = r#"
export const meta = {
  name: 'denied-inline',
  description: 'Verify denied workflows do not copy source artifacts',
}
return 'should-not-run';
"#;
        let arguments = serde_json::json!({
            "script": script,
            "title": "denied-inline",
        })
        .to_string();
        let result = WorkflowHandler::new(Vec::new())
            .handle_call(ToolInvocation {
                session,
                turn,
                cancellation_token: tokio_util::sync::CancellationToken::new(),
                tracker: Arc::new(tokio::sync::Mutex::new(
                    crate::turn_diff_tracker::TurnDiffTracker::new(),
                )),
                call_id: "call-denied-workflow".to_string(),
                tool_name: ToolName::plain(WORKFLOW_TOOL_NAME),
                source: crate::tools::context::ToolCallSource::Direct,
                payload: ToolPayload::Function { arguments },
            })
            .await;

        assert!(result.is_err(), "denied workflow should fail");
        let mut entries = tokio::fs::read_dir(&snapshot_dir)
            .await
            .expect("read workflow snapshot dir");
        let mut snapshot_paths = Vec::new();
        let mut artifact_dirs = Vec::new();
        while let Some(entry) = entries.next_entry().await.expect("read snapshot entry") {
            let path = entry.path();
            let file_type = entry.file_type().await.expect("read snapshot file type");
            if file_type.is_dir() {
                artifact_dirs.push(path);
            } else if path.extension().and_then(|ext| ext.to_str()) == Some("json") {
                snapshot_paths.push(path);
            }
        }
        assert_eq!(1, snapshot_paths.len(), "expected one failed snapshot");
        assert!(
            artifact_dirs.is_empty(),
            "denied workflow should not create artifact dirs: {artifact_dirs:?}"
        );
        let snapshot: JsonValue = serde_json::from_str(
            &tokio::fs::read_to_string(&snapshot_paths[0])
                .await
                .expect("read failed snapshot"),
        )
        .expect("parse failed snapshot");
        assert_eq!(
            Some("failed"),
            snapshot.get("status").and_then(JsonValue::as_str)
        );
        assert!(
            snapshot.get("run_dir").is_none_or(JsonValue::is_null),
            "denied workflow snapshot should not expose run_dir"
        );
        assert!(
            snapshot.get("script_path").is_none_or(JsonValue::is_null),
            "denied workflow snapshot should not expose script_path"
        );
        assert!(
            snapshot
                .get("transcript_dir")
                .is_none_or(JsonValue::is_null),
            "denied workflow snapshot should not expose transcript_dir"
        );
    }

    #[tokio::test]
    async fn workflow_wrapper_waits_for_unawaited_workflow_helper_call() {
        let args = WorkflowArgs {
            name: None,
            script: Some(
                r#"
export const meta = {
  name: 'unawaited-helper',
  description: 'Verify helper tasks are drained',
}
workflow(async () => {
  phase("start");
  await new Promise((resolve) => setTimeout(resolve, 1));
  log("after-await");
  return "workflow-ok";
});
"#
                .to_string(),
            ),
            script_path: None,
            args: None,
            resume_from_run_id: None,
            title: Some("unawaited".to_string()),
            description: None,
            max_output_tokens: None,
        };
        let source = WorkflowSource {
            name: "unawaited".to_string(),
            code: args.script.clone().expect("script"),
            kind: WorkflowSourceKind::Inline,
            path: None,
        };
        let validated = validate_workflow_script(&source.code).expect("valid workflow script");
        let script = build_test_workflow_script("wf_test", &args, &validated, &[], &[], None)
            .expect("workflow script");
        let service = codex_code_mode::CodeModeService::new();
        let response = service
            .execute(codex_code_mode::ExecuteRequest {
                tool_call_id: "call-1".to_string(),
                enabled_tools: Vec::new(),
                source: script,
                yield_time_ms: None,
                max_output_tokens: None,
            })
            .await
            .expect("start workflow script")
            .initial_response()
            .await
            .expect("workflow result");
        let text = output_text(response);

        assert!(text.contains("phase: start"), "{text}");
        assert!(text.contains("after-await"), "{text}");
        assert!(text.contains("workflow-ok"), "{text}");
    }

    #[test]
    fn workflow_agent_helper_accepts_claude_option_aliases() {
        let args = WorkflowArgs {
            name: None,
            script: Some(
                r#"
export const meta = {
  name: 'agent-aliases',
  description: 'Exercise agent option aliases',
}
await agent('check aliases', {
  label: 'alias-label',
  phase: 'Review',
  agentType: 'Explore',
  reasoningEffort: 'high',
  serviceTier: 'fast',
  isolation: 'worktree',
  forkTurns: '1',
  retries: 2,
  outputSchema: { type: 'object' },
  wait: false,
});
"#
                .to_string(),
            ),
            script_path: None,
            args: None,
            resume_from_run_id: None,
            title: None,
            description: None,
            max_output_tokens: None,
        };
        let validated =
            validate_workflow_script(args.script.as_deref().expect("script")).expect("valid");
        let script = build_test_workflow_script("wf_test", &args, &validated, &[], &[], None)
            .expect("workflow script");

        assert!(script.contains("if (options.phase) phase(String(options.phase));"));
        assert!(script.contains("const WORKFLOW_AGENT_RETRY_CAP = 5;"));
        assert!(
            script.contains(
                "const WORKFLOW_PROGRESS_NOTIFICATION_TYPE = \"codex_workflow_progress\";"
            )
        );
        assert!(script.contains("__workflowProgress(\"agent_start\""));
        assert!(script.contains("__workflowProgress(\"agent_detached\""));
        assert!(script.contains("__workflowProgress(\"agent_complete\""));
        assert!(script.contains("__workflowProgress(\"agent_retry\""));
        assert!(script.contains("let __workflowAgentJournalPriorKey = \"\";"));
        assert!(script.contains("const __workflowAgentJournalStarted = new Map();"));
        assert!(script.contains("`codex-v2:${__workflowHashString(material)}`"));
        assert!(script.contains("__workflowAgentJournalLookup(journalKey)"));
        assert!(script.contains("__workflowAgentJournalStartedAttempts(journalKey)"));
        assert!(script.contains("__workflowProgress(\"agent_journal_started_hit\""));
        assert!(
            script.contains("__workflowRecordAgentJournalStarted(journalKey, task_name, spawn)")
        );
        assert!(script.contains("function __workflowRecordAgentTranscript"));
        assert!(script.contains("function __workflowAgentTranscriptMetadata"));
        assert!(script.contains("__workflowProgress(\"agent_transcript_entry\""));
        assert!(script.contains("const __workflowAgentTranscriptStarted = new Set();"));
        assert!(script.contains("function __workflowRecordAgentTranscriptStart"));
        assert!(script.contains("data.promptRecorded = true"));
        assert!(script.contains("data.transcriptRecorded = true"));
        assert!(script.contains("spawn.workflow_live_transcript"));
        assert!(script.contains("runId: workflowRunId"));
        assert!(script.contains("const workflowCwd = \"/tmp/workflow-cwd\";"));
        assert!(script.contains("const workflowGitBranch = \"workflow-branch\";"));
        assert!(script.contains("const workflowParentThreadId = \"thread-workflow-parent\";"));
        assert!(script.contains("cwd: workflowCwd"));
        assert!(script.contains("agentName: request.task_name === undefined"));
        assert!(script.contains("sessionKind: \"workflow_agent\""));
        assert!(script.contains("metadata.gitBranch = String(workflowGitBranch)"));
        assert!(script.contains("metadata.parentThreadId = String(workflowParentThreadId)"));
        assert!(script.contains("spawn.tool_use_id ?? spawn.toolUseId"));
        assert!(script.contains("metadata.toolUseId = String(toolUseId)"));
        assert!(script.contains("spawn.worktree_path ?? spawn.worktreePath"));
        assert!(script.contains("metadata.worktreePath = String(worktreePath)"));
        assert!(script.contains("!liveTranscript && Array.isArray(details.transcript)"));
        assert!(script.contains("data.metadata = details.metadata"));
        assert!(script.contains("data.toolCalls = details.toolCalls"));
        assert!(script.contains("data.reasoning = details.reasoning"));
        assert!(script.contains("data.transcript = details.transcript"));
        assert!(script.contains("ownMessage && ownMessage.tool_calls"));
        assert!(script.contains("ownMessage && ownMessage.reasoning"));
        assert!(script.contains("ownMessage && ownMessage.transcript"));
        assert!(script.contains("const metadata = __workflowAgentTranscriptMetadata"));
        assert!(script.contains("finalMessage, result, toolCalls"));
        assert!(script.contains("toolCalls, reasoning, transcript, metadata"));
        assert!(
            script.contains(
                "__workflowRecordAgentTranscriptStart(task_name, message, spawn, request)"
            )
        );
        assert!(script.contains("__workflowRecordAgentTranscript(task_name, message, details)"));
        assert!(script.contains(
            "__workflowAgentTaskName(options.label ?? options.task_name ?? options.name"
        ));
        assert!(script.contains("__workflowAgentAttemptTaskName(baseTaskName, retryIndex)"));
        assert!(script.contains(
            "options.retries ?? options.max_retries ?? options.maxRetries ?? options.retry"
        ));
        assert!(script.contains("__workflowInterruptAgent"));
        assert!(script.contains("__workflowTool(\"interrupt_agent\")"));
        assert!(script.contains("const workflowRunId = \"wf_test\";"));
        assert!(script.contains("const WORKFLOW_AGENT_CONTROL_POLL_MS = 10000;"));
        assert!(script.contains("function __workflowAgentControlState(agentId)"));
        assert!(script.contains("function __workflowAgentProgressStallMs"));
        assert!(
            script.contains(
                "options.progress_stall_ms ?? options.progressStallMs ?? options.stall_warning_ms ?? options.stallWarningMs"
            )
        );
        assert!(script.contains("__workflowTool(\"workflow_control\")"));
        assert!(script.contains("function __workflowWaitAgentWithControl"));
        assert!(script.contains("__workflowProgress(\"agent_waiting\""));
        assert!(script.contains("const controlPollMs = __workflowAgentControlPollMs"));
        assert!(
            script
                .contains("__workflowAgentProgressStallMs(options, waitTimeoutMs, controlPollMs)")
        );
        assert!(script.contains("__workflowProgress(\"agent_skipped\""));
        assert!(script.contains("waited && waited.timed_out"));
        assert!(script.contains("options.agent_type ?? options.agentType"));
        assert!(script.contains("options.reasoning_effort ?? options.reasoningEffort"));
        assert!(script.contains("options.service_tier ?? options.serviceTier"));
        assert!(script.contains("isolation: options.isolation"));
        assert!(script.contains("options.fork_turns ?? options.forkTurns"));
        assert!(script.contains("options.schema ?? options.output_schema ?? options.outputSchema"));
        assert!(script.contains("include_messages: true"));
        assert!(script.contains("JSON.parse(finalMessage)"));
        assert!(script.contains("options.stall_ms ?? options.stallMs ?? 180000"));
        assert!(script.contains("return returnMetadata ? details : result;"));
    }

    #[tokio::test]
    async fn workflow_parallel_all_settles_failed_items() {
        let text = run_inline_workflow_text(
            r#"
export const meta = {
  name: 'parallel-settled',
  description: 'Verify failed parallel items do not fail the whole workflow',
}
return await parallel([
  () => 'one',
  () => { throw new Error('bad branch'); },
  async () => 'three',
]);
"#,
        )
        .await;

        assert!(
            text.contains("parallel item 2 failed: bad branch"),
            "{text}"
        );
        assert!(text.contains("\"one\""), "{text}");
        assert!(text.contains("null"), "{text}");
        assert!(text.contains("\"three\""), "{text}");
    }

    #[tokio::test]
    async fn workflow_parallel_respects_concurrency_cap() {
        let text = run_inline_workflow_text(
            r#"
export const meta = {
  name: 'parallel-concurrency',
  description: 'Verify parallel branches are bounded',
}
let active = 0;
let maxActive = 0;
const total = WORKFLOW_CONCURRENCY_CAP + 3;
const items = Array.from({ length: total }, (_, index) => async () => {
  active += 1;
  if (active > maxActive) maxActive = active;
  await new Promise((resolve) => setTimeout(resolve, 5));
  active -= 1;
  return index;
});
const results = await parallel(items);
return {
  cap: WORKFLOW_CONCURRENCY_CAP,
  maxActive,
  count: results.length,
  ordered: results[0] === 0 && results.at(-1) === total - 1,
  withinCap: maxActive <= WORKFLOW_CONCURRENCY_CAP,
};
"#,
        )
        .await;

        assert!(text.contains("\"ordered\": true"), "{text}");
        assert!(text.contains("\"withinCap\": true"), "{text}");
    }

    #[tokio::test]
    async fn workflow_log_output_is_capped() {
        let text = run_inline_workflow_text(
            r#"
export const meta = {
  name: 'log-cap',
  description: 'Verify runaway logs are capped',
}
for (let index = 0; index < WORKFLOW_LOG_CAP + 5; index++) {
  log(`entry-${index}`);
}
return 'workflow-result-still-visible';
"#,
        )
        .await;

        assert!(text.contains("entry-0"), "{text}");
        assert!(text.contains("entry-998"), "{text}");
        assert!(
            text.contains("workflow log cap reached (1000); further log output suppressed"),
            "{text}"
        );
        assert!(!text.contains("entry-999"), "{text}");
        assert!(!text.contains("entry-1004"), "{text}");
        assert!(text.contains("workflow-result-still-visible"), "{text}");
    }

    #[tokio::test]
    async fn workflow_console_methods_write_capped_logs() {
        let text = run_inline_workflow_text(
            r#"
export const meta = {
  name: 'console-helper',
  description: 'Verify Claude-style console helper',
}
console.log('console-log', { ok: true });
console.info('console-info');
console.warn('console-warn');
console.error('console-error');
console.debug('console-debug');
console.dir({ nested: ['value'] });
return 'console-result';
"#,
        )
        .await;

        assert!(text.contains("console-log"), "{text}");
        assert!(text.contains(r#""ok": true"#), "{text}");
        assert!(text.contains("console-info"), "{text}");
        assert!(text.contains("console-warn"), "{text}");
        assert!(text.contains("console-error"), "{text}");
        assert!(text.contains("console-debug"), "{text}");
        assert!(text.contains(r#""nested": ["#), "{text}");
        assert!(text.contains("console-result"), "{text}");
    }

    #[tokio::test]
    async fn workflow_parallel_rejects_raw_promise_items() {
        let response = run_inline_workflow_response(
            r#"
export const meta = {
  name: 'parallel-promises',
  description: 'Reject eager parallel work',
}
const eager = Promise.resolve('started-before-parallel');
return await parallel([eager]);
"#,
            None,
            &[],
        )
        .await;

        match response {
            codex_code_mode::RuntimeResponse::Result {
                error_text: Some(error),
                content_items,
                ..
            } => {
                assert!(
                    error.contains("parallel() expects an array of functions or step arrays"),
                    "{error}"
                );
                assert!(
                    content_items.is_empty(),
                    "raw promise result should not be collected: {content_items:?}"
                );
            }
            other => panic!("expected parallel raw promise validation error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn workflow_parallel_rejects_non_function_pipeline_steps() {
        let response = run_inline_workflow_response(
            r#"
export const meta = {
  name: 'parallel-bad-step',
  description: 'Reject non-function parallel item steps',
}
return await parallel([
  [
    () => 'ok',
    'not-a-step',
  ],
]);
"#,
            None,
            &[],
        )
        .await;

        match response {
            codex_code_mode::RuntimeResponse::Result {
                error_text: Some(error),
                ..
            } => {
                assert!(
                    error.contains("parallel() item 1 pipeline step 2 must be a function"),
                    "{error}"
                );
            }
            other => panic!("expected parallel pipeline step validation error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn workflow_child_helper_runs_named_definition_with_args() {
        let child = ChildWorkflowDefinition {
            keys: vec!["child".to_string(), "child.js".to_string()],
            metadata: WorkflowMetadata {
                name: "child".to_string(),
                description: "Child workflow".to_string(),
                when_to_use: None,
                input_schema: None,
                phases: Vec::new(),
            },
            body: r#"
phase("child-phase", args);
return `${args.value}-child`;
"#
            .to_string(),
        };
        let text = run_inline_workflow_text_with_child_definitions(
            r#"
export const meta = {
  name: 'parent',
  description: 'Parent workflow',
}
return await workflow("child", { value: "ok" });
"#,
            &[child],
        )
        .await;

        assert!(text.contains("workflow: child start"), "{text}");
        assert!(text.contains("[child] phase: child-phase"), "{text}");
        assert!(text.contains("workflow: child complete"), "{text}");
        assert!(text.contains("ok-child"), "{text}");
    }

    #[tokio::test]
    async fn workflow_child_helper_runs_script_path_reference() {
        let child = ChildWorkflowDefinition {
            keys: vec![
                "child".to_string(),
                "child.js".to_string(),
                "./child.js".to_string(),
            ],
            metadata: WorkflowMetadata {
                name: "child".to_string(),
                description: "Child workflow".to_string(),
                when_to_use: None,
                input_schema: None,
                phases: Vec::new(),
            },
            body: "return `${args.value}-path-child`;".to_string(),
        };
        let text = run_inline_workflow_text_with_child_definitions(
            r#"
export const meta = {
  name: 'parent',
  description: 'Parent workflow',
}
return await workflow({ scriptPath: "./child.js" }, { value: "ok" });
"#,
            &[child],
        )
        .await;

        assert!(text.contains("workflow: child start"), "{text}");
        assert!(text.contains("workflow: child complete"), "{text}");
        assert!(text.contains("ok-path-child"), "{text}");
    }

    #[tokio::test]
    async fn workflow_child_helper_runs_object_name_reference() {
        let child = ChildWorkflowDefinition {
            keys: vec!["child".to_string()],
            metadata: WorkflowMetadata {
                name: "child".to_string(),
                description: "Child workflow".to_string(),
                when_to_use: None,
                input_schema: None,
                phases: Vec::new(),
            },
            body: "return `${args.value}-object-child`;".to_string(),
        };
        let text = run_inline_workflow_text_with_child_definitions(
            r#"
export const meta = {
  name: 'parent',
  description: 'Parent workflow',
}
return await workflow({ name: "child" }, { value: "ok" });
"#,
            &[child],
        )
        .await;

        assert!(text.contains("workflow: child start"), "{text}");
        assert!(text.contains("workflow: child complete"), "{text}");
        assert!(text.contains("ok-object-child"), "{text}");
    }

    #[tokio::test]
    async fn workflow_child_helper_blocks_second_level_nesting() {
        let child = ChildWorkflowDefinition {
            keys: vec!["child".to_string()],
            metadata: WorkflowMetadata {
                name: "child".to_string(),
                description: "Child workflow".to_string(),
                when_to_use: None,
                input_schema: None,
                phases: Vec::new(),
            },
            body: "return await workflow(\"grandchild\");".to_string(),
        };
        let grandchild = ChildWorkflowDefinition {
            keys: vec!["grandchild".to_string()],
            metadata: WorkflowMetadata {
                name: "grandchild".to_string(),
                description: "Grandchild workflow".to_string(),
                when_to_use: None,
                input_schema: None,
                phases: Vec::new(),
            },
            body: "return 'too-deep';".to_string(),
        };
        let text = run_inline_workflow_text_with_child_definitions(
            r#"
export const meta = {
  name: 'parent',
  description: 'Parent workflow',
}
return await workflow("child");
"#,
            &[child, grandchild],
        )
        .await;

        assert!(
            text.contains("workflow: child failed: Child workflow nesting is limited to one level"),
            "{text}"
        );
        assert!(!text.contains("too-deep"), "{text}");
    }

    #[tokio::test]
    async fn workflow_child_discovery_loads_configured_workflow_definitions() {
        use codex_utils_absolute_path::test_support::PathExt as _;

        let workflow_dir = tempfile::tempdir().expect("workflow dir");
        std::fs::write(
            workflow_dir.path().join("child.js"),
            r#"
export const meta = {
  name: 'child',
  description: 'Direct child workflow',
}
return args.value;
"#,
        )
        .expect("write direct child");
        let folder_dir = workflow_dir.path().join("folder");
        std::fs::create_dir_all(&folder_dir).expect("create folder workflow dir");
        std::fs::write(
            folder_dir.join("workflow.js"),
            r#"
export const meta = {
  name: 'folder',
  description: 'Folder child workflow',
}
return 'folder';
"#,
        )
        .expect("write folder child");
        std::fs::write(
            workflow_dir.path().join("invalid.js"),
            "return 'missing meta';",
        )
        .expect("write invalid child");

        let (_session, mut turn) = crate::session::tests::make_session_and_context().await;
        std::sync::Arc::make_mut(&mut turn.config)
            .workflows
            .workflow_dirs = vec![workflow_dir.path().abs()];
        let root_source = WorkflowSource {
            name: "inline".to_string(),
            code: String::new(),
            kind: WorkflowSourceKind::Inline,
            path: None,
        };

        let definitions = collect_child_workflow_definitions(&turn, &root_source).await;

        let names = definitions
            .iter()
            .map(|definition| definition.metadata.name.as_str())
            .collect::<Vec<_>>();
        assert!(names.contains(&"child"), "{names:?}");
        assert!(names.contains(&"folder"), "{names:?}");
        assert!(!names.contains(&"invalid"), "{names:?}");
        let child = definitions
            .iter()
            .find(|definition| definition.metadata.name == "child")
            .expect("child definition");
        assert!(child.keys.iter().any(|key| key == "child"), "{child:?}");
        assert!(child.keys.iter().any(|key| key == "child.js"), "{child:?}");
        let folder = definitions
            .iter()
            .find(|definition| definition.metadata.name == "folder")
            .expect("folder definition");
        assert!(
            folder.keys.iter().any(|key| key == "folder/workflow.js"),
            "{folder:?}"
        );
    }

    #[tokio::test]
    async fn workflow_child_discovery_loads_namespaced_plugin_workflow_definitions() {
        use codex_utils_absolute_path::test_support::PathExt as _;

        let workflow_dir = tempfile::tempdir().expect("plugin workflow dir");
        std::fs::write(
            workflow_dir.path().join("release.js"),
            r#"
export const meta = {
  name: 'release',
  description: 'Plugin release workflow',
}
return 'plugin-release';
"#,
        )
        .expect("write plugin workflow");

        let (_session, mut turn) = crate::session::tests::make_session_and_context().await;
        let config = std::sync::Arc::make_mut(&mut turn.config);
        config.workflows.plugin_workflow_dirs = vec![crate::config::WorkflowPluginDirectory {
            namespace: "sample".to_string(),
            plugin_id: "sample@test".to_string(),
            dir: workflow_dir.path().abs(),
        }];
        let root_source = WorkflowSource {
            name: "inline".to_string(),
            code: String::new(),
            kind: WorkflowSourceKind::Inline,
            path: None,
        };

        let definitions = collect_child_workflow_definitions(&turn, &root_source).await;
        let release = definitions
            .iter()
            .find(|definition| definition.metadata.name == "release")
            .expect("plugin release definition");
        assert!(
            release.keys.iter().any(|key| key == "sample:release"),
            "{release:?}"
        );
        assert!(
            !release.keys.iter().any(|key| key == "release"),
            "{release:?}"
        );
    }

    #[tokio::test]
    async fn workflow_named_resolution_finds_namespaced_plugin_workflow() {
        use codex_utils_absolute_path::test_support::PathExt as _;

        let workflow_dir = tempfile::tempdir().expect("plugin workflow dir");
        let workflow_path = workflow_dir.path().join("release.js");
        std::fs::write(
            &workflow_path,
            r#"
export const meta = {
  name: 'release',
  description: 'Plugin release workflow',
}
return 'plugin-release';
"#,
        )
        .expect("write plugin workflow");

        let (_session, mut turn) = crate::session::tests::make_session_and_context().await;
        std::sync::Arc::make_mut(&mut turn.config)
            .workflows
            .plugin_workflow_dirs = vec![crate::config::WorkflowPluginDirectory {
            namespace: "sample".to_string(),
            plugin_id: "sample@test".to_string(),
            dir: workflow_dir.path().abs(),
        }];

        let resolved = find_named_workflow(&turn, "sample:release")
            .await
            .expect("resolve namespaced plugin workflow");
        assert_eq!(resolved, workflow_path);
        assert!(
            find_named_workflow(&turn, "release").await.is_err(),
            "plugin workflow should not leak into unnamespaced lookup"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn workflow_resolution_rejects_symlink_escapes() {
        use codex_utils_absolute_path::test_support::PathExt as _;
        use std::os::unix::fs::symlink;

        let workflow_dir = tempfile::tempdir().expect("workflow dir");
        let outside_dir = tempfile::tempdir().expect("outside dir");
        let outside_path = outside_dir.path().join("outside.js");
        std::fs::write(
            &outside_path,
            r#"
export const meta = {
  name: 'outside',
  description: 'Outside workflow',
}
return 'outside';
"#,
        )
        .expect("write outside workflow");
        symlink(&outside_path, workflow_dir.path().join("release.js"))
            .expect("symlink named workflow");
        symlink(&outside_path, workflow_dir.path().join("direct.js"))
            .expect("symlink script workflow");

        let (_session, mut turn) = crate::session::tests::make_session_and_context().await;
        std::sync::Arc::make_mut(&mut turn.config)
            .workflows
            .workflow_dirs = vec![workflow_dir.path().abs()];

        assert!(
            find_named_workflow(&turn, "release").await.is_err(),
            "named workflow symlink escaping configured roots should be rejected"
        );
        assert!(
            resolve_script_path(
                &turn,
                workflow_dir.path().join("direct.js").to_str().unwrap()
            )
            .await
            .is_err(),
            "scriptPath symlink escaping configured roots should be rejected"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn workflow_resume_rejects_artifact_symlink_escape() {
        use std::os::unix::fs::symlink;

        let outside_dir = tempfile::tempdir().expect("outside dir");
        let outside_path = outside_dir.path().join("outside.js");
        std::fs::write(
            &outside_path,
            r#"
export const meta = {
  name: 'outside',
  description: 'Outside workflow',
}
return 'outside';
"#,
        )
        .expect("write outside workflow");

        let (_session, turn) = crate::session::tests::make_session_and_context().await;
        let run_dir = workflow_run_snapshot_dir(&turn).join("wf_prev");
        std::fs::create_dir_all(&run_dir).expect("create run dir");
        let script_path = run_dir.join("script.js");
        symlink(&outside_path, &script_path).expect("symlink artifact script");
        std::fs::write(
            workflow_run_snapshot_dir(&turn).join("wf_prev.json"),
            serde_json::json!({
                "run_id": "wf_prev",
                "status": "completed",
                "script_path": script_path.display().to_string(),
                "source": { "kind": "inline" }
            })
            .to_string(),
        )
        .expect("write snapshot");

        let args = workflow_args_with_resume("wf_prev", serde_json::json!({}));
        let err = match resolve_resume_workflow_source(&turn, &args).await {
            Ok(_) => panic!("resume artifact symlink escape should fail"),
            Err(err) => err,
        };
        let message = match err {
            FunctionCallError::RespondToModel(message) => message,
            other => panic!("unexpected error: {other:?}"),
        };
        assert!(message.contains("outside the project"), "{message}");
    }

    #[tokio::test]
    async fn workflow_parallel_runs_independent_item_pipelines() {
        let text = run_inline_workflow_text(
            r#"
export const meta = {
  name: 'parallel-pipelines',
  description: 'Verify parallel items can be independent pipelines',
}
return await parallel([
  [
    () => 'a',
    (value) => value + 'b',
  ],
  [
    () => { throw new Error('first step failed'); },
    () => 'unreachable',
  ],
]);
"#,
        )
        .await;

        assert!(
            text.contains("pipeline step 1 failed: first step failed"),
            "{text}"
        );
        assert!(text.contains("\"ab\""), "{text}");
        assert!(text.contains("null"), "{text}");
    }

    #[tokio::test]
    async fn workflow_pipeline_runs_items_through_stages_independently() {
        let text = run_inline_workflow_text(
            r#"
export const meta = {
  name: 'pipeline-items',
  description: 'Verify Claude-style item pipeline',
}
return await pipeline(
  ['a', 'b', 'fail'],
  (value) => {
    if (value === 'fail') throw new Error('stage failed');
    return value + '1';
  },
  (value, original, index) => `${value}:${original}:${index}`
);
"#,
        )
        .await;

        assert!(
            text.contains("pipeline item 3 stage 1 failed: stage failed"),
            "{text}"
        );
        assert!(text.contains("\"a1:a:0\""), "{text}");
        assert!(text.contains("\"b1:b:1\""), "{text}");
        assert!(text.contains("null"), "{text}");
    }

    #[tokio::test]
    async fn workflow_pipeline_respects_concurrency_cap() {
        let text = run_inline_workflow_text(
            r#"
export const meta = {
  name: 'pipeline-concurrency',
  description: 'Verify item-stage pipeline branches are bounded',
}
let active = 0;
let maxActive = 0;
const total = WORKFLOW_CONCURRENCY_CAP + 3;
const results = await pipeline(
  Array.from({ length: total }, (_, index) => index),
  async (value) => {
    active += 1;
    if (active > maxActive) maxActive = active;
    await new Promise((resolve) => setTimeout(resolve, 5));
    active -= 1;
    return value;
  }
);
return {
  cap: WORKFLOW_CONCURRENCY_CAP,
  maxActive,
  count: results.length,
  ordered: results[0] === 0 && results.at(-1) === total - 1,
  withinCap: maxActive <= WORKFLOW_CONCURRENCY_CAP,
};
"#,
        )
        .await;

        assert!(text.contains("\"ordered\": true"), "{text}");
        assert!(text.contains("\"withinCap\": true"), "{text}");
    }

    #[tokio::test]
    async fn workflow_pipeline_without_stages_passes_through_items() {
        let text = run_inline_workflow_text(
            r#"
export const meta = {
  name: 'pipeline-no-stages',
  description: 'Verify item passthrough without stages',
}
return await pipeline(['a', Promise.resolve('b')]);
"#,
        )
        .await;

        assert!(text.contains("\"a\""), "{text}");
        assert!(text.contains("\"b\""), "{text}");
        assert!(!text.contains("pipeline step"), "{text}");
    }

    #[tokio::test]
    async fn workflow_pipeline_without_stages_preserves_legacy_step_list() {
        let text = run_inline_workflow_text(
            r#"
export const meta = {
  name: 'pipeline-legacy-steps',
  description: 'Verify function-only pipeline remains a step list',
}
return await pipeline([
  () => 'a',
  (value) => value + 'b',
]);
"#,
        )
        .await;

        assert!(text.contains("ab"), "{text}");
    }

    #[tokio::test]
    async fn workflow_budget_object_uses_no_target_semantics() {
        let text = run_inline_workflow_text(
            r#"
export const meta = {
  name: 'budget-object',
  description: 'Verify budget helper shape',
}
const legacy = budget();
return `${budget.total}:${budget.spent()}:${budget.remaining() === Infinity}:${legacy.remaining === Infinity}:${legacy.output.total === null}:${legacy.output.spent}:${legacy.output.remaining === Infinity}`;
"#,
        )
        .await;

        assert!(text.contains("null:0:true:true:true:0:true"), "{text}");
    }

    #[tokio::test]
    async fn workflow_budget_object_tracks_configured_output_budget() {
        let text = run_inline_workflow_text_with_output_budget(
            r#"
export const meta = {
  name: 'budget-output',
  description: 'Verify output budget helper accounting',
}
const before = budget();
log('12345678');
const after = budget();
return `${budget.total}:${budget.spent()}:${budget.remaining() === Infinity}:${before.output.total}:${before.output.spent}:${before.output.remaining}:${after.output.spent > before.output.spent}:${after.output.remaining < before.output.remaining}`;
"#,
            8,
        )
        .await;

        assert!(text.contains("null:0:true:8:0:8:true:true"), "{text}");
    }

    #[tokio::test]
    async fn workflow_result_must_be_json_serializable() {
        let cyclic = run_inline_workflow_response(
            r#"
export const meta = {
  name: 'cyclic-result',
  description: 'Reject cyclic result values',
}
const value = { ok: true };
value.self = value;
return value;
"#,
            None,
            &[],
        )
        .await;
        match cyclic {
            codex_code_mode::RuntimeResponse::Result {
                error_text: Some(error),
                ..
            } => {
                assert!(
                    error
                        .contains("must be JSON-serializable; circular references are not allowed"),
                    "{error}"
                );
            }
            other => panic!("expected cyclic result error, got {other:?}"),
        }

        let bigint = run_inline_workflow_response(
            r#"
export const meta = {
  name: 'bigint-result',
  description: 'Reject BigInt result values',
}
return BigInt(1);
"#,
            None,
            &[],
        )
        .await;
        match bigint {
            codex_code_mode::RuntimeResponse::Result {
                error_text: Some(error),
                ..
            } => {
                assert!(
                    error.contains(
                        "workflow result must be JSON-serializable; bigint is not allowed"
                    ),
                    "{error}"
                );
            }
            other => panic!("expected BigInt result error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn workflow_helper_result_must_be_json_serializable() {
        let response = run_inline_workflow_response(
            r#"
export const meta = {
  name: 'helper-cyclic-result',
  description: 'Reject unserializable helper-drained results',
}
workflow(async () => {
  const value = { ok: true };
  value.self = value;
  return value;
});
"#,
            None,
            &[],
        )
        .await;

        match response {
            codex_code_mode::RuntimeResponse::Result {
                error_text: Some(error),
                ..
            } => {
                assert!(
                    error
                        .contains("must be JSON-serializable; circular references are not allowed"),
                    "{error}"
                );
            }
            other => panic!("expected helper result serialization error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn workflow_input_schema_validates_args_before_body_runs() {
        let script = r#"
export const meta = {
  name: 'schema-args',
  description: 'Validate workflow args',
  inputSchema: {
    type: 'object',
    properties: {
      channel: { type: 'string', enum: ['alpha', 'stable'] },
      count: { type: 'integer' },
      tags: { type: 'array', items: { type: 'string' } },
    },
    required: ['channel', 'count'],
  },
}
return `${args.channel}:${args.count}:${args.tags.join(',')}`;
"#;

        let valid = output_text(
            run_inline_workflow_response(
                script,
                Some(serde_json::json!({
                    "channel": "alpha",
                    "count": 2,
                    "tags": ["build", "ship"]
                })),
                &[],
            )
            .await,
        );
        assert!(valid.contains("alpha:2:build,ship"), "{valid}");

        let missing = run_inline_workflow_response(
            script,
            Some(serde_json::json!({ "channel": "alpha" })),
            &[],
        )
        .await;
        match missing {
            codex_code_mode::RuntimeResponse::Result {
                error_text: Some(error),
                ..
            } => {
                assert!(error.contains("args.count is required"), "{error}");
            }
            other => panic!("expected schema validation error, got {other:?}"),
        }

        let wrong_type = run_inline_workflow_response(
            script,
            Some(serde_json::json!({
                "channel": "alpha",
                "count": "2",
                "tags": ["build"]
            })),
            &[],
        )
        .await;
        match wrong_type {
            codex_code_mode::RuntimeResponse::Result {
                error_text: Some(error),
                ..
            } => {
                assert!(error.contains("args.count must be integer"), "{error}");
            }
            other => panic!("expected schema validation error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn workflow_agent_helper_replays_matching_journal_entry() {
        let args = WorkflowArgs {
            name: None,
            script: Some(
                r#"
export const meta = {
  name: 'agent-journal',
  description: 'Replay cached agent output',
}
const result = await agent('cached task');
return `cached:${result}`;
"#
                .to_string(),
            ),
            script_path: None,
            args: None,
            resume_from_run_id: Some("wf_prior".to_string()),
            title: None,
            description: None,
            max_output_tokens: None,
        };
        let validated =
            validate_workflow_script(args.script.as_deref().expect("script")).expect("valid");
        let script = build_test_workflow_script(
            "wf_test",
            &args,
            &validated,
            &[],
            &[WorkflowAgentJournalEntry {
                entry_type: Some("result".to_string()),
                key: r#"{"message":"cached task"}"#.to_string(),
                agent_id: Some("legacy-agent".to_string()),
                child: None,
                child_run_id: None,
                result: Some(JsonValue::String("journal-ok".to_string())),
            }],
            None,
        )
        .expect("workflow script");
        let service = codex_code_mode::CodeModeService::new();
        let response = service
            .execute(codex_code_mode::ExecuteRequest {
                tool_call_id: "call-1".to_string(),
                enabled_tools: Vec::new(),
                source: script,
                yield_time_ms: None,
                max_output_tokens: None,
            })
            .await
            .expect("start workflow script")
            .initial_response()
            .await
            .expect("workflow result");
        let text = output_text(response);

        assert!(text.contains("cached:journal-ok"), "{text}");
        assert!(!text.contains("Workflow tool not available"), "{text}");
    }

    #[tokio::test]
    async fn workflow_agent_helper_replays_chained_v2_journal_entries() {
        let first_key =
            test_agent_journal_key("", "first task", serde_json::json!({ "model": "gpt-test" }));
        let second_key = test_agent_journal_key(
            &first_key,
            "second task",
            serde_json::json!({ "agentType": "reviewer" }),
        );
        let args = WorkflowArgs {
            name: None,
            script: Some(
                r#"
export const meta = {
  name: 'agent-journal-chain',
  description: 'Replay chained cached agent output',
}
const first = await agent('first task', { model: 'gpt-test' });
const second = await agent('second task', { agentType: 'reviewer' });
return `cached:${first}/${second}`;
"#
                .to_string(),
            ),
            script_path: None,
            args: None,
            resume_from_run_id: Some("wf_prior".to_string()),
            title: None,
            description: None,
            max_output_tokens: None,
        };
        let validated =
            validate_workflow_script(args.script.as_deref().expect("script")).expect("valid");
        let script = build_test_workflow_script(
            "wf_test",
            &args,
            &validated,
            &[],
            &[
                WorkflowAgentJournalEntry {
                    entry_type: Some("result".to_string()),
                    key: first_key,
                    agent_id: Some("agent-one".to_string()),
                    child: None,
                    child_run_id: None,
                    result: Some(JsonValue::String("one".to_string())),
                },
                WorkflowAgentJournalEntry {
                    entry_type: Some("result".to_string()),
                    key: second_key,
                    agent_id: Some("agent-two".to_string()),
                    child: None,
                    child_run_id: None,
                    result: Some(JsonValue::String("two".to_string())),
                },
            ],
            None,
        )
        .expect("workflow script");
        let service = codex_code_mode::CodeModeService::new();
        let response = service
            .execute(codex_code_mode::ExecuteRequest {
                tool_call_id: "call-1".to_string(),
                enabled_tools: Vec::new(),
                source: script,
                yield_time_ms: None,
                max_output_tokens: None,
            })
            .await
            .expect("start workflow script")
            .initial_response()
            .await
            .expect("workflow result");
        let text = output_text(response);

        assert!(text.contains("cached:one/two"), "{text}");
        assert!(!text.contains("Workflow tool not available"), "{text}");
    }

    #[tokio::test]
    async fn workflow_agent_helper_stops_cache_replay_after_first_journal_miss() {
        let first_key = test_agent_journal_key("", "first task", serde_json::json!({}));
        let second_key = test_agent_journal_key(&first_key, "second task", serde_json::json!({}));
        let args = WorkflowArgs {
            name: None,
            script: Some(
                r#"
export const meta = {
  name: 'agent-journal-linear-miss',
  description: 'Do not replay later journal entries after an earlier miss',
}
const first = await agent('first task');
const second = await agent('second task');
return `result:${first}/${second}`;
"#
                .to_string(),
            ),
            script_path: None,
            args: None,
            resume_from_run_id: Some("wf_prior".to_string()),
            title: None,
            description: None,
            max_output_tokens: None,
        };
        let validated =
            validate_workflow_script(args.script.as_deref().expect("script")).expect("valid");
        let script = build_test_workflow_script(
            "wf_test",
            &args,
            &validated,
            &[],
            &[WorkflowAgentJournalEntry {
                entry_type: Some("result".to_string()),
                key: second_key,
                agent_id: Some("stale-second-agent".to_string()),
                child: None,
                child_run_id: None,
                result: Some(JsonValue::String("stale-two".to_string())),
            }],
            None,
        )
        .expect("workflow script");
        let probe = Arc::new(WorkflowAgentRuntimeProbe::always_complete());
        let service = codex_code_mode::CodeModeService::with_delegate(probe.clone());
        let response = service
            .execute(codex_code_mode::ExecuteRequest {
                tool_call_id: "call-1".to_string(),
                enabled_tools: workflow_agent_test_tools(),
                source: script,
                yield_time_ms: None,
                max_output_tokens: None,
            })
            .await
            .expect("start workflow script")
            .initial_response()
            .await
            .expect("workflow result");
        let text = output_text(response);

        assert!(text.contains("result:agent-ok/agent-ok"), "{text}");
        assert!(!text.contains("stale-two"), "{text}");
        assert_eq!(probe.spawns().len(), 2);
    }

    #[tokio::test]
    async fn workflow_agent_timeout_interrupts_retries_respawns_and_propagates_result() {
        let args = WorkflowArgs {
            name: None,
            script: Some(
                r#"
export const meta = {
  name: 'agent-timeout-retry',
  description: 'Retry a timed out workflow agent branch',
}
const results = await parallel([
  () => agent('slow task', {
    label: 'slow agent',
    timeoutMs: 1000,
    controlPollMs: 1000,
    progressStallMs: false,
    retries: 1,
  }),
  () => 'parallel-branch-ok',
]);
return results.join('|');
"#
                .to_string(),
            ),
            script_path: None,
            args: None,
            resume_from_run_id: None,
            title: None,
            description: None,
            max_output_tokens: None,
        };
        let validated =
            validate_workflow_script(args.script.as_deref().expect("script")).expect("valid");
        let script = build_test_workflow_script_with_concurrency_cap(
            "wf_test",
            &args,
            &validated,
            &[],
            &[],
            None,
            2,
        )
        .expect("workflow script");
        let probe = Arc::new(WorkflowAgentRuntimeProbe::default());
        let service = codex_code_mode::CodeModeService::with_delegate(probe.clone());

        let response = service
            .execute(codex_code_mode::ExecuteRequest {
                tool_call_id: "call-1".to_string(),
                enabled_tools: workflow_agent_test_tools(),
                source: script,
                yield_time_ms: None,
                max_output_tokens: None,
            })
            .await
            .expect("start workflow script")
            .initial_response()
            .await
            .expect("workflow result");
        let text = output_text(response);

        assert!(text.contains("agent-ok|parallel-branch-ok"), "{text}");
        assert_eq!(probe.spawns(), vec!["slow_agent", "slow_agent_retry_1"]);
        assert_eq!(probe.wait_calls(), 2);
        assert_eq!(probe.interrupts(), vec!["slow_agent"]);
        assert_eq!(probe.interrupt_reasons(), vec![Some("stalled".to_string())]);
        assert!(
            probe
                .control_polls()
                .iter()
                .any(|agent_id| agent_id == "slow_agent"),
            "expected workflow control to poll the first agent"
        );
        assert!(
            probe
                .control_polls()
                .iter()
                .any(|agent_id| agent_id == "slow_agent_retry_1"),
            "expected workflow control to poll the retried agent"
        );

        let notifications = probe.notifications();
        assert!(
            notifications.iter().any(|event| {
                event.get("event").and_then(JsonValue::as_str) == Some("agent_stalled")
                    && event.get("agent").and_then(JsonValue::as_str) == Some("slow_agent")
                    && event.get("agentId").and_then(JsonValue::as_str) == Some("slow_agent")
            }),
            "missing agent_stalled notification: {notifications:?}"
        );
        assert!(
            notifications.iter().any(|event| {
                event.get("event").and_then(JsonValue::as_str) == Some("agent_retry")
                    && event.get("agent").and_then(JsonValue::as_str) == Some("slow_agent")
                    && event.get("message").and_then(JsonValue::as_str) == Some("1/1")
            }),
            "missing agent_retry notification: {notifications:?}"
        );
        assert!(
            notifications.iter().any(|event| {
                event.get("event").and_then(JsonValue::as_str) == Some("agent_complete")
                    && event.get("agent").and_then(JsonValue::as_str) == Some("slow_agent_retry_1")
            }),
            "missing retry completion notification: {notifications:?}"
        );
        assert!(
            notifications.iter().any(|event| {
                event.get("event").and_then(JsonValue::as_str) == Some("agent_journal_entry")
                    && event.get("agent").and_then(JsonValue::as_str) == Some("slow_agent_retry_1")
                    && event.get("data").and_then(|data| data.get("result"))
                        == Some(&JsonValue::String("agent-ok".to_string()))
            }),
            "missing successful retry journal notification: {notifications:?}"
        );
        assert_eq!(
            notifications
                .iter()
                .filter(|event| {
                    event.get("event").and_then(JsonValue::as_str) == Some("agent_journal_started")
                })
                .count(),
            2,
            "expected a journal started marker for both attempts: {notifications:?}"
        );
    }

    #[tokio::test]
    async fn workflow_child_helper_replays_matching_journal_entry() {
        let child = ChildWorkflowDefinition {
            keys: vec!["child".to_string()],
            metadata: WorkflowMetadata {
                name: "child".to_string(),
                description: "Child workflow".to_string(),
                when_to_use: None,
                input_schema: None,
                phases: Vec::new(),
            },
            body: r#"
log("child-body-ran");
return `${args.value}-live`;
"#
            .to_string(),
        };
        let args = WorkflowArgs {
            name: None,
            script: Some(
                r#"
export const meta = {
  name: 'parent',
  description: 'Replay cached child output',
}
return await workflow("child", { value: "ok" });
"#
                .to_string(),
            ),
            script_path: None,
            args: None,
            resume_from_run_id: Some("wf_prior".to_string()),
            title: None,
            description: None,
            max_output_tokens: None,
        };
        let validated =
            validate_workflow_script(args.script.as_deref().expect("script")).expect("valid");
        let key =
            test_child_journal_key("", "child", "child", serde_json::json!({ "value": "ok" }));
        let script = build_test_workflow_script(
            "wf_test",
            &args,
            &validated,
            &[child],
            &[WorkflowAgentJournalEntry {
                entry_type: Some("child_result".to_string()),
                key,
                agent_id: None,
                child: Some("child".to_string()),
                child_run_id: Some("child#1".to_string()),
                result: Some(JsonValue::String("cached-child".to_string())),
            }],
            None,
        )
        .expect("workflow script");
        let service = codex_code_mode::CodeModeService::new();
        let response = service
            .execute(codex_code_mode::ExecuteRequest {
                tool_call_id: "call-1".to_string(),
                enabled_tools: Vec::new(),
                source: script,
                yield_time_ms: None,
                max_output_tokens: None,
            })
            .await
            .expect("start workflow script")
            .initial_response()
            .await
            .expect("workflow result");
        let text = output_text(response);

        assert!(text.contains("workflow: child cache hit"), "{text}");
        assert!(text.contains("cached-child"), "{text}");
        assert!(!text.contains("child-body-ran"), "{text}");
    }

    #[test]
    fn workflow_concurrency_cap_respects_agent_capacity() {
        assert_eq!(3, workflow_concurrency_cap(16, Some(3)));
        assert_eq!(4, workflow_concurrency_cap(4, Some(8)));
        assert_eq!(1, workflow_concurrency_cap(16, Some(0)));
        assert_eq!(8, workflow_concurrency_cap(8, None));
    }

    #[test]
    fn workflow_wrapper_has_agent_cap_guardrail() {
        let args = WorkflowArgs {
            name: None,
            script: Some(
                r#"
export const meta = {
  name: 'agent-cap',
  description: 'Check generated agent cap',
}
return 'ok';
"#
                .to_string(),
            ),
            script_path: None,
            args: None,
            resume_from_run_id: None,
            title: None,
            description: None,
            max_output_tokens: None,
        };
        let validated =
            validate_workflow_script(args.script.as_deref().expect("script")).expect("valid");
        let script = build_test_workflow_script_with_concurrency_cap(
            "wf_test",
            &args,
            &validated,
            &[],
            &[],
            None,
            3,
        )
        .expect("workflow script");

        assert!(script.contains("const WORKFLOW_AGENT_CALL_CAP = 1000;"));
        assert!(script.contains("Workflow agent() call cap reached"));
        assert!(script.contains("const WORKFLOW_CONCURRENCY_CAP = 3;"));
        assert!(script.contains("const WORKFLOW_LOG_CAP = 1000;"));
        assert!(script.contains("function __workflowEmitLog(message)"));
        assert!(script.contains("workflow log cap reached (${WORKFLOW_LOG_CAP})"));
        assert!(script.contains("function __workflowMetrics(extra = {})"));
        assert!(script.contains("agentCount: __workflowState.agentCount"));
        assert!(script.contains("childCount: __workflowState.childCount"));
        assert!(script.contains("logCount: __workflowState.logCount"));
        assert!(
            script.contains(
                "__workflowProgress(\"workflow_complete\", { data: __workflowMetrics() })"
            )
        );
        assert!(script.contains(
            "__workflowProgress(\"workflow_failed\", { message: __workflowErrorMessage(error), data: __workflowMetrics({ error: __workflowErrorMessage(error) }) })"
        ));
        assert!(script.contains("const console = Object.freeze({"));
        assert!(script.contains("info: log"));
        assert!(script.contains("warn: log"));
        assert!(script.contains("error: log"));
        assert!(script.contains("debug: log"));
        assert!(script.contains("dir: log"));
        assert!(script.contains("function __runWorkflowLimited(items, runner)"));
        assert!(script.contains("Math.min(WORKFLOW_CONCURRENCY_CAP, items.length)"));
        assert!(
            script.contains("await __runWorkflowLimited(items, (item) => __runParallelItem(item))")
        );
        assert!(script.contains("const WORKFLOW_SEQUENCE_ITEM_CAP = 4096;"));
        assert!(script.contains("function __assertWorkflowParallelItems(items)"));
        assert!(script.contains("parallel() expects an array of functions or step arrays"));
        assert!(script.contains("function __runPipelineItemsWithoutStages(items)"));
    }

    #[test]
    fn workflow_wrapper_emits_structured_sequence_failure_progress() {
        let args = WorkflowArgs {
            name: None,
            script: Some(
                r#"
export const meta = {
  name: 'sequence-metadata',
  description: 'Verify sequence helper metadata',
}
return await parallel([]);
"#
                .to_string(),
            ),
            script_path: None,
            args: None,
            resume_from_run_id: None,
            title: None,
            description: None,
            max_output_tokens: None,
        };
        let validated =
            validate_workflow_script(args.script.as_deref().expect("script")).expect("valid");
        let script = build_test_workflow_script("wf_test", &args, &validated, &[], &[], None)
            .expect("workflow script");

        assert!(script.contains("data: { stepIndex: index + 1, error: errorMessage }"));
        assert!(script.contains("data: { itemIndex: index + 1, error: errorMessage }"));
        assert!(script.contains(
            "data: { itemIndex: itemIndex + 1, stageIndex: stageIndex + 1, error: errorMessage }"
        ));
        assert!(script.contains("childCount: 0"));
        assert!(script.contains("const childRunId = `${definition.name}#${childIndex}`"));
        assert!(script.contains("const __workflowChildJournal = new Map();"));
        assert!(script.contains("codex-child-v1:${__workflowHashString(material)}"));
        assert!(script.contains("__workflowProgress(\"child_journal_entry\""));
        assert!(script.contains("__workflowProgress(\"child_journal_hit\""));
        assert!(script.contains("typeof ref.name === \"string\""));
        assert!(script.contains("\"name\" in bodyOrRef"));
    }

    #[test]
    fn workflow_validation_requires_meta_header() {
        let err = validate_workflow_script("phase('Scan');").expect_err("missing meta should fail");
        let message = match err {
            FunctionCallError::RespondToModel(message) => message,
            other => panic!("unexpected error: {other:?}"),
        };

        assert!(message.contains("export const meta"), "{message}");
    }

    #[test]
    fn workflow_validation_requires_literal_meta_strings() {
        let err = validate_workflow_script(
            r#"
export const meta = {
  name: workflowName(),
  description: 'bad',
}
phase('Scan');
"#,
        )
        .expect_err("dynamic meta should fail");
        let message = match err {
            FunctionCallError::RespondToModel(message) => message,
            other => panic!("unexpected error: {other:?}"),
        };

        assert!(message.contains("pure object literal"), "{message}");
    }

    #[test]
    fn workflow_validation_rejects_nondeterministic_body_calls() {
        let err = validate_workflow_script(
            r#"
export const meta = {
  name: 'clock',
  description: 'Use clock',
}
const now = Date.now();
return now;
"#,
        )
        .expect_err("nondeterministic body should fail");
        let message = match err {
            FunctionCallError::RespondToModel(message) => message,
            other => panic!("unexpected error: {other:?}"),
        };

        assert!(message.contains("Date.now"), "{message}");
    }

    #[test]
    fn workflow_validation_rejects_forbidden_calls_with_whitespace() {
        let cases = [
            ("await import ('./x.js');", "dynamic import"),
            ("const fs = require ('fs');", "CommonJS require"),
            ("eval ('1 + 1');", "eval"),
            ("const fn = Function ('return 1');", "Function constructor"),
            (
                "const fn = new Function ('return 1');",
                "Function constructor",
            ),
            ("const now = Date.now ();", "Date.now"),
            ("const date = new Date ();", "new Date"),
            ("const date = Date ();", "Date"),
            ("const value = globalThis.eval ('1 + 1');", "eval"),
            ("const random = Math.random ();", "Math.random"),
            ("const now = Date . now ();", "Date.now"),
            ("const random = Math . random ();", "Math.random"),
            ("const now = Date ['now'] ();", "Date.now"),
            ("const random = Math [\"random\"] ();", "Math.random"),
            (
                "const fn = globalThis ['Function'] ('return 1');",
                "Function constructor",
            ),
            ("globalThis [\"eval\"] ('1 + 1');", "eval"),
            (
                "Reflect.construct (Function, ['return 1']);",
                "Reflect.construct",
            ),
            (
                "(() => {}).constructor ('return 1');",
                "constructor-chain execution",
            ),
            ("eval /* comment */ ('1 + 1');", "eval"),
            ("await import /* comment */ ('./x.js');", "dynamic import"),
            (
                "const fs = require /* comment */ ('fs');",
                "CommonJS require",
            ),
            (
                "const fn = new /* comment */ Function ('return 1');",
                "Function constructor",
            ),
            ("const date = new /* comment */ Date ();", "new Date"),
            (
                "const now = Date /* comment */ . now /* comment */ ();",
                "Date.now",
            ),
            (
                "const random = Math /* comment */ . random /* comment */ ();",
                "Math.random",
            ),
            (
                "const random = Math // comment\n . random ();",
                "Math.random",
            ),
        ];

        for (body, label) in cases {
            let script = format!(
                r#"
export const meta = {{
  name: 'forbidden',
  description: 'Forbidden call',
}}
{body}
return 'unreachable';
"#
            );
            let err = match validate_workflow_script(script.as_str()) {
                Ok(_) => panic!("expected `{body}` to be rejected"),
                Err(err) => err,
            };
            let message = match err {
                FunctionCallError::RespondToModel(message) => message,
                other => panic!("unexpected error: {other:?}"),
            };
            assert!(message.contains(label), "{body}: {message}");
        }
    }

    #[test]
    fn workflow_validation_ignores_forbidden_call_text_in_strings_and_comments() {
        let validated = validate_workflow_script(
            r#"
export const meta = {
  name: 'scanner-boundaries',
  description: 'Forbidden words in non-code positions',
}
// eval ('ignored')
/* require ('ignored') */
const text = "import ('ignored') Date.now () Math.random () new Date ()";
return text;
"#,
        )
        .expect("non-code forbidden text should be ignored");

        assert!(validated.body.contains("eval ('ignored')"));
        assert!(validated.body.contains("return text"));
    }

    #[test]
    fn workflow_validation_strips_meta_from_executable_body() {
        let validated = validate_workflow_script(
            r#"
export const meta = {
  name: 'strip-meta',
  description: 'Strip metadata before execution',
  whenToUse: 'When testing parser metadata',
  inputSchema: {
    type: 'object',
    properties: { channel: { type: 'string' } },
    required: ['channel'],
  },
  phases: [
    { title: 'Run' },
    { title: 'Review', model: 'xhigh' },
  ],
};
phase('Run');
return 'ok';
"#,
        )
        .expect("valid workflow");

        assert_eq!("strip-meta", validated.metadata.name);
        assert_eq!(
            "Strip metadata before execution",
            validated.metadata.description
        );
        assert_eq!(
            Some("When testing parser metadata"),
            validated.metadata.when_to_use.as_deref()
        );
        assert_eq!(
            Some(
                "{
    type: 'object',
    properties: { channel: { type: 'string' } },
    required: ['channel'],
  }"
            ),
            validated.metadata.input_schema.as_deref()
        );
        assert_eq!(2, validated.metadata.phases.len());
        assert_eq!("Run", validated.metadata.phases[0].title);
        assert_eq!(None, validated.metadata.phases[0].model.as_deref());
        assert_eq!("Review", validated.metadata.phases[1].title);
        assert_eq!(Some("xhigh"), validated.metadata.phases[1].model.as_deref());
        assert!(!validated.body.contains("export const meta"));
        assert!(validated.body.contains("phase('Run')"));
    }

    #[test]
    fn workflow_validation_rejects_invalid_phase_metadata() {
        let err = validate_workflow_script(
            r#"
export const meta = {
  name: 'bad-phases',
  description: 'Bad phase metadata',
  phases: ['Review'],
}
return 'ok';
"#,
        )
        .expect_err("invalid phase metadata should fail");
        let message = match err {
            FunctionCallError::RespondToModel(message) => message,
            other => panic!("unexpected error: {other:?}"),
        };

        assert!(message.contains("phases"), "{message}");
        assert!(message.contains("object literals"), "{message}");
    }

    #[test]
    fn workflow_validation_rejects_invalid_input_schema_metadata() {
        let err = validate_workflow_script(
            r#"
export const meta = {
  name: 'bad-schema',
  description: 'Bad schema metadata',
  inputSchema: makeSchema(),
}
return 'ok';
"#,
        )
        .expect_err("invalid input schema metadata should fail");
        let message = match err {
            FunctionCallError::RespondToModel(message) => message,
            other => panic!("unexpected error: {other:?}"),
        };

        assert!(message.contains("pure object literal"), "{message}");
    }

    #[test]
    fn workflow_run_status_tracks_runtime_response_kind() {
        let content_items = Vec::new();

        assert_eq!(
            WorkflowRunStatus::Running,
            workflow_run_status_for_runtime_response(&codex_code_mode::RuntimeResponse::Yielded {
                cell_id: codex_code_mode::CellId::new("running".to_string()),
                content_items: content_items.clone(),
            })
        );
        assert_eq!(
            WorkflowRunStatus::Terminated,
            workflow_run_status_for_runtime_response(
                &codex_code_mode::RuntimeResponse::Terminated {
                    cell_id: codex_code_mode::CellId::new("terminated".to_string()),
                    content_items: content_items.clone(),
                }
            )
        );
        assert_eq!(
            WorkflowRunStatus::Completed,
            workflow_run_status_for_runtime_response(&codex_code_mode::RuntimeResponse::Result {
                cell_id: codex_code_mode::CellId::new("completed".to_string()),
                content_items: content_items.clone(),
                error_text: None,
            })
        );
        assert_eq!(
            WorkflowRunStatus::Failed,
            workflow_run_status_for_runtime_response(&codex_code_mode::RuntimeResponse::Result {
                cell_id: codex_code_mode::CellId::new("failed".to_string()),
                content_items,
                error_text: Some("boom".to_string()),
            })
        );
    }

    #[test]
    fn workflow_status_prefix_includes_run_id() {
        let mut output =
            FunctionToolOutput::from_text("Script completed\nok".to_string(), Some(true));
        let artifacts = WorkflowRunArtifacts {
            run_dir: PathBuf::from("/tmp/workflows/wf_abc123"),
            script_path: Some(PathBuf::from("/tmp/workflows/wf_abc123/script.js")),
            transcript_dir: PathBuf::from("/tmp/workflows/wf_abc123/transcripts"),
        };
        let args = WorkflowArgs {
            name: None,
            script: None,
            script_path: None,
            args: None,
            resume_from_run_id: None,
            title: None,
            description: None,
            max_output_tokens: None,
        };

        prefix_workflow_status(&mut output, "release", "wf_abc123", Some(&artifacts), &args);
        let text = output.into_text();

        assert!(
            text.starts_with("Workflow `release`\nRun: `wf_abc123`\n"),
            "{text}"
        );
        assert!(
            text.contains("Script: `/tmp/workflows/wf_abc123/script.js`"),
            "{text}"
        );
        assert!(
            text.contains("Transcripts: `/tmp/workflows/wf_abc123/transcripts`"),
            "{text}"
        );
        assert!(text.contains("Monitor: `/workflows`"), "{text}");
        assert!(text.contains("resumeFromRunId: \"wf_abc123\""), "{text}");
        assert!(text.contains("Script completed"), "{text}");
    }

    fn workflow_args_with_resume(run_id: &str, args: serde_json::Value) -> WorkflowArgs {
        WorkflowArgs {
            name: None,
            script: None,
            script_path: None,
            args: Some(args),
            resume_from_run_id: Some(run_id.to_string()),
            title: None,
            description: None,
            max_output_tokens: None,
        }
    }
}
