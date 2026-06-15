use super::*;
use crate::tools::context::FunctionToolOutput;
use serde_json::Value as JsonValue;
use std::path::PathBuf;

use super::super::WorkflowArgs;
use super::super::WorkflowSource;
use super::super::WorkflowSourceKind;
use super::super::metadata::ValidatedWorkflowScript;
use super::super::metadata::WorkflowMetadata;
use super::super::metadata::WorkflowPhaseMetadata;

#[test]
fn workflow_snapshot_records_failed_script_output() {
    let args = WorkflowArgs {
        name: Some("release".to_string()),
        script: None,
        script_path: None,
        args: Some(serde_json::json!({ "channel": "alpha" })),
        resume_from_run_id: None,
        title: None,
        description: Some("Release channel workflow".to_string()),
        max_output_tokens: None,
    };
    let source = WorkflowSource {
        name: "release".to_string(),
        code: String::new(),
        kind: WorkflowSourceKind::Named,
        path: Some(PathBuf::from("release.js")),
    };
    let validated = ValidatedWorkflowScript {
        metadata: WorkflowMetadata {
            name: "release-meta".to_string(),
            description: "Metadata description".to_string(),
            when_to_use: Some("Use for release channels".to_string()),
            input_schema: Some(
                "{ type: 'object', properties: { channel: { type: 'string' } } }".to_string(),
            ),
            phases: vec![WorkflowPhaseMetadata {
                title: "Build".to_string(),
                model: Some("xhigh".to_string()),
            }],
        },
        body: "throw new Error('boom')".to_string(),
    };
    let snapshot = workflow_run_snapshot(
        "wf_failed",
        "release",
        &source,
        &validated,
        &args,
        Some(2048),
        Some(&WorkflowRunIdentity {
            session_id: Some("session-7".to_string()),
            thread_id: Some("thread-7".to_string()),
            workflow_tool_call_id: Some("call-7".to_string()),
            cwd: Some("/tmp/project".to_string()),
            git_branch: Some("feature/workflows".to_string()),
        }),
        Some(&WorkflowRunArtifacts {
            run_dir: PathBuf::from("/tmp/workflows/wf_failed"),
            script_path: Some(PathBuf::from("release.js")),
            transcript_dir: PathBuf::from("/tmp/workflows/wf_failed/transcripts"),
        }),
        WorkflowRunStatus::Failed,
        100,
        Some("cell-7"),
        Some("Script failed\nScript error:\nboom"),
        Some("Script failed\nScript error:\nboom"),
    );
    let value = serde_json::to_value(snapshot).expect("serialize snapshot");

    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["run_id"], "wf_failed");
    assert_eq!(value["session_id"], "session-7");
    assert_eq!(value["thread_id"], "thread-7");
    assert_eq!(value["workflow_tool_call_id"], "call-7");
    assert_eq!(value["workflow_name"], "release");
    assert_eq!(value["metadata_name"], "release-meta");
    assert_eq!(value["description"], "Release channel workflow");
    assert_eq!(value["when_to_use"], "Use for release channels");
    assert_eq!(
        value["input_schema"],
        "{ type: 'object', properties: { channel: { type: 'string' } } }"
    );
    assert_eq!(value["phases"][0]["title"], "Build");
    assert_eq!(value["phases"][0]["model"], "xhigh");
    assert_eq!(value["status"], "failed");
    assert_eq!(value["cell_id"], "cell-7");
    assert_eq!(value["cwd"], "/tmp/project");
    assert_eq!(value["git_branch"], "feature/workflows");
    assert_eq!(value["run_dir"], "/tmp/workflows/wf_failed");
    assert_eq!(value["script_path"], "release.js");
    assert_eq!(
        value["transcript_dir"],
        "/tmp/workflows/wf_failed/transcripts"
    );
    assert_eq!(value["resume_from_run_id"], JsonValue::Null);
    assert_eq!(value["script_hash"], workflow_script_hash(&source.code));
    assert_eq!(value["source"]["kind"], "named");
    assert_eq!(value["source"]["name"], "release");
    assert_eq!(value["source"]["path"], "release.js");
    assert_eq!(value["args"]["channel"], "alpha");
    assert_eq!(value["max_output_tokens"], 2048);
    assert_eq!(value["started_unix_ms"], 100);
    assert!(value["ended_unix_ms"].as_u64().unwrap_or_default() >= 100);
    assert!(value["duration_ms"].as_u64().is_some());
    assert_eq!(value["status_history"][0]["event"], "started");
    assert_eq!(value["status_history"][0]["unix_ms"], 100);
    assert_eq!(value["status_history"][1]["event"], "failed");
    assert_eq!(value["status_history"][1]["status"], "failed");
    assert!(
        value["status_history"][1]["message"]
            .as_str()
            .expect("history message")
            .contains("Script error"),
        "{value:#}"
    );
    assert!(
        value["error"]
            .as_str()
            .expect("error")
            .contains("Script error"),
        "{value:#}"
    );
}

