use super::*;
use crate::tools::handlers::multi_agents_spec::WaitAgentTimeoutOptions;
use crate::tools::handlers::multi_agents_spec::create_wait_agent_tool_v2;
use crate::turn_timing::now_unix_timestamp_ms;
use codex_protocol::models::ContentItem;
use codex_protocol::models::LocalShellAction;
use codex_protocol::models::ReasoningItemReasoningSummary;
use codex_protocol::models::ResponseItem;
use codex_tools::ToolSpec;
use std::collections::HashMap;
use std::time::Duration;
use tokio::time::Instant;
use tokio::time::timeout_at;

const WAIT_AGENT_REASONING_LIMIT: usize = 16;
const WAIT_AGENT_REASONING_MAX_CHARS: usize = 1_000;
const WAIT_AGENT_TOOL_CALL_LIMIT: usize = 64;
const WAIT_AGENT_TOOL_OUTPUT_MAX_CHARS: usize = 2_000;
const WAIT_AGENT_TRANSCRIPT_ITEM_LIMIT: usize = 128;
const WAIT_AGENT_TRANSCRIPT_TEXT_MAX_CHARS: usize = 4_000;

#[derive(Default)]
pub(crate) struct Handler {
    options: WaitAgentTimeoutOptions,
}

impl Handler {
    pub(crate) fn new(options: WaitAgentTimeoutOptions) -> Self {
        Self { options }
    }
}

impl ToolExecutor<ToolInvocation> for Handler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain("wait_agent")
    }

    fn spec(&self) -> ToolSpec {
        create_wait_agent_tool_v2(self.options)
    }

    fn handle(&self, invocation: ToolInvocation) -> codex_tools::ToolExecutorFuture<'_> {
        Box::pin(self.handle_call(invocation))
    }
}

impl Handler {
    async fn handle_call(
        &self,
        invocation: ToolInvocation,
    ) -> Result<Box<dyn crate::tools::context::ToolOutput>, FunctionCallError> {
        let ToolInvocation {
            session,
            turn,
            payload,
            call_id,
            ..
        } = invocation;
        let arguments = function_arguments(payload)?;
        let args: WaitArgs = parse_arguments(&arguments)?;
        let min_timeout_ms = turn.config.multi_agent_v2.min_wait_timeout_ms;
        let max_timeout_ms = turn.config.multi_agent_v2.max_wait_timeout_ms;
        let default_timeout_ms = turn.config.multi_agent_v2.default_wait_timeout_ms;
        let timeout_ms = match args.timeout_ms {
            Some(ms) if ms < min_timeout_ms => {
                return Err(FunctionCallError::RespondToModel(format!(
                    "timeout_ms must be at least {min_timeout_ms}"
                )));
            }
            Some(ms) if ms > max_timeout_ms => {
                return Err(FunctionCallError::RespondToModel(format!(
                    "timeout_ms must be at most {max_timeout_ms}"
                )));
            }
            Some(ms) => ms,
            None => default_timeout_ms,
        };

        let mut mailbox_rx = session.input_queue.subscribe_mailbox().await;

        session
            .send_event(
                &turn,
                CollabWaitingBeginEvent {
                    started_at_ms: now_unix_timestamp_ms(),
                    sender_thread_id: session.thread_id,
                    receiver_thread_ids: Vec::new(),
                    receiver_agents: Vec::new(),
                    call_id: call_id.clone(),
                }
                .into(),
            )
            .await;

        let deadline = Instant::now() + Duration::from_millis(timeout_ms as u64);
        let timed_out = !wait_for_mailbox_change(&mut mailbox_rx, deadline).await;
        let messages = if args.include_messages && !timed_out {
            let communications = session.input_queue.pending_mailbox_communications().await;
            let mut messages = Vec::with_capacity(communications.len());
            for communication in communications {
                let (tool_calls, reasoning, transcript) = match session
                    .services
                    .agent_control
                    .load_agent_response_history_for_path(&communication.author)
                    .await
                {
                    Ok(history_items) => {
                        summarize_agent_history(&communication.author, history_items)
                    }
                    Err(_) => (Vec::new(), Vec::new(), Vec::new()),
                };
                messages.push(WaitAgentMessage::from_communication(
                    communication,
                    tool_calls,
                    reasoning,
                    transcript,
                ));
            }
            messages
        } else {
            Vec::new()
        };
        let result = WaitAgentResult::from_timed_out(timed_out, messages);

        session
            .send_event(
                &turn,
                CollabWaitingEndEvent {
                    sender_thread_id: session.thread_id,
                    call_id,
                    completed_at_ms: now_unix_timestamp_ms(),
                    agent_statuses: Vec::new(),
                    statuses: HashMap::new(),
                }
                .into(),
            )
            .await;

        Ok(boxed_tool_output(result))
    }
}

