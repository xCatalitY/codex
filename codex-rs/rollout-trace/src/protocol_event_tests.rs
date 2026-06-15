use codex_protocol::AgentPath;
use codex_protocol::ThreadId;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::SubAgentActivityEvent;
use codex_protocol::protocol::SubAgentActivityKind;
use codex_protocol::protocol::WorkflowProgressEvent;
use pretty_assertions::assert_eq;
use serde_json::json;

use super::ToolRuntimeTraceEvent;
use super::codex_turn_trace_event;
use super::tool_runtime_trace_event;
use crate::ExecutionStatus;
use crate::RawTraceEventPayload;

#[test]
fn sub_agent_activity_is_a_terminal_tool_runtime_event() -> anyhow::Result<()> {
    let agent_thread_id = ThreadId::new();
    let event = EventMsg::SubAgentActivity(SubAgentActivityEvent {
        event_id: "call-spawn".to_string(),
        occurred_at_ms: 1234,
        agent_thread_id,
        agent_path: AgentPath::try_from("/root/reviewer").map_err(anyhow::Error::msg)?,
        kind: SubAgentActivityKind::Started,
    });

    let Some(ToolRuntimeTraceEvent::Ended {
        tool_call_id,
        status,
        payload,
    }) = tool_runtime_trace_event(&event)
    else {
        panic!("expected terminal tool runtime event");
    };

    assert_eq!(tool_call_id, "call-spawn");
    assert_eq!(status, ExecutionStatus::Completed);
    assert_eq!(
        serde_json::to_value(payload)?,
        json!({
            "event_id": "call-spawn",
            "occurred_at_ms": 1234,
            "agent_thread_id": agent_thread_id,
            "agent_path": "/root/reviewer",
            "kind": "started"
        })
    );
    Ok(())
}

#[test]
fn workflow_progress_maps_to_code_cell_progress_event() {
    let event = EventMsg::WorkflowProgress(WorkflowProgressEvent {
        thread_id: "thread-root".to_string(),
        turn_id: "turn-1".to_string(),
        run_id: "wf-trace".to_string(),
        cell_id: "1".to_string(),
        event: "agent_completed".to_string(),
        unix_ms: 1235,
        session_id: None,
        workflow_tool_call_id: None,
        cwd: None,
        git_branch: None,
        workflow: Some("release".to_string()),
        phase: Some("verify".to_string()),
        agent: Some("tester".to_string()),
        agent_id: Some("/root/tester".to_string()),
        child: Some("smoke".to_string()),
        child_index: Some(2),
        child_run_id: Some("child-run".to_string()),
        item_index: Some(3),
        stage_index: Some(4),
        step_index: Some(5),
        error: None,
        message: Some("ok".to_string()),
    });

    let trace_event = codex_turn_trace_event("thread-root".to_string(), "fallback-turn", &event)
        .expect("workflow progress trace event");

    assert_eq!(trace_event.context_turn_id, "turn-1");
    let RawTraceEventPayload::CodeCellWorkflowProgress {
        runtime_cell_id,
        progress,
    } = trace_event.payload
    else {
        panic!("expected code cell workflow progress");
    };
    assert_eq!(runtime_cell_id, "1");
    assert_eq!(progress.run_id, "wf-trace");
    assert_eq!(progress.event, "agent_completed");
    assert_eq!(progress.workflow.as_deref(), Some("release"));
    assert_eq!(progress.phase.as_deref(), Some("verify"));
    assert_eq!(progress.agent.as_deref(), Some("tester"));
    assert_eq!(progress.agent_id.as_deref(), Some("/root/tester"));
    assert_eq!(progress.child.as_deref(), Some("smoke"));
    assert_eq!(progress.child_index, Some(2));
    assert_eq!(progress.child_run_id.as_deref(), Some("child-run"));
    assert_eq!(progress.item_index, Some(3));
    assert_eq!(progress.stage_index, Some(4));
    assert_eq!(progress.step_index, Some(5));
    assert_eq!(progress.error, None);
    assert_eq!(progress.message.as_deref(), Some("ok"));
}