#[test]
fn workflow_output_preview_is_truncated() {
    let long_text = "x".repeat(WORKFLOW_OUTPUT_PREVIEW_MAX_CHARS + 20);
    let output = FunctionToolOutput::from_text(long_text, Some(true));
    let preview = workflow_output_preview(&output);

    assert!(preview.ends_with("\n[truncated]"));
    assert!(preview.len() < WORKFLOW_OUTPUT_PREVIEW_MAX_CHARS + 100);
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

#[tokio::test]
async fn workflow_resume_cache_hit_requires_safe_completed_matching_snapshot() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let source = WorkflowSource {
        name: "release".to_string(),
        code: "export const meta = { name: 'release', description: 'release' };\nreturn 'ok';"
            .to_string(),
        kind: WorkflowSourceKind::Named,
        path: Some(PathBuf::from("release.js")),
    };
    let args = workflow_args_with_resume("wf_prev", serde_json::json!({ "channel": "alpha" }));
    tokio::fs::write(
        temp_dir.path().join("wf_prev.json"),
        serde_json::json!({
            "run_id": "wf_prev",
            "status": "completed",
            "script_hash": workflow_script_hash(&source.code),
            "args": { "channel": "alpha" },
            "output_preview": "cached output"
        })
        .to_string(),
    )
    .await
    .expect("write previous snapshot");

    let hit = workflow_resume_cache_hit_from_dir(temp_dir.path(), &args, &source)
        .await
        .expect("cache hit");
    assert_eq!(
        hit,
        WorkflowResumeCacheHit {
            run_id: "wf_prev".to_string(),
            output_preview: "cached output".to_string()
        }
    );

    let mismatched_args =
        workflow_args_with_resume("wf_prev", serde_json::json!({ "channel": "stable" }));
    assert!(
        workflow_resume_cache_hit_from_dir(temp_dir.path(), &mismatched_args, &source)
            .await
            .is_none()
    );
    let invalid_run_id =
        workflow_args_with_resume("../wf_prev", serde_json::json!({ "channel": "alpha" }));
    assert!(
        workflow_resume_cache_hit_from_dir(temp_dir.path(), &invalid_run_id, &source)
            .await
            .is_none()
    );
}

#[tokio::test]
async fn workflow_artifacts_persist_inline_script_and_transcript_dir() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let source = WorkflowSource {
        name: "inline".to_string(),
        code: "export const meta = { name: 'x', description: 'x' };\nreturn 'ok';".to_string(),
        kind: WorkflowSourceKind::Inline,
        path: None,
    };
    let artifacts = WorkflowRunArtifacts {
        run_dir: temp_dir.path().join("wf_artifact"),
        script_path: Some(temp_dir.path().join("wf_artifact").join("script.js")),
        transcript_dir: temp_dir.path().join("wf_artifact").join("transcripts"),
    };

    persist_workflow_artifacts(&source, &artifacts).await;

    let saved_script = tokio::fs::read_to_string(artifacts.script_path.as_ref().unwrap())
        .await
        .expect("read persisted script");
    assert!(saved_script.contains("return 'ok';"), "{saved_script}");
    assert!(artifacts.transcript_dir.is_dir());
}

