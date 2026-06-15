use std::collections::BTreeMap;

use codex_tools::JsonSchema;
use codex_tools::JsonToolOutput;
use codex_tools::ResponsesApiTool;
use codex_tools::ToolName;
use codex_tools::ToolSpec;
use serde::Deserialize;
use serde_json::json;

use crate::function_tool::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::context::boxed_tool_output;
use crate::tools::handlers::parse_arguments;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::ToolExecutor;

pub(crate) const WORKFLOW_CONTROL_TOOL_NAME: &str = "workflow_control";

pub(crate) struct WorkflowControlHandler;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkflowControlArgs {
    #[serde(rename = "runId", alias = "run_id")]
    run_id: String,
    #[serde(rename = "agentId", alias = "agent_id")]
    agent_id: String,
}

impl ToolExecutor<ToolInvocation> for WorkflowControlHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain(WORKFLOW_CONTROL_TOOL_NAME)
    }

    fn spec(&self) -> ToolSpec {
        let parameters = JsonSchema::object(
            BTreeMap::from([
                (
                    "run_id".to_string(),
                    JsonSchema::string(Some("Workflow run id.".to_string())),
                ),
                (
                    "agent_id".to_string(),
                    JsonSchema::string(Some("Canonical workflow agent path.".to_string())),
                ),
            ]),
            Some(vec!["run_id".to_string(), "agent_id".to_string()]),
            Some(false.into()),
        );
        ToolSpec::Function(ResponsesApiTool {
            name: WORKFLOW_CONTROL_TOOL_NAME.to_string(),
            description: "Internal workflow runtime control-state query.".to_string(),
            strict: false,
            defer_loading: None,
            parameters,
            output_schema: None,
        })
    }

    fn handle(&self, invocation: ToolInvocation) -> codex_tools::ToolExecutorFuture<'_> {
        Box::pin(async move {
            let ToolInvocation { turn, payload, .. } = invocation;
            let arguments = match payload {
                ToolPayload::Function { arguments } => arguments,
                _ => {
                    return Err(FunctionCallError::RespondToModel(
                        "workflow_control expects JSON function arguments".to_string(),
                    ));
                }
            };
            let args: WorkflowControlArgs = parse_arguments(&arguments)?;
            let run_id = args.run_id.trim();
            let agent_id = args.agent_id.trim();
            if run_id.is_empty() || agent_id.is_empty() {
                return Err(FunctionCallError::RespondToModel(
                    "workflow_control requires non-empty run_id and agent_id".to_string(),
                ));
            }
            let request = crate::tools::code_mode::workflow_agent_control_request_for_run(
                turn.as_ref(),
                run_id,
                agent_id,
            )
            .await;
            let value = match request {
                Some(request) => json!({
                    "action": request.action,
                    "event": request.event,
                    "message": request.message,
                }),
                None => json!({
                    "action": null,
                }),
            };
            Ok(boxed_tool_output(JsonToolOutput::new(value)))
        })
    }
}

impl CoreToolRuntime for WorkflowControlHandler {}
