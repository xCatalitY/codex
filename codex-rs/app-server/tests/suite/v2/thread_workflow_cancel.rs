use anyhow::Result;
use app_test_support::TestAppServer;
use app_test_support::create_mock_responses_server_sequence;
use app_test_support::to_response;
use codex_app_server_protocol::JSONRPCError;
use codex_app_server_protocol::JSONRPCResponse;
use codex_app_server_protocol::RequestId;
use codex_app_server_protocol::ThreadStartParams;
use codex_app_server_protocol::ThreadStartResponse;
use codex_app_server_protocol::ThreadWorkflowAgentControlParams;
use codex_app_server_protocol::ThreadWorkflowAgentControlResponse;
use codex_app_server_protocol::ThreadWorkflowAgentInterruptParams;
use codex_app_server_protocol::ThreadWorkflowAgentInterruptResponse;
use codex_app_server_protocol::ThreadWorkflowCancelParams;
use codex_app_server_protocol::ThreadWorkflowCancelResponse;
use codex_app_server_protocol::ThreadWorkflowContinueParams;
use codex_app_server_protocol::ThreadWorkflowContinueResponse;
use codex_app_server_protocol::ThreadWorkflowPauseParams;
use codex_app_server_protocol::ThreadWorkflowPauseResponse;
use codex_protocol::ThreadId;
use codex_protocol::protocol::WorkflowAgentControlAction;
use pretty_assertions::assert_eq;
use std::path::Path;
use tempfile::TempDir;
use tokio::time::timeout;

const DEFAULT_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const INVALID_REQUEST_ERROR_CODE: i64 = -32600;

#[tokio::test]
async fn thread_workflow_cancel_routes_to_loaded_thread_and_returns_response() -> Result<()> {
    let server = create_mock_responses_server_sequence(Vec::new()).await;
    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), &server.uri())?;

    let mut mcp = TestAppServer::new(codex_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;
    let thread_id = start_thread(&mut mcp).await?;

    let request_id = send_thread_workflow_cancel_request(
        &mut mcp,
        ThreadWorkflowCancelParams {
            thread_id,
            run_id: "run-1".to_string(),
            cell_id: "cell-1".to_string(),
        },
    )
    .await?;

    let response: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let _: ThreadWorkflowCancelResponse = to_response(response)?;
    Ok(())
}

#[tokio::test]
async fn thread_workflow_pause_routes_to_loaded_thread_and_returns_response() -> Result<()> {
    let server = create_mock_responses_server_sequence(Vec::new()).await;
    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), &server.uri())?;

    let mut mcp = TestAppServer::new(codex_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;
    let thread_id = start_thread(&mut mcp).await?;

    let request_id = send_thread_workflow_pause_request(
        &mut mcp,
        ThreadWorkflowPauseParams {
            thread_id,
            run_id: "run-1".to_string(),
            cell_id: "cell-1".to_string(),
        },
    )
    .await?;

    let response: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let _: ThreadWorkflowPauseResponse = to_response(response)?;
    Ok(())
}

#[tokio::test]
async fn thread_workflow_continue_routes_to_loaded_thread_and_returns_response() -> Result<()> {
    let server = create_mock_responses_server_sequence(Vec::new()).await;
    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), &server.uri())?;

    let mut mcp = TestAppServer::new(codex_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;
    let thread_id = start_thread(&mut mcp).await?;

    let request_id = send_thread_workflow_continue_request(
        &mut mcp,
        ThreadWorkflowContinueParams {
            thread_id,
            run_id: "run-1".to_string(),
            cell_id: "cell-1".to_string(),
        },
    )
    .await?;

    let response: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let _: ThreadWorkflowContinueResponse = to_response(response)?;
    Ok(())
}

#[tokio::test]
async fn thread_workflow_agent_interrupt_routes_to_loaded_thread_and_returns_response() -> Result<()>
{
    let server = create_mock_responses_server_sequence(Vec::new()).await;
    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), &server.uri())?;

    let mut mcp = TestAppServer::new(codex_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;
    let thread_id = start_thread(&mut mcp).await?;

    let request_id = send_thread_workflow_agent_interrupt_request(
        &mut mcp,
        ThreadWorkflowAgentInterruptParams {
            thread_id,
            run_id: "run-1".to_string(),
            agent_id: "/root/workflow_1".to_string(),
        },
    )
    .await?;

    let response: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let _: ThreadWorkflowAgentInterruptResponse = to_response(response)?;
    Ok(())
}