impl CoreToolRuntime for Handler {
    fn matches_kind(&self, payload: &ToolPayload) -> bool {
        matches!(payload, ToolPayload::Function { .. })
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WaitArgs {
    timeout_ms: Option<i64>,
    #[serde(default)]
    include_messages: bool,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct WaitAgentResult {
    pub(crate) message: String,
    pub(crate) timed_out: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) messages: Vec<WaitAgentMessage>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct WaitAgentMessage {
    pub(crate) author: String,
    pub(crate) recipient: String,
    pub(crate) content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) status: Option<AgentStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) final_message: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) tool_calls: Vec<WaitAgentToolCall>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) reasoning: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) transcript: Vec<JsonValue>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct WaitAgentToolCall {
    pub(crate) name: String,
    pub(crate) input: JsonValue,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) output: Option<String>,
    #[serde(skip)]
    call_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ParsedSubagentNotification {
    status: AgentStatus,
}

impl WaitAgentResult {
    fn from_timed_out(timed_out: bool, messages: Vec<WaitAgentMessage>) -> Self {
        let message = if timed_out {
            "Wait timed out."
        } else {
            "Wait completed."
        };
        Self {
            message: message.to_string(),
            timed_out,
            messages,
        }
    }
}

impl WaitAgentMessage {
    fn from_communication(
        communication: InterAgentCommunication,
        tool_calls: Vec<WaitAgentToolCall>,
        reasoning: Vec<String>,
        transcript: Vec<JsonValue>,
    ) -> Self {
        let status = parse_subagent_notification_status(&communication.content);
        let final_message = match status.as_ref() {
            Some(AgentStatus::Completed(Some(message))) => Some(message.clone()),
            _ => None,
        };
        Self {
            author: communication.author.to_string(),
            recipient: communication.recipient.to_string(),
            content: communication.content,
            status,
            final_message,
            tool_calls,
            reasoning,
            transcript,
        }
    }
}

impl ToolOutput for WaitAgentResult {
    fn log_preview(&self) -> String {
        tool_output_json_text(self, "wait_agent")
    }

    fn success_for_logging(&self) -> bool {
        true
    }

    fn to_response_item(&self, call_id: &str, payload: &ToolPayload) -> ResponseInputItem {
        tool_output_response_item(call_id, payload, self, /*success*/ None, "wait_agent")
    }

    fn code_mode_result(&self, _payload: &ToolPayload) -> JsonValue {
        tool_output_code_mode_result(self, "wait_agent")
    }
}

async fn wait_for_mailbox_change(
    mailbox_rx: &mut tokio::sync::watch::Receiver<()>,
    deadline: Instant,
) -> bool {
    match timeout_at(deadline, mailbox_rx.changed()).await {
        Ok(Ok(())) => true,
        Ok(Err(_)) | Err(_) => false,
    }
}

fn parse_subagent_notification_status(content: &str) -> Option<AgentStatus> {
    const OPEN: &str = "<subagent_notification>";
    const CLOSE: &str = "</subagent_notification>";

    let body_start = content.find(OPEN)? + OPEN.len();
    let body_end = content[body_start..].find(CLOSE)? + body_start;
    let parsed: ParsedSubagentNotification =
        serde_json::from_str(content[body_start..body_end].trim()).ok()?;
    Some(parsed.status)
}

