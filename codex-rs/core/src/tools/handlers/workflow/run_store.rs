use crate::session::turn_context::TurnContext;
use crate::tools::context::FunctionToolOutput;
use codex_protocol::models::function_call_output_content_items_to_text;
use serde::Serialize;
use serde_json::Value as JsonValue;
use std::path::Path;
use std::path::PathBuf;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use super::WorkflowArgs;
use super::WorkflowSource;
use super::WorkflowSourceKind;
use super::metadata::ValidatedWorkflowScript;
use super::metadata::WorkflowPhaseMetadata;

const WORKFLOW_RUNS_DIR: &str = "workflow-runs";
const WORKFLOW_ACTIVE_RUNS_DIR: &str = "active";
const WORKFLOW_TRANSCRIPT_RUN_FILE: &str = "run.json";
const WORKFLOW_TRANSCRIPT_OUTPUT_FILE: &str = "output.txt";
const WORKFLOW_TRANSCRIPT_ERROR_FILE: &str = "error.txt";
const WORKFLOW_OUTPUT_PREVIEW_MAX_CHARS: usize = 4000;

#[derive(Debug, Serialize)]
pub(super) struct WorkflowRunSnapshot {
    pub(super) schema_version: u8,
    pub(super) run_id: String,
    pub(super) session_id: Option<String>,
    pub(super) thread_id: Option<String>,
    pub(super) workflow_tool_call_id: Option<String>,
    pub(super) workflow_name: String,
    pub(super) metadata_name: String,
    pub(super) description: String,
    pub(super) when_to_use: Option<String>,
    pub(super) input_schema: Option<String>,
    pub(super) phases: Vec<WorkflowPhaseMetadata>,
    pub(super) status: WorkflowRunStatus,
    pub(super) status_history: Vec<WorkflowRunStatusEvent>,
    pub(super) progress: Vec<WorkflowProgressEvent>,
    pub(super) cell_id: Option<String>,
    pub(super) cwd: Option<String>,
    pub(super) git_branch: Option<String>,
    pub(super) run_dir: Option<String>,
    pub(super) script_path: Option<String>,
    pub(super) transcript_dir: Option<String>,
    pub(super) resume_from_run_id: Option<String>,
    pub(super) script_hash: String,
    pub(super) source: WorkflowRunSourceSnapshot,
    pub(super) args: Option<JsonValue>,
    pub(super) max_output_tokens: Option<usize>,
    pub(super) started_unix_ms: u128,
    pub(super) ended_unix_ms: u128,
    pub(super) duration_ms: u128,
    pub(super) output_preview: Option<String>,
    pub(super) error: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub(super) struct WorkflowRunIdentity {
    pub(super) session_id: Option<String>,
    pub(super) thread_id: Option<String>,
    pub(super) workflow_tool_call_id: Option<String>,
    pub(super) cwd: Option<String>,
    pub(super) git_branch: Option<String>,
}

#[derive(Debug, Serialize)]
pub(super) struct WorkflowRunStatusEvent {
    pub(super) event: String,
    pub(super) status: Option<WorkflowRunStatus>,
    pub(super) unix_ms: u128,
    pub(super) message: Option<String>,
}

#[derive(Debug, Serialize)]
pub(super) struct WorkflowProgressEvent {
    pub(super) event: String,
    pub(super) unix_ms: u128,
    pub(super) workflow: Option<String>,
    pub(super) phase: Option<String>,
    pub(super) agent: Option<String>,
    pub(super) child: Option<String>,
    pub(super) message: Option<String>,
}

#[derive(Debug, Serialize)]
pub(super) struct WorkflowRunSourceSnapshot {
    pub(super) kind: WorkflowSourceKind,
    pub(super) name: String,
    pub(super) path: Option<String>,
}

#[derive(Clone, Debug)]
pub(super) struct WorkflowRunArtifacts {
    pub(super) run_dir: PathBuf,
    pub(super) script_path: Option<PathBuf>,
    pub(super) transcript_dir: PathBuf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum WorkflowRunStatus {
    Running,
    #[allow(dead_code)]
    Paused,
    Completed,
    Failed,
    Terminated,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct WorkflowResumeCacheHit {
    pub(super) run_id: String,
    pub(super) output_preview: String,
}

pub(super) fn workflow_run_snapshot(
    run_id: &str,
    workflow_name: &str,
    workflow_source: &WorkflowSource,
    validated: &ValidatedWorkflowScript,
    args: &WorkflowArgs,
    max_output_tokens: Option<usize>,
    identity: Option<&WorkflowRunIdentity>,
    artifacts: Option<&WorkflowRunArtifacts>,
    status: WorkflowRunStatus,
    started_unix_ms: u128,
    cell_id: Option<&str>,
    output_preview: Option<&str>,
    error: Option<&str>,
) -> WorkflowRunSnapshot {
    let ended_unix_ms = unix_time_millis();
    let output_preview = output_preview.map(truncate_workflow_preview);
    let error = error.map(truncate_workflow_preview);
    let status_history = workflow_initial_status_history(
        started_unix_ms,
        ended_unix_ms,
        status,
        output_preview.as_deref(),
        error.as_deref(),
    );
    WorkflowRunSnapshot {
        schema_version: 1,
        run_id: run_id.to_string(),
        session_id: identity.and_then(|identity| identity.session_id.clone()),
        thread_id: identity.and_then(|identity| identity.thread_id.clone()),
        workflow_tool_call_id: identity.and_then(|identity| identity.workflow_tool_call_id.clone()),
        workflow_name: workflow_name.to_string(),
        metadata_name: validated.metadata.name.clone(),
        description: args
            .description
            .clone()
            .unwrap_or_else(|| validated.metadata.description.clone()),
        when_to_use: validated.metadata.when_to_use.clone(),
        input_schema: validated.metadata.input_schema.clone(),
        phases: validated.metadata.phases.clone(),
        status,
        status_history,
        progress: Vec::new(),
        cell_id: cell_id.map(ToString::to_string),
        cwd: identity.and_then(|identity| identity.cwd.clone()),
        git_branch: identity.and_then(|identity| identity.git_branch.clone()),
        run_dir: artifacts.map(|artifacts| artifacts.run_dir.to_string_lossy().into_owned()),
        script_path: artifacts
            .and_then(|artifacts| artifacts.script_path.as_ref())
            .map(|path| path.to_string_lossy().into_owned()),
        transcript_dir: artifacts
            .map(|artifacts| artifacts.transcript_dir.to_string_lossy().into_owned()),
        resume_from_run_id: args
            .resume_from_run_id
            .as_deref()
            .map(str::trim)
            .filter(|run_id| !run_id.is_empty())
            .map(ToString::to_string),
        script_hash: workflow_script_hash(&workflow_source.code),
        source: WorkflowRunSourceSnapshot {
            kind: workflow_source.kind,
            name: workflow_source.name.clone(),
            path: workflow_source
                .path
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned()),
        },
        args: args.args.clone(),
        max_output_tokens,
        started_unix_ms,
        ended_unix_ms,
        duration_ms: ended_unix_ms.saturating_sub(started_unix_ms),
        output_preview,
        error,
    }
}

fn workflow_initial_status_history(
    started_unix_ms: u128,
    ended_unix_ms: u128,
    status: WorkflowRunStatus,
    output_preview: Option<&str>,
    error: Option<&str>,
) -> Vec<WorkflowRunStatusEvent> {
    let mut history = vec![WorkflowRunStatusEvent {
        event: "started".to_string(),
        status: None,
        unix_ms: started_unix_ms,
        message: None,
    }];
    let message = error
        .or(output_preview)
        .map(str::trim)
        .filter(|message| !message.is_empty())
        .map(truncate_workflow_preview);
    history.push(WorkflowRunStatusEvent {
        event: workflow_status_event_name(status).to_string(),
        status: Some(status),
        unix_ms: ended_unix_ms,
        message,
    });
    history
}

fn workflow_status_event_name(status: WorkflowRunStatus) -> &'static str {
    match status {
        WorkflowRunStatus::Running => "running",
        WorkflowRunStatus::Paused => "paused",
        WorkflowRunStatus::Completed => "completed",
        WorkflowRunStatus::Failed => "failed",
        WorkflowRunStatus::Terminated => "terminated",
    }
}

fn workflow_script_hash(source: &str) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in source.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("fnv1a64:{hash:016x}")
}

