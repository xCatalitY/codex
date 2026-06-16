use chrono::Utc;
use serde::Deserialize;
use serde_json::Number as JsonNumber;
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;
use tokio::io::AsyncWriteExt;

use crate::function_tool::FunctionCallError;
use crate::hook_runtime::run_workflow_task_completed_hooks;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::context::ToolPayload;
use crate::tools::context::boxed_tool_output;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::PostToolUsePayload;
use crate::tools::registry::PreToolUsePayload;
use crate::tools::registry::ToolExecutor;
use codex_tools::ToolName;
use codex_tools::ToolSpec;
use uuid::Uuid;

use super::DEFAULT_WAIT_YIELD_TIME_MS;
use super::ExecContext;
use super::WAIT_TOOL_NAME;
use super::handle_runtime_response;
use super::wait_spec::create_wait_tool;

pub struct CodeModeWaitHandler;

const WORKFLOW_RUNS_DIR: &str = "workflow-runs";
const WORKFLOW_ACTIVE_RUNS_DIR: &str = "active";
const WORKFLOW_TRANSCRIPT_RUN_FILE: &str = "run.json";
const WORKFLOW_TRANSCRIPT_OUTPUT_FILE: &str = "output.txt";
const WORKFLOW_TRANSCRIPT_ERROR_FILE: &str = "error.txt";
const WORKFLOW_AGENT_JOURNAL_FILE: &str = "journal.jsonl";
const WORKFLOW_AGENT_TRANSCRIPT_FILE_MAX_AGENT_ID_CHARS: usize = 128;
const WORKFLOW_AGENT_TRANSCRIPT_NOTIFICATION_MAX_LINES: usize = 128;
const WORKFLOW_AGENT_TRANSCRIPT_NOTIFICATION_MAX_LINE_BYTES: usize = 16 * 1024;
const WORKFLOW_OUTPUT_PREVIEW_MAX_CHARS: usize = 4000;
const WORKFLOW_PROGRESS_NOTIFICATION_TYPE: &str = "codex_workflow_progress";
const WORKFLOW_PROGRESS_MAX_EVENTS: usize = 200;
const WORKFLOW_PROGRESS_FIELD_MAX_CHARS: usize = 512;
const WORKFLOW_PROGRESS_NOTIFICATION_MAX_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct WorkflowProgressUpdate {
    pub(crate) run_id: String,
    pub(crate) cell_id: String,
    pub(crate) event: String,
    pub(crate) unix_ms: i64,
    pub(crate) session_id: Option<String>,
    pub(crate) workflow_tool_call_id: Option<String>,
    pub(crate) cwd: Option<String>,
    pub(crate) git_branch: Option<String>,
    pub(crate) workflow: Option<String>,
    pub(crate) phase: Option<String>,
    pub(crate) agent: Option<String>,
    pub(crate) agent_id: Option<String>,
    pub(crate) child: Option<String>,
    pub(crate) child_index: Option<u64>,
    pub(crate) child_run_id: Option<String>,
    pub(crate) item_index: Option<u64>,
    pub(crate) stage_index: Option<u64>,
    pub(crate) step_index: Option<u64>,
    pub(crate) error: Option<String>,
    pub(crate) message: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum WorkflowProgressNotificationResult {
    NotWorkflowNotification,
    Consumed {
        update: Option<WorkflowProgressUpdate>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct WorkflowWaitUpdate {
    pub(crate) run_id: String,
    pub(crate) cell_id: String,
    pub(crate) workflow: Option<String>,
    pub(crate) previous_status: Option<String>,
    pub(crate) status: String,
    pub(crate) max_output_tokens: Option<usize>,
}

impl WorkflowWaitUpdate {
    pub(crate) fn completed_from_running(&self) -> bool {
        self.previous_status
            .as_deref()
            .is_some_and(workflow_status_is_active)
            && matches!(self.status.as_str(), "completed" | "failed" | "terminated")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WorkflowRunCellState {
    pub(crate) run_id: String,
    pub(crate) status: String,
    pub(crate) session_id: Option<String>,
    pub(crate) workflow_tool_call_id: Option<String>,
    pub(crate) cwd: Option<String>,
    pub(crate) git_branch: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WorkflowAgentTarget {
    pub(crate) run_id: String,
    pub(crate) cell_id: String,
    pub(crate) status: String,
    pub(crate) agent: Option<String>,
    pub(crate) agent_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WorkflowAgentControlRequest {
    pub(crate) action: String,
    pub(crate) event: String,
    pub(crate) message: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WorkflowAgentTranscriptTarget {
    pub(crate) transcript_path: PathBuf,
    pub(crate) mirror_transcript_path: Option<PathBuf>,
    pub(crate) envelope: WorkflowAgentTranscriptEnvelope,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct WorkflowAgentTranscriptEnvelope {
    pub(crate) agent_id: Option<String>,
    pub(crate) agent_name: Option<String>,
    pub(crate) session_kind: Option<String>,
    pub(crate) session_id: Option<String>,
    pub(crate) thread_id: Option<String>,
    pub(crate) parent_thread_id: Option<String>,
    pub(crate) run_id: Option<String>,
    pub(crate) cell_id: Option<String>,
    pub(crate) workflow_tool_call_id: Option<String>,
    pub(crate) cwd: Option<String>,
    pub(crate) version: Option<String>,
    pub(crate) git_branch: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ExecWaitArgs {
    cell_id: String,
    #[serde(default = "default_wait_yield_time_ms")]
    yield_time_ms: u64,
    #[serde(default)]
    max_tokens: Option<usize>,
    #[serde(default)]
    terminate: bool,
}

fn default_wait_yield_time_ms() -> u64 {
    DEFAULT_WAIT_YIELD_TIME_MS
}

fn parse_arguments<T>(arguments: &str) -> Result<T, FunctionCallError>
where
    T: for<'de> Deserialize<'de>,
{
    serde_json::from_str(arguments).map_err(|err| {
        FunctionCallError::RespondToModel(format!("failed to parse function arguments: {err}"))
    })
}

impl ToolExecutor<ToolInvocation> for CodeModeWaitHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain(WAIT_TOOL_NAME)
    }

    fn spec(&self) -> ToolSpec {
        create_wait_tool()
    }

    fn handle(&self, invocation: ToolInvocation) -> codex_tools::ToolExecutorFuture<'_> {
        Box::pin(self.handle_call(invocation))
    }
}

impl CodeModeWaitHandler {
    async fn handle_call(
        &self,
        invocation: ToolInvocation,
    ) -> Result<Box<dyn crate::tools::context::ToolOutput>, FunctionCallError> {
        let ToolInvocation {
            session,
            turn,
            tool_name,
            payload,
            ..
        } = invocation;

        match payload {
            ToolPayload::Function { arguments }
                if tool_name.namespace.is_none() && tool_name.name.as_str() == WAIT_TOOL_NAME =>
            {
                let args: ExecWaitArgs = parse_arguments(&arguments)?;
                let exec = ExecContext { session, turn };
                let started_at = std::time::Instant::now();
                let cell_id = codex_code_mode::CellId::new(args.cell_id);
                let wait_response = if args.terminate {
                    exec.session
                        .services
                        .code_mode_service
                        .terminate(cell_id)
                        .await
                } else {
                    exec.session
                        .services
                        .code_mode_service
                        .wait(codex_code_mode::WaitRequest {
                            cell_id,
                            yield_time_ms: args.yield_time_ms,
                        })
                        .await
                }
                .map_err(FunctionCallError::RespondToModel)?;
                let workflow_wait_update =
                    update_workflow_snapshot_for_wait(exec.turn.as_ref(), &wait_response).await;
                if let Some(update) = workflow_wait_update
                    .as_ref()
                    .filter(|update| update.completed_from_running())
                {
                    let workflow_name = update.workflow.as_deref().unwrap_or(&update.run_id);
                    run_workflow_task_completed_hooks(
                        &exec.session,
                        &exec.turn,
                        workflow_name,
                        &update.run_id,
                        Some(update.cell_id.as_str()),
                        update.status.as_str(),
                        None,
                    )
                    .await;
                }
                if let codex_code_mode::WaitOutcome::LiveCell(response) = &wait_response
                    && !matches!(response, codex_code_mode::RuntimeResponse::Yielded { .. })
                {
                    // Only a live-cell wait can close a CodeCell. A missing
                    // cell is still an ordinary `wait` tool result, but there
                    // is no runtime object for the reducer to complete.
                    let runtime_cell_id = match response {
                        codex_code_mode::RuntimeResponse::Yielded { cell_id, .. }
                        | codex_code_mode::RuntimeResponse::Terminated { cell_id, .. }
                        | codex_code_mode::RuntimeResponse::Result { cell_id, .. } => cell_id,
                    };
                    exec.session
                        .services
                        .rollout_thread_trace
                        .code_cell_trace_context(
                            exec.turn.sub_id.as_str(),
                            runtime_cell_id.as_str(),
                        )
                        .record_ended(response);
                    exec.session
                        .services
                        .code_mode_service
                        .finish_cell_dispatch(runtime_cell_id);
                }
                let max_output_tokens = effective_wait_max_output_tokens(
                    args.max_tokens,
                    workflow_wait_update
                        .as_ref()
                        .and_then(|update| update.max_output_tokens),
                );
                handle_runtime_response(&exec, wait_response.into(), max_output_tokens, started_at)
                    .await
                    .map(boxed_tool_output)
                    .map_err(FunctionCallError::RespondToModel)
            }
            _ => Err(FunctionCallError::RespondToModel(format!(
                "{WAIT_TOOL_NAME} expects JSON arguments"
            ))),
        }
    }
}

pub(crate) async fn update_workflow_snapshot_for_wait(
    turn: &crate::session::turn_context::TurnContext,
    wait_response: &codex_code_mode::WaitOutcome,
) -> Option<WorkflowWaitUpdate> {
    let snapshot_dir = turn.config.codex_home.join(WORKFLOW_RUNS_DIR).to_path_buf();
    match update_workflow_snapshot_in_dir(snapshot_dir.as_path(), wait_response).await {
        Ok(update) => update,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "failed to update workflow run snapshot after code-mode wait"
            );
            None
        }
    }
}

pub(crate) async fn update_workflow_snapshot_for_pause(
    turn: &crate::session::turn_context::TurnContext,
    pause_response: &codex_code_mode::WaitToPendingOutcome,
) -> Option<WorkflowWaitUpdate> {
    let snapshot_dir = turn.config.codex_home.join(WORKFLOW_RUNS_DIR).to_path_buf();
    match update_workflow_snapshot_for_pending_in_dir(snapshot_dir.as_path(), pause_response).await
    {
        Ok(update) => update,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "failed to update workflow run snapshot after workflow pause"
            );
            None
        }
    }
}

pub(crate) async fn active_workflow_run_for_cell(
    turn: &crate::session::turn_context::TurnContext,
    cell_id: &str,
) -> Option<WorkflowRunCellState> {
    let snapshot_dir = turn.config.codex_home.join(WORKFLOW_RUNS_DIR).to_path_buf();
    match newest_running_workflow_snapshot_for_cell(snapshot_dir.as_path(), cell_id).await {
        Ok(Some((_path, snapshot))) => Some(WorkflowRunCellState {
            run_id: workflow_snapshot_run_id(&snapshot)?,
            status: workflow_snapshot_status(&snapshot)?,
            session_id: workflow_string_field(&snapshot, &["session_id", "sessionId"]),
            workflow_tool_call_id: workflow_string_field(
                &snapshot,
                &["workflow_tool_call_id", "workflowToolCallId"],
            ),
            cwd: workflow_string_field(&snapshot, &["cwd"]),
            git_branch: workflow_string_field(&snapshot, &["git_branch", "gitBranch"]),
        }),
        Ok(None) => None,
        Err(err) => {
            tracing::warn!(
                error = %err,
                cell_id,
                "failed to find active workflow snapshot for code-mode cell"
            );
            None
        }
    }
}

pub(crate) async fn running_workflow_agent_for_run(
    turn: &crate::session::turn_context::TurnContext,
    run_id: &str,
    agent_id: &str,
) -> Option<WorkflowAgentTarget> {
    let snapshot_dir = turn.config.codex_home.join(WORKFLOW_RUNS_DIR).to_path_buf();
    match running_workflow_agent_for_run_in_dir(snapshot_dir.as_path(), run_id, agent_id).await {
        Ok(target) => target,
        Err(err) => {
            tracing::warn!(
                error = %err,
                run_id,
                agent_id,
                "failed to find running workflow agent"
            );
            None
        }
    }
}

pub(crate) async fn workflow_agent_control_request_for_run(
    turn: &crate::session::turn_context::TurnContext,
    run_id: &str,
    agent_id: &str,
) -> Option<WorkflowAgentControlRequest> {
    let snapshot_dir = turn.config.codex_home.join(WORKFLOW_RUNS_DIR).to_path_buf();
    match workflow_agent_control_request_for_run_in_dir(snapshot_dir.as_path(), run_id, agent_id)
        .await
    {
        Ok(request) => request,
        Err(err) => {
            tracing::warn!(
                error = %err,
                run_id,
                agent_id,
                "failed to read workflow agent control request"
            );
            None
        }
    }
}

pub(crate) async fn workflow_agent_transcript_target_for_cell(
    turn: &crate::session::turn_context::TurnContext,
    cell_id: &str,
    agent_id: &str,
) -> Option<WorkflowAgentTranscriptTarget> {
    let snapshot_dir = turn.config.codex_home.join(WORKFLOW_RUNS_DIR).to_path_buf();
    match workflow_agent_transcript_target_for_cell_in_dir(
        snapshot_dir.as_path(),
        cell_id,
        agent_id,
    )
    .await
    {
        Ok(target) => target,
        Err(err) => {
            tracing::warn!(
                error = %err,
                cell_id,
                agent_id,
                "failed to resolve workflow agent transcript target"
            );
            None
        }
    }
}

pub(crate) async fn append_workflow_agent_transcript_lines_to_target(
    target: &WorkflowAgentTranscriptTarget,
    lines: &[String],
) -> Result<(), String> {
    let primary_lines =
        workflow_agent_transcript_envelope_lines(target.transcript_path.as_path(), lines, target)
            .await;
    append_workflow_agent_transcript(target.transcript_path.as_path(), &primary_lines).await?;
    if let Some(mirror_path) = target.mirror_transcript_path.as_ref() {
        let mirror_lines =
            workflow_agent_transcript_envelope_lines(mirror_path.as_path(), lines, target).await;
        append_workflow_agent_transcript(mirror_path.as_path(), &mirror_lines).await?;
    }
    Ok(())
}

pub(crate) async fn record_workflow_agent_control_event(
    turn: &crate::session::turn_context::TurnContext,
    target: &WorkflowAgentTarget,
    event: &str,
    message: Option<&str>,
) -> Option<WorkflowProgressUpdate> {
    let snapshot_dir = turn.config.codex_home.join(WORKFLOW_RUNS_DIR).to_path_buf();
    match record_workflow_agent_control_event_in_dir(snapshot_dir.as_path(), target, event, message)
        .await
    {
        Ok(update) => update,
        Err(err) => {
            tracing::warn!(
                error = %err,
                run_id = %target.run_id,
                agent_id = %target.agent_id,
                "failed to record workflow agent control event"
            );
            None
        }
    }
}

pub(crate) async fn update_workflow_snapshot_for_notify(
    turn: &crate::session::turn_context::TurnContext,
    cell_id: &codex_code_mode::CellId,
    text: &str,
) -> WorkflowProgressNotificationResult {
    let Some(notification) = parse_workflow_progress_notification(text) else {
        return WorkflowProgressNotificationResult::NotWorkflowNotification;
    };
    let snapshot_dir = turn.config.codex_home.join(WORKFLOW_RUNS_DIR).to_path_buf();
    match append_workflow_progress_in_dir(snapshot_dir.as_path(), cell_id.as_str(), notification)
        .await
    {
        Ok(update) => WorkflowProgressNotificationResult::Consumed { update },
        Err(err) => {
            tracing::warn!(
                error = %err,
                cell_id = %cell_id,
                "failed to update workflow run snapshot after code-mode notification"
            );
            WorkflowProgressNotificationResult::Consumed { update: None }
        }
    }
}

async fn update_workflow_snapshot_in_dir(
    snapshot_dir: &Path,
    wait_response: &codex_code_mode::WaitOutcome,
) -> Result<Option<WorkflowWaitUpdate>, String> {
    let response = wait_outcome_response(wait_response);
    let cell_id = runtime_response_cell_id(response).as_str();
    let snapshot = match newest_running_workflow_snapshot_for_cell(snapshot_dir, cell_id).await? {
        Some(snapshot) => Some(snapshot),
        None => newest_completed_progress_workflow_snapshot_for_cell(snapshot_dir, cell_id).await?,
    };
    let Some((path, mut snapshot)) = snapshot else {
        return Ok(None);
    };
    let matched_run_id = workflow_snapshot_run_id(&snapshot);
    let workflow = workflow_snapshot_workflow_name(&snapshot);
    let previous_status = workflow_snapshot_status(&snapshot);
    let max_output_tokens = workflow_snapshot_max_output_tokens(&snapshot);

    update_workflow_snapshot_value(&mut snapshot, wait_response);
    let payload = serde_json::to_string_pretty(&snapshot)
        .map_err(|err| format!("failed to serialize workflow snapshot: {err}"))?;
    tokio::fs::write(&path, format!("{payload}\n"))
        .await
        .map_err(|err| {
            format!(
                "failed to write workflow snapshot {}: {err}",
                path.display()
            )
        })?;
    sync_workflow_active_marker_from_snapshot(snapshot_dir, &snapshot).await?;
    write_workflow_transcript_from_snapshot(&snapshot).await?;
    Ok(matched_run_id.map(|run_id| WorkflowWaitUpdate {
        run_id,
        cell_id: cell_id.to_string(),
        workflow,
        previous_status,
        status: workflow_status_for_wait_outcome(wait_response).to_string(),
        max_output_tokens,
    }))
}

async fn update_workflow_snapshot_for_pending_in_dir(
    snapshot_dir: &Path,
    pending_response: &codex_code_mode::WaitToPendingOutcome,
) -> Result<Option<WorkflowWaitUpdate>, String> {
    let cell_id = wait_to_pending_outcome_cell_id(pending_response).as_str();
    let Some((path, mut snapshot)) =
        newest_running_workflow_snapshot_for_cell(snapshot_dir, cell_id).await?
    else {
        return Ok(None);
    };
    let matched_run_id = workflow_snapshot_run_id(&snapshot);
    let workflow = workflow_snapshot_workflow_name(&snapshot);
    let previous_status = workflow_snapshot_status(&snapshot);
    let max_output_tokens = workflow_snapshot_max_output_tokens(&snapshot);
    let status = workflow_status_for_wait_to_pending_outcome(pending_response);

    update_workflow_snapshot_for_pending_value(&mut snapshot, pending_response);
    let payload = serde_json::to_string_pretty(&snapshot)
        .map_err(|err| format!("failed to serialize workflow snapshot: {err}"))?;
    tokio::fs::write(&path, format!("{payload}\n"))
        .await
        .map_err(|err| {
            format!(
                "failed to write workflow snapshot {}: {err}",
                path.display()
            )
        })?;
    sync_workflow_active_marker_from_snapshot(snapshot_dir, &snapshot).await?;
    write_workflow_transcript_from_snapshot(&snapshot).await?;
    Ok(matched_run_id.map(|run_id| WorkflowWaitUpdate {
        run_id,
        cell_id: cell_id.to_string(),
        workflow,
        previous_status,
        status: status.to_string(),
        max_output_tokens,
    }))
}

async fn append_workflow_progress_in_dir(
    snapshot_dir: &Path,
    cell_id: &str,
    notification: WorkflowProgressNotification,
) -> Result<Option<WorkflowProgressUpdate>, String> {
    let Some((path, mut snapshot)) =
        newest_running_workflow_snapshot_for_cell(snapshot_dir, cell_id).await?
    else {
        return Ok(None);
    };

    if matches!(
        notification.event.as_str(),
        "agent_journal_entry" | "agent_journal_started" | "child_journal_entry"
    ) {
        if let Some((journal_paths, line)) =
            workflow_journal_lines_from_notification(snapshot_dir, &snapshot, &notification)
        {
            for journal_path in journal_paths {
                if let Err(err) =
                    append_workflow_agent_journal_entry(journal_path.as_path(), line.as_str()).await
                {
                    tracing::warn!(
                        path = %journal_path.display(),
                        error = %err,
                        "failed to append workflow agent journal entry"
                    );
                }
            }
        }
        return Ok(None);
    }
    if notification.event == "agent_transcript_entry" {
        if let Some((target, lines)) = workflow_agent_transcript_lines_from_notification(
            snapshot_dir,
            &snapshot,
            &notification,
        ) && let Err(err) =
            append_workflow_agent_transcript_lines_to_target(&target, &lines).await
        {
            tracing::warn!(
                path = %target.transcript_path.display(),
                error = %err,
                "failed to append workflow agent transcript"
            );
        }
        if let Some((metadata_paths, metadata)) =
            workflow_agent_transcript_metadata_from_notification(
                snapshot_dir,
                &snapshot,
                &notification,
            )
        {
            for metadata_path in metadata_paths {
                if let Err(err) =
                    write_workflow_agent_transcript_metadata(metadata_path.as_path(), &metadata)
                        .await
                {
                    tracing::warn!(
                        path = %metadata_path.display(),
                        error = %err,
                        "failed to write workflow agent transcript metadata"
                    );
                }
            }
        }
        return Ok(None);
    }

    apply_workflow_agent_pending_control(&mut snapshot, &notification);
    let update = append_workflow_progress_event(&mut snapshot, cell_id, notification);
    let payload = serde_json::to_string_pretty(&snapshot)
        .map_err(|err| format!("failed to serialize workflow snapshot: {err}"))?;
    tokio::fs::write(&path, format!("{payload}\n"))
        .await
        .map_err(|err| {
            format!(
                "failed to write workflow snapshot {}: {err}",
                path.display()
            )
        })?;
    sync_workflow_active_marker_from_snapshot(snapshot_dir, &snapshot).await?;
    write_workflow_transcript_from_snapshot(&snapshot).await?;
    Ok(update)
}

fn workflow_journal_lines_from_notification(
    snapshot_dir: &Path,
    snapshot: &JsonValue,
    notification: &WorkflowProgressNotification,
) -> Option<(Vec<PathBuf>, String)> {
    let data = notification.data.as_ref()?;
    let key = data
        .get("key")
        .and_then(JsonValue::as_str)
        .map(str::trim)
        .filter(|key| !key.is_empty())?;
    let run_dir = snapshot
        .get("run_dir")
        .and_then(JsonValue::as_str)
        .map(str::trim)
        .filter(|run_dir| !run_dir.is_empty())?;
    let line = match notification.event.as_str() {
        "agent_journal_entry" => {
            let result = data.get("result")?;
            serde_json::to_string(&serde_json::json!({
                "type": "result",
                "key": key,
                "agentId": notification.agent.as_deref().unwrap_or(""),
                "result": result,
            }))
            .ok()?
        }
        "agent_journal_started" => {
            let agent_id = data
                .get("agentId")
                .and_then(JsonValue::as_str)
                .or(notification.agent.as_deref())
                .unwrap_or("");
            serde_json::to_string(&serde_json::json!({
                "type": "started",
                "key": key,
                "agentId": agent_id,
            }))
            .ok()?
        }
        "child_journal_entry" => {
            let result = data.get("result")?;
            let child_run_id = notification
                .child_run_id
                .as_deref()
                .or_else(|| data.get("childRunId").and_then(JsonValue::as_str))
                .or_else(|| data.get("child_run_id").and_then(JsonValue::as_str))
                .unwrap_or("");
            serde_json::to_string(&serde_json::json!({
                "type": "child_result",
                "key": key,
                "child": notification.child.as_deref().unwrap_or(""),
                "childRunId": child_run_id,
                "result": result,
            }))
            .ok()?
        }
        _ => return None,
    };
    let primary_path = PathBuf::from(run_dir).join(WORKFLOW_AGENT_JOURNAL_FILE);
    let mut paths = vec![primary_path.clone()];
    if let Some(mirror_dir) = workflow_claude_sidechain_dir(snapshot_dir, snapshot) {
        let mirror_path = mirror_dir.join(WORKFLOW_AGENT_JOURNAL_FILE);
        if mirror_path != primary_path {
            paths.push(mirror_path);
        }
    }
    Some((paths, line))
}

async fn append_workflow_agent_journal_entry(path: &Path, line: &str) -> Result<(), String> {
    let parent = path.parent().ok_or_else(|| {
        format!(
            "workflow agent journal path has no parent: {}",
            path.display()
        )
    })?;
    tokio::fs::create_dir_all(parent).await.map_err(|err| {
        format!(
            "failed to create workflow agent journal directory {}: {err}",
            parent.display()
        )
    })?;
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await
        .map_err(|err| {
            format!(
                "failed to open workflow agent journal {}: {err}",
                path.display()
            )
        })?;
    file.write_all(line.as_bytes()).await.map_err(|err| {
        format!(
            "failed to write workflow agent journal {}: {err}",
            path.display()
        )
    })?;
    file.write_all(b"\n").await.map_err(|err| {
        format!(
            "failed to finish workflow agent journal line {}: {err}",
            path.display()
        )
    })?;
    Ok(())
}

fn workflow_agent_transcript_lines_from_notification(
    snapshot_dir: &Path,
    snapshot: &JsonValue,
    notification: &WorkflowProgressNotification,
) -> Option<(WorkflowAgentTranscriptTarget, Vec<String>)> {
    let data = notification.data.as_ref()?;
    let agent_id = data
        .get("agentId")
        .and_then(JsonValue::as_str)
        .or(notification.agent.as_deref())
        .map(str::trim)
        .filter(|agent_id| !agent_id.is_empty())?;
    let safe_agent_id = workflow_agent_transcript_file_agent_id(agent_id)?;
    let transcript_dir = snapshot
        .get("transcript_dir")
        .and_then(JsonValue::as_str)
        .or_else(|| snapshot.get("run_dir").and_then(JsonValue::as_str))
        .map(str::trim)
        .filter(|dir| !dir.is_empty())?;
    let prompt = data
        .get("prompt")
        .and_then(JsonValue::as_str)
        .or(notification.message.as_deref())
        .map(str::trim)
        .filter(|prompt| !prompt.is_empty());
    let prompt_recorded = data
        .get("promptRecorded")
        .or_else(|| data.get("prompt_recorded"))
        .and_then(JsonValue::as_bool)
        .unwrap_or(false);
    let transcript_recorded = data
        .get("transcriptRecorded")
        .or_else(|| data.get("transcript_recorded"))
        .and_then(JsonValue::as_bool)
        .unwrap_or(false);
    let final_text = data
        .get("finalText")
        .and_then(JsonValue::as_str)
        .or_else(|| data.get("final_text").and_then(JsonValue::as_str))
        .or_else(|| data.get("result").and_then(JsonValue::as_str))
        .map(str::trim)
        .filter(|final_text| !final_text.is_empty())
        .map(ToString::to_string)
        .or_else(|| data.get("result").map(std::string::ToString::to_string));

    let mut lines = Vec::new();
    if let Some(prompt) = prompt.filter(|_| !prompt_recorded && !transcript_recorded) {
        lines.push(
            serde_json::to_string(&serde_json::json!({
                "type": "user",
                "message": {
                    "content": [
                        { "type": "text", "text": truncate_chars(prompt, WORKFLOW_OUTPUT_PREVIEW_MAX_CHARS) }
                    ]
                }
            }))
            .ok()?,
        );
    }
    let transcript_lines = if transcript_recorded {
        Vec::new()
    } else {
        workflow_agent_transcript_lines_from_data(data)
    };
    let has_raw_transcript = !transcript_lines.is_empty();
    let raw_transcript_has_assistant_text = workflow_agent_transcript_data_has_assistant_text(data);
    lines.extend(transcript_lines);
    if let Some(final_text) = final_text
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .filter(|_| !raw_transcript_has_assistant_text)
        .filter(|_| !transcript_recorded)
    {
        let content = if has_raw_transcript {
            workflow_agent_transcript_final_text_content(final_text)
        } else {
            workflow_agent_transcript_assistant_content(data, final_text)
        };
        lines.push(
            serde_json::to_string(&serde_json::json!({
                "type": "assistant",
                "message": { "content": content }
            }))
            .ok()?,
        );
    }
    if lines.is_empty() {
        return None;
    }
    let envelope = workflow_agent_transcript_envelope_from_snapshot(
        snapshot,
        Some(data),
        Some(agent_id),
        notification.agent.as_deref(),
    );
    Some((
        WorkflowAgentTranscriptTarget {
            transcript_path: PathBuf::from(transcript_dir)
                .join(format!("agent-{safe_agent_id}.jsonl")),
            mirror_transcript_path: workflow_claude_sidechain_dir(snapshot_dir, snapshot)
                .map(|dir| dir.join(format!("agent-{safe_agent_id}.jsonl"))),
            envelope,
        },
        lines,
    ))
}

fn workflow_agent_transcript_data_has_assistant_text(data: &JsonValue) -> bool {
    let Some(entries) = data.get("transcript").and_then(JsonValue::as_array) else {
        return false;
    };
    entries.iter().any(|entry| {
        let role = entry
            .get("role")
            .or_else(|| entry.get("type"))
            .and_then(JsonValue::as_str)
            .map(str::trim);
        if role != Some("assistant") {
            return false;
        }
        let content = entry
            .get("message")
            .and_then(|message| message.get("content"))
            .or_else(|| entry.get("content"))
            .unwrap_or(entry);
        workflow_agent_transcript_content_has_visible_text(content)
    })
}

fn workflow_agent_transcript_content_has_visible_text(content: &JsonValue) -> bool {
    if content.as_str().is_some_and(|text| !text.trim().is_empty()) {
        return true;
    }
    let Some(items) = content.as_array() else {
        return false;
    };
    items.iter().any(|item| {
        if matches!(
            item.get("type").and_then(JsonValue::as_str),
            Some("reasoning" | "thinking" | "tool_use" | "tool_result")
        ) {
            return false;
        }
        item.as_str().is_some_and(|text| !text.trim().is_empty())
            || item
                .get("text")
                .and_then(JsonValue::as_str)
                .is_some_and(|text| !text.trim().is_empty())
            || item
                .get("content")
                .and_then(JsonValue::as_str)
                .is_some_and(|text| !text.trim().is_empty())
    })
}

fn workflow_agent_transcript_lines_from_data(data: &JsonValue) -> Vec<String> {
    let Some(entries) = data.get("transcript").and_then(JsonValue::as_array) else {
        return Vec::new();
    };
    entries
        .iter()
        .filter(|entry| entry.is_object())
        .take(WORKFLOW_AGENT_TRANSCRIPT_NOTIFICATION_MAX_LINES)
        .filter_map(|entry| {
            let line = serde_json::to_string(entry).ok()?;
            (line.len() <= WORKFLOW_AGENT_TRANSCRIPT_NOTIFICATION_MAX_LINE_BYTES).then_some(line)
        })
        .collect()
}

fn workflow_agent_transcript_metadata_from_notification(
    snapshot_dir: &Path,
    snapshot: &JsonValue,
    notification: &WorkflowProgressNotification,
) -> Option<(Vec<PathBuf>, JsonValue)> {
    let data = notification.data.as_ref()?;
    let agent_id = data
        .get("agentId")
        .and_then(JsonValue::as_str)
        .or(notification.agent.as_deref())
        .map(str::trim)
        .filter(|agent_id| !agent_id.is_empty())?;
    let safe_agent_id = workflow_agent_transcript_file_agent_id(agent_id)?;
    let transcript_dir = snapshot
        .get("transcript_dir")
        .and_then(JsonValue::as_str)
        .or_else(|| snapshot.get("run_dir").and_then(JsonValue::as_str))
        .map(str::trim)
        .filter(|dir| !dir.is_empty())?;

    let mut metadata = serde_json::Map::new();
    metadata.insert(
        "version".to_string(),
        JsonValue::String("codex-workflow-agent-meta-v1".to_string()),
    );
    metadata.insert(
        "agentId".to_string(),
        JsonValue::String(agent_id.to_string()),
    );
    metadata.insert(
        "name".to_string(),
        JsonValue::String(
            notification
                .agent
                .as_deref()
                .and_then(non_empty_str)
                .unwrap_or(agent_id)
                .to_string(),
        ),
    );
    metadata.insert(
        "agentName".to_string(),
        JsonValue::String(
            notification
                .agent
                .as_deref()
                .and_then(non_empty_str)
                .unwrap_or(agent_id)
                .to_string(),
        ),
    );
    metadata.insert(
        "sessionKind".to_string(),
        JsonValue::String("workflow_agent".to_string()),
    );
    if let Some(parent_thread_id) = workflow_string_field(snapshot, &["thread_id", "threadId"]) {
        metadata.insert(
            "parentThreadId".to_string(),
            JsonValue::String(parent_thread_id),
        );
    }
    if let Some(workflow) = notification.workflow.as_deref().and_then(non_empty_str) {
        metadata.insert(
            "workflow".to_string(),
            JsonValue::String(workflow.to_string()),
        );
    }
    for (key, value) in [
        (
            "runId",
            workflow_string_field(snapshot, &["run_id", "runId"]),
        ),
        (
            "cellId",
            workflow_string_field(snapshot, &["cell_id", "cellId"]),
        ),
        (
            "runDir",
            workflow_string_field(snapshot, &["run_dir", "runDir"]),
        ),
        (
            "transcriptDir",
            workflow_string_field(snapshot, &["transcript_dir", "transcriptDir"]),
        ),
        (
            "scriptPath",
            workflow_string_field(snapshot, &["script_path", "scriptPath"]),
        ),
    ] {
        if let Some(value) = value.as_deref().and_then(non_empty_str) {
            metadata.insert(
                key.to_string(),
                JsonValue::String(truncate_chars(value, WORKFLOW_OUTPUT_PREVIEW_MAX_CHARS)),
            );
        }
    }
    if let Some(prompt) = data
        .get("prompt")
        .and_then(JsonValue::as_str)
        .and_then(non_empty_str)
    {
        let prompt = truncate_chars(prompt, WORKFLOW_OUTPUT_PREVIEW_MAX_CHARS);
        metadata.insert("description".to_string(), JsonValue::String(prompt.clone()));
        metadata.insert("prompt".to_string(), JsonValue::String(prompt));
    }
    if let Some(final_text) = data
        .get("finalText")
        .and_then(JsonValue::as_str)
        .or_else(|| data.get("final_text").and_then(JsonValue::as_str))
        .and_then(non_empty_str)
    {
        metadata.insert(
            "finalTextPreview".to_string(),
            JsonValue::String(truncate_chars(
                final_text,
                WORKFLOW_OUTPUT_PREVIEW_MAX_CHARS,
            )),
        );
    }
    if let Some(input) = data.get("metadata").and_then(JsonValue::as_object) {
        for key in [
            "taskName",
            "agentName",
            "sessionKind",
            "parentThreadId",
            "agentType",
            "model",
            "reasoningEffort",
            "serviceTier",
            "isolation",
            "nickname",
            "author",
            "recipient",
            "worktreePath",
            "toolUseId",
            "cwd",
            "gitBranch",
            "runId",
            "cellId",
            "runDir",
            "transcriptDir",
            "scriptPath",
        ] {
            if let Some(value) = input.get(key).and_then(JsonValue::as_str) {
                let value = value.trim();
                if !value.is_empty() {
                    metadata.insert(
                        key.to_string(),
                        JsonValue::String(truncate_chars(value, WORKFLOW_OUTPUT_PREVIEW_MAX_CHARS)),
                    );
                }
            }
        }
    }

    let primary_path =
        PathBuf::from(transcript_dir).join(format!("agent-{safe_agent_id}.meta.json"));
    let mut paths = vec![primary_path.clone()];
    if let Some(mirror_dir) = workflow_claude_sidechain_dir(snapshot_dir, snapshot) {
        let mirror_path = mirror_dir.join(format!("agent-{safe_agent_id}.meta.json"));
        if mirror_path != primary_path {
            paths.push(mirror_path);
        }
    }
    Some((paths, JsonValue::Object(metadata)))
}

fn non_empty_str(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()).then_some(value)
}

fn workflow_agent_transcript_file_agent_id(agent_id: &str) -> Option<String> {
    let safe = agent_id
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.') {
                ch
            } else {
                '_'
            }
        })
        .take(WORKFLOW_AGENT_TRANSCRIPT_FILE_MAX_AGENT_ID_CHARS)
        .collect::<String>()
        .trim_matches('.')
        .trim_matches('_')
        .to_string();
    if safe.is_empty() || safe == "." || safe == ".." {
        None
    } else {
        Some(safe)
    }
}

