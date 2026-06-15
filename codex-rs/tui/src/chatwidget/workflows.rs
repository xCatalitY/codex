//! Workflow slash-command discovery, browsing, and run rendering.

use super::*;
use crate::bottom_pane::WorkflowNamedPolicyItem;
use crate::bottom_pane::slash_commands::WorkflowSlashCommand;
use codex_config::types::WorkflowApproval;
use codex_protocol::protocol::WorkflowAgentControlAction;
use serde::Deserialize;
use std::collections::BTreeSet;
use std::fs;
use std::io::Read;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

const WORKFLOW_USAGE: &str = "Usage: /workflow [on|off|status|ultracode]";
const WORKFLOWS_USAGE: &str = "Usage: /workflows [run_id] | /workflows detail <run_id> | /workflows run <name> [args] | /workflows resume <run_id> | /workflows retry <run_id> | /workflows cancel <run_id> | /workflows pause <run_id> | /workflows continue <run_id> | /workflows interrupt-agent <run_id> <agent_id> | /workflows skip-agent <run_id> <agent_id> | /workflows retry-agent <run_id> <agent_id> | /workflows restart-agent <run_id> <agent_id> | /workflows save <run_id> <name> | /workflows approval <name> <auto|ask|allow|deny|clear> | /workflows enabled <name> <on|off|clear> | /workflows enable|disable <name>";
const WORKFLOW_RUNS_DIR: &str = "workflow-runs";
const WORKFLOW_ACTIVE_RUNS_DIR: &str = "active";
const WORKFLOW_RUN_LIST_LIMIT: usize = 8;
const WORKFLOW_DEFINITION_LIST_LIMIT: usize = 12;
const WORKFLOW_DEFINITION_READ_LIMIT_BYTES: usize = 16 * 1024;
const WORKFLOW_RUN_DETAIL_PROGRESS_LIMIT: usize = 20;
const WORKFLOW_AGENT_JOURNAL_FILE: &str = "journal.jsonl";
const WORKFLOW_AGENT_JOURNAL_READ_LIMIT_BYTES: u64 = 1024 * 1024;
const WORKFLOW_AGENT_JOURNAL_READ_LIMIT_ENTRIES: usize = 1000;
const WORKFLOW_AGENT_JOURNAL_AGENT_DISPLAY_LIMIT: usize = 20;
const WORKFLOW_AGENT_TRANSCRIPT_READ_LIMIT_BYTES: u64 = 1024 * 1024;
const WORKFLOW_AGENT_TRANSCRIPT_FILE_MAX_AGENT_ID_CHARS: usize = 128;
const WORKFLOW_AGENT_TRANSCRIPT_TOOL_CALL_DISPLAY_LIMIT: usize = 8;
const WORKFLOW_TRANSCRIPT_DIR: &str = "transcripts";
const WORKFLOW_TRANSCRIPT_RUN_FILE: &str = "run.json";
const CLAUDE_WORKFLOW_SNAPSHOT_DIR: &str = "workflows";
const CLAUDE_WORKFLOW_SUBAGENT_DIR: &str = "subagents";

#[derive(Debug, Deserialize)]
struct StoredWorkflowRun {
    #[serde(default, alias = "runId")]
    run_id: String,
    #[serde(default, alias = "workflowName")]
    workflow_name: String,
    #[serde(default, alias = "metadataName")]
    metadata_name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default, alias = "inputSchema")]
    input_schema: Option<String>,
    #[serde(default)]
    status: String,
    #[serde(default, alias = "statusHistory")]
    status_history: Vec<StoredWorkflowStatusEvent>,
    #[serde(default, alias = "workflowProgress")]
    progress: Vec<StoredWorkflowProgressEvent>,
    #[serde(default, alias = "cellId")]
    cell_id: Option<String>,
    #[serde(default, alias = "sessionId")]
    session_id: Option<String>,
    #[serde(default, alias = "threadId")]
    thread_id: Option<String>,
    #[serde(default, alias = "workflowToolCallId")]
    workflow_tool_call_id: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default, alias = "gitBranch")]
    git_branch: Option<String>,
    #[serde(default, alias = "runDir")]
    run_dir: Option<String>,
    #[serde(default, alias = "scriptPath")]
    script_path: Option<String>,
    #[serde(default, alias = "transcriptDir")]
    transcript_dir: Option<String>,
    #[serde(default, alias = "resumeFromRunId")]
    resume_from_run_id: Option<String>,
    #[serde(default, alias = "scriptHash")]
    script_hash: Option<String>,
    #[serde(default)]
    source: Option<StoredWorkflowRunSource>,
    #[serde(default, alias = "sourceKind")]
    source_kind: Option<String>,
    #[serde(default, alias = "sourceName")]
    source_name: Option<String>,
    #[serde(default, alias = "sourcePath")]
    source_path: Option<String>,
    #[serde(default)]
    args: Option<serde_json::Value>,
    #[serde(default, alias = "maxOutputTokens")]
    max_output_tokens: Option<usize>,
    #[serde(default, alias = "startedUnixMs")]
    started_unix_ms: Option<u128>,
    #[serde(default, alias = "updatedUnixMs")]
    updated_unix_ms: Option<u128>,
    #[serde(default, alias = "endedUnixMs")]
    ended_unix_ms: Option<u128>,
    #[serde(default, alias = "durationMs")]
    duration_ms: Option<u128>,
    #[serde(default, alias = "outputPreview")]
    output_preview: Option<String>,
    #[serde(default)]
    error: Option<String>,
    #[serde(skip)]
    journal_summary: Option<StoredWorkflowJournalSummary>,
}

#[derive(Debug, Deserialize)]
struct StoredWorkflowRunSource {
    #[serde(default)]
    kind: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    path: Option<String>,
}

#[derive(Debug, Deserialize)]
struct StoredWorkflowStatusEvent {
    #[serde(default)]
    event: String,
    #[serde(default)]
    status: Option<String>,
    #[serde(default, alias = "unixMs")]
    unix_ms: Option<u128>,
    #[serde(default)]
    message: Option<String>,
}

#[derive(Debug)]
struct StoredWorkflowJournalSummary {
    started: usize,
    results: usize,
    child_results: usize,
    invalid: usize,
    agents: Vec<StoredWorkflowJournalAgent>,
    children: Vec<StoredWorkflowJournalChild>,
}

#[derive(Debug)]
struct StoredWorkflowJournalAgent {
    agent_id: String,
    key: Option<String>,
    status: &'static str,
}

#[derive(Debug)]
struct StoredWorkflowJournalChild {
    child: String,
    child_run_id: Option<String>,
    key: Option<String>,
}

#[derive(Debug, Deserialize)]
struct StoredWorkflowProgressEvent {
    #[serde(default, alias = "type")]
    event: String,
    #[serde(
        default,
        alias = "unixMs",
        alias = "lastProgressAt",
        alias = "startedAt",
        alias = "queuedAt"
    )]
    unix_ms: Option<u128>,
    #[serde(default)]
    workflow: Option<String>,
    #[serde(default, alias = "title", alias = "phaseTitle")]
    phase: Option<String>,
    #[serde(default, alias = "label")]
    agent: Option<String>,
    #[serde(default, alias = "agentId")]
    agent_id: Option<String>,
    #[serde(default)]
    child: Option<String>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    index: Option<u64>,
    #[serde(default)]
    #[serde(alias = "childIndex")]
    child_index: Option<u64>,
    #[serde(default)]
    #[serde(alias = "childRunId")]
    child_run_id: Option<String>,
    #[serde(default)]
    #[serde(alias = "itemIndex")]
    item_index: Option<u64>,
    #[serde(default)]
    #[serde(alias = "stageIndex")]
    stage_index: Option<u64>,
    #[serde(default)]
    #[serde(alias = "stepIndex")]
    step_index: Option<u64>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default, alias = "promptPreview")]
    prompt_preview: Option<String>,
    #[serde(default, alias = "resultPreview")]
    result_preview: Option<String>,
    #[serde(default)]
    data: Option<serde_json::Value>,
}

struct WorkflowProgressSummaryItem {
    name: String,
    status: String,
    unix_ms: Option<u128>,
    message: Option<String>,
}

struct WorkflowRunMetrics {
    agent_count: Option<u64>,
    child_count: Option<u64>,
    log_count: Option<u64>,
    log_suppressed: bool,
    failure_count: Option<u64>,
}

struct WorkflowAgentDetailItem {
    key: String,
    name: String,
    agent_id: Option<String>,
    phase: Option<String>,
    status: String,
    unix_ms: Option<u128>,
    index: Option<u64>,
    prompt_preview: Option<String>,
    result_preview: Option<String>,
    error: Option<String>,
}

struct WorkflowAgentTranscript {
    prompt: Option<String>,
    reasoning: Vec<String>,
    final_text: Option<String>,
    tool_calls: Vec<WorkflowAgentTranscriptToolCall>,
    invalid: usize,
}

struct WorkflowAgentMetadata {
    fields: Vec<(String, String)>,
}

struct WorkflowAgentTranscriptToolCall {
    id: Option<String>,
    assistant_uuid: Option<String>,
    name: String,
    summary: Option<String>,
    output: Option<String>,
}

struct StoredWorkflowRunItem {
    run: StoredWorkflowRun,
    sort_key: u128,
}

struct StoredWorkflowRunListing {
    dir: PathBuf,
    runs: Vec<StoredWorkflowRun>,
    skipped: usize,
}

struct StoredWorkflowDefinition {
    invocation_name: String,
    metadata_name: String,
    description: String,
    when_to_use: Option<String>,
    input_schema: Option<String>,
    phases: Vec<StoredWorkflowPhaseDefinition>,
    source_label: &'static str,
    config_enabled: Option<bool>,
    config_approval: Option<WorkflowApproval>,
    path: PathBuf,
}

struct StoredWorkflowDefinitionListing {
    definitions: Vec<StoredWorkflowDefinition>,
    skipped: usize,
    shadowed: usize,
}

struct StoredWorkflowPhaseDefinition {
    title: String,
    model: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum JsScanState {
    Code,
    SingleString,
    DoubleString,
    TemplateString,
    LineComment,
    BlockComment,
}

fn system_time_millis(time: SystemTime) -> Option<u128> {
    time.duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_millis())
}

fn non_empty_trimmed(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then_some(trimmed)
}

fn workflow_status_is_active(status: &str) -> bool {
    matches!(status, "running" | "paused")
}

fn compact_workflow_run_preview(value: &str) -> String {
    const PREVIEW_MAX_CHARS: usize = 180;
    let compact = value
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" / ");
    if compact.chars().count() <= PREVIEW_MAX_CHARS {
        return compact;
    }
    let mut truncated = compact.chars().take(PREVIEW_MAX_CHARS).collect::<String>();
    truncated.push_str("...");
    truncated
}

fn is_safe_workflow_run_id(run_id: &str) -> bool {
    !run_id.is_empty()
        && run_id.len() <= 128
        && run_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn read_workflow_definition_prefix(path: &Path) -> Result<String, std::io::Error> {
    let file = fs::File::open(path)?;
    let mut bytes = Vec::new();
    file.take(WORKFLOW_DEFINITION_READ_LIMIT_BYTES as u64)
        .read_to_end(&mut bytes)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn workflow_invocation_name_from_path(path: &Path) -> Option<String> {
    if path.file_name().and_then(|name| name.to_str()) == Some("workflow.js") {
        return path
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            .filter(|name| is_valid_workflow_invocation_name(name))
            .map(ToString::to_string);
    }
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|name| is_valid_workflow_invocation_name(name))
        .map(ToString::to_string)
}

fn is_valid_workflow_invocation_name(name: &str) -> bool {
    let trimmed = name.trim();
    !trimmed.is_empty()
        && trimmed == name
        && !trimmed.starts_with('.')
        && !trimmed.contains('/')
        && !trimmed.contains('\\')
}

fn is_valid_saved_workflow_name(name: &str) -> bool {
    is_valid_workflow_invocation_name(name)
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn normalize_workflows_run_args(args: &str) -> String {
    let trimmed = args.trim();
    if trimmed.is_empty() || serde_json::from_str::<serde_json::Value>(trimmed).is_ok() {
        return trimmed.to_string();
    }
    let Some(tokens) = shlex::split(trimmed) else {
        return trimmed.to_string();
    };
    let [token] = tokens.as_slice() else {
        return trimmed.to_string();
    };
    if serde_json::from_str::<serde_json::Value>(token).is_ok() {
        token.clone()
    } else {
        trimmed.to_string()
    }
}

fn workflow_definition_sort_key(path: &Path) -> String {
    workflow_invocation_name_from_path(path).unwrap_or_else(|| path.display().to_string())
}

fn workflow_definition_candidates_for_dir(dir: &Path) -> (Vec<PathBuf>, usize) {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return (Vec::new(), 0),
        Err(_) => return (Vec::new(), 1),
    };

    let mut direct_files = Vec::new();
    let mut folder_workflows = Vec::new();
    let mut skipped = 0usize;
    for entry in entries {
        let Ok(entry) = entry else {
            skipped += 1;
            continue;
        };
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            skipped += 1;
            continue;
        };
        if file_type.is_file() && path.extension().and_then(|ext| ext.to_str()) == Some("js") {
            direct_files.push(path);
            continue;
        }
        if file_type.is_dir() {
            let workflow_path = path.join("workflow.js");
            if workflow_path.is_file() {
                folder_workflows.push(workflow_path);
            }
        }
    }

    direct_files.sort_by_key(|path| workflow_definition_sort_key(path));
    folder_workflows.sort_by_key(|path| workflow_definition_sort_key(path));
    direct_files.extend(folder_workflows);
    (direct_files, skipped)
}

fn parse_workflow_definition_for_listing(
    path: &Path,
    namespace: Option<&str>,
    source_label: &'static str,
) -> Option<StoredWorkflowDefinition> {
    let base_invocation_name = workflow_invocation_name_from_path(path)?;
    let invocation_name = workflow_invocation_name(namespace, &base_invocation_name);
    let contents = read_workflow_definition_prefix(path).ok()?;
    let metadata = parse_workflow_metadata_for_listing(&contents)?;
    Some(StoredWorkflowDefinition {
        invocation_name,
        metadata_name: metadata.name,
        description: metadata.description,
        when_to_use: metadata.when_to_use,
        input_schema: metadata.input_schema,
        phases: metadata.phases,
        source_label,
        config_enabled: None,
        config_approval: None,
        path: path.to_path_buf(),
    })
}

fn workflow_invocation_name(namespace: Option<&str>, name: &str) -> String {
    match namespace {
        Some(namespace) if !namespace.is_empty() => format!("{namespace}:{name}"),
        _ => name.to_string(),
    }
}

fn workflow_source_label(namespace: Option<&str>, dir: &Path) -> &'static str {
    match namespace {
        Some(namespace) if !namespace.is_empty() => "[Plugin Workflow]",
        _ if is_system_workflow_dir(dir) => "[System Workflow]",
        _ => "[Workflow]",
    }
}

fn is_system_workflow_dir(dir: &Path) -> bool {
    dir.file_name().and_then(|name| name.to_str()) == Some(".system")
        && dir
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            == Some("workflows")
}

struct StoredWorkflowMetadata {
    name: String,
    description: String,
    when_to_use: Option<String>,
    input_schema: Option<String>,
    phases: Vec<StoredWorkflowPhaseDefinition>,
}