fn summarize_agent_history(
    agent_path: &AgentPath,
    history_items: Vec<ResponseItem>,
) -> (Vec<WaitAgentToolCall>, Vec<String>, Vec<JsonValue>) {
    let start_index = history_items
        .iter()
        .rposition(is_agent_turn_input_item)
        .map(|index| index + 1)
        .unwrap_or(0);
    let latest_turn_items = history_items.get(start_index..).unwrap_or_default();
    let mut tool_calls = Vec::new();
    let mut reasoning = Vec::new();
    for item in latest_turn_items {
        if reasoning.len() < WAIT_AGENT_REASONING_LIMIT {
            reasoning.extend(
                wait_agent_reasoning_from_response_item(item)
                    .into_iter()
                    .take(WAIT_AGENT_REASONING_LIMIT.saturating_sub(reasoning.len())),
            );
        }
        if let Some(tool_call) = wait_agent_tool_call_from_response_item(item) {
            if tool_calls.len() < WAIT_AGENT_TOOL_CALL_LIMIT {
                tool_calls.push(tool_call);
            }
            continue;
        }
        if let Some((call_id, output)) = wait_agent_tool_output_from_response_item(item) {
            attach_wait_agent_tool_output(&mut tool_calls, call_id.as_deref(), output);
        }
    }
    for tool_call in &mut tool_calls {
        tool_call.call_id = None;
    }
    let transcript = summarize_agent_transcript(agent_path, &history_items);
    (tool_calls, reasoning, transcript)
}

fn is_agent_turn_input_item(item: &ResponseItem) -> bool {
    matches!(item, ResponseItem::AgentMessage { .. })
        || matches!(item, ResponseItem::Message { role, .. } if role == "user")
}

fn wait_agent_tool_call_from_response_item(item: &ResponseItem) -> Option<WaitAgentToolCall> {
    match item {
        ResponseItem::LocalShellCall {
            call_id, action, ..
        } => {
            let LocalShellAction::Exec(exec) = action;
            Some(WaitAgentToolCall {
                name: "exec_command".to_string(),
                input: serde_json::json!({
                    "cmd": exec.command.join(" "),
                    "command": exec.command,
                    "working_directory": exec.working_directory,
                }),
                output: None,
                call_id: call_id.clone(),
            })
        }
        ResponseItem::FunctionCall {
            name,
            namespace,
            arguments,
            call_id,
            ..
        } => Some(WaitAgentToolCall {
            name: tool_call_name(namespace.as_deref(), name),
            input: parse_tool_call_input(arguments),
            output: None,
            call_id: Some(call_id.clone()),
        }),
        ResponseItem::ToolSearchCall {
            call_id,
            execution,
            arguments,
            ..
        } => Some(WaitAgentToolCall {
            name: tool_call_name(Some("tool_search"), execution),
            input: arguments.clone(),
            output: None,
            call_id: call_id.clone(),
        }),
        ResponseItem::CustomToolCall {
            call_id,
            name,
            input,
            ..
        } => Some(WaitAgentToolCall {
            name: name.clone(),
            input: parse_tool_call_input(input),
            output: None,
            call_id: Some(call_id.clone()),
        }),
        ResponseItem::WebSearchCall { action, .. } => Some(WaitAgentToolCall {
            name: "web_search".to_string(),
            input: action
                .as_ref()
                .and_then(|action| serde_json::to_value(action).ok())
                .unwrap_or(JsonValue::Null),
            output: None,
            call_id: None,
        }),
        ResponseItem::ImageGenerationCall {
            status,
            revised_prompt,
            ..
        } => Some(WaitAgentToolCall {
            name: "image_generation".to_string(),
            input: serde_json::json!({
                "status": status,
                "revised_prompt": revised_prompt,
            }),
            output: None,
            call_id: None,
        }),
        ResponseItem::Message { .. }
        | ResponseItem::AgentMessage { .. }
        | ResponseItem::Reasoning { .. }
        | ResponseItem::FunctionCallOutput { .. }
        | ResponseItem::CustomToolCallOutput { .. }
        | ResponseItem::ToolSearchOutput { .. }
        | ResponseItem::Compaction { .. }
        | ResponseItem::CompactionTrigger
        | ResponseItem::ContextCompaction { .. }
        | ResponseItem::Other => None,
    }
}