fn workflow_claude_sidechain_dir(snapshot_dir: &Path, snapshot: &JsonValue) -> Option<PathBuf> {
    let session_id = workflow_string_field(snapshot, &["session_id", "sessionId"])
        .and_then(|value| workflow_safe_path_segment(value.as_str()))?;
    let run_id = workflow_string_field(snapshot, &["run_id", "runId"])
        .and_then(|value| workflow_safe_path_segment(value.as_str()))?;
    Some(
        snapshot_dir
            .join(session_id)
            .join("subagents")
            .join("workflows")
            .join(run_id),
    )
}

fn workflow_safe_path_segment(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty()
        || value == "."
        || value == ".."
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        None
    } else {
        Some(value.to_string())
    }
}

fn workflow_agent_transcript_assistant_content(
    data: &JsonValue,
    final_text: &str,
) -> Vec<JsonValue> {
    let mut content = Vec::new();
    if let Some(reasoning) = data.get("reasoning").and_then(JsonValue::as_array) {
        for item in reasoning {
            let text = item
                .as_str()
                .or_else(|| item.get("summary").and_then(JsonValue::as_str))
                .or_else(|| item.get("text").and_then(JsonValue::as_str));
            let Some(text) = text.map(str::trim).filter(|text| !text.is_empty()) else {
                continue;
            };
            content.push(serde_json::json!({
                "type": "reasoning",
                "text": truncate_chars(text, WORKFLOW_OUTPUT_PREVIEW_MAX_CHARS),
            }));
        }
    }
    if let Some(tool_calls) = data.get("toolCalls").and_then(JsonValue::as_array) {
        for call in tool_calls {
            let Some(name) = call.get("name").and_then(JsonValue::as_str).map(str::trim) else {
                continue;
            };
            if name.is_empty() {
                continue;
            }
            let mut item = serde_json::json!({
                "type": "tool_use",
                "name": truncate_chars(name, WORKFLOW_PROGRESS_FIELD_MAX_CHARS),
                "input": call.get("input").cloned().unwrap_or(JsonValue::Null),
            });
            if let Some(output) = call.get("output").and_then(JsonValue::as_str)
                && !output.trim().is_empty()
            {
                item.as_object_mut().unwrap().insert(
                    "output".to_string(),
                    JsonValue::String(truncate_chars(output, WORKFLOW_OUTPUT_PREVIEW_MAX_CHARS)),
                );
            }
            content.push(item);
        }
    }
    content.push(serde_json::json!({
        "type": "text",
        "text": truncate_chars(final_text, WORKFLOW_OUTPUT_PREVIEW_MAX_CHARS),
    }));
    content
}