#[tokio::test]
async fn thread_workflow_agent_control_routes_to_loaded_thread_and_returns_response() -> Result<()>
{
    let server = create_mock_responses_server_sequence(Vec::new()).await;
    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), &server.uri())?;

    let mut mcp = TestAppServer::new(codex_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;
    let thread_id = start_thread(&mut mcp).await?;

    let request_id = send_thread_workflow_agent_control_request(
        &mut mcp,
        ThreadWorkflowAgentControlParams {
            thread_id,
            run_id: "run-1".to_string(),
            agent_id: "/root/workflow_1".to_string(),
            action: WorkflowAgentControlAction::Skip,
        },
    )
    .await?;

    let response: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let _: ThreadWorkflowAgentControlResponse = to_response(response)?;
    Ok(())
}

#[tokio::test]
async fn thread_workflow_cancel_rejects_blank_run_or_cell_id() -> Result<()> {
    let server = create_mock_responses_server_sequence(Vec::new()).await;
    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), &server.uri())?;

    let mut mcp = TestAppServer::new(codex_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    for (run_id, cell_id, expected_message) in [
        (" ", "cell-1", "runId must not be empty"),
        ("run-1", " ", "cellId must not be empty"),
    ] {
        let request_id = send_thread_workflow_cancel_request(
            &mut mcp,
            ThreadWorkflowCancelParams {
                thread_id: ThreadId::new().to_string(),
                run_id: run_id.to_string(),
                cell_id: cell_id.to_string(),
            },
        )
        .await?;

        let error: JSONRPCError = timeout(
            DEFAULT_READ_TIMEOUT,
            mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
        )
        .await??;
        assert_eq!(error.error.code, INVALID_REQUEST_ERROR_CODE);
        assert_eq!(error.error.message, expected_message);
    }

    Ok(())
}

#[tokio::test]
async fn thread_workflow_pause_rejects_blank_run_or_cell_id() -> Result<()> {
    let server = create_mock_responses_server_sequence(Vec::new()).await;
    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), &server.uri())?;

    let mut mcp = TestAppServer::new(codex_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    for (run_id, cell_id, expected_message) in [
        (" ", "cell-1", "runId must not be empty"),
        ("run-1", " ", "cellId must not be empty"),
    ] {
        let request_id = send_thread_workflow_pause_request(
            &mut mcp,
            ThreadWorkflowPauseParams {
                thread_id: ThreadId::new().to_string(),
                run_id: run_id.to_string(),
                cell_id: cell_id.to_string(),
            },
        )
        .await?;

        let error: JSONRPCError = timeout(
            DEFAULT_READ_TIMEOUT,
            mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
        )
        .await??;
        assert_eq!(error.error.code, INVALID_REQUEST_ERROR_CODE);
        assert_eq!(error.error.message, expected_message);
    }

    Ok(())
}

#[tokio::test]
async fn thread_workflow_continue_rejects_blank_run_or_cell_id() -> Result<()> {
    let server = create_mock_responses_server_sequence(Vec::new()).await;
    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), &server.uri())?;

    let mut mcp = TestAppServer::new(codex_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    for (run_id, cell_id, expected_message) in [
        (" ", "cell-1", "runId must not be empty"),
        ("run-1", " ", "cellId must not be empty"),
    ] {
        let request_id = send_thread_workflow_continue_request(
            &mut mcp,
            ThreadWorkflowContinueParams {
                thread_id: ThreadId::new().to_string(),
                run_id: run_id.to_string(),
                cell_id: cell_id.to_string(),
            },
        )
        .await?;

        let error: JSONRPCError = timeout(
            DEFAULT_READ_TIMEOUT,
            mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
        )
        .await??;
        assert_eq!(error.error.code, INVALID_REQUEST_ERROR_CODE);
        assert_eq!(error.error.message, expected_message);
    }

    Ok(())
}

#[tokio::test]
async fn thread_workflow_agent_interrupt_rejects_blank_run_or_agent_id() -> Result<()> {
    let server = create_mock_responses_server_sequence(Vec::new()).await;
    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), &server.uri())?;

    let mut mcp = TestAppServer::new(codex_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    for (run_id, agent_id, expected_message) in [
        (" ", "/root/workflow_1", "runId must not be empty"),
        ("run-1", " ", "agentId must not be empty"),
    ] {
        let request_id = send_thread_workflow_agent_interrupt_request(
            &mut mcp,
            ThreadWorkflowAgentInterruptParams {
                thread_id: ThreadId::new().to_string(),
                run_id: run_id.to_string(),
                agent_id: agent_id.to_string(),
            },
        )
        .await?;

        let error: JSONRPCError = timeout(
            DEFAULT_READ_TIMEOUT,
            mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
        )
        .await??;
        assert_eq!(error.error.code, INVALID_REQUEST_ERROR_CODE);
        assert_eq!(error.error.message, expected_message);
    }

    Ok(())
}