fn wait_agent_reasoning_from_response_item(item: &ResponseItem) -> Vec<String> {
    let ResponseItem::Reasoning { summary, .. } = item else {
        return Vec::new();
    };
    summary
        .iter()
        .filter_map(|item| match item {
            ReasoningItemReasoningSummary::SummaryText { text } => non_empty_str(text)
                .map(|text| truncate_wait_agent_text(text, WAIT_AGENT_REASONING_MAX_CHARS)),
        })
        .collect()
}

fn wait_agent_transcript_entries_from_response_item(item: &ResponseItem) -> Vec<JsonValue> {
    match item {
        ResponseItem::Message { role, content, .. } => {
            let content = wait_agent_transcript_content_items(content);
            if content.is_empty() {
                Vec::new()
            } else {
                vec![serde_json::json!({
                    "role": role,
                    "content": content,
                })]
            }
        }
        ResponseItem::Reasoning { summary, .. } => {
            let summary = summary
                .iter()
                .filter_map(|item| match item {
                    ReasoningItemReasoningSummary::SummaryText { text } => {
                        non_empty_str(text).map(|text| {
                            serde_json::json!({
                                "type": "summary_text",
                                "text": truncate_wait_agent_text(
                                    text,
                                    WAIT_AGENT_TRANSCRIPT_TEXT_MAX_CHARS
                                ),
                            })
                        })
                    }
                })
                .collect::<Vec<_>>();
            if summary.is_empty() {
                Vec::new()
            } else {
                vec![serde_json::json!({
                    "role": "assistant",
                    "content": [
                        {
                            "type": "reasoning",
                            "summary": summary,
                        }
                    ],
                })]
            }
        }
        ResponseItem::FunctionCall {
            name,
            namespace,
            arguments,
            call_id,
            ..
        } => {
            vec![serde_json::json!({
                "role": "assistant",
                "content": [
                    {
                        "type": "tool_use",
                        "id": call_id,
                        "name": tool_call_name(namespace.as_deref(), name),
                        "input": parse_tool_call_input(arguments),
                    }
                ],
            })]
        }
        ResponseItem::CustomToolCall {
            call_id,
            name,
            input,
            ..
        } => {
            vec![serde_json::json!({
                "role": "assistant",
                "content": [
                    {
                        "type": "tool_use",
                        "id": call_id,
                        "name": name,
                        "input": parse_tool_call_input(input),
                    }
                ],
            })]
        }
        ResponseItem::ToolSearchCall {
            call_id,
            execution,
            arguments,
            ..
        } => wait_agent_transcript_tool_use(
            call_id.as_deref(),
            &tool_call_name(Some("tool_search"), execution),
            arguments.clone(),
        ),
        ResponseItem::LocalShellCall {
            call_id, action, ..
        } => {
            let LocalShellAction::Exec(exec) = action;
            wait_agent_transcript_tool_use(
                call_id.as_deref(),
                "exec_command",
                serde_json::json!({
                    "cmd": exec.command.join(" "),
                    "command": exec.command,
                    "working_directory": exec.working_directory,
                }),
            )
        }
        ResponseItem::WebSearchCall { action, .. } => wait_agent_transcript_tool_use(
            None,
            "web_search",
            action
                .as_ref()
                .and_then(|action| serde_json::to_value(action).ok())
                .unwrap_or(JsonValue::Null),
        ),
        ResponseItem::ImageGenerationCall {
            status,
            revised_prompt,
            ..
        } => wait_agent_transcript_tool_use(
            None,
            "image_generation",
            serde_json::json!({
                "status": status,
                "revised_prompt": revised_prompt,
            }),
        ),
        ResponseItem::FunctionCallOutput { call_id, output }
        | ResponseItem::CustomToolCallOutput {
            call_id, output, ..
        } => output.body.to_text().map_or_else(Vec::new, |text| {
            wait_agent_transcript_tool_result(Some(call_id), text)
        }),
        ResponseItem::ToolSearchOutput {
            call_id,
            status,
            execution,
            tools,
        } => wait_agent_transcript_tool_result(
            call_id.as_ref(),
            serde_json::json!({
                "status": status,
                "execution": execution,
                "tools": tools,
            })
            .to_string(),
        ),
        ResponseItem::AgentMessage { .. }
        | ResponseItem::Compaction { .. }
        | ResponseItem::CompactionTrigger
        | ResponseItem::ContextCompaction { .. }
        | ResponseItem::Other => Vec::new(),
    }
}