#[tokio::test]
async fn workflow_claude_compatibility_layout_mirrors_snapshot_script_and_sidechain() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let script_path = temp_dir.path().join("wf_compat").join("script.js");
    tokio::fs::create_dir_all(script_path.parent().expect("script parent"))
        .await
        .expect("create script parent");
    tokio::fs::write(
        &script_path,
        "export const meta = { name: 'release', description: 'release' };\nreturn 'ok';",
    )
    .await
    .expect("write script");
    let artifacts = WorkflowRunArtifacts {
        run_dir: temp_dir.path().join("wf_compat"),
        script_path: Some(script_path),
        transcript_dir: temp_dir.path().join("wf_compat").join("transcripts"),
    };
    let snapshot = WorkflowRunSnapshot {
        schema_version: 1,
        run_id: "wf_compat".to_string(),
        session_id: Some("session-compat".to_string()),
        thread_id: Some("thread-compat".to_string()),
        workflow_tool_call_id: Some("call-compat".to_string()),
        workflow_name: "Release Workflow".to_string(),
        metadata_name: "release".to_string(),
        description: "Compat layout".to_string(),
        when_to_use: None,
        input_schema: None,
        phases: Vec::new(),
        status: WorkflowRunStatus::Completed,
        status_history: Vec::new(),
        progress: Vec::new(),
        cell_id: Some("cell-compat".to_string()),
        cwd: None,
        git_branch: None,
        run_dir: Some(artifacts.run_dir.display().to_string()),
        script_path: artifacts
            .script_path
            .as_ref()
            .map(|path| path.display().to_string()),
        transcript_dir: Some(artifacts.transcript_dir.display().to_string()),
        resume_from_run_id: None,
        script_hash: "fnv1a64:2222222222222222".to_string(),
        source: WorkflowRunSourceSnapshot {
            kind: WorkflowSourceKind::Inline,
            name: "inline".to_string(),
            path: None,
        },
        args: None,
        max_output_tokens: None,
        started_unix_ms: 10,
        ended_unix_ms: 12,
        duration_ms: 2,
        output_preview: Some("ok".to_string()),
        error: None,
    };

    persist_workflow_claude_compatibility_layout(
        temp_dir.path(),
        &snapshot,
        None,
        Some(&artifacts),
    )
    .await
    .expect("persist compatibility layout");

    let compat_snapshot = temp_dir
        .path()
        .join("session-compat")
        .join("workflows")
        .join("wf_compat.json");
    let saved = tokio::fs::read_to_string(&compat_snapshot)
        .await
        .expect("read compat snapshot");
    let saved: serde_json::Value = serde_json::from_str(&saved).expect("compat snapshot json");
    let sidechain_dir = temp_dir
        .path()
        .join("session-compat")
        .join("subagents")
        .join("workflows")
        .join("wf_compat");
    let compat_script = temp_dir
        .path()
        .join("session-compat")
        .join("workflows")
        .join("scripts")
        .join("release-workflow-wf_compat.js");

    assert_eq!(saved["run_id"], "wf_compat");
    assert_eq!(saved["run_dir"], sidechain_dir.display().to_string());
    assert_eq!(saved["transcript_dir"], sidechain_dir.display().to_string());
    assert_eq!(saved["script_path"], compat_script.display().to_string());
    assert!(compat_script.is_file());
    assert!(sidechain_dir.join(WORKFLOW_TRANSCRIPT_RUN_FILE).is_file());
}