#[tokio::test]
async fn thread_workflow_agent_control_rejects_blank_run_or_agent_id() -> Result<()> {
    let server = create_mock_responses_server_sequence(Vec::new()).await;
    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), &server.uri())?;

    let mut mcp = TestAppServer::new(codex_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    for (run_id, agent_id, expected_message) in [
        (" ", "/root/workflow_1", "runId must not be empty"),
        ("run-1", " ", "agentId must not be empty"),
    ] {
        let request_id = send_thread_workflow_agent_control_request(
            &mut mcp,
            ThreadWorkflowAgentControlParams {
                thread_id: ThreadId::new().to_string(),
                run_id: run_id.to_string(),
                agent_id: agent_id.to_string(),
                action: WorkflowAgentControlAction::Retry,
            },
        )
        .await?;

        let error: JSONRPCError = timeout(
            DEFAULT_READ_TIMEOUT,
            mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
        )
        .await??;
        assert_eq!(error.error.code, INVALID_REQUEST_ERROR_CODE);
        assert_eq!(error.error.message, expected_message);
    }

    Ok(())
}

#[tokio::test]
async fn thread_workflow_cancel_returns_error_for_missing_thread() -> Result<()> {
    let server = create_mock_responses_server_sequence(Vec::new()).await;
    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), &server.uri())?;

    let mut mcp = TestAppServer::new(codex_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let thread_id = ThreadId::new().to_string();
    let request_id = send_thread_workflow_cancel_request(
        &mut mcp,
        ThreadWorkflowCancelParams {
            thread_id: thread_id.clone(),
            run_id: "run-1".to_string(),
            cell_id: "cell-1".to_string(),
        },
    )
    .await?;

    let error: JSONRPCError = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;
    assert_eq!(error.error.code, INVALID_REQUEST_ERROR_CODE);
    assert_eq!(
        error.error.message,
        format!("thread not found: {thread_id}")
    );
    Ok(())
}

async fn start_thread(mcp: &mut TestAppServer) -> Result<String> {
    let request_id = mcp
        .send_thread_start_request(ThreadStartParams {
            model: Some("mock-model".to_string()),
            ..Default::default()
        })
        .await?;
    let response: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let ThreadStartResponse { thread, .. } = to_response(response)?;
    Ok(thread.id)
}

async fn send_thread_workflow_cancel_request(
    mcp: &mut TestAppServer,
    params: ThreadWorkflowCancelParams,
) -> Result<i64> {
    mcp.send_raw_request(
        "thread/workflow/cancel",
        Some(serde_json::to_value(params)?),
    )
    .await
}

async fn send_thread_workflow_pause_request(
    mcp: &mut TestAppServer,
    params: ThreadWorkflowPauseParams,
) -> Result<i64> {
    mcp.send_raw_request("thread/workflow/pause", Some(serde_json::to_value(params)?))
        .await
}

async fn send_thread_workflow_continue_request(
    mcp: &mut TestAppServer,
    params: ThreadWorkflowContinueParams,
) -> Result<i64> {
    mcp.send_raw_request(
        "thread/workflow/continue",
        Some(serde_json::to_value(params)?),
    )
    .await
}

async fn send_thread_workflow_agent_interrupt_request(
    mcp: &mut TestAppServer,
    params: ThreadWorkflowAgentInterruptParams,
) -> Result<i64> {
    mcp.send_raw_request(
        "thread/workflow/agent/interrupt",
        Some(serde_json::to_value(params)?),
    )
    .await
}

async fn send_thread_workflow_agent_control_request(
    mcp: &mut TestAppServer,
    params: ThreadWorkflowAgentControlParams,
) -> Result<i64> {
    mcp.send_raw_request(
        "thread/workflow/agent/control",
        Some(serde_json::to_value(params)?),
    )
    .await
}

fn create_config_toml(codex_home: &Path, server_uri: &str) -> std::io::Result<()> {
    let config_toml = codex_home.join("config.toml");
    std::fs::write(
        config_toml,
        format!(
            r#"
model = "mock-model"
approval_policy = "never"
sandbox_mode = "read-only"

model_provider = "mock_provider"

[model_providers.mock_provider]
name = "Mock provider for test"
base_url = "{server_uri}/v1"
wire_api = "responses"
request_max_retries = 0
stream_max_retries = 0
"#
        ),
    )
}