fn summarize_agent_transcript(
    agent_path: &AgentPath,
    history_items: &[ResponseItem],
) -> Vec<JsonValue> {
    let start_index = history_items
        .iter()
        .position(|item| is_child_input_boundary(item, agent_path))
        .map(|index| index + 1)
        .or_else(|| {
            history_items
                .iter()
                .position(is_agent_turn_input_item)
                .map(|index| index + 1)
        })
        .unwrap_or(0);
    let mut transcript = Vec::new();
    for item in history_items.get(start_index..).unwrap_or_default() {
        if is_child_input_boundary(item, agent_path) || is_agent_turn_input_item(item) {
            continue;
        }
        transcript.extend(wait_agent_transcript_entries_from_response_item(item));
    }
    if transcript.len() > WAIT_AGENT_TRANSCRIPT_ITEM_LIMIT {
        transcript.split_off(transcript.len() - WAIT_AGENT_TRANSCRIPT_ITEM_LIMIT)
    } else {
        transcript
    }
}

pub(crate) fn workflow_agent_live_transcript_entries_from_response_item(
    item: &ResponseItem,
) -> Vec<JsonValue> {
    match item {
        ResponseItem::Message { role, .. } if role != "assistant" => Vec::new(),
        ResponseItem::Message { role, content, .. }
            if role == "assistant"
                && InterAgentCommunication::from_message_content(content).is_some() =>
        {
            Vec::new()
        }
        ResponseItem::AgentMessage { .. } => Vec::new(),
        _ => wait_agent_transcript_entries_from_response_item(item),
    }
}

fn is_child_input_boundary(item: &ResponseItem, agent_path: &AgentPath) -> bool {
    match item {
        ResponseItem::AgentMessage { recipient, .. } => recipient == agent_path.as_str(),
        ResponseItem::Message { role, content, .. } if role == "assistant" => {
            InterAgentCommunication::from_message_content(content).is_some_and(|communication| {
                communication.recipient == *agent_path
                    || communication
                        .other_recipients
                        .iter()
                        .any(|recipient| recipient == agent_path)
            })
        }
        ResponseItem::Message { role, .. } if role == "user" => true,
        _ => false,
    }
}

fn wait_agent_transcript_content_items(content: &[ContentItem]) -> Vec<JsonValue> {
    content
        .iter()
        .map(|item| match item {
            ContentItem::InputText { text } | ContentItem::OutputText { text } => {
                serde_json::json!({
                    "type": "text",
                    "text": truncate_wait_agent_text(text, WAIT_AGENT_TRANSCRIPT_TEXT_MAX_CHARS),
                })
            }
            ContentItem::InputImage { image_url, detail } => {
                let mut item = serde_json::json!({
                    "type": "image",
                    "image_url": image_url,
                });
                if let Some(detail) = detail
                    && let Some(object) = item.as_object_mut()
                {
                    object.insert(
                        "detail".to_string(),
                        serde_json::to_value(detail).unwrap_or(JsonValue::Null),
                    );
                }
                item
            }
        })
        .collect()
}

fn wait_agent_transcript_tool_use(
    call_id: Option<&str>,
    name: &str,
    input: JsonValue,
) -> Vec<JsonValue> {
    let mut tool_use = serde_json::json!({
        "type": "tool_use",
        "name": name,
        "input": input,
    });
    if let Some(call_id) = call_id.and_then(non_empty_str)
        && let Some(object) = tool_use.as_object_mut()
    {
        object.insert("id".to_string(), JsonValue::String(call_id.to_string()));
    }
    vec![serde_json::json!({
        "role": "assistant",
        "content": [tool_use],
    })]
}