#[tokio::test]
async fn workflow_transcript_persists_run_output_error_and_cleans_stale_files() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let transcript_dir = temp_dir.path().join("transcripts");
    let mut snapshot = WorkflowRunSnapshot {
        schema_version: 1,
        run_id: "wf_transcript".to_string(),
        session_id: None,
        thread_id: None,
        workflow_tool_call_id: None,
        workflow_name: "transcript".to_string(),
        metadata_name: "transcript-meta".to_string(),
        description: "Persist transcript".to_string(),
        when_to_use: None,
        input_schema: None,
        phases: Vec::new(),
        status: WorkflowRunStatus::Failed,
        status_history: Vec::new(),
        progress: Vec::new(),
        cell_id: None,
        cwd: None,
        git_branch: None,
        run_dir: Some(temp_dir.path().join("wf_transcript").display().to_string()),
        script_path: Some(
            temp_dir
                .path()
                .join("wf_transcript/script.js")
                .display()
                .to_string(),
        ),
        transcript_dir: Some(transcript_dir.display().to_string()),
        resume_from_run_id: None,
        script_hash: "fnv1a64:0000000000000000".to_string(),
        source: WorkflowRunSourceSnapshot {
            kind: WorkflowSourceKind::Inline,
            name: "inline".to_string(),
            path: None,
        },
        args: None,
        max_output_tokens: None,
        started_unix_ms: 10,
        ended_unix_ms: 12,
        duration_ms: 2,
        output_preview: Some("partial output".to_string()),
        error: Some("boom".to_string()),
    };

    write_workflow_transcript(transcript_dir.as_path(), &snapshot)
        .await
        .expect("write transcript");
    let run = tokio::fs::read_to_string(transcript_dir.join(WORKFLOW_TRANSCRIPT_RUN_FILE))
        .await
        .expect("read run transcript");
    let run_json: serde_json::Value = serde_json::from_str(&run).expect("run transcript json");
    assert_eq!(run_json["run_id"], "wf_transcript");
    assert_eq!(
        tokio::fs::read_to_string(transcript_dir.join(WORKFLOW_TRANSCRIPT_OUTPUT_FILE))
            .await
            .expect("read output transcript"),
        "partial output\n"
    );
    assert_eq!(
        tokio::fs::read_to_string(transcript_dir.join(WORKFLOW_TRANSCRIPT_ERROR_FILE))
            .await
            .expect("read error transcript"),
        "boom\n"
    );

    snapshot.status = WorkflowRunStatus::Completed;
    snapshot.output_preview = None;
    snapshot.error = None;
    write_workflow_transcript(transcript_dir.as_path(), &snapshot)
        .await
        .expect("rewrite transcript");
    assert!(
        !transcript_dir
            .join(WORKFLOW_TRANSCRIPT_OUTPUT_FILE)
            .exists()
    );
    assert!(!transcript_dir.join(WORKFLOW_TRANSCRIPT_ERROR_FILE).exists());
}