fn workflow_agent_transcript_final_text_content(final_text: &str) -> Vec<JsonValue> {
    vec![serde_json::json!({
        "type": "text",
        "text": truncate_chars(final_text, WORKFLOW_OUTPUT_PREVIEW_MAX_CHARS),
    })]
}

async fn workflow_agent_transcript_envelope_lines(
    path: &Path,
    lines: &[String],
    target: &WorkflowAgentTranscriptTarget,
) -> Vec<String> {
    let (mut parent_uuid, mut tool_use_assistant_uuids) =
        workflow_agent_transcript_existing_state(path).await;
    let mut enriched = Vec::with_capacity(lines.len());
    for line in lines {
        let Ok(mut value) = serde_json::from_str::<JsonValue>(line) else {
            enriched.push(line.clone());
            continue;
        };
        let source_tool_assistant_uuid = workflow_agent_transcript_tool_result_ids(&value)
            .into_iter()
            .find_map(|tool_use_id| tool_use_assistant_uuids.get(&tool_use_id).cloned());
        let uuid = insert_workflow_agent_transcript_envelope(
            &mut value,
            &target.envelope,
            parent_uuid.as_deref(),
            source_tool_assistant_uuid.as_deref(),
        );
        match serde_json::to_string(&value) {
            Ok(serialized) => {
                if let Some(uuid) = uuid.as_deref() {
                    for tool_use_id in workflow_agent_transcript_tool_use_ids(&value) {
                        tool_use_assistant_uuids.insert(tool_use_id, uuid.to_string());
                    }
                }
                parent_uuid = uuid.or(parent_uuid);
                enriched.push(serialized);
            }
            Err(_) => enriched.push(line.clone()),
        }
    }
    enriched
}

fn insert_workflow_agent_transcript_envelope(
    value: &mut JsonValue,
    envelope: &WorkflowAgentTranscriptEnvelope,
    parent_uuid: Option<&str>,
    source_tool_assistant_uuid: Option<&str>,
) -> Option<String> {
    let JsonValue::Object(object) = value else {
        return None;
    };
    let uuid = object
        .get("uuid")
        .and_then(JsonValue::as_str)
        .and_then(non_empty_str)
        .map(ToString::to_string)
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    object
        .entry("uuid".to_string())
        .or_insert_with(|| JsonValue::String(uuid.clone()));
    object
        .entry("timestamp".to_string())
        .or_insert_with(|| JsonValue::String(Utc::now().to_rfc3339()));
    object
        .entry("isSidechain".to_string())
        .or_insert(JsonValue::Bool(true));
    object
        .entry("entrypoint".to_string())
        .or_insert_with(|| JsonValue::String("workflow".to_string()));
    if let Some(parent_uuid) = parent_uuid.and_then(non_empty_str) {
        object
            .entry("parentUuid".to_string())
            .or_insert_with(|| JsonValue::String(parent_uuid.to_string()));
        object
            .entry("logicalParentUuid".to_string())
            .or_insert_with(|| JsonValue::String(parent_uuid.to_string()));
    }
    if let Some(source_tool_assistant_uuid) = source_tool_assistant_uuid.and_then(non_empty_str) {
        object
            .entry("sourceToolAssistantUUID".to_string())
            .or_insert_with(|| JsonValue::String(source_tool_assistant_uuid.to_string()));
    }
    for (key, value) in [
        ("agentId", envelope.agent_id.as_deref()),
        ("agentName", envelope.agent_name.as_deref()),
        ("sessionKind", envelope.session_kind.as_deref()),
        ("sessionId", envelope.session_id.as_deref()),
        ("threadId", envelope.thread_id.as_deref()),
        ("parentThreadId", envelope.parent_thread_id.as_deref()),
        ("runId", envelope.run_id.as_deref()),
        ("cellId", envelope.cell_id.as_deref()),
        (
            "workflowToolCallId",
            envelope.workflow_tool_call_id.as_deref(),
        ),
        ("cwd", envelope.cwd.as_deref()),
        ("version", envelope.version.as_deref()),
        ("gitBranch", envelope.git_branch.as_deref()),
    ] {
        if let Some(value) = value.and_then(non_empty_str) {
            object
                .entry(key.to_string())
                .or_insert_with(|| JsonValue::String(value.to_string()));
        }
    }
    Some(uuid)
}

async fn workflow_agent_transcript_existing_state(
    path: &Path,
) -> (Option<String>, HashMap<String, String>) {
    let Some(contents) = tokio::fs::read_to_string(path).await.ok() else {
        return (None, HashMap::new());
    };
    let mut last_uuid = None;
    let mut tool_use_assistant_uuids = HashMap::new();
    for line in contents.lines() {
        let Ok(value) = serde_json::from_str::<JsonValue>(line) else {
            continue;
        };
        let Some(uuid) = value
            .get("uuid")
            .and_then(JsonValue::as_str)
            .and_then(non_empty_str)
            .map(ToString::to_string)
        else {
            continue;
        };
        for tool_use_id in workflow_agent_transcript_tool_use_ids(&value) {
            tool_use_assistant_uuids.insert(tool_use_id, uuid.clone());
        }
        last_uuid = Some(uuid);
    }
    (last_uuid, tool_use_assistant_uuids)
}

fn workflow_agent_transcript_tool_use_ids(value: &JsonValue) -> Vec<String> {
    workflow_agent_transcript_content_values(value)
        .into_iter()
        .flat_map(|content| {
            let mut ids = Vec::new();
            collect_workflow_agent_transcript_tool_ids(content, "tool_use", &mut ids);
            ids
        })
        .collect()
}

fn workflow_agent_transcript_tool_result_ids(value: &JsonValue) -> Vec<String> {
    let mut ids = workflow_agent_transcript_content_values(value)
        .into_iter()
        .flat_map(|content| {
            let mut ids = Vec::new();
            collect_workflow_agent_transcript_tool_ids(content, "tool_result", &mut ids);
            ids
        })
        .collect::<Vec<_>>();
    if let Some(object) = value.as_object()
        && (object.contains_key("toolUseResult") || object.contains_key("tool_use_result"))
    {
        ids.extend(
            ["toolUseId", "tool_use_id", "parent_tool_use_id"]
                .iter()
                .filter_map(|field| {
                    object
                        .get(*field)
                        .and_then(JsonValue::as_str)
                        .and_then(non_empty_str)
                        .map(ToString::to_string)
                }),
        );
    }
    ids
}

fn workflow_agent_transcript_content_values(value: &JsonValue) -> Vec<&JsonValue> {
    let mut values = Vec::new();
    values.push(value);
    if let Some(content) = value.get("content") {
        values.push(content);
    }
    if let Some(content) = value
        .get("message")
        .and_then(|message| message.get("content"))
    {
        values.push(content);
    }
    values
}

fn collect_workflow_agent_transcript_tool_ids(
    value: &JsonValue,
    expected_type: &str,
    ids: &mut Vec<String>,
) {
    match value {
        JsonValue::Array(items) => {
            for item in items {
                collect_workflow_agent_transcript_tool_ids(item, expected_type, ids);
            }
        }
        JsonValue::Object(object) => {
            if object.get("type").and_then(JsonValue::as_str) == Some(expected_type)
                && let Some(id) =
                    workflow_agent_transcript_tool_id_from_object(object, expected_type)
            {
                ids.push(id);
            }
        }
        _ => {}
    }
}

fn workflow_agent_transcript_tool_id_from_object(
    object: &serde_json::Map<String, JsonValue>,
    expected_type: &str,
) -> Option<String> {
    let fields: &[&str] = if expected_type == "tool_use" {
        &["id", "tool_use_id", "toolUseId"]
    } else {
        &["tool_use_id", "toolUseId", "parent_tool_use_id"]
    };
    fields.iter().find_map(|field| {
        object
            .get(*field)
            .and_then(JsonValue::as_str)
            .and_then(non_empty_str)
            .map(ToString::to_string)
    })
}

async fn append_workflow_agent_transcript(path: &Path, lines: &[String]) -> Result<(), String> {
    let parent = path.parent().ok_or_else(|| {
        format!(
            "workflow agent transcript path has no parent: {}",
            path.display()
        )
    })?;
    tokio::fs::create_dir_all(parent).await.map_err(|err| {
        format!(
            "failed to create workflow agent transcript directory {}: {err}",
            parent.display()
        )
    })?;
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await
        .map_err(|err| {
            format!(
                "failed to open workflow agent transcript {}: {err}",
                path.display()
            )
        })?;
    for line in lines {
        file.write_all(line.as_bytes()).await.map_err(|err| {
            format!(
                "failed to write workflow agent transcript {}: {err}",
                path.display()
            )
        })?;
        file.write_all(b"\n").await.map_err(|err| {
            format!(
                "failed to finish workflow agent transcript line {}: {err}",
                path.display()
            )
        })?;
    }
    Ok(())
}

async fn write_workflow_agent_transcript_metadata(
    path: &Path,
    metadata: &JsonValue,
) -> Result<(), String> {
    let parent = path.parent().ok_or_else(|| {
        format!(
            "workflow agent transcript metadata path has no parent: {}",
            path.display()
        )
    })?;
    tokio::fs::create_dir_all(parent).await.map_err(|err| {
        format!(
            "failed to create workflow agent transcript metadata directory {}: {err}",
            parent.display()
        )
    })?;
    let mut metadata = match (
        tokio::fs::read_to_string(path)
            .await
            .ok()
            .and_then(|contents| serde_json::from_str::<JsonValue>(&contents).ok()),
        metadata,
    ) {
        (Some(JsonValue::Object(mut existing)), JsonValue::Object(next)) => {
            for (key, value) in next {
                existing.insert(key.clone(), value.clone());
            }
            JsonValue::Object(existing)
        }
        _ => metadata.clone(),
    };
    insert_workflow_agent_metadata_compat_record(&mut metadata);
    let payload = serde_json::to_string_pretty(&metadata)
        .map_err(|err| format!("failed to serialize workflow agent metadata: {err}"))?;
    tokio::fs::write(path, format!("{payload}\n"))
        .await
        .map_err(|err| {
            format!(
                "failed to write workflow agent transcript metadata {}: {err}",
                path.display()
            )
        })
}

fn insert_workflow_agent_metadata_compat_record(metadata: &mut JsonValue) {
    let JsonValue::Object(object) = metadata else {
        return;
    };
    let mut record = serde_json::Map::new();
    record.insert(
        "type".to_string(),
        JsonValue::String("agent_metadata".to_string()),
    );
    for key in [
        "agentType",
        "agentName",
        "sessionKind",
        "parentThreadId",
        "worktreePath",
        "cwd",
        "description",
        "name",
        "toolUseId",
    ] {
        if let Some(value) = object
            .get(key)
            .and_then(JsonValue::as_str)
            .and_then(non_empty_str)
        {
            record.insert(
                key.to_string(),
                JsonValue::String(truncate_chars(value, WORKFLOW_OUTPUT_PREVIEW_MAX_CHARS)),
            );
        }
    }
    object.insert("agent_metadata".to_string(), JsonValue::Object(record));
}

async fn workflow_agent_transcript_target_for_cell_in_dir(
    snapshot_dir: &Path,
    cell_id: &str,
    agent_id: &str,
) -> Result<Option<WorkflowAgentTranscriptTarget>, String> {
    let Some((_path, snapshot)) =
        newest_running_workflow_snapshot_for_cell(snapshot_dir, cell_id).await?
    else {
        return Ok(None);
    };
    Ok(workflow_agent_transcript_target_from_snapshot(
        snapshot_dir,
        &snapshot,
        agent_id,
    ))
}

fn workflow_agent_transcript_target_from_snapshot(
    snapshot_dir: &Path,
    snapshot: &JsonValue,
    agent_id: &str,
) -> Option<WorkflowAgentTranscriptTarget> {
    let safe_agent_id = workflow_agent_transcript_file_agent_id(agent_id)?;
    let transcript_dir = snapshot
        .get("transcript_dir")
        .and_then(JsonValue::as_str)
        .or_else(|| snapshot.get("run_dir").and_then(JsonValue::as_str))
        .map(str::trim)
        .filter(|dir| !dir.is_empty())?;
    Some(WorkflowAgentTranscriptTarget {
        transcript_path: PathBuf::from(transcript_dir).join(format!("agent-{safe_agent_id}.jsonl")),
        mirror_transcript_path: workflow_claude_sidechain_dir(snapshot_dir, snapshot)
            .map(|dir| dir.join(format!("agent-{safe_agent_id}.jsonl"))),
        envelope: workflow_agent_transcript_envelope_from_snapshot(
            snapshot,
            None,
            Some(agent_id),
            None,
        ),
    })
}

fn workflow_agent_transcript_envelope_from_snapshot(
    snapshot: &JsonValue,
    data: Option<&JsonValue>,
    agent_id: Option<&str>,
    agent_name_hint: Option<&str>,
) -> WorkflowAgentTranscriptEnvelope {
    let agent_id = agent_id
        .and_then(non_empty_str)
        .map(ToString::to_string)
        .or_else(|| workflow_data_metadata_string(data, &["agentId", "agent_id"]));
    let agent_name = workflow_data_metadata_string(data, &["agentName", "agent_name", "name"])
        .or_else(|| {
            agent_name_hint
                .and_then(non_empty_str)
                .map(ToString::to_string)
        })
        .or_else(|| agent_id.clone());
    let session_kind = workflow_data_metadata_string(data, &["sessionKind", "session_kind"])
        .or_else(|| Some("workflow_agent".to_string()));
    let session_id = workflow_string_field(snapshot, &["session_id", "sessionId"])
        .or_else(|| workflow_data_metadata_string(data, &["sessionId", "session_id"]));
    let thread_id = workflow_string_field(snapshot, &["thread_id", "threadId"])
        .or_else(|| workflow_data_metadata_string(data, &["threadId", "thread_id"]));
    let parent_thread_id =
        workflow_data_metadata_string(data, &["parentThreadId", "parent_thread_id"])
            .or_else(|| thread_id.clone());
    let run_id = workflow_string_field(snapshot, &["run_id", "runId"])
        .or_else(|| workflow_data_metadata_string(data, &["runId", "run_id"]));
    let cell_id = workflow_string_field(snapshot, &["cell_id", "cellId"])
        .or_else(|| workflow_data_metadata_string(data, &["cellId", "cell_id"]));
    let workflow_tool_call_id = workflow_string_field(
        snapshot,
        &["workflow_tool_call_id", "workflowToolCallId"],
    )
    .or_else(|| {
        workflow_data_metadata_string(data, &["workflowToolCallId", "workflow_tool_call_id"])
    });
    let cwd = workflow_string_field(snapshot, &["cwd"])
        .or_else(|| workflow_data_metadata_string(data, &["cwd"]));
    let version = workflow_string_field(snapshot, &["version"])
        .or_else(|| workflow_data_metadata_string(data, &["version"]))
        .or_else(|| Some(env!("CARGO_PKG_VERSION").to_string()));
    let git_branch = workflow_string_field(snapshot, &["git_branch", "gitBranch"])
        .or_else(|| workflow_data_metadata_string(data, &["gitBranch", "git_branch"]));
    WorkflowAgentTranscriptEnvelope {
        agent_id,
        agent_name,
        session_kind,
        session_id,
        thread_id,
        parent_thread_id,
        run_id,
        cell_id,
        workflow_tool_call_id,
        cwd,
        version,
        git_branch,
    }
}