fn wait_agent_transcript_tool_result(call_id: Option<&String>, output: String) -> Vec<JsonValue> {
    let output = truncate_wait_agent_text(output.trim(), WAIT_AGENT_TRANSCRIPT_TEXT_MAX_CHARS);
    if output.is_empty() {
        return Vec::new();
    }
    let mut result = serde_json::json!({
        "type": "tool_result",
        "content": output,
    });
    if let Some(call_id) = call_id.and_then(|call_id| non_empty_str(call_id))
        && let Some(object) = result.as_object_mut()
    {
        object.insert(
            "tool_use_id".to_string(),
            JsonValue::String(call_id.to_string()),
        );
    }
    vec![result]
}

fn wait_agent_tool_output_from_response_item(
    item: &ResponseItem,
) -> Option<(Option<String>, String)> {
    match item {
        ResponseItem::FunctionCallOutput { call_id, output } => output
            .body
            .to_text()
            .map(|text| (Some(call_id.clone()), text)),
        ResponseItem::CustomToolCallOutput {
            call_id, output, ..
        } => output
            .body
            .to_text()
            .map(|text| (Some(call_id.clone()), text)),
        ResponseItem::ToolSearchOutput {
            call_id,
            status,
            execution,
            tools,
        } => Some((
            call_id.clone(),
            serde_json::json!({
                "status": status,
                "execution": execution,
                "tools": tools,
            })
            .to_string(),
        )),
        _ => None,
    }
}

fn attach_wait_agent_tool_output(
    tool_calls: &mut [WaitAgentToolCall],
    call_id: Option<&str>,
    output: String,
) {
    let output = truncate_wait_agent_tool_output(output);
    if output.trim().is_empty() {
        return;
    }
    let target = match call_id.and_then(non_empty_str) {
        Some(call_id) => tool_calls
            .iter_mut()
            .rev()
            .find(|tool_call| tool_call.call_id.as_deref() == Some(call_id)),
        None => tool_calls
            .iter_mut()
            .rev()
            .find(|tool_call| tool_call.output.is_none()),
    };
    if let Some(tool_call) = target {
        tool_call.output = Some(output);
    }
}

fn truncate_wait_agent_tool_output(output: String) -> String {
    truncate_wait_agent_text(output.trim(), WAIT_AGENT_TOOL_OUTPUT_MAX_CHARS)
}

fn truncate_wait_agent_text(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    text.chars().take(max_chars).collect::<String>()
}

fn non_empty_str(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()).then_some(value)
}

fn tool_call_name(namespace: Option<&str>, name: &str) -> String {
    match namespace {
        Some(namespace) if !namespace.trim().is_empty() => {
            format!("{}.{}", namespace.trim(), name)
        }
        _ => name.to_string(),
    }
}

fn parse_tool_call_input(input: &str) -> JsonValue {
    serde_json::from_str(input).unwrap_or_else(|_| JsonValue::String(input.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workflow_agent_live_transcript_entries_skip_child_input_messages() {
        let user_item = ResponseItem::Message {
            id: None,
            role: "user".to_string(),
            content: vec![ContentItem::InputText {
                text: "run this child task".to_string(),
            }],
            phase: None,
        };
        assert!(workflow_agent_live_transcript_entries_from_response_item(&user_item).is_empty());

        let agent_message = ResponseItem::AgentMessage {
            author: "/root/workflow_release_1".to_string(),
            recipient: "/root".to_string(),
            content: Vec::new(),
        };
        assert!(
            workflow_agent_live_transcript_entries_from_response_item(&agent_message).is_empty()
        );

        let assistant_item = ResponseItem::Message {
            id: None,
            role: "assistant".to_string(),
            content: vec![ContentItem::OutputText {
                text: "child result".to_string(),
            }],
            phase: None,
        };
        let entries = workflow_agent_live_transcript_entries_from_response_item(&assistant_item);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["role"], "assistant");
        assert_eq!(entries[0]["content"][0]["type"], "text");
        assert_eq!(entries[0]["content"][0]["text"], "child result");
    }
}