#[tokio::test]
async fn write_workflow_snapshot_persists_json_by_run_id() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let snapshot = WorkflowRunSnapshot {
        schema_version: 1,
        run_id: "wf_disk".to_string(),
        session_id: None,
        thread_id: None,
        workflow_tool_call_id: None,
        workflow_name: "disk".to_string(),
        metadata_name: "disk-meta".to_string(),
        description: "Persist snapshot".to_string(),
        when_to_use: None,
        input_schema: Some("{ type: 'object' }".to_string()),
        phases: Vec::new(),
        status: WorkflowRunStatus::Completed,
        status_history: vec![WorkflowRunStatusEvent {
            event: "completed".to_string(),
            status: Some(WorkflowRunStatus::Completed),
            unix_ms: 12,
            message: Some("ok".to_string()),
        }],
        progress: vec![WorkflowProgressEvent {
            event: "phase".to_string(),
            unix_ms: 11,
            workflow: Some("disk".to_string()),
            phase: Some("write".to_string()),
            agent: None,
            child: None,
            message: Some("writing".to_string()),
        }],
        cell_id: Some("cell-disk".to_string()),
        cwd: None,
        git_branch: None,
        run_dir: Some("/tmp/workflows/wf_disk".to_string()),
        script_path: Some("/tmp/workflows/wf_disk/script.js".to_string()),
        transcript_dir: Some("/tmp/workflows/wf_disk/transcripts".to_string()),
        resume_from_run_id: Some("wf_prev".to_string()),
        script_hash: "fnv1a64:1111111111111111".to_string(),
        source: WorkflowRunSourceSnapshot {
            kind: WorkflowSourceKind::Inline,
            name: "inline".to_string(),
            path: None,
        },
        args: None,
        max_output_tokens: Some(512),
        started_unix_ms: 10,
        ended_unix_ms: 12,
        duration_ms: 2,
        output_preview: Some("ok".to_string()),
        error: None,
    };

    let path = write_workflow_snapshot(temp_dir.path(), &snapshot)
        .await
        .expect("write snapshot");
    let saved = tokio::fs::read_to_string(path)
        .await
        .expect("read snapshot");
    let value: serde_json::Value = serde_json::from_str(&saved).expect("snapshot json");

    assert_eq!(value["run_id"], "wf_disk");
    assert_eq!(value["status"], "completed");
    assert_eq!(value["run_dir"], "/tmp/workflows/wf_disk");
    assert_eq!(value["script_path"], "/tmp/workflows/wf_disk/script.js");
    assert_eq!(
        value["transcript_dir"],
        "/tmp/workflows/wf_disk/transcripts"
    );
    assert_eq!(value["resume_from_run_id"], "wf_prev");
    assert_eq!(value["max_output_tokens"], 512);
    assert_eq!(value["script_hash"], "fnv1a64:1111111111111111");
    assert_eq!(value["input_schema"], "{ type: 'object' }");
    assert_eq!(value["output_preview"], "ok");
    assert_eq!(value["status_history"][0]["event"], "completed");
    assert_eq!(value["status_history"][0]["status"], "completed");
    assert_eq!(value["status_history"][0]["message"], "ok");
    assert_eq!(value["progress"][0]["event"], "phase");
    assert_eq!(value["progress"][0]["phase"], "write");
    assert!(temp_dir.path().join("wf_disk.json").is_file());
}

#[tokio::test]
async fn write_workflow_snapshot_preserves_existing_progress_and_updated_time() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let snapshot_path = temp_dir.path().join("wf_disk.json");
    let existing = serde_json::json!({
        "schema_version": 1,
        "run_id": "wf_disk",
        "workflow_name": "release",
        "status": "running",
        "status_history": [],
        "progress": [
            {
                "event": "phase",
                "unix_ms": 20,
                "workflow": "release",
                "phase": "build",
                "message": "artifact"
            }
        ],
        "updated_unix_ms": 20,
        "ended_unix_ms": 12
    });
    tokio::fs::write(
        snapshot_path.as_path(),
        format!(
            "{}\n",
            serde_json::to_string_pretty(&existing).expect("existing snapshot json")
        ),
    )
    .await
    .expect("write existing snapshot");

    let snapshot = WorkflowRunSnapshot {
        schema_version: 1,
        run_id: "wf_disk".to_string(),
        session_id: None,
        thread_id: None,
        workflow_tool_call_id: None,
        workflow_name: "release".to_string(),
        metadata_name: "release-meta".to_string(),
        description: "Persist snapshot".to_string(),
        when_to_use: None,
        input_schema: None,
        phases: Vec::new(),
        status: WorkflowRunStatus::Completed,
        status_history: vec![WorkflowRunStatusEvent {
            event: "completed".to_string(),
            status: Some(WorkflowRunStatus::Completed),
            unix_ms: 30,
            message: Some("ok".to_string()),
        }],
        progress: Vec::new(),
        cell_id: None,
        cwd: None,
        git_branch: None,
        run_dir: Some("/tmp/workflows/wf_disk".to_string()),
        script_path: Some("/tmp/workflows/wf_disk/script.js".to_string()),
        transcript_dir: Some("/tmp/workflows/wf_disk/transcripts".to_string()),
        resume_from_run_id: None,
        script_hash: "fnv1a64:1111111111111111".to_string(),
        source: WorkflowRunSourceSnapshot {
            kind: WorkflowSourceKind::Inline,
            name: "inline".to_string(),
            path: None,
        },
        args: None,
        max_output_tokens: None,
        started_unix_ms: 10,
        ended_unix_ms: 30,
        duration_ms: 20,
        output_preview: Some("ok".to_string()),
        error: None,
    };

    let (path, payload) = write_workflow_snapshot_with_payload(temp_dir.path(), &snapshot)
        .await
        .expect("write merged snapshot");
    assert_eq!(path, snapshot_path);

    let saved = tokio::fs::read_to_string(path)
        .await
        .expect("read merged snapshot");
    let value: serde_json::Value = serde_json::from_str(&saved).expect("merged snapshot json");
    assert_eq!(value["status"], "completed");
    assert_eq!(value["progress"][0]["event"], "phase");
    assert_eq!(value["progress"][0]["phase"], "build");
    assert_eq!(value["progress"][0]["message"], "artifact");
    assert_eq!(value["updated_unix_ms"], 30);
    assert_eq!(payload["progress"][0]["message"], "artifact");
    assert_eq!(payload["updated_unix_ms"], 30);

    let transcript_dir = temp_dir.path().join("transcripts");
    write_workflow_transcript_value(transcript_dir.as_path(), &payload)
        .await
        .expect("write transcript from merged payload");
    let run = tokio::fs::read_to_string(transcript_dir.join(WORKFLOW_TRANSCRIPT_RUN_FILE))
        .await
        .expect("read transcript run json");
    let run_json: serde_json::Value = serde_json::from_str(&run).expect("transcript json");
    assert_eq!(run_json["progress"][0]["message"], "artifact");
    assert_eq!(run_json["updated_unix_ms"], 30);
    assert_eq!(
        tokio::fs::read_to_string(transcript_dir.join(WORKFLOW_TRANSCRIPT_OUTPUT_FILE))
            .await
            .expect("read transcript output"),
        "ok\n"
    );
}