pub(super) async fn workflow_resume_cache_hit(
    turn: &TurnContext,
    args: &WorkflowArgs,
    workflow_source: &WorkflowSource,
) -> Option<WorkflowResumeCacheHit> {
    workflow_resume_cache_hit_from_dir(
        workflow_run_snapshot_dir(turn).as_path(),
        args,
        workflow_source,
    )
    .await
}

#[cfg_attr(not(test), allow(dead_code))]
async fn workflow_resume_cache_hit_from_dir(
    snapshot_dir: &Path,
    args: &WorkflowArgs,
    workflow_source: &WorkflowSource,
) -> Option<WorkflowResumeCacheHit> {
    let resume_run_id = normalized_resume_run_id(args)?;
    let snapshot_path = snapshot_dir.join(format!("{resume_run_id}.json"));
    let contents = tokio::fs::read_to_string(snapshot_path).await.ok()?;
    let snapshot = serde_json::from_str::<JsonValue>(&contents).ok()?;
    workflow_resume_cache_hit_from_snapshot(&resume_run_id, args, workflow_source, &snapshot)
}

fn workflow_resume_cache_hit_from_snapshot(
    resume_run_id: &str,
    args: &WorkflowArgs,
    workflow_source: &WorkflowSource,
    snapshot: &JsonValue,
) -> Option<WorkflowResumeCacheHit> {
    if snapshot.get("status").and_then(JsonValue::as_str) != Some("completed") {
        return None;
    }
    let current_hash = workflow_script_hash(&workflow_source.code);
    if snapshot.get("script_hash").and_then(JsonValue::as_str) != Some(current_hash.as_str()) {
        return None;
    }
    let current_args = args.args.clone().unwrap_or(JsonValue::Null);
    if snapshot.get("args").cloned().unwrap_or(JsonValue::Null) != current_args {
        return None;
    }

    Some(WorkflowResumeCacheHit {
        run_id: resume_run_id.to_string(),
        output_preview: snapshot
            .get("output_preview")
            .and_then(JsonValue::as_str)
            .unwrap_or_default()
            .to_string(),
    })
}