fn workflow_data_metadata_string(data: Option<&JsonValue>, fields: &[&str]) -> Option<String> {
    let metadata = data?.get("metadata")?;
    workflow_string_field(metadata, fields)
}

async fn newest_running_workflow_snapshot_for_cell(
    snapshot_dir: &Path,
    cell_id: &str,
) -> Result<Option<(PathBuf, JsonValue)>, String> {
    newest_workflow_snapshot_for_cell_matching(snapshot_dir, cell_id, |snapshot| {
        snapshot
            .get("status")
            .and_then(JsonValue::as_str)
            .is_some_and(workflow_status_is_active)
    })
    .await
}

async fn newest_completed_progress_workflow_snapshot_for_cell(
    snapshot_dir: &Path,
    cell_id: &str,
) -> Result<Option<(PathBuf, JsonValue)>, String> {
    newest_workflow_snapshot_for_cell_matching(snapshot_dir, cell_id, |snapshot| {
        snapshot.get("status").and_then(JsonValue::as_str) == Some("completed")
            && workflow_last_progress_event(snapshot) == Some("workflow_complete")
    })
    .await
}

async fn newest_workflow_snapshot_for_cell_matching(
    snapshot_dir: &Path,
    cell_id: &str,
    include: impl Fn(&JsonValue) -> bool,
) -> Result<Option<(PathBuf, JsonValue)>, String> {
    let entries = match tokio::fs::read_dir(snapshot_dir).await {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(format!(
                "failed to read workflow snapshot directory {}: {err}",
                snapshot_dir.display()
            ));
        }
    };

    let mut entries = entries;
    let mut candidates = Vec::new();
    while let Some(entry) = entries.next_entry().await.map_err(|err| {
        format!(
            "failed to scan workflow snapshot directory {}: {err}",
            snapshot_dir.display()
        )
    })? {
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
            continue;
        }
        let contents = match tokio::fs::read_to_string(&path).await {
            Ok(contents) => contents,
            Err(_) => continue,
        };
        let snapshot = match serde_json::from_str::<JsonValue>(&contents) {
            Ok(snapshot) => snapshot,
            Err(_) => continue,
        };
        if snapshot.get("cell_id").and_then(JsonValue::as_str) != Some(cell_id) {
            continue;
        }
        if !include(&snapshot) {
            continue;
        }
        let sort_key = snapshot
            .get("progress")
            .and_then(JsonValue::as_array)
            .and_then(|events| events.last())
            .and_then(|event| event.get("unix_ms"))
            .and_then(JsonValue::as_u64)
            .or_else(|| snapshot.get("ended_unix_ms").and_then(JsonValue::as_u64))
            .map(u128::from)
            .unwrap_or_default();
        candidates.push((sort_key, path, snapshot));
    }

    candidates.sort_by_key(|candidate| std::cmp::Reverse(candidate.0));
    Ok(candidates
        .into_iter()
        .next()
        .map(|(_sort_key, path, snapshot)| (path, snapshot)))
}

fn workflow_last_progress_event(snapshot: &JsonValue) -> Option<&str> {
    snapshot
        .get("progress")
        .and_then(JsonValue::as_array)
        .and_then(|events| events.last())
        .and_then(|event| event.get("event"))
        .and_then(JsonValue::as_str)
}

async fn workflow_snapshot_for_run(
    snapshot_dir: &Path,
    run_id: &str,
) -> Result<Option<(PathBuf, JsonValue)>, String> {
    let entries = match tokio::fs::read_dir(snapshot_dir).await {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(format!(
                "failed to read workflow snapshot directory {}: {err}",
                snapshot_dir.display()
            ));
        }
    };

    let run_id = run_id.trim();
    let mut entries = entries;
    while let Some(entry) = entries.next_entry().await.map_err(|err| {
        format!(
            "failed to scan workflow snapshot directory {}: {err}",
            snapshot_dir.display()
        )
    })? {
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
            continue;
        }
        let contents = match tokio::fs::read_to_string(&path).await {
            Ok(contents) => contents,
            Err(_) => continue,
        };
        let snapshot = match serde_json::from_str::<JsonValue>(&contents) {
            Ok(snapshot) => snapshot,
            Err(_) => continue,
        };
        if workflow_string_field(&snapshot, &["run_id", "runId"]).as_deref() == Some(run_id) {
            return Ok(Some((path, snapshot)));
        }
    }
    Ok(None)
}

async fn running_workflow_agent_for_run_in_dir(
    snapshot_dir: &Path,
    run_id: &str,
    agent_id: &str,
) -> Result<Option<WorkflowAgentTarget>, String> {
    let Some((_path, snapshot)) = workflow_snapshot_for_run(snapshot_dir, run_id).await? else {
        return Ok(None);
    };
    Ok(workflow_running_agent_target_from_snapshot(
        &snapshot, run_id, agent_id,
    ))
}

async fn workflow_agent_control_request_for_run_in_dir(
    snapshot_dir: &Path,
    run_id: &str,
    agent_id: &str,
) -> Result<Option<WorkflowAgentControlRequest>, String> {
    let Some((_path, snapshot)) = workflow_snapshot_for_run(snapshot_dir, run_id).await? else {
        return Ok(None);
    };
    Ok(workflow_agent_control_request_from_snapshot(
        &snapshot, run_id, agent_id,
    ))
}

async fn record_workflow_agent_control_event_in_dir(
    snapshot_dir: &Path,
    target: &WorkflowAgentTarget,
    event: &str,
    message: Option<&str>,
) -> Result<Option<WorkflowProgressUpdate>, String> {
    let Some((path, mut snapshot)) =
        workflow_snapshot_for_run(snapshot_dir, target.run_id.as_str()).await?
    else {
        return Ok(None);
    };
    let Some(current_target) = workflow_running_agent_target_from_snapshot(
        &snapshot,
        target.run_id.as_str(),
        target.agent_id.as_str(),
    ) else {
        return Ok(None);
    };
    let notification = WorkflowProgressNotification {
        event: event.to_string(),
        workflow: workflow_string_field(&snapshot, &["workflow_name", "workflowName"]),
        phase: None,
        agent: current_target.agent.clone(),
        agent_id: Some(current_target.agent_id.clone()),
        child: None,
        child_index: None,
        child_run_id: None,
        item_index: None,
        stage_index: None,
        step_index: None,
        error: None,
        message: message.map(ToString::to_string),
        data: None,
    };
    apply_workflow_agent_pending_control(&mut snapshot, &notification);
    let update = append_workflow_progress_event(
        &mut snapshot,
        current_target.cell_id.as_str(),
        notification,
    );
    let payload = serde_json::to_string_pretty(&snapshot)
        .map_err(|err| format!("failed to serialize workflow snapshot: {err}"))?;
    tokio::fs::write(&path, format!("{payload}\n"))
        .await
        .map_err(|err| {
            format!(
                "failed to write workflow snapshot {}: {err}",
                path.display()
            )
        })?;
    sync_workflow_active_marker_from_snapshot(snapshot_dir, &snapshot).await?;
    write_workflow_transcript_from_snapshot(&snapshot).await?;
    Ok(update)
}

fn workflow_agent_control_request_from_snapshot(
    snapshot: &JsonValue,
    run_id: &str,
    agent_id: &str,
) -> Option<WorkflowAgentControlRequest> {
    if !workflow_string_field(snapshot, &["status"])
        .as_deref()
        .is_some_and(workflow_status_is_active)
    {
        return None;
    }
    if workflow_string_field(snapshot, &["run_id", "runId"]).as_deref() != Some(run_id.trim()) {
        return None;
    }
    let agent_id = agent_id.trim();
    if agent_id.is_empty() {
        return None;
    }

    if let Some(request) = workflow_pending_agent_control_request(snapshot, agent_id) {
        return Some(request);
    }

    let mut latest: Option<&JsonValue> = None;
    for event in snapshot
        .get("progress")
        .or_else(|| snapshot.get("workflowProgress"))
        .and_then(JsonValue::as_array)?
    {
        let Some(event_agent_id) = workflow_progress_event_agent_id(event) else {
            continue;
        };
        if event_agent_id == agent_id {
            latest = Some(event);
        }
    }

    let latest = latest?;
    let event = workflow_string_field(latest, &["event", "type"])?;
    let action = match event.as_str() {
        "agent_skip_requested" => "skip",
        "agent_retry_requested" => "retry",
        "workflow_agent" => match workflow_string_field(latest, &["state"]).as_deref() {
            Some("skip_requested") => "skip",
            Some("retry_requested") => "retry",
            _ => return None,
        },
        _ => return None,
    };
    Some(WorkflowAgentControlRequest {
        action: action.to_string(),
        event,
        message: workflow_string_field(latest, &["message"]),
    })
}

fn workflow_pending_agent_control_request(
    snapshot: &JsonValue,
    agent_id: &str,
) -> Option<WorkflowAgentControlRequest> {
    let pending = snapshot.get("agent_controls")?.get(agent_id)?;
    let action = workflow_string_field(pending, &["action"])?;
    if !matches!(action.as_str(), "skip" | "retry") {
        return None;
    }
    Some(WorkflowAgentControlRequest {
        action,
        event: workflow_string_field(pending, &["event"])
            .unwrap_or_else(|| "agent_control_requested".to_string()),
        message: workflow_string_field(pending, &["message"]),
    })
}

fn apply_workflow_agent_pending_control(
    snapshot: &mut JsonValue,
    notification: &WorkflowProgressNotification,
) {
    let Some(agent_id) = notification.agent_id.as_deref().map(str::trim) else {
        return;
    };
    if agent_id.is_empty() {
        return;
    }

    if let Some(action) = workflow_control_action_for_event(notification.event.as_str()) {
        let Some(object) = snapshot.as_object_mut() else {
            return;
        };
        let controls = object
            .entry("agent_controls".to_string())
            .or_insert_with(|| JsonValue::Object(serde_json::Map::new()));
        let Some(controls) = controls.as_object_mut() else {
            return;
        };
        controls.insert(
            agent_id.to_string(),
            serde_json::json!({
                "action": action,
                "event": notification.event,
                "agent": notification.agent,
                "agent_id": agent_id,
                "message": notification.message,
                "unix_ms": json_millis(unix_time_millis()),
            }),
        );
        return;
    }

    if workflow_control_event_consumes_request(notification.event.as_str()) {
        clear_workflow_agent_pending_control(snapshot, agent_id);
    }
}

fn workflow_control_action_for_event(event: &str) -> Option<&'static str> {
    match event {
        "agent_skip_requested" => Some("skip"),
        "agent_retry_requested" => Some("retry"),
        _ => None,
    }
}

fn workflow_control_event_consumes_request(event: &str) -> bool {
    matches!(
        event,
        "agent_skipped"
            | "agent_retry"
            | "agent_complete"
            | "agent_failed"
            | "agent_interrupted"
            | "agent_stalled"
    )
}

fn clear_workflow_agent_pending_control(snapshot: &mut JsonValue, agent_id: &str) {
    let Some(object) = snapshot.as_object_mut() else {
        return;
    };
    let Some(controls) = object
        .get_mut("agent_controls")
        .and_then(JsonValue::as_object_mut)
    else {
        return;
    };
    controls.remove(agent_id);
    if controls.is_empty() {
        object.remove("agent_controls");
    }
}

fn workflow_running_agent_target_from_snapshot(
    snapshot: &JsonValue,
    run_id: &str,
    agent_id: &str,
) -> Option<WorkflowAgentTarget> {
    if !workflow_string_field(snapshot, &["status"])
        .as_deref()
        .is_some_and(workflow_status_is_active)
    {
        return None;
    }
    if workflow_string_field(snapshot, &["run_id", "runId"]).as_deref() != Some(run_id.trim()) {
        return None;
    }
    let cell_id = workflow_string_field(snapshot, &["cell_id", "cellId"])?;
    let agent_id = agent_id.trim();
    if agent_id.is_empty() {
        return None;
    }

    let mut latest: Option<(String, Option<String>)> = None;
    for event in snapshot
        .get("progress")
        .or_else(|| snapshot.get("workflowProgress"))
        .and_then(JsonValue::as_array)?
    {
        let Some(event_agent_id) = workflow_progress_event_agent_id(event) else {
            continue;
        };
        if event_agent_id != agent_id {
            continue;
        }
        latest = Some((
            workflow_agent_control_status(event),
            workflow_string_field(event, &["agent", "label"]),
        ));
    }

    match latest {
        Some((status, agent)) if status == "running" => Some(WorkflowAgentTarget {
            run_id: run_id.trim().to_string(),
            cell_id,
            status,
            agent,
            agent_id: agent_id.to_string(),
        }),
        Some((status, agent)) if status == "detached" => Some(WorkflowAgentTarget {
            run_id: run_id.trim().to_string(),
            cell_id,
            status,
            agent,
            agent_id: agent_id.to_string(),
        }),
        _ => None,
    }
}

fn workflow_progress_event_agent_id(event: &JsonValue) -> Option<String> {
    workflow_string_field(event, &["agent_id", "agentId"]).or_else(|| {
        event
            .get("data")
            .and_then(|data| workflow_string_field(data, &["agent_id", "agentId"]))
    })
}

fn workflow_agent_control_status(event: &JsonValue) -> String {
    match workflow_string_field(event, &["event", "type"]).as_deref() {
        Some("workflow_agent") => match workflow_string_field(event, &["state"]).as_deref() {
            Some("done" | "completed") => "completed",
            Some("error" | "failed") => "failed",
            Some("skipped") => "skipped",
            Some("interrupted") => "interrupted",
            Some("skip_requested") => "skip requested",
            Some("retry_requested") => "retry requested",
            Some("stalled") => "stalled",
            _ => "running",
        },
        Some("agent_complete") => "completed",
        Some("agent_detached") => "detached",
        Some("agent_failed") => "failed",
        Some("agent_stalled") => "stalled",
        Some("agent_interrupted") => "interrupted",
        Some("agent_skip_requested") => "skip requested",
        Some("agent_retry_requested") => "retry requested",
        Some("agent_skipped") => "skipped",
        Some("agent_start" | "agent_waiting") => "running",
        Some("agent_retry") => "retrying",
        _ => "unknown",
    }
    .to_string()
}