fn parse_workflow_metadata_for_listing(code: &str) -> Option<StoredWorkflowMetadata> {
    let trimmed_start = code.len().saturating_sub(code.trim_start().len());
    let trimmed = &code[trimmed_start..];
    let prefix = "export const meta";
    if !trimmed.starts_with(prefix) {
        return None;
    }

    let mut cursor = trimmed_start + prefix.len();
    cursor = skip_js_whitespace(code, cursor);
    if !code[cursor..].starts_with('=') {
        return None;
    }
    cursor += '='.len_utf8();
    cursor = skip_js_whitespace(code, cursor);
    if !code[cursor..].starts_with('{') {
        return None;
    }

    let meta_end = find_matching_workflow_meta_brace(code, cursor)?;
    let meta_literal = &code[cursor..=meta_end];
    let name = extract_meta_string_field(meta_literal, "name")?;
    let description = extract_meta_string_field(meta_literal, "description")?;
    Some(StoredWorkflowMetadata {
        name,
        description,
        when_to_use: extract_meta_string_field(meta_literal, "whenToUse"),
        input_schema: extract_meta_literal_field(meta_literal, "inputSchema")?,
        phases: extract_meta_phases(meta_literal)?,
    })
}

fn find_matching_workflow_meta_brace(source: &str, open_index: usize) -> Option<usize> {
    let mut state = JsScanState::Code;
    let mut depth = 0usize;
    let mut escaped = false;
    let mut chars = source[open_index..].char_indices().peekable();

    while let Some((relative_index, ch)) = chars.next() {
        let index = open_index + relative_index;
        match state {
            JsScanState::Code => match ch {
                '{' => depth += 1,
                '}' => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return Some(index);
                    }
                }
                '\'' => state = JsScanState::SingleString,
                '"' => state = JsScanState::DoubleString,
                '`' => state = JsScanState::TemplateString,
                '/' => match chars.peek().map(|(_, next)| *next) {
                    Some('/') => {
                        chars.next();
                        state = JsScanState::LineComment;
                    }
                    Some('*') => {
                        chars.next();
                        state = JsScanState::BlockComment;
                    }
                    _ => {}
                },
                _ => {}
            },
            JsScanState::SingleString => scan_string_char(ch, '\'', &mut escaped, &mut state),
            JsScanState::DoubleString => scan_string_char(ch, '"', &mut escaped, &mut state),
            JsScanState::TemplateString => scan_string_char(ch, '`', &mut escaped, &mut state),
            JsScanState::LineComment => {
                if ch == '\n' {
                    state = JsScanState::Code;
                }
            }
            JsScanState::BlockComment => {
                if ch == '*' && chars.peek().is_some_and(|(_, next)| *next == '/') {
                    chars.next();
                    state = JsScanState::Code;
                }
            }
        }
    }

    None
}

fn skip_js_whitespace(source: &str, mut index: usize) -> usize {
    while let Some(ch) = source[index..].chars().next() {
        if !ch.is_whitespace() {
            break;
        }
        index += ch.len_utf8();
    }
    index
}

fn find_meta_field_value_index(meta_literal: &str, field: &str) -> Option<usize> {
    for key in [
        field.to_string(),
        format!("'{field}'"),
        format!("\"{field}\""),
    ] {
        let mut search_start = 0usize;
        while let Some(relative_index) = meta_literal[search_start..].find(key.as_str()) {
            let key_index = search_start + relative_index;
            if key == field && !has_identifier_boundaries(meta_literal, key_index, field.len()) {
                search_start = key_index + key.len();
                continue;
            }
            if !is_code_position(meta_literal, key_index) {
                search_start = key_index + key.len();
                continue;
            }

            let mut value_index = key_index + key.len();
            value_index = skip_js_whitespace(meta_literal, value_index);
            if !meta_literal[value_index..].starts_with(':') {
                search_start = key_index + key.len();
                continue;
            }
            value_index += ':'.len_utf8();
            value_index = skip_js_whitespace(meta_literal, value_index);
            return Some(value_index);
        }
    }
    None
}

fn extract_meta_string_field(meta_literal: &str, field: &str) -> Option<String> {
    let value_index = find_meta_field_value_index(meta_literal, field)?;
    parse_js_string_literal(meta_literal, value_index).map(|(value, _end_index)| value)
}

fn extract_meta_literal_field(meta_literal: &str, field: &str) -> Option<Option<String>> {
    let Some(value_index) = find_meta_field_value_index(meta_literal, field) else {
        return Some(None);
    };
    let open = meta_literal[value_index..].chars().next()?;
    let close = match open {
        '{' => '}',
        '[' => ']',
        _ => return None,
    };
    let end = find_matching_workflow_delimiter(meta_literal, value_index, open, close)?;
    Some(Some(meta_literal[value_index..=end].trim().to_string()))
}

fn extract_meta_phases(meta_literal: &str) -> Option<Vec<StoredWorkflowPhaseDefinition>> {
    let Some(value_index) = find_meta_field_value_index(meta_literal, "phases") else {
        return Some(Vec::new());
    };
    if !meta_literal[value_index..].starts_with('[') {
        return None;
    }
    let array_end = find_matching_workflow_delimiter(meta_literal, value_index, '[', ']')?;
    let mut phases = Vec::new();
    let mut cursor = value_index + '['.len_utf8();
    while cursor < array_end {
        cursor = skip_js_whitespace(meta_literal, cursor);
        if cursor >= array_end {
            break;
        }
        if meta_literal[cursor..].starts_with(',') {
            cursor += ','.len_utf8();
            continue;
        }
        if !meta_literal[cursor..].starts_with('{') {
            return None;
        }
        let object_end = find_matching_workflow_delimiter(meta_literal, cursor, '{', '}')?;
        let entry = &meta_literal[cursor..=object_end];
        let title = extract_meta_string_field(entry, "title")?;
        let model = extract_meta_string_field(entry, "model");
        phases.push(StoredWorkflowPhaseDefinition { title, model });
        cursor = object_end + '}'.len_utf8();
        cursor = skip_js_whitespace(meta_literal, cursor);
        if cursor < array_end && !meta_literal[cursor..].starts_with(',') {
            return None;
        }
    }
    Some(phases)
}

fn find_matching_workflow_delimiter(
    source: &str,
    open_index: usize,
    open: char,
    close: char,
) -> Option<usize> {
    let mut state = JsScanState::Code;
    let mut depth = 0usize;
    let mut escaped = false;
    let mut chars = source[open_index..].char_indices().peekable();

    while let Some((relative_index, ch)) = chars.next() {
        let index = open_index + relative_index;
        match state {
            JsScanState::Code => {
                if ch == open {
                    depth += 1;
                } else if ch == close {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return Some(index);
                    }
                } else {
                    match ch {
                        '\'' => state = JsScanState::SingleString,
                        '"' => state = JsScanState::DoubleString,
                        '`' => state = JsScanState::TemplateString,
                        '/' => match chars.peek().map(|(_, next)| *next) {
                            Some('/') => {
                                chars.next();
                                state = JsScanState::LineComment;
                            }
                            Some('*') => {
                                chars.next();
                                state = JsScanState::BlockComment;
                            }
                            _ => {}
                        },
                        _ => {}
                    }
                }
            }
            JsScanState::SingleString => scan_string_char(ch, '\'', &mut escaped, &mut state),
            JsScanState::DoubleString => scan_string_char(ch, '"', &mut escaped, &mut state),
            JsScanState::TemplateString => scan_string_char(ch, '`', &mut escaped, &mut state),
            JsScanState::LineComment => {
                if ch == '\n' {
                    state = JsScanState::Code;
                }
            }
            JsScanState::BlockComment => {
                if ch == '*' && chars.peek().is_some_and(|(_, next)| *next == '/') {
                    chars.next();
                    state = JsScanState::Code;
                }
            }
        }
    }

    None
}

fn has_identifier_boundaries(source: &str, start: usize, len: usize) -> bool {
    let before = source[..start].chars().next_back();
    let after = source[start + len..].chars().next();
    !before.is_some_and(is_js_identifier_char) && !after.is_some_and(is_js_identifier_char)
}