pub(super) fn normalized_resume_run_id(args: &WorkflowArgs) -> Option<String> {
    let run_id = args.resume_from_run_id.as_deref()?.trim();
    is_safe_workflow_run_id(run_id).then(|| run_id.to_string())
}

pub(super) fn is_safe_workflow_run_id(run_id: &str) -> bool {
    !run_id.is_empty()
        && run_id.len() <= 128
        && run_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

pub(super) fn workflow_run_artifacts(
    turn: &TurnContext,
    run_id: &str,
    workflow_source: &WorkflowSource,
) -> WorkflowRunArtifacts {
    let run_dir = workflow_run_snapshot_dir(turn).join(run_id);
    let transcript_dir = run_dir.join("transcripts");
    let script_path = match workflow_source.kind {
        WorkflowSourceKind::Inline => Some(run_dir.join("script.js")),
        WorkflowSourceKind::ScriptPath | WorkflowSourceKind::Named => workflow_source.path.clone(),
    };
    WorkflowRunArtifacts {
        run_dir,
        script_path,
        transcript_dir,
    }
}

pub(super) async fn persist_workflow_artifacts(
    source: &WorkflowSource,
    artifacts: &WorkflowRunArtifacts,
) {
    if let Err(err) = tokio::fs::create_dir_all(&artifacts.transcript_dir).await {
        tracing::warn!(
            run_dir = %artifacts.run_dir.display(),
            error = %err,
            "failed to create workflow transcript directory"
        );
        return;
    }
    if source.kind == WorkflowSourceKind::Inline
        && let Some(script_path) = artifacts.script_path.as_ref()
        && let Err(err) = tokio::fs::write(script_path, source.code.as_bytes()).await
    {
        tracing::warn!(
            script_path = %script_path.display(),
            error = %err,
            "failed to persist workflow script artifact"
        );
    }
}

async fn persist_workflow_snapshot(
    turn: &TurnContext,
    snapshot: &WorkflowRunSnapshot,
) -> Option<JsonValue> {
    let snapshot_dir = workflow_run_snapshot_dir(turn);
    match write_workflow_snapshot_with_payload(snapshot_dir.as_path(), snapshot).await {
        Ok((_path, payload)) => Some(payload),
        Err(err) => {
            tracing::warn!(
                run_id = %snapshot.run_id,
                error = %err,
                "failed to persist workflow run snapshot"
            );
            None
        }
    }
}

pub(super) async fn persist_workflow_run_state(
    turn: &TurnContext,
    snapshot: &WorkflowRunSnapshot,
    artifacts: Option<&WorkflowRunArtifacts>,
) {
    let snapshot_dir = workflow_run_snapshot_dir(turn);
    let persisted_payload = persist_workflow_snapshot(turn, snapshot).await;
    if let Some(artifacts) = artifacts {
        persist_workflow_transcript(snapshot, persisted_payload.as_ref(), artifacts).await;
    }
    if let Err(err) = persist_workflow_claude_compatibility_layout(
        snapshot_dir.as_path(),
        snapshot,
        persisted_payload.as_ref(),
        artifacts,
    )
    .await
    {
        tracing::warn!(
            run_id = %snapshot.run_id,
            error = %err,
            "failed to persist Claude-compatible workflow layout"
        );
    }
    if let Err(err) = sync_workflow_active_run_marker(snapshot_dir.as_path(), snapshot).await {
        tracing::warn!(
            run_id = %snapshot.run_id,
            error = %err,
            "failed to sync workflow active-run marker"
        );
    }
}

async fn persist_workflow_claude_compatibility_layout(
    snapshot_dir: &Path,
    snapshot: &WorkflowRunSnapshot,
    persisted_payload: Option<&JsonValue>,
    artifacts: Option<&WorkflowRunArtifacts>,
) -> Result<(), String> {
    if artifacts.is_none() && snapshot.run_dir.is_none() && snapshot.transcript_dir.is_none() {
        return Ok(());
    }

    let Some(session_id) = snapshot
        .session_id
        .as_deref()
        .and_then(workflow_safe_path_segment)
    else {
        return Ok(());
    };
    let Some(run_id) = workflow_safe_path_segment(snapshot.run_id.as_str()) else {
        return Ok(());
    };

    let session_dir = snapshot_dir.join(session_id);
    let workflows_dir = session_dir.join("workflows");
    let sidechain_dir = session_dir
        .join("subagents")
        .join("workflows")
        .join(run_id.as_str());
    tokio::fs::create_dir_all(&workflows_dir)
        .await
        .map_err(|err| {
            format!(
                "failed to create Claude-compatible workflows directory {}: {err}",
                workflows_dir.display()
            )
        })?;
    tokio::fs::create_dir_all(&sidechain_dir)
        .await
        .map_err(|err| {
            format!(
                "failed to create Claude-compatible workflow sidechain directory {}: {err}",
                sidechain_dir.display()
            )
        })?;

    let mut payload = if let Some(payload) = persisted_payload {
        payload.clone()
    } else {
        serde_json::to_value(snapshot)
            .map_err(|err| format!("failed to serialize workflow compatibility snapshot: {err}"))?
    };
    if let Some(object) = payload.as_object_mut() {
        object.insert(
            "run_dir".to_string(),
            JsonValue::String(sidechain_dir.to_string_lossy().into_owned()),
        );
        object.insert(
            "transcript_dir".to_string(),
            JsonValue::String(sidechain_dir.to_string_lossy().into_owned()),
        );
    }

    if let Some(compat_script_path) =
        persist_workflow_claude_compatibility_script(&workflows_dir, snapshot, artifacts).await?
        && let Some(object) = payload.as_object_mut()
    {
        object.insert(
            "script_path".to_string(),
            JsonValue::String(compat_script_path.to_string_lossy().into_owned()),
        );
    }

    let payload_text = serde_json::to_string_pretty(&payload)
        .map_err(|err| format!("failed to serialize workflow compatibility snapshot: {err}"))?;
    let snapshot_path = workflows_dir.join(format!("{run_id}.json"));
    tokio::fs::write(&snapshot_path, format!("{payload_text}\n"))
        .await
        .map_err(|err| {
            format!(
                "failed to write Claude-compatible workflow snapshot {}: {err}",
                snapshot_path.display()
            )
        })?;
    write_workflow_transcript_value(sidechain_dir.as_path(), &payload).await?;
    Ok(())
}

async fn persist_workflow_claude_compatibility_script(
    workflows_dir: &Path,
    snapshot: &WorkflowRunSnapshot,
    artifacts: Option<&WorkflowRunArtifacts>,
) -> Result<Option<PathBuf>, String> {
    let Some(source_path) = artifacts
        .and_then(|artifacts| artifacts.script_path.as_ref())
        .map(PathBuf::as_path)
        .or_else(|| snapshot.script_path.as_deref().map(Path::new))
    else {
        return Ok(None);
    };
    let Ok(source) = tokio::fs::read(source_path).await else {
        return Ok(None);
    };

    let scripts_dir = workflows_dir.join("scripts");
    tokio::fs::create_dir_all(&scripts_dir)
        .await
        .map_err(|err| {
            format!(
                "failed to create Claude-compatible workflow scripts directory {}: {err}",
                scripts_dir.display()
            )
        })?;
    let slug = workflow_safe_filename_slug(snapshot.workflow_name.as_str())
        .unwrap_or_else(|| "workflow".to_string());
    let run_id =
        workflow_safe_filename_slug(snapshot.run_id.as_str()).unwrap_or_else(|| "run".to_string());
    let compat_script_path = scripts_dir.join(format!("{slug}-{run_id}.js"));
    tokio::fs::write(&compat_script_path, source)
        .await
        .map_err(|err| {
            format!(
                "failed to write Claude-compatible workflow script {}: {err}",
                compat_script_path.display()
            )
        })?;
    Ok(Some(compat_script_path))
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

fn workflow_safe_filename_slug(value: &str) -> Option<String> {
    let slug = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else if matches!(ch, '_' | '-') {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .trim_matches('_')
        .chars()
        .take(64)
        .collect::<String>();
    if slug.is_empty() || slug == "." || slug == ".." {
        None
    } else {
        Some(slug)
    }
}

async fn persist_workflow_transcript(
    snapshot: &WorkflowRunSnapshot,
    persisted_payload: Option<&JsonValue>,
    artifacts: &WorkflowRunArtifacts,
) {
    let result = if let Some(payload) = persisted_payload {
        write_workflow_transcript_value(artifacts.transcript_dir.as_path(), payload).await
    } else {
        write_workflow_transcript(artifacts.transcript_dir.as_path(), snapshot).await
    };
    if let Err(err) = result {
        tracing::warn!(
            run_id = %snapshot.run_id,
            transcript_dir = %artifacts.transcript_dir.display(),
            error = %err,
            "failed to persist workflow transcript"
        );
    }
}

pub(super) fn workflow_run_snapshot_dir(turn: &TurnContext) -> PathBuf {
    turn.config.codex_home.join(WORKFLOW_RUNS_DIR).to_path_buf()
}

#[cfg(test)]
async fn write_workflow_snapshot(
    snapshot_dir: &Path,
    snapshot: &WorkflowRunSnapshot,
) -> Result<PathBuf, String> {
    write_workflow_snapshot_with_payload(snapshot_dir, snapshot)
        .await
        .map(|(path, _payload)| path)
}

async fn write_workflow_snapshot_with_payload(
    snapshot_dir: &Path,
    snapshot: &WorkflowRunSnapshot,
) -> Result<(PathBuf, JsonValue), String> {
    tokio::fs::create_dir_all(snapshot_dir)
        .await
        .map_err(|err| {
            format!(
                "failed to create workflow snapshot directory {}: {err}",
                snapshot_dir.display()
            )
        })?;
    let snapshot_path = snapshot_dir.join(format!("{}.json", snapshot.run_id));
    let mut payload = serde_json::to_value(snapshot)
        .map_err(|err| format!("failed to serialize workflow snapshot: {err}"))?;
    merge_existing_workflow_snapshot_state(snapshot_path.as_path(), &mut payload).await;
    let payload_text = serde_json::to_string_pretty(&payload)
        .map_err(|err| format!("failed to serialize workflow snapshot: {err}"))?;
    tokio::fs::write(&snapshot_path, format!("{payload_text}\n"))
        .await
        .map_err(|err| {
            format!(
                "failed to write workflow snapshot {}: {err}",
                snapshot_path.display()
            )
        })?;
    Ok((snapshot_path, payload))
}

async fn merge_existing_workflow_snapshot_state(path: &Path, payload: &mut JsonValue) {
    let Ok(contents) = tokio::fs::read_to_string(path).await else {
        return;
    };
    let Ok(existing) = serde_json::from_str::<JsonValue>(&contents) else {
        return;
    };
    merge_existing_workflow_progress(&existing, payload);
    merge_existing_workflow_updated_time(&existing, payload);
}

fn merge_existing_workflow_progress(existing: &JsonValue, payload: &mut JsonValue) {
    let Some(existing_progress) = existing.get("progress").and_then(JsonValue::as_array) else {
        return;
    };
    if existing_progress.is_empty() {
        return;
    }
    let Some(object) = payload.as_object_mut() else {
        return;
    };
    let payload_progress_is_empty = object
        .get("progress")
        .and_then(JsonValue::as_array)
        .is_none_or(Vec::is_empty);
    if payload_progress_is_empty {
        object.insert(
            "progress".to_string(),
            JsonValue::Array(existing_progress.clone()),
        );
    }
}

fn merge_existing_workflow_updated_time(existing: &JsonValue, payload: &mut JsonValue) {
    let existing_updated = existing.get("updated_unix_ms").and_then(JsonValue::as_u64);
    let payload_ended = payload
        .get("ended_unix_ms")
        .and_then(JsonValue::as_u64)
        .filter(|ended| *ended > 0);
    let updated = match (existing_updated, payload_ended) {
        (Some(existing_updated), Some(payload_ended)) => Some(existing_updated.max(payload_ended)),
        (Some(existing_updated), None) => Some(existing_updated),
        (None, Some(payload_ended)) => Some(payload_ended),
        (None, None) => None,
    };
    let Some(updated) = updated else {
        return;
    };
    if let Some(object) = payload.as_object_mut() {
        object.insert("updated_unix_ms".to_string(), JsonValue::from(updated));
    }
}

async fn sync_workflow_active_run_marker(
    snapshot_dir: &Path,
    snapshot: &WorkflowRunSnapshot,
) -> Result<(), String> {
    let marker_path = workflow_active_run_marker_path(snapshot_dir, snapshot.run_id.as_str());
    if matches!(
        snapshot.status,
        WorkflowRunStatus::Running | WorkflowRunStatus::Paused
    ) {
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

fn workflow_active_run_marker_path(snapshot_dir: &Path, run_id: &str) -> PathBuf {
    snapshot_dir
        .join(WORKFLOW_ACTIVE_RUNS_DIR)
        .join(format!("{run_id}.json"))
}

async fn write_workflow_transcript(
    transcript_dir: &Path,
    snapshot: &WorkflowRunSnapshot,
) -> Result<(), String> {
    let payload = serde_json::to_value(snapshot)
        .map_err(|err| format!("failed to serialize workflow transcript: {err}"))?;
    write_workflow_transcript_value(transcript_dir, &payload).await
}

async fn write_workflow_transcript_value(
    transcript_dir: &Path,
    payload: &JsonValue,
) -> Result<(), String> {
    tokio::fs::create_dir_all(transcript_dir)
        .await
        .map_err(|err| {
            format!(
                "failed to create workflow transcript directory {}: {err}",
                transcript_dir.display()
            )
        })?;
    let payload_text = serde_json::to_string_pretty(payload)
        .map_err(|err| format!("failed to serialize workflow transcript: {err}"))?;
    tokio::fs::write(
        transcript_dir.join(WORKFLOW_TRANSCRIPT_RUN_FILE),
        format!("{payload_text}\n"),
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
        payload.get("output_preview").and_then(JsonValue::as_str),
    )
    .await?;
    write_optional_transcript_text(
        transcript_dir
            .join(WORKFLOW_TRANSCRIPT_ERROR_FILE)
            .as_path(),
        payload.get("error").and_then(JsonValue::as_str),
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

pub(super) fn workflow_output_preview(output: &FunctionToolOutput) -> String {
    let text = function_call_output_content_items_to_text(&output.body).unwrap_or_default();
    truncate_workflow_preview(&text)
}

fn truncate_workflow_preview(text: &str) -> String {
    if text.chars().count() <= WORKFLOW_OUTPUT_PREVIEW_MAX_CHARS {
        return text.to_string();
    }
    let mut truncated = text
        .chars()
        .take(WORKFLOW_OUTPUT_PREVIEW_MAX_CHARS)
        .collect::<String>();
    truncated.push_str("\n[truncated]");
    truncated
}

pub(super) fn unix_time_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[cfg(test)]
#[path = "run_store_tests.rs"]
mod tests;