fn workflow_string_field(value: &JsonValue, fields: &[&str]) -> Option<String> {
    fields.iter().find_map(|field| {
        value
            .get(*field)
            .and_then(JsonValue::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
    })
}

async fn sync_workflow_active_marker_from_snapshot(
    snapshot_dir: &Path,
    snapshot: &JsonValue,
) -> Result<(), String> {
    let Some(run_id) = workflow_snapshot_run_id(snapshot) else {
        return Ok(());
    };
    let marker_path = snapshot_dir
        .join(WORKFLOW_ACTIVE_RUNS_DIR)
        .join(format!("{run_id}.json"));
    if snapshot
        .get("status")
        .and_then(JsonValue::as_str)
        .is_some_and(workflow_status_is_active)
    {
        let Some(active_dir) = marker_path.parent() else {
            return Err(format!(
                "failed to resolve workflow active marker directory for {}",
                marker_path.display()
            ));
        };
        tokio::fs::create_dir_all(active_dir).await.map_err(|err| {
            format!(
                "failed to create workflow active marker directory {}: {err}",
                active_dir.display()
            )
        })?;
        let payload = serde_json::to_string_pretty(snapshot)
            .map_err(|err| format!("failed to serialize workflow active marker: {err}"))?;
        tokio::fs::write(&marker_path, format!("{payload}\n"))
            .await
            .map_err(|err| {
                format!(
                    "failed to write workflow active marker {}: {err}",
                    marker_path.display()
                )
            })?;
        return Ok(());
    }

    match tokio::fs::remove_file(&marker_path).await {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(format!(
            "failed to remove workflow active marker {}: {err}",
            marker_path.display()
        )),
    }
}

fn workflow_snapshot_run_id(snapshot: &JsonValue) -> Option<String> {
    snapshot
        .get("run_id")
        .and_then(JsonValue::as_str)
        .map(str::trim)
        .filter(|run_id| !run_id.is_empty())
        .map(ToString::to_string)
}

fn workflow_snapshot_workflow_name(snapshot: &JsonValue) -> Option<String> {
    workflow_string_field(snapshot, &["workflow_name", "workflowName"])
}

fn workflow_snapshot_status(snapshot: &JsonValue) -> Option<String> {
    workflow_string_field(snapshot, &["status"])
}

fn workflow_snapshot_max_output_tokens(snapshot: &JsonValue) -> Option<usize> {
    snapshot
        .get("max_output_tokens")
        .and_then(JsonValue::as_u64)
        .and_then(|value| usize::try_from(value).ok())
}

fn effective_wait_max_output_tokens(
    wait_max_tokens: Option<usize>,
    workflow_max_output_tokens: Option<usize>,
) -> Option<usize> {
    match (wait_max_tokens, workflow_max_output_tokens) {
        (Some(wait), Some(workflow)) => Some(wait.min(workflow)),
        (Some(wait), None) => Some(wait),
        (None, Some(workflow)) => Some(workflow),
        (None, None) => None,
    }
}

fn update_workflow_snapshot_value(
    snapshot: &mut JsonValue,
    wait_response: &codex_code_mode::WaitOutcome,
) {
    let response = wait_outcome_response(wait_response);
    let status = workflow_status_for_wait_outcome(wait_response);
    let preview = runtime_response_preview(response);
    let history_message = (!preview.trim().is_empty()).then_some(preview.as_str());
    let now = unix_time_millis();
    let started = snapshot
        .get("started_unix_ms")
        .and_then(JsonValue::as_u64)
        .map(u128::from)
        .unwrap_or(now);

    let Some(object) = snapshot.as_object_mut() else {
        return;
    };
    object.insert("status".to_string(), JsonValue::String(status.to_string()));
    object.insert("updated_unix_ms".to_string(), json_millis(now));
    object.insert("ended_unix_ms".to_string(), json_millis(now));
    object.insert(
        "duration_ms".to_string(),
        json_millis(now.saturating_sub(started)),
    );
    object.insert(
        "output_preview".to_string(),
        if preview.is_empty() {
            JsonValue::Null
        } else {
            JsonValue::String(preview.clone())
        },
    );
    object.insert(
        "error".to_string(),
        if matches!(status, "failed" | "terminated") && !preview.is_empty() {
            JsonValue::String(preview.clone())
        } else {
            JsonValue::Null
        },
    );
    append_workflow_status_history_event(object, status, now, history_message);
}

fn update_workflow_snapshot_for_pending_value(
    snapshot: &mut JsonValue,
    pending_response: &codex_code_mode::WaitToPendingOutcome,
) {
    let status = workflow_status_for_wait_to_pending_outcome(pending_response);
    let preview = wait_to_pending_outcome_preview(pending_response);
    let history_message = (!preview.trim().is_empty()).then_some(preview.as_str());
    let now = unix_time_millis();
    let started = snapshot
        .get("started_unix_ms")
        .and_then(JsonValue::as_u64)
        .map(u128::from)
        .unwrap_or(now);

    let Some(object) = snapshot.as_object_mut() else {
        return;
    };
    object.insert("status".to_string(), JsonValue::String(status.to_string()));
    object.insert("updated_unix_ms".to_string(), json_millis(now));
    object.insert("ended_unix_ms".to_string(), json_millis(now));
    object.insert(
        "duration_ms".to_string(),
        json_millis(now.saturating_sub(started)),
    );
    object.insert(
        "output_preview".to_string(),
        if preview.is_empty() {
            JsonValue::Null
        } else {
            JsonValue::String(preview.clone())
        },
    );
    object.insert(
        "error".to_string(),
        if matches!(status, "failed" | "terminated") && !preview.is_empty() {
            JsonValue::String(preview.clone())
        } else {
            JsonValue::Null
        },
    );
    append_workflow_status_history_event(object, status, now, history_message);
}

#[derive(Debug, PartialEq)]
struct WorkflowProgressNotification {
    event: String,
    workflow: Option<String>,
    phase: Option<String>,
    agent: Option<String>,
    agent_id: Option<String>,
    child: Option<String>,
    child_index: Option<u64>,
    child_run_id: Option<String>,
    item_index: Option<u64>,
    stage_index: Option<u64>,
    step_index: Option<u64>,
    error: Option<String>,
    message: Option<String>,
    data: Option<JsonValue>,
}

fn parse_workflow_progress_notification(text: &str) -> Option<WorkflowProgressNotification> {
    if text.len() > WORKFLOW_PROGRESS_NOTIFICATION_MAX_BYTES {
        return None;
    }
    let value = serde_json::from_str::<JsonValue>(text).ok()?;
    if value.get("type").and_then(JsonValue::as_str) != Some(WORKFLOW_PROGRESS_NOTIFICATION_TYPE) {
        return None;
    }
    let event = workflow_progress_string_field(&value, "event", 96)?;
    let data = value.get("data").cloned().filter(|data| !data.is_null());
    Some(WorkflowProgressNotification {
        event,
        workflow: workflow_progress_string_field(
            &value,
            "workflow",
            WORKFLOW_PROGRESS_FIELD_MAX_CHARS,
        ),
        phase: workflow_progress_string_field(&value, "phase", WORKFLOW_PROGRESS_FIELD_MAX_CHARS),
        agent: workflow_progress_string_field(&value, "agent", WORKFLOW_PROGRESS_FIELD_MAX_CHARS),
        agent_id: workflow_progress_string_field(
            &value,
            "agent_id",
            WORKFLOW_PROGRESS_FIELD_MAX_CHARS,
        )
        .or_else(|| {
            workflow_progress_string_field(&value, "agentId", WORKFLOW_PROGRESS_FIELD_MAX_CHARS)
        })
        .or_else(|| {
            data.as_ref().and_then(|data| {
                workflow_progress_string_field(data, "agent_id", WORKFLOW_PROGRESS_FIELD_MAX_CHARS)
            })
        })
        .or_else(|| {
            data.as_ref().and_then(|data| {
                workflow_progress_string_field(data, "agentId", WORKFLOW_PROGRESS_FIELD_MAX_CHARS)
            })
        }),
        child: workflow_progress_string_field(&value, "child", WORKFLOW_PROGRESS_FIELD_MAX_CHARS),
        child_index: workflow_progress_u64_field(&value, "child_index")
            .or_else(|| workflow_progress_u64_field(&value, "childIndex"))
            .or_else(|| {
                data.as_ref()
                    .and_then(|data| workflow_progress_u64_field(data, "child_index"))
            })
            .or_else(|| {
                data.as_ref()
                    .and_then(|data| workflow_progress_u64_field(data, "childIndex"))
            }),
        child_run_id: workflow_progress_string_field(
            &value,
            "child_run_id",
            WORKFLOW_PROGRESS_FIELD_MAX_CHARS,
        )
        .or_else(|| {
            workflow_progress_string_field(&value, "childRunId", WORKFLOW_PROGRESS_FIELD_MAX_CHARS)
        })
        .or_else(|| {
            data.as_ref().and_then(|data| {
                workflow_progress_string_field(
                    data,
                    "child_run_id",
                    WORKFLOW_PROGRESS_FIELD_MAX_CHARS,
                )
            })
        })
        .or_else(|| {
            data.as_ref().and_then(|data| {
                workflow_progress_string_field(
                    data,
                    "childRunId",
                    WORKFLOW_PROGRESS_FIELD_MAX_CHARS,
                )
            })
        }),
        item_index: workflow_progress_u64_field(&value, "item_index")
            .or_else(|| workflow_progress_u64_field(&value, "itemIndex"))
            .or_else(|| {
                data.as_ref()
                    .and_then(|data| workflow_progress_u64_field(data, "item_index"))
            })
            .or_else(|| {
                data.as_ref()
                    .and_then(|data| workflow_progress_u64_field(data, "itemIndex"))
            }),
        stage_index: workflow_progress_u64_field(&value, "stage_index")
            .or_else(|| workflow_progress_u64_field(&value, "stageIndex"))
            .or_else(|| {
                data.as_ref()
                    .and_then(|data| workflow_progress_u64_field(data, "stage_index"))
            })
            .or_else(|| {
                data.as_ref()
                    .and_then(|data| workflow_progress_u64_field(data, "stageIndex"))
            }),
        step_index: workflow_progress_u64_field(&value, "step_index")
            .or_else(|| workflow_progress_u64_field(&value, "stepIndex"))
            .or_else(|| {
                data.as_ref()
                    .and_then(|data| workflow_progress_u64_field(data, "step_index"))
            })
            .or_else(|| {
                data.as_ref()
                    .and_then(|data| workflow_progress_u64_field(data, "stepIndex"))
            }),
        error: workflow_progress_string_field(&value, "error", WORKFLOW_OUTPUT_PREVIEW_MAX_CHARS)
            .or_else(|| {
                data.as_ref().and_then(|data| {
                    workflow_progress_string_field(data, "error", WORKFLOW_OUTPUT_PREVIEW_MAX_CHARS)
                })
            }),
        message: workflow_progress_string_field(
            &value,
            "message",
            WORKFLOW_OUTPUT_PREVIEW_MAX_CHARS,
        ),
        data,
    })
}

fn workflow_progress_string_field(
    value: &JsonValue,
    field: &str,
    max_chars: usize,
) -> Option<String> {
    let raw = value.get(field)?.as_str()?.trim();
    if raw.is_empty() {
        return None;
    }
    Some(truncate_chars(raw, max_chars))
}

fn workflow_progress_u64_field(value: &JsonValue, field: &str) -> Option<u64> {
    value
        .get(field)
        .and_then(JsonValue::as_u64)
        .filter(|value| *value > 0)
}

fn append_workflow_progress_event(
    snapshot: &mut JsonValue,
    cell_id: &str,
    notification: WorkflowProgressNotification,
) -> Option<WorkflowProgressUpdate> {
    let now = unix_time_millis();
    let unix_ms = millis_i64(now);
    let notification_event = notification.event.clone();
    let run_id = snapshot
        .get("run_id")
        .and_then(JsonValue::as_str)
        .map(str::trim)
        .filter(|run_id| !run_id.is_empty())
        .map(ToString::to_string)?;
    let update = WorkflowProgressUpdate {
        run_id,
        cell_id: cell_id.to_string(),
        event: notification.event.clone(),
        unix_ms,
        session_id: workflow_string_field(snapshot, &["session_id", "sessionId"]),
        workflow_tool_call_id: workflow_string_field(
            snapshot,
            &["workflow_tool_call_id", "workflowToolCallId"],
        ),
        cwd: workflow_string_field(snapshot, &["cwd"]),
        git_branch: workflow_string_field(snapshot, &["git_branch", "gitBranch"]),
        workflow: notification.workflow.clone(),
        phase: notification.phase.clone(),
        agent: notification.agent.clone(),
        agent_id: notification.agent_id.clone(),
        child: notification.child.clone(),
        child_index: notification.child_index,
        child_run_id: notification.child_run_id.clone(),
        item_index: notification.item_index,
        stage_index: notification.stage_index,
        step_index: notification.step_index,
        error: notification.error.clone(),
        message: notification.message.clone(),
    };
    let mut event = serde_json::Map::new();
    event.insert(
        "event".to_string(),
        JsonValue::String(notification.event.clone()),
    );
    event.insert("unix_ms".to_string(), json_millis(now));
    insert_optional_string(&mut event, "workflow", notification.workflow);
    insert_optional_string(&mut event, "phase", notification.phase);
    insert_optional_string(&mut event, "agent", notification.agent);
    insert_optional_string(&mut event, "agent_id", notification.agent_id);
    insert_optional_string(&mut event, "child", notification.child);
    insert_optional_u64(&mut event, "child_index", notification.child_index);
    insert_optional_string(&mut event, "child_run_id", notification.child_run_id);
    insert_optional_u64(&mut event, "item_index", notification.item_index);
    insert_optional_u64(&mut event, "stage_index", notification.stage_index);
    insert_optional_u64(&mut event, "step_index", notification.step_index);
    insert_optional_string(&mut event, "error", notification.error);
    insert_optional_string(&mut event, "message", notification.message);
    let completion_output_preview = if notification_event == "workflow_complete" {
        notification
            .data
            .as_ref()
            .and_then(workflow_complete_progress_output_preview)
    } else {
        None
    };
    if let Some(data) = notification.data {
        event.insert("data".to_string(), data);
    }

    let object = snapshot.as_object_mut()?;
    object.insert("updated_unix_ms".to_string(), json_millis(now));
    let progress = object
        .entry("progress".to_string())
        .or_insert_with(|| JsonValue::Array(Vec::new()));
    match progress {
        JsonValue::Array(events) => {
            events.push(JsonValue::Object(event));
            if events.len() > WORKFLOW_PROGRESS_MAX_EVENTS {
                let excess = events.len() - WORKFLOW_PROGRESS_MAX_EVENTS;
                events.drain(0..excess);
            }
        }
        other => {
            *other = JsonValue::Array(vec![JsonValue::Object(event)]);
        }
    }
    if notification_event == "workflow_complete" {
        mark_workflow_snapshot_completed_from_progress(
            object,
            now,
            completion_output_preview.as_deref(),
        );
    }
    Some(update)
}

fn mark_workflow_snapshot_completed_from_progress(
    object: &mut serde_json::Map<String, JsonValue>,
    now: u128,
    output_preview: Option<&str>,
) {
    if object
        .get("status")
        .and_then(JsonValue::as_str)
        .is_some_and(|status| !workflow_status_is_active(status))
    {
        return;
    }
    let started = object
        .get("started_unix_ms")
        .and_then(JsonValue::as_u64)
        .map(u128::from)
        .unwrap_or(now);
    object.insert(
        "status".to_string(),
        JsonValue::String("completed".to_string()),
    );
    object.insert("updated_unix_ms".to_string(), json_millis(now));
    object.insert("ended_unix_ms".to_string(), json_millis(now));
    object.insert(
        "duration_ms".to_string(),
        json_millis(now.saturating_sub(started)),
    );
    object.insert("error".to_string(), JsonValue::Null);
    if let Some(output_preview) = output_preview
        && !output_preview.trim().is_empty()
    {
        object.insert(
            "output_preview".to_string(),
            JsonValue::String(output_preview.to_string()),
        );
    }
    append_workflow_status_history_event(object, "completed", now, output_preview);
}

fn workflow_complete_progress_output_preview(data: &JsonValue) -> Option<String> {
    ["output", "output_preview", "outputPreview", "result"]
        .iter()
        .find_map(|field| {
            data.get(*field)
                .and_then(JsonValue::as_str)
                .map(truncate_workflow_preview)
                .filter(|preview| !preview.trim().is_empty())
        })
}

fn insert_optional_string(
    object: &mut serde_json::Map<String, JsonValue>,
    key: &str,
    value: Option<String>,
) {
    if let Some(value) = value {
        object.insert(key.to_string(), JsonValue::String(value));
    }
}

fn insert_optional_u64(
    object: &mut serde_json::Map<String, JsonValue>,
    key: &str,
    value: Option<u64>,
) {
    if let Some(value) = value {
        object.insert(key.to_string(), JsonValue::from(value));
    }
}

fn append_workflow_status_history_event(
    object: &mut serde_json::Map<String, JsonValue>,
    status: &str,
    unix_ms: u128,
    message: Option<&str>,
) {
    let mut event = serde_json::Map::new();
    event.insert("event".to_string(), JsonValue::String(status.to_string()));
    event.insert("status".to_string(), JsonValue::String(status.to_string()));
    event.insert("unix_ms".to_string(), json_millis(unix_ms));
    event.insert(
        "message".to_string(),
        message
            .map(truncate_workflow_preview)
            .map(JsonValue::String)
            .unwrap_or(JsonValue::Null),
    );

    let history = object
        .entry("status_history".to_string())
        .or_insert_with(|| JsonValue::Array(Vec::new()));
    match history {
        JsonValue::Array(events) => events.push(JsonValue::Object(event)),
        other => *other = JsonValue::Array(vec![JsonValue::Object(event)]),
    }
}

fn wait_outcome_response(
    wait_response: &codex_code_mode::WaitOutcome,
) -> &codex_code_mode::RuntimeResponse {
    match wait_response {
        codex_code_mode::WaitOutcome::LiveCell(response)
        | codex_code_mode::WaitOutcome::MissingCell(response) => response,
    }
}

fn runtime_response_cell_id(
    response: &codex_code_mode::RuntimeResponse,
) -> &codex_code_mode::CellId {
    match response {
        codex_code_mode::RuntimeResponse::Yielded { cell_id, .. }
        | codex_code_mode::RuntimeResponse::Terminated { cell_id, .. }
        | codex_code_mode::RuntimeResponse::Result { cell_id, .. } => cell_id,
    }
}

fn workflow_status_for_wait_outcome(wait_response: &codex_code_mode::WaitOutcome) -> &'static str {
    match wait_outcome_response(wait_response) {
        codex_code_mode::RuntimeResponse::Yielded { .. } => "running",
        codex_code_mode::RuntimeResponse::Terminated { .. } => "terminated",
        codex_code_mode::RuntimeResponse::Result { error_text, .. } => {
            if error_text.is_some() {
                "failed"
            } else {
                "completed"
            }
        }
    }
}

fn workflow_status_for_wait_to_pending_outcome(
    pending_response: &codex_code_mode::WaitToPendingOutcome,
) -> &'static str {
    match pending_response {
        codex_code_mode::WaitToPendingOutcome::LiveCell(
            codex_code_mode::ExecuteToPendingOutcome::Pending { .. },
        ) => "paused",
        codex_code_mode::WaitToPendingOutcome::LiveCell(
            codex_code_mode::ExecuteToPendingOutcome::Completed(response),
        ) => workflow_status_for_runtime_response(response),
        codex_code_mode::WaitToPendingOutcome::MissingCell(_) => "failed",
    }
}

fn workflow_status_for_runtime_response(
    response: &codex_code_mode::RuntimeResponse,
) -> &'static str {
    match response {
        codex_code_mode::RuntimeResponse::Yielded { .. } => "running",
        codex_code_mode::RuntimeResponse::Terminated { .. } => "terminated",
        codex_code_mode::RuntimeResponse::Result { error_text, .. } => {
            if error_text.is_some() {
                "failed"
            } else {
                "completed"
            }
        }
    }
}

fn workflow_status_is_active(status: &str) -> bool {
    matches!(status, "running" | "paused")
}