fn parse_js_string_literal(source: &str, start: usize) -> Option<(String, usize)> {
    let quote = source[start..].chars().next()?;
    if quote != '\'' && quote != '"' {
        return None;
    }

    let mut value = String::new();
    let mut escaped = false;
    for (relative_index, ch) in source[start + quote.len_utf8()..].char_indices() {
        let index = start + quote.len_utf8() + relative_index;
        if escaped {
            value.push(ch);
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if ch == quote {
            return Some((value, index + ch.len_utf8()));
        }
        value.push(ch);
    }
    None
}

fn is_code_position(source: &str, target_index: usize) -> bool {
    let mut state = JsScanState::Code;
    let mut escaped = false;
    let mut chars = source.char_indices().peekable();

    while let Some((index, ch)) = chars.next() {
        if index >= target_index {
            return state == JsScanState::Code;
        }
        match state {
            JsScanState::Code => match ch {
                '\'' => state = JsScanState::SingleString,
                '"' => state = JsScanState::DoubleString,
                '`' => state = JsScanState::TemplateString,
                '/' => match chars.peek().map(|(_, next)| *next) {
                    Some('/') => {
                        chars.next();
                        state = JsScanState::LineComment;
                    }
                    Some('*') => {
                        chars.next();
                        state = JsScanState::BlockComment;
                    }
                    _ => {}
                },
                _ => {}
            },
            JsScanState::SingleString => scan_string_char(ch, '\'', &mut escaped, &mut state),
            JsScanState::DoubleString => scan_string_char(ch, '"', &mut escaped, &mut state),
            JsScanState::TemplateString => scan_string_char(ch, '`', &mut escaped, &mut state),
            JsScanState::LineComment => {
                if ch == '\n' {
                    state = JsScanState::Code;
                }
            }
            JsScanState::BlockComment => {
                if ch == '*' && chars.peek().is_some_and(|(_, next)| *next == '/') {
                    chars.next();
                    state = JsScanState::Code;
                }
            }
        }
    }

    state == JsScanState::Code
}

fn scan_string_char(ch: char, quote: char, escaped: &mut bool, state: &mut JsScanState) {
    if *escaped {
        *escaped = false;
    } else if ch == '\\' {
        *escaped = true;
    } else if ch == quote {
        *state = JsScanState::Code;
    }
}

fn is_js_identifier_char(ch: char) -> bool {
    ch == '_' || ch == '$' || ch.is_ascii_alphanumeric()
}

impl ChatWidget {
    pub(super) fn apply_workflow_slash_arg(&mut self, arg: &str) {
        match arg.trim().to_ascii_lowercase().as_str() {
            "on" | "enable" | "enabled" | "dynamic" => {
                self.set_workflow_mode_from_user_action(WorkflowMode::Dynamic);
                self.add_info_message(
                    "Dynamic workflows enabled for this thread.".to_string(),
                    Some("The model can use the workflow tool to orchestrate JS, tools, and subagents.".to_string()),
                );
            }
            "off" | "disable" | "disabled" => {
                self.set_workflow_mode_from_user_action(WorkflowMode::Disabled);
                self.add_info_message("Workflows disabled for this thread.".to_string(), None);
            }
            "ultracode" => {
                self.set_ultracode_from_user_action();
                self.add_info_message(
                    "Ultracode enabled for this thread.".to_string(),
                    Some("Uses xhigh reasoning plus workflow orchestration.".to_string()),
                );
            }
            "status" | "" => self.add_workflows_output(/*include_runs*/ false),
            _ => self.add_error_message(WORKFLOW_USAGE.to_string()),
        }
    }

    fn workflow_mode_label(mode: WorkflowMode) -> &'static str {
        match mode {
            WorkflowMode::Disabled => "disabled",
            WorkflowMode::Dynamic => "dynamic",
            WorkflowMode::Ultracode => "ultracode",
        }
    }

    fn workflow_approval_label(approval: WorkflowApproval) -> &'static str {
        match approval {
            WorkflowApproval::Auto => "auto",
            WorkflowApproval::Ask => "ask",
            WorkflowApproval::Allow => "allow",
            WorkflowApproval::Deny => "deny",
        }
    }

    fn workflow_approval_from_slash_value(value: &str) -> Option<Option<WorkflowApproval>> {
        match value.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(Some(WorkflowApproval::Auto)),
            "ask" | "prompt" => Some(Some(WorkflowApproval::Ask)),
            "allow" | "approve" => Some(Some(WorkflowApproval::Allow)),
            "deny" | "block" | "never" => Some(Some(WorkflowApproval::Deny)),
            "clear" | "default" | "inherit" => Some(None),
            _ => None,
        }
    }

    fn workflow_enabled_from_slash_value(value: &str) -> Option<Option<bool>> {
        match value.trim().to_ascii_lowercase().as_str() {
            "on" | "true" | "yes" | "enable" | "enabled" => Some(Some(true)),
            "off" | "false" | "no" | "disable" | "disabled" => Some(Some(false)),
            "clear" | "default" | "inherit" => Some(None),
            _ => None,
        }
    }

    pub(super) fn add_workflows_output(&mut self, include_runs: bool) {
        self.sync_workflow_slash_commands();
        let effective = self.effective_collaboration_mode();
        let mode = effective.workflow_mode();
        let runtime = if self.config.workflows.enabled {
            "enabled"
        } else {
            "disabled by config"
        };
        let dirs = if self.config.workflows.workflow_dirs.is_empty() {
            "none".to_string()
        } else {
            self.config
                .workflows
                .workflow_dirs
                .iter()
                .map(|dir| format!("- {}", dir.display()))
                .collect::<Vec<_>>()
                .join("\n")
        };
        let plugin_dirs = if self.config.workflows.plugin_workflow_dirs.is_empty() {
            "none".to_string()
        } else {
            self.config
                .workflows
                .plugin_workflow_dirs
                .iter()
                .map(|source| {
                    format!(
                        "- {}: {} ({})",
                        source.namespace,
                        source.dir.display(),
                        source.plugin_id
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
        let approval = Self::workflow_approval_label(self.config.workflows.approval);
        let mut message = format!(
            "Workflows: {}\nRuntime: {runtime}\nApproval: {approval}\nDirectories:\n{dirs}\nPlugin directories:\n{plugin_dirs}",
            Self::workflow_mode_label(mode)
        );
        if include_runs {
            message.push_str("\n\n");
            Self::append_workflow_definition_listing(
                &mut message,
                self.load_workflow_definitions(WORKFLOW_DEFINITION_LIST_LIMIT),
            );
            message.push_str("\n\n");
            let active_listing = self.load_active_workflow_runs(WORKFLOW_RUN_LIST_LIMIT);
            let active_run_ids = active_listing
                .as_ref()
                .map(|listing| {
                    listing
                        .runs
                        .iter()
                        .map(|run| run.run_id.clone())
                        .collect::<HashSet<_>>()
                })
                .unwrap_or_default();
            match active_listing {
                Ok(listing) => Self::append_workflow_active_run_listing(&mut message, listing),
                Err(err) => {
                    message.push_str("Active runs:\n");
                    message.push_str(&format!("- unavailable: {err}"));
                }
            }
            message.push_str("\n\n");
            match self.load_recent_workflow_runs(WORKFLOW_RUN_LIST_LIMIT) {
                Ok(mut listing) => {
                    if !active_run_ids.is_empty() {
                        listing
                            .runs
                            .retain(|run| !active_run_ids.contains(run.run_id.as_str()));
                    }
                    Self::append_workflow_run_listing(&mut message, listing)
                }
                Err(err) => {
                    message.push_str("Recent runs:\n");
                    message.push_str(&format!("- unavailable: {err}"));
                }
            }
        }
        self.add_info_message(
            message,
            Some("Use /workflow on|off|ultracode or /effort ultracode.".to_string()),
        );
    }

    fn load_workflow_definitions(&self, limit: usize) -> StoredWorkflowDefinitionListing {
        let mut definitions = Vec::new();
        let mut skipped = 0usize;
        let mut shadowed = 0usize;
        let mut seen_names = HashSet::new();

        for (namespace, source_label, dir) in self.workflow_definition_dirs() {
            let (candidates, dir_skipped) = workflow_definition_candidates_for_dir(dir.as_path());
            skipped += dir_skipped;
            for candidate in candidates {
                let Some(base_invocation_name) = workflow_invocation_name_from_path(&candidate)
                else {
                    skipped += 1;
                    continue;
                };
                let invocation_name =
                    workflow_invocation_name(namespace.as_deref(), &base_invocation_name);
                if !seen_names.insert(invocation_name) {
                    shadowed += 1;
                    continue;
                }
                let Some(mut definition) = parse_workflow_definition_for_listing(
                    &candidate,
                    namespace.as_deref(),
                    source_label,
                ) else {
                    skipped += 1;
                    continue;
                };
                if let Some(config) = self.config.workflows.named.get(&definition.invocation_name) {
                    definition.config_enabled = config.enabled;
                    definition.config_approval = config.approval;
                }
                if definitions.len() < limit {
                    definitions.push(definition);
                }
            }
        }

        StoredWorkflowDefinitionListing {
            definitions,
            skipped,
            shadowed,
        }
    }

    fn workflow_definition_dirs(
        &self,
    ) -> Vec<(
        Option<String>,
        &'static str,
        &codex_utils_absolute_path::AbsolutePathBuf,
    )> {
        let mut dirs = Vec::new();
        dirs.extend(
            self.config
                .workflows
                .workflow_dirs
                .iter()
                .map(|dir| (None, workflow_source_label(None, dir.as_path()), dir)),
        );
        dirs.extend(
            self.config
                .workflows
                .plugin_workflow_dirs
                .iter()
                .map(|source| {
                    (
                        Some(source.namespace.clone()),
                        workflow_source_label(Some(&source.namespace), source.dir.as_path()),
                        &source.dir,
                    )
                }),
        );
        dirs
    }

    pub(super) fn sync_workflow_slash_commands(&mut self) {
        self.bottom_pane
            .set_workflow_slash_commands(self.current_workflow_slash_commands());
    }

    pub(super) fn current_workflow_slash_commands(&self) -> Vec<WorkflowSlashCommand> {
        self.load_workflow_definitions(usize::MAX)
            .definitions
            .into_iter()
            .map(|definition| WorkflowSlashCommand {
                name: definition.invocation_name,
                description: definition.when_to_use.unwrap_or(definition.description),
                input_schema: definition.input_schema,
                source_label: Some(definition.source_label.to_string()),
            })
            .collect()
    }

    pub(crate) fn workflow_policy_items_for_settings(&self) -> Vec<WorkflowNamedPolicyItem> {
        let mut names = self
            .config
            .workflows
            .named
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();
        names.extend(
            self.load_workflow_definitions(usize::MAX)
                .definitions
                .into_iter()
                .map(|definition| definition.invocation_name),
        );
        names
            .into_iter()
            .map(|name| {
                let config = self.config.workflows.named.get(&name);
                WorkflowNamedPolicyItem {
                    name,
                    enabled: config.and_then(|config| config.enabled),
                    approval: config.and_then(|config| config.approval),
                }
            })
            .collect()
    }

    fn append_workflow_definition_listing(
        message: &mut String,
        listing: StoredWorkflowDefinitionListing,
    ) {
        message.push_str("Available workflows:");
        if listing.definitions.is_empty() {
            message.push_str("\n- none");
        } else {
            for definition in listing.definitions {
                message.push('\n');
                message.push_str(&Self::format_workflow_definition_line(&definition));
            }
        }
        if listing.shadowed > 0 {
            message.push_str(&format!(
                "\nShadowed workflow definitions: {}",
                listing.shadowed
            ));
        }
        if listing.skipped > 0 {
            message.push_str(&format!(
                "\nSkipped invalid workflow definitions: {}",
                listing.skipped
            ));
        }
    }

    fn format_workflow_definition_line(definition: &StoredWorkflowDefinition) -> String {
        let mut line = format!("- `{}`", definition.invocation_name);
        if definition.metadata_name != definition.invocation_name {
            line.push_str(&format!(" (meta `{}`)", definition.metadata_name));
        }
        line.push_str(&format!(
            " - {} ({})",
            definition.description,
            definition.path.display()
        ));
        if let Some(when_to_use) = definition
            .when_to_use
            .as_deref()
            .and_then(non_empty_trimmed)
        {
            line.push_str(&format!("; when: {when_to_use}"));
        }
        if let Some(input_schema) = definition
            .input_schema
            .as_deref()
            .and_then(non_empty_trimmed)
            .map(compact_workflow_run_preview)
        {
            line.push_str(&format!("; input: {input_schema}"));
        }
        line.push_str(&format!("; source: {}", definition.source_label));
        if !definition.phases.is_empty() {
            let phases = definition
                .phases
                .iter()
                .map(|phase| {
                    if let Some(model) = phase.model.as_deref().and_then(non_empty_trimmed) {
                        format!("{} [{}]", phase.title, model)
                    } else {
                        phase.title.clone()
                    }
                })
                .collect::<Vec<_>>()
                .join(", ");
            line.push_str(&format!("; phases: {phases}"));
        }
        let mut policy_parts = Vec::new();
        if let Some(enabled) = definition.config_enabled {
            policy_parts.push(format!(
                "policy: {}",
                if enabled { "enabled" } else { "disabled" }
            ));
        }
        if let Some(approval) = definition.config_approval {
            policy_parts.push(format!(
                "approval: {}",
                Self::workflow_approval_label(approval)
            ));
        }
        if !policy_parts.is_empty() {
            line.push_str(&format!("; {}", policy_parts.join("; ")));
        }
        line
    }

    fn workflow_runs_dir(&self) -> PathBuf {
        self.config.codex_home.join(WORKFLOW_RUNS_DIR).to_path_buf()
    }

    fn workflow_active_runs_dir(&self) -> PathBuf {
        self.workflow_runs_dir().join(WORKFLOW_ACTIVE_RUNS_DIR)
    }

    fn load_active_workflow_runs(&self, limit: usize) -> Result<StoredWorkflowRunListing, String> {
        self.load_workflow_runs_from_dir(self.workflow_active_runs_dir(), limit)
    }

    fn load_recent_workflow_runs(&self, limit: usize) -> Result<StoredWorkflowRunListing, String> {
        self.load_workflow_runs_from_dir(self.workflow_runs_dir(), limit)
    }

    fn load_workflow_runs_from_dir(
        &self,
        dir: PathBuf,
        limit: usize,
    ) -> Result<StoredWorkflowRunListing, String> {
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Ok(StoredWorkflowRunListing {
                    dir,
                    runs: Vec::new(),
                    skipped: 0,
                });
            }
            Err(err) => {
                return Err(format!("failed to read {}: {err}", dir.display()));
            }
        };

        let mut items = Vec::new();
        let mut seen_run_ids = HashSet::new();
        let mut skipped = 0usize;
        for entry in entries {
            let Ok(entry) = entry else {
                skipped += 1;
                continue;
            };
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            let sort_key = entry
                .metadata()
                .ok()
                .and_then(|metadata| metadata.modified().ok())
                .and_then(system_time_millis)
                .unwrap_or_default();
            let Ok(contents) = fs::read_to_string(&path) else {
                skipped += 1;
                continue;
            };
            let Ok(run) = serde_json::from_str::<StoredWorkflowRun>(&contents) else {
                skipped += 1;
                continue;
            };
            let run = Self::normalize_stored_workflow_run(run, Some(path.as_path()));
            let sort_key = Self::workflow_run_sort_key(&run, sort_key);
            let run_id = run.run_id.trim().to_string();
            if !run_id.is_empty() {
                seen_run_ids.insert(run_id);
            }
            items.push(StoredWorkflowRunItem { run, sort_key });
        }

        let (transcript_items, transcript_skipped) =
            Self::load_workflow_transcript_run_items(&dir, &seen_run_ids)?;
        skipped += transcript_skipped;
        for item in &transcript_items {
            let run_id = item.run.run_id.trim();
            if !run_id.is_empty() {
                seen_run_ids.insert(run_id.to_string());
            }
        }
        items.extend(transcript_items);

        let (claude_items, claude_skipped) =
            Self::load_claude_native_workflow_run_items(&dir, &seen_run_ids)?;
        skipped += claude_skipped;
        items.extend(claude_items);

        items.sort_by_key(|item| std::cmp::Reverse(item.sort_key));
        let runs = items
            .into_iter()
            .take(limit)
            .map(|item| item.run)
            .collect::<Vec<_>>();
        Ok(StoredWorkflowRunListing { dir, runs, skipped })
    }

    fn load_workflow_transcript_run_items(
        dir: &Path,
        seen_run_ids: &HashSet<String>,
    ) -> Result<(Vec<StoredWorkflowRunItem>, usize), String> {
        if dir.file_name().and_then(|name| name.to_str()) == Some(WORKFLOW_ACTIVE_RUNS_DIR) {
            return Ok((Vec::new(), 0));
        }
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok((Vec::new(), 0)),
            Err(err) => return Err(format!("failed to read {}: {err}", dir.display())),
        };

        let mut items = Vec::new();
        let mut seen_run_ids = seen_run_ids.clone();
        let mut skipped = 0usize;
        for entry in entries {
            let Ok(entry) = entry else {
                skipped += 1;
                continue;
            };
            let run_dir = entry.path();
            let Ok(metadata) = entry.metadata() else {
                skipped += 1;
                continue;
            };
            if !metadata.is_dir() {
                continue;
            }
            let path = run_dir
                .join(WORKFLOW_TRANSCRIPT_DIR)
                .join(WORKFLOW_TRANSCRIPT_RUN_FILE);
            if !path.exists() {
                continue;
            }
            let sort_key = fs::metadata(&path)
                .ok()
                .and_then(|metadata| metadata.modified().ok())
                .and_then(system_time_millis)
                .unwrap_or_default();
            let Ok(contents) = fs::read_to_string(&path) else {
                skipped += 1;
                continue;
            };
            let Ok(run) = serde_json::from_str::<StoredWorkflowRun>(&contents) else {
                skipped += 1;
                continue;
            };
            let run = Self::normalize_stored_workflow_run(run, Some(run_dir.as_path()));
            let run_id = run.run_id.trim().to_string();
            if seen_run_ids.contains(&run_id) {
                continue;
            }
            if !run_id.is_empty() {
                seen_run_ids.insert(run_id);
            }
            let sort_key = Self::workflow_run_sort_key(&run, sort_key);
            items.push(StoredWorkflowRunItem { run, sort_key });
        }
        Ok((items, skipped))
    }

    fn load_claude_native_workflow_run_items(
        dir: &Path,
        seen_run_ids: &HashSet<String>,
    ) -> Result<(Vec<StoredWorkflowRunItem>, usize), String> {
        if dir.file_name().and_then(|name| name.to_str()) == Some(WORKFLOW_ACTIVE_RUNS_DIR) {
            return Ok((Vec::new(), 0));
        }
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok((Vec::new(), 0)),
            Err(err) => return Err(format!("failed to read {}: {err}", dir.display())),
        };

        let mut items = Vec::new();
        let mut seen_run_ids = seen_run_ids.clone();
        let mut skipped = 0usize;
        for entry in entries {
            let Ok(entry) = entry else {
                skipped += 1;
                continue;
            };
            let session_dir = entry.path();
            let Ok(metadata) = entry.metadata() else {
                skipped += 1;
                continue;
            };
            if !metadata.is_dir()
                || session_dir.file_name().and_then(|name| name.to_str())
                    == Some(WORKFLOW_ACTIVE_RUNS_DIR)
            {
                continue;
            }
            let workflows_dir = session_dir.join(CLAUDE_WORKFLOW_SNAPSHOT_DIR);
            let workflow_entries = match fs::read_dir(&workflows_dir) {
                Ok(entries) => entries,
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
                Err(_) => {
                    skipped += 1;
                    continue;
                }
            };
            for workflow_entry in workflow_entries {
                let Ok(workflow_entry) = workflow_entry else {
                    skipped += 1;
                    continue;
                };
                let path = workflow_entry.path();
                if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                    continue;
                }
                let sort_key = workflow_entry
                    .metadata()
                    .ok()
                    .and_then(|metadata| metadata.modified().ok())
                    .and_then(system_time_millis)
                    .unwrap_or_default();
                let Ok(contents) = fs::read_to_string(&path) else {
                    skipped += 1;
                    continue;
                };
                let Ok(run) = serde_json::from_str::<StoredWorkflowRun>(&contents) else {
                    skipped += 1;
                    continue;
                };
                let run = Self::normalize_stored_workflow_run(run, Some(path.as_path()));
                let run_id = run.run_id.trim().to_string();
                if seen_run_ids.contains(&run_id) {
                    continue;
                }
                if !run_id.is_empty() {
                    seen_run_ids.insert(run_id);
                }
                let sort_key = Self::workflow_run_sort_key(&run, sort_key);
                items.push(StoredWorkflowRunItem { run, sort_key });
            }
        }
        Ok((items, skipped))
    }

    fn workflow_run_sort_key(run: &StoredWorkflowRun, fallback: u128) -> u128 {
        run.updated_unix_ms
            .or_else(|| run.progress.last().and_then(|event| event.unix_ms))
            .or(run.ended_unix_ms)
            .or(run.started_unix_ms)
            .unwrap_or(fallback)
    }

    fn normalize_stored_workflow_run(
        mut run: StoredWorkflowRun,
        path: Option<&Path>,
    ) -> StoredWorkflowRun {
        let artifact_dirs = path.and_then(Self::workflow_run_artifact_dirs_from_path);
        if run.run_id.trim().is_empty() {
            run.run_id = path
                .and_then(Path::file_stem)
                .and_then(|stem| stem.to_str())
                .unwrap_or("unknown")
                .to_string();
        }
        if run.workflow_name.trim().is_empty() {
            run.workflow_name = "(unnamed)".to_string();
        }
        if run.status.trim().is_empty() {
            run.status = "unknown".to_string();
        }
        if run.source.is_none() {
            let source_kind = run.source_kind.take().and_then(|kind| {
                non_empty_trimmed(kind.as_str()).map(std::string::ToString::to_string)
            });
            let source_name = run.source_name.take().and_then(|name| {
                non_empty_trimmed(name.as_str()).map(std::string::ToString::to_string)
            });
            let source_path = run.source_path.take().and_then(|path| {
                non_empty_trimmed(path.as_str()).map(std::string::ToString::to_string)
            });
            if source_kind.is_some() || source_name.is_some() || source_path.is_some() {
                run.source = Some(StoredWorkflowRunSource {
                    kind: source_kind.unwrap_or_else(|| "unknown".to_string()),
                    name: source_name,
                    path: source_path,
                });
            }
        }
        if run.run_dir.as_deref().and_then(non_empty_trimmed).is_none()
            && let Some((run_dir, _)) = artifact_dirs.as_ref()
        {
            run.run_dir = Some(run_dir.display().to_string());
        }
        if run
            .transcript_dir
            .as_deref()
            .and_then(non_empty_trimmed)
            .is_none()
            && let Some((_, transcript_dir)) = artifact_dirs.as_ref()
        {
            run.transcript_dir = Some(transcript_dir.display().to_string());
        }
        run.journal_summary = run
            .run_dir
            .as_deref()
            .and_then(non_empty_trimmed)
            .and_then(Self::load_workflow_agent_journal_summary);
        run
    }

    fn workflow_run_artifact_dirs_from_path(path: &Path) -> Option<(PathBuf, PathBuf)> {
        if path.file_name().and_then(|name| name.to_str()) == Some(WORKFLOW_TRANSCRIPT_RUN_FILE) {
            let transcript_dir = path.parent()?;
            if transcript_dir.file_name().and_then(|name| name.to_str())
                == Some(WORKFLOW_TRANSCRIPT_DIR)
            {
                let run_dir = transcript_dir.parent()?;
                return Some((run_dir.to_path_buf(), transcript_dir.to_path_buf()));
            }
            return None;
        }
        if path.extension().and_then(|ext| ext.to_str()) == Some("json") {
            let workflows_dir = path.parent()?;
            if workflows_dir.file_name().and_then(|name| name.to_str())
                == Some(CLAUDE_WORKFLOW_SNAPSHOT_DIR)
            {
                let session_dir = workflows_dir.parent()?;
                let run_id = path.file_stem()?.to_str()?;
                let sidechain_dir = session_dir
                    .join(CLAUDE_WORKFLOW_SUBAGENT_DIR)
                    .join(CLAUDE_WORKFLOW_SNAPSHOT_DIR)
                    .join(run_id);
                return Some((sidechain_dir.clone(), sidechain_dir));
            }
        }
        if path.is_dir() {
            return Some((path.to_path_buf(), path.join(WORKFLOW_TRANSCRIPT_DIR)));
        }
        None
    }

    fn workflow_source_summary(source: &StoredWorkflowRunSource) -> Option<String> {
        let kind = non_empty_trimmed(source.kind.as_str())?;
        let mut summary = kind.replace('_', "-");
        if let Some(name) = source.name.as_deref().and_then(non_empty_trimmed) {
            summary.push_str(&format!(" `{name}`"));
        }
        Some(summary)
    }

    fn load_workflow_agent_journal_summary(run_dir: &str) -> Option<StoredWorkflowJournalSummary> {
        let journal_path = Path::new(run_dir).join(WORKFLOW_AGENT_JOURNAL_FILE);
        let metadata = fs::metadata(&journal_path).ok()?;
        if !metadata.is_file() || metadata.len() > WORKFLOW_AGENT_JOURNAL_READ_LIMIT_BYTES {
            return None;
        }
        let contents = fs::read_to_string(&journal_path).ok()?;
        let mut summary = StoredWorkflowJournalSummary {
            started: 0,
            results: 0,
            child_results: 0,
            invalid: 0,
            agents: Vec::new(),
            children: Vec::new(),
        };
        for line in contents
            .lines()
            .filter(|line| !line.trim().is_empty())
            .take(WORKFLOW_AGENT_JOURNAL_READ_LIMIT_ENTRIES)
        {
            let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
                summary.invalid += 1;
                continue;
            };
            let status = match value.get("type").and_then(serde_json::Value::as_str) {
                Some("child_result") => {
                    summary.child_results += 1;
                    Self::record_workflow_journal_child(&mut summary.children, &value);
                    None
                }
                Some("started") => {
                    summary.started += 1;
                    Some("started")
                }
                Some("result") => {
                    summary.results += 1;
                    Some("completed")
                }
                _ if value.get("result").is_some() => {
                    summary.results += 1;
                    Some("completed")
                }
                _ => {
                    summary.invalid += 1;
                    None
                }
            };
            if let Some(status) = status {
                Self::record_workflow_journal_agent(&mut summary.agents, &value, status);
            }
        }
        Some(summary)
    }

    fn record_workflow_journal_child(
        children: &mut Vec<StoredWorkflowJournalChild>,
        value: &serde_json::Value,
    ) {
        let child = value
            .get("child")
            .and_then(serde_json::Value::as_str)
            .and_then(non_empty_trimmed)
            .unwrap_or("(unknown)")
            .to_string();
        let child_run_id = value
            .get("childRunId")
            .and_then(serde_json::Value::as_str)
            .and_then(non_empty_trimmed)
            .map(ToString::to_string);
        let key = value
            .get("key")
            .and_then(serde_json::Value::as_str)
            .and_then(non_empty_trimmed)
            .map(ToString::to_string);
        if children.iter().any(|entry| {
            entry.child == child && entry.child_run_id == child_run_id && entry.key == key
        }) {
            return;
        }
        if children.len() < WORKFLOW_AGENT_JOURNAL_AGENT_DISPLAY_LIMIT {
            children.push(StoredWorkflowJournalChild {
                child,
                child_run_id,
                key,
            });
        }
    }

    fn record_workflow_journal_agent(
        agents: &mut Vec<StoredWorkflowJournalAgent>,
        value: &serde_json::Value,
        status: &'static str,
    ) {
        let agent_id = value
            .get("agentId")
            .and_then(serde_json::Value::as_str)
            .and_then(non_empty_trimmed)
            .unwrap_or("(unknown)")
            .to_string();
        let key = value
            .get("key")
            .and_then(serde_json::Value::as_str)
            .and_then(non_empty_trimmed)
            .map(ToString::to_string);
        if let Some(existing) = agents
            .iter_mut()
            .find(|agent| agent.agent_id == agent_id && agent.key == key)
        {
            if status == "completed" {
                existing.status = status;
            }
            return;
        }
        if agents.len() < WORKFLOW_AGENT_JOURNAL_AGENT_DISPLAY_LIMIT {
            agents.push(StoredWorkflowJournalAgent {
                agent_id,
                key,
                status,
            });
        }
    }

    fn load_workflow_agent_transcript(
        run: &StoredWorkflowRun,
        agent_id: &str,
    ) -> Option<WorkflowAgentTranscript> {
        let agent_id = non_empty_trimmed(agent_id)?;
        let safe_agent_id = Self::workflow_agent_transcript_file_agent_id(agent_id)?;
        let file_name = format!("agent-{safe_agent_id}.jsonl");
        let mut dirs = Vec::new();
        if let Some(transcript_dir) = run.transcript_dir.as_deref().and_then(non_empty_trimmed) {
            dirs.push(PathBuf::from(transcript_dir));
        }
        if let Some(run_dir) = run.run_dir.as_deref().and_then(non_empty_trimmed) {
            let run_dir = Path::new(run_dir);
            dirs.push(run_dir.join(WORKFLOW_TRANSCRIPT_DIR));
            dirs.push(run_dir.to_path_buf());
        }
        for dir in dirs {
            let path = dir.join(&file_name);
            let Some(transcript) = Self::read_workflow_agent_transcript(path.as_path()) else {
                continue;
            };
            return Some(transcript);
        }
        None
    }

    fn load_workflow_agent_metadata(
        run: &StoredWorkflowRun,
        agent_id: &str,
    ) -> Option<WorkflowAgentMetadata> {
        let agent_id = non_empty_trimmed(agent_id)?;
        let safe_agent_id = Self::workflow_agent_transcript_file_agent_id(agent_id)?;
        let file_name = format!("agent-{safe_agent_id}.meta.json");
        let mut dirs = Vec::new();
        if let Some(transcript_dir) = run.transcript_dir.as_deref().and_then(non_empty_trimmed) {
            dirs.push(PathBuf::from(transcript_dir));
        }
        if let Some(run_dir) = run.run_dir.as_deref().and_then(non_empty_trimmed) {
            let run_dir = Path::new(run_dir);
            dirs.push(run_dir.join(WORKFLOW_TRANSCRIPT_DIR));
            dirs.push(run_dir.to_path_buf());
        }
        for dir in dirs {
            let path = dir.join(&file_name);
            let Some(metadata) = Self::read_workflow_agent_metadata(path.as_path()) else {
                continue;
            };
            return Some(metadata);
        }
        None
    }

    fn read_workflow_agent_metadata(path: &Path) -> Option<WorkflowAgentMetadata> {
        let metadata = fs::metadata(path).ok()?;
        if !metadata.is_file() || metadata.len() > WORKFLOW_AGENT_TRANSCRIPT_READ_LIMIT_BYTES {
            return None;
        }
        let contents = fs::read_to_string(path).ok()?;
        let value = serde_json::from_str::<serde_json::Value>(&contents).ok()?;
        let fields = [
            ("task", "taskName"),
            ("agent", "agentName"),
            ("session", "sessionKind"),
            ("parent thread", "parentThreadId"),
            ("type", "agentType"),
            ("model", "model"),
            ("effort", "reasoningEffort"),
            ("tier", "serviceTier"),
            ("isolation", "isolation"),
            ("nick", "nickname"),
            ("tool", "toolUseId"),
            ("run", "runId"),
            ("cell", "cellId"),
            ("cwd", "cwd"),
            ("branch", "gitBranch"),
            ("worktree", "worktreePath"),
            ("author", "author"),
            ("recipient", "recipient"),
        ]
        .into_iter()
        .filter_map(|(label, key)| {
            value
                .get(key)
                .and_then(serde_json::Value::as_str)
                .and_then(non_empty_trimmed)
                .map(|text| (label.to_string(), compact_workflow_run_preview(text)))
        })
        .collect::<Vec<_>>();
        (!fields.is_empty()).then_some(WorkflowAgentMetadata { fields })
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

    fn read_workflow_agent_transcript(path: &Path) -> Option<WorkflowAgentTranscript> {
        let metadata = fs::metadata(path).ok()?;
        if !metadata.is_file() || metadata.len() > WORKFLOW_AGENT_TRANSCRIPT_READ_LIMIT_BYTES {
            return None;
        }
        let contents = fs::read_to_string(path).ok()?;
        let mut transcript = WorkflowAgentTranscript {
            prompt: None,
            reasoning: Vec::new(),
            final_text: None,
            tool_calls: Vec::new(),
            invalid: 0,
        };
        for line in contents.lines().filter(|line| !line.trim().is_empty()) {
            let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
                transcript.invalid += 1;
                continue;
            };
            match Self::workflow_transcript_record_kind(&value) {
                Some("user") => {
                    if transcript.prompt.is_none() {
                        transcript.prompt = Self::workflow_transcript_message_text(&value);
                    }
                    Self::record_workflow_agent_transcript_tool_use_result(&mut transcript, &value);
                    Self::record_workflow_agent_transcript_tool_results(
                        &mut transcript,
                        Self::workflow_transcript_record_content(&value),
                    );
                }
                Some("assistant") => {
                    Self::record_workflow_agent_transcript_assistant(
                        &mut transcript,
                        Self::workflow_transcript_record_content(&value),
                        Self::workflow_transcript_record_uuid(&value).as_deref(),
                    );
                }
                Some("tool_result" | "toolResult") => {
                    Self::record_workflow_agent_transcript_tool_result(&mut transcript, &value);
                }
                _ => {}
            }
        }
        Some(transcript)
    }

    fn workflow_transcript_record_kind(value: &serde_json::Value) -> Option<&str> {
        value
            .get("type")
            .or_else(|| value.get("role"))
            .and_then(serde_json::Value::as_str)
            .and_then(non_empty_trimmed)
    }

    fn workflow_transcript_record_content(value: &serde_json::Value) -> &serde_json::Value {
        let message = value.get("message").unwrap_or(value);
        message
            .get("content")
            .or_else(|| value.get("content"))
            .unwrap_or(message)
    }

    fn workflow_transcript_message_text(value: &serde_json::Value) -> Option<String> {
        let message = value.get("message").unwrap_or(value);
        if let Some(text) = message.as_str().and_then(non_empty_trimmed) {
            return Some(compact_workflow_run_preview(text));
        }
        if let Some(text) = message
            .get("content")
            .and_then(Self::workflow_transcript_content_text)
        {
            return Some(text);
        }
        Self::workflow_transcript_content_text(message)
    }

    fn workflow_transcript_content_text(content: &serde_json::Value) -> Option<String> {
        if let Some(text) = content.as_str().and_then(non_empty_trimmed) {
            return Some(compact_workflow_run_preview(text));
        }
        let values = content.as_array()?;
        let text = values
            .iter()
            .filter_map(|item| {
                if matches!(
                    item.get("type").and_then(serde_json::Value::as_str),
                    Some("reasoning" | "thinking" | "tool_use" | "tool_result")
                ) {
                    return None;
                }
                item.as_str()
                    .and_then(non_empty_trimmed)
                    .or_else(|| {
                        item.get("text")
                            .and_then(serde_json::Value::as_str)
                            .and_then(non_empty_trimmed)
                    })
                    .or_else(|| {
                        item.get("content")
                            .and_then(serde_json::Value::as_str)
                            .and_then(non_empty_trimmed)
                    })
            })
            .collect::<Vec<_>>()
            .join("\n");
        non_empty_trimmed(text.as_str()).map(compact_workflow_run_preview)
    }

    fn record_workflow_agent_transcript_assistant(
        transcript: &mut WorkflowAgentTranscript,
        content: &serde_json::Value,
        assistant_uuid: Option<&str>,
    ) {
        if let Some(text) = Self::workflow_transcript_content_text(content) {
            transcript.final_text = Some(text);
        }
        let Some(items) = content.as_array() else {
            return;
        };
        for item in items {
            if matches!(
                item.get("type").and_then(serde_json::Value::as_str),
                Some("reasoning" | "thinking")
            ) {
                if let Some(text) = Self::workflow_transcript_reasoning_text(item) {
                    transcript.reasoning.push(text);
                }
                continue;
            }
            if item.get("type").and_then(serde_json::Value::as_str) != Some("tool_use") {
                continue;
            }
            let name = item
                .get("name")
                .and_then(serde_json::Value::as_str)
                .and_then(non_empty_trimmed)
                .unwrap_or("tool")
                .to_string();
            let id = Self::workflow_transcript_tool_id(item);
            let summary = item
                .get("input")
                .and_then(|input| serde_json::to_string(input).ok())
                .and_then(|text| {
                    non_empty_trimmed(text.as_str()).map(compact_workflow_run_preview)
                });
            let output = item
                .get("output")
                .and_then(serde_json::Value::as_str)
                .and_then(non_empty_trimmed)
                .map(compact_workflow_run_preview);
            transcript.tool_calls.push(WorkflowAgentTranscriptToolCall {
                id,
                assistant_uuid: assistant_uuid.map(ToString::to_string),
                name,
                summary,
                output,
            });
        }
    }

    fn record_workflow_agent_transcript_tool_results(
        transcript: &mut WorkflowAgentTranscript,
        content: &serde_json::Value,
    ) {
        if Self::workflow_transcript_record_kind(content) == Some("tool_result") {
            Self::record_workflow_agent_transcript_tool_result(transcript, content);
            return;
        }
        let Some(items) = content.as_array() else {
            return;
        };
        for item in items {
            if matches!(
                Self::workflow_transcript_record_kind(item),
                Some("tool_result" | "toolResult")
            ) {
                Self::record_workflow_agent_transcript_tool_result(transcript, item);
            }
        }
    }

    fn record_workflow_agent_transcript_tool_result(
        transcript: &mut WorkflowAgentTranscript,
        item: &serde_json::Value,
    ) {
        let Some(output) = Self::workflow_transcript_tool_result_text(item) else {
            return;
        };
        let id = Self::workflow_transcript_tool_result_id(item);
        if let Some(id) = id.as_deref()
            && let Some(call) = transcript
                .tool_calls
                .iter_mut()
                .rev()
                .find(|call| call.id.as_deref() == Some(id) && call.output.is_none())
        {
            call.output = Some(output);
            return;
        }
        if let Some(source_assistant_uuid) =
            Self::workflow_transcript_source_tool_assistant_uuid(item).as_deref()
            && let Some(call) = transcript.tool_calls.iter_mut().rev().find(|call| {
                call.assistant_uuid.as_deref() == Some(source_assistant_uuid)
                    && call.output.is_none()
            })
        {
            call.output = Some(output);
            return;
        }
        if let Some(call) = transcript
            .tool_calls
            .iter_mut()
            .rev()
            .find(|call| call.output.is_none())
        {
            call.output = Some(output);
        }
    }

    fn record_workflow_agent_transcript_tool_use_result(
        transcript: &mut WorkflowAgentTranscript,
        value: &serde_json::Value,
    ) {
        let Some(result) = value
            .get("toolUseResult")
            .or_else(|| value.get("tool_use_result"))
        else {
            return;
        };
        let mut item = serde_json::json!({
            "content": result,
        });
        if let Some(id) = Self::workflow_transcript_tool_result_id(value)
            && let Some(object) = item.as_object_mut()
        {
            object.insert("tool_use_id".to_string(), serde_json::Value::String(id));
        }
        if let Some(source_assistant_uuid) =
            Self::workflow_transcript_source_tool_assistant_uuid(value)
            && let Some(object) = item.as_object_mut()
        {
            object.insert(
                "sourceToolAssistantUUID".to_string(),
                serde_json::Value::String(source_assistant_uuid),
            );
        }
        Self::record_workflow_agent_transcript_tool_result(transcript, &item);
    }

    fn workflow_transcript_reasoning_text(item: &serde_json::Value) -> Option<String> {
        if let Some(text) = item
            .get("text")
            .and_then(serde_json::Value::as_str)
            .and_then(non_empty_trimmed)
            .or_else(|| {
                item.get("summary")
                    .and_then(serde_json::Value::as_str)
                    .and_then(non_empty_trimmed)
            })
            .or_else(|| {
                item.get("thinking")
                    .and_then(serde_json::Value::as_str)
                    .and_then(non_empty_trimmed)
            })
            .or_else(|| {
                item.get("content")
                    .and_then(serde_json::Value::as_str)
                    .and_then(non_empty_trimmed)
            })
        {
            return Some(compact_workflow_run_preview(text));
        }
        for key in ["summary", "content"] {
            if let Some(text) = item
                .get(key)
                .and_then(Self::workflow_transcript_text_from_array)
            {
                return Some(text);
            }
        }
        None
    }

    fn workflow_transcript_tool_id(item: &serde_json::Value) -> Option<String> {
        item.get("id")
            .or_else(|| item.get("toolUseId"))
            .or_else(|| item.get("tool_call_id"))
            .or_else(|| item.get("toolCallId"))
            .and_then(serde_json::Value::as_str)
            .and_then(non_empty_trimmed)
            .map(ToString::to_string)
    }

    fn workflow_transcript_tool_result_id(item: &serde_json::Value) -> Option<String> {
        item.get("tool_use_id")
            .or_else(|| item.get("toolUseId"))
            .or_else(|| item.get("source_tool_use_id"))
            .or_else(|| item.get("sourceToolUseId"))
            .or_else(|| item.get("sourceToolUseID"))
            .or_else(|| item.get("parent_tool_use_id"))
            .or_else(|| item.get("parentToolUseId"))
            .or_else(|| item.get("tool_call_id"))
            .or_else(|| item.get("toolCallId"))
            .or_else(|| item.get("id"))
            .and_then(serde_json::Value::as_str)
            .and_then(non_empty_trimmed)
            .map(ToString::to_string)
    }

    fn workflow_transcript_source_tool_assistant_uuid(item: &serde_json::Value) -> Option<String> {
        item.get("sourceToolAssistantUUID")
            .or_else(|| item.get("source_tool_assistant_uuid"))
            .and_then(serde_json::Value::as_str)
            .and_then(non_empty_trimmed)
            .map(ToString::to_string)
    }

    fn workflow_transcript_record_uuid(value: &serde_json::Value) -> Option<String> {
        value
            .get("uuid")
            .and_then(serde_json::Value::as_str)
            .and_then(non_empty_trimmed)
            .map(ToString::to_string)
    }

    fn workflow_transcript_tool_result_text(item: &serde_json::Value) -> Option<String> {
        if let Some(text) = item.as_str().and_then(non_empty_trimmed) {
            return Some(compact_workflow_run_preview(text));
        }
        for key in ["output", "result", "text"] {
            if let Some(text) = item
                .get(key)
                .and_then(serde_json::Value::as_str)
                .and_then(non_empty_trimmed)
            {
                return Some(compact_workflow_run_preview(text));
            }
        }
        let content = item.get("content")?;
        if let Some(text) = content.as_str().and_then(non_empty_trimmed) {
            return Some(compact_workflow_run_preview(text));
        }
        if let Some(text) = Self::workflow_transcript_text_from_array(content) {
            return Some(text);
        }
        serde_json::to_string(content)
            .ok()
            .and_then(|text| non_empty_trimmed(text.as_str()).map(compact_workflow_run_preview))
    }

    fn workflow_transcript_text_from_array(content: &serde_json::Value) -> Option<String> {
        let values = content.as_array()?;
        let text = values
            .iter()
            .filter_map(|item| {
                item.as_str()
                    .and_then(non_empty_trimmed)
                    .or_else(|| {
                        item.get("text")
                            .and_then(serde_json::Value::as_str)
                            .and_then(non_empty_trimmed)
                    })
                    .or_else(|| {
                        item.get("content")
                            .and_then(serde_json::Value::as_str)
                            .and_then(non_empty_trimmed)
                    })
                    .or_else(|| {
                        item.get("summary")
                            .and_then(serde_json::Value::as_str)
                            .and_then(non_empty_trimmed)
                    })
            })
            .collect::<Vec<_>>()
            .join("\n");
        non_empty_trimmed(text.as_str()).map(compact_workflow_run_preview)
    }

    fn append_workflow_active_run_listing(message: &mut String, listing: StoredWorkflowRunListing) {
        message.push_str(&format!(
            "Active registry: {}\nActive runs:",
            listing.dir.display()
        ));
        if listing.runs.is_empty() {
            message.push_str("\n- none");
        } else {
            for run in listing.runs {
                message.push('\n');
                message.push_str(&Self::format_workflow_run_line(&run));
            }
        }
        if listing.skipped > 0 {
            message.push_str(&format!(
                "\nSkipped invalid active run markers: {}",
                listing.skipped
            ));
        }
    }

    fn append_workflow_run_listing(message: &mut String, listing: StoredWorkflowRunListing) {
        message.push_str(&format!(
            "Run store: {}\nRecent runs:",
            listing.dir.display()
        ));
        if listing.runs.is_empty() {
            message.push_str("\n- none");
        } else {
            for run in listing.runs {
                message.push('\n');
                message.push_str(&Self::format_workflow_run_line(&run));
            }
        }
        if listing.skipped > 0 {
            message.push_str(&format!(
                "\nSkipped invalid run snapshots: {}",
                listing.skipped
            ));
        }
    }

    fn format_workflow_run_line(run: &StoredWorkflowRun) -> String {
        let status = run.status.trim();
        let name = run.workflow_name.trim();
        let run_id = run.run_id.trim();
        let mut line = format!("- {status} {name} `{run_id}`");
        if let Some(cell_id) = run.cell_id.as_deref().and_then(non_empty_trimmed) {
            line.push_str(&format!(" (cell `{cell_id}`)"));
        }
        if let Some(description) = run.description.as_deref().and_then(non_empty_trimmed) {
            line.push_str(&format!(" - {description}"));
        }
        if let Some(source) = run.source.as_ref().and_then(Self::workflow_source_summary) {
            line.push_str(&format!("\n  source: {source}"));
        }
        if let Some(script_path) = run.script_path.as_deref().and_then(non_empty_trimmed) {
            line.push_str(&format!("\n  script: `{script_path}`"));
        }
        if let Some(transcript_dir) = run.transcript_dir.as_deref().and_then(non_empty_trimmed) {
            line.push_str(&format!("\n  transcripts: `{transcript_dir}`"));
        }
        if let Some(resume_from_run_id) = run
            .resume_from_run_id
            .as_deref()
            .and_then(non_empty_trimmed)
        {
            line.push_str(&format!("\n  resumed from: `{resume_from_run_id}`"));
        }
        if let Some(progress) = run.progress.last() {
            line.push_str(&format!(
                "\n  progress: {}",
                Self::format_workflow_progress_event_inline(progress)
            ));
        }
        if let Some(actions) = Self::format_workflow_run_actions(run) {
            line.push_str(&format!("\n  actions: {actions}"));
        }
        if let Some(preview) = run
            .error
            .as_deref()
            .or(run.output_preview.as_deref())
            .and_then(non_empty_trimmed)
            .map(compact_workflow_run_preview)
        {
            line.push_str(&format!("\n  {preview}"));
        }
        line
    }

    fn add_workflow_run_detail_output(&mut self, run_id: &str) {
        match self.load_workflow_run_detail(run_id) {
            Ok(Some(run)) => self.add_info_message(
                Self::format_workflow_run_detail(&run),
                Some("Use /workflows for the recent run list.".to_string()),
            ),
            Ok(None) => {
                self.add_error_message(format!("No workflow run found for `{}`.", run_id.trim()));
            }
            Err(err) => self.add_error_message(err),
        }
    }

    pub(super) fn handle_workflows_slash_arg(&mut self, args: &str) {
        let trimmed = args.trim();
        let mut parts = trimmed.splitn(2, char::is_whitespace);
        let command = parts.next().unwrap_or_default();
        let Some(rest) = parts.next() else {
            if matches!(
                command,
                "resume"
                    | "retry"
                    | "cancel"
                    | "pause"
                    | "continue"
                    | "interrupt-agent"
                    | "skip-agent"
                    | "retry-agent"
                    | "restart-agent"
                    | "save"
                    | "run"
                    | "detail"
                    | "approval"
                    | "enabled"
                    | "enable"
                    | "disable"
            ) {
                self.add_error_message(WORKFLOWS_USAGE.to_string());
                return;
            }
            self.add_workflow_run_detail_output(trimmed);
            return;
        };
        let run_id = rest.trim();
        match command {
            "resume" if !run_id.is_empty() => self.resume_workflow_run(run_id),
            "retry" if !run_id.is_empty() => self.retry_workflow_run(run_id),
            "cancel" if !run_id.is_empty() => self.cancel_workflow_run(run_id),
            "pause" if !run_id.is_empty() => self.pause_workflow_run(run_id),
            "continue" if !run_id.is_empty() => self.continue_workflow_run(run_id),
            "interrupt-agent" => self.interrupt_workflow_agent_from_args(rest),
            "skip-agent" => {
                self.control_workflow_agent_from_args(rest, WorkflowAgentControlAction::Skip)
            }
            "retry-agent" => {
                self.control_workflow_agent_from_args(rest, WorkflowAgentControlAction::Retry)
            }
            "restart-agent" => self.restart_workflow_agent_from_args(rest),
            "save" => self.save_workflow_run(rest),
            "run" => self.run_named_workflow_from_args(rest),
            "approval" => self.set_named_workflow_approval_from_args(rest),
            "enabled" => self.set_named_workflow_enabled_from_args(rest),
            "enable" => self.set_named_workflow_enabled_shortcut(rest, true),
            "disable" => self.set_named_workflow_enabled_shortcut(rest, false),
            "detail" if !run_id.is_empty() => self.add_workflow_run_detail_output(run_id),
            _ => self.add_error_message(WORKFLOWS_USAGE.to_string()),
        }
    }

    fn set_named_workflow_approval_from_args(&mut self, args: &str) {
        let mut parts = args.split_whitespace();
        let workflow_name = parts.next().unwrap_or_default();
        let approval_value = parts.next().unwrap_or_default();
        if workflow_name.is_empty() || approval_value.is_empty() || parts.next().is_some() {
            self.add_error_message(WORKFLOWS_USAGE.to_string());
            return;
        }
        if !is_valid_workflow_invocation_name(workflow_name) {
            self.add_error_message(
                "Workflow name must not be empty, hidden, or contain path separators.".to_string(),
            );
            return;
        }
        let Some(approval) = Self::workflow_approval_from_slash_value(approval_value) else {
            self.add_error_message(
                "Workflow approval must be one of auto, ask, allow, deny, or clear.".to_string(),
            );
            return;
        };
        self.app_event_tx
            .send(AppEvent::UpdateNamedWorkflowApproval {
                workflow_name: workflow_name.to_string(),
                approval,
            });
    }

    fn set_named_workflow_enabled_from_args(&mut self, args: &str) {
        let mut parts = args.split_whitespace();
        let workflow_name = parts.next().unwrap_or_default();
        let enabled_value = parts.next().unwrap_or_default();
        if workflow_name.is_empty() || enabled_value.is_empty() || parts.next().is_some() {
            self.add_error_message(WORKFLOWS_USAGE.to_string());
            return;
        }
        let Some(enabled) = Self::workflow_enabled_from_slash_value(enabled_value) else {
            self.add_error_message(
                "Workflow enabled override must be one of on, off, or clear.".to_string(),
            );
            return;
        };
        self.emit_named_workflow_enabled_update(workflow_name, enabled);
    }

    fn set_named_workflow_enabled_shortcut(&mut self, args: &str, enabled: bool) {
        let mut parts = args.split_whitespace();
        let workflow_name = parts.next().unwrap_or_default();
        if workflow_name.is_empty() || parts.next().is_some() {
            self.add_error_message(WORKFLOWS_USAGE.to_string());
            return;
        }
        self.emit_named_workflow_enabled_update(workflow_name, Some(enabled));
    }

    fn emit_named_workflow_enabled_update(&mut self, workflow_name: &str, enabled: Option<bool>) {
        if !is_valid_workflow_invocation_name(workflow_name) {
            self.add_error_message(
                "Workflow name must not be empty, hidden, or contain path separators.".to_string(),
            );
            return;
        }
        self.app_event_tx
            .send(AppEvent::UpdateNamedWorkflowEnabled {
                workflow_name: workflow_name.to_string(),
                enabled,
            });
    }

    fn run_named_workflow_from_args(&mut self, args: &str) {
        let mut parts = args.trim().splitn(2, char::is_whitespace);
        let workflow_name = parts.next().unwrap_or_default();
        let workflow_args = normalize_workflows_run_args(parts.next().unwrap_or_default());
        if workflow_name.is_empty() {
            self.add_error_message("Usage: /workflows run <name> [args]".to_string());
            return;
        }
        if !is_valid_workflow_invocation_name(workflow_name) {
            self.add_error_message(
                "Workflow name must not be empty, hidden, or contain path separators.".to_string(),
            );
            return;
        }
        let listing = self.load_workflow_definitions(usize::MAX);
        let Some(definition) = listing
            .definitions
            .into_iter()
            .find(|definition| definition.invocation_name == workflow_name)
        else {
            self.add_error_message(format!(
                "No workflow definition found for `{workflow_name}`."
            ));
            return;
        };
        if definition.config_enabled == Some(false) {
            self.add_error_message(format!(
                "Workflow `{workflow_name}` is disabled by workflow config."
            ));
            return;
        }
        let command = WorkflowSlashCommand {
            name: definition.invocation_name,
            description: definition.when_to_use.unwrap_or(definition.description),
            input_schema: definition.input_schema,
            source_label: Some(definition.source_label.to_string()),
        };
        self.submit_user_message(UserMessage {
            text: command.invocation_prompt(&workflow_args),
            local_images: Vec::new(),
            remote_image_urls: Vec::new(),
            text_elements: Vec::new(),
            mention_bindings: Vec::new(),
        });
    }

    fn resume_workflow_run(&mut self, run_id: &str) {
        match self.load_workflow_run_detail(run_id) {
            Ok(Some(run)) => {
                self.submit_user_message(UserMessage {
                    text: Self::workflow_resume_invocation_prompt(&run),
                    local_images: Vec::new(),
                    remote_image_urls: Vec::new(),
                    text_elements: Vec::new(),
                    mention_bindings: Vec::new(),
                });
            }
            Ok(None) => {
                self.add_error_message(format!("No workflow run found for `{}`.", run_id.trim()));
            }
            Err(err) => self.add_error_message(err),
        }
    }

    fn retry_workflow_run(&mut self, run_id: &str) {
        match self.load_workflow_run_detail(run_id) {
            Ok(Some(run)) => {
                if run
                    .script_path
                    .as_deref()
                    .and_then(non_empty_trimmed)
                    .is_none()
                {
                    self.add_error_message(format!(
                        "Workflow run `{}` has no script path to retry.",
                        run.run_id.trim()
                    ));
                    return;
                }
                self.submit_user_message(UserMessage {
                    text: Self::workflow_retry_invocation_prompt(&run),
                    local_images: Vec::new(),
                    remote_image_urls: Vec::new(),
                    text_elements: Vec::new(),
                    mention_bindings: Vec::new(),
                });
            }
            Ok(None) => {
                self.add_error_message(format!("No workflow run found for `{}`.", run_id.trim()));
            }
            Err(err) => self.add_error_message(err),
        }
    }

    fn save_workflow_run(&mut self, args: &str) {
        let mut parts = args.split_whitespace();
        let run_id = parts.next().unwrap_or_default();
        let workflow_name = parts.next().unwrap_or_default();
        if run_id.is_empty() || workflow_name.is_empty() || parts.next().is_some() {
            self.add_error_message("Usage: /workflows save <run_id> <name>".to_string());
            return;
        }
        if !is_valid_saved_workflow_name(workflow_name) {
            self.add_error_message(
                "Workflow name must contain only letters, numbers, `_`, or `-`.".to_string(),
            );
            return;
        }
        match self.load_workflow_run_detail(run_id) {
            Ok(Some(run)) => self.save_loaded_workflow_run(&run, workflow_name),
            Ok(None) => {
                self.add_error_message(format!("No workflow run found for `{}`.", run_id.trim()));
            }
            Err(err) => self.add_error_message(err),
        }
    }

    fn save_loaded_workflow_run(&mut self, run: &StoredWorkflowRun, workflow_name: &str) {
        let Some(script_path) = run
            .script_path
            .as_deref()
            .and_then(non_empty_trimmed)
            .map(PathBuf::from)
        else {
            self.add_error_message(format!(
                "Workflow run `{}` has no script path to save.",
                run.run_id.trim()
            ));
            return;
        };
        let Some(workflow_dir) = self.config.workflows.workflow_dirs.first().cloned() else {
            self.add_error_message("No workflow directory is configured.".to_string());
            return;
        };
        let target_path = workflow_dir.join(format!("{workflow_name}.js"));
        if target_path.exists() {
            self.add_error_message(format!(
                "Workflow `{workflow_name}` already exists at {}.",
                target_path.display()
            ));
            return;
        }
        let script = match fs::read_to_string(&script_path) {
            Ok(script) => script,
            Err(err) => {
                self.add_error_message(format!("failed to read {}: {err}", script_path.display()));
                return;
            }
        };
        if parse_workflow_metadata_for_listing(&script).is_none() {
            self.add_error_message(format!(
                "Workflow run `{}` script does not contain a valid `export const meta` header.",
                run.run_id.trim()
            ));
            return;
        }
        if let Err(err) = fs::create_dir_all(&workflow_dir) {
            self.add_error_message(format!(
                "failed to create {}: {err}",
                workflow_dir.display()
            ));
            return;
        }
        if let Err(err) = fs::write(&target_path, script) {
            self.add_error_message(format!("failed to write {}: {err}", target_path.display()));
            return;
        }
        self.sync_workflow_slash_commands();
        self.add_info_message(
            format!(
                "Saved workflow `{workflow_name}` to {}.",
                target_path.display()
            ),
            Some(format!(
                "Use /workflows run {workflow_name} or /{workflow_name} to run it."
            )),
        );
    }

    fn cancel_workflow_run(&mut self, run_id: &str) {
        match self.load_workflow_run_detail(run_id) {
            Ok(Some(run)) => {
                if !workflow_status_is_active(run.status.trim()) {
                    self.add_error_message(format!(
                        "Workflow run `{}` is `{}` and cannot be cancelled.",
                        run.run_id.trim(),
                        run.status.trim()
                    ));
                    return;
                }
                let Some(cell_id) = run.cell_id.as_deref().and_then(non_empty_trimmed) else {
                    self.add_error_message(format!(
                        "Workflow run `{}` has no cell id to cancel.",
                        run.run_id.trim()
                    ));
                    return;
                };
                self.submit_op(AppCommand::workflow_cancel(
                    run.run_id.trim().to_string(),
                    cell_id.to_string(),
                ));
                self.add_info_message(
                    format!(
                        "Cancellation requested for workflow run `{}`.",
                        run.run_id.trim()
                    ),
                    Some(format!("Cell `{cell_id}` will be terminated directly.")),
                );
            }
            Ok(None) => {
                self.add_error_message(format!("No workflow run found for `{}`.", run_id.trim()));
            }
            Err(err) => self.add_error_message(err),
        }
    }

    fn pause_workflow_run(&mut self, run_id: &str) {
        match self.load_workflow_run_detail(run_id) {
            Ok(Some(run)) => {
                if run.status.trim() != "running" {
                    self.add_error_message(format!(
                        "Workflow run `{}` is `{}` and cannot be paused.",
                        run.run_id.trim(),
                        run.status.trim()
                    ));
                    return;
                }
                let Some(cell_id) = run.cell_id.as_deref().and_then(non_empty_trimmed) else {
                    self.add_error_message(format!(
                        "Workflow run `{}` has no cell id to pause.",
                        run.run_id.trim()
                    ));
                    return;
                };
                self.submit_op(AppCommand::workflow_pause(
                    run.run_id.trim().to_string(),
                    cell_id.to_string(),
                ));
                self.add_info_message(
                    format!("Pause requested for workflow run `{}`.", run.run_id.trim()),
                    Some(
                        "The workflow will stop at its next pending runtime boundary.".to_string(),
                    ),
                );
            }
            Ok(None) => {
                self.add_error_message(format!("No workflow run found for `{}`.", run_id.trim()));
            }
            Err(err) => self.add_error_message(err),
        }
    }

    fn continue_workflow_run(&mut self, run_id: &str) {
        match self.load_workflow_run_detail(run_id) {
            Ok(Some(run)) => {
                if run.status.trim() != "paused" {
                    self.add_error_message(format!(
                        "Workflow run `{}` is `{}` and cannot be continued.",
                        run.run_id.trim(),
                        run.status.trim()
                    ));
                    return;
                }
                let Some(cell_id) = run.cell_id.as_deref().and_then(non_empty_trimmed) else {
                    self.add_error_message(format!(
                        "Workflow run `{}` has no cell id to continue.",
                        run.run_id.trim()
                    ));
                    return;
                };
                self.submit_op(AppCommand::workflow_continue(
                    run.run_id.trim().to_string(),
                    cell_id.to_string(),
                ));
                self.add_info_message(
                    format!(
                        "Continue requested for workflow run `{}`.",
                        run.run_id.trim()
                    ),
                    Some(
                        "The workflow will run until its next pending or terminal boundary."
                            .to_string(),
                    ),
                );
            }
            Ok(None) => {
                self.add_error_message(format!("No workflow run found for `{}`.", run_id.trim()));
            }
            Err(err) => self.add_error_message(err),
        }
    }

    fn interrupt_workflow_agent_from_args(&mut self, args: &str) {
        let mut parts = args.split_whitespace();
        let run_id = parts.next().unwrap_or_default();
        let agent_id = parts.next().unwrap_or_default();
        if run_id.is_empty() || agent_id.is_empty() || parts.next().is_some() {
            self.add_error_message(WORKFLOWS_USAGE.to_string());
            return;
        }
        match self.load_workflow_run_detail(run_id) {
            Ok(Some(run)) => {
                if run.status.trim() != "running" {
                    self.add_error_message(format!(
                        "Workflow run `{}` is `{}` and has no running agents to interrupt.",
                        run.run_id.trim(),
                        run.status.trim()
                    ));
                    return;
                }
                let Some(status) =
                    Self::workflow_run_agent_status(run.progress.as_slice(), agent_id)
                else {
                    self.add_error_message(format!(
                        "Workflow run `{}` has no agent `{agent_id}` in progress.",
                        run.run_id.trim()
                    ));
                    return;
                };
                if status != "running" {
                    self.add_error_message(format!(
                        "Workflow run `{}` agent `{agent_id}` is `{status}` and cannot be interrupted.",
                        run.run_id.trim()
                    ));
                    return;
                }
                self.submit_op(AppCommand::workflow_agent_interrupt(
                    run.run_id.trim().to_string(),
                    agent_id.to_string(),
                ));
                self.add_info_message(
                    format!(
                        "Interrupt requested for workflow run `{}` agent `{agent_id}`.",
                        run.run_id.trim()
                    ),
                    Some("The agent turn will be interrupted directly.".to_string()),
                );
            }
            Ok(None) => {
                self.add_error_message(format!("No workflow run found for `{}`.", run_id.trim()));
            }
            Err(err) => self.add_error_message(err),
        }
    }

    fn restart_workflow_agent_from_args(&mut self, args: &str) {
        self.control_workflow_agent_from_args_with_labels(
            args,
            WorkflowAgentControlAction::Retry,
            "restart",
            "restarted",
        );
    }

    fn control_workflow_agent_from_args(&mut self, args: &str, action: WorkflowAgentControlAction) {
        let (action_label, action_past) = match action {
            WorkflowAgentControlAction::Skip => ("skip", "skipped"),
            WorkflowAgentControlAction::Retry => ("retry", "retried"),
        };
        self.control_workflow_agent_from_args_with_labels(args, action, action_label, action_past);
    }

    fn control_workflow_agent_from_args_with_labels(
        &mut self,
        args: &str,
        action: WorkflowAgentControlAction,
        action_label: &str,
        action_past: &str,
    ) {
        let mut parts = args.split_whitespace();
        let run_id = parts.next().unwrap_or_default();
        let agent_id = parts.next().unwrap_or_default();
        if run_id.is_empty() || agent_id.is_empty() || parts.next().is_some() {
            self.add_error_message(WORKFLOWS_USAGE.to_string());
            return;
        }
        match self.load_workflow_run_detail(run_id) {
            Ok(Some(run)) => {
                if run.status.trim() != "running" {
                    self.add_error_message(format!(
                        "Workflow run `{}` is `{}` and has no running agents to {action_label}.",
                        run.run_id.trim(),
                        run.status.trim()
                    ));
                    return;
                }
                let Some(status) =
                    Self::workflow_run_agent_status(run.progress.as_slice(), agent_id)
                else {
                    self.add_error_message(format!(
                        "Workflow run `{}` has no agent `{agent_id}` in progress.",
                        run.run_id.trim()
                    ));
                    return;
                };
                if status != "running" {
                    self.add_error_message(format!(
                        "Workflow run `{}` agent `{agent_id}` is `{status}` and cannot be {action_past}.",
                        run.run_id.trim()
                    ));
                    return;
                }
                self.submit_op(AppCommand::workflow_agent_control(
                    run.run_id.trim().to_string(),
                    agent_id.to_string(),
                    action,
                ));
                self.add_info_message(
                    format!(
                        "{} requested for workflow run `{}` agent `{agent_id}`.",
                        crate::text_formatting::capitalize_first(action_label),
                        run.run_id.trim()
                    ),
                    Some(
                        "The workflow runtime will apply the request without cancelling the run."
                            .to_string(),
                    ),
                );
            }
            Ok(None) => {
                self.add_error_message(format!("No workflow run found for `{}`.", run_id.trim()));
            }
            Err(err) => self.add_error_message(err),
        }
    }

    fn load_workflow_run_detail(&self, run_id: &str) -> Result<Option<StoredWorkflowRun>, String> {
        let run_id = run_id.trim();
        if !is_safe_workflow_run_id(run_id) {
            return Err(
                "Workflow run id must contain only letters, numbers, `_`, or `-`.".to_string(),
            );
        }
        let path = self.workflow_runs_dir().join(format!("{run_id}.json"));
        let contents = match fs::read_to_string(&path) {
            Ok(contents) => contents,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                let transcript_path = self
                    .workflow_runs_dir()
                    .join(run_id)
                    .join(WORKFLOW_TRANSCRIPT_DIR)
                    .join(WORKFLOW_TRANSCRIPT_RUN_FILE);
                match fs::read_to_string(&transcript_path) {
                    Ok(transcript_contents) => {
                        let run = serde_json::from_str::<StoredWorkflowRun>(&transcript_contents)
                            .map_err(|err| {
                            format!("failed to parse {}: {err}", transcript_path.display())
                        })?;
                        let run_dir = self.workflow_runs_dir().join(run_id);
                        return Ok(Some(Self::normalize_stored_workflow_run(
                            run,
                            Some(run_dir.as_path()),
                        )));
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                        let Some(claude_path) = Self::find_claude_native_workflow_snapshot(
                            &self.workflow_runs_dir(),
                            run_id,
                        )?
                        else {
                            return Ok(None);
                        };
                        let contents = fs::read_to_string(&claude_path).map_err(|err| {
                            format!("failed to read {}: {err}", claude_path.display())
                        })?;
                        let run = serde_json::from_str::<StoredWorkflowRun>(&contents).map_err(
                            |err| format!("failed to parse {}: {err}", claude_path.display()),
                        )?;
                        return Ok(Some(Self::normalize_stored_workflow_run(
                            run,
                            Some(claude_path.as_path()),
                        )));
                    }
                    Err(err) => {
                        return Err(format!(
                            "failed to read {}: {err}",
                            transcript_path.display()
                        ));
                    }
                }
            }
            Err(err) => {
                return Err(format!("failed to read {}: {err}", path.display()));
            }
        };
        let run = serde_json::from_str::<StoredWorkflowRun>(&contents)
            .map_err(|err| format!("failed to parse {}: {err}", path.display()))?;
        Ok(Some(Self::normalize_stored_workflow_run(
            run,
            Some(path.as_path()),
        )))
    }

    fn find_claude_native_workflow_snapshot(
        dir: &Path,
        run_id: &str,
    ) -> Result<Option<PathBuf>, String> {
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(format!("failed to read {}: {err}", dir.display())),
        };
        let mut best: Option<(u128, PathBuf)> = None;
        for entry in entries {
            let Ok(entry) = entry else {
                continue;
            };
            let session_dir = entry.path();
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if !metadata.is_dir()
                || session_dir.file_name().and_then(|name| name.to_str())
                    == Some(WORKFLOW_ACTIVE_RUNS_DIR)
            {
                continue;
            }
            let path = session_dir
                .join(CLAUDE_WORKFLOW_SNAPSHOT_DIR)
                .join(format!("{run_id}.json"));
            let Ok(metadata) = fs::metadata(&path) else {
                continue;
            };
            if !metadata.is_file() {
                continue;
            }
            let sort_key = metadata
                .modified()
                .ok()
                .and_then(system_time_millis)
                .unwrap_or_default();
            if best
                .as_ref()
                .is_none_or(|(best_sort_key, _)| sort_key > *best_sort_key)
            {
                best = Some((sort_key, path));
            }
        }
        Ok(best.map(|(_, path)| path))
    }

    fn workflow_resume_invocation_prompt(run: &StoredWorkflowRun) -> String {
        let run_id = run.run_id.trim();
        let mut prompt = format!(
            "Resume workflow run `{run_id}` by calling the workflow tool with `resumeFromRunId: \"{run_id}\"`. Do not run the workflow script manually; use the workflow tool so Codex can load the prior script artifact, inherit prior args when needed, and apply the completed-run cache when script hash and args still match."
        );
        if let Some(name) = non_empty_trimmed(run.workflow_name.as_str()) {
            prompt.push_str(&format!("\nPrior workflow name: {name}"));
        }
        if let Some(script_path) = run.script_path.as_deref().and_then(non_empty_trimmed) {
            prompt.push_str(&format!("\nPrior script path: {script_path}"));
        }
        if let Some(script_hash) = run.script_hash.as_deref().and_then(non_empty_trimmed) {
            prompt.push_str(&format!("\nPrior script hash: {script_hash}"));
        }
        if let Some(args) = run.args.as_ref().filter(|args| !args.is_null()) {
            let args = serde_json::to_string_pretty(args).unwrap_or_else(|_| args.to_string());
            prompt.push_str(&format!("\nPrior args:\n{args}"));
        }
        prompt
    }

    fn workflow_retry_invocation_prompt(run: &StoredWorkflowRun) -> String {
        let run_id = run.run_id.trim();
        let script_path = run
            .script_path
            .as_deref()
            .and_then(non_empty_trimmed)
            .unwrap_or("");
        let mut prompt = format!(
            "Retry workflow run `{run_id}` by calling the workflow tool with `scriptPath: \"{script_path}\"`. Do not use `resumeFromRunId`; this retry should re-execute the workflow instead of replaying a completed-run cache."
        );
        if let Some(name) = non_empty_trimmed(run.workflow_name.as_str()) {
            prompt.push_str(&format!("\nPrior workflow name: {name}"));
        }
        if let Some(script_hash) = run.script_hash.as_deref().and_then(non_empty_trimmed) {
            prompt.push_str(&format!("\nPrior script hash: {script_hash}"));
        }
        if let Some(args) = run.args.as_ref().filter(|args| !args.is_null()) {
            let args = serde_json::to_string_pretty(args).unwrap_or_else(|_| args.to_string());
            prompt.push_str(&format!("\nPrior args:\n{args}"));
        }
        prompt
    }

    fn format_workflow_run_detail(run: &StoredWorkflowRun) -> String {
        let mut message = format!(
            "Workflow run `{}`\nStatus: {}\nName: {}",
            run.run_id.trim(),
            run.status.trim(),
            run.workflow_name.trim()
        );
        if let Some(metadata_name) = run.metadata_name.as_deref().and_then(non_empty_trimmed) {
            message.push_str(&format!("\nMetadata: `{metadata_name}`"));
        }
        if let Some(description) = run.description.as_deref().and_then(non_empty_trimmed) {
            message.push_str(&format!("\nDescription: {description}"));
        }
        if let Some(input_schema) = run.input_schema.as_deref().and_then(non_empty_trimmed) {
            message.push_str(&format!("\nInput schema:\n{input_schema}"));
        }
        if let Some(cell_id) = run.cell_id.as_deref().and_then(non_empty_trimmed) {
            message.push_str(&format!("\nCell: `{cell_id}`"));
        }
        if let Some(session_id) = run.session_id.as_deref().and_then(non_empty_trimmed) {
            message.push_str(&format!("\nSession: `{session_id}`"));
        }
        if let Some(thread_id) = run.thread_id.as_deref().and_then(non_empty_trimmed) {
            message.push_str(&format!("\nThread: `{thread_id}`"));
        }
        if let Some(workflow_tool_call_id) = run
            .workflow_tool_call_id
            .as_deref()
            .and_then(non_empty_trimmed)
        {
            message.push_str(&format!("\nWorkflow tool: `{workflow_tool_call_id}`"));
        }
        if let Some(cwd) = run.cwd.as_deref().and_then(non_empty_trimmed) {
            message.push_str(&format!("\nCwd: `{cwd}`"));
        }
        if let Some(git_branch) = run.git_branch.as_deref().and_then(non_empty_trimmed) {
            message.push_str(&format!("\nBranch: `{git_branch}`"));
        }
        if let Some(duration_ms) = run.duration_ms {
            message.push_str(&format!("\nDuration: {duration_ms} ms"));
        }
        if let Some(source) = run.source.as_ref().and_then(Self::workflow_source_summary) {
            message.push_str(&format!("\nSource: {source}"));
        }
        if let Some(source_path) = run
            .source
            .as_ref()
            .and_then(|source| source.path.as_deref())
            .and_then(non_empty_trimmed)
        {
            message.push_str(&format!("\nSource path: `{source_path}`"));
        }
        if let Some(run_dir) = run.run_dir.as_deref().and_then(non_empty_trimmed) {
            message.push_str(&format!("\nRun dir: `{run_dir}`"));
            let journal_path = Path::new(run_dir).join(WORKFLOW_AGENT_JOURNAL_FILE);
            message.push_str(&format!("\nAgent journal: `{}`", journal_path.display()));
            if let Some(summary) = &run.journal_summary {
                message.push_str(&format!(
                    "\nJournal entries: {} started, {} result{}{}",
                    summary.started,
                    summary.results,
                    if summary.child_results > 0 {
                        format!(
                            ", {} child result{}",
                            summary.child_results,
                            if summary.child_results == 1 { "" } else { "s" }
                        )
                    } else {
                        String::new()
                    },
                    if summary.invalid > 0 {
                        format!(", {} invalid", summary.invalid)
                    } else {
                        String::new()
                    }
                ));
                if !summary.agents.is_empty() {
                    message.push_str(&format!("\nJournal agents ({}):", summary.agents.len()));
                    for agent in &summary.agents {
                        message.push_str(&format!("\n- {}: {}", agent.agent_id, agent.status));
                        if let Some(key) = agent.key.as_deref().and_then(non_empty_trimmed) {
                            message.push_str(&format!(" `{key}`"));
                        }
                    }
                }
                if !summary.children.is_empty() {
                    message.push_str(&format!(
                        "\nJournal child workflows ({}):",
                        summary.children.len()
                    ));
                    for child in &summary.children {
                        let label = child
                            .child_run_id
                            .as_deref()
                            .and_then(non_empty_trimmed)
                            .unwrap_or(child.child.as_str());
                        message.push_str(&format!("\n- {label}: completed"));
                        if label != child.child {
                            message.push_str(&format!(" `{}`", child.child));
                        }
                        if let Some(key) = child.key.as_deref().and_then(non_empty_trimmed) {
                            message.push_str(&format!(" `{key}`"));
                        }
                    }
                }
            }
        }
        if let Some(script_path) = run.script_path.as_deref().and_then(non_empty_trimmed) {
            message.push_str(&format!("\nScript: `{script_path}`"));
        }
        if let Some(transcript_dir) = run.transcript_dir.as_deref().and_then(non_empty_trimmed) {
            message.push_str(&format!("\nTranscripts: `{transcript_dir}`"));
        }
        if let Some(script_hash) = run.script_hash.as_deref().and_then(non_empty_trimmed) {
            message.push_str(&format!("\nScript hash: `{script_hash}`"));
        }
        if let Some(max_output_tokens) = run.max_output_tokens {
            message.push_str(&format!("\nMax output tokens: {max_output_tokens}"));
        }
        if let Some(resume_from_run_id) = run
            .resume_from_run_id
            .as_deref()
            .and_then(non_empty_trimmed)
        {
            message.push_str(&format!("\nResumed from: `{resume_from_run_id}`"));
        }
        if let Some(args) = run.args.as_ref().filter(|args| !args.is_null()) {
            let args = serde_json::to_string_pretty(args).unwrap_or_else(|_| args.to_string());
            message.push_str(&format!("\nArgs:\n{args}"));
        }
        if let Some(actions) = Self::format_workflow_run_actions(run) {
            message.push_str(&format!("\nActions: {actions}"));
        }
        if let Some(metrics) = Self::format_workflow_run_metrics(run) {
            message.push_str(&format!("\n{metrics}"));
        }
        if let Some(summary) = Self::format_workflow_progress_summary(run) {
            message.push('\n');
            message.push_str(&summary);
        }
        if let Some(details) = Self::format_workflow_agent_details(run) {
            message.push('\n');
            message.push_str(&details);
        }
        if !run.status_history.is_empty() {
            message.push_str("\nHistory:");
            for event in &run.status_history {
                message.push('\n');
                message.push_str(&Self::format_workflow_status_event_line(event));
            }
        }
        if !run.progress.is_empty() {
            message.push_str("\nProgress:");
            let start = run
                .progress
                .len()
                .saturating_sub(WORKFLOW_RUN_DETAIL_PROGRESS_LIMIT);
            for event in &run.progress[start..] {
                message.push('\n');
                message.push_str(&Self::format_workflow_progress_event_line(event));
            }
        }
        if let Some(error) = run.error.as_deref().and_then(non_empty_trimmed) {
            message.push_str(&format!("\nError:\n{error}"));
        } else if let Some(output) = run.output_preview.as_deref().and_then(non_empty_trimmed) {
            message.push_str(&format!("\nOutput:\n{output}"));
        }
        message
    }

    fn format_workflow_run_actions(run: &StoredWorkflowRun) -> Option<String> {
        let run_id = run.run_id.trim();
        if run_id.is_empty() {
            return None;
        }

        let status = run.status.trim();
        let has_script = run
            .script_path
            .as_deref()
            .and_then(non_empty_trimmed)
            .is_some();
        let mut actions = vec![format!("detail `/workflows {run_id}`")];
        if status == "running" && run.cell_id.as_deref().and_then(non_empty_trimmed).is_some() {
            actions.push(format!("pause `/workflows pause {run_id}`"));
        }
        if status == "paused" && run.cell_id.as_deref().and_then(non_empty_trimmed).is_some() {
            actions.push(format!("continue `/workflows continue {run_id}`"));
        }
        if workflow_status_is_active(status)
            && run.cell_id.as_deref().and_then(non_empty_trimmed).is_some()
        {
            actions.push(format!("cancel `/workflows cancel {run_id}`"));
        }
        if status == "running"
            && let Some(agent_id) =
                Self::workflow_run_first_running_agent_id(run.progress.as_slice())
        {
            actions.push(format!(
                "interrupt-agent `/workflows interrupt-agent {run_id} {agent_id}`"
            ));
            actions.push(format!(
                "skip-agent `/workflows skip-agent {run_id} {agent_id}`"
            ));
            actions.push(format!(
                "retry-agent `/workflows retry-agent {run_id} {agent_id}`"
            ));
            actions.push(format!(
                "restart-agent `/workflows restart-agent {run_id} {agent_id}`"
            ));
        }
        if status == "completed" && has_script {
            actions.push(format!("resume `/workflows resume {run_id}`"));
        }
        if has_script {
            actions.push(format!("retry `/workflows retry {run_id}`"));
            actions.push(format!("save `/workflows save {run_id} <name>`"));
        }
        Some(actions.join("; "))
    }

    fn workflow_run_first_running_agent_id(
        progress: &[StoredWorkflowProgressEvent],
    ) -> Option<String> {
        let mut seen = BTreeSet::new();
        for event in progress.iter().rev() {
            let Some(agent_id) = event.agent_id.as_deref().and_then(non_empty_trimmed) else {
                continue;
            };
            if !seen.insert(agent_id.to_string()) {
                continue;
            }
            if Self::workflow_progress_summary_status(event, "agent") == "running" {
                return Some(agent_id.to_string());
            }
        }
        None
    }

    fn workflow_run_agent_status(
        progress: &[StoredWorkflowProgressEvent],
        agent_id: &str,
    ) -> Option<String> {
        let agent_id = non_empty_trimmed(agent_id)?;
        progress.iter().rev().find_map(|event| {
            let event_agent_id = event.agent_id.as_deref().and_then(non_empty_trimmed)?;
            (event_agent_id == agent_id)
                .then(|| Self::workflow_progress_summary_status(event, "agent"))
        })
    }

    fn format_workflow_run_metrics(run: &StoredWorkflowRun) -> Option<String> {
        let metrics = Self::workflow_run_metrics(run)?;
        let mut parts = Vec::new();
        if let Some(count) = metrics.agent_count {
            parts.push(Self::format_count(count, "agent", "agents"));
        }
        if let Some(count) = metrics.child_count {
            parts.push(Self::format_count(
                count,
                "child workflow",
                "child workflows",
            ));
        }
        if let Some(count) = metrics.log_count {
            let mut logs = Self::format_count(count, "log", "logs");
            if metrics.log_suppressed {
                logs.push_str(" (suppressed)");
            }
            parts.push(logs);
        }
        if let Some(count) = metrics.failure_count.filter(|count| *count > 0) {
            parts.push(Self::format_count(count, "failure", "failures"));
        }
        if parts.is_empty() {
            return None;
        }
        Some(format!("Run metrics: {}", parts.join("; ")))
    }

    fn workflow_run_metrics(run: &StoredWorkflowRun) -> Option<WorkflowRunMetrics> {
        let terminal = run.progress.iter().rev().find(|event| {
            matches!(
                event.event.as_str(),
                "workflow_complete" | "workflow_completed" | "workflow_failed" | "workflow_error"
            )
        });
        let data = terminal.and_then(|event| event.data.as_ref());
        let failure_count = Self::workflow_metric_u64(data, "failure_count", "failureCount")
            .or_else(|| {
                let count = run
                    .progress
                    .iter()
                    .filter(|event| Self::workflow_progress_is_failure(event))
                    .count() as u64;
                (count > 0).then_some(count)
            });
        let metrics = WorkflowRunMetrics {
            agent_count: Self::workflow_metric_u64(data, "agent_count", "agentCount"),
            child_count: Self::workflow_metric_u64(data, "child_count", "childCount"),
            log_count: Self::workflow_metric_u64(data, "log_count", "logCount"),
            log_suppressed: Self::workflow_metric_bool(data, "log_suppressed", "logSuppressed")
                .unwrap_or(false),
            failure_count,
        };
        if metrics.agent_count.is_some()
            || metrics.child_count.is_some()
            || metrics.log_count.is_some()
            || metrics.failure_count.is_some()
        {
            Some(metrics)
        } else {
            None
        }
    }

    fn workflow_metric_u64(
        data: Option<&serde_json::Value>,
        snake_key: &str,
        camel_key: &str,
    ) -> Option<u64> {
        data.and_then(|data| data.get(snake_key).or_else(|| data.get(camel_key)))
            .and_then(serde_json::Value::as_u64)
    }

    fn workflow_metric_bool(
        data: Option<&serde_json::Value>,
        snake_key: &str,
        camel_key: &str,
    ) -> Option<bool> {
        data.and_then(|data| data.get(snake_key).or_else(|| data.get(camel_key)))
            .and_then(serde_json::Value::as_bool)
    }

    fn workflow_progress_is_failure(event: &StoredWorkflowProgressEvent) -> bool {
        matches!(
            event.event.as_str(),
            "workflow_failed"
                | "workflow_error"
                | "agent_failed"
                | "child_failed"
                | "parallel_failed"
                | "pipeline_failed"
        ) || matches!(
            event.state.as_deref().and_then(non_empty_trimmed),
            Some("error" | "failed")
        )
    }

    fn format_count(count: u64, singular: &str, plural: &str) -> String {
        if count == 1 {
            format!("{count} {singular}")
        } else {
            format!("{count} {plural}")
        }
    }

    fn format_workflow_progress_summary(run: &StoredWorkflowRun) -> Option<String> {
        if run.progress.is_empty() {
            return None;
        }
        let mut phases = Vec::<WorkflowProgressSummaryItem>::new();
        let mut agents = Vec::<WorkflowProgressSummaryItem>::new();
        let mut children = Vec::<WorkflowProgressSummaryItem>::new();
        for event in &run.progress {
            if matches!(event.event.as_str(), "phase" | "workflow_phase")
                && let Some(phase) = event.phase.as_deref().and_then(non_empty_trimmed)
            {
                Self::upsert_workflow_summary_item(
                    &mut phases,
                    phase,
                    Self::workflow_progress_summary_status(event, "phase"),
                    event,
                );
            }
            if let Some(agent) = Self::workflow_progress_agent_name(event) {
                Self::upsert_workflow_summary_item(
                    &mut agents,
                    agent.as_str(),
                    Self::workflow_progress_summary_status(event, "agent"),
                    event,
                );
            }
            if let Some(child) = event.child.as_deref().and_then(non_empty_trimmed) {
                Self::upsert_workflow_summary_item(
                    &mut children,
                    Self::workflow_child_summary_name(event, child).as_str(),
                    Self::workflow_progress_summary_status(event, "child"),
                    event,
                );
            }
        }
        if phases.is_empty() && agents.is_empty() && children.is_empty() {
            return None;
        }

        let mut lines = vec!["Summary:".to_string()];
        Self::append_workflow_summary_section(&mut lines, "Phases", &phases);
        Self::append_workflow_summary_section(&mut lines, "Agents", &agents);
        Self::append_workflow_summary_section(&mut lines, "Child workflows", &children);
        Some(lines.join("\n"))
    }

    fn format_workflow_agent_details(run: &StoredWorkflowRun) -> Option<String> {
        let mut items = Vec::<WorkflowAgentDetailItem>::new();
        for event in &run.progress {
            let Some(agent_name) = Self::workflow_progress_agent_name(event) else {
                continue;
            };
            let agent_id = event
                .agent_id
                .as_deref()
                .and_then(non_empty_trimmed)
                .map(ToString::to_string);
            let key = agent_id.clone().unwrap_or_else(|| agent_name.clone());
            let status = Self::workflow_progress_summary_status(event, "agent");
            let existing = items.iter().position(|item| item.key == key);
            let item = if let Some(existing) = existing {
                &mut items[existing]
            } else {
                items.push(WorkflowAgentDetailItem {
                    key,
                    name: agent_name.clone(),
                    agent_id: agent_id.clone(),
                    phase: None,
                    status: status.clone(),
                    unix_ms: None,
                    index: event.index.filter(|index| *index > 0),
                    prompt_preview: None,
                    result_preview: None,
                    error: None,
                });
                let index = items.len() - 1;
                &mut items[index]
            };
            if let Some(label) = event.agent.as_deref().and_then(non_empty_trimmed) {
                item.name = label.to_string();
            }
            if item.agent_id.is_none() {
                item.agent_id = agent_id;
            }
            if item.index.is_none() {
                item.index = event.index.filter(|index| *index > 0);
            }
            if let Some(phase) = event.phase.as_deref().and_then(non_empty_trimmed) {
                item.phase = Some(phase.to_string());
            }
            item.status = status;
            if event.unix_ms.is_some() {
                item.unix_ms = event.unix_ms;
            }
            if let Some(prompt) = event.prompt_preview.as_deref().and_then(non_empty_trimmed) {
                item.prompt_preview = Some(compact_workflow_run_preview(prompt));
            }
            if let Some(result) = event.result_preview.as_deref().and_then(non_empty_trimmed) {
                item.result_preview = Some(compact_workflow_run_preview(result));
            }
            if let Some(error) = Self::workflow_progress_error(event) {
                item.error = Some(error);
            }
        }
        if items.is_empty() {
            return None;
        }
        let mut lines = vec![format!("Agent details ({}):", items.len())];
        for item in items {
            let mut heading = format!("- {}", item.name);
            let mut tags = Vec::new();
            if let Some(index) = item.index {
                tags.push(format!("#{index}"));
            }
            if let Some(agent_id) = item.agent_id.as_deref().and_then(non_empty_trimmed) {
                tags.push(agent_id.to_string());
            }
            if !tags.is_empty() {
                heading.push_str(&format!(" ({}): {}", tags.join(", "), item.status));
            } else {
                heading.push_str(&format!(": {}", item.status));
            }
            if let Some(unix_ms) = item.unix_ms {
                heading.push_str(&format!(" at {unix_ms}"));
            }
            lines.push(heading);
            if let Some(phase) = item.phase.as_deref().and_then(non_empty_trimmed) {
                lines.push(format!("  phase: {phase}"));
            }
            if let Some(prompt) = item.prompt_preview.as_deref().and_then(non_empty_trimmed) {
                lines.push(format!("  prompt: {prompt}"));
            }
            if let Some(result) = item.result_preview.as_deref().and_then(non_empty_trimmed) {
                lines.push(format!("  result: {result}"));
            }
            if let Some(error) = item.error.as_deref().and_then(non_empty_trimmed) {
                lines.push(format!("  error: {error}"));
            }
            if let Some(metadata) = item
                .agent_id
                .as_deref()
                .and_then(|agent_id| Self::load_workflow_agent_metadata(run, agent_id))
                && !metadata.fields.is_empty()
            {
                let fields = metadata
                    .fields
                    .iter()
                    .map(|(label, value)| format!("{label} `{value}`"))
                    .collect::<Vec<_>>()
                    .join("; ");
                lines.push(format!("  metadata: {fields}"));
            }
            if let Some(transcript) = item
                .agent_id
                .as_deref()
                .and_then(|agent_id| Self::load_workflow_agent_transcript(run, agent_id))
            {
                if let Some(prompt) = transcript.prompt.as_deref().and_then(non_empty_trimmed)
                    && item.prompt_preview.as_deref() != Some(prompt)
                {
                    lines.push(format!("  transcript prompt: {prompt}"));
                }
                if !transcript.reasoning.is_empty() {
                    let mut reasoning = transcript
                        .reasoning
                        .iter()
                        .take(WORKFLOW_AGENT_TRANSCRIPT_TOOL_CALL_DISPLAY_LIMIT)
                        .cloned()
                        .collect::<Vec<_>>();
                    let remaining = transcript
                        .reasoning
                        .len()
                        .saturating_sub(WORKFLOW_AGENT_TRANSCRIPT_TOOL_CALL_DISPLAY_LIMIT);
                    if remaining > 0 {
                        reasoning.push(format!("... {remaining} more"));
                    }
                    lines.push(format!("  transcript reasoning: {}", reasoning.join("; ")));
                }
                if !transcript.tool_calls.is_empty() {
                    let mut calls = transcript
                        .tool_calls
                        .iter()
                        .take(WORKFLOW_AGENT_TRANSCRIPT_TOOL_CALL_DISPLAY_LIMIT)
                        .map(|call| {
                            let mut text = call.name.clone();
                            if let Some(summary) =
                                call.summary.as_deref().and_then(non_empty_trimmed)
                            {
                                text.push(' ');
                                text.push_str(summary);
                            }
                            if let Some(output) = call.output.as_deref().and_then(non_empty_trimmed)
                            {
                                text.push_str(" => ");
                                text.push_str(output);
                            }
                            text
                        })
                        .collect::<Vec<_>>();
                    let remaining = transcript
                        .tool_calls
                        .len()
                        .saturating_sub(WORKFLOW_AGENT_TRANSCRIPT_TOOL_CALL_DISPLAY_LIMIT);
                    if remaining > 0 {
                        calls.push(format!("... {remaining} more"));
                    }
                    lines.push(format!("  activity: {}", calls.join("; ")));
                }
                if let Some(final_text) =
                    transcript.final_text.as_deref().and_then(non_empty_trimmed)
                    && item.result_preview.as_deref() != Some(final_text)
                {
                    lines.push(format!("  transcript final: {final_text}"));
                }
                if transcript.invalid > 0 {
                    lines.push(format!(
                        "  transcript skipped: {} invalid lines",
                        transcript.invalid
                    ));
                }
            }
        }
        Some(lines.join("\n"))
    }

    fn upsert_workflow_summary_item(
        items: &mut Vec<WorkflowProgressSummaryItem>,
        name: &str,
        status: String,
        event: &StoredWorkflowProgressEvent,
    ) {
        let item = WorkflowProgressSummaryItem {
            name: name.to_string(),
            status,
            unix_ms: event.unix_ms,
            message: Self::workflow_progress_message(event),
        };
        if let Some(existing) = items.iter_mut().find(|existing| existing.name == item.name) {
            *existing = item;
        } else {
            items.push(item);
        }
    }

    fn workflow_progress_summary_status(event: &StoredWorkflowProgressEvent, kind: &str) -> String {
        let event_name = event.event.as_str();
        match event_name {
            "workflow_agent" => match event.state.as_deref().and_then(non_empty_trimmed) {
                Some("start" | "progress" | "queued" | "running") => "running".to_string(),
                Some("done" | "completed") => "completed".to_string(),
                Some("error" | "failed") => "failed".to_string(),
                Some("skipped") => "skipped".to_string(),
                Some(other) => other.replace('_', " "),
                None => "agent".to_string(),
            },
            "workflow_phase" => "reached".to_string(),
            "agent_start" | "agent_waiting" | "child_start" => "running".to_string(),
            "agent_complete" | "child_complete" => "completed".to_string(),
            "agent_failed" | "child_failed" => "failed".to_string(),
            "agent_stalled" => "stalled".to_string(),
            "phase" => "reached".to_string(),
            other => other
                .strip_prefix(kind)
                .and_then(|suffix| suffix.strip_prefix('_'))
                .unwrap_or(other)
                .replace('_', " "),
        }
    }

    fn append_workflow_summary_section(
        lines: &mut Vec<String>,
        title: &str,
        items: &[WorkflowProgressSummaryItem],
    ) {
        if items.is_empty() {
            return;
        }
        lines.push(format!("{title} ({}):", items.len()));
        for item in items {
            let mut line = format!("- {}: {}", item.name, item.status);
            if let Some(unix_ms) = item.unix_ms {
                line.push_str(&format!(" at {unix_ms}"));
            }
            if let Some(message) = item.message.as_deref().and_then(non_empty_trimmed) {
                line.push_str(&format!(" - {message}"));
            }
            lines.push(line);
        }
    }

    fn format_workflow_status_event_line(event: &StoredWorkflowStatusEvent) -> String {
        let label = event
            .status
            .as_deref()
            .and_then(non_empty_trimmed)
            .or_else(|| non_empty_trimmed(event.event.as_str()))
            .unwrap_or("event");
        let mut line = format!("- {label}");
        if let Some(unix_ms) = event.unix_ms {
            line.push_str(&format!(" at {unix_ms}"));
        }
        if let Some(message) = event
            .message
            .as_deref()
            .and_then(non_empty_trimmed)
            .map(compact_workflow_run_preview)
        {
            line.push_str(&format!(" - {message}"));
        }
        line
    }

    fn format_workflow_progress_event_line(event: &StoredWorkflowProgressEvent) -> String {
        let mut line = format!("- {}", Self::format_workflow_progress_event_inline(event));
        if let Some(unix_ms) = event.unix_ms {
            line.push_str(&format!(" at {unix_ms}"));
        }
        line
    }

    fn format_workflow_progress_event_inline(event: &StoredWorkflowProgressEvent) -> String {
        let label = non_empty_trimmed(event.event.as_str()).unwrap_or("event");
        let mut parts = vec![label.to_string()];
        if let Some(workflow) = event.workflow.as_deref().and_then(non_empty_trimmed) {
            parts.push(format!("workflow `{workflow}`"));
        }
        if let Some(index) = event.index.filter(|index| *index > 0) {
            parts.push(format!("#{index}"));
        }
        if let Some(phase) = event.phase.as_deref().and_then(non_empty_trimmed) {
            parts.push(format!("phase `{phase}`"));
        }
        if let Some(agent) = Self::workflow_progress_agent_name(event) {
            parts.push(format!("agent `{agent}`"));
        }
        if let Some(state) = event.state.as_deref().and_then(non_empty_trimmed) {
            parts.push(format!("state `{state}`"));
        }
        if let Some(child) = event.child.as_deref().and_then(non_empty_trimmed) {
            parts.push(format!("child `{child}`"));
        }
        if let Some(child_run_id) = Self::workflow_progress_child_run_id(event) {
            parts.push(format!("run `{child_run_id}`"));
        } else if let Some(child_index) =
            Self::workflow_progress_u64(event, "child_index", "childIndex")
        {
            parts.push(format!("child #{child_index}"));
        }
        if let Some(step_index) = Self::workflow_progress_u64(event, "step_index", "stepIndex") {
            parts.push(format!("step {step_index}"));
        }
        if let Some(item_index) = Self::workflow_progress_u64(event, "item_index", "itemIndex") {
            parts.push(format!("item {item_index}"));
        }
        if let Some(stage_index) = Self::workflow_progress_u64(event, "stage_index", "stageIndex") {
            parts.push(format!("stage {stage_index}"));
        }
        let mut line = parts.join(" ");
        let message = Self::workflow_progress_message(event);
        if let Some(message) = message.as_deref() {
            line.push_str(&format!(" - {message}"));
        }
        if let Some(error) = Self::workflow_progress_error(event) {
            let already_in_message = message
                .as_deref()
                .is_some_and(|message| message.contains(&error));
            if !already_in_message {
                line.push_str(&format!(" (error: {error})"));
            }
        }
        line
    }

    fn workflow_progress_u64(
        event: &StoredWorkflowProgressEvent,
        snake_key: &str,
        camel_key: &str,
    ) -> Option<u64> {
        let direct = match snake_key {
            "child_index" => event.child_index,
            "item_index" => event.item_index,
            "stage_index" => event.stage_index,
            "step_index" => event.step_index,
            _ => None,
        };
        direct.filter(|value| *value > 0).or_else(|| {
            event
                .data
                .as_ref()
                .and_then(|data| data.get(snake_key).or_else(|| data.get(camel_key)))
                .and_then(serde_json::Value::as_u64)
                .filter(|value| *value > 0)
        })
    }

    fn workflow_child_summary_name(event: &StoredWorkflowProgressEvent, child: &str) -> String {
        Self::workflow_progress_child_run_id(event).unwrap_or_else(|| {
            Self::workflow_progress_u64(event, "child_index", "childIndex")
                .map(|index| format!("{child}#{index}"))
                .unwrap_or_else(|| child.to_string())
        })
    }

    fn workflow_progress_child_run_id(event: &StoredWorkflowProgressEvent) -> Option<String> {
        event
            .child_run_id
            .as_deref()
            .and_then(non_empty_trimmed)
            .or_else(|| {
                event
                    .data
                    .as_ref()
                    .and_then(|data| {
                        data.get("child_run_id")
                            .or_else(|| data.get("childRunId"))
                            .and_then(serde_json::Value::as_str)
                    })
                    .and_then(non_empty_trimmed)
            })
            .map(ToString::to_string)
    }

    fn workflow_progress_message(event: &StoredWorkflowProgressEvent) -> Option<String> {
        event
            .message
            .as_deref()
            .and_then(non_empty_trimmed)
            .or_else(|| event.result_preview.as_deref().and_then(non_empty_trimmed))
            .or_else(|| event.prompt_preview.as_deref().and_then(non_empty_trimmed))
            .map(compact_workflow_run_preview)
    }

    fn workflow_progress_agent_name(event: &StoredWorkflowProgressEvent) -> Option<String> {
        event
            .agent
            .as_deref()
            .and_then(non_empty_trimmed)
            .or_else(|| event.agent_id.as_deref().and_then(non_empty_trimmed))
            .map(ToString::to_string)
    }

    fn workflow_progress_error(event: &StoredWorkflowProgressEvent) -> Option<String> {
        event
            .error
            .as_deref()
            .and_then(non_empty_trimmed)
            .or_else(|| {
                event
                    .data
                    .as_ref()
                    .and_then(|data| data.get("error"))
                    .and_then(serde_json::Value::as_str)
                    .and_then(non_empty_trimmed)
            })
            .map(compact_workflow_run_preview)
    }
}