#[tokio::test]
async fn workflow_active_run_marker_tracks_running_status() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let mut snapshot = WorkflowRunSnapshot {
        schema_version: 1,
        run_id: "wf_active".to_string(),
        session_id: None,
        thread_id: None,
        workflow_tool_call_id: None,
        workflow_name: "active".to_string(),
        metadata_name: "active-meta".to_string(),
        description: "Track active run".to_string(),
        when_to_use: None,
        input_schema: None,
        phases: Vec::new(),
        status: WorkflowRunStatus::Running,
        status_history: Vec::new(),
        progress: Vec::new(),
        cell_id: Some("cell-active".to_string()),
        cwd: None,
        git_branch: None,
        run_dir: Some("/tmp/workflows/wf_active".to_string()),
        script_path: Some("/tmp/workflows/wf_active/script.js".to_string()),
        transcript_dir: Some("/tmp/workflows/wf_active/transcripts".to_string()),
        resume_from_run_id: None,
        script_hash: "fnv1a64:2222222222222222".to_string(),
        source: WorkflowRunSourceSnapshot {
            kind: WorkflowSourceKind::Inline,
            name: "inline".to_string(),
            path: None,
        },
        args: None,
        max_output_tokens: None,
        started_unix_ms: 10,
        ended_unix_ms: 12,
        duration_ms: 2,
        output_preview: None,
        error: None,
    };
    let marker_path = workflow_active_run_marker_path(temp_dir.path(), "wf_active");

    sync_workflow_active_run_marker(temp_dir.path(), &snapshot)
        .await
        .expect("write active marker");
    let saved = tokio::fs::read_to_string(&marker_path)
        .await
        .expect("read active marker");
    let value: serde_json::Value = serde_json::from_str(&saved).expect("active marker json");
    assert_eq!(value["run_id"], "wf_active");
    assert_eq!(value["status"], "running");
    assert_eq!(value["cell_id"], "cell-active");

    snapshot.status = WorkflowRunStatus::Completed;
    sync_workflow_active_run_marker(temp_dir.path(), &snapshot)
        .await
        .expect("remove active marker");
    assert!(!marker_path.exists());
}