fn wait_to_pending_outcome_cell_id(
    pending_response: &codex_code_mode::WaitToPendingOutcome,
) -> &codex_code_mode::CellId {
    match pending_response {
        codex_code_mode::WaitToPendingOutcome::LiveCell(
            codex_code_mode::ExecuteToPendingOutcome::Pending { cell_id, .. },
        ) => cell_id,
        codex_code_mode::WaitToPendingOutcome::LiveCell(
            codex_code_mode::ExecuteToPendingOutcome::Completed(response),
        )
        | codex_code_mode::WaitToPendingOutcome::MissingCell(response) => {
            runtime_response_cell_id(response)
        }
    }
}

fn wait_to_pending_outcome_preview(
    pending_response: &codex_code_mode::WaitToPendingOutcome,
) -> String {
    match pending_response {
        codex_code_mode::WaitToPendingOutcome::LiveCell(
            codex_code_mode::ExecuteToPendingOutcome::Pending { content_items, .. },
        ) => runtime_content_preview(content_items, None),
        codex_code_mode::WaitToPendingOutcome::LiveCell(
            codex_code_mode::ExecuteToPendingOutcome::Completed(response),
        )
        | codex_code_mode::WaitToPendingOutcome::MissingCell(response) => {
            runtime_response_preview(response)
        }
    }
}

fn runtime_response_preview(response: &codex_code_mode::RuntimeResponse) -> String {
    let (content_items, error_text) = match response {
        codex_code_mode::RuntimeResponse::Yielded { content_items, .. }
        | codex_code_mode::RuntimeResponse::Terminated { content_items, .. } => {
            (content_items, None)
        }
        codex_code_mode::RuntimeResponse::Result {
            content_items,
            error_text,
            ..
        } => (content_items, error_text.as_deref()),
    };
    runtime_content_preview(content_items, error_text)
}

fn runtime_content_preview(
    content_items: &[codex_code_mode::FunctionCallOutputContentItem],
    error_text: Option<&str>,
) -> String {
    let mut parts = content_items
        .iter()
        .filter_map(|item| match item {
            codex_code_mode::FunctionCallOutputContentItem::InputText { text } => {
                Some(text.as_str())
            }
            codex_code_mode::FunctionCallOutputContentItem::InputImage { .. } => None,
        })
        .filter(|text| !text.trim().is_empty())
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    if let Some(error_text) = error_text
        && !error_text.trim().is_empty()
    {
        parts.push(format!("Script error:\n{error_text}"));
    }
    truncate_workflow_preview(&parts.join("\n"))
}

fn truncate_workflow_preview(text: &str) -> String {
    truncate_chars(text, WORKFLOW_OUTPUT_PREVIEW_MAX_CHARS)
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut truncated = text.chars().take(max_chars).collect::<String>();
    truncated.push_str("\n[truncated]");
    truncated
}

async fn write_workflow_transcript_from_snapshot(snapshot: &JsonValue) -> Result<(), String> {
    let Some(transcript_dir) = snapshot
        .get("transcript_dir")
        .and_then(JsonValue::as_str)
        .map(str::trim)
        .filter(|transcript_dir| !transcript_dir.is_empty())
        .map(Path::new)
    else {
        return Ok(());
    };

    tokio::fs::create_dir_all(transcript_dir)
        .await
        .map_err(|err| {
            format!(
                "failed to create workflow transcript directory {}: {err}",
                transcript_dir.display()
            )
        })?;
    let payload = serde_json::to_string_pretty(snapshot)
        .map_err(|err| format!("failed to serialize workflow transcript: {err}"))?;
    tokio::fs::write(
        transcript_dir.join(WORKFLOW_TRANSCRIPT_RUN_FILE),
        format!("{payload}\n"),
    )
    .await
    .map_err(|err| {
        format!(
            "failed to write workflow transcript metadata {}: {err}",
            transcript_dir.join(WORKFLOW_TRANSCRIPT_RUN_FILE).display()
        )
    })?;
    write_optional_transcript_text(
        transcript_dir
            .join(WORKFLOW_TRANSCRIPT_OUTPUT_FILE)
            .as_path(),
        snapshot.get("output_preview").and_then(JsonValue::as_str),
    )
    .await?;
    write_optional_transcript_text(
        transcript_dir
            .join(WORKFLOW_TRANSCRIPT_ERROR_FILE)
            .as_path(),
        snapshot.get("error").and_then(JsonValue::as_str),
    )
    .await?;
    Ok(())
}

async fn write_optional_transcript_text(path: &Path, content: Option<&str>) -> Result<(), String> {
    if let Some(content) = content.map(str::trim).filter(|content| !content.is_empty()) {
        tokio::fs::write(path, format!("{content}\n"))
            .await
            .map_err(|err| {
                format!(
                    "failed to write workflow transcript {}: {err}",
                    path.display()
                )
            })?;
        return Ok(());
    }

    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(format!(
            "failed to remove stale workflow transcript {}: {err}",
            path.display()
        )),
    }
}

fn unix_time_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn json_millis(value: u128) -> JsonValue {
    JsonValue::Number(JsonNumber::from(u64::try_from(value).unwrap_or(u64::MAX)))
}

fn millis_i64(value: u128) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

impl CoreToolRuntime for CodeModeWaitHandler {
    fn pre_tool_use_payload(&self, _invocation: &ToolInvocation) -> Option<PreToolUsePayload> {
        // Code-mode `wait` is runtime control for an existing code cell, not a
        // standalone user action. Tool calls made from code mode still flow
        // through normal dispatch, but hooks should not block or rewrite the
        // wait loop itself.
        None
    }

    fn post_tool_use_payload(
        &self,
        _invocation: &ToolInvocation,
        _result: &dyn ToolOutput,
    ) -> Option<PostToolUsePayload> {
        // The wait result feeds code-mode control flow, so do not let
        // PostToolUse replace it with model-facing hook feedback.
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workflow_snapshot(
        run_id: &str,
        cell_id: &str,
        status: &str,
        ended_unix_ms: u64,
    ) -> JsonValue {
        serde_json::json!({
            "schema_version": 1,
            "run_id": run_id,
            "workflow_name": "release",
            "metadata_name": "release",
            "description": "Release",
            "status": status,
            "status_history": [
                { "event": "started", "unix_ms": 10_u64 },
                { "event": status, "status": status, "unix_ms": ended_unix_ms }
            ],
            "cell_id": cell_id,
            "source": { "kind": "inline", "name": "inline", "path": null },
            "args": null,
            "started_unix_ms": 10_u64,
            "ended_unix_ms": ended_unix_ms,
            "duration_ms": ended_unix_ms.saturating_sub(10),
            "output_preview": null,
            "error": null,
        })
    }

    async fn write_snapshot(dir: &Path, name: &str, value: JsonValue) {
        tokio::fs::write(dir.join(name), format!("{value}\n"))
            .await
            .expect("write snapshot");
    }

    async fn read_snapshot(dir: &Path, name: &str) -> JsonValue {
        let contents = tokio::fs::read_to_string(dir.join(name))
            .await
            .expect("read snapshot");
        serde_json::from_str(&contents).expect("snapshot json")
    }

    #[test]
    fn workflow_wait_output_cap_uses_most_restrictive_limit() {
        assert_eq!(effective_wait_max_output_tokens(None, None), None);
        assert_eq!(effective_wait_max_output_tokens(Some(128), None), Some(128));
        assert_eq!(effective_wait_max_output_tokens(None, Some(64)), Some(64));
        assert_eq!(
            effective_wait_max_output_tokens(Some(128), Some(64)),
            Some(64)
        );
        assert_eq!(
            effective_wait_max_output_tokens(Some(32), Some(64)),
            Some(32)
        );
    }

    #[tokio::test]
    async fn wait_updates_newest_running_workflow_snapshot_for_cell() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let active_dir = temp_dir.path().join(WORKFLOW_ACTIVE_RUNS_DIR);
        tokio::fs::create_dir_all(&active_dir)
            .await
            .expect("create active dir");
        write_snapshot(
            temp_dir.path(),
            "wf_old.json",
            workflow_snapshot("wf_old", "cell-1", "running", 20),
        )
        .await;
        let mut new_snapshot = workflow_snapshot("wf_new", "cell-1", "running", 30);
        new_snapshot.as_object_mut().unwrap().insert(
            "max_output_tokens".to_string(),
            JsonValue::Number(JsonNumber::from(64)),
        );
        write_snapshot(temp_dir.path(), "wf_new.json", new_snapshot.clone()).await;
        write_snapshot(active_dir.as_path(), "wf_new.json", new_snapshot).await;
        write_snapshot(
            temp_dir.path(),
            "wf_completed.json",
            workflow_snapshot("wf_completed", "cell-1", "completed", 40),
        )
        .await;

        let updated = update_workflow_snapshot_in_dir(
            temp_dir.path(),
            &codex_code_mode::WaitOutcome::LiveCell(codex_code_mode::RuntimeResponse::Result {
                cell_id: codex_code_mode::CellId::new("cell-1".to_string()),
                content_items: vec![codex_code_mode::FunctionCallOutputContentItem::InputText {
                    text: "done".to_string(),
                }],
                error_text: None,
            }),
        )
        .await
        .expect("update snapshot")
        .expect("matching snapshot");

        assert_eq!(updated.run_id, "wf_new");
        assert_eq!(updated.cell_id, "cell-1");
        assert_eq!(updated.workflow.as_deref(), Some("release"));
        assert_eq!(updated.previous_status.as_deref(), Some("running"));
        assert_eq!(updated.status, "completed");
        assert!(updated.completed_from_running());
        assert_eq!(updated.max_output_tokens, Some(64));
        let old = read_snapshot(temp_dir.path(), "wf_old.json").await;
        let new = read_snapshot(temp_dir.path(), "wf_new.json").await;
        let completed = read_snapshot(temp_dir.path(), "wf_completed.json").await;
        assert_eq!(old["status"], "running");
        assert_eq!(new["status"], "completed");
        assert_eq!(new["output_preview"], "done");
        assert!(new["error"].is_null());
        assert_eq!(new["status_history"].as_array().unwrap().len(), 3);
        assert_eq!(new["status_history"][2]["event"], "completed");
        assert_eq!(new["status_history"][2]["status"], "completed");
        assert_eq!(new["status_history"][2]["message"], "done");
        assert_eq!(completed["status"], "completed");
        assert!(!active_dir.join("wf_new.json").exists());
    }

    #[tokio::test]
    async fn wait_update_writes_workflow_transcript_files() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let transcript_dir = temp_dir.path().join("wf_new").join("transcripts");
        let mut snapshot = workflow_snapshot("wf_new", "cell-1", "running", 30);
        snapshot.as_object_mut().unwrap().insert(
            "transcript_dir".to_string(),
            JsonValue::String(transcript_dir.display().to_string()),
        );
        write_snapshot(temp_dir.path(), "wf_new.json", snapshot).await;
        tokio::fs::create_dir_all(&transcript_dir)
            .await
            .expect("create transcript dir");
        tokio::fs::write(
            transcript_dir.join(WORKFLOW_TRANSCRIPT_ERROR_FILE),
            "stale\n",
        )
        .await
        .expect("write stale error");

        update_workflow_snapshot_in_dir(
            temp_dir.path(),
            &codex_code_mode::WaitOutcome::LiveCell(codex_code_mode::RuntimeResponse::Result {
                cell_id: codex_code_mode::CellId::new("cell-1".to_string()),
                content_items: vec![codex_code_mode::FunctionCallOutputContentItem::InputText {
                    text: "done".to_string(),
                }],
                error_text: None,
            }),
        )
        .await
        .expect("update snapshot")
        .expect("matching snapshot");

