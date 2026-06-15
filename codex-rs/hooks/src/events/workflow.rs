use std::path::PathBuf;

use codex_protocol::ThreadId;
use codex_protocol::protocol::HookCompletedEvent;
use codex_protocol::protocol::HookEventName;
use codex_protocol::protocol::HookOutputEntry;
use codex_protocol::protocol::HookOutputEntryKind;
use codex_protocol::protocol::HookRunStatus;
use codex_protocol::protocol::HookRunSummary;
use codex_utils_absolute_path::AbsolutePathBuf;
use serde_json::Value;

use super::common;
use crate::engine::CommandShell;
use crate::engine::ConfiguredHandler;
use crate::engine::command_runner::CommandRunResult;
use crate::engine::dispatcher;
use crate::engine::output_parser;
use crate::schema::NotificationCommandInput;
use crate::schema::NullableString;
use crate::schema::SubagentCommandInputFields;
use crate::schema::TaskCompletedCommandInput;
use crate::schema::TaskCreatedCommandInput;

#[derive(Debug, Clone)]
pub struct TaskCreatedRequest {
    pub session_id: ThreadId,
    pub turn_id: String,
    pub subagent: Option<common::SubagentHookContext>,
    pub cwd: AbsolutePathBuf,
    pub transcript_path: Option<PathBuf>,
    pub model: String,
    pub permission_mode: String,
    pub workflow_name: String,
    pub task_id: String,
    pub task_subject: String,
    pub task_description: Option<String>,
    pub cell_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TaskCompletedRequest {
    pub session_id: ThreadId,
    pub turn_id: String,
    pub subagent: Option<common::SubagentHookContext>,
    pub cwd: AbsolutePathBuf,
    pub transcript_path: Option<PathBuf>,
    pub model: String,
    pub permission_mode: String,
    pub workflow_name: String,
    pub task_id: String,
    pub task_subject: String,
    pub task_description: Option<String>,
    pub cell_id: Option<String>,
    pub status: String,
}

#[derive(Debug, Clone)]
pub struct NotificationRequest {
    pub session_id: ThreadId,
    pub turn_id: String,
    pub subagent: Option<common::SubagentHookContext>,
    pub cwd: AbsolutePathBuf,
    pub transcript_path: Option<PathBuf>,
    pub model: String,
    pub permission_mode: String,
    pub run_id: String,
    pub cell_id: String,
    pub event: String,
    pub unix_ms: i64,
    pub workflow: Option<String>,
    pub phase: Option<String>,
    pub agent: Option<String>,
    pub agent_id: Option<String>,
    pub child: Option<String>,
    pub child_index: Option<u64>,
    pub child_run_id: Option<String>,
    pub item_index: Option<u64>,
    pub stage_index: Option<u64>,
    pub step_index: Option<u64>,
    pub error: Option<String>,
    pub message: Option<String>,
    pub data: Option<Value>,
}

#[derive(Debug, Default)]
pub struct WorkflowHookOutcome {
    pub hook_events: Vec<HookCompletedEvent>,
}

#[derive(Debug, Default, PartialEq, Eq)]
struct WorkflowHandlerData;

pub(crate) fn preview_task_created(
    handlers: &[ConfiguredHandler],
    request: &TaskCreatedRequest,
) -> Vec<HookRunSummary> {
    preview(
        handlers,
        HookEventName::TaskCreated,
        request.workflow_name.as_str(),
    )
}

pub(crate) async fn run_task_created(
    handlers: &[ConfiguredHandler],
    shell: &CommandShell,
    request: TaskCreatedRequest,
) -> WorkflowHookOutcome {
    let matched = dispatcher::select_handlers(
        handlers,
        HookEventName::TaskCreated,
        Some(request.workflow_name.as_str()),
    );
    if matched.is_empty() {
        return WorkflowHookOutcome::default();
    }

    let input_json = match task_created_command_input_json(&request) {
        Ok(input_json) => input_json,
        Err(error) => {
            return serialization_failure_outcome(common::serialization_failure_hook_events(
                matched,
                Some(request.turn_id),
                format!("failed to serialize task created hook input: {error}"),
            ));
        }
    };

    execute_stateless_handlers(
        shell,
        matched,
        input_json,
        request.cwd.as_path(),
        request.turn_id,
    )
    .await
}

pub(crate) fn preview_task_completed(
    handlers: &[ConfiguredHandler],
    request: &TaskCompletedRequest,
) -> Vec<HookRunSummary> {
    preview(
        handlers,
        HookEventName::TaskCompleted,
        request.workflow_name.as_str(),
    )
}

pub(crate) async fn run_task_completed(
    handlers: &[ConfiguredHandler],
    shell: &CommandShell,
    request: TaskCompletedRequest,
) -> WorkflowHookOutcome {
    let matched = dispatcher::select_handlers(
        handlers,
        HookEventName::TaskCompleted,
        Some(request.workflow_name.as_str()),
    );
    if matched.is_empty() {
        return WorkflowHookOutcome::default();
    }

    let input_json = match task_completed_command_input_json(&request) {
        Ok(input_json) => input_json,
        Err(error) => {
            return serialization_failure_outcome(common::serialization_failure_hook_events(
                matched,
                Some(request.turn_id),
                format!("failed to serialize task completed hook input: {error}"),
            ));
        }
    };

    execute_stateless_handlers(
        shell,
        matched,
        input_json,
        request.cwd.as_path(),
        request.turn_id,
    )
    .await
}

pub(crate) fn preview_notification(
    handlers: &[ConfiguredHandler],
    request: &NotificationRequest,
) -> Vec<HookRunSummary> {
    preview(
        handlers,
        HookEventName::Notification,
        request.event.as_str(),
    )
}

pub(crate) async fn run_notification(
    handlers: &[ConfiguredHandler],
    shell: &CommandShell,
    request: NotificationRequest,
) -> WorkflowHookOutcome {
    let matched = dispatcher::select_handlers(
        handlers,
        HookEventName::Notification,
        Some(request.event.as_str()),
    );
    if matched.is_empty() {
        return WorkflowHookOutcome::default();
    }

    let input_json = match notification_command_input_json(&request) {
        Ok(input_json) => input_json,
        Err(error) => {
            return serialization_failure_outcome(common::serialization_failure_hook_events(
                matched,
                Some(request.turn_id),
                format!("failed to serialize notification hook input: {error}"),
            ));
        }
    };

    execute_stateless_handlers(
        shell,
        matched,
        input_json,
        request.cwd.as_path(),
        request.turn_id,
    )
    .await
}

fn preview(
    handlers: &[ConfiguredHandler],
    event_name: HookEventName,
    matcher_input: &str,
) -> Vec<HookRunSummary> {
    dispatcher::select_handlers(handlers, event_name, Some(matcher_input))
        .into_iter()
        .map(|handler| dispatcher::running_summary(&handler))
        .collect()
}

async fn execute_stateless_handlers(
    shell: &CommandShell,
    matched: Vec<ConfiguredHandler>,
    input_json: String,
    cwd: &std::path::Path,
    turn_id: String,
) -> WorkflowHookOutcome {
    let results = dispatcher::execute_handlers(
        shell,
        matched,
        input_json,
        cwd,
        Some(turn_id),
        parse_completed,
    )
    .await;

    WorkflowHookOutcome {
        hook_events: results.into_iter().map(|result| result.completed).collect(),
    }
}

fn task_created_command_input_json(
    request: &TaskCreatedRequest,
) -> Result<String, serde_json::Error> {
    let subagent = SubagentCommandInputFields::from(request.subagent.as_ref());
    serde_json::to_string(&TaskCreatedCommandInput {
        session_id: request.session_id.to_string(),
        turn_id: request.turn_id.clone(),
        agent_id: subagent.agent_id,
        agent_type: subagent.agent_type,
        transcript_path: NullableString::from_path(request.transcript_path.clone()),
        cwd: request.cwd.display().to_string(),
        hook_event_name: "TaskCreated".to_string(),
        model: request.model.clone(),
        permission_mode: request.permission_mode.clone(),
        workflow_name: request.workflow_name.clone(),
        task_id: request.task_id.clone(),
        task_subject: request.task_subject.clone(),
        task_description: request.task_description.clone(),
        cell_id: request.cell_id.clone(),
    })
}

fn task_completed_command_input_json(
    request: &TaskCompletedRequest,
) -> Result<String, serde_json::Error> {
    let subagent = SubagentCommandInputFields::from(request.subagent.as_ref());
    serde_json::to_string(&TaskCompletedCommandInput {
        session_id: request.session_id.to_string(),
        turn_id: request.turn_id.clone(),
        agent_id: subagent.agent_id,
        agent_type: subagent.agent_type,
        transcript_path: NullableString::from_path(request.transcript_path.clone()),
        cwd: request.cwd.display().to_string(),
        hook_event_name: "TaskCompleted".to_string(),
        model: request.model.clone(),
        permission_mode: request.permission_mode.clone(),
        workflow_name: request.workflow_name.clone(),
        task_id: request.task_id.clone(),
        task_subject: request.task_subject.clone(),
        task_description: request.task_description.clone(),
        cell_id: request.cell_id.clone(),
        status: request.status.clone(),
    })
}

fn notification_command_input_json(
    request: &NotificationRequest,
) -> Result<String, serde_json::Error> {
    let subagent = SubagentCommandInputFields::from(request.subagent.as_ref());
    serde_json::to_string(&NotificationCommandInput {
        session_id: request.session_id.to_string(),
        turn_id: request.turn_id.clone(),
        agent_id: subagent.agent_id,
        agent_type: subagent.agent_type,
        transcript_path: NullableString::from_path(request.transcript_path.clone()),
        cwd: request.cwd.display().to_string(),
        hook_event_name: "Notification".to_string(),
        model: request.model.clone(),
        permission_mode: request.permission_mode.clone(),
        notification_type: "workflow_progress".to_string(),
        run_id: request.run_id.clone(),
        cell_id: request.cell_id.clone(),
        event: request.event.clone(),
        unix_ms: request.unix_ms,
        workflow: request.workflow.clone(),
        phase: request.phase.clone(),
        agent: request.agent.clone(),
        agent_id_value: request.agent_id.clone(),
        child: request.child.clone(),
        child_index: request.child_index,
        child_run_id: request.child_run_id.clone(),
        item_index: request.item_index,
        stage_index: request.stage_index,
        step_index: request.step_index,
        error: request.error.clone(),
        message: request.message.clone(),
        data: request.data.clone(),
    })
}

fn parse_completed(
    handler: &ConfiguredHandler,
    run_result: CommandRunResult,
    turn_id: Option<String>,
) -> dispatcher::ParsedHandler<WorkflowHandlerData> {
    let mut entries = Vec::new();
    let mut status = HookRunStatus::Completed;

    match run_result.error.as_deref() {
        Some(error) => {
            status = HookRunStatus::Failed;
            entries.push(HookOutputEntry {
                kind: HookOutputEntryKind::Error,
                text: error.to_string(),
            });
        }
        None => match run_result.exit_code {
            Some(0) => {
                let trimmed_stdout = run_result.stdout.trim();
                if trimmed_stdout.is_empty() {
                } else if let Some(parsed) = match handler.event_name {
                    HookEventName::TaskCreated => {
                        output_parser::parse_task_created(&run_result.stdout)
                    }
                    HookEventName::TaskCompleted => {
                        output_parser::parse_task_completed(&run_result.stdout)
                    }
                    HookEventName::Notification => {
                        output_parser::parse_notification(&run_result.stdout)
                    }
                    event_name => {
                        panic!("expected workflow hook event, got {event_name:?}");
                    }
                } {
                    if let Some(system_message) = parsed.universal.system_message {
                        entries.push(HookOutputEntry {
                            kind: HookOutputEntryKind::Warning,
                            text: system_message,
                        });
                    }
                    let _ = parsed.universal.continue_processing;
                    let _ = parsed.universal.stop_reason;
                    let _ = parsed.universal.suppress_output;
                    if let Some(invalid_reason) = parsed.invalid_reason {
                        status = HookRunStatus::Failed;
                        entries.push(HookOutputEntry {
                            kind: HookOutputEntryKind::Error,
                            text: invalid_reason,
                        });
                    }
                } else if output_parser::looks_like_json(&run_result.stdout) {
                    status = HookRunStatus::Failed;
                    entries.push(HookOutputEntry {
                        kind: HookOutputEntryKind::Error,
                        text: match handler.event_name {
                            HookEventName::TaskCreated => {
                                "hook returned invalid TaskCreated hook JSON output"
                            }
                            HookEventName::TaskCompleted => {
                                "hook returned invalid TaskCompleted hook JSON output"
                            }
                            HookEventName::Notification => {
                                "hook returned invalid Notification hook JSON output"
                            }
                            _ => unreachable!("validated workflow hook event"),
                        }
                        .to_string(),
                    });
                }
            }
            Some(code) => {
                status = HookRunStatus::Failed;
                entries.push(HookOutputEntry {
                    kind: HookOutputEntryKind::Error,
                    text: common::trimmed_non_empty(&run_result.stderr)
                        .unwrap_or_else(|| format!("hook exited with code {code}")),
                });
            }
            None => {
                status = HookRunStatus::Failed;
                entries.push(HookOutputEntry {
                    kind: HookOutputEntryKind::Error,
                    text: "hook process terminated without an exit code".to_string(),
                });
            }
        },
    }

    let completed = HookCompletedEvent {
        turn_id,
        run: dispatcher::completed_summary(handler, &run_result, status, entries),
    };

    dispatcher::ParsedHandler {
        completed,
        data: WorkflowHandlerData,
        completion_order: 0,
    }
}

fn serialization_failure_outcome(hook_events: Vec<HookCompletedEvent>) -> WorkflowHookOutcome {
    WorkflowHookOutcome { hook_events }
}