        let run = tokio::fs::read_to_string(transcript_dir.join(WORKFLOW_TRANSCRIPT_RUN_FILE))
            .await
            .expect("read transcript metadata");
        let run_json: JsonValue = serde_json::from_str(&run).expect("transcript metadata json");
        assert_eq!(run_json["status"], "completed");
        assert_eq!(run_json["status_history"][2]["status"], "completed");
        assert_eq!(
            tokio::fs::read_to_string(transcript_dir.join(WORKFLOW_TRANSCRIPT_OUTPUT_FILE))
                .await
                .expect("read output transcript"),
            "done\n"
        );
        assert!(!transcript_dir.join(WORKFLOW_TRANSCRIPT_ERROR_FILE).exists());
    }

    #[test]
    fn workflow_progress_notification_parse_requires_private_type() {
        assert_eq!(
            parse_workflow_progress_notification(
                r#"{"type":"codex_workflow_progress","event":"pipeline_failed","workflow":"release","message":"item 3 stage 2: boom","data":{"itemIndex":3,"stageIndex":2,"error":"boom"}}"#
            ),
            Some(WorkflowProgressNotification {
                event: "pipeline_failed".to_string(),
                workflow: Some("release".to_string()),
                phase: None,
                agent: None,
                agent_id: None,
                child: None,
                child_index: None,
                child_run_id: None,
                item_index: Some(3),
                stage_index: Some(2),
                step_index: None,
                error: Some("boom".to_string()),
                message: Some("item 3 stage 2: boom".to_string()),
                data: Some(serde_json::json!({
                    "itemIndex": 3,
                    "stageIndex": 2,
                    "error": "boom",
                })),
            })
        );
        assert!(parse_workflow_progress_notification("ordinary notify text").is_none());
        assert!(
            parse_workflow_progress_notification(r#"{"type":"other","event":"phase"}"#).is_none()
        );
        assert_eq!(
            parse_workflow_progress_notification(
                r#"{"type":"codex_workflow_progress","event":"child_complete","workflow":"release","child":"smoke","childIndex":2,"childRunId":"smoke#2","data":{"reference":"smoke"}}"#
            )
            .map(|notification| (
                notification.child,
                notification.child_index,
                notification.child_run_id,
            )),
            Some((Some("smoke".to_string()), Some(2), Some("smoke#2".to_string())))
        );
    }

    #[tokio::test]
    async fn workflow_progress_notification_updates_snapshot_marker_and_transcript() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let active_dir = temp_dir.path().join(WORKFLOW_ACTIVE_RUNS_DIR);
        let transcript_dir = temp_dir.path().join("wf_progress").join("transcripts");
        tokio::fs::create_dir_all(&active_dir)
            .await
            .expect("create active dir");
        let mut snapshot = workflow_snapshot("wf_progress", "cell-4", "running", 20);
        snapshot.as_object_mut().unwrap().insert(
            "transcript_dir".to_string(),
            JsonValue::String(transcript_dir.display().to_string()),
        );
        snapshot.as_object_mut().unwrap().insert(
            "session_id".to_string(),
            JsonValue::String("session-progress".to_string()),
        );
        snapshot.as_object_mut().unwrap().insert(
            "workflow_tool_call_id".to_string(),
            JsonValue::String("toolu_workflow".to_string()),
        );
        snapshot.as_object_mut().unwrap().insert(
            "cwd".to_string(),
            JsonValue::String("/tmp/project".to_string()),
        );
        snapshot.as_object_mut().unwrap().insert(
            "git_branch".to_string(),
            JsonValue::String("feature/workflows".to_string()),
        );
        write_snapshot(temp_dir.path(), "wf_progress.json", snapshot.clone()).await;
        write_snapshot(active_dir.as_path(), "wf_progress.json", snapshot).await;

        let update = append_workflow_progress_in_dir(
            temp_dir.path(),
            "cell-4",
            WorkflowProgressNotification {
                event: "agent_start".to_string(),
                workflow: Some("release".to_string()),
                phase: None,
                agent: Some("build_agent".to_string()),
                agent_id: None,
                child: None,
                child_index: None,
                child_run_id: None,
                item_index: None,
                stage_index: None,
                step_index: None,
                error: None,
                message: Some("build artifacts".to_string()),
                data: None,
            },
        )
        .await
        .expect("append progress")
        .expect("matching snapshot");
        assert_eq!(update.session_id.as_deref(), Some("session-progress"));
        assert_eq!(
            update.workflow_tool_call_id.as_deref(),
            Some("toolu_workflow")
        );
        assert_eq!(update.cwd.as_deref(), Some("/tmp/project"));
        assert_eq!(update.git_branch.as_deref(), Some("feature/workflows"));

        let snapshot = read_snapshot(temp_dir.path(), "wf_progress.json").await;
        assert_eq!(snapshot["status"], "running");
        assert_eq!(snapshot["progress"][0]["event"], "agent_start");
        assert_eq!(snapshot["progress"][0]["workflow"], "release");
        assert_eq!(snapshot["progress"][0]["agent"], "build_agent");
        assert_eq!(snapshot["progress"][0]["message"], "build artifacts");
        assert!(snapshot["updated_unix_ms"].as_u64().is_some());

        let active = read_snapshot(active_dir.as_path(), "wf_progress.json").await;
        assert_eq!(active["progress"][0]["event"], "agent_start");

        let run = tokio::fs::read_to_string(transcript_dir.join(WORKFLOW_TRANSCRIPT_RUN_FILE))
            .await
            .expect("read transcript metadata");
        let run_json: JsonValue = serde_json::from_str(&run).expect("transcript metadata json");
        assert_eq!(run_json["progress"][0]["agent"], "build_agent");
    }

    #[tokio::test]
    async fn workflow_complete_progress_clears_active_marker_and_wait_can_enrich_snapshot() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let active_dir = temp_dir.path().join(WORKFLOW_ACTIVE_RUNS_DIR);
        let transcript_dir = temp_dir.path().join("wf_complete").join("transcripts");
        tokio::fs::create_dir_all(&active_dir)
            .await
            .expect("create active dir");
        let mut snapshot = workflow_snapshot("wf_complete", "cell-5", "running", 20);
        snapshot.as_object_mut().unwrap().insert(
            "transcript_dir".to_string(),
            JsonValue::String(transcript_dir.display().to_string()),
        );
        write_snapshot(temp_dir.path(), "wf_complete.json", snapshot.clone()).await;
        write_snapshot(active_dir.as_path(), "wf_complete.json", snapshot).await;

        append_workflow_progress_in_dir(
            temp_dir.path(),
            "cell-5",
            WorkflowProgressNotification {
                event: "workflow_complete".to_string(),
                workflow: Some("release".to_string()),
                phase: None,
                agent: None,
                agent_id: None,
                child: None,
                child_index: None,
                child_run_id: None,
                item_index: None,
                stage_index: None,
                step_index: None,
                error: None,
                message: None,
                data: Some(serde_json::json!({ "agentCount": 1, "output": "progress done" })),
            },
        )
        .await
        .expect("append workflow complete progress")
        .expect("matching snapshot");

        let snapshot = read_snapshot(temp_dir.path(), "wf_complete.json").await;
        assert_eq!(snapshot["status"], "completed");
        assert_eq!(snapshot["output_preview"], "progress done");
        assert_eq!(snapshot["status_history"][2]["status"], "completed");
        assert_eq!(snapshot["status_history"][2]["message"], "progress done");
        assert!(!active_dir.join("wf_complete.json").exists());

        update_workflow_snapshot_in_dir(
            temp_dir.path(),
            &codex_code_mode::WaitOutcome::LiveCell(codex_code_mode::RuntimeResponse::Result {
                cell_id: codex_code_mode::CellId::new("cell-5".to_string()),
                content_items: vec![codex_code_mode::FunctionCallOutputContentItem::InputText {
                    text: "done".to_string(),
                }],
                error_text: None,
            }),
        )
        .await
        .expect("update completed workflow snapshot")
        .expect("matching snapshot");

        let snapshot = read_snapshot(temp_dir.path(), "wf_complete.json").await;
        assert_eq!(snapshot["status"], "completed");
        assert_eq!(snapshot["output_preview"], "done");
        assert!(!active_dir.join("wf_complete.json").exists());
    }

    #[tokio::test]
    async fn workflow_progress_notification_persists_structured_failure_metadata() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let transcript_dir = temp_dir.path().join("wf_failure").join("transcripts");
        let mut snapshot = workflow_snapshot("wf_failure", "cell-6", "running", 20);
        snapshot.as_object_mut().unwrap().insert(
            "transcript_dir".to_string(),
            JsonValue::String(transcript_dir.display().to_string()),
        );
        write_snapshot(temp_dir.path(), "wf_failure.json", snapshot).await;

        let update = append_workflow_progress_in_dir(
            temp_dir.path(),
            "cell-6",
            WorkflowProgressNotification {
                event: "pipeline_failed".to_string(),
                workflow: Some("release".to_string()),
                phase: None,
                agent: None,
                agent_id: None,
                child: None,
                child_index: None,
                child_run_id: None,
                item_index: Some(3),
                stage_index: Some(2),
                step_index: None,
                error: Some("boom".to_string()),
                message: Some("item 3 stage 2: boom".to_string()),
                data: Some(serde_json::json!({
                    "itemIndex": 3,
                    "stageIndex": 2,
                    "error": "boom",
                })),
            },
        )
        .await
        .expect("append progress")
        .expect("matching snapshot");

        assert_eq!(update.item_index, Some(3));
        assert_eq!(update.stage_index, Some(2));
        assert_eq!(update.step_index, None);
        assert_eq!(update.error.as_deref(), Some("boom"));

        let snapshot = read_snapshot(temp_dir.path(), "wf_failure.json").await;
        assert_eq!(snapshot["progress"][0]["event"], "pipeline_failed");
        assert_eq!(snapshot["progress"][0]["item_index"], 3);
        assert_eq!(snapshot["progress"][0]["stage_index"], 2);
        assert_eq!(snapshot["progress"][0]["error"], "boom");
        assert_eq!(snapshot["progress"][0]["data"]["itemIndex"], 3);
        assert_eq!(snapshot["progress"][0]["data"]["stageIndex"], 2);

        let run = tokio::fs::read_to_string(transcript_dir.join(WORKFLOW_TRANSCRIPT_RUN_FILE))
            .await
            .expect("read transcript metadata");
        let run_json: JsonValue = serde_json::from_str(&run).expect("transcript metadata json");
        assert_eq!(run_json["progress"][0]["item_index"], 3);
        assert_eq!(run_json["progress"][0]["stage_index"], 2);
        assert_eq!(run_json["progress"][0]["error"], "boom");
    }

    #[tokio::test]
    async fn workflow_progress_notification_persists_child_invocation_metadata() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let transcript_dir = temp_dir.path().join("wf_child").join("transcripts");
        let mut snapshot = workflow_snapshot("wf_child", "cell-7", "running", 20);
        snapshot.as_object_mut().unwrap().insert(
            "transcript_dir".to_string(),
            JsonValue::String(transcript_dir.display().to_string()),
        );
        write_snapshot(temp_dir.path(), "wf_child.json", snapshot).await;

        let update = append_workflow_progress_in_dir(
            temp_dir.path(),
            "cell-7",
            WorkflowProgressNotification {
                event: "child_complete".to_string(),
                workflow: Some("release".to_string()),
                phase: None,
                agent: None,
                agent_id: None,
                child: Some("smoke".to_string()),
                child_index: Some(2),
                child_run_id: Some("smoke#2".to_string()),
                item_index: None,
                stage_index: None,
                step_index: None,
                error: None,
                message: None,
                data: Some(serde_json::json!({
                    "childIndex": 2,
                    "childRunId": "smoke#2",
                    "reference": "smoke",
                })),
            },
        )
        .await
        .expect("append progress")
        .expect("matching snapshot");

        assert_eq!(update.child.as_deref(), Some("smoke"));
        assert_eq!(update.child_index, Some(2));
        assert_eq!(update.child_run_id.as_deref(), Some("smoke#2"));

        let snapshot = read_snapshot(temp_dir.path(), "wf_child.json").await;
        assert_eq!(snapshot["progress"][0]["child"], "smoke");
        assert_eq!(snapshot["progress"][0]["child_index"], 2);
        assert_eq!(snapshot["progress"][0]["child_run_id"], "smoke#2");
        assert_eq!(snapshot["progress"][0]["data"]["reference"], "smoke");

        let run = tokio::fs::read_to_string(transcript_dir.join(WORKFLOW_TRANSCRIPT_RUN_FILE))
            .await
            .expect("read transcript metadata");
        let run_json: JsonValue = serde_json::from_str(&run).expect("transcript metadata json");
        assert_eq!(run_json["progress"][0]["child_index"], 2);
        assert_eq!(run_json["progress"][0]["child_run_id"], "smoke#2");
    }

    #[tokio::test]
    async fn workflow_agent_control_event_requires_running_matching_agent() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let mut snapshot = workflow_snapshot("wf_control", "cell-9", "running", 20);
        snapshot.as_object_mut().unwrap().insert(
            "progress".to_string(),
            serde_json::json!([
                {
                    "event": "agent_start",
                    "agent": "builder",
                    "agent_id": "/root/workflow_release_1",
                    "unix_ms": 100_u64
                },
                {
                    "event": "agent_start",
                    "agent": "reviewer",
                    "agent_id": "/root/workflow_release_2",
                    "unix_ms": 110_u64
                },
                {
                    "event": "agent_complete",
                    "agent": "reviewer",
                    "agent_id": "/root/workflow_release_2",
                    "unix_ms": 120_u64
                },
                {
                    "event": "agent_start",
                    "agent": "detached",
                    "agent_id": "/root/workflow_release_3",
                    "unix_ms": 130_u64
                },
                {
                    "event": "agent_detached",
                    "agent": "detached",
                    "agent_id": "/root/workflow_release_3",
                    "unix_ms": 140_u64
                }
            ]),
        );
        write_snapshot(temp_dir.path(), "wf_control.json", snapshot).await;

        assert!(
            running_workflow_agent_for_run_in_dir(
                temp_dir.path(),
                "wf_control",
                "/root/workflow_release_2",
            )
            .await
            .expect("lookup completed agent")
            .is_none()
        );
        let target = running_workflow_agent_for_run_in_dir(
            temp_dir.path(),
            "wf_control",
            "/root/workflow_release_1",
        )
        .await
        .expect("lookup running agent")
        .expect("running target");
        assert_eq!(target.cell_id, "cell-9");
        assert_eq!(target.status, "running");
        assert_eq!(target.agent.as_deref(), Some("builder"));
        let detached_target = running_workflow_agent_for_run_in_dir(
            temp_dir.path(),
            "wf_control",
            "/root/workflow_release_3",
        )
        .await
        .expect("lookup detached agent")
        .expect("detached target");
        assert_eq!(detached_target.status, "detached");
        assert_eq!(detached_target.agent.as_deref(), Some("detached"));

        let update = record_workflow_agent_control_event_in_dir(
            temp_dir.path(),
            &target,
            "agent_interrupted",
            Some("interrupt requested"),
        )
        .await
        .expect("record control event")
        .expect("progress update");
        assert_eq!(update.run_id, "wf_control");
        assert_eq!(update.cell_id, "cell-9");
        assert_eq!(update.event, "agent_interrupted");
        assert_eq!(update.agent_id.as_deref(), Some("/root/workflow_release_1"));

        let snapshot = read_snapshot(temp_dir.path(), "wf_control.json").await;
        let progress = snapshot["progress"].as_array().expect("progress array");
        let last = progress.last().expect("control progress event");
        assert_eq!(last["event"], "agent_interrupted");
        assert_eq!(last["agent_id"], "/root/workflow_release_1");
        assert_eq!(last["message"], "interrupt requested");
        assert!(
            running_workflow_agent_for_run_in_dir(
                temp_dir.path(),
                "wf_control",
                "/root/workflow_release_1",
            )
            .await
            .expect("lookup interrupted agent")
            .is_none()
        );
    }

    #[tokio::test]
    async fn workflow_agent_control_request_reads_latest_selected_agent_request() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let mut snapshot = workflow_snapshot("wf_control", "cell-9", "running", 20);
        snapshot.as_object_mut().unwrap().insert(
            "progress".to_string(),
            serde_json::json!([
                {
                    "event": "agent_start",
                    "agent": "builder",
                    "agent_id": "/root/workflow_release_1",
                    "unix_ms": 100_u64
                }
            ]),
        );
        write_snapshot(temp_dir.path(), "wf_control.json", snapshot).await;

        assert!(
            workflow_agent_control_request_for_run_in_dir(
                temp_dir.path(),
                "wf_control",
                "/root/workflow_release_1",
            )
            .await
            .expect("control lookup before request")
            .is_none()
        );

        let target = running_workflow_agent_for_run_in_dir(
            temp_dir.path(),
            "wf_control",
            "/root/workflow_release_1",
        )
        .await
        .expect("lookup running agent")
        .expect("running target");
        record_workflow_agent_control_event_in_dir(
            temp_dir.path(),
            &target,
            "agent_skip_requested",
            Some("skip requested"),
        )
        .await
        .expect("record skip request")
        .expect("progress update");

        let request = workflow_agent_control_request_for_run_in_dir(
            temp_dir.path(),
            "wf_control",
            "/root/workflow_release_1",
        )
        .await
        .expect("control lookup after request")
        .expect("skip request");
        assert_eq!(request.action, "skip");
        assert_eq!(request.event, "agent_skip_requested");
        assert_eq!(request.message.as_deref(), Some("skip requested"));
        let snapshot = read_snapshot(temp_dir.path(), "wf_control.json").await;
        assert_eq!(
            snapshot["agent_controls"]["/root/workflow_release_1"]["action"],
            "skip"
        );
        assert!(
            running_workflow_agent_for_run_in_dir(
                temp_dir.path(),
                "wf_control",
                "/root/workflow_release_1",
            )
            .await
            .expect("lookup requested agent")
            .is_none()
        );

        append_workflow_progress_in_dir(
            temp_dir.path(),
            "cell-9",
            WorkflowProgressNotification {
                event: "agent_skipped".to_string(),
                workflow: Some("release".to_string()),
                phase: None,
                agent: Some("builder".to_string()),
                agent_id: Some("/root/workflow_release_1".to_string()),
                child: None,
                child_index: None,
                child_run_id: None,
                item_index: None,
                stage_index: None,
                step_index: None,
                error: None,
                message: Some("skip applied".to_string()),
                data: None,
            },
        )
        .await
        .expect("append skip ack")
        .expect("progress update");
        let snapshot = read_snapshot(temp_dir.path(), "wf_control.json").await;
        assert!(snapshot.get("agent_controls").is_none());
        assert!(
            workflow_agent_control_request_for_run_in_dir(
                temp_dir.path(),
                "wf_control",
                "/root/workflow_release_1",
            )
            .await
            .expect("control lookup after ack")
            .is_none()
        );
    }

    #[tokio::test]
    async fn workflow_agent_journal_notification_appends_journal_file() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let run_dir = temp_dir.path().join("wf_journal");
        let mut snapshot = workflow_snapshot("wf_journal", "cell-5", "running", 20);
        snapshot.as_object_mut().unwrap().insert(
            "run_dir".to_string(),
            JsonValue::String(run_dir.display().to_string()),
        );
        snapshot.as_object_mut().unwrap().insert(
            "session_id".to_string(),
            JsonValue::String("session-journal".to_string()),
        );
        write_snapshot(temp_dir.path(), "wf_journal.json", snapshot).await;

        let started_update = append_workflow_progress_in_dir(
            temp_dir.path(),
            "cell-5",
            WorkflowProgressNotification {
                event: "agent_journal_started".to_string(),
                workflow: Some("release".to_string()),
                phase: None,
                agent: Some("reviewer".to_string()),
                agent_id: None,
                child: None,
                child_index: None,
                child_run_id: None,
                item_index: None,
                stage_index: None,
                step_index: None,
                error: None,
                message: None,
                data: Some(serde_json::json!({
                    "key": "{\"message\":\"review task\"}",
                    "agentId": "agent-123",
                })),
            },
        )
        .await
        .expect("append started journal");
        assert_eq!(started_update, None);

        let update = append_workflow_progress_in_dir(
            temp_dir.path(),
            "cell-5",
            WorkflowProgressNotification {
                event: "agent_journal_entry".to_string(),
                workflow: Some("release".to_string()),
                phase: None,
                agent: Some("reviewer".to_string()),
                agent_id: None,
                child: None,
                child_index: None,
                child_run_id: None,
                item_index: None,
                stage_index: None,
                step_index: None,
                error: None,
                message: Some("review task".to_string()),
                data: Some(serde_json::json!({
                    "key": "{\"message\":\"review task\"}",
                    "result": { "ok": true },
                })),
            },
        )
        .await
        .expect("append journal");
        assert_eq!(update, None);

        let child_update = append_workflow_progress_in_dir(
            temp_dir.path(),
            "cell-5",
            WorkflowProgressNotification {
                event: "child_journal_entry".to_string(),
                workflow: Some("release".to_string()),
                phase: None,
                agent: None,
                agent_id: None,
                child: Some("smoke".to_string()),
                child_index: Some(1),
                child_run_id: Some("smoke#1".to_string()),
                item_index: None,
                stage_index: None,
                step_index: None,
                error: None,
                message: None,
                data: Some(serde_json::json!({
                    "key": "codex-child-v1:abc123",
                    "childRunId": "smoke#1",
                    "result": { "child": "ok" },
                })),
            },
        )
        .await
        .expect("append child journal");
        assert_eq!(child_update, None);

        let journal = tokio::fs::read_to_string(run_dir.join(WORKFLOW_AGENT_JOURNAL_FILE))
            .await
            .expect("read journal");
        let mirror_journal = tokio::fs::read_to_string(
            temp_dir
                .path()
                .join("session-journal")
                .join("subagents")
                .join("workflows")
                .join("wf_journal")
                .join(WORKFLOW_AGENT_JOURNAL_FILE),
        )
        .await
        .expect("read mirrored journal");
        assert_eq!(journal, mirror_journal);
        let entries = journal
            .lines()
            .map(|line| serde_json::from_str::<JsonValue>(line).expect("journal line json"))
            .collect::<Vec<_>>();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0]["type"], "started");
        assert_eq!(entries[0]["key"], "{\"message\":\"review task\"}");
        assert_eq!(entries[0]["agentId"], "agent-123");
        assert_eq!(entries[1]["type"], "result");
        assert_eq!(entries[1]["key"], "{\"message\":\"review task\"}");
        assert_eq!(entries[1]["agentId"], "reviewer");
        assert_eq!(entries[1]["result"]["ok"], true);
        assert_eq!(entries[2]["type"], "child_result");
        assert_eq!(entries[2]["key"], "codex-child-v1:abc123");
        assert_eq!(entries[2]["child"], "smoke");
        assert_eq!(entries[2]["childRunId"], "smoke#1");
        assert_eq!(entries[2]["result"]["child"], "ok");
        let snapshot = read_snapshot(temp_dir.path(), "wf_journal.json").await;
        assert!(snapshot.get("progress").is_none());
    }

    #[tokio::test]
    async fn workflow_agent_transcript_notification_appends_agent_transcript_file() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let transcript_dir = temp_dir.path().join("wf_transcript").join("transcripts");
        let mut snapshot = workflow_snapshot("wf_transcript", "cell-8", "running", 20);
        snapshot.as_object_mut().unwrap().insert(
            "transcript_dir".to_string(),
            JsonValue::String(transcript_dir.display().to_string()),
        );
        snapshot.as_object_mut().unwrap().insert(
            "session_id".to_string(),
            JsonValue::String("session-transcript".to_string()),
        );
        write_snapshot(temp_dir.path(), "wf_transcript.json", snapshot).await;

        let update = append_workflow_progress_in_dir(
            temp_dir.path(),
            "cell-8",
            WorkflowProgressNotification {
                event: "agent_transcript_entry".to_string(),
                workflow: Some("release".to_string()),
                phase: None,
                agent: Some("reviewer".to_string()),
                agent_id: None,
                child: None,
                child_index: None,
                child_run_id: None,
                item_index: None,
                stage_index: None,
                step_index: None,
                error: None,
                message: Some("review task".to_string()),
                data: Some(serde_json::json!({
                    "agentId": "reviewer",
                    "prompt": "review task",
                    "reasoning": [
                        { "summary": "checked relevant files" }
                    ],
                    "toolCalls": [
                        {
                            "name": "Read",
                            "input": { "file_path": "src/lib.rs" },
                            "output": "pub fn ok() {}"
                        }
                    ],
                    "finalText": "review complete",
                })),
            },
        )
        .await
        .expect("append transcript");
        assert_eq!(update, None);

        let transcript = tokio::fs::read_to_string(transcript_dir.join("agent-reviewer.jsonl"))
            .await
            .expect("read agent transcript");
        let mirrored_transcript = tokio::fs::read_to_string(
            temp_dir
                .path()
                .join("session-transcript")
                .join("subagents")
                .join("workflows")
                .join("wf_transcript")
                .join("agent-reviewer.jsonl"),
        )
        .await
        .expect("read mirrored agent transcript");
        let mirrored_entries = mirrored_transcript
            .lines()
            .map(|line| {
                serde_json::from_str::<JsonValue>(line).expect("mirror transcript line json")
            })
            .collect::<Vec<_>>();
        let entries = transcript
            .lines()
            .map(|line| serde_json::from_str::<JsonValue>(line).expect("transcript line json"))
            .collect::<Vec<_>>();
        assert_eq!(mirrored_entries.len(), 2);
        assert_eq!(mirrored_entries[0]["type"], "user");
        assert_eq!(
            mirrored_entries[0]["message"]["content"][0]["text"],
            "review task"
        );
        assert_eq!(mirrored_entries[1]["type"], "assistant");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0]["type"], "user");
        assert_eq!(entries[0]["message"]["content"][0]["text"], "review task");
        assert_eq!(entries[1]["type"], "assistant");
        assert_eq!(entries[1]["message"]["content"][0]["type"], "reasoning");
        assert_eq!(
            entries[1]["message"]["content"][0]["text"],
            "checked relevant files"
        );
        assert_eq!(entries[1]["message"]["content"][1]["type"], "tool_use");
        assert_eq!(entries[1]["message"]["content"][1]["name"], "Read");
        assert_eq!(
            entries[1]["message"]["content"][1]["input"]["file_path"],
            "src/lib.rs"
        );
        assert_eq!(
            entries[1]["message"]["content"][1]["output"],
            "pub fn ok() {}"
        );
        assert_eq!(
            entries[1]["message"]["content"][2]["text"],
            "review complete"
        );
        let snapshot = read_snapshot(temp_dir.path(), "wf_transcript.json").await;
        assert!(snapshot.get("progress").is_none());
    }

    #[tokio::test]
    async fn workflow_agent_transcript_notification_appends_raw_child_transcript_records() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let transcript_dir = temp_dir
            .path()
            .join("wf_raw_transcript")
            .join("transcripts");
        let mut snapshot = workflow_snapshot("wf_raw_transcript", "cell-9", "running", 20);
        snapshot.as_object_mut().unwrap().insert(
            "transcript_dir".to_string(),
            JsonValue::String(transcript_dir.display().to_string()),
        );
        snapshot.as_object_mut().unwrap().insert(
            "session_id".to_string(),
            JsonValue::String("session-raw".to_string()),
        );
        snapshot.as_object_mut().unwrap().insert(
            "thread_id".to_string(),
            JsonValue::String("thread-raw".to_string()),
        );
        snapshot.as_object_mut().unwrap().insert(
            "workflow_tool_call_id".to_string(),
            JsonValue::String("call-workflow".to_string()),
        );
        snapshot.as_object_mut().unwrap().insert(
            "cwd".to_string(),
            JsonValue::String("/tmp/project".to_string()),
        );
        snapshot.as_object_mut().unwrap().insert(
            "git_branch".to_string(),
            JsonValue::String("feature/workflows".to_string()),
        );
        write_snapshot(temp_dir.path(), "wf_raw_transcript.json", snapshot).await;

        let start_update = append_workflow_progress_in_dir(
            temp_dir.path(),
            "cell-9",
            WorkflowProgressNotification {
                event: "agent_transcript_entry".to_string(),
                workflow: Some("release".to_string()),
                phase: None,
                agent: Some("reviewer".to_string()),
                agent_id: None,
                child: None,
                child_index: None,
                child_run_id: None,
                item_index: None,
                stage_index: None,
                step_index: None,
                error: None,
                message: Some("review task".to_string()),
                data: Some(serde_json::json!({
                    "agentId": "/root/workflow_release_1",
                    "prompt": "review task",
                    "metadata": {
                        "taskName": "/root/workflow_release_1",
                        "agentType": "explorer",
                        "model": "gpt-5.5",
                        "reasoningEffort": "xhigh",
                        "serviceTier": "priority",
                        "nickname": "Ada",
                        "toolUseId": "toolu_spawn",
                        "cwd": "/tmp/project",
                        "gitBranch": "feature/workflows",
                        "worktreePath": "/tmp/project/.codex/worktrees/release"
                    }
                })),
            },
        )
        .await
        .expect("append transcript start");
        assert_eq!(start_update, None);

        let update = append_workflow_progress_in_dir(
            temp_dir.path(),
            "cell-9",
            WorkflowProgressNotification {
                event: "agent_transcript_entry".to_string(),
                workflow: Some("release".to_string()),
                phase: None,
                agent: Some("reviewer".to_string()),
                agent_id: None,
                child: None,
                child_index: None,
                child_run_id: None,
                item_index: None,
                stage_index: None,
                step_index: None,
                error: None,
                message: Some("review task".to_string()),
                data: Some(serde_json::json!({
                    "agentId": "/root/workflow_release_1",
                    "prompt": "review task",
                    "promptRecorded": true,
                    "metadata": {
                        "taskName": "/root/workflow_release_1",
                        "agentType": "explorer",
                        "model": "gpt-5.5",
                        "reasoningEffort": "xhigh",
                        "serviceTier": "priority",
                        "author": "/root/workflow_release_1",
                        "recipient": "/root"
                    },
                    "transcript": [
                        {
                            "role": "assistant",
                            "content": [
                                {
                                    "type": "reasoning",
                                    "summary": [
                                        { "type": "summary_text", "text": "planned raw child history" }
                                    ]
                                }
                            ]
                        },
                        {
                            "role": "assistant",
                            "content": [
                                {
                                    "type": "tool_use",
                                    "id": "toolu_glob",
                                    "name": "Glob",
                                    "input": { "pattern": "src/**/*.rs" }
                                }
                            ]
                        },
                        {
                            "type": "tool_result",
                            "tool_use_id": "toolu_glob",
                            "content": "src/lib.rs"
                        },
                        {
                            "role": "assistant",
                            "content": [
                                {
                                    "type": "reasoning",
                                    "summary": [
                                        { "type": "summary_text", "text": "read raw child history" }
                                    ]
                                }
                            ]
                        },
                        {
                            "type": "user",
                            "toolUseId": "toolu_glob",
                            "toolUseResult": "docs.md"
                        },
                        {
                            "role": "assistant",
                            "content": [
                                {
                                    "type": "tool_use",
                                    "id": "toolu_read",
                                    "name": "Read",
                                    "input": { "file_path": "src/lib.rs" }
                                }
                            ]
                        },
                        {
                            "type": "tool_result",
                            "tool_use_id": "toolu_read",
                            "content": "pub fn ok() {}"
                        }
                    ],
                    "reasoning": [
                        { "summary": "summary fallback should not duplicate" }
                    ],
                    "toolCalls": [
                        {
                            "name": "Write",
                            "input": { "file_path": "ignored.rs" },
                            "output": "ignored"
                        }
                    ],
                    "finalText": "review complete",
                })),
            },
        )
        .await
        .expect("append transcript");
        assert_eq!(update, None);

        let transcript =
            tokio::fs::read_to_string(transcript_dir.join("agent-root_workflow_release_1.jsonl"))
                .await
                .expect("read agent transcript");
        let entries = transcript
            .lines()
            .map(|line| serde_json::from_str::<JsonValue>(line).expect("transcript line json"))
            .collect::<Vec<_>>();
        assert_eq!(entries.len(), 9);
        for entry in &entries {
            assert_eq!(entry["agentName"], "reviewer");
            assert_eq!(entry["sessionKind"], "workflow_agent");
            assert_eq!(entry["parentThreadId"], "thread-raw");
        }
        assert_eq!(entries[0]["type"], "user");
        assert_eq!(entries[0]["isSidechain"], true);
        assert_eq!(entries[0]["agentId"], "/root/workflow_release_1");
        assert_eq!(entries[0]["sessionId"], "session-raw");
        assert_eq!(entries[0]["threadId"], "thread-raw");
        assert_eq!(entries[0]["runId"], "wf_raw_transcript");
        assert_eq!(entries[0]["cellId"], "cell-9");
        assert_eq!(entries[0]["workflowToolCallId"], "call-workflow");
        assert_eq!(entries[0]["cwd"], "/tmp/project");
        assert_eq!(entries[0]["gitBranch"], "feature/workflows");
        assert_eq!(entries[0]["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(entries[0]["entrypoint"], "workflow");
        assert!(
            entries[0]["uuid"]
                .as_str()
                .is_some_and(|uuid| !uuid.is_empty())
        );
        assert_eq!(entries[1]["role"], "assistant");
        assert_eq!(entries[1]["parentUuid"], entries[0]["uuid"]);
        assert_eq!(entries[1]["logicalParentUuid"], entries[0]["uuid"]);
        assert_eq!(entries[1]["isSidechain"], true);
        assert_eq!(entries[1]["content"][0]["type"], "reasoning");
        assert_eq!(
            entries[1]["content"][0]["summary"][0]["text"],
            "planned raw child history"
        );
        assert_eq!(entries[2]["content"][0]["type"], "tool_use");
        assert_eq!(entries[2]["content"][0]["id"], "toolu_glob");
        assert_eq!(entries[2]["content"][0]["name"], "Glob");
        assert_eq!(entries[2]["content"][0]["input"]["pattern"], "src/**/*.rs");
        assert_eq!(entries[3]["type"], "tool_result");
        assert_eq!(entries[3]["parentUuid"], entries[2]["uuid"]);
        assert_eq!(entries[3]["sourceToolAssistantUUID"], entries[2]["uuid"]);
        assert_eq!(entries[3]["tool_use_id"], "toolu_glob");
        assert_eq!(entries[3]["content"], "src/lib.rs");
        assert_eq!(entries[4]["role"], "assistant");
        assert_eq!(entries[4]["content"][0]["type"], "reasoning");
        assert_eq!(
            entries[4]["content"][0]["summary"][0]["text"],
            "read raw child history"
        );
        assert_eq!(entries[5]["type"], "user");
        assert_eq!(entries[5]["sourceToolAssistantUUID"], entries[2]["uuid"]);
        assert_eq!(entries[5]["toolUseId"], "toolu_glob");
        assert_eq!(entries[5]["toolUseResult"], "docs.md");
        assert_eq!(entries[6]["content"][0]["type"], "tool_use");
        assert_eq!(entries[6]["content"][0]["id"], "toolu_read");
        assert_eq!(entries[7]["type"], "tool_result");
        assert_eq!(entries[7]["sourceToolAssistantUUID"], entries[6]["uuid"]);
        assert_eq!(entries[7]["tool_use_id"], "toolu_read");
        assert_eq!(entries[7]["content"], "pub fn ok() {}");
        assert_eq!(entries[8]["type"], "assistant");
        assert_eq!(entries[8]["parentUuid"], entries[7]["uuid"]);
        assert_eq!(entries[8]["message"]["content"][0]["type"], "text");
        assert_eq!(
            entries[8]["message"]["content"][0]["text"],
            "review complete"
        );
        assert!(!transcript.contains("summary fallback should not duplicate"));
        assert!(!transcript.contains("ignored.rs"));
        let live_mirrored_update = append_workflow_progress_in_dir(
            temp_dir.path(),
            "cell-9",
            WorkflowProgressNotification {
                event: "agent_transcript_entry".to_string(),
                workflow: Some("release".to_string()),
                phase: None,
                agent: Some("reviewer".to_string()),
                agent_id: None,
                child: None,
                child_index: None,
                child_run_id: None,
                item_index: None,
                stage_index: None,
                step_index: None,
                error: None,
                message: Some("review task".to_string()),
                data: Some(serde_json::json!({
                    "agentId": "/root/workflow_release_1",
                    "prompt": "review task",
                    "promptRecorded": true,
                    "transcriptRecorded": true,
                    "metadata": {
                        "taskName": "/root/workflow_release_1",
                        "author": "/root/workflow_release_1",
                        "recipient": "/root"
                    },
                    "transcript": [
                        {
                            "role": "assistant",
                            "content": [{ "type": "text", "text": "already mirrored" }]
                        }
                    ],
                    "finalText": "live mirrored review complete",
                })),
            },
        )
        .await
        .expect("append live-mirrored transcript completion");
        assert_eq!(live_mirrored_update, None);

        let transcript =
            tokio::fs::read_to_string(transcript_dir.join("agent-root_workflow_release_1.jsonl"))
                .await
                .expect("read live-mirrored agent transcript");
        assert_eq!(transcript.lines().count(), entries.len());

        let metadata = tokio::fs::read_to_string(
            transcript_dir.join("agent-root_workflow_release_1.meta.json"),
        )
        .await
        .expect("read agent metadata");
        let metadata =
            serde_json::from_str::<JsonValue>(&metadata).expect("agent metadata should be json");
        assert_eq!(metadata["version"], "codex-workflow-agent-meta-v1");
        assert_eq!(metadata["agentId"], "/root/workflow_release_1");
        assert_eq!(metadata["name"], "reviewer");
        assert_eq!(metadata["agentName"], "reviewer");
        assert_eq!(metadata["sessionKind"], "workflow_agent");
        assert_eq!(metadata["parentThreadId"], "thread-raw");
        assert_eq!(metadata["workflow"], "release");
        assert_eq!(metadata["runId"], "wf_raw_transcript");
        assert_eq!(metadata["cellId"], "cell-9");
        assert_eq!(
            metadata["transcriptDir"],
            transcript_dir.display().to_string()
        );
        assert_eq!(metadata["taskName"], "/root/workflow_release_1");
        assert_eq!(metadata["agentType"], "explorer");
        assert_eq!(metadata["model"], "gpt-5.5");
        assert_eq!(
            metadata["finalTextPreview"],
            "live mirrored review complete"
        );
        assert_eq!(metadata["reasoningEffort"], "xhigh");
        assert_eq!(metadata["serviceTier"], "priority");
        assert_eq!(metadata["nickname"], "Ada");
        assert_eq!(metadata["toolUseId"], "toolu_spawn");
        assert_eq!(metadata["cwd"], "/tmp/project");
        assert_eq!(metadata["gitBranch"], "feature/workflows");
        assert_eq!(
            metadata["worktreePath"],
            "/tmp/project/.codex/worktrees/release"
        );
        assert_eq!(metadata["author"], "/root/workflow_release_1");
        assert_eq!(metadata["recipient"], "/root");
        assert_eq!(metadata["prompt"], "review task");
        assert_eq!(metadata["agent_metadata"]["type"], "agent_metadata");
        assert_eq!(metadata["agent_metadata"]["agentType"], "explorer");
        assert_eq!(metadata["agent_metadata"]["agentName"], "reviewer");
        assert_eq!(metadata["agent_metadata"]["sessionKind"], "workflow_agent");
        assert_eq!(metadata["agent_metadata"]["parentThreadId"], "thread-raw");
        assert_eq!(
            metadata["agent_metadata"]["worktreePath"],
            "/tmp/project/.codex/worktrees/release"
        );
        assert_eq!(metadata["agent_metadata"]["cwd"], "/tmp/project");
        assert_eq!(metadata["agent_metadata"]["description"], "review task");
        assert_eq!(metadata["agent_metadata"]["name"], "reviewer");
        assert_eq!(metadata["agent_metadata"]["toolUseId"], "toolu_spawn");
    }

    #[tokio::test]
    async fn wait_missing_cell_marks_running_workflow_snapshot_failed() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        write_snapshot(
            temp_dir.path(),
            "wf_missing.json",
            workflow_snapshot("wf_missing", "cell-2", "running", 20),
        )
        .await;

        update_workflow_snapshot_in_dir(
            temp_dir.path(),
            &codex_code_mode::WaitOutcome::MissingCell(codex_code_mode::RuntimeResponse::Result {
                cell_id: codex_code_mode::CellId::new("cell-2".to_string()),
                content_items: Vec::new(),
                error_text: Some("exec cell cell-2 not found".to_string()),
            }),
        )
        .await
        .expect("update snapshot")
        .expect("matching snapshot");

        let snapshot = read_snapshot(temp_dir.path(), "wf_missing.json").await;
        assert_eq!(snapshot["status"], "failed");
        assert_eq!(snapshot["status_history"][2]["status"], "failed");
        assert!(
            snapshot["error"]
                .as_str()
                .expect("error")
                .contains("exec cell cell-2 not found"),
            "{snapshot:#}"
        );
    }

    #[tokio::test]
    async fn wait_terminate_marks_running_workflow_snapshot_terminated() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        write_snapshot(
            temp_dir.path(),
            "wf_terminated.json",
            workflow_snapshot("wf_terminated", "cell-3", "running", 20),
        )
        .await;

        update_workflow_snapshot_in_dir(
            temp_dir.path(),
            &codex_code_mode::WaitOutcome::LiveCell(codex_code_mode::RuntimeResponse::Terminated {
                cell_id: codex_code_mode::CellId::new("cell-3".to_string()),
                content_items: vec![codex_code_mode::FunctionCallOutputContentItem::InputText {
                    text: "stopped".to_string(),
                }],
            }),
        )
        .await
        .expect("update snapshot")
        .expect("matching snapshot");

        let snapshot = read_snapshot(temp_dir.path(), "wf_terminated.json").await;
        assert_eq!(snapshot["status"], "terminated");
        assert_eq!(snapshot["status_history"][2]["status"], "terminated");
        assert_eq!(snapshot["error"], "stopped");
    }
}
